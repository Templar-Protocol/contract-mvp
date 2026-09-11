# Templar Soroban Vault CLI

`tmplr-soroban-vault` deploys and operates Templar Soroban vault stacks.

The CLI delegates most transaction construction, signing, and submission to the installed
`stellar` CLI. It owns the Templar-specific pieces around artifact hashing, WASM upload/reuse,
deployment manifests, compact vault command payloads, and operator command routing.

This is the Stellar vault operator surface. It does not deploy or operate the separate NEAR vault
executor.

## Deployment

Before deploying, run `doctor` to check local readiness without submitting transactions:

```sh
tmplr-soroban-vault doctor
```

`doctor` checks the installed `stellar` CLI, configured network/passphrase/RPC inputs, source
identity availability without printing secret values, release-artifact cache readiness, Docker
mount health when running inside a container, and mainnet guard status.

```sh
cargo run -p templar-soroban-vault-cli -- \
  deploy stack \
  --governance-timelock-ns 86400000000000
```

By default, deployment state is stored in:

```text
contract/vault/soroban/.deploy-state/manifest.json
```

The deploy flow reuses contract IDs already recorded in the manifest unless `--force-new` is set.
It reuses uploaded WASM by local SHA-256 hash when the hash can be fetched from the configured
network, and uploads only when the WASM is missing remotely.
Only an explicit `--build` compiles WASM artifacts from the checked-out source. Such builds embed
`source_repo` contract metadata for explorer build-info and source-attestation discovery. The
default is `github:Templar-Protocol/contracts`; override it with `--contract-source-repo` or
`SOROBAN_CONTRACT_SOURCE_REPO`, or pass an empty value to disable the metadata. These source-repo
settings affect explicit builds only; they do not change the pinned release artifacts below.
During non-dry-run deployment writes, the manifest is checkpointed after each successful artifact
upload/reuse decision, contract deploy/import record, asset-token record, and initialization. If a
later initialize call fails, rerunning the command can reuse the IDs already written to the manifest.
### Release artifacts and local builds

Normal deployment resolves the fixed, reviewed `soroban-v1.1.1` release catalog. It never compiles
implicitly and never embeds WASM in the CLI or image. Source selection is deliberately fixed:

1. A valid entry under the release cache is rechecked for its exact byte length and SHA-256 and is
   used without an HTTP request or build.
2. If the cache entry is absent, an existing workspace output is accepted only when it exactly
   matches the catalog pin; the verified bytes are atomically seeded into the cache.
3. Otherwise the CLI downloads the recorded asset from the fixed GitHub release URL, verifies its
   exact length and SHA-256, atomically caches it, and uses that verified path.

An invalid cache entry or failed/mismatching download is a hard error; there is no compilation
fallback. The cache root is `<platform-cache>/templar/soroban-vault-cli/artifacts` by default. Set
the non-empty `TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE` value to use another writable root. In the
published image the configured root is
`/home/templar/.cache/templar/soroban-vault-cli/artifacts`.
`doctor` remains offline. It verifies existing cached/workspace bytes and performs a temporary
create/sync/remove probe to prove first-run cache writability; the probe tree is removed completely
and the real cache root is not created when it was previously absent.

Local compilation is an explicit migration choice: pass bare `--build` to `deploy stack`,
`deploy adapters`, `deploy curator-proxy`, or `deploy wasm <artifact>`. `--build` bypasses both
release cache and download, uses the checked-out workspace, and never writes locally built bytes
into the release cache. The old implicit-build behavior and `--build false` form are not used.

Use `reconcile` or `deploy repair` to compare a checkpointed manifest with chain state before
manual recovery. The repair plan classifies each component as `missing`, `deployed`, `initialized`,
`unknown`, or `mismatched`, includes fetched on-chain WASM hashes where available, and reports
safe next steps. `deploy resume` runs the same reconciliation gate and continues only when recorded
contracts are not mismatched or unknown.
In interactive human mode, `deploy stack` shows a progress bar across WASM upload/reuse, contract
deployment/reuse, initialization, and adapter deployment stages. Progress rendering is disabled for
`--json`, `--json-lines`, `--dry-run`, and non-TTY stderr.

The vault runtime WASM keeps its embedded contract spec. Vault initialization uses the ordinary
`stellar contract invoke -- initialize ...` path, so the Stellar CLI can resolve the ABI from the
deployed WASM. Configure RPC with `--rpc-url`, `STELLAR_RPC_URL`, or a profile `rpc_url`. Keep
signing material in the Stellar keystore or `STELLAR_ACCOUNT`; the CLI does not require secrets in
argv.

For write commands that emit a transaction hash, the CLI polls `stellar tx fetch` until RPC reports
success or failure, up to 300 seconds. If the original Stellar command exits after emitting a hash
and RPC later reports success, the CLI treats the transaction as confirmed and records the hash.

Pass `--blend-pool` once per Blend pool to deploy one adapter per pool. The manifest stores these
as `blend_adapter_0`, `blend_adapter_1`, and so on. On an existing deployment, new pools are
appended and pools already present in the manifest are left unchanged unless `--force-new` is set.
Pass `--custodian` once per custodian or multisig address to deploy custodial adapters in the same
flow. The manifest stores these as `custodial_adapter_0`, `custodial_adapter_1`, and so on, with
the `admin`, `vault`, `custodian`, and bound `asset` constructor args recorded for reconciliation
and status. Custodial adapters are single-asset; `deploy adapters --custodian ...` requires
`asset_token` in the manifest or `--asset-token`.
Every deployment that requests a Blend or custodial adapter must also pass
`--adapter-admin <address|vault>` (or `SOROBAN_ADAPTER_ADMIN`). Governance is never selected
implicitly. The `vault` alias, and an explicit address equal to the vault, are accepted only after
the CLI calls `vault.version()` and detects companion-upgrade capability `0x40`. The current default
runtime does not advertise that capability. Blend and custodial adapter admins may be
independently controlled accounts or contracts.

For a new stack, `--admin` is passed explicitly to the governance contract, the initial vault
curator configuration, and the share-token constructor. The share token keeps a separate immutable
`vault` binding for mint and burn; it rejects deployments or later admin rotations that set
`admin == vault`. The manifest records the initial admin as constructor provenance, while
reconciliation checks only the immutable vault binding because the token admin can rotate. For a
replacement stack, pass global `--fresh-state` with a new `--state` path. The CLI reserves that path
before any network call and refuses to overwrite an existing file. Omit `--fresh-state` when
resuming an interrupted deployment from its reserved manifest.

```sh
tmplr-soroban-vault \
  --state contract/vault/soroban/.deploy-state/new-vault.manifest.json \
  --fresh-state \
  deploy stack \
  --admin GADMIN... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --blend-pool CPOOL2... \
  --custodian G... \
  --adapter-admin GADAPTERADMIN...
```

To add adapters later without redeploying the stack, use `deploy adapters`. If the manifest already
contains `vault` and `governance`, only the requested adapter WASM and new adapter instances are
touched. For imported deployments, pass the existing contract ids once and the CLI records them in
the manifest before appending adapters.

```sh
tmplr-soroban-vault deploy adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --asset-token CASSET... \
  --blend-pool CPOOL... \
  --custodian G... \
  --adapter-admin GADAPTERADMIN...
```

To deploy only a fresh curator proxy for an existing vault, use `deploy curator-proxy`. Vault and
governance addresses are read from the manifest unless supplied explicitly. The command uploads or
reuses the current curator-proxy WASM and deploys a new proxy instance whose constructor atomically
pins the resolved source-account address as its one-time initialization authority. Deployment and
initialization remain separate transactions, but the deployed contract ID and authority are
checkpointed before `initialize` runs. Only that same source account can complete or retry
initialization. Rerunning the command reuses a matching checkpoint for initialization or
`vault_version` verification; pass `--force-new` only to abandon it and deploy a replacement. The
command checkpoints successful initialization provenance before querying `vault_version` and marks
the replacement version-discovery-capable only after that query succeeds.

```sh
tmplr-soroban-vault deploy curator-proxy \
  --vault CVAULT... \
  --governance CGOV...
```

For an approved versionless v1 vault, pass its verified current on-chain WASM hash. The CLI checks
the supplied hash against the vault before deploying the proxy, then invokes
`initialize_legacy_v1` instead of `initialize`.

```sh
tmplr-soroban-vault deploy curator-proxy \
  --vault CVAULT... \
  --governance CGOV... \
  --legacy-v1-wasm-hash cd8ad034fd37e246ca578b1cfd1dc3d95fb532a6591ecf491ee01d2db03f5244
```

Reconciliation calls `vault_version` for curator proxies recorded by this flow. Older manifests and
older curator proxies remain backward compatible: without the capability marker, reconciliation
continues to verify only their existing vault and governance wiring.

Use `deploy plan` to inspect reuse, upload, deploy, and manifest decisions without network writes or
manifest changes:

```sh
tmplr-soroban-vault deploy plan stack \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian G... \
  --adapter-admin GADAPTERADMIN...

tmplr-soroban-vault deploy plan adapters \
  --vault CVAULT... \
  --governance CGOV... \
  --blend-pool CPOOL... \
  --custodian G... \
  --adapter-admin GADAPTERADMIN...
```

Recover an interrupted deployment by reconciling first, then resuming if the repair plan reports
`safe_to_resume: true`:

```sh
tmplr-soroban-vault reconcile --json
tmplr-soroban-vault reconcile --skip-view-verification --json
tmplr-soroban-vault deploy repair --json
tmplr-soroban-vault deploy resume \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian G... \
  --adapter-admin GADAPTERADMIN...
```

The CLI validates Soroban account and contract addresses at parse time for operational commands.
WASM hashes accepted by governance upgrade commands must be 32-byte hex values.

## Curator Role Models

The CLI uses the contract name `governance admin` for the address that controls the governance
contract, but operationally that address can represent several curator models. It can be a single
operator key, a multisig contract, or a governance contract controlled by an external process. A new
stack uses `--admin` as both the governance admin and the initial vault curator. After deployment,
the same address is also the share-token administrator, while the installed token implementation
reserves mint/burn authority for the immutable vault binding. Governance proposals can split the
runtime roles into separate curator, sentinel, and allocator identities.

Common role terms:

- `governance admin`: the address passed as `--admin` on governance commands. It submits and accepts
  governance proposals. For multisig governance, this is the multisig contract address.
- `vault curator`: the runtime curator address. It starts as the deployment `--admin` and can be
  changed with `governance submit-set-curator`.
- `share-token admin`: the deployment `--admin`. It controls token pause, restrictions, upgrades,
  TTL maintenance, and two-step admin rotation. The installed token code reserves mint and burn for
  the immutable vault, but the admin can replace that code through an upgrade. Treat the admin as
  the ultimate trust boundary for all share-token behavior and keep it reachable and recoverable.
- `sentinel`: an emergency backstop configured with `governance submit-set-sentinel`. It can only
  take protective actions such as pausing or tightening restrictions; governance admin is still
  required to relax or unpause.
- `allocator`: one or more addresses configured with `governance submit-set-allocators`. Allocators
  run market allocation and refresh operations. The vault curator is always authorized as an
  allocator too.
- `adapter`: a market route such as a Blend adapter or custodial adapter. The governance admin must
  allow adapters, set a nonzero market cap, and place them into the typed supply queue before
  allocations can route through them.

### Allocation Through Adapters

Allocation is a two-step mental model:

1. Governance configures which adapter contract serves each market id.
2. Allocators later move assets by market id and amount; they do not choose an adapter at execution
   time.

Adapters can exist in the deployment manifest before they are usable by the vault. A Blend or
custodial adapter becomes an active supply route only after governance adds it to the allowed
adapter set, sets a nonzero market cap, and binds it into the supply queue:

```sh
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR_OR_MULTISIG... \
  --adapters CBLENDADAPTER...,CCUSTODIALADAPTER...
tmplr-soroban-vault governance accept-ready \
  --admin GCURATOR_OR_MULTISIG... \
  --kind allowed-adapters

tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 0 \
  --cap 1000000000
tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 1 \
  --cap 1000000000
# After the cap proposals are ready, verify and accept the specific SetCap proposal ids.
tmplr-soroban-vault governance explain --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault governance explain --proposal-id CAP_MARKET_1_PROPOSAL_ID
tmplr-soroban-vault governance accept \
  --admin GCURATOR_OR_MULTISIG... \
  --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault governance accept \
  --admin GCURATOR_OR_MULTISIG... \
  --proposal-id CAP_MARKET_1_PROPOSAL_ID

tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR_OR_MULTISIG... \
  --entry 0:CBLENDADAPTER... \
  --entry 1:CCUSTODIALADAPTER...
tmplr-soroban-vault governance accept-ready \
  --admin GCURATOR_OR_MULTISIG... \
  --kind supply-queue
```

Each `--entry` is `market_id:adapter_address`. The `market_id` is the same value passed later to
`curator allocate-supply --market` and `curator allocate-withdraw --market`.

Once setup is accepted, the allocation command is deliberately small:

```sh
tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR... \
  --market 0 \
  --amount 100 \
  --asset-decimals 7
```

The CLI sends a compact vault command containing the caller, market id, amount, and direction. The
vault resolves the adapter from its stored market binding. For supply, the bound adapter must still
be in the allowed adapter set. For withdrawal, the vault uses the stored market binding so assets can
be recovered from an existing route even if governance has removed that adapter from new supply.

The supply queue is typed by market id, not by list position. Reordering the queue does not change
which adapter a market id uses, and an existing market id cannot be rebound to a different adapter by
submitting a new queue entry. UIs should present adapter selection as a governance/setup workflow and
present routine allocation as `direction + market id + amount + caller`.

For any route that leaves Stellar, keep the accounting boundary at the adapter. The vault does not
track off-chain hops or map individual vault shares to external accounts or market positions. User
shares are derived from aggregate vault NAV: `total_assets / total_shares`. The adapter reports route
NAV through `total_assets(asset)`, and the vault incorporates that value when allocators supply or
run `curator refresh-markets`. `curator allocate-withdraw` does not refresh route NAV; it verifies
the realized Stellar token-balance delta and subtracts that amount from stored principal.

Withdrawals use distinct CLI surfaces:

- `curator allocate-withdraw` is an allocator operation. It pulls liquidity back from the adapter
  bound to a market id by calling the adapter's `progress_withdrawal(vault, asset, amount)`. The
  `amount` is the requested adapter withdrawal amount; the vault accounts the assets actually
  returned by the adapter without calling adapter `total_assets`.
- `user withdraw` and `user redeem` preserve the ERC-4626 proxy's asynchronous compatibility
  methods. `user request-withdraw` is the lower-level direct vault request with explicit share and
  minimum-asset inputs. These paths queue and escrow shares immediately; the configured cooldown
  gates `user execute-withdraw`. Execution attempts to pay the next ready request from idle vault
  assets and requires an allocator-authorized operator. If idle assets are not sufficient, an
  allocator first uses `curator allocate-withdraw` to bring liquidity back from one or more market
  adapters, then `user execute-withdraw` can settle the queued request.
- `user atomic-withdraw` and `user atomic-redeem` are the synchronous, slippage-protected idle
  liquidity exits. They verify that a recorded ERC-4626 proxy exposes both atomic entrypoints before
  calling it. A legacy recorded proxy is rejected with a replacement instruction; the direct vault
  fallback is used only when `proxy_4626` is absent from the manifest.
- `curator abort-withdrawing` is the recovery operation for a stale in-flight withdrawal operation.
  Use it only when the vault is stuck in `Withdrawing` and operators have identified the operation id
  to abort.

```sh
tmplr-soroban-vault curator allocate-withdraw \
  --caller GALLOCATOR... \
  --market 0 \
  --amount 25 \
  --asset-decimals 7

tmplr-soroban-vault user request-withdraw \
  --owner GUSER... \
  --shares 10 \
  --share-decimals manifest \
  --min-assets-out 9.9 \
  --asset-decimals 7

tmplr-soroban-vault user execute-withdraw --operator GALLOCATOR...

tmplr-soroban-vault curator abort-withdrawing \
  --caller GALLOCATOR_OR_SENTINEL... \
  --op-id 42
```

Curator commands fall into three groups:

- Allocation: `curator allocate-supply`, `curator allocate-withdraw`, and
  `curator abort-withdrawing`.
- Maintenance: `curator refresh-markets`, `curator refresh-fees`, and `curator resync-idle`.
- Governance conveniences for zero-timelock/local workflows: `curator set-allowed-adapters` and
  `curator set-supply-queue`. Production or timelocked deployments normally use the matching
  `governance submit-*` and `governance accept-ready` commands.

For timelocked deployments, submit commands create proposals and `accept-ready` accepts them only
after the relevant timelock has elapsed. Use `governance queue` and `governance explain` to inspect
pending proposal ids and readiness. For market caps, prefer accepting the specific SetCap proposal id
after `explain` confirms the market id; avoid filtering ready proposals by cap kind because that
text match can also include cap-group proposals.

### Single Curator

Use this model when one operational address controls governance and day-to-day allocation. This is
the simplest testnet or custodial setup.

```sh
tmplr-soroban-vault deploy stack \
  --admin GCURATOR... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --adapter-admin GCURATOR...

tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR... \
  --adapters CBLENDADAPTER...
tmplr-soroban-vault governance accept-ready --admin GCURATOR... --kind allowed-adapters

tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR... \
  --market-id 0 \
  --cap 1000000000
# After the cap proposal is ready, verify and accept the specific SetCap proposal id.
tmplr-soroban-vault governance explain --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault governance accept \
  --admin GCURATOR... \
  --proposal-id CAP_MARKET_0_PROPOSAL_ID

tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR... \
  --entry 0:CBLENDADAPTER...
tmplr-soroban-vault governance accept-ready --admin GCURATOR... --kind supply-queue

tmplr-soroban-vault curator allocate-supply \
  --caller GCURATOR... \
  --market 0 \
  --amount 1.25 \
  --asset-decimals 7
```

For zero-timelock local deployments, the curator convenience commands can submit and immediately
accept adapter setup:

```sh
tmplr-soroban-vault curator set-allowed-adapters \
  --admin GCURATOR... \
  --adapters CBLENDADAPTER... \
  --auto-accept
tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR... \
  --market-id 0 \
  --cap 1000000000
# After the cap proposal is ready, verify and accept the specific SetCap proposal id.
tmplr-soroban-vault governance explain --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault governance accept \
  --admin GCURATOR... \
  --proposal-id CAP_MARKET_0_PROPOSAL_ID
tmplr-soroban-vault curator set-supply-queue \
  --admin GCURATOR... \
  --entry 0:CBLENDADAPTER... \
  --auto-accept
```

### Multisig Governance Curator

Use this model when the curator is decentralized and governance actions are controlled by a multisig
or governance contract. Deploy with the multisig as `--admin`; the CLI treats that address as the
governance caller, while the Stellar source account supplies the transaction envelope.

```sh
tmplr-soroban-vault deploy stack \
  --admin CMULTISIG... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --adapter-admin CMULTISIG...

tmplr-soroban-vault governance plan-submit-set-supply-queue \
  --admin CMULTISIG... \
  --entry 0:CBLENDADAPTER...
```

Fresh markets need an accepted `submit-set-cap --market-id ... --cap ...` proposal before governance
accepts a supply queue containing that market. `--cap` is expressed in raw asset base units.

When the multisig can authorize the child invocation directly, submit and accept through the CLI:

```sh
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin CMULTISIG... \
  --adapters CBLENDADAPTER...
tmplr-soroban-vault governance queue --kind allowed-adapters
tmplr-soroban-vault governance accept-ready --admin CMULTISIG... --kind allowed-adapters
```

If the multisig requires a separate proposal flow, use the `plan-*`, `queue`, `explain`, and
`pending` commands to prepare and audit the intended action, then submit the equivalent invocation
through the multisig workflow.

### Curator With Sentinel

Use this model when a separate emergency key or contract can pause the vault or tighten transfer
restrictions, while normal governance remains with the governance admin.

```sh
tmplr-soroban-vault governance submit-set-sentinel \
  --admin GCURATOR_OR_MULTISIG... \
  --sentinel GSENTINEL...
tmplr-soroban-vault governance queue --kind sentinel
```

The first Sentinel appointment executes immediately. Replacing an existing Sentinel creates a
timelocked proposal; inspect that proposal and accept it after maturity.

Governance-admin restriction changes use the normal timelocked proposal path. The governance
submission path cannot pause; `submit-set-paused` is the timelocked unpause path. Omit its
`--paused` flag so the boolean remains false:

```sh
tmplr-soroban-vault governance submit-set-restrictions \
  --admin GCURATOR_OR_MULTISIG... \
  --mode blacklist \
  --accounts GACCOUNT...
```

Sentinel emergency actions are immediate governance-contract entrypoints. They bypass the proposal
queue and are intentionally one-way: the sentinel can pause or tighten restrictions, but cannot
unpause or relax restrictions. Use `stellar contract invoke` with the same network/profile
environment as the vault CLI:

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

The governance admin restores normal operation:

```sh
tmplr-soroban-vault governance submit-set-paused \
  --admin GCURATOR_OR_MULTISIG...
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG... --kind pause
```

### Curator With Allocators

Use this model when governance is retained by the curator or multisig, but allocation execution is
delegated to one or more hot or automated addresses. Configure allocators through governance, then
use the allocator address as the `--caller` for market operations.

```sh
tmplr-soroban-vault governance submit-set-allocators \
  --admin GCURATOR_OR_MULTISIG... \
  --allocators GALLOCATOR1...,GALLOCATOR2...
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG... --kind allocators

tmplr-soroban-vault curator refresh-markets \
  --caller GALLOCATOR1... \
  --markets 0,1
tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR1... \
  --market 0 \
  --amount 100 \
  --asset-decimals 7
tmplr-soroban-vault curator allocate-withdraw \
  --caller GALLOCATOR1... \
  --market 0 \
  --amount 25 \
  --asset-decimals 7
```

### Curator With Sentinel And Allocators

Use this model for a more separated production setup: governance admin or multisig controls policy,
allocator keys execute routine market operations, and a sentinel key or contract handles emergency
pause/restriction tightening.

```sh
tmplr-soroban-vault deploy stack \
  --admin GCURATOR_OR_MULTISIG... \
  --governance-timelock-ns 86400000000000 \
  --blend-pool CPOOL... \
  --custodian GCUSTODIAN... \
  --adapter-admin GADAPTERADMIN...

tmplr-soroban-vault governance submit-set-sentinel \
  --admin GCURATOR_OR_MULTISIG... \
  --sentinel GSENTINEL...
tmplr-soroban-vault governance submit-set-allocators \
  --admin GCURATOR_OR_MULTISIG... \
  --allocators GALLOCATOR1...,GALLOCATOR2...
tmplr-soroban-vault governance submit-set-allowed-adapters \
  --admin GCURATOR_OR_MULTISIG... \
  --adapters CBLENDADAPTER...,CCUSTODIALADAPTER...
tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 0 \
  --cap 1000000000
tmplr-soroban-vault governance submit-set-cap \
  --admin GCURATOR_OR_MULTISIG... \
  --market-id 1 \
  --cap 1000000000
tmplr-soroban-vault governance submit-set-supply-queue \
  --admin GCURATOR_OR_MULTISIG... \
  --entry 0:CBLENDADAPTER... \
  --entry 1:CCUSTODIALADAPTER...

tmplr-soroban-vault governance queue
tmplr-soroban-vault governance accept-ready --admin GCURATOR_OR_MULTISIG...
```

After setup, allocators operate the supply queue and sentinel handles emergencies:

```sh
tmplr-soroban-vault curator allocate-supply \
  --caller GALLOCATOR1... \
  --market 1 \
  --amount 1000 \
  --asset-decimals 7

stellar contract invoke \
  --id "$SOROBAN_GOVERNANCE" \
  --source-account sentinel \
  -- set_paused \
  --caller GSENTINEL... \
  --paused true
```

Run `curator refresh-markets` after adapter NAV changes and before relying on share-rate or
accounting views. `curator allocate-supply` observes adapter `total_assets` after supplying, but
`curator allocate-withdraw` verifies the realized token balance delta and subtracts that amount from
stored principal; it does not refresh route NAV.

For custodial adapters, use
`deploy adapters --custodian <address> --adapter-admin <address|vault>` to append custodial routes,
then allow the deployed adapter, set a nonzero market cap, refresh reported NAV, and add it to the
supply queue before allocating to it. Each custodial adapter is bound to the manifest asset token at
deployment and rejects calls for any other asset. The custodian, adapter admin, or vault can
explicitly report route NAV on the adapter. Reports include the current stored NAV and a
nonce that must be exactly one greater than the stored nonce, so stale heartbeats cannot re-add
assets that have already been released back to the vault:

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

## Safety

- Mainnet write commands require `--allow-mainnet-write`.
- Zero governance timelocks require `--allow-zero-timelock`.
- Existing manifests must record a nonempty network label matching `--network`. This label check does not authenticate custom network passphrases; do not reuse one network label with a different passphrase.
- Adapter deployment requires an explicit `--adapter-admin`; selecting the vault fails closed unless runtime capability `0x40` is detected first.
- Blend and custodial adapter admins may be accounts or contracts, but must differ from the
  configured governance contract. Custodial adapter admins must also differ from the bound asset
  token.
- Share-token deployment rejects `admin == vault`; the manifest retains the initial admin as constructor provenance and reconciliation verifies the immutable vault binding.
- `--fresh-state deploy stack` atomically reserves an unused manifest path before network calls; planning and dry-run modes require the path to be absent without creating it.
- `--dry-run` prints the `stellar` commands with source-account environment overrides redacted, returns planned contract ids in the response, and never writes the manifest.
- `--json` emits stable machine-readable envelopes with `type`, `ok`, `network`, `manifest`, `commands`, `tx_hashes`, `warnings`, and command-specific `data`.
- `--json-lines` emits the same envelope format as newline-delimited JSON for long-running automation.
- Contract writes run a Stellar preflight simulation before submission: invokes use `--send no`, while deploy/upload transactions use `--build-only` followed by `stellar tx simulate`. The CLI prints simulation output to stderr, including Stellar-reported auth, footprint/resource, fee, and contract-error details when the Stellar CLI provides them; JSON modes keep stdout machine-readable and surface preflight failures as structured command errors.
- Prefer Stellar keystore identities: run `stellar keys use <identity>` in the mounted/configured Stellar config directory, or pass a non-secret identity alias/public account with `--source-account`.
- Do not pass raw secret keys or seed phrases to `--source-account`; the CLI rejects obvious secret material there. If an operator must use an ephemeral secret, set `STELLAR_ACCOUNT` for the `stellar` child process environment instead of putting it in CLI argv.
- Source-account overrides use `secrecy`/`zeroize` wrappers, are redacted from command displays and tracing logs, are zeroized from in-process environment override copies after use, and are never persisted to the deployment manifest.
- Tracing logs are opt-in with `TEMPLAR_SOROBAN_VAULT_LOG` or `RUST_LOG` and are written to stderr. For example, `TEMPLAR_SOROBAN_VAULT_LOG=templar_soroban_vault_cli=info` logs deployment checkpoints and redacted Stellar command execution.
- Decimal amount flags such as `--assets 1.25 --asset-decimals 7` and `--shares 10 --share-decimals manifest` are converted to raw contract units without floating point. Exact machine callers can use raw flags such as `--assets-raw`, `--shares-raw`, and `--amount-raw`.
- Machine output is described by `tools/soroban-vault-cli/schema/output.schema.json`. Structured errors include codes such as `missing_manifest_contract`, `mainnet_guard`, and `secret_in_argv`.
- Successful non-dry-run write commands append an audit record to the manifest `transactions` list with timestamp, command/action, target contract/function when known, tx hash when visible in Stellar output, source public address when known, status, and artifact hash when applicable.
- Dangerous governance submissions such as admin rotation, timelock updates, supply queue replacement, and fee updates print a semantic old/new diff in human mode and require `--yes` or interactive confirmation before submitting. JSON/JSON-lines mode skips prompts for automation.

## Profiles

Profiles are public TOML config files for repeated local and Docker operations. They can store
network, RPC URL, passphrase, manifest path, workspace path, Stellar config directory, and default
public admin/caller/operator addresses. Do not store secret keys or seed phrases in profile files.

```sh
tmplr-soroban-vault profile init testnet
tmplr-soroban-vault --profile testnet status
```

By default, profiles live under the user config directory in
`templar/soroban-vault-cli/profiles/<name>.toml`. Set
`TEMPLAR_SOROBAN_VAULT_PROFILE_DIR` to use a project-local or Docker-mounted profile directory.
Explicit CLI flags and environment variables override profile values.

## Operator Assistance

```sh
tmplr-soroban-vault completions zsh > _tmplr-soroban-vault
tmplr-soroban-vault completions bash
tmplr-soroban-vault completions fish
tmplr-soroban-vault man > tmplr-soroban-vault.1
```

## Docker

Build the operator image from the repository root:

```sh
docker build \
  -f tools/soroban-vault-cli/Dockerfile \
  -t templar/soroban-vault-cli:local \
  .
```

The image includes `tmplr-soroban-vault`, `stellar-cli` v26, and Rust toolchains/targets retained
for explicit `stellar contract build` when `--build` is passed. Normal deployment instead uses
the fixed release catalog and verified artifact cache. It defaults to `/workspace` as the Templar
workspace and persists Stellar config, deployment state, the verified release cache, Cargo cache,
and build outputs through mount points.

```sh
docker run --rm templar/soroban-vault-cli:local --help

docker volume create templar-soroban-vault-artifacts

docker run --rm -it \
  -v "$PWD:/workspace" \
  -v "$HOME/.config/stellar:/home/templar/.config/stellar" \
  -v "$PWD/contract/vault/soroban/.deploy-state:/workspace/contract/vault/soroban/.deploy-state" \
  -v "$PWD/target:/workspace/target" \
  --mount type=volume,source=templar-soroban-vault-artifacts,target=/home/templar/.cache/templar/soroban-vault-cli/artifacts \
  templar/soroban-vault-cli:local status

The same mount pattern supports deployment commands. The cache volume persists verified release
bytes between runs; mounting `target` and the workspace keeps checked-out source available for
`deploy ... --build`. Mounting the Stellar config preserves identities and network configuration
across runs. Use `stellar keys use <identity>` in that config, or pass `-e STELLAR_ACCOUNT` to
Docker when an ephemeral source account must come from the environment. The release cache does not
contain bundled image WASM; it is populated only after runtime pin verification.

## Common Operations

```sh
# Slippage-protected user deposit. A configured proxy is invoked through deposit_with_min.
tmplr-soroban-vault user deposit \
  --operator G... \
  --assets 1.25 \
  --asset-decimals 7 \
  --min-shares-out-raw 0

# Exact raw units remain available for automation.
tmplr-soroban-vault user deposit --operator G... --assets-raw 12500000

# Queue an asynchronous exact-asset withdrawal through the ERC-4626 proxy.
tmplr-soroban-vault user withdraw --operator G... --assets 1 --asset-decimals 7

# Exit synchronously from idle liquidity with an explicit maximum share burn.
tmplr-soroban-vault user atomic-withdraw \
  --operator G... \
  --assets 1 \
  --asset-decimals 7 \
  --max-shares-burned 1.01 \
  --share-decimals manifest

# Allocator supply through the vault compact command ABI.
tmplr-soroban-vault curator allocate-supply \
  --caller G... \
  --market 0 \
  --amount 1.25 \
  --asset-decimals 7

# Allocator recovery from an in-flight withdrawing state.
tmplr-soroban-vault curator abort-withdrawing --caller G... --op-id 42

# Curator maintenance operations. Refresh adapter NAV before relying on share-rate or accounting views.
tmplr-soroban-vault curator refresh-markets --caller G... --markets 0,1
tmplr-soroban-vault curator refresh-fees
tmplr-soroban-vault curator resync-idle

# Submit and optionally accept governance-backed supply queue changes.
tmplr-soroban-vault curator set-supply-queue \
  --admin G... \
  --entry 0:C...

# Fresh markets need a nonzero raw asset-unit cap before governance accepts them in the supply queue.
tmplr-soroban-vault governance submit-set-cap \
  --admin G... \
  --market-id 0 \
  --cap 1000000000

# Submit the same typed supply queue directly to governance.
tmplr-soroban-vault governance submit-set-supply-queue \
  --admin G... \
  --entry 0:C... \
  --entry 1:C...

# Plan common governance transactions without submitting them.
tmplr-soroban-vault governance plan-submit-set-supply-queue \
  --admin G... \
  --entry 0:C...
tmplr-soroban-vault governance plan-submit-set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000
tmplr-soroban-vault governance plan-accept --admin G... --proposal-id 7

# Inspect and progress pending governance proposals.
tmplr-soroban-vault governance queue
tmplr-soroban-vault governance explain --proposal-id 7
tmplr-soroban-vault governance accept-ready --admin G...
tmplr-soroban-vault governance submit-and-wait \
  --max-wait-seconds 3600 \
  set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000
tmplr-soroban-vault governance submit-and-wait proposal \
  --admin G... \
  --proposal-id 7

# Update a specific governance timelock using the contract TimelockKind variants.
tmplr-soroban-vault governance submit-set-timelock \
  --admin G... \
  --kind supply-queue \
  --timelock-ns 86400000000000

# Update restrictions with a typed mode: none, blacklist, or whitelist.
tmplr-soroban-vault governance submit-set-restrictions \
  --admin G... \
  --mode whitelist \
  --accounts G...,G...

# Submit typed cap, fee, cooldown, skim, allocator, and admin handoff proposals.
tmplr-soroban-vault governance submit-set-cap --admin G... --market-id 0 --cap 1000000
tmplr-soroban-vault governance submit-set-fees \
  --admin G... \
  --performance-fee-wad 0 \
  --performance-recipient G... \
  --management-fee-wad 0 \
  --management-recipient G...

# Select the second Blend adapter by deploy order.
tmplr-soroban-vault adapter --adapter-index 1 pool

# Or select an adapter by manifest key or pool address.
tmplr-soroban-vault adapter --adapter-key blend_adapter_1 admin
tmplr-soroban-vault adapter --adapter-pool CPOOL... total-assets --asset C...

# Extend TTL for every TTL-capable contract in the manifest.
tmplr-soroban-vault extend-ttl

# Print shell environment values from the manifest.
tmplr-soroban-vault export-env
```

`export-env` emits `BLEND_ADAPTER_ID` for the first adapter for compatibility, plus indexed
`BLEND_ADAPTER_0_ID`, `BLEND_ADAPTER_1_ID`, and matching `BLEND_POOL_0_ID` values when pool
constructor args are known. Custodial adapters use the same pattern with `CUSTODIAL_ADAPTER_ID`,
`CUSTODIAL_ADAPTER_0_ID`, matching indexed `CUSTODIAL_0_ADDRESS`, and `CUSTODIAL_0_ASSET` values
when constructor args are known. The unindexed `CUSTODIAL_ADDRESS` name is reserved for explicit
deploy input and is not emitted by `export-env`.

`extend-ttl` runs the vault compact `ExtendTtl` command and the permissionless `extend_ttl`
entrypoints on governance, the ERC-4626 proxy, the curator proxy, and custodial adapters. For the
share token and each Blend adapter, it uses Stellar protocol-level TTL operations to extend both the
contract instance and its WASM code without invoking the admin-gated contract entrypoint. Shared
Blend WASM code is extended once per unique hash. Manifest contracts without an explicit
deployment-wide TTL path, such as the asset token, are reported as skipped.

The configured source account signs and pays for the protocol-level operations. The aggregate
command still accepts `--caller` and `SOROBAN_TTL_CALLER` for backward compatibility, but ignores
them; new automation should omit them.

Run `extend-ttl` proactively while every contract entry is live. An extend operation cannot revive
an archived entry. Restore an archived contract instance and its WASM code with the corresponding
`stellar contract restore --id ...` and `stellar contract restore --wasm-hash ...` operations, then
rerun the aggregate command. Contract-specific persistent entries may require their own restore or
renewal flow.
