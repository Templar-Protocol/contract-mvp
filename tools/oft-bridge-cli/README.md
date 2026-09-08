# Templar OFT Bridge CLI

`tmplr-oft-bridge` is the operator CLI for a non-USDC LayerZero OFT route between Stellar and an
EVM network. It prepares and verifies deployments, manages route configuration and authority,
quotes and submits capped canary transfers, tracks LayerZero messages, and reconciles token custody.

The CLI emits a stable JSON envelope for every command, including failures, so the same commands can
be used interactively or from an external scheduler. It is an operator tool, not a bridge service or
an unattended relayer.

## Run the packaged image

The release image is distributed through GitHub Container Registry. Pin a release tag or immutable
digest for operator use; do not rely on `latest`.

```sh
export OFT_BRIDGE_IMAGE=ghcr.io/templar-protocol/oft-bridge-cli:0.1.0
docker pull "$OFT_BRIDGE_IMAGE"
docker run --rm "$OFT_BRIDGE_IMAGE" --version
docker run --rm "$OFT_BRIDGE_IMAGE" --help
```

To build the image from an audited repository checkout instead:

```sh
docker build \
  --file tools/oft-bridge-cli/Dockerfile \
  --tag tmplr-oft-bridge:local \
  .
```

The image runs as the unprivileged `templar` user with UID/GID `10001`. For bind mounts owned by
your local account, pass `--user "$(id -u):$(id -g)"`. The CLI requires sensitive files to be
regular, non-symlink files with mode `0600`, so using the host UID is normally the simplest setup.

## Persistent operator data

`--state` names a **directory**, not one JSON file. Each route directory contains the authoritative
`route.json`, append-only `operations.jsonl` and `messages.jsonl`, lock files, and preserved
artifacts. Keep the entire directory together and back it up atomically.

Every executing command also requires `--operation-store-root`. This separate, pre-existing
directory holds authority-domain locks and reservations shared across every route controlled by the
same signing authority. All processes and hosts using that authority must use the same store on a
filesystem that provides reliable atomic file creation, advisory locks, rename, and `fsync`.

Prepare the mounts once:

```sh
mkdir -p operator/routes operator/operations operator/secrets operator/input
chmod 0700 operator/routes operator/operations operator/secrets
```

Use the same mounts for every invocation:

```sh
docker run --rm \
  --user "$(id -u):$(id -g)" \
  --volume "$PWD/operator/routes:/data/routes" \
  --volume "$PWD/operator/operations:/data/operations" \
  --volume "$PWD/operator/secrets:/run/templar-secrets:ro" \
  --volume "$PWD/operator/input:/input:ro" \
  "$OFT_BRIDGE_IMAGE" \
  health --state /data/routes/example
```

Do not mount one route directory over another, copy only `route.json`, or run two independent
operation stores for the same signer. Those arrangements break the CLI's custody history or its
cross-route serialization boundary.

## RPC and signer credentials

RPC URLs are accepted only through a named environment variable or a mode-`0600` file. File-backed
providers avoid putting a credential-bearing URL in argv or shell history:

```sh
printf '%s\n' 'https://stellar.example.invalid/rpc' > operator/secrets/stellar-rpc
printf '%s\n' 'https://sepolia.example.invalid/v2/API_TOKEN' > operator/secrets/evm-rpc
chmod 0600 operator/secrets/stellar-rpc operator/secrets/evm-rpc
```

Pass them to a command with:

```text
--stellar-rpc-file /run/templar-secrets/stellar-rpc
--evm-rpc-file /run/templar-secrets/evm-rpc
```

The equivalent environment-backed flags are `--stellar-rpc-env NAME` and `--evm-rpc-env NAME`.
Environment values do not enter CLI argv or route state, but anyone with Docker-daemon access may
inspect a container's environment. Use a short-lived container and a narrowly scoped disposable
credential when environment providers are unavoidable.

Authenticated RPC headers may be supplied as a mode-`0600` JSON object:

```json
{
  "Authorization": "Bearer replace-with-provider-token"
}
```

Mount the file read-only and pass `--rpc-headers-file /run/templar-secrets/rpc-headers.json`.
The v1 header set is process-global and is sent to both configured RPC providers. Use it only when
both URLs share the same credential trust boundary; otherwise use credentials embedded in separate
mode-`0600` RPC URL files.

Testnet Stellar signing uses `--stellar-secret-env NAME`; the named value must contain an `S...`
secret seed. The CLI intentionally has no seed-in-argv or seed-file flag. EVM signing uses a Foundry
V3 keystore and a separate password file:

```text
--evm-keystore /run/templar-secrets/operator-keystore.json
--evm-password-file /run/templar-secrets/evm-password
```

Both files must be regular non-symlink files with mode `0600`. Never put seeds, passwords, API
tokens, keystores, RPC URLs, or header files inside the route or operation-store mounts.

## Operator workflow

Start from a reviewed `DesiredRouteV1` JSON document. It binds the asset policy, endpoint identities
and code hashes, owners and delegates, libraries, DVNs, executor, enforced options, economic limits,
finality policy, artifacts, and custody limits. The CLI validates this document against live network
identity before creating authoritative state.

1. Preview initialization, then create the route directory with `--write`:

   ```sh
   docker run --rm --user "$(id -u):$(id -g)" \
     -v "$PWD/operator/routes:/data/routes" \
     -v "$PWD/operator/secrets:/run/templar-secrets:ro" \
     -v "$PWD/operator/input:/input:ro" \
     "$OFT_BRIDGE_IMAGE" \
     init \
     --desired /input/desired.json \
     --state /data/routes/example \
     --network stellar_testnet_sepolia \
     --stellar-rpc-file /run/templar-secrets/stellar-rpc \
     --evm-rpc-file /run/templar-secrets/evm-rpc

   ```

   Repeat the reviewed command with `--write` to create authoritative state.

2. Verify pinned artifacts and inspect the route:

   ```sh
   docker run --rm --user "$(id -u):$(id -g)" \
     -v "$PWD/operator/routes:/data/routes" \
     "$OFT_BRIDGE_IMAGE" artifact verify --state /data/routes/example

   docker run --rm --user "$(id -u):$(id -g)" \
     -v "$PWD/operator/routes:/data/routes" \
     "$OFT_BRIDGE_IMAGE" route inspect --state /data/routes/example
   ```

3. Preview each chain mutation by omitting both `--execute` and `--proposal-out`. Use
   `--proposal-out FILE` for an externally authorized testnet proposal, or `--execute` with the
   appropriate testnet signer inputs and shared operation store. Never modify a generated intent,
   proposal, operation log, or message log in place.

4. Quote a canary leg into a create-new intent, review it, then execute the same intent. The compact
   examples below show commands after the image entrypoint; append them after `"$OFT_BRIDGE_IMAGE"`
   in the mounted `docker run` form above:

   ```sh
   tmplr-oft-bridge leg quote \
     --state /data/routes/example \
     --direction stellar-to-evm \
     --amount-raw 1000000 \
     --to 0x0123456789abcdef0123456789abcdef01234567 \
     --out /data/routes/example/canary-intent.json

   docker run --rm \
     --user "$(id -u):$(id -g)" \
     --env STELLAR_OPERATOR_SECRET \
     --volume "$PWD/operator/routes:/data/routes" \
     --volume "$PWD/operator/operations:/data/operations" \
     --volume "$PWD/operator/secrets:/run/templar-secrets:ro" \
     "$OFT_BRIDGE_IMAGE" \
     leg send \
     --state /data/routes/example \
     --intent /data/routes/example/canary-intent.json \
     --operation-store-root /data/operations \
     --stellar-rpc-file /run/templar-secrets/stellar-rpc \
     --evm-rpc-file /run/templar-secrets/evm-rpc \
     --stellar-secret-env STELLAR_OPERATOR_SECRET \
     --execute
   ```

   Add `--allow-additional-obligation` only after explicitly reviewing the existing unsettled
   obligation. It does not waive the route's per-send or total outstanding caps.

5. Track the returned GUID until terminal delivery and reconcile custody:

   ```sh
   tmplr-oft-bridge message watch \
     --state /data/routes/example \
     --guid 0xGUID \
     --until terminal \
     --scan-url https://scan.layerzero-api.com \
     --stellar-rpc-file /run/templar-secrets/stellar-rpc \
     --evm-rpc-file /run/templar-secrets/evm-rpc

   tmplr-oft-bridge reconcile \
     --state /data/routes/example \
     --fail-on-deficit

   tmplr-oft-bridge health --state /data/routes/example
   ```

Use `message recover` only for a recorded GUID after reviewing its current stage. Contain outbound
traffic before recovery when custody or route configuration is uncertain.

## Production monitoring

[`ops/compose.yaml`](ops/compose.yaml) runs a credential-free route monitor around the packaged CLI.
On every interval it runs both `health` and `reconcile --fail-on-deficit` against the mounted route
state. Any health finding, custody deficit, corrupt log, unreadable state, or invalid configuration
causes a nonzero exit. Compose restarts the service, while the restart count, JSON logs, and Docker
health status remain available to an external alert manager.

The monitor has no network access and cannot sign or submit transactions. It evaluates the latest
authoritative state and custody observations already recorded by an operator workflow. Chain
watching and observation refresh must therefore run separately, with the narrow RPC and signer
access appropriate to those operations.

Copy the example environment file outside the repository, replace every path, and set the image to
the reviewed `sha256` digest published for the release:

```sh
cp tools/oft-bridge-cli/ops/monitor.env.example /etc/templar/oft-monitor.env
chmod 0600 /etc/templar/oft-monitor.env

docker compose \
  --env-file /etc/templar/oft-monitor.env \
  --file tools/oft-bridge-cli/ops/compose.yaml \
  config

docker compose \
  --env-file /etc/templar/oft-monitor.env \
  --file tools/oft-bridge-cli/ops/compose.yaml \
  run --rm route-monitor --once

docker compose \
  --env-file /etc/templar/oft-monitor.env \
  --file tools/oft-bridge-cli/ops/compose.yaml \
  up --detach
```

The routes and operation-store directories are durable operator records even though the monitor
mounts them read-only. Back them up together and mount the same directories into chain-connected
operator jobs. The monitor mounts no secrets or reviewed inputs, and Compose refuses to create any
missing bind source.

Alert on an unhealthy container, a rising restart count, or either `route health check failed` or
`custody reconciliation failed` in the service logs. A restart can restore monitoring after a
transient filesystem problem; it does not clear a finding or custody deficit.

## Image promotion and rollback

The publish workflow emits a version tag and a `sha-<commit>` tag. Resolve the reviewed image to its
registry digest after publication and record that digest with the deployment change. Production
Compose configuration must use `ghcr.io/templar-protocol/oft-bridge-cli@sha256:...`; mutable tags,
including version tags, are discovery aliases rather than deployment identities.

Promote an image by running the one-shot monitor against a read-only copy of production state,
reviewing its JSON output, then updating only `OFT_BRIDGE_IMAGE` to the approved digest and recreating
the service:

```sh
docker compose --env-file /etc/templar/oft-monitor.env \
  --file tools/oft-bridge-cli/ops/compose.yaml \
  up --detach --force-recreate
```

Keep the immediately previous approved digest in the deployment record. To roll back, restore that
exact digest, rerun the one-shot check, and recreate the service. Route and operation state are not
rolled back with the binary; they are append-only custody evidence and must continue from the latest
valid state.

## Mainnet proposal-only workflow

Mainnet operators should keep direct execution credentials out of this CLI environment. Prepare a
closed operation draft, create the chain-specific proposal, and verify that proposal against current
route state and live RPC observations. Stellar signatures and EVM Safe approvals happen in their
respective external governance systems; `proposal stellar-signature attach` only records a supplied
public signature in a new artifact. The CLI must never receive `--execute` for a mainnet route.

Use this sequence for each production change:

1. Run `health`, `reconcile --fail-on-deficit`, `artifact verify`, and `route inspect` against the
   current state.
2. Create a new operation artifact with `operation draft`; write every output to a create-new file.
3. Convert it with `proposal create`, then run `proposal stellar-signature verify` for Stellar or
   `proposal safe-verify` against the exact Safe transaction for EVM.
4. Submit the verified proposal through the external multisig or governance process.
5. After finality, run `proposal ingest --executed-tx ...` first as a preview and then with `--write`
   to bind the externally executed transaction into route history.
6. Run health and deficit-failing reconciliation again before preparing the next proposal.

Generated drafts and proposals are single-use review artifacts. A state, nonce, authority, code,
configuration, or simulation change invalidates the review; discard the artifact and create a new
one. Proposal creation, verification, and ingest do not grant authorization to execute a chain
transaction, and direct mainnet signing remains blocked.

## Command groups

| Command | Purpose |
| --- | --- |
| `init`, `adopt` | Create a new route state or bind verified existing deployments and opening custody. |
| `artifact verify`, `artifact build` | Verify the embedded artifact lock or prepare/build pinned OFT artifacts. |
| `asset wrap` | Plan or execute the Stellar lock/unlock and EVM mint/burn wrappers for a non-USDC asset. |
| `route ...` | Inspect or change peers, libraries, ULN/DVN settings, executors, and enforced options. |
| `authority ...` | Manage Stellar ownership/delegate and EVM ownership/delegate transitions. |
| `stellar ...` | Manage fees, rate limits, TTL, emergency pause, and Stellar OFT roles. |
| `contain ...` | Inspect, block, or restore outbound flow. |
| `leg quote`, `leg send` | Create an integrity-bound canary intent and submit the capped transfer. |
| `message watch`, `message recover` | Follow or recover a recorded LayerZero packet. |
| `evidence import` | Validate and append a historical custody evidence bundle. |
| `reconcile`, `health` | Check lockbox/supply accounting, packet state, configuration drift, and health findings. |
| `operation ...`, `proposal ...` | Create closed operations and externally authorized proposal artifacts. |

Run `tmplr-oft-bridge <group> --help` for the exact arguments supported by the installed version.

## Security and production boundary

The CLI is deliberately fail-closed:

- It recognizes only the release-reviewed Stellar-testnet/Sepolia and Stellar-mainnet/Ethereum
  environment classifications. Unknown or mixed identities fail classification, and initialization,
  adoption, and proposal creation verify both deployed endpoint code hashes against route state.
- Direct mainnet mutation and CLI-held signing return `production_mutation_unsupported_v1`.
  Mainnet proposal creation, attachment of externally produced public signatures, proposal
  verification, and finalized-transaction ingest are allowed only through the proposal workflow.
- USDC is rejected with `unsupported_use_cctp`; use Circle CCTP rather than this OFT route.
- A preview, draft, or proposal is not authorization. `--execute` is testnet-only and signs
  immediately; mainnet transactions must be authorized and submitted by external governance.
- Route state and its append-only logs are part of the custody proof. Restore them as one consistent
  backup and investigate any digest, sequence, nonce, duplicate-GUID, or log-chain failure.
- `health` is a point-in-time check. Run the supplied monitoring loop and schedule chain-connected
  `message watch` or observation refresh at a cadence appropriate for the route's finality and
  delivery limits.
- The container packages the operator binary, artifact lock, and wrapper inputs. The specialized
  `artifact build --write` flow additionally requires the lock-pinned source/dependency archives and
  qualified `forge` and `stellar` build tools; run that release-engineering step in the separately
  reviewed artifact builder environment. Ordinary operation does not require those compilers.

The contracts and RPC providers remain external trust boundaries. The CLI can validate configured
code hashes, effective settings, balances, receipts, and message evidence, but it cannot make a
compromised signer, endpoint, DVN, executor, RPC provider, or custodian trustworthy.

## Build from source

For developers working in this repository:

```sh
cargo build --locked --release \
  -p templar-oft-bridge-cli \
  --bin tmplr-oft-bridge

cargo test -p templar-oft-bridge-cli --all-targets
```

The workspace pins Rust in `rust-toolchain.toml`; use that toolchain and keep `Cargo.lock` unchanged
for a reproducible CLI build.
