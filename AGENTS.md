# Agent Guide

This repository is a Rust workspace for Templar Protocol smart contracts, services, test helpers, and CLI tools.

## Repository Layout

- `common`: shared protocol logic used by contracts and tests
- `contract`: deployable smart contracts (`market`, `registry`, `vault`, `universal-account`, `lst-oracle`, `pyth-lazer`)
- `service`: standalone executables and off-chain services
- `tools`: operator and developer CLIs
- `test-utils`: shared test harness utilities
- `mock`: mock contracts used in tests
- `universal-account`: shared account crate
- `fuzz`: fuzz targets

## High-Impact Areas

Use this section as an execution checklist: read the local docs first, preserve the listed invariants, and run at least the listed checks.

- `common` (`templar-common`)
  Read/inspect: `common/src/borrow.rs`, `common/src/market/impl.rs`, `common/src/event.rs`, `common/src/vault/mod.rs`.
  Why it matters: this crate is the protocol source of truth. Accounting, event schemas, oracle types, borrow/supply logic, and shared vault interfaces all live here.
  Watch for: arithmetic edge cases, state-transition semantics, serialization changes, and event-schema drift. A small change here can silently alter multiple contract APIs.
  Minimum verification: `cargo test -p templar-common --lib -- --nocapture`.
- `contract/market` (`templar-market-contract`)
  Read/inspect: `contract/market/src/lib.rs`, `contract/market/src/impl_market_external.rs`, plus the corresponding logic in `templar_common::market`.
  Why it matters: the deployable contract is thin, but it adds NEP-145 storage behavior and wraps asynchronous borrow/collateral/withdraw flows around shared market logic.
  Watch for: storage charging/refunds, `storage_unregister` implications, force-unregister behavior, cross-contract finalize paths, and in-flight accounting.
  Minimum verification: `cargo test -p templar-common --lib -- --nocapture`; if contract entrypoints or callbacks changed, also run `cargo test -p templar-market-contract -- --nocapture`.
- `contract/vault` (`templar-vault-contract`)
  Read first: `contract/vault/README.md` and `contract/vault/near/README.md`.
  Read/inspect: `contract/vault/src/lib.rs`, `contract/vault/src/impl_callbacks.rs`, `contract/vault/src/governance.rs`, `common/src/vault/*`.
  Why it matters: this is the most complex state machine in the repository and the highest-risk place for async accounting bugs.
  Watch for: `OpState` transitions, escrow accounting, keeper-routed withdrawals, callback ordering, idle-balance resync, fee accrual, and reconciliation after partial failures.
  Minimum verification: `cargo test -p templar-vault-contract -- --nocapture`.
- `contract/registry` (`templar-registry-contract`)
  Read/inspect: `contract/registry/src/lib.rs`.
  Why it matters: this is a deployment/orchestration contract, not just a map of version keys.
  Watch for: the distinction between `Reserved` and `Deployed`, deployment finalization paths, soft deletion of version code, and failure cleanup after partial deploy flows.
  Minimum verification: `cargo test -p templar-registry-contract -- --nocapture`.
- `contract/universal-account` (`templar-universal-account-contract`) and `universal-account` (`templar-universal-account`)
  Read first: `contract/universal-account/README.md`.
  Read/inspect: `contract/universal-account/src/lib.rs`, `contract/universal-account/src/impl_migrate.rs`, and the shared transaction/signature code in `universal-account`.
  Why it matters: these crates define authentication, signature verification, nonce progression, transaction execution, and migration behavior.
  Watch for: replay protection, signing payload compatibility, migration compatibility, supported signature schemes, and any wire-format changes.
  Minimum verification: `cargo test -p templar-universal-account-contract -- --nocapture`.
- `service/relayer` (`templar-relayer`)
  Read first: `service/relayer/README.md`.
  Why it matters: this service is an operational security boundary for delegated actions and universal-account flows.
  Watch for: allowed-method changes, nonce handling, gas settings, SQL query changes, storage-deposit behavior, and universal-account deployment/execution integration.
  Minimum verification: run the narrowest relevant `cargo test -p templar-relayer ...`; if SQL changes, update prepared queries as documented in the README.
- `contract/pyth-lazer` (`templar-pyth-lazer-verifier` / `templar-pyth-lazer-adapter-contract`)
  Read first: `contract/pyth-lazer/README.md`, `contract/pyth-lazer/SPEC.md`, `contract/pyth-lazer/TRUSTED_SIGNERS.md`.
  Read/inspect: `contract/pyth-lazer/verifier/src/verify.rs`, `contract/pyth-lazer/contract/src/lib.rs`, `contract/pyth-lazer/contract/src/events.rs` (`FeedData` + its price projections live in `common/src/oracle/lazer.rs`).
  Why it matters: a feed-id-native Lazer price oracle read by the proxy-oracle's `Lazer` source — forged, stale, or mis-scaled prices flow straight into borrow accounting. The wire parser is a forked `pyth-lazer-protocol` pinned by fork tag (Cargo.lock records the commit); bumps are security-sensitive.
  Watch for: signer/trust/expiry + ed25519 checks, canonical-encoding (`NonCanonical`) rejection, the freshness window and per-feed monotonic anti-replay, confidence/EMA discipline, `SignerSet` invariants, and storage-fee/refund.
  Minimum verification: `cargo test -p templar-pyth-lazer-verifier -p templar-pyth-lazer-adapter-contract`; `cargo check --target wasm32-unknown-unknown -p templar-pyth-lazer-adapter-contract`.
- `contract/proxy-oracle/soroban/pyth-lazer-source-contract` (`templar-proxy-oracle-soroban-pyth-lazer-source-contract`)
  Read first: `contract/proxy-oracle/soroban/README.md` (PythLazerSource section), `contract/proxy-oracle/soroban/AUDIT.md`.
  Read/inspect: `contract/proxy-oracle/soroban/pyth-lazer-source-contract/src/lib.rs`.
  Why it matters: it is a SEP-40 source in the Soroban proxy oracle's price path. Pyth's on-chain verifier only checks the signature; replay, ordering, and freshness are enforced here, so a gap ships a stale or replayed price into aggregation. The payload parser is Pyth's `pyth-lazer-stellar-sdk` vendored into the `pyth-lazer-public` fork and pinned by fork tag (Cargo.lock records the commit); bumps are security-sensitive.
  Watch for: the per-asset strictly-advancing publish time, the payload freshness window and channel filter, the positive-price / exponent-present skip rules, the `decimals` rescale in `lastprice`, and the 32 KiB size budget.
  Minimum verification: `cargo test -p templar-proxy-oracle-soroban-pyth-lazer-source-contract --features testutils`; `cargo test -p templar-proxy-oracle-soroban-integration-tests --test lazer_source`.
- `gateway/*` (the Templar gateway: `templar-gateway-*`)
  Read first: `gateway/README.md` (RPC naming) and `gateway/METHODS.md` (the generated catalog of every method: kind, input → output, summary).
  Why it matters: the gateway is the single standardized implementation of NEAR reads and writes (planning, signing, multi-step finalization, idempotency/replay). Rust consumers integrate it in-process via `templar-gateway-client`; the JSON-RPC service is for non-Rust clients.
  Watch for: when migrating a consumer onto the gateway, diff it against the original operation-by-operation and map each call by **semantics, not name**. Prefer domain/standard-agnostic methods over low-level ones (e.g. `token.transfer`, which dispatches NEP-141 vs NEP-245, over `ft.transfer` — an asset may be a multi-token). If the gateway lacks a method a consumer needs, add it to the gateway rather than hand-rolling a NEAR call in the consumer. The method lists are canonical in the spec crates' `for_each_*_method!` macros (the RPC service registration and `METHODS.md` both expand them); add or remove a method's line there whenever you add or remove a method — it is the only step, and a removed spec left in the list is a compile error.
  Minimum verification: `cargo check --workspace`; `cargo test -p templar-gateway-catalog` (keeps `METHODS.md` in sync — regenerate with `cargo test -p templar-gateway-catalog regenerate_methods_md -- --ignored`); plus the narrowest relevant `cargo test -p templar-gateway-<crate> -- --nocapture`.

## Working Norms

- Prefer small, targeted changes over broad refactors.
- Never fully delete and recreate an existing file when editing. Apply small, in-place patches that preserve unaffected content.
- Do not revert unrelated user changes in the worktree.
- Treat `common` changes as high-impact: they often affect multiple contracts and tests.
- Keep event/schema changes deliberate. If a public event or JSON payload changes, check versioning and downstream compatibility.
- Preserve existing crate structure and naming patterns unless there is a strong reason to change them.
- This codebase is security-sensitive. Review changes with an auditor mindset, especially in smart contracts and cross-contract flows.

## Commit And PR Titles

PRs are squash-merged, so **the PR title becomes the commit message on `dev`**, and `release-plz` reads those messages to decide each crate's next version. A title that does not parse still bumps — release-plz falls back to a patch — but it lands with **no changelog entry**, so the release says nothing about what changed. `.github/workflows/pr-title.yml` enforces the format on every PR.

Format: `type(scope): summary`

- **Allowed types** — `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `ci`, `build`, `chore`, `deploy`. Keep this list in sync with `commit_parsers` in `release-plz.toml` and `types` in `.github/workflows/pr-title.yml`.
- **Version impact** — `feat` bumps minor and a breaking change bumps major. *Every* other type, `chore` and `docs` included, still bumps patch: release-plz releases any crate with commits since its last tag, and `skip = true` in `release-plz.toml` only keeps an entry out of the changelog, it does not suppress the bump. On a pre-1.0 crate a plain `feat` bumps patch too (`0.1.0` → `0.1.1`), since under Cargo's rules a minor bump there would signal a break.
- **Breaking changes** — mark with `!` after the type or scope: `feat(gateway)!: drop the legacy read path`. Put it in the **title**, not the PR description — this repo squashes with `COMMIT_MESSAGES`, so the PR body never reaches the commit and a `BREAKING CHANGE:` footer written there is silently discarded. Forgetting the `!` ships a breaking change as a compatible bump — minor at best, patch on a pre-1.0 crate or a non-`feat` type.
- **Scope is optional and free-form.** It is conventional to name the crate or area (`gateway`, `market`, `vault`, `relayer`, `proxy-oracle`, `manager`), but nothing enforces a fixed list: release-plz determines *which* crate to bump from the files a commit touches, not from the scope.
- **Never start a title with a Linear ID.** `ENG-504: Nest governance under proxy-oracle` leads with no valid type, so the lint rejects it and release-plz falls back to a patch with no changelog entry — write `refactor(proxy-oracle): nest governance`. A trailing reference (`… (ENG-504)`) is fine: the type still parses, and the ID carries into the changelog.

Releases themselves are cut by merging the standing release PR, which release-plz titles `chore: release`. See `RELEASING.md`.

## Build And Test

- Format: `cargo fmt`
- Fast gate (everyday inner loop): `just test-fast`. Complete non-node partition, including integration targets. The recipe provisions Postgres when needed; `fast_filter` in the root `justfile` owns the test selection. It makes no network calls of its own, but a few tests deploy released contract WASM — run `just artifacts-fetch` once (CI does) and a warm cache keeps the gate offline thereafter.
- Node gate: `just test-sandbox`. The recipe provisions Postgres, narrows Cargo to the node-backed packages, prebuilds test Wasms, and manages a pooled out-of-band `neard`. Pass `--stale` (also accepted by `just test` and `just sandbox-up`) to reuse the Wasms already in `target/near` instead of rebuilding them.
- Full local gate: `just test`, which runs the same fast and sandbox entrypoints used by CI.
- Common crate: `cargo test -p templar-common --lib -- --nocapture`
- One test file: `cargo test -p <package> --test <name> -- --nocapture`
- One unit test: `cargo test -p <package> <test_name> -- --nocapture`

Notes:

- Node-backed integration tests attach to a `SandboxHarness` (`gateway/testing/src/sandbox.rs`) instead of each booting its own sandbox. Raw sandbox RPC/config belongs solely to publishable `gateway/sandbox` (`templar-sandbox`); `gateway/testing` re-exports it. Under the sandbox gate they attach over RPC to the shared `neard` pool, one node per `NEXTEST_TEST_GLOBAL_SLOT`. With no pool running the harness falls back to _owned_ mode and starts its own `neard` per test — acceptable for a single test, but slow and prone to nonce contention across a whole file, so prefer `just test-sandbox` for more than one node test.
- The sandbox gate derives test selection and Cargo package narrowing from one package classification. See "Cross-Cutting Lists To Keep in Sync" below before adding a node-backed crate. If these tests fail because no neard is available, say that clearly instead of silently skipping them. `templar-sandbox` minimum verification is `cargo test -p templar-sandbox`; the full acceptance gate is `just test-sandbox`.
- `cargo test -p templar-common --lib` is a good fast regression check for logic changes in `common`.
- Node-backed tests need the contract Wasms prebuilt; rebuilding WASM inside each run is much slower. `just test-sandbox` sources `script/sandbox-up.sh`, which prebuilds them and exports `TEST_CONTRACTS_PREBUILT=1`. If you run a node test by hand outside that recipe (e.g. a plain `cargo test` in owned mode), do the same first. `--stale` works by setting that same variable on the way in: the script then only verifies the artifacts exist (`prebuild-test-contracts --check`) and fails before booting nodes if any are missing.
- The sandbox harness runs `neard` at a 40ms block delay locally but **CI pins the stock 120ms** (`NEAR_SANDBOX_BLOCK_MS` in `.github/workflows/test.yml`) — a 4-vCPU runner cannot sustain the faster cadence. Local blocks are therefore ~3× faster than CI's, so a test that relies on _incidental_ block cadence to cross a time boundary (time chunks, TTLs, interest accrual) passes locally and fails on CI. Advance chain time explicitly with `fast_forward`. `avg(min, max)` block delay is held at 310ms whatever the cadence, so `fast_forward`'s simulated time advance does not change.
- Before optimizing anything about node-test speed, read "Test-Gate Timing" in `docs/src/testing.md` and measure with `just bench-sandbox` — several plausible ideas (installing contract code via `sandbox_patch_state`, global contracts) are measured dead ends recorded there. `just test-sandbox` writes per-test timings to `target/nextest/sandbox/junit.xml`; `script/bench/junit-diff.py` compares two such runs.
- Run `./script/check-artifact-drift.sh` when validating the artifact catalog; it is pure and build-free (seconds), covering release-list well-formedness, ordering, and that no release claims a version its crate never reached.
- Released contract bytes are **GitHub Release assets, not repository content**: each release records the tag and asset that carry it. `templar-contract-artifacts`'s `fetch` feature downloads them into a shared cache outside the repo (`just artifacts-fetch` warms it; CI does this before the sandbox tests) and verifies them against the SHA-256 pinned in `contract/artifacts/releases/`. Releases are immutable — ship new bytes by *adding* a release, never editing one, since historical bytes back the migration and upgrade tests.
- **A release means a release tag was cut and its WASM published, not that a version was bumped** — those diverge routinely (market's crate version reached 1.4.0 while 1.3.0 was the newest release). Source is expected to run ahead of the newest release. An entry is the canonical build for that version, not evidence it runs on a chain. The release list in `contract/artifacts/releases/` is appended to by CI and is never hand-edited.
- **Cutting the blob is deliberate; a tag with no WASM is expected.** release-plz bumps every contract that links a released library, so most tags are versions nobody ships. `just release-wasm <tag>` dispatches `.github/workflows/release-artifacts.yml` for a version you intend to deploy, and `just release-wasm-status <tag>` says whether one was already built. Building *at the tag's commit* is what keeps the NEP-330 commit permanently reachable for external verifiers such as nearblocks.io — a squash-merged feature-branch commit is not. Nothing is lost by building late: the tag is permanent and the build reproducible, so the bytes are the same whenever it runs.
- A contract binary built without a release tag behind it is **not** a release — it is test data. `contract/universal-account/tests/migration/0_4_0.wasm` is the one such fixture, sitting beside the state patch it pairs with.

## Cross-Cutting Lists To Keep In Sync

Several CI/test-infra files enumerate crates, contracts, or paths by hand. A feature change elsewhere easily leaves one stale, and the failure is silent — tests that never run, jobs that never trigger. When your change matches a trigger below, update the listed files in the _same_ change.

- **Adding a node-backed test crate** (tests that need a `SandboxHarness`):
  - `justfile` — add the package to `sandbox_full_packages`; the sandbox filter and Cargo package boundary are both derived from that list. (`templar-gateway-service`'s node tests live in `src/` and are matched separately by module path.)
  - `.github/workflows/test.yml` — add the crate's `src/**` and `tests/**` under the `changes` job's `near_integration` paths filter, or the test job won't trigger on changes to it.
- **Adding a node-backed test inside a `src/` unit-test module**: the package list above selects `kind(test)` targets only, so name the test `requires_sandbox_*`. Without the prefix it stays in the fast gate, which runs no `neard` and installs no `cargo-near`.
- **Adding or removing a contract / mock WASM**:
  - `contract/artifacts/src/ids.rs` — the `ArtifactId` enum and each artifact's name/path. `contract/artifacts/releases/` holds the released history, compiled in by `build.rs` (empty for mocks, which are never released). See `contract/artifacts/README.md`. This is the canonical list; `script/prebuild-test-contracts.sh` derives from it.
  - `gateway/testing/src/wasm.rs` — the `wasm_fns!` list, so the harness can load it.
- **Releasing a new version of an existing contract**: merging the Release PR tags it but builds nothing. For a version you intend to deploy, run `just release-wasm <tag>`; CI then builds the WASM reproducibly at that tag, uploads it, and adds one file under `contract/artifacts/releases/` on the catalog PR that shared batch of builds. Tags you never ship keep no WASM, which is expected. See `RELEASING.md`.
- **Adding a new top-level source area / crate**:
  - `.github/workflows/test.yml` paths groups (`near_integration`, `soroban`, `feature_matrix`, `artifact_manifests`) and `.github/workflows/gas-report.yml` paths — add it to the right group so the relevant jobs fire.
  - Root `Cargo.toml` `[workspace] members` if an existing glob (`gateway/*`, `tools/*`, …) doesn't already cover the path.
- **Tuning sandbox parallelism**: update `sandbox_test_threads` in `justfile`; the sandbox recipe uses it for both Nextest threads and pooled node count.
- **Adding or removing a gateway method**: update the `for_each_*_method!` macros in the spec crates — see the `gateway/*` entry under High-Impact Areas.

## Code Search

- Use `rg` for text search.
- Use `rg --files` to find files.
- When reviewing behavior changes, check both staged and unstaged diffs if the worktree is dirty.

## Rust Conventions

- Prefer parsing over validation. Express invariants in types wherever practical, and make invalid states unrepresentable.
- Prefer stronger types over loosely constrained values: enums over well-known strings, sets over vectors when uniqueness matters, dedicated newtypes over primitive obsession, and structured state machines over ad hoc flags.
- Follow existing error-handling patterns with `anyhow`, `thiserror`, and `require!`/panic helpers already used in the codebase.
- Prefer a functional, pure style when it remains readable and efficient. Favor transformations over mutation, message passing over locks, and declarative code over imperative code. If the purely functional version would be materially less readable or obviously less efficient, prefer the simpler maintainable implementation.
- Keep the codebase DRY. When non-trivial logic is repeated, consider extracting a helper, module, or shared crate instead of duplicating it.
- Avoid shorthands and abbreviations in identifiers unless the shorter form is already standard and clearer.
- Avoid introducing `unwrap()` in non-test code unless the surrounding file already relies on an invariant and documents it clearly.
- Keep serialization explicit for contract-facing and event-facing structs.
- When changing emitted events, verify the payload compiles and still reflects the intended business semantics.

## Validation Expectations

- If you change public events, contract methods, or shared structs, run at least the narrowest relevant crate tests.
- If you change a high-impact crate, use the crate-specific verification command from the "High-Impact Areas" section unless you have a good reason not to.
- Write comprehensive unit tests for new, non-trivial logic. Prefer `#[rstest]` parameterization when it improves coverage and keeps cases readable.
- Keep fuzzers current when behavior changes affect parsing, arithmetic, state transitions, or other high-risk logic. Run them periodically, not only after major rewrites.
- If you cannot run an important verification step, say so explicitly.

## Documentation

- Pay attention to documentation comments, READMEs, and Markdown documents across the repository. Update them when behavior, interfaces, or operational expectations change.
- Read crate-local documentation before changing high-impact areas, especially `contract/vault/README.md`, `contract/vault/near/README.md`, `contract/universal-account/README.md`, and `service/relayer/README.md`.
- Keep this `AGENTS.md` file up to date when repository workflows, verification steps, important invariants, or high-impact crate guidance change in a way that would matter to future agents.

## Security Notes

- Treat every code change as potentially security-relevant.
- Evaluate edge cases around asynchronous receipt execution and cross-contract call ordering. Watch for TOCTOU-style issues.
- Be careful with gas usage and callback chains. Out-of-gas behavior in cross-contract flows can produce surprising partial-failure states.
- On NEAR, storage registration and unregistration behavior matters. In particular, consider the consequences of accounts calling `storage_unregister` and then becoming unable to receive returned NEP-141 assets or interact with NEP-145-aware contracts.
- Check invariants around refunds, withdrawals, account deletion, authorization, and replay or double-execution risks.
