use std::{
    collections::BTreeMap,
    fs,
    sync::{Mutex, MutexGuard},
};

use sha2::{Digest, Sha256};
use templar_soroban_shared_types::{
    VaultCommand as WireVaultCommand, RUNTIME_FEATURE_COMPANION_UPGRADE,
};

use crate::{
    artifacts::ArtifactSpec,
    cli::{
        AdapterArgs, AdapterCommand, ArtifactName, Cli, Commands, CuratorArgs, CuratorCommand,
        DeployArgs, DeployCommand, DeployCuratorProxyArgs, DeployPlanCommand, DeployStackArgs,
        ExtendTtlArgs, GovernanceArgs, GovernanceCommand, GovernanceSubmitAndWaitCommand,
        ShareTokenArgs, ShareTokenCommand, UserArgs, UserCommand, DEFAULT_CONTRACT_SOURCE_REPO,
    },
    manifest::{ContractRecord, Manifest},
    stellar::{CommandExecutor, CommandOutput, Stellar},
    types::{GovernanceActionKindArg, ShareDecimalsArg, SupplyQueueEntryArg},
};

use super::{
    context::CommandContext,
    deploy::*,
    doctor::manifest_writable_check,
    governance::{governance_proposal_view, proposal_matches_kind},
    inventory::*,
    invoke::supply_queue_entries_json,
    output::{
        DoctorStatus, OutputEnvelope, ParseErrorEnvelope, ReconcileStatus, Response, WiringStatus,
    },
    run,
    safety::guard_fresh_state_usage,
    CURATOR_PROXY_GOVERNANCE_ARG, CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG,
    CURATOR_PROXY_INITIALIZER_ARG, CURATOR_PROXY_LEGACY_V1_HASH_ARG, CURATOR_PROXY_VAULT_ARG,
    CURATOR_PROXY_VERSION_DISCOVERY_ARG,
};

const ACCOUNT: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
static CACHE_ENV_LOCK: Mutex<()> = Mutex::new(());

struct CacheEnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl CacheEnvGuard {
    fn set(path: &std::path::Path) -> Self {
        let lock = CACHE_ENV_LOCK.lock().expect("cache env lock");
        let previous = std::env::var_os(crate::artifacts::CACHE_ENV);
        // SAFETY: every in-process mutation of this variable is serialized by
        // CACHE_ENV_LOCK and restored before the guard releases the lock.
        unsafe { std::env::set_var(crate::artifacts::CACHE_ENV, path) };
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CacheEnvGuard {
    fn drop(&mut self) {
        // SAFETY: this guard still owns CACHE_ENV_LOCK, so no sibling test can
        // read or mutate the variable while its prior value is restored.
        if let Some(previous) = self.previous.take() {
            unsafe { std::env::set_var(crate::artifacts::CACHE_ENV, previous) };
        } else {
            unsafe { std::env::remove_var(crate::artifacts::CACHE_ENV) };
        }
    }
}
const CONTRACT: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";
const OTHER_CONTRACT: &str = "CBTLODGACWPBEZIDGHDLYQPGZDZRK4ITXHCET7EVPYAPP42CPIUDBUTK";
const ASSET_CONTRACT: &str = "CCW67TSZV3SSS2HXMBQ5JFGCKJNXKZM7UQUWUZPUTHXSTZLEO7SJMI75";

struct RecordingExecutor {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    runtime_feature_flags: u64,
    proxy_atomic_exits: bool,
}

impl RecordingExecutor {
    fn new() -> Self {
        Self::with_runtime_feature_flags(0x1f)
    }

    fn with_runtime_feature_flags(runtime_feature_flags: u64) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            runtime_feature_flags,
            proxy_atomic_exits: true,
        }
    }

    fn legacy_proxy() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            runtime_feature_flags: 0x1f,
            proxy_atomic_exits: false,
        }
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().expect("lock calls").clone()
    }
}

impl CommandExecutor for RecordingExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        _redacted_args: &[usize],
        _env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        self.calls
            .lock()
            .expect("lock calls")
            .push((program.to_string(), args.to_vec()));
        if matches!(args, [keys, address, ..] if keys == "keys" && address == "address") {
            return Ok(CommandOutput {
                stdout: ACCOUNT.to_string(),
                stderr: String::new(),
            });
        }
        if matches!(args, [contract, id, asset, ..] if contract == "contract" && id == "id" && asset == "asset")
        {
            return Ok(CommandOutput {
                stdout: ASSET_CONTRACT.to_string(),
                stderr: String::new(),
            });
        }
        if matches!(args, [contract, info, interface, ..] if contract == "contract" && info == "info" && interface == "interface")
        {
            let stdout = if self.proxy_atomic_exits {
                r#"[{"function_v0":{"name":"deposit_with_min"}},{"function_v0":{"name":"atomic_withdraw"}},{"function_v0":{"name":"atomic_redeem"}}]"#
            } else {
                r#"[{"function_v0":{"name":"deposit_with_min"}},{"function_v0":{"name":"withdraw"}},{"function_v0":{"name":"redeem"}}]"#
            };
            return Ok(CommandOutput {
                stdout: stdout.to_string(),
                stderr: String::new(),
            });
        }
        if args.iter().any(|arg| arg == "pending_ids") {
            return Ok(CommandOutput {
                stdout: "[1, 2]".to_string(),
                stderr: String::new(),
            });
        }
        if args.iter().any(|arg| arg == "version") {
            return Ok(CommandOutput {
                stdout: format!("[\"1.1.0\",{}]", self.runtime_feature_flags),
                stderr: String::new(),
            });
        }
        if args
            .iter()
            .any(|arg| arg == "submit_set_timelock" || arg == "submit_set_supply_queue")
        {
            return Ok(CommandOutput {
                stdout: "proposal 1".to_string(),
                stderr: String::new(),
            });
        }
        if args.iter().any(|arg| arg == "pending") {
            let proposal_id = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--proposal_id").then_some(pair[1].as_str()))
                .unwrap_or("0");
            let valid_after_ns = if proposal_id == "1" { 0 } else { u64::MAX };
            return Ok(CommandOutput {
                stdout: format!(
                    "{{id: {proposal_id}, action: SetPaused(false), valid_after_ns: {valid_after_ns}}}"
                ),
                stderr: String::new(),
            });
        }
        Ok(CommandOutput {
            stdout: CONTRACT.to_string(),
            stderr: String::new(),
        })
    }
}

struct TtlRecordingExecutor {
    inner: RecordingExecutor,
}

impl TtlRecordingExecutor {
    fn new() -> Self {
        Self {
            inner: RecordingExecutor::new(),
        }
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.inner.calls()
    }
}

impl CommandExecutor for TtlRecordingExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        if matches!(args, [contract, fetch, ..] if contract == "contract" && fetch == "fetch")
            && args.iter().any(|arg| arg == "--id")
        {
            let contract_id = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--id").then_some(pair[1].as_str()))
                .expect("contract fetch id");
            let output_path = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--out-file").then_some(pair[1].as_str()))
                .expect("contract fetch output path");
            let wasm = if contract_id.starts_with("CADAPTER") {
                b"shared blend adapter wasm".as_slice()
            } else {
                contract_id.as_bytes()
            };
            fs::write(output_path, wasm).expect("write fetched contract WASM");
        }
        self.inner.run(program, args, redacted_args, env)
    }
}

struct FailingVaultVersionExecutor {
    inner: TtlRecordingExecutor,
}

impl FailingVaultVersionExecutor {
    fn new() -> Self {
        Self {
            inner: TtlRecordingExecutor::new(),
        }
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.inner.calls()
    }
}

impl CommandExecutor for FailingVaultVersionExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        if args.iter().any(|arg| arg == "vault_version") {
            self.inner
                .inner
                .calls
                .lock()
                .expect("lock calls")
                .push((program.to_string(), args.to_vec()));
            anyhow::bail!("forced vault_version failure");
        }
        self.inner.run(program, args, redacted_args, env)
    }
}

struct FailingInitializeExecutor {
    inner: RecordingExecutor,
}

impl FailingInitializeExecutor {
    fn new() -> Self {
        Self {
            inner: RecordingExecutor::new(),
        }
    }
}

impl CommandExecutor for FailingInitializeExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        if args.iter().any(|arg| arg == "initialize") {
            self.inner
                .calls
                .lock()
                .expect("lock calls")
                .push((program.to_string(), args.to_vec()));
            anyhow::bail!("forced initialize failure");
        }
        self.inner.run(program, args, redacted_args, env)
    }
}

struct ChainStateExecutor {
    wasm: &'static [u8],
}

impl CommandExecutor for ChainStateExecutor {
    fn run(
        &self,
        _program: &str,
        args: &[String],
        _redacted_args: &[usize],
        _env: &[crate::stellar::CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        if matches!(args, [contract, id, asset, ..] if contract == "contract" && id == "id" && asset == "asset")
        {
            return Ok(CommandOutput {
                stdout: ASSET_CONTRACT.to_string(),
                stderr: String::new(),
            });
        }
        if args
            .windows(2)
            .any(|pair| pair[0] == "--id" && pair[1] == CONTRACT)
        {
            if let Some(path) = args
                .windows(2)
                .find_map(|pair| (pair[0] == "--out-file").then_some(&pair[1]))
            {
                fs::write(path, self.wasm).expect("write fetched wasm");
            }
            return Ok(CommandOutput {
                stdout: CONTRACT.to_string(),
                stderr: String::new(),
            });
        }
        anyhow::bail!("contract not found")
    }
}

fn submitted_calls(calls: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    calls
        .iter()
        .filter(|(_, args)| {
            !args
                .windows(2)
                .any(|pair| pair[0] == "--send" && pair[1] == "no")
                && !args.iter().any(|arg| arg == "--build-only")
                && !matches!(args.as_slice(), [first, second, ..] if first == "tx" && second == "simulate")
                && !matches!(args.as_slice(), [first, second, ..] if first == "contract" && second == "fetch")
                && !matches!(args.as_slice(), [first, second, ..] if first == "contract" && second == "info")
        })
        .cloned()
        .collect()
}

fn decoded_payload(calls: &[(String, Vec<String>)]) -> WireVaultCommand {
    let payload = calls
        .iter()
        .flat_map(|(_, args)| args.windows(2))
        .find_map(|pair| (pair[0] == "--payload").then_some(pair[1].as_str()))
        .expect("payload argument");
    let bytes = hex::decode(payload).expect("decode payload hex");
    WireVaultCommand::decode(&bytes).expect("decode vault command")
}

fn assert_protocol_ttl_call(calls: &[(String, Vec<String>)], selector: &str, value: &str) {
    assert!(calls.iter().any(|(program, args)| {
        program == "stellar"
            && matches!(args.as_slice(), [contract, extend, ..] if contract == "contract" && extend == "extend")
            && args
                .windows(2)
                .any(|pair| pair[0] == selector && pair[1] == value)
            && args
                .windows(2)
                .any(|pair| pair == ["--ledgers-to-extend", "3110400"])
    }));
}

fn assert_contract_invokes_are_views(calls: &[(String, Vec<String>)]) {
    for (_, args) in calls
        .iter()
        .filter(|(_, args)| args.windows(2).any(|pair| pair == ["contract", "invoke"]))
    {
        assert!(
            args.windows(2).any(|pair| pair == ["--send", "no"]),
            "contract invoke should use --send no: {args:?}"
        );
    }
}

fn base_cli(state: std::path::PathBuf, command: Commands) -> Cli {
    Cli {
        profile: None,
        network: "testnet".to_string(),
        rpc_url: None,
        network_passphrase: "Test SDF Network ; September 2015".to_string(),
        source_account: Some("alice".parse().expect("source account")),
        config_dir: None,
        contract_source_repo: Some(DEFAULT_CONTRACT_SOURCE_REPO.to_string()),
        state,
        fresh_state: false,
        workspace_path: ".".into(),
        json: true,
        json_lines: false,
        dry_run: false,
        yes: false,
        allow_mainnet_write: false,
        allow_zero_timelock: false,
        command,
    }
}

fn manifest_with_governance(path: &std::path::Path) {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest.save(path).expect("save manifest");
}

fn manifest_with_governance_and_vault(path: &std::path::Path) {
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest.save(path).expect("save manifest");
}

fn manifest_with_view_contracts(path: &std::path::Path) {
    let mut manifest = Manifest::new("testnet", None);
    for key in ["vault", "proxy_4626", "share_token", "blend_adapter_0"] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(CONTRACT));
    }
    manifest.save(path).expect("save manifest");
}

fn imported_record(contract_id: &str) -> ContractRecord {
    ContractRecord {
        contract_id: contract_id.to_string(),
        wasm_hash: "predeployed".to_string(),
        salt: None,
        constructor_args: BTreeMap::new(),
        deploy_tx: None,
        initialized: true,
    }
}

fn uninitialized_record(contract_id: &str) -> ContractRecord {
    ContractRecord {
        initialized: false,
        ..imported_record(contract_id)
    }
}

fn test_deploy_stack_args(admin: &str) -> DeployStackArgs {
    DeployStackArgs {
        admin: Some(admin.parse().expect("admin")),
        asset_token: Some(CONTRACT.parse().expect("asset token")),
        governance_timelock_ns: Some(1_000),
        virtual_shares: 0,
        virtual_assets: 0,
        share_name: "Templar Vault Share".to_string(),
        share_symbol: "tvSHARE".to_string(),
        share_decimals: 7,
        blend_pools: Vec::new(),
        custodians: Vec::new(),
        adapter_admin: None,
        build: true,
        force_new: false,
    }
}

fn write_fake_stack_wasms(root: &std::path::Path) {
    for artifact in [
        ArtifactName::Vault,
        ArtifactName::Governance,
        ArtifactName::ShareToken,
        ArtifactName::BlendAdapter,
        ArtifactName::CustodialAdapter,
        ArtifactName::Proxy4626,
        ArtifactName::CuratorProxy,
    ] {
        let path = ArtifactSpec::from_name(artifact).wasm_path(root);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, format!("{artifact:?}")).expect("write wasm");
    }
}

fn write_fake_blend_wasm(root: &std::path::Path) {
    let path = ArtifactSpec::from_name(ArtifactName::BlendAdapter).wasm_path(root);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, "blend").expect("write wasm");
}

fn write_fake_custodial_wasm(root: &std::path::Path) {
    let path = ArtifactSpec::from_name(ArtifactName::CustodialAdapter).wasm_path(root);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, "custodial").expect("write wasm");
}

fn write_fake_curator_proxy_wasm(root: &std::path::Path) {
    let path = ArtifactSpec::from_name(ArtifactName::CuratorProxy).wasm_path(root);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, "curator proxy").expect("write wasm");
}

#[path = "tests/deploy/adapters.rs"]
mod deploy_adapters;
#[path = "tests/deploy/curator_proxy.rs"]
mod deploy_curator_proxy;
#[path = "tests/deploy/plan.rs"]
mod deploy_plan;
#[path = "tests/deploy/reconcile.rs"]
mod deploy_reconcile;
#[path = "tests/deploy/session.rs"]
mod deploy_session;
#[path = "tests/deploy/stack.rs"]
mod deploy_stack;
#[path = "tests/deploy/wasm.rs"]
mod deploy_wasm;
#[path = "tests/doctor.rs"]
mod doctor;
#[path = "tests/facade.rs"]
mod facade;
#[path = "tests/governance.rs"]
mod governance;
#[path = "tests/output.rs"]
mod output;
#[path = "tests/ttl.rs"]
mod ttl;
#[path = "tests/vault_ops.rs"]
mod vault_ops;
