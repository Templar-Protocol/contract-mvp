# Stellar Vault Curator Guide

This is the operator runbook for Templar vaults on **Stellar/Soroban**. The
`tmplr-soroban-vault` CLI and its deployment manifest are the supported curator
interface for deployment, governance, allocation, withdrawal servicing,
accounting maintenance, and TTL renewal.

> **Scope**
>
> The separate NEAR vault executor is not a deployed or audited curator surface
> and is not an operations reference for this guide. A Stellar vault adapter may
> represent a route that ultimately leaves Stellar, but the vault, shares,
> governance, accounting, and curator actions described here remain on Stellar.

> **Authoritative references**
>
> - **Operator CLI:** [`tools/soroban-vault-cli/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/tools/soroban-vault-cli/README.md)
> - **Stellar runtime mechanics:** [`contract/vault/soroban/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/README.md)
> - **Vault state machine:** [`contract/vault/README.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/README.md)
> - **Stellar threat model:** [`contract/vault/soroban/STRIDE.md`](https://github.com/Templar-Protocol/contracts/blob/dev/contract/vault/soroban/STRIDE.md)

## What a curator operates

A Templar Stellar vault is a single-asset vault with ERC-4626-compatible
deposit, mint, withdraw, redeem, conversion, and limit semantics exposed through
a Soroban proxy. Depositors supply one SEP-41 asset and receive transferable
SEP-41 vault shares. Curators configure adapter-backed markets; allocators move
pooled assets between the vault's idle balance and those markets.

A deployed stack contains:

- **Vault runtime** — canonical custody, accounting, state machine, RBAC,
  withdrawal queue, and applied policy state.
- **Share token** — the SEP-41 receipt token. Only the vault can mint and burn
  shares for vault flows.
- **Governance contract** — proposal submission, per-action timelocks,
  acceptance, revocation, and irreversible abdication.
- **ERC-4626 proxy** — user-friendly deposit, mint, atomic withdraw, redeem, and
  preview methods.
- **Curator proxy** — the typed curator-facing proxy included in a full stack.
- **Adapters** — one contract per market route, such as a Blend pool adapter or
  a custodial adapter.

The runtime uses the shared `templar-vault-kernel` state machine, but all
operational calls in this guide target the Stellar contracts through
`tmplr-soroban-vault` or `stellar contract invoke`.

### Accounting model

The core invariant is:

```text
total_assets = idle_assets + external_assets
```

- `idle_assets` are underlying tokens held by the vault. They fund deposits,
  atomic exits, and queued-withdrawal payouts.
- `external_assets` are the aggregate market principals/NAV recorded from
  adapters. The vault does not create a per-user market position.
- Direct transfers of the underlying asset to the vault are reconciled as idle
  assets for existing shareholders. They are not captured by the next
  depositor.
- Adapter NAV is not live-read by every preview. Run `curator refresh-markets`
  before relying on share-rate or fee-accounting views after a route's value
  changes.

### Roles and authority

| Identity | Authority |
|---|---|
| **Governance admin** | Submits and accepts governance actions. This is the address passed as `--admin` and may be a Stellar account or a contract/multisig. |
| **Vault curator** | Runtime policy authority and an implicit allocator. A new stack uses the deployment `--admin` as the initial curator; governance can later replace it. |
| **Allocator** | Supplies to markets, recalls liquidity, refreshes adapter NAV, executes ready queued withdrawals, and may abort a stale `Withdrawing` operation. |
| **Sentinel** | Separate emergency backstop. It can pause, tighten restrictions, revoke specified operational/economic proposals, and use allocator-emergency recovery. It cannot unpause, relax restrictions, or accept proposals. |

The governance contract is not implicitly the Sentinel. Production deployments
should separate governance, allocator, and emergency keys or contracts according
to their operational risk.

## Curator economics (fees)

There are two fee types. Both are **minted as new SEP-41 shares** to a
configurable recipient.

| Fee | Basis | Cap |
|-----|-------|-----|
| **Management** | Time-weighted on AUM (`rate × AUM × elapsed / 1yr`), accrues regardless of performance | **5% / year** |
| **Performance** | AUM **growth** since the last accrual checkpoint; zero on flat or down periods | **50% of profit** |

Rates are WAD-scaled (`1e18 = 100%`). Each fee has its own recipient, and they
can differ.

Operational details:

- **Checkpoint, not all-time high-water mark.** Stellar share-pricing paths
  (`DepositWithMin`, `RefreshFees`, `ResyncIdleBalance`) first reconcile
  `idle_assets` against the live asset-token balance, then reset the `fee_anchor`
  to the reconciled total at the current ledger time. Profit is measured as
  `current_AUM − anchor_AUM`; if AUM is flat or down, the performance fee is zero.
  Because the anchor resets after each interaction, a recovery following a loss is
  chargeable — this is "growth since the last checkpoint", not "above the all-time
  peak". When fees are active, a deposit first crystallizes elapsed fees before
  the post-deposit anchor is written, so deposit principal cannot erase accrued
  fees.
- **Growth-rate cap** (`max_total_assets_growth_rate` internally and
  `--max-growth-rate-wad` in the CLI, optional). Caps how fast
  AUM is allowed to count for fee accrual:
  `effective_AUM = min(current, last × (1 + max_rate × dt/yr))`. Relaxing or
  removing this cap is timelocked.
- **Refresh order matters.** `curator refresh-fees` reconciles the live idle
  token balance, but it does not query every adapter. Refresh changed markets
  first, then crystallize fees against the resulting aggregate NAV.

```sh
tmplr-soroban-vault curator refresh-markets \
  --caller GALLOCATOR... \
  --markets 0,1
tmplr-soroban-vault curator refresh-fees
```

## Set up the operator CLI

The examples below assume `tmplr-soroban-vault` is on `PATH`. From a source
checkout, run the same commands with:

```sh
cargo run -p templar-soroban-vault-cli -- <arguments>
```

The current stack uses `stellar-cli` v26 and Rust 1.92. Run the repository's
Stellar CLI installer or enter its devenv before operating a vault.

Create a public profile for repeatable network, RPC, manifest, and address
defaults:

```sh
tmplr-soroban-vault profile init testnet
tmplr-soroban-vault --profile testnet doctor
```

Profiles must not contain seeds or secret keys. Keep signing material in the
Stellar keystore, select it with `stellar keys use <identity>`, or provide an
ephemeral secret through `STELLAR_ACCOUNT`. Never place a seed phrase or secret
key in `--source-account`.

The deployment manifest defaults to:

```text
contract/vault/soroban/.deploy-state/manifest.json
```

It records contract IDs, constructor arguments, artifact hashes, initialization
state, and successful transaction audit records. Treat it as operational state,
back it up, and pass `--state` explicitly when operating more than one vault.
### Artifact source and cache policy

Normal deployment uses the fixed reviewed `soroban-v1.1.1` catalog. Resolution is strict and
ordered: a verified release-cache entry first, an exact-pin workspace output seeded into that
cache second, and then the fixed GitHub release asset. Every byte is checked for the catalog's
length and SHA-256. Corrupt cache entries and download failures stop the command; deployment
never falls back to an implicit local build.

The default cache root is `<platform-cache>/templar/soroban-vault-cli/artifacts`. Set a non-empty
`TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE` value to use another writable root. In the published
image, mount `/home/templar/.cache/templar/soroban-vault-cli/artifacts` as persistent storage. A
normal deployment does not bundle WASM bytes in the executable or image.

Local compilation is opt-in. Pass bare `--build` to the relevant deploy command when
intentionally using checked-out source; it bypasses release cache/download and never writes
built bytes into the release cache. The source-repository setting (`--contract-source-repo` or
`SOROBAN_CONTRACT_SOURCE_REPO`) applies only to this explicit build path.

## Deploy a Stellar vault stack

Plan the deployment before writing to the network:

```sh
tmplr-soroban-vault deploy plan stack \
  --admin GCURATOR_OR_MULTISIG... \
  --asset-token CASSET... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CBLENDPOOL...
```

Then deploy the same configuration:

```sh
tmplr-soroban-vault deploy stack \
  --admin GCURATOR_OR_MULTISIG... \
  --asset-token CASSET... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CBLENDPOOL...

tmplr-soroban-vault status
tmplr-soroban-vault reconcile --json
```

`deploy stack` checkpoints the manifest after each upload, deployment, import,
and initialization step. Reruns reuse recorded contract IDs and remotely
available WASM. Use `--force-new` only when fresh contract instances are the
explicit intent.

If deployment stops after one or more transactions:

```sh
tmplr-soroban-vault reconcile --json
tmplr-soroban-vault deploy repair --json
tmplr-soroban-vault deploy resume \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CBLENDPOOL...
```

Resume only when reconciliation reports `safe_to_resume: true`. `status` reads
the manifest; `reconcile` compares it with chain state and is the stronger
check.

Mainnet writes require the global `--allow-mainnet-write` flag. A zero governance
timelock additionally requires `--allow-zero-timelock` and should be limited to
explicit local/test configurations.

## Governance lifecycle

The deployment `--admin` becomes both the governance admin and initial vault
curator. Governance can later assign a different curator, Sentinel, and
allocator set.

A submission always returns a proposal ID. Directionally safe actions may be
executed during submission; timelocked actions remain in the pending queue. Do
not assume every returned ID needs a later `accept` call.

Inspect queued proposals before accepting them:

```sh
tmplr-soroban-vault governance queue
tmplr-soroban-vault governance explain --proposal-id 7
tmplr-soroban-vault governance accept \
  --admin GCURATOR_OR_MULTISIG... \
  --proposal-id 7
```

`governance accept-ready` is useful for routine automation, but exact proposal
IDs are safer for high-impact changes. In particular, inspect and accept market
cap proposals by ID; a textual `cap` filter can also match cap-group actions.

### Exact timing rules

Governance timelocks are configured per action kind between 0 and 30 days. The
initial value is chosen at deployment; there is no universal production default.

| Change | Contract behavior |
|---|---|
| Pause | Only the Sentinel can pause, and it is immediate. Governance `submit-set-paused --paused` is rejected. |
| Unpause | Governance proposal; timelocked under `Pause`. |
| Restrictions | Sentinel may apply only a tightening change immediately. Every governance-admin restrictions submission is timelocked. |
| Fees | A proposal containing only fee decreases and/or a tighter growth cap executes immediately if recipients do not change. Any fee increase, recipient change, or growth-cap relaxation/removal is timelocked. |
| Market cap | Lowering an existing cap, including setting it to 0, executes immediately. A new market or cap increase is timelocked. |
| Cap groups | A new group cap, a cap increase, and every membership change are timelocked. Decreasing a known absolute or relative group cap executes immediately. Relative caps cannot exceed 100%. |
| Supply queue, allowed adapters, allocators | Timelocked. |
| Curator, governance, admin, market removal, skim, upgrade, migration | Timelocked. |
| Sentinel appointment | The first appointment may execute immediately; replacing an existing Sentinel is timelocked. |
| Timelock configuration | Increasing a duration executes immediately. Decreasing one is queued under the `TimelockConfig` timelock. |
| Withdrawal and idle-resync cooldowns | Every change is timelocked. |

The governance admin can permanently disable an action kind with `abdicate`.
Abdication is irreversible; confirm the exact action kind and recovery
implications before submitting it.

### Fees example

Fee values are WAD-scaled integers: `1e18 = 100%`.

```sh
tmplr-soroban-vault governance submit-set-fees \
  --admin GCURATOR_OR_MULTISIG... \
  --performance-fee-wad 200000000000000000 \
  --performance-recipient GPERFORMANCE... \
  --management-fee-wad 20000000000000000 \
  --management-recipient GMANAGEMENT...
```

The command prints a semantic old/new diff and requires interactive
confirmation or `--yes`. Inspect whether the proposal executed immediately or
entered the queue before running `accept`.

### Cap groups

Cap groups limit correlated routes together. When both limits are configured,
the effective ceiling is:

```text
min(absolute_cap, relative_cap × total_assets)
```

Absolute caps use raw asset base units and relative caps use WAD:

```sh
tmplr-soroban-vault governance submit-set-group-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --group blue-chip \
  --cap 50000000000000

tmplr-soroban-vault governance submit-set-group-rel-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --group blue-chip \
  --relative-cap 400000000000000000

tmplr-soroban-vault governance submit-set-group-member \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 0 \
  --group blue-chip
```

New group limits and all membership assignments are timelocked. Inspect and
accept each proposal ID before relying on the group.

### Sentinel emergency actions

Sentinel pause and restriction tightening are direct governance-contract
entrypoints, not queued CLI governance proposals:

```sh
stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_paused \
  --caller GSENTINEL... \
  --paused true

stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_restrictions \
  --caller GSENTINEL... \
  --mode 1 \
  --accounts '["GACCOUNT..."]'
```

Restriction modes are `0 = none`, `1 = blacklist`, and `2 = whitelist`.
The governance contract rejects a Sentinel restriction change that relaxes the
current policy.

Restoring normal operation uses governance and waits for the configured
timelock:

```sh
tmplr-soroban-vault governance submit-set-paused \
  --admin GCURATOR_OR_MULTISIG...
tmplr-soroban-vault governance queue --kind pause
```

The CLI's boolean `--paused` flag defaults to false when omitted. Supplying the
flag requests `true`, which this governance submission path rejects.

Pausing also blocks ordinary allocation and refresh operations. If an incident
requires liquidity recall, decide whether to lower market caps and unwind routes
before a global pause. Allocator-emergency recovery such as
`abort-withdrawing` remains available while paused.

## Add and activate market routes

Deploying an adapter does not make it usable by the vault. An active market
requires three accepted governance states:

1. The adapter contract is in the allowed-adapter set.
2. The market ID has a nonzero cap in raw asset base units.
3. The supply queue binds that market ID to the adapter address.

Add adapters to an existing or imported stack:

```sh
tmplr-soroban-vault deploy adapters \
  --vault CVAULT... \
  --governance CGOVERNANCE... \
  --asset-token CASSET... \
  --blend-pool CBLENDPOOL... \
  --custodian GCUSTODIAN...
```

Then submit the policy in order:

```sh
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR_OR_MULTISIG... \
  --adapters CBLENDADAPTER...,CCUSTODIALADAPTER...
# After the allowed-adapters proposal is ready:
tmplr-soroban-vault governance accept-ready \
  --admin GCURATOR_OR_MULTISIG... \
  --kind allowed-adapters

tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 0 \
  --cap 1000000000
# After the cap proposal is ready, verify and accept its exact ID:
tmplr-soroban-vault governance explain \
  --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault governance accept \
  --admin GCURATOR_OR_MULTISIG... \
  --proposal-id CAP_MARKET_0_PROPOSAL_ID

tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR_OR_MULTISIG... \
  --entry 0:CBLENDADAPTER... \
  --entry 1:CCUSTODIALADAPTER...
# After the supply-queue proposal is ready:
tmplr-soroban-vault governance accept-ready \
  --admin GCURATOR_OR_MULTISIG... \
  --kind supply-queue
```

Each `--entry` is `market_id:adapter_address`. Market IDs are stable identities,
not queue positions:

- Reordering the queue does not remap a market to another adapter.
- An existing market ID cannot be rebound to a different adapter.
- Supply requires the bound adapter to remain allowed.
- Withdrawal keeps using the stored binding, so liquidity can be recovered after
  an adapter is removed from new supply.
- Queue entries must be unique, enabled markets with nonzero caps. Practical
  queue size is also bounded by Soroban transaction resource limits.

### Adapter trust boundaries

- **Blend adapter** — queries and operates a configured Blend pool on Stellar.
- **Custodial adapter** — forwards assets to a configured custodian or multisig.
  The off-chain route, custody controls, NAV reporting, and liquidity-return
  procedure are part of the vault's trust boundary.

A custodial withdrawal only releases assets already returned to the adapter on
Stellar; it does not initiate or prove an external unwind. Reported NAV updates
must match both the current stored amount and the exact next nonce:

```sh
stellar contract invoke \
  --id "$CUSTODIAL_ADAPTER_ID" \
  --source-account custodian \
  -- set_reported_assets \
  --caller GCUSTODIAN... \
  --asset CASSET... \
  --expected_current 800000000 \
  --amount 1000000000 \
  --report_nonce 42
```

Before using a custodial route, document signer recovery, NAV cadence, report
approval, reconciliation, and delayed-liquidity procedures.

## Day-to-day allocation and accounting

Routine allocation commands use a positive amount and a stable market ID. The
allocator does not choose an adapter at execution time.

```sh
tmplr-soroban-vault curator refresh-markets \
  --caller GALLOCATOR... \
  --markets 0,1

tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR... \
  --market 0 \
  --amount 100 \
  --asset-decimals 7

tmplr-soroban-vault curator allocate-withdraw \
  --caller GALLOCATOR... \
  --market 0 \
  --amount 25 \
  --asset-decimals 7
```

Decimal flags are converted without floating point. Automation can use
`--amount-raw`, `--assets-raw`, or `--shares-raw` for exact base units.

The accounting behavior differs by direction:

- `allocate-supply` transfers assets to the bound adapter, calls its supply
  method, reads `total_assets(asset)`, and stores the observed route NAV.
- `allocate-withdraw` requests an amount from the adapter, verifies the actual
  vault token-balance delta matches the adapter's return value, and subtracts
  the realized amount. It does not refresh the adapter's remaining NAV.
- `refresh-markets` reads `total_assets(asset)` for the selected routes and
  replaces their stored principals. Run it after yield, loss, or a custodial NAV
  report and before fee/share-rate decisions.

Two maintenance calls are permissionless even though they are grouped under
`curator` in the CLI:

```sh
tmplr-soroban-vault curator resync-idle
tmplr-soroban-vault curator refresh-fees
```

`resync-idle` requires the vault to be idle and is rate-limited by the idle
resync cooldown, which defaults to 120 seconds. `refresh-fees` reconciles the
live idle balance and advances the fee checkpoint. Neither call substitutes for
`refresh-markets` when adapter NAV has changed.

## Withdrawal operations

The Stellar vault has two distinct exit paths.

### Atomic idle-liquidity exit

`user atomic-withdraw` and `user atomic-redeem` use the ERC-4626 proxy's slippage-protected atomic
exit methods when the deployment manifest contains `proxy_4626`. The CLI first verifies that the
recorded proxy interface exposes both atomic entrypoints; legacy proxies must be replaced rather
than bypassed. Proxy-less imported deployments fall back to the vault's equivalent atomic commands.
Both routes complete in one transaction only when the vault has enough idle assets. They never pull
liquidity from an adapter. As a result, `maxWithdraw` and `maxRedeem` can be zero
while the user's shares still represent assets deployed to markets.

```sh
tmplr-soroban-vault user preview --owner GUSER...

tmplr-soroban-vault user atomic-withdraw \
  --operator GUSER... \
  --assets 25 \
  --asset-decimals 7 \
  --max-shares-burned 25 \
  --share-decimals manifest
```

### Queued withdrawal

Use the queued path when idle liquidity is insufficient:

The proxy-facing `user withdraw` and `user redeem` commands preserve its asynchronous
ERC-7540-style compatibility methods. Use `request-withdraw` for the lower-level vault request
surface with explicit share and minimum-asset inputs.

```sh
tmplr-soroban-vault user request-withdraw \
  --owner GUSER... \
  --shares 10 \
  --share-decimals manifest \
  --min-assets-out 9.9 \
  --asset-decimals 7

# After cooldown, recall enough market liquidity if needed.
tmplr-soroban-vault curator allocate-withdraw \
  --caller GALLOCATOR... \
  --market 0 \
  --amount 10 \
  --asset-decimals 7

# The operator is an authorized allocator/curator, not the withdrawing user.
tmplr-soroban-vault user execute-withdraw \
  --operator GALLOCATOR...
```

The queued path has these mechanics:

- `request-withdraw` escrows shares and records a fixed asset claim at request
  time. The default cooldown is one hour.
- `execute-withdraw` services the queue head; it does not select a request ID.
- The caller must have allocator authority. The command is under `user` because
  it completes a user flow, not because any user may execute it.
- The head request must be cooled down and fully covered by idle assets. There is
  no partial payout.
- Finishing an allocation does not automatically progress the withdrawal queue;
  call `execute-withdraw` separately.
- The queue does not reserve idle assets against later atomic exits. Curators
  must monitor queued claims and maintain enough idle liquidity.
- There is no user cancellation path. Monitor request and payout events by
  request ID and alert on stalled heads.

If execution is already stuck in `Withdrawing`, an allocator, Sentinel, or
curator may abort the exact active operation:

```sh
tmplr-soroban-vault curator abort-withdrawing \
  --caller GALLOCATOR_OR_SENTINEL... \
  --op-id 42
```

This is an incident-recovery action, not a normal withdrawal tool. A successful
abort validates the active operation ID, restores collected idle accounting,
refunds escrowed shares, removes the affected queue head, emits a
`WithdrawalStopped` event, and returns the vault to `Idle`.

## TTL and archival operations

Soroban contract data is not permanent. Every vault needs an automated TTL job;
ordinary transaction traffic is not a substitute for a keeper schedule.

```sh
tmplr-soroban-vault extend-ttl
```

The CLI attempts the vault runtime, governance, ERC-4626 proxy, curator proxy,
share token, and every adapter recorded in the manifest. The asset token has no
deployment-wide TTL entrypoint and is reported as skipped. Treat a failed or
unexpectedly skipped component as an operational alert.

Vault, governance, proxy, and custodial-adapter maintenance uses permissionless
contract entrypoints. Share-token and Blend-adapter TTL entrypoints are
admin-gated, so the aggregate command does not invoke them. It instead uses
Stellar protocol-level operations to extend each contract instance and its WASM
code. The configured source account signs and pays for those operations; no
vault or governance contract authorization is required. The legacy `--caller`
option remains accepted for backward compatibility but is ignored.

Each contract owns its own TTL. Extending the vault runtime does not extend
governance, proxies, share-token holder entries, adapter storage, or oracle
storage. Run the aggregate command before archival: an extend operation cannot
revive an archived entry. If a contract is already archived, restore both its
instance with `stellar contract restore --id ...` and its WASM code with
`stellar contract restore --wasm-hash ...` before rerunning `extend-ttl`.
Contract-specific persistent entries may require separate restore or renewal
operations.

## Safety and automation

- Use `--dry-run` to print redacted Stellar commands and manifest decisions
  without writes.
- Every contract write is simulated before submission. Review auth, footprint,
  resource, fee, and contract-error output on stderr.
- Use `--json` for stable machine-readable responses or `--json-lines` for
  long-running automation. The schema lives at
  `tools/soroban-vault-cli/schema/output.schema.json`.
- Mainnet writes require `--allow-mainnet-write`.
- Dangerous governance submissions print an old/new semantic diff and require
  `--yes` or interactive confirmation.
- Successful writes append transaction metadata to the manifest. Preserve that
  audit trail alongside external monitoring and event indexing.
- Run `reconcile` after interrupted deployment, unexpected RPC results, or any
  manual contract operation that may have diverged from the manifest.
- Generate operator completions or a manpage with `completions` and `man` rather
  than copying stale command snippets into private runbooks.

Before a policy or allocation change, verify the manifest/network, refresh any
changed adapter NAV, inspect the current proposal queue, and identify the exact
role that must authorize the action. Afterward, verify the transaction result,
runtime state, adapter accounting, relevant events, and the manifest audit
record.
