# Soroban Proxy Oracle

Aggregates external SEP-40 price feeds into a normalized, exponent-form cache. A companion `Sep40Adapter` contract re-exposes the cached prices as SEP-40 `PriceFeedTrait` for downstream consumers at per-adapter `decimals` / `resolution` / `base`. `PythLazerSource` turns Pyth Lazer's stateless on-chain verifier into a SEP-40 source the runtime can pull, and `ProxyOracleBatcher` fans the permissionless `refresh` / `extend_ttl` calls across assets so a keeper needs one transaction per sweep.

The runtime is **not** itself a SEP-40 contract. It exposes:

- `refresh(asset)` — pull one asset's source prices, aggregate through `templar-proxy-oracle-kernel`, apply freshness + breakers, and write the resulting status to its cache. A failed or blocked refresh replaces an accepted cache, so readers fail closed. A candidate with a non-advancing publication timestamp is still evaluated by breakers; if accepted, the cache retains the latest source-time aggregate and `RefreshEvaluated` records the candidate when it differs. The only path that performs source IO.
- `aggregated_latest(asset) -> Option<NormalizedPrice>` — the most recently accepted source-time aggregate `{ mantissa, expo, timestamp }`, or `None` if not accepted or stale.
- `aggregated_history(asset, records)` — the last N accepted aggregates with strictly increasing publication timestamps, or `None` while a manual or enforced automatic breaker blocks the asset. It is a monotonic source-time record, not a time-bucket view of `aggregated_latest`.
- Introspection: `registered_assets`, `source_base`, `get_proxy`, `get_cached`, `get_breaker_set_view`, `get_owner`. `get_breaker_set_view` reports the live, semantically valid breaker configuration. Named to avoid colliding with SEP-40's `assets()` / `base()`: `source_base` is the validation invariant every source must report against; `registered_assets` enumerates assets with a proxy config.

Reads fail closed: `aggregated_latest` and adapter `lastprice` return `None` unless the latest cached status is accepted and still fresh. `aggregated_history` and adapter `price` / `prices` return `None` while the asset is blocked but otherwise retain historical records independent of freshness.

RedStone enters through its `RedStoneSep40` adapter (`CBMGLKUQZVSAIL5CPDDAWSUY7MAKXISHMOZEVLMBUWBMFGHRJSR4WYRF` on mainnet, assets keyed by SAC address); its published per-feed contracts are Chainlink-shaped, not SEP-40, and cannot be sources. Pyth Lazer enters through `PythLazerSource`. This proxy verifies no oracle payloads itself.

## Governance

The runtime's owner — normally the companion `templar-proxy-oracle-soroban-governance-contract` — is managed by `stellar_access::ownable` (two-step `transfer_ownership` / `accept_ownership` / `renounce_ownership`, plus `get_owner`). Every config mutation is `#[only_owner]`, so the owner must authorize it.

- **Handoff**: an Admin `TransferOwnership(new_owner)` proposal dispatches `transfer_ownership`; the new owner finalizes with `accept_ownership` (directly, or via an `AcceptOwnership` proposal on its own governance contract).
- **Renounce**: `RenounceOwnership` permanently clears the owner; every later `#[only_owner]` call then panics and the config is frozen. No undo.
- **Roles** (`stellar-access` RBAC): `Admin`, `ManualTripper`, `CircuitBreakerOperator`, `ProxyConfigurationManager`. Admin overrides any action; the last Admin cannot be removed. Emergency trips use the `SetManualTrip` action (ManualTripper role).
- **Proposals**: `create_proposal(caller, id, operation, requested_ttl)`, executed by id after maturity via `execute_proposal`; `cancel_proposal` frees a slot. At most 64 pending. Query with `active_ids`, `get_proposal`, `get_operation_ttl`, `get_effective_proposal_ttl`. Per-operation maturity (`OperationKind` / `TtlConfig`) is seeded uniform at construction and adjusted with `SetActionTtl`; `Rearm` and `SetEnforced` carry independent TTLs.
- **Upgrades**: `upgrade(new_wasm_hash, operator)` on the runtime, proposed via the Admin `Upgrade` action. NEAR's `AdminFunctionCall` arbitrary dispatch is intentionally not ported — the upgrade surface stays typed.

The proposal state machine is shared with NEAR via the `no_std` `templar-proxy-oracle-governance-kernel`; each runtime owns its own authorization, storage encoding, and events.

## Sep40Adapter

Each adapter is independently `Ownable`, binds one immutable
`(parent_oracle, asset, base, resolution)` tuple, and requires the parent's
`source_base` to equal its base at construction and before every price read. To
repoint or relabel a feed, deploy a new adapter. Owner entrypoints:

- `set_decimals(decimals)` — updates only the output precision and emits `DecimalsUpdated`; `decimals ≤ 18`.
- `decommission()` — permanently disables `price`, `prices`, and `lastprice`; call it before `renounce_ownership`.
- `extend_ttl()` — permissionless instance-storage maintenance for adapter config.
- `config() -> Option<Config>` — the full `{ parent_oracle, asset, decimals, resolution, base }`.
- `upgrade(new_wasm_hash, operator)` — owner-gated wasm swap; emits `AdapterUpgraded`.

`PriceFeedTrait` projects parent prices to the adapter precision and resolution
buckets; unrepresentable values and a parent-base mismatch fail closed. SEP-40
metadata (`contractmeta!(key = "sep", val = "40")`) is declared here, not on the
runtime. Official adapters are listed in the release manifest.

## PythLazerSource

Pyth's Lazer contract on Stellar is a stateless verifier: `verify_update(Bytes) -> Bytes`
proves a payload was signed by a trusted signer and returns it, with no replay protection,
ordering, or freshness check. `PythLazerSource` owns all of that and serves the result as
SEP-40. It is `Ownable`, binds one immutable `(verifier, base)`, and holds a `feed_id ↔ Asset`
map so a single instance can back every asset's proxy.

- `update_price_feeds(payload)` — permissionless. Verifies through the configured verifier,
  requires the configured channel and a payload timestamp inside the freshness window, caps the
  feed count, then stores every mapped feed whose per-feed publish time strictly advances (the
  anti-replay guard). Feeds without a positive price, an exponent, or a feed update timestamp, or whose feed update time falls outside the window, are skipped. Returns the
  number of feeds stored. One payload covering all subscribed feeds updates every asset.
- `lastprice(asset)` rescales the stored `(mantissa, expo)` to the contract's `decimals` and
  keeps the second-precision publish time; `resolution` is 1 and `price` / `prices` serve only
  the latest record (no history is kept).
- Owner entrypoints: `add_feed` / `remove_feed` (dropping the stored price), `set_freshness`,
  `set_decimals`, `upgrade(new_wasm_hash, operator)`. Permissionless `extend_ttl()` renews the
  instance and every stored price. Views: `config`, `feed_mappings`, `stored_price`.

The payload parser and verifier client are Pyth's own `pyth-lazer-stellar-sdk` 0.3.0, vendored
into the `Templar-Protocol/pyth-lazer-public` fork on soroban-sdk 25 (crates.io 0.3.0 requires
soroban-sdk 26.1 and therefore Rust ≥ 1.91). Swap to the crates.io release once the workspace
toolchain moves.

## ProxyOracleBatcher

Stateless, ownerless. `refresh_many(oracle, assets)`, `extend_ttl_many(oracle, assets)` and
`extend_ttl_contracts(contracts)` forward the runtime's and sibling contracts' permissionless
maintenance calls inside a single Soroban operation (Stellar allows one per transaction). A
keeper therefore needs two transactions per cycle regardless of asset count — one Lazer push,
one batch — and holds no privileged role. The TTL calls also renew every target's instance and
code entries, so no separate `stellar contract extend` step is needed.

## Operational notes

- Configure 3–16 sources; `min_sources` must be in `[3, sources.len()]`. Invalid quorum is rejected.
- `refresh(asset)` is the only source-IO path; all reads are storage-only.
- Manage breakers with the governed `add_breaker` / `remove_breaker` / `rearm` / `set_enforced`. Inert params and insufficient history are rejected; `MonotonicRun` requires zero sampling, while a `CumulativeChange` baseline is intentionally rebased only by remove → successful refresh → add. Changing an asset's source set or quorum clears its breaker set, cache, and history; configure and add breakers again after the source migration. Every persisted breaker set is semantically valid. An invalid stored set is unreachable; recover from genuine corruption with `remove_proxy` → `set_proxy`.
- Manual-trip metadata is event-only, capped at 1024 bytes, not stored in breaker state.
- Schedule an ops/keeper job for Soroban TTL maintenance; do not rely on curators to remember this manually.
- Runtime `extend_ttl(asset)` is permissionless for registered assets and renews every surviving persistent `Proxy`, `Breakers`, `Cache`, and `History` entry.
- Governance `extend_ttl()` is permissionless and renews governance instance state plus active persistent proposal bodies.
- SEP-40 adapter `extend_ttl()` is permissionless and renews adapter instance config. Adapter reads also refresh instance TTL when the remaining TTL is below threshold.
- Keep optimized WASMs within budget: runtime & governance ≤ 128 KiB; adapter, Lazer source, and batcher ≤ 32 KiB. Recheck after ABI/event changes.
- Reflector buckets `lastprice` timestamps to its 300s resolution, and RedStone's relayer only fires on 0.2% deviation or a 12h heartbeat, so pegged assets can sit hours stale. With three sources and `min_sources = 3` one filtered-out source fails the refresh, so set `max_age_secs` per asset to clear the slowest source and lean on breakers where the window is wide.

## Known limits

- Source contracts must expose the SEP-40 ABI used here. NEAR Pyth sources and NEAR price transformers are not ported.
- Soroban storage is not permanent (unlike NEAR); a missed `extend_ttl` risks eviction. Events are compact typed events, not byte-for-byte equal to NEAR's JSON events.
- Not an in-place migration target for earlier prototype storage layouts — redeploy/reinitialize or ship an explicit migration first.
- **OZ `upgradeable` not adopted**: crates.io v0.7.1 needs Rust ≥ 1.87 (`is_multiple_of`) but the toolchain pins 1.86; the 1.86-compat fork is locked to soroban-sdk 23.x, not the 25.0.1 used here. The hand-rolled `upgrade` is the stopgap until the toolchain bumps or the fork rebases — don't re-investigate without one of those.

## Verification

```bash
cargo test -p templar-proxy-oracle-kernel --features serde --lib
cargo test -p templar-proxy-oracle-soroban-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-governance-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-sep40-adapter-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-pyth-lazer-source-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-batcher-contract --features testutils
cargo test -p templar-proxy-oracle-soroban-integration-tests
just -f contract/proxy-oracle/soroban/justfile build     # unoptimized WASMs
just -f contract/proxy-oracle/soroban/justfile optimize   # optimized WASMs
```

All five contracts must build via `stellar contract build` (not plain `cargo build`): `stellar-access` enables soroban-sdk's `experimental_spec_shaking_v2`, which only resolves under the Stellar CLI (v25.2.0+).
