# Soroban Proxy Oracle — Operational Runbook

Deploy, configure, monitor, respond to incidents, upgrade, and roll back.
Companion docs: `README.md` (overview), `PARITY.md` (NEAR parity), `AUDIT.md`
(boundary).

## Conventions

Examples assume these are exported, and use the `inv` helper for brevity:

```bash
export NET=<network> SRC=<identity>                 # network + signing identity
export RT=<runtime_id> GOV=<governance_id> AD=<adapter_id> LZ=<lazer_source_id> BATCH=<batcher_id>
JF=contract/proxy-oracle/soroban/justfile
inv() { stellar contract invoke --network "$NET" --source "$SRC" "$@"; }
```

So `inv --id "$GOV" -- next_proposal_id` runs a view. Never put key material in
scripts or logs.

## 1. Build and release gates

```bash
just -f $JF release-gate   # test + optimize + size-check + budget-check
just -f $JF release        # write target/proxy-oracle-soroban/release-manifest.json
just -f $JF dry-run        # validate artifacts (SHA-256 + size), no broadcast
```

Size budgets: runtime & governance ≤ 131072 bytes (128 KiB); adapter, Lazer
source, and batcher ≤ 32768 (32 KiB), enforced by `size-check`. The manifest records git commit, Stellar CLI
and toolchain versions, SHA-256 checksums, and optimized sizes — cross-check the
SHA-256 against the on-chain hash after install.

## 1b. Live end-to-end rehearsal

`scripts/e2e_live.sh` drives the whole stack against the real Reflector,
RedStone and Pyth Lazer contracts, resumably, recording every id and tx hash
under `target/proxy-oracle-soroban/e2e/<net>/`:

```bash
NET=testnet SRC=<identity> PYTH_LAZER_API_KEY=<key> \
  contract/proxy-oracle/soroban/scripts/e2e_live.sh all      # or: deploy|configure|push|refresh
```

Mainnet needs `ALLOW_MAINNET_WRITE=1` and bids `FEE_STROOPS` (default 0.1 XLM).
Size `MAX_AGE_SECS` to the slowest source on that network (RedStone testnet
XLM lags ~30 min; mainnet ~3 min).

## 2. Deploy

Install each optimized WASM and record the returned hash:

```bash
stellar contract install --network $NET --source $SRC \
  --wasm target/proxy-oracle-soroban/wasm/<artifact>.optimized.wasm
```

for `templar_proxy_oracle_soroban_contract`, `..._governance_contract`,
`..._sep40_adapter_contract`, `..._pyth_lazer_source_contract`, and
`..._batcher_contract`.

## 3. Initialize

Constructors are one-shot (`AlreadyInitialized` on re-call). The runtime takes
its owner and governance takes the runtime, so deploy the runtime with a
bootstrap owner first and hand it to governance once governance exists:

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <RUNTIME_HASH> -- \
  --governance <ADMIN> --base '{"Other":"USD"}'          # bootstrap owner = ADMIN
stellar contract deploy --network $NET --source $SRC --wasm-hash <GOV_HASH> -- \
  --admin <ADMIN> --proxy_oracle $RT \
  --initial_uniform_ttl_ns 86400000000000                # 24h, uniform across all OperationKinds
```

- `base` is the source-validation invariant — every source's `base()` must match
  it. Per-feed `decimals`/`resolution` are adapter-side, not here.
- Tune per-kind maturity later with `SetActionTtl`.

Hand the runtime's owner to governance via the two-step `Ownable` transfer:

```bash
inv --id $RT -- transfer_ownership --new_owner $GOV --live_until_ledger <MAX_TTL_LEDGER>
# $GOV is now the pending owner. Finalize through governance — it dispatches
# accept_ownership on the runtime; every later owner-only call goes the same way.
inv --id $GOV -- create_proposal --caller <ADMIN> --id <NEXT> --requested_ttl 0 \
  --operation '"AcceptOwnership"'                # Admin-only; void action, no payload
inv --id $GOV -- execute_proposal --caller <ADMIN> --id <ID>
inv --id $RT -- get_owner          # verify == $GOV
inv --id $GOV -- proxy_oracle      # verify == $RT
```

Deploy one adapter per feed (`decimals ≤ 18`, `resolution ≠ 0`):

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <ADAPTER_HASH> -- \
  --owner <OWNER> --parent_oracle $RT --asset '{"Other":"BTC"}' \
  --decimals 8 --resolution 1 --base '{"Other":"USD"}'
```

Deploy the Pyth Lazer source once, mapping every Lazer feed id to the asset key
the proxy config will use for it (mainnet verifier
`CACZ3GBAKUPIAFRILUFO27J5RUH5GJ2VSJ46LP6GJYSKGDRTQ5MS3HCH`, testnet
`CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52`; feed ids XLM 23,
USDC 7, EURC 240), and the stateless batcher with no arguments:

```bash
stellar contract deploy --network $NET --source $SRC --wasm-hash <LAZER_HASH> -- \
  --owner <OWNER> \
  --config '{"verifier":"<PYTH_VERIFIER>","base":{"Other":"USD"},"decimals":8,
             "channel":"FixedRate200ms","freshness":{"max_age_secs":120,"max_ahead_secs":5}}' \
  --feed_mappings '[{"feed_id":23,"asset":{"Other":"XLM"}},{"feed_id":7,"asset":{"Other":"USDC"}}]'
stellar contract deploy --network $NET --source $SRC --wasm-hash <BATCHER_HASH>
```

## 4. Governance proposals

Every config, role, ownership, and upgrade change is a governance proposal; the
owner (governance) authorizes the resulting runtime call. Lifecycle:

```bash
inv --id $GOV -- create_proposal --caller <ADDR> --id <NEXT> \
    --operation '<ACTION_JSON>' --requested_ttl 0
inv --id $GOV -- execute_proposal --caller <ADDR> --id <ID>   # after maturity
inv --id $GOV -- cancel_proposal  --caller <ADDR> --id <ID>   # frees a slot
```

- `id` must equal `next_proposal_id`. `requested_ttl 0` uses the configured
  per-kind minimum; the effective TTL is `max(requested, minimum)`.
- A 64-pending cap returns `InvalidInput`; `execute` before maturity returns
  `ProposalNotMature`. Execution is by id — no FIFO ordering.
- Queries: `next_proposal_id`, `active_ids`, `get_proposal --id`,
  `get_operation_ttl --kind`, `get_effective_proposal_ttl --operation --requested_ttl`.

`<ACTION_JSON>` is a `GovernanceAction` variant. Its `action_code` appears in the
`ProposalSubmitted` event; Admin overrides every role.

| code | action | required role |
|------|--------|---------------|
| 1 | `SetProxy(asset, config)` | ProxyConfigurationManager |
| 2 | `RemoveProxy(asset)` | ProxyConfigurationManager |
| 3 | `ConfigureBreakers(asset, sample_interval_secs, history_len)` | ProxyConfigurationManager |
| 4 | `AddBreaker(asset, config)` | ProxyConfigurationManager |
| 5 | `RemoveBreaker(asset, breaker_id)` | ProxyConfigurationManager |
| 6 | `RenounceOwnership` | Admin |
| 7 | `SetManualTrip(asset, tripped, metadata)` | ManualTripper |
| 8 | `AcceptOwnership` | Admin |
| 9 | `TransferOwnership(new_owner)` | Admin |
| 10 | `SetActionTtl(kind, new_ttl_ns)` | ProxyConfigurationManager |
| 11 | `SetRole(account, role, set)` | Admin |
| 12 | `Upgrade(new_wasm_hash)` | Admin |
| 13 | `Rearm(asset, breaker_id, config)` | CircuitBreakerOperator |
| 14 | `SetEnforced(asset, breaker_id, config)` | CircuitBreakerOperator |

## 5. Configure sources, breakers, and roles

Wrap each action in `create_proposal` + `execute_proposal` (examples show only
the `--action` JSON).

**Sources** — `SetProxy` / `RemoveProxy`:

```json
{"SetProxy": [{"Other":"XLM"}, {
  "sources": [{"oracle":"CAFJZQWSED6YAWZU3GWRTOCNPPCGBN32L7QV43XX5LZLFTK6JLN34DLN","asset":{"Other":"XLM"}},
              {"oracle":"CBMGLKUQZVSAIL5CPDDAWSUY7MAKXISHMOZEVLMBUWBMFGHRJSR4WYRF","asset":{"Stellar":"CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA"}},
              {"oracle":"<LZ>","asset":{"Other":"XLM"}}],
  "min_sources": 3, "max_age_secs": 600, "max_clock_drift_secs": 60 }]}
```

3–16 sources; `min_sources ∈ [3, n]`; no duplicate oracle address; both
freshness bounds are required. `max_age_secs` is capped at 604800 (seven days)
and `max_clock_drift_secs` at 3600 (one hour). The asset key is per source
(Reflector keys by symbol, RedStone by SAC address). Size `max_age_secs` to the
slowest source: Reflector buckets `lastprice` timestamps to its 300s
resolution, so anything ≤ 300 drops it on most refreshes; RedStone updates on
0.2% deviation or a 12h heartbeat, so pegged assets (USDC, EURC) need ~46800
and rely on breakers instead. With three sources and `min_sources = 3`, one
stale source fails the refresh and the feed goes dark until the next accepted
one. Changing the source set or quorum
clears the breaker set, history, and cache; configure and add breakers again after
the source migration. `RemoveProxy` clears all state for the asset.

**Breakers** — configure the set, add a breaker, then enforce it:

```json
{"ConfigureBreakers": [{"Other":"BTC"}, 60, 16]}                          // sample_interval_secs, history_len (1–32)
{"AddBreaker": [{"Other":"BTC"}, {"StepwiseChange": {"max_relative_change":"<hex>"}}]}
{"SetEnforced": [{"Other":"BTC"}, <BREAKER_ID>, {"is_enforced": true}]}
{"Rearm": [{"Other":"BTC"}, <BREAKER_ID>, {"arming_delay_secs": 3600}]}
```

Kinds: `StepwiseChange` (1, sudden jumps), `MonotonicRun` (2, staged ramps),
`WindowedChangeDelta` (3, window-mean drift), and `CumulativeChange` (4,
change from its immutable accepted baseline). Unenforced breakers still evaluate
but a trip does not block the feed. `history_len` must cover every installed
rule; undersized configurations are rejected. `MonotonicRun` requires
`sample_interval_secs = 0` so every accepted step is evaluated. Rearm preserves
both shared histories. To recover any breaker after a sustained legitimate move,
disable enforcement, refresh an accepted price, rearm, then re-enable enforcement.
To intentionally establish a new cumulative baseline, remove the breaker,
complete a successful refresh, then add it again. Every persisted breaker set is
semantically valid. An invalid stored set is unreachable; recover from genuine
corruption with `remove_proxy` followed by `set_proxy`.

**Roles** — `SetRole(account, role, set)` for `Admin`, `ManualTripper`,
`CircuitBreakerOperator`, `ProxyConfigurationManager`. The last `Admin` cannot be
revoked. Inspect on governance: `has_role --account --role`, `list_role --role`,
`get_roles --account`.

## 6. Refresh cadence

`refresh` is the only path that reads source contracts; all other reads are
storage-only.

```bash
inv --id $RT -- refresh --asset '{"Other":"BTC"}'
# or every asset in one operation:
inv --id $BATCH -- refresh_many --oracle $RT --assets '[{"Other":"XLM"},{"Other":"USDC"}]'
```

A Lazer-backed proxy needs the source fed first: fetch a `leEcdsa`-format update
covering every mapped feed (one subscription, one payload) and push it with
`inv --id $LZ -- update_price_feeds --payload <hex>`; the return value is the
number of feeds stored (0 means nothing advanced).

Returns `RefreshStatus`: `Accepted` (cache updated), `Blocked` (breaker),
`ResolveFailed` (aggregation or internal storage state), `SourceUnavailable` (no
source responded), or `UnknownAsset`. Every candidate is evaluated by breakers
before its publication time determines whether it advances source-time storage. A
successful candidate with an equal or predating timestamp leaves
`aggregated_latest` and `aggregated_history` at their newest source-time
aggregate; a failed or blocked refresh replaces the cache and fails closed. Use
`aggregated_latest` for the current aggregate and `aggregated_history` for
monotonic audit samples. Historical reads are unavailable while the asset is
manually or automatically blocked, but they do not apply a freshness cutoff.

## 7. TTL extension

```bash
inv --id $RT  -- extend_ttl --asset '{"Other":"BTC"}'
inv --id $GOV -- extend_ttl
# or batched:
inv --id $BATCH -- extend_ttl_many --oracle $RT --assets '[{"Other":"XLM"},{"Other":"USDC"}]'
inv --id $BATCH -- extend_ttl_contracts --contracts '["'$GOV'","'$AD'","'$LZ'"]'
```
Every TTL entrypoint is permissionless: the invoker pays the transaction fee, but no role or authorization is required. The batcher also renews each target's (and its own) instance and code entries, so the WASM code cannot be archived out from under live instances.

The runtime accepts only registered assets. Once the proxy exists, it renews every
surviving per-asset key independently; a missing cache, history, breaker set, or
asset registry does not prevent maintenance.

## 8. Monitoring events

Compact typed events. Topics are indexed; alert on anything unexpected.

**Runtime**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `RefreshSuccess` | asset | mantissa, expo, timestamp | source-time price accepted for cache/history |
| `RefreshEvaluated` | asset | mantissa, expo, timestamp | candidate accepted by breakers but not advanced; paired with the served `RefreshSuccess` |
| `RefreshFailure` | asset | code | failed refresh — 1 aggregation/quorum, 3 internal storage state, 5 all sources down, 6 unknown asset |
| `CacheBlocked` | asset | reason_code | valid price blocked — 1 manual, 2 automatic breaker |
| `CircuitBreakerConfigSet` | asset | sample_interval_secs, history_len | breaker set reconfigured |
| `CircuitBreakerAdded` | asset, breaker_id | breaker_kind (1/2/3/4) | breaker added |
| `CircuitBreakerRemoved` | asset, breaker_id | — | breaker removed; state cleared, cache invalidated |
| `CircuitBreakerEnforcementSet` | asset, breaker_id | is_enforced | enforcement toggled |
| `CircuitBreakerRearmed` | asset, breaker_id | armed_at_secs | breaker rearmed |
| `CircuitBreakerTripped` | asset, breaker_id | tripped_at_secs, price, expo, publish_timestamp_secs, is_enforced | automatic trip; blocks iff `is_enforced` |
| `ManualTripSet` | asset | is_manually_tripped, metadata | governed trip/untrip — correlate the operator via the governance proposal |
| `ProxySet` | asset | source_count, min_sources | source/quorum change clears breaker state, history, and cache; other config changes clear history and cache |
| `ProxyRemoved` | asset | — | proxy + all state cleared; downstream now reads `None` |
| `ContractUpgraded` | — | new_wasm_hash | runtime code swapped — high impact, verify |
| `TtlExtended` | asset | — | runtime `extend_ttl(asset)` ran |

The runtime also emits `stellar_access` ownership events on transfer / accept /
renounce.

**Governance**

| Event | Topics | Payload | Meaning / response |
|-------|--------|---------|--------------------|
| `ProposalSubmitted` | id | valid_after_ns, action_code | proposal queued; do not execute before `valid_after_ns` |
| `ProposalAccepted` | id | — | proposal executed; confirm the matching runtime event fired |
| `ProposalRevoked` | id | — | proposal cancelled without executing |
| `OwnershipTransferSubmitted` | id, new_owner | — | a `TransferOwnership` proposal exists — alert and verify `new_owner` before maturity |
| `ActionTtlSet` | — | kind, new_ttl_ns | per-kind TTL changed; a shorter TTL shrinks the catch/revoke window |
| `TtlExtended` | — | — | governance `extend_ttl` ran |

## 9. Incident — manual trip / untrip

Trip a feed when you suspect manipulation or compromise that breakers have not
caught. Trip and untrip both need the `ManualTripper` role (or `Admin`); metadata
is event-only (≤ 1024 bytes).

```bash
# trip (set false to untrip); execute after the SetManualTrip maturity delay
inv --id $GOV -- create_proposal --caller <OP> --id <NEXT> --requested_ttl 0 \
  --operation '{"SetManualTrip": [{"Other":"BTC"}, true, "<reason ≤1024B>"]}'
inv --id $GOV -- execute_proposal --caller <OP> --id <ID>
inv --id $RT  -- get_breaker_set_view --asset '{"Other":"BTC"}'   # is_manually_tripped / is_blocking
```

A trip invalidates the cache immediately; `aggregated_latest` and adapter
`lastprice` return `None` until untripped and refreshed. After untripping, call
`refresh` before downstream services resume.

## 10. Incident — source outage

`RefreshFailure code 5` = all sources down while unblocked; `code 1` = fewer
than `min_sources` positive-weight sources responded; and `code 2` = a breaker
rejected the aggregate. A pre-existing manual or enforced breaker reports
`CacheBlocked` instead. Every failure status replaces the cached result,
including source and quorum failures, so downstream reads fail closed until a
later accepted refresh.

## 11. Upgrade

Run the release gate and dry-run on the new code first; cross-check the manifest
SHA-256 against the installed artifact. Zero WASM hashes are rejected; there is
no `AdminFunctionCall`.

```bash
stellar contract install --network $NET --source $SRC --wasm <new>.optimized.wasm   # returns <HASH>

# runtime — direct (governance authorizes) or via the Upgrade proposal action:
inv --id $RT --source <gov-signer> -- upgrade --new_wasm_hash <HASH> --operator $GOV
# or: create_proposal {"Upgrade":"<HASH>"} (Admin) → execute_proposal after maturity

# adapter — owner-gated:
inv --id $AD -- upgrade --new_wasm_hash <HASH> --operator <OWNER>
```

## 12. Rollback

Roll back within the first 30 minutes if any of these appear: `RefreshFailure`
for previously-healthy assets; `aggregated_latest` / adapter `lastprice`
returning `None` for assets that had accepted prices; governance proposals
failing to submit/execute/cancel; `extend_ttl` erroring; an unexpected ownership
transfer; or an optimized WASM over budget.

Procedure: install the previous WASM (identify it by the SHA-256 in the prior
manifest), upgrade back to its hash, and verify `refresh` succeeds. Keep the
previous manifest and artifacts until the new version has been stable for 48
hours.
