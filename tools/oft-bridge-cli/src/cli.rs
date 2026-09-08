use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::{
    canonical_sha256,
    config::SecretProvider,
    domain::{
        AssetKind, DesiredRouteV1, Direction, Environment, OperationDraftV1, OperationV1, Vm,
        SCHEMA_VERSION,
    },
    environment,
    error::{Error, Result},
    evm::EvmChain as _,
    output::CommandData,
    state::{read_json, write_create_new_json, RouteStore},
};

#[derive(Debug, Parser)]
#[command(
    name = "tmplr-oft-bridge",
    version,
    about = "Non-USDC LayerZero OFT route operator"
)]
pub struct Cli {
    /// Emit the stable JSON envelope (accepted; v1 already emits JSON for
    /// every command). Retained so existing `--json` invocations parse and
    /// so `main_entry` can render parse failures as JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(flatten)]
    rpc: RpcArgs,
    /// Mode-0600 non-symlink JSON object containing RPC headers.
    #[arg(long, global = true)]
    rpc_headers_file: Option<PathBuf>,
    /// Named environment provider for the Stellar signing secret.
    #[arg(long, global = true)]
    stellar_secret_env: Option<String>,
    /// Mode-0600 non-symlink Foundry V3 keystore.
    #[arg(long, global = true, requires = "evm_password_file")]
    evm_keystore: Option<PathBuf>,
    /// Mode-0600 non-symlink keystore password provider.
    #[arg(long, global = true, requires = "evm_keystore")]
    evm_password_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(InitArgs),
    Adopt(AdoptArgs),
    Operation(OperationArgs),
    Proposal(ProposalArgs),
    Artifact(ArtifactArgs),
    Asset(AssetArgs),
    Route(RouteArgs),
    Authority(AuthorityArgs),
    Stellar(StellarArgs),
    Contain(ContainArgs),
    Leg(LegArgs),
    Message(MessageArgs),
    Evidence(EvidenceArgs),
    Reconcile(ReconcileArgs),
    Health(StateOnlyArgs),
}

#[derive(Debug, Args)]
struct ReconcileArgs {
    #[arg(long)]
    state: PathBuf,
    /// Exit nonzero when custody reports any deficit.
    #[arg(long)]
    fail_on_deficit: bool,
}

#[derive(Clone, Debug, Args)]
struct RpcArgs {
    /// Environment variable holding the Stellar RPC URL.
    #[arg(long, global = true)]
    stellar_rpc_env: Option<String>,
    /// Mode-0600 non-symlink file holding the Stellar RPC URL.
    #[arg(long, global = true, conflicts_with = "stellar_rpc_env")]
    stellar_rpc_file: Option<PathBuf>,
    /// Environment variable holding the EVM RPC URL.
    #[arg(long, global = true)]
    evm_rpc_env: Option<String>,
    /// Mode-0600 non-symlink file holding the EVM RPC URL.
    #[arg(long, global = true, conflicts_with = "evm_rpc_env")]
    evm_rpc_file: Option<PathBuf>,
}

impl RpcArgs {
    fn stellar_url(&self) -> Result<Option<String>> {
        rpc_url(
            self.stellar_rpc_env.as_ref(),
            self.stellar_rpc_file.as_ref(),
        )
    }

    fn evm_url(&self) -> Result<Option<String>> {
        rpc_url(self.evm_rpc_env.as_ref(), self.evm_rpc_file.as_ref())
    }
}

/// Resolves an RPC URL from one credential-safe provider. Values never enter
/// argv or durable state.
fn rpc_url(env_var: Option<&String>, file: Option<&PathBuf>) -> Result<Option<String>> {
    match (env_var, file) {
        (Some(name), None) => SecretProvider::Environment(name.clone())
            .read()
            .map(|value| Some(value.to_string())),
        (None, Some(path)) => SecretProvider::File(path.clone())
            .read()
            .map(|value| Some(value.to_string())),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => unreachable!("clap enforces provider exclusivity"),
    }
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    write: bool,
    /// Operator assertion of the target environment; must equal the
    /// identity-derived environment.
    #[arg(long)]
    network: Option<String>,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Debug, Args)]
struct AdoptArgs {
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    stellar_oft: String,
    #[arg(long)]
    evm_oft: String,
    #[arg(long)]
    write: bool,
    /// Finalized zero-history proof or imported-history custody baseline.
    #[arg(long)]
    opening_custody: PathBuf,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Debug, Args)]
struct StateOnlyArgs {
    #[arg(long)]
    state: PathBuf,
}

#[derive(Debug, Args)]
struct OperationArgs {
    #[command(subcommand)]
    command: OperationCommand,
}

#[derive(Debug, Subcommand)]
enum OperationCommand {
    Draft(DraftArgs),
}

#[derive(Debug, Args)]
struct DraftArgs {
    #[arg(long)]
    state: PathBuf,
    /// Closed operation command name (see OperationV1 variants, kebab-case).
    #[arg(long)]
    command: String,
    /// JSON file with the closed argument set for --command.
    #[arg(long)]
    args: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalArgs {
    #[command(subcommand)]
    command: ProposalCommand,
}

#[derive(Debug, Subcommand)]
enum ProposalCommand {
    Create(ProposalCreateArgs),
    Ingest(ProposalIngestArgs),
    StellarSignature(ProposalSignatureArgs),
    SafeVerify(ProposalSafeArgs),
}

#[derive(Debug, Args)]
struct ProposalCreateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    draft: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Debug, Args)]
struct ProposalIngestArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    executed_tx: String,
    #[arg(long)]
    write: bool,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Debug, Args)]
struct ProposalSignatureArgs {
    #[command(subcommand)]
    command: ProposalSignatureCommand,
}

#[derive(Debug, Subcommand)]
enum ProposalSignatureCommand {
    Attach(ProposalAttachArgs),
    Verify(ProposalVerifyArgs),
}

#[derive(Debug, Args)]
struct ProposalAttachArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    public_key: String,
    #[arg(long)]
    signature: String,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalVerifyArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
}

#[derive(Debug, Args)]
struct ProposalSafeArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    proposal: PathBuf,
    #[arg(long)]
    safe_tx: PathBuf,
    #[command(flatten)]
    rpc: RpcArgs,
}

#[derive(Debug, Args)]
struct ArtifactArgs {
    #[command(subcommand)]
    command: ArtifactCommand,
}

#[derive(Debug, Subcommand)]
enum ArtifactCommand {
    Verify(StateOnlyArgs),
    Build(ArtifactBuildArgs),
}

#[derive(Debug, Args)]
struct ArtifactBuildArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    out_dir: PathBuf,
    #[arg(long)]
    write: bool,
    /// Digest-verified build dependency archive (npm package closure).
    #[arg(long)]
    deps_archive: Option<PathBuf>,
    /// Digest-verified resolved LayerZero Stellar OFT source closure.
    #[arg(long)]
    source_archive: Option<PathBuf>,
}
#[derive(Debug, Args)]
struct AssetArgs {
    #[command(subcommand)]
    command: AssetCommand,
}

#[derive(Debug, Subcommand)]
enum AssetCommand {
    Wrap(WrapArgs),
}

#[derive(Debug, Args)]
struct WrapArgs {
    /// Asset identifier, asserted against the bound route's asset.
    #[arg(long)]
    asset: String,
    #[arg(long, value_enum)]
    asset_kind: AssetKindArg,
    #[arg(long)]
    state: PathBuf,
    /// Desired route JSON binding the operator topology and evidence.
    #[arg(long)]
    desired: PathBuf,
    #[arg(long)]
    name: String,
    #[arg(long)]
    symbol: String,
    /// Reserved live EVM deployer nonce; fetched via --evm-rpc-env when absent.
    #[arg(long)]
    evm_nonce: Option<u64>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AssetKindArg {
    NativeSac,
    IssuedSep41,
    Usdc,
}

impl From<AssetKindArg> for AssetKind {
    fn from(value: AssetKindArg) -> Self {
        match value {
            AssetKindArg::NativeSac => Self::NativeSac,
            AssetKindArg::IssuedSep41 => Self::IssuedSep41,
            AssetKindArg::Usdc => Self::Usdc,
        }
    }
}

#[derive(Debug, Args)]
struct RouteArgs {
    #[command(subcommand)]
    command: RouteCommand,
}

#[derive(Debug, Subcommand)]
enum RouteCommand {
    DraftConfig(OutputStateArgs),
    Inspect(StateOnlyArgs),
    Apply(ApplyRouteArgs),
    SetPeer(SetPeerArgs),
    SetLibrary(SetLibraryArgs),
    RemoveReceiveTimeout(VmRemoteArgs),
    SetUln(ConfigHashArgs),
    SetExecutor(ConfigHashArgs),
    SetOptions(SetOptionsArgs),
}

#[derive(Debug, Args)]
struct OutputStateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Debug, Args)]
struct ApplyRouteArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    config: PathBuf,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct SetPeerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    peer: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct SetLibraryArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    direction: String,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    library: String,
    #[arg(long)]
    grace_period_seconds: Option<u64>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct VmRemoteArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct ConfigHashArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    /// ULN config direction; required for set-uln, rejected for set-executor.
    #[arg(long)]
    direction: Option<String>,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    config: PathBuf,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct SetOptionsArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    vm: VmArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    message_type: u16,
    #[arg(long)]
    options: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum VmArg {
    Stellar,
    Evm,
}
impl From<VmArg> for Vm {
    fn from(value: VmArg) -> Self {
        if matches!(value, VmArg::Stellar) {
            Vm::Stellar
        } else {
            Vm::Evm
        }
    }
}

#[derive(Debug, Args)]
struct AuthorityArgs {
    #[command(subcommand)]
    command: AuthorityCommand,
}
#[derive(Debug, Subcommand)]
enum AuthorityCommand {
    StellarBeginOwner(StellarBeginOwnerArgs),
    StellarAcceptOwner(StateEffectArgs),
    StellarCancelOwner(StateEffectArgs),
    StellarSetDelegate(StellarSetDelegateArgs),
    EvmTransferOwner(EvmTransferOwnerArgs),
    EvmSetDelegate(EvmSetDelegateArgs),
}
#[derive(Debug, Args)]
struct StellarBeginOwnerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    new_owner: String,
    #[arg(long, default_value_t = 0)]
    ttl: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct StateEffectArgs {
    #[arg(long)]
    state: PathBuf,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct StellarSetDelegateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    delegate: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct TtlFreezeArgs {
    #[arg(long)]
    state: PathBuf,
    /// Exact acknowledgement that freezing TTL configuration is irreversible.
    #[arg(long)]
    acknowledge_irreversible: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct EvmTransferOwnerArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    new_owner: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct EvmSetDelegateArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    delegate: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct StellarArgs {
    #[command(subcommand)]
    command: StellarCommand,
}
#[derive(Debug, Subcommand)]
enum StellarCommand {
    SetFee(SetFeeArgs),
    SetFeeDepositAddress(SetFeeDepositAddressArgs),
    SetMessageInspector(SetMessageInspectorArgs),
    SetRateLimit(SetRateLimitArgs),
    TtlSet(TtlSetArgs),
    TtlFreeze(TtlFreezeArgs),
    TtlExtendInstance(TtlExtendArgs),
    EmergencyPause(StateEffectArgs),
    EmergencyUnpause(StateEffectArgs),
    RoleGrant(RoleArgs),
    RoleRevoke(RoleArgs),
    RoleSetAdmin(RoleAdminArgs),
}

#[derive(Debug, Args)]
struct RoleAdminArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    role: String,
    #[arg(long)]
    admin_role: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetFeeArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bps: u32,
    #[arg(long)]
    remote_eid: Option<u32>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetFeeDepositAddressArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    recipient: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetMessageInspectorArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    inspector: Option<String>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct SetRateLimitArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: RateDirectionArg,
    #[arg(long)]
    remote_eid: u32,
    #[arg(long)]
    limit_raw: u128,
    #[arg(long)]
    window_seconds: u64,
    #[arg(long, default_value = "net")]
    mode: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct TtlSetArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    instance_threshold: u32,
    #[arg(long)]
    instance_extend_to: u32,
    #[arg(long)]
    persistent_threshold: u32,
    #[arg(long)]
    persistent_extend_to: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct TtlExtendArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    ledgers: u32,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct RoleArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    role: String,
    #[arg(long)]
    address: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum RateDirectionArg {
    Inbound,
    Outbound,
}

#[derive(Debug, Args)]
struct ContainArgs {
    #[command(subcommand)]
    command: ContainCommand,
}
#[derive(Debug, Subcommand)]
enum ContainCommand {
    Outbound(ContainOutboundArgs),
    Inspect(StateOnlyArgs),
    Restore(ContainRestoreArgs),
}
#[derive(Debug, Args)]
struct ContainOutboundArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: DirectionArg,
    /// Containment cap in base units; v1 supports only a full block (0).
    #[arg(long)]
    limit_raw: Option<u128>,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Debug, Args)]
struct ContainRestoreArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    snapshot: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum DirectionArg {
    StellarToEvm,
    EvmToStellar,
}
impl From<DirectionArg> for Direction {
    fn from(value: DirectionArg) -> Self {
        if matches!(value, DirectionArg::StellarToEvm) {
            Direction::StellarToEvm
        } else {
            Direction::EvmToStellar
        }
    }
}

#[derive(Debug, Args)]
struct LegArgs {
    #[command(subcommand)]
    command: LegCommand,
}
#[derive(Debug, Subcommand)]
enum LegCommand {
    Quote(LegQuoteArgs),
    Send(LegSendArgs),
}
#[derive(Debug, Args)]
struct LegQuoteArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, value_enum)]
    direction: DirectionArg,
    #[arg(long)]
    amount_raw: u128,
    #[arg(long)]
    to: String,
    #[arg(long)]
    out: PathBuf,
}
#[derive(Debug, Args)]
struct LegSendArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    intent: PathBuf,
    #[arg(long)]
    allow_additional_obligation: bool,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct MessageArgs {
    #[command(subcommand)]
    command: MessageCommand,
}
#[derive(Debug, Subcommand)]
enum MessageCommand {
    Watch(MessageWatchArgs),
    Recover(MessageRecoverArgs),
}
#[derive(Debug, Args)]
struct MessageWatchArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    guid: String,
    /// Stop condition for the watch; v1 accepts only `terminal`.
    #[arg(long)]
    until: Option<String>,
    /// LayerZero Scan API base URL.
    #[arg(long)]
    scan_url: String,
    #[command(flatten)]
    rpc: RpcArgs,
}
#[derive(Debug, Args)]
struct MessageRecoverArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    guid: String,
    #[command(flatten)]
    effect: ChainEffectArgs,
}

#[derive(Debug, Args)]
struct EvidenceArgs {
    #[command(subcommand)]
    command: EvidenceCommand,
}
#[derive(Debug, Subcommand)]
enum EvidenceCommand {
    Import(EvidenceImportArgs),
}
#[derive(Debug, Args)]
struct EvidenceImportArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    bundle: PathBuf,
    #[arg(long)]
    write: bool,
}

#[derive(Clone, Debug, Args)]
struct ChainEffectArgs {
    #[arg(long, conflicts_with = "proposal_out")]
    execute: bool,
    #[arg(long, conflicts_with = "execute")]
    proposal_out: Option<PathBuf>,
    #[command(flatten)]
    rpc: RpcArgs,
    /// Named environment provider for the Stellar signing secret.
    #[arg(long, global = true)]
    stellar_secret_env: Option<String>,
    /// Mode-0600 non-symlink Foundry V3 keystore.
    #[arg(long, global = true, requires = "evm_password_file")]
    evm_keystore: Option<PathBuf>,
    /// Mode-0600 non-symlink keystore password provider.
    #[arg(long, global = true, requires = "evm_keystore")]
    evm_password_file: Option<PathBuf>,
    /// Shared, pre-existing operation-store root used to serialize every
    /// route controlled by the same signing authority.
    #[arg(long, global = true, requires = "execute")]
    operation_store_root: Option<PathBuf>,
}

impl Cli {
    pub fn command_name(&self) -> String {
        self.command.name().into()
    }

    pub fn run(mut self) -> Result<CommandData> {
        if let Some(path) = self.rpc_headers_file.take().as_deref() {
            crate::config::set_rpc_headers(crate::config::read_headers_file(path)?);
        }
        let rpc = self.rpc;
        match self.command {
            Command::Init(args) => init(&args),
            Command::Adopt(args) => adopt(args),
            Command::Operation(args) => operation(args),
            Command::Proposal(args) => proposal(args),
            Command::Artifact(args) => artifact(args),
            Command::Asset(args) => asset(args),
            Command::Route(args) => route(args),
            Command::Authority(args) => generic_authority(args),
            Command::Stellar(args) => generic_stellar(args),
            Command::Contain(args) => contain(args),
            Command::Leg(args) => leg(args, &rpc),
            Command::Message(args) => message(args),
            Command::Evidence(args) => evidence(args),
            Command::Reconcile(args) => {
                crate::reconcile::run_command(&args.state, args.fail_on_deficit)
            }
            Command::Health(args) => health(&args.state),
        }
    }
}

/// Compile-time exhaustive effect classification over the whole CLI surface.
/// Every new Clap variant must pick a class here; there are no wildcard
/// arms, so adding a command without classifying it breaks the build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandEffect {
    /// Read-only state inspection.
    LocalRead,
    /// Local file or state preparation; no chain interaction.
    LocalPrepare,
    /// Constructs, signs, or ingests a chain mutation. Testnet-only in v1.
    ChainMutation,
    /// Evidence import into the custody ledger; no chain interaction.
    EvidenceWrite,
}

impl CommandEffect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalRead => "local_read",
            Self::LocalPrepare => "local_prepare",
            Self::ChainMutation => "chain_mutation",
            Self::EvidenceWrite => "evidence_write",
        }
    }
}

impl Cli {
    pub fn effect(&self) -> &'static str {
        self.command.effect().as_str()
    }
}

impl Command {
    const fn name(&self) -> &'static str {
        match self {
            Self::Init(_) => "init",
            Self::Adopt(_) => "adopt",
            Self::Operation(_) => "operation",
            Self::Proposal(_) => "proposal",
            Self::Artifact(_) => "artifact",
            Self::Asset(_) => "asset",
            Self::Route(_) => "route",
            Self::Authority(_) => "authority",
            Self::Stellar(_) => "stellar",
            Self::Contain(_) => "contain",
            Self::Leg(_) => "leg",
            Self::Message(_) => "message",
            Self::Evidence(_) => "evidence",
            Self::Reconcile(_) => "reconcile",
            Self::Health(_) => "health",
        }
    }

    pub(crate) fn effect(&self) -> CommandEffect {
        match self {
            Self::Init(_) | Self::Adopt(_) => CommandEffect::LocalPrepare,
            Self::Operation(args) => args.command.effect(),
            Self::Proposal(args) => args.command.effect(),
            Self::Artifact(args) => args.command.effect(),
            Self::Asset(args) => args.command.effect(),
            Self::Route(args) => args.command.effect(),
            Self::Authority(args) => args.command.effect(),
            Self::Stellar(args) => args.command.effect(),
            Self::Contain(args) => args.command.effect(),
            Self::Leg(args) => args.command.effect(),
            Self::Message(args) => args.command.effect(),
            Self::Evidence(args) => args.command.effect(),
            Self::Reconcile(_) | Self::Health(_) => CommandEffect::LocalRead,
        }
    }
}

impl OperationCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Draft(_) => CommandEffect::LocalPrepare,
        }
    }
}

impl ProposalCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Create(_) | Self::Ingest(_) => CommandEffect::ChainMutation,
            Self::StellarSignature(args) => match args.command {
                ProposalSignatureCommand::Attach(_) => CommandEffect::LocalPrepare,
                ProposalSignatureCommand::Verify(_) => CommandEffect::LocalRead,
            },
            Self::SafeVerify(_) => CommandEffect::LocalRead,
        }
    }
}

impl ArtifactCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Verify(_) => CommandEffect::LocalRead,
            Self::Build(_) => CommandEffect::LocalPrepare,
        }
    }
}

impl AssetCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Wrap(_) => CommandEffect::ChainMutation,
        }
    }
}

impl RouteCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::DraftConfig(_) => CommandEffect::LocalPrepare,
            Self::Inspect(_) => CommandEffect::LocalRead,
            Self::Apply(_)
            | Self::SetPeer(_)
            | Self::SetLibrary(_)
            | Self::RemoveReceiveTimeout(_)
            | Self::SetUln(_)
            | Self::SetExecutor(_)
            | Self::SetOptions(_) => CommandEffect::ChainMutation,
        }
    }
}

impl AuthorityCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::StellarBeginOwner(_)
            | Self::StellarAcceptOwner(_)
            | Self::StellarCancelOwner(_)
            | Self::StellarSetDelegate(_)
            | Self::EvmTransferOwner(_)
            | Self::EvmSetDelegate(_) => CommandEffect::ChainMutation,
        }
    }
}

impl StellarCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::SetFee(_)
            | Self::SetFeeDepositAddress(_)
            | Self::SetMessageInspector(_)
            | Self::SetRateLimit(_)
            | Self::TtlSet(_)
            | Self::TtlFreeze(_)
            | Self::TtlExtendInstance(_)
            | Self::EmergencyPause(_)
            | Self::EmergencyUnpause(_)
            | Self::RoleGrant(_)
            | Self::RoleRevoke(_)
            | Self::RoleSetAdmin(_) => CommandEffect::ChainMutation,
        }
    }
}

impl ContainCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Inspect(_) => CommandEffect::LocalRead,
            Self::Outbound(_) | Self::Restore(_) => CommandEffect::ChainMutation,
        }
    }
}

impl LegCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Quote(_) => CommandEffect::LocalPrepare,
            Self::Send(_) => CommandEffect::ChainMutation,
        }
    }
}

impl MessageCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Watch(_) => CommandEffect::LocalRead,
            Self::Recover(_) => CommandEffect::ChainMutation,
        }
    }
}

impl EvidenceCommand {
    fn effect(&self) -> CommandEffect {
        match self {
            Self::Import(_) => CommandEffect::EvidenceWrite,
        }
    }
}

fn init(args: &InitArgs) -> Result<CommandData> {
    let desired = read_desired_precontext(&args.desired)?;
    let environment = environment::classify(&desired.identity)?;
    if let Some(network) = args.network.as_deref() {
        let expected = match environment {
            Environment::StellarTestnetSepolia => "stellar_testnet_sepolia",
            Environment::StellarMainnetEthereum => "stellar_mainnet_ethereum",
        };
        if network != expected {
            return Err(Error::InvalidInput(format!(
                "network mismatch: {network} but identity binds {expected}"
            )));
        }
    }
    if !args.write {
        return data(
            serde_json::json!({"preview": true, "desired_sha256": canonical_sha256(&desired)?}),
        );
    }
    environment::init_environment(
        &desired,
        args.rpc.stellar_url()?.as_deref(),
        args.rpc.evm_url()?.as_deref(),
        true,
    )?;
    let (store, mut state) = RouteStore::create(&args.state, desired.clone())?;
    // Route authority records: proposal construction binds owners from state.
    state
        .contracts
        .insert("stellar_owner".into(), desired.stellar_owner.clone());
    state
        .contracts
        .insert("stellar_delegate".into(), desired.stellar_delegate.clone());
    state
        .contracts
        .insert("evm_owner".into(), desired.evm_owner.clone());
    state
        .contracts
        .insert("evm_delegate".into(), desired.evm_delegate.clone());
    store.save_state(&state)?;
    data(serde_json::to_value(state)?)
}

fn adopt(args: AdoptArgs) -> Result<CommandData> {
    use crate::evm::EvmChain as _;
    use crate::stellar::StellarChain as _;

    let desired = read_desired_precontext(&args.desired)?;
    let asserted = environment::classify(&desired.identity)?;
    let opening: crate::domain::OpeningCustodyV1 = read_json(&args.opening_custody)?;
    crate::state::validate_opening_custody(Some(&opening))?;
    if opening.artifact_lock_sha256 != crate::artifacts::lock_sha256()? {
        return Err(Error::Custody(
            "opening custody artifact lock differs from the embedded closure".into(),
        ));
    }
    if opening.effective_config_sha256 != canonical_sha256(&desired.config)? {
        return Err(Error::Conflict(
            "opening custody effective configuration differs from desired route".into(),
        ));
    }
    let output = serde_json::json!({
        "action": "adopt",
        "environment": asserted,
        "route_id": desired.route_id,
        "stellar_oft": args.stellar_oft,
        "evm_oft": args.evm_oft,
        "artifact_lock_sha256": opening.artifact_lock_sha256,
        "opening_custody_sha256": canonical_sha256(&opening)?,
    });
    if !args.write {
        return data(output);
    }
    let lock = crate::artifacts::embedded_lock()?;
    let stellar_rpc = args.rpc.stellar_url()?.ok_or_else(|| {
        Error::InvalidInput("adoption requires --stellar-rpc-env or --stellar-rpc-file".into())
    })?;
    let stellar = crate::stellar::HttpStellarChain::new(&stellar_rpc)?;
    let observed_stellar_hash = stellar.contract_code_hash(&args.stellar_oft)?;
    if !observed_stellar_hash.eq_ignore_ascii_case(&lock.stellar.oft_wasm_sha256) {
        return Err(Error::Custody(
            "deployed Stellar OFT WASM hash differs from the artifact closure".into(),
        ));
    }
    let evm_address = crate::evm::parse_address(&args.evm_oft)?;
    let evm_rpc = args.rpc.evm_url()?.ok_or_else(|| {
        Error::InvalidInput("adoption requires --evm-rpc-env or --evm-rpc-file".into())
    })?;
    let evm = crate::evm::HttpEvmChain::new(&evm_rpc)?;
    environment::verify_live_endpoint_code(&desired.identity, &stellar, &evm)?;
    let code = crate::block_on_result(evm.code(evm_address))?;
    crate::deployment::verify_runtime_code_hash(&code, &lock.evm.runtime_bytecode_keccak256)?;

    if args.state.exists() {
        return Err(Error::Conflict(format!(
            "route state already exists: {}",
            args.state.display()
        )));
    }
    let temporary = args
        .state
        .with_extension(format!("adopt-{}.tmp", std::process::id()));
    if temporary.exists() {
        return Err(Error::Conflict(format!(
            "adoption staging path already exists: {}",
            temporary.display()
        )));
    }
    let result: Result<()> = (|| {
        let (store, mut state) = RouteStore::create(&temporary, desired.clone())?;
        state
            .contracts
            .insert("stellar_oft".into(), args.stellar_oft);
        state.contracts.insert("evm_oft".into(), args.evm_oft);
        store.save_state(&state)?;
        store.record_opening_custody(opening)?;
        state = store.load_state()?;
        crate::route::apply_adoption_readback(&stellar, &evm, &mut state, &desired)?;
        store.save_state(&state)?;
        std::fs::rename(&temporary, &args.state)?;
        if let Some(parent) = args.state.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() && temporary.exists() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    result?;
    data(output)
}
fn operation(args: OperationArgs) -> Result<CommandData> {
    match args.command {
        OperationCommand::Draft(args) => {
            let store = RouteStore::open(&args.state)?;
            let state = store.load_state()?;
            let value = read_json::<serde_json::Value>(&args.args)?;
            let operation = draft_operation(&args.command, &value)?;
            let draft = OperationDraftV1 {
                schema_name: "operation_draft".into(),
                schema_version: SCHEMA_VERSION,
                route_id: state.route_id,
                desired_sha256: state.desired_sha256,
                operation,
                observed_sha256: canonical_sha256(&state.effective_config)?,
            };
            write_create_new_json(&args.out, &draft)?;
            artifact_data("operation_draft", args.out, &draft, false)
        }
    }
}

fn str_field(value: &serde_json::Value, name: &str) -> Result<String> {
    value
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or_else(|| Error::InvalidInput(format!("draft args: missing string field {name}")))
}

fn opt_str_field(value: &serde_json::Value, name: &str) -> Result<Option<String>> {
    match value.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(Error::InvalidInput(format!(
            "draft args: field {name} must be a string or null"
        ))),
    }
}

fn num_field<T>(value: &serde_json::Value, name: &str) -> Result<T>
where
    T: TryFrom<u64>,
{
    let raw = value
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::InvalidInput(format!("draft args: missing numeric field {name}")))?;
    T::try_from(raw)
        .map_err(|_| Error::InvalidInput(format!("draft args: field {name} overflows its type")))
}
/// Closed dispatch from a kebab-case command name and checked JSON args to
/// an OperationV1. No OperationV1 JSON is ever read directly.
fn draft_operation(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "install-stellar-wasm" => Ok(OperationV1::InstallStellarWasm {
            wasm_sha256: str_field(value, "wasm_sha256")?,
        }),
        "deploy-stellar-oft" => Ok(OperationV1::DeployStellarOft {
            deployer: str_field(value, "deployer")?,
            salt: str_field(value, "salt")?,
            wasm_sha256: str_field(value, "wasm_sha256")?,
            token: str_field(value, "token")?,
            shared_decimals: num_field(value, "shared_decimals")?,
            endpoint: str_field(value, "endpoint")?,
            delegate: str_field(value, "delegate")?,
            expected_address: str_field(value, "expected_address")?,
        }),
        "deploy-evm-oft" => Ok(OperationV1::DeployEvmOft {
            deployer: str_field(value, "deployer")?,
            nonce: num_field(value, "nonce")?,
            creation_bytecode_keccak256: str_field(value, "creation_bytecode_keccak256")?,
            name: str_field(value, "name")?,
            symbol: str_field(value, "symbol")?,
            endpoint: str_field(value, "endpoint")?,
            owner_delegate: str_field(value, "owner_delegate")?,
            expected_address: str_field(value, "expected_address")?,
        }),
        "begin-stellar-ownership-transfer" => Ok(OperationV1::BeginStellarOwnershipTransfer {
            new_owner: str_field(value, "new_owner")?,
            ttl: num_field(value, "ttl")?,
        }),
        "accept-stellar-ownership" => Ok(OperationV1::AcceptStellarOwnership),
        "cancel-stellar-ownership-transfer" => Ok(OperationV1::CancelStellarOwnershipTransfer),
        "transfer-evm-ownership" => Ok(OperationV1::TransferEvmOwnership {
            new_owner: str_field(value, "new_owner")?,
        }),
        "set-stellar-delegate" => Ok(OperationV1::SetStellarDelegate {
            delegate: str_field(value, "delegate")?,
        }),
        "set-evm-delegate" => Ok(OperationV1::SetEvmDelegate {
            delegate: str_field(value, "delegate")?,
        }),
        "set-stellar-peer" => Ok(OperationV1::SetStellarPeer {
            remote_eid: num_field(value, "remote_eid")?,
            peer: str_field(value, "peer")?,
        }),
        "set-evm-peer" => Ok(OperationV1::SetEvmPeer {
            remote_eid: num_field(value, "remote_eid")?,
            peer: str_field(value, "peer")?,
        }),
        "set-stellar-send-library" => Ok(OperationV1::SetStellarSendLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
        }),
        "set-stellar-receive-library" => Ok(OperationV1::SetStellarReceiveLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
            grace_period_seconds: num_field(value, "grace_period_seconds")?,
        }),
        "remove-stellar-receive-library-timeout" => {
            Ok(OperationV1::RemoveStellarReceiveLibraryTimeout {
                remote_eid: num_field(value, "remote_eid")?,
            })
        }
        "set-evm-send-library" => Ok(OperationV1::SetEvmSendLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
        }),
        "set-evm-receive-library" => Ok(OperationV1::SetEvmReceiveLibrary {
            remote_eid: num_field(value, "remote_eid")?,
            library: str_field(value, "library")?,
            grace_period_seconds: num_field(value, "grace_period_seconds")?,
        }),
        "remove-evm-receive-library-timeout" => Ok(OperationV1::RemoveEvmReceiveLibraryTimeout {
            remote_eid: num_field(value, "remote_eid")?,
        }),
        "set-stellar-uln-config" => Ok(OperationV1::SetStellarUlnConfig {
            remote_eid: num_field(value, "remote_eid")?,
            direction: str_field(value, "direction")?,
            caller: str_field(value, "caller")?,
            oapp: str_field(value, "oapp")?,
            library: str_field(value, "library")?,
            config_sha256: str_field(value, "config_sha256")?,
            config: value
                .get("config")
                .cloned()
                .ok_or_else(|| Error::InvalidInput("draft args: missing config".into()))?,
        }),
        "set-evm-uln-config" => Ok(OperationV1::SetEvmUlnConfig {
            remote_eid: num_field(value, "remote_eid")?,
            direction: str_field(value, "direction")?,
            caller: str_field(value, "caller")?,
            oapp: str_field(value, "oapp")?,
            library: str_field(value, "library")?,
            config_sha256: str_field(value, "config_sha256")?,
            config: value
                .get("config")
                .cloned()
                .ok_or_else(|| Error::InvalidInput("draft args: missing config".into()))?,
        }),
        _ => draft_operation_tail(command, value),
    }
}

/// Second half of the closed draft dispatch (executor/economic/admin/
/// containment commands).
fn draft_operation_tail(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "set-stellar-executor-config" => Ok(OperationV1::SetStellarExecutorConfig {
            remote_eid: num_field(value, "remote_eid")?,
            caller: str_field(value, "caller")?,
            oapp: str_field(value, "oapp")?,
            library: str_field(value, "library")?,
            config_sha256: str_field(value, "config_sha256")?,
            config: value
                .get("config")
                .cloned()
                .ok_or_else(|| Error::InvalidInput("draft args: missing config".into()))?,
        }),
        "set-evm-executor-config" => Ok(OperationV1::SetEvmExecutorConfig {
            remote_eid: num_field(value, "remote_eid")?,
            caller: str_field(value, "caller")?,
            oapp: str_field(value, "oapp")?,
            library: str_field(value, "library")?,
            config_sha256: str_field(value, "config_sha256")?,
            config: value
                .get("config")
                .cloned()
                .ok_or_else(|| Error::InvalidInput("draft args: missing config".into()))?,
        }),
        "set-stellar-receive-options" => Ok(OperationV1::SetStellarReceiveOptions {
            remote_eid: num_field(value, "remote_eid")?,
            message_type: num_field(value, "message_type")?,
            options: str_field(value, "options")?,
        }),
        "set-evm-receive-options" => Ok(OperationV1::SetEvmReceiveOptions {
            remote_eid: num_field(value, "remote_eid")?,
            message_type: num_field(value, "message_type")?,
            options: str_field(value, "options")?,
        }),
        "set-default-fee" => Ok(OperationV1::SetDefaultFee {
            bps: num_field(value, "bps")?,
        }),
        "set-destination-fee" => Ok(OperationV1::SetDestinationFee {
            remote_eid: num_field(value, "remote_eid")?,
            bps: num_field(value, "bps")?,
        }),
        "set-fee-recipient" => Ok(OperationV1::SetFeeRecipient {
            recipient: str_field(value, "recipient")?,
        }),
        "set-message-inspector" => Ok(OperationV1::SetMessageInspector {
            inspector: opt_str_field(value, "inspector")?,
        }),
        "set-inbound-rate-limit" => Ok(OperationV1::SetInboundRateLimit {
            remote_eid: num_field(value, "remote_eid")?,
            limit_raw: num_field(value, "limit_raw")?,
            window_seconds: num_field(value, "window_seconds")?,
            mode: str_field(value, "mode")?,
        }),
        "set-outbound-rate-limit" => Ok(OperationV1::SetOutboundRateLimit {
            remote_eid: num_field(value, "remote_eid")?,
            limit_raw: num_field(value, "limit_raw")?,
            window_seconds: num_field(value, "window_seconds")?,
            mode: str_field(value, "mode")?,
        }),
        _ => draft_operation_admin(command, value),
    }
}

/// Third half of the closed draft dispatch (ttl/role/recovery/containment).
fn draft_operation_admin(command: &str, value: &serde_json::Value) -> Result<OperationV1> {
    match command {
        "pause-emergency" => Ok(OperationV1::PauseEmergency),
        "unpause-emergency" => Ok(OperationV1::UnpauseEmergency),
        "set-ttl-config" => Ok(OperationV1::SetTtlConfig {
            instance_threshold: num_field(value, "instance_threshold")?,
            instance_extend_to: num_field(value, "instance_extend_to")?,
            persistent_threshold: num_field(value, "persistent_threshold")?,
            persistent_extend_to: num_field(value, "persistent_extend_to")?,
        }),
        "freeze-ttl-config" => Ok(OperationV1::FreezeTtlConfig {
            acknowledgement: str_field(value, "acknowledgement")?,
        }),
        "extend-instance-ttl" => Ok(OperationV1::ExtendInstanceTtl {
            ledgers: num_field(value, "ledgers")?,
        }),
        "grant-role" => Ok(OperationV1::GrantRole {
            role: str_field(value, "role")?,
            address: str_field(value, "address")?,
        }),
        "revoke-role" => Ok(OperationV1::RevokeRole {
            role: str_field(value, "role")?,
            address: str_field(value, "address")?,
        }),
        "set-role-admin" => Ok(OperationV1::SetRoleAdmin {
            role: str_field(value, "role")?,
            admin_role: str_field(value, "admin_role")?,
        }),
        "remove-role-admin" => Ok(OperationV1::RemoveRoleAdmin {
            role: str_field(value, "role")?,
            admin_role: str_field(value, "admin_role")?,
        }),
        "send-leg" => Ok(OperationV1::SendLeg {
            vm: match str_field(value, "vm")?.as_str() {
                "stellar" => Vm::Stellar,
                "evm" => Vm::Evm,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown vm {other}"
                    )))
                }
            },
            intent: Box::new(serde_json::from_value(
                value
                    .get("intent")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("draft args: missing intent".into()))?,
            )?),
        }),
        "commit-verification" => Ok(OperationV1::CommitVerification {
            vm: match str_field(value, "vm")?.as_str() {
                "stellar" => Vm::Stellar,
                "evm" => Vm::Evm,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown vm {other}"
                    )))
                }
            },
            message: Box::new(serde_json::from_value(
                value
                    .get("message")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("draft args: missing message".into()))?,
            )?),
        }),
        "execute-receive" => Ok(OperationV1::ExecuteReceive {
            vm: match str_field(value, "vm")?.as_str() {
                "stellar" => Vm::Stellar,
                "evm" => Vm::Evm,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "draft args: unknown vm {other}"
                    )))
                }
            },
            message: Box::new(serde_json::from_value(
                value
                    .get("message")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("draft args: missing message".into()))?,
            )?),
        }),
        "contain-outbound" => Ok(OperationV1::ContainOutbound {
            snapshot: Box::new(serde_json::from_value(
                value
                    .get("snapshot")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("draft args: missing snapshot".into()))?,
            )?),
        }),
        "restore-outbound" => Ok(OperationV1::RestoreOutbound {
            snapshot: Box::new(serde_json::from_value(
                value
                    .get("snapshot")
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput("draft args: missing snapshot".into()))?,
            )?),
        }),
        other => Err(Error::InvalidInput(format!(
            "unknown draft command {other}"
        ))),
    }
}

fn proposal(args: ProposalArgs) -> Result<CommandData> {
    match args.command {
        ProposalCommand::Create(args) => {
            let state = RouteStore::open(&args.state)?.load_state()?;
            environment::classify(&state.identity)?;
            crate::governance::create_proposal(
                &args.state,
                &args.draft,
                &args.out,
                args.rpc.stellar_url()?.as_deref(),
                args.rpc.evm_url()?.as_deref(),
            )
        }
        ProposalCommand::Ingest(args) => {
            let state = RouteStore::open(&args.state)?.load_state()?;
            environment::classify(&state.identity)?;
            crate::governance::ingest_proposal(
                &args.state,
                &args.proposal,
                &args.executed_tx,
                args.rpc.stellar_url()?.as_deref(),
                args.rpc.evm_url()?.as_deref(),
                args.write,
            )
        }
        ProposalCommand::StellarSignature(args) => match args.command {
            ProposalSignatureCommand::Attach(args) => crate::governance::attach_signature_command(
                &args.state,
                &args.proposal,
                &args.public_key,
                &args.signature,
                &args.out,
            ),
            ProposalSignatureCommand::Verify(args) => {
                crate::governance::verify_stellar_proposal(&args.state, &args.proposal)
            }
        },
        ProposalCommand::SafeVerify(args) => {
            let state = RouteStore::open(&args.state)?.load_state()?;
            environment::classify(&state.identity)?;
            crate::governance::verify_safe_proposal(
                &args.state,
                &args.proposal,
                &args.safe_tx,
                args.rpc.evm_url()?.as_deref(),
            )
        }
    }
}

fn artifact(args: ArtifactArgs) -> Result<CommandData> {
    match args.command {
        ArtifactCommand::Verify(args) => crate::artifacts::verify_command(&args.state),
        ArtifactCommand::Build(args) => crate::artifacts::build_command(
            &args.state,
            &args.out_dir,
            args.write,
            args.deps_archive.as_deref(),
            args.source_archive.as_deref(),
        ),
    }
}

fn asset(args: AssetArgs) -> Result<CommandData> {
    match args.command {
        AssetCommand::Wrap(args) => wrap(args),
    }
}

fn wrap(args: WrapArgs) -> Result<CommandData> {
    // Pre-dispatch USDC boundary before any state, artifact, or signer access.
    let kind: AssetKind = args.asset_kind.into();
    if kind == AssetKind::Usdc || crate::domain::is_known_usdc(&args.asset) {
        return Err(Error::Policy("unsupported_use_cctp".into()));
    }
    let desired = read_desired_precontext(&args.desired)?;
    let store = RouteStore::open(&args.state)?;
    let state = store.load_state()?;
    if state.asset.kind != kind || state.asset.asset_id != args.asset {
        return Err(Error::InvalidInput(
            "wrap asset must equal the bound route asset".into(),
        ));
    }
    if desired.route_id != state.route_id || canonical_sha256(&desired)? != state.desired_sha256 {
        return Err(Error::Conflict(
            "desired route does not bind to this route state".into(),
        ));
    }
    let require_evidence =
        environment::classify(&state.identity)? == Environment::StellarMainnetEthereum;
    let nonce = if let Some(nonce) = args.evm_nonce {
        nonce
    } else {
        let url = args.effect.rpc.evm_url()?.ok_or_else(|| {
            Error::InvalidInput(
                "live_environment_required: EVM RPC URL is required without --evm-nonce".into(),
            )
        })?;
        let evm = crate::evm::HttpEvmChain::new(&url)?;
        let deployer = crate::evm::parse_address(&desired.evm_owner)?;
        crate::block_on_result(evm.account_nonce(deployer))?
    };
    let plan = crate::wrap::plan_wrap(
        &desired,
        &state.desired_sha256,
        &args.name,
        &args.symbol,
        nonce,
        require_evidence,
    )?;
    if args.effect.execute {
        return execute_wrap_deployment(&args.state, &desired, &plan, args.effect);
    }
    if let Some(out) = args.effect.proposal_out {
        // v1 emits a proposal for the first plan node; the remaining nodes
        // are driven by the granular commands in plan order.
        let first = plan
            .operations
            .first()
            .cloned()
            .ok_or_else(|| Error::InvalidInput("wrap plan has no operations".into()))?;
        return crate::governance::proposal_for_operation(
            &args.state,
            &first,
            &out,
            args.effect.rpc.stellar_url()?.as_deref(),
            args.effect.rpc.evm_url()?.as_deref(),
        );
    }
    data(serde_json::to_value(&plan)?)
}

fn execute_wrap_deployment(
    state_path: &Path,
    desired: &DesiredRouteV1,
    plan: &crate::wrap::WrapPlanV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    use alloy::primitives::B256;

    let stellar_url = effect.rpc.stellar_url()?.ok_or_else(|| {
        Error::InvalidInput("wrap execution requires a Stellar RPC provider".into())
    })?;
    let evm_url = effect
        .rpc
        .evm_url()?
        .ok_or_else(|| Error::InvalidInput("wrap execution requires an EVM RPC provider".into()))?;
    let stellar =
        crate::stellar::HttpStellarChain::new(&stellar_url)?.with_artifact_root(state_path);
    let evm = crate::evm::HttpEvmChain::new(&evm_url)?.with_artifact_root(state_path);
    let lock = crate::artifacts::embedded_lock()?;
    let creation_hash: [u8; 32] = hex::decode(&lock.evm.creation_bytecode_keccak256)
        .map_err(|_| Error::Custody("artifact lock EVM creation hash is not hex".into()))?
        .try_into()
        .map_err(|_| Error::Custody("artifact lock EVM creation hash must be 32 bytes".into()))?;
    let binding = crate::evm::DeployEvmOftBindingV1::bind(
        crate::evm::parse_address(&plan.evm_deployer)?,
        plan.evm_nonce,
        Some(B256::from(creation_hash)),
        plan.name.clone(),
        plan.symbol.clone(),
        crate::evm::parse_address(&desired.identity.evm_endpoint)?,
        crate::evm::parse_address(&desired.evm_delegate)?,
    )?;
    let mut executed = Vec::new();
    loop {
        let observed = crate::deployment::observe_deployments(&stellar, &evm, desired, plan)?;
        let node_plan = crate::deployment::deployment_node_plan(
            desired,
            &crate::canonical_sha256(desired)?,
            plan,
            &binding,
            &observed,
        )?;
        let Some(index) = crate::deployment::require_resumable(&node_plan)? else {
            return data(serde_json::json!({
                "plan": plan,
                "deployment": node_plan,
                "executed": executed
            }));
        };
        let operation = node_plan.nodes[index].operation.clone();
        let result = chain_effect(state_path, &operation, effect.clone())?;
        let after = crate::deployment::observe_deployments(&stellar, &evm, desired, plan)?;
        let after_plan = crate::deployment::deployment_node_plan(
            desired,
            &crate::canonical_sha256(desired)?,
            plan,
            &binding,
            &after,
        )?;
        if after_plan.nodes[index].status != crate::deployment::DeploymentNodeStatus::Satisfied {
            return Err(Error::Chain(format!(
                "deployment node {} did not satisfy exact readback",
                after_plan.nodes[index].kind
            )));
        }
        if matches!(
            operation,
            OperationV1::DeployStellarOft { .. } | OperationV1::DeployEvmOft { .. }
        ) {
            let store = RouteStore::open(state_path)?;
            let _lock = store.lock()?;
            let mut state = store.load_state()?;
            match operation {
                OperationV1::DeployStellarOft {
                    expected_address, ..
                } => {
                    state
                        .contracts
                        .insert("stellar_oft".into(), expected_address);
                }
                OperationV1::DeployEvmOft {
                    expected_address, ..
                } => {
                    state.contracts.insert("evm_oft".into(), expected_address);
                }
                _ => unreachable!(),
            }
            store.save_state(&state)?;
        }
        executed.push(result.result);
    }
}

fn route(args: RouteArgs) -> Result<CommandData> {
    match args.command {
        RouteCommand::DraftConfig(args) => draft_config(&args.state, &args.out),
        RouteCommand::Inspect(args) => inspect(&args.state),
        RouteCommand::Apply(args) => apply_route(args),
        RouteCommand::SetPeer(args) => {
            let remote_eid = args.remote_eid;
            let peer = args.peer;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::SetStellarPeer { remote_eid, peer },
                Vm::Evm => OperationV1::SetEvmPeer { remote_eid, peer },
            };
            route_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::SetLibrary(args) => {
            let direction = match args.direction.as_str() {
                "send" => LibraryDirection::Send,
                "receive" => LibraryDirection::Receive,
                other => {
                    return Err(Error::InvalidInput(format!(
                        "library direction must be send or receive, got {other}"
                    )))
                }
            };
            let operation = library_operation(
                args.vm.into(),
                direction,
                args.remote_eid,
                args.library,
                args.grace_period_seconds,
            )?;
            route_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::RemoveReceiveTimeout(args) => {
            let remote_eid = args.remote_eid;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::RemoveStellarReceiveLibraryTimeout { remote_eid },
                Vm::Evm => OperationV1::RemoveEvmReceiveLibraryTimeout { remote_eid },
            };
            route_effect(&args.state, &operation, args.effect)
        }
        RouteCommand::SetUln(args) => config_hash_effect(args, true),
        RouteCommand::SetExecutor(args) => config_hash_effect(args, false),
        RouteCommand::SetOptions(args) => {
            let remote_eid = args.remote_eid;
            let message_type = args.message_type;
            let options = args.options;
            let operation = match args.vm.into() {
                Vm::Stellar => OperationV1::SetStellarReceiveOptions {
                    remote_eid,
                    message_type,
                    options,
                },
                Vm::Evm => OperationV1::SetEvmReceiveOptions {
                    remote_eid,
                    message_type,
                    options,
                },
            };
            route_effect(&args.state, &operation, args.effect)
        }
    }
}

fn route_effect(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    let execute = effect.execute;
    if execute {
        let state = RouteStore::open(state_path)?.load_state()?;
        environment::require_testnet(&state.identity)?;
    }
    let stellar_url = if execute {
        Some(effect.rpc.stellar_url()?.ok_or_else(|| {
            Error::InvalidInput("route execution requires a Stellar RPC provider".into())
        })?)
    } else {
        None
    };
    let evm_url = if execute {
        Some(effect.rpc.evm_url()?.ok_or_else(|| {
            Error::InvalidInput("route execution requires an EVM RPC provider".into())
        })?)
    } else {
        None
    };
    let result = chain_effect(state_path, operation, effect)?;
    if execute {
        let stellar_url = stellar_url
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("Stellar RPC provider is absent".into()))?;
        let evm_url = evm_url
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("EVM RPC provider is absent".into()))?;
        let stellar =
            crate::stellar::HttpStellarChain::new(stellar_url)?.with_artifact_root(state_path);
        let evm = crate::evm::HttpEvmChain::new(evm_url)?.with_artifact_root(state_path);
        let store = RouteStore::open(state_path)?;
        let _lock = store.lock()?;
        let mut state = store.load_state()?;
        crate::route::apply_live_readback(&stellar, &evm, &mut state, operation)?;
        store.save_state(&state)?;
    }
    Ok(result)
}

fn management_effect(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    let execute = effect.execute;
    if execute {
        let state = RouteStore::open(state_path)?.load_state()?;
        environment::require_testnet(&state.identity)?;
    }
    let stellar_url = if execute {
        Some(effect.rpc.stellar_url()?.ok_or_else(|| {
            Error::InvalidInput("management execution requires a Stellar RPC provider".into())
        })?)
    } else {
        None
    };
    let evm_url = if execute {
        Some(effect.rpc.evm_url()?.ok_or_else(|| {
            Error::InvalidInput("management execution requires an EVM RPC provider".into())
        })?)
    } else {
        None
    };
    let result = chain_effect(state_path, operation, effect)?;
    if execute {
        let stellar = crate::stellar::HttpStellarChain::new(
            stellar_url
                .as_deref()
                .ok_or_else(|| Error::InvalidInput("Stellar RPC provider is absent".into()))?,
        )?
        .with_artifact_root(state_path);
        let evm = crate::evm::HttpEvmChain::new(
            evm_url
                .as_deref()
                .ok_or_else(|| Error::InvalidInput("EVM RPC provider is absent".into()))?,
        )?
        .with_artifact_root(state_path);
        let store = RouteStore::open(state_path)?;
        let _lock = store.lock()?;
        let mut state = store.load_state()?;
        crate::route::apply_management_readback(&stellar, &evm, &mut state, operation)?;
        store.save_state(&state)?;
    }
    Ok(result)
}

fn apply_route(args: ApplyRouteArgs) -> Result<CommandData> {
    let desired: DesiredRouteV1 = read_json(&args.config)?;
    let store = RouteStore::open(&args.state)?;
    let state = store.load_state()?;
    crate::route::mutation_gate(&state.identity)?;
    let initial = crate::route::plan_route_mutations(&desired, &state)?;
    if !args.effect.execute && args.effect.proposal_out.is_none() {
        return data(serde_json::to_value(initial)?);
    }
    if args.effect.proposal_out.is_some() {
        let operation = initial
            .steps
            .iter()
            .find_map(|step| {
                (step.status == crate::route::RouteStepStatus::Pending)
                    .then_some(step.operation.as_ref())
                    .flatten()
            })
            .ok_or_else(|| {
                if initial.converged {
                    Error::Conflict("route is already converged".into())
                } else {
                    Error::Conflict("route mutation plan is blocked".into())
                }
            })?;
        return route_effect(&args.state, operation, args.effect);
    }
    let mut results = Vec::new();
    loop {
        let state = store.load_state()?;
        let plan = crate::route::plan_route_mutations(&desired, &state)?;
        if plan.converged {
            return data(serde_json::json!({
                "plan": plan,
                "executed": results,
            }));
        }
        let operation = plan
            .steps
            .iter()
            .find_map(|step| {
                (step.status == crate::route::RouteStepStatus::Pending)
                    .then_some(step.operation.as_ref())
                    .flatten()
            })
            .ok_or_else(|| Error::Conflict("route mutation plan is blocked".into()))?;
        let result = route_effect(&args.state, operation, args.effect.clone())?;
        results.push(result.result);
    }
}

fn generic_authority(args: AuthorityArgs) -> Result<CommandData> {
    match args.command {
        AuthorityCommand::StellarBeginOwner(a) => management_effect(
            &a.state,
            &OperationV1::BeginStellarOwnershipTransfer {
                new_owner: a.new_owner,
                ttl: a.ttl,
            },
            a.effect,
        ),
        AuthorityCommand::StellarAcceptOwner(a) => {
            management_effect(&a.state, &OperationV1::AcceptStellarOwnership, a.effect)
        }
        AuthorityCommand::StellarCancelOwner(a) => management_effect(
            &a.state,
            &OperationV1::CancelStellarOwnershipTransfer,
            a.effect,
        ),
        AuthorityCommand::StellarSetDelegate(a) => management_effect(
            &a.state,
            &OperationV1::SetStellarDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
        AuthorityCommand::EvmTransferOwner(a) => management_effect(
            &a.state,
            &OperationV1::TransferEvmOwnership {
                new_owner: a.new_owner,
            },
            a.effect,
        ),
        AuthorityCommand::EvmSetDelegate(a) => management_effect(
            &a.state,
            &OperationV1::SetEvmDelegate {
                delegate: a.delegate,
            },
            a.effect,
        ),
    }
}

fn chain_effect(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    chain_effect_with_send_policy(state_path, operation, effect, false)
}

fn chain_effect_with_send_policy(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
    allow_additional_obligation: bool,
) -> Result<CommandData> {
    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    if !effect.execute && effect.proposal_out.is_none() {
        return data(serde_json::json!({"preview": true, "operation": operation}));
    }
    if effect.execute && effect.operation_store_root.is_none() {
        return Err(Error::InvalidInput(
            "execution requires --operation-store-root".into(),
        ));
    }
    if let Some(out) = effect.proposal_out {
        return crate::governance::proposal_for_operation(
            state_path,
            operation,
            &out,
            effect.rpc.stellar_url()?.as_deref(),
            effect.rpc.evm_url()?.as_deref(),
        );
    }
    environment::require_testnet(&state.identity)?;
    match crate::governance::operation_vm(operation) {
        Vm::Stellar => {
            execute_stellar_operation(state_path, operation, effect, allow_additional_obligation)
        }
        Vm::Evm => {
            execute_evm_operation(state_path, operation, effect, allow_additional_obligation)
        }
    }
}

fn verify_stellar_recovery_payload(payload: &str, expected_sha256: &str) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use sha2::{Digest as _, Sha256};
    let bytes = BASE64_STANDARD
        .decode(payload)
        .map_err(|_| Error::Custody("journaled Stellar payload is not base64".into()))?;
    if hex::encode(Sha256::digest(bytes)) != expected_sha256 {
        return Err(Error::Custody(
            "journaled Stellar signed payload digest mismatch".into(),
        ));
    }
    Ok(())
}

fn verify_evm_recovery_payload(
    payload: &[u8],
    expected_sha256: &str,
    transaction_hash: &str,
) -> Result<()> {
    use sha2::{Digest as _, Sha256};
    if hex::encode(Sha256::digest(payload)) != expected_sha256 {
        return Err(Error::Custody(
            "journaled EVM signed payload digest mismatch".into(),
        ));
    }
    let derived_hash = format!("0x{}", hex::encode(crate::evm::keccak256_of(payload)));
    if !derived_hash.eq_ignore_ascii_case(transaction_hash) {
        return Err(Error::Custody(
            "journaled EVM transaction hash does not bind signed payload".into(),
        ));
    }
    Ok(())
}

fn journaled_operation(history: &[crate::state::OperationEventV1]) -> Result<OperationV1> {
    history
        .iter()
        .find_map(|event| event.detail.get("operation"))
        .cloned()
        .ok_or_else(|| Error::Custody("operation journal omitted the planned operation".into()))
        .and_then(|value| serde_json::from_value(value).map_err(Error::from))
}

fn stellar_terminal_state(
    store: &RouteStore,
    operation: &OperationV1,
    stellar: &dyn crate::stellar::StellarChain,
    status: &crate::stellar::StellarTransactionStatusV1,
    transaction_hash: &str,
) -> Result<crate::state::OperationState> {
    if let OperationV1::SendLeg { intent, .. } = operation {
        let latest = stellar.latest_ledger()?;
        let canonical = stellar.transaction_status(transaction_hash)?;
        if !crate::canary::stellar_source_finalized(intent, status, latest, &canonical)? {
            return Ok(crate::state::OperationState::Ambiguous);
        }
        if crate::canary::record_stellar_source_message(store, intent, &canonical, transaction_hash)
            .is_err()
        {
            return Ok(crate::state::OperationState::Ambiguous);
        }
    }
    Ok(match status.status.as_str() {
        "success" => crate::state::OperationState::Confirmed,
        "failed" => crate::state::OperationState::Failed,
        _ => crate::state::OperationState::Ambiguous,
    })
}

fn evm_terminal_state(
    store: &RouteStore,
    operation: &OperationV1,
    evm: &dyn crate::evm::EvmChain,
    receipt: Option<&crate::evm::EvmReceiptV1>,
) -> Result<crate::state::OperationState> {
    let Some(receipt) = receipt else {
        return Ok(crate::state::OperationState::Ambiguous);
    };
    if let OperationV1::SendLeg { intent, .. } = operation {
        let latest = crate::block_on_result(evm.latest_block())?;
        let canonical = crate::block_on_result(evm.transaction_receipt(&receipt.transaction_hash))?;
        if !crate::canary::evm_source_finalized(intent, receipt, latest, canonical.as_ref())? {
            return Ok(crate::state::OperationState::Ambiguous);
        }
        let Some(canonical) = canonical.as_ref() else {
            return Ok(crate::state::OperationState::Ambiguous);
        };
        if crate::canary::record_evm_source_message(store, intent, canonical).is_err() {
            return Ok(crate::state::OperationState::Ambiguous);
        }
    }
    Ok(match receipt.succeeded {
        Some(true) => crate::state::OperationState::Confirmed,
        Some(false) => crate::state::OperationState::Failed,
        None => crate::state::OperationState::Ambiguous,
    })
}

fn recover_stellar_submission(
    store: &RouteStore,
    operation_id: &str,
    stellar: &dyn crate::stellar::StellarChain,
) -> Result<Option<CommandData>> {
    let history = store.operation_history(operation_id)?;
    let Some(last) = history.last() else {
        return Ok(None);
    };
    let operation = journaled_operation(&history)?;
    match last.state {
        crate::state::OperationState::Planned => return Ok(None),
        crate::state::OperationState::Confirmed | crate::state::OperationState::Failed => {
            return data(serde_json::json!({
                "operation_id": operation_id,
                "status": last.state,
                "recovered": true,
            }))
            .map(Some)
        }
        crate::state::OperationState::Signed
        | crate::state::OperationState::SubmissionPending
        | crate::state::OperationState::Ambiguous => {}
        _ => {
            return Err(Error::Conflict(
                "operation journal is not resumable as a direct Stellar execution".into(),
            ))
        }
    }
    let checkpoint = history
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.state,
                crate::state::OperationState::Signed
                    | crate::state::OperationState::SubmissionPending
            ) && event.detail.get("signed_payload").is_some()
        })
        .ok_or_else(|| Error::Custody("signed Stellar payload is missing from journal".into()))?;
    let payload = checkpoint.detail["signed_payload"]
        .as_str()
        .ok_or_else(|| Error::Custody("journaled Stellar payload is not a string".into()))?;
    let transaction_hash = checkpoint.detail["transaction_hash"]
        .as_str()
        .ok_or_else(|| Error::Custody("journaled Stellar transaction hash is missing".into()))?;
    let signed_sha256 = checkpoint.detail["signed_transaction_sha256"]
        .as_str()
        .ok_or_else(|| Error::Custody("journaled Stellar payload digest is missing".into()))?;
    verify_stellar_recovery_payload(payload, signed_sha256)?;
    if last.state == crate::state::OperationState::Signed {
        store.append_operation(
            crate::state::OperationEventV1 {
                operation_id: operation_id.into(),
                state: crate::state::OperationState::SubmissionPending,
                detail: checkpoint.detail.clone(),
            },
            None,
        )?;
    }
    let mut status = stellar.transaction_status(transaction_hash)?;
    if status.status == "not_found" {
        let returned = stellar.submit_transaction(payload)?;
        if returned != transaction_hash {
            store.append_operation(
                crate::state::OperationEventV1 {
                    operation_id: operation_id.into(),
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({
                        "transaction_hash": returned,
                        "expected_transaction_hash": transaction_hash,
                    }),
                },
                None,
            )?;
            return Err(Error::Custody(
                "RPC returned an unexpected transaction hash".into(),
            ));
        }
        status = stellar.transaction_status(transaction_hash)?;
    }
    let terminal = stellar_terminal_state(store, &operation, stellar, &status, transaction_hash)?;
    store.append_operation(
        crate::state::OperationEventV1 {
            operation_id: operation_id.into(),
            state: terminal,
            detail: serde_json::json!({
                "transaction_hash": transaction_hash,
                "ledger": status.ledger,
                "signed_transaction_sha256": signed_sha256,
                "recovered": true,
            }),
        },
        None,
    )?;
    data(serde_json::json!({
        "transaction_hash": transaction_hash,
        "status": status.status,
        "ledger": status.ledger,
        "recovered": true,
    }))
    .map(Some)
}

fn recover_evm_submission(
    store: &RouteStore,
    operation_id: &str,
    evm: &dyn crate::evm::EvmChain,
) -> Result<Option<CommandData>> {
    let history = store.operation_history(operation_id)?;
    let Some(last) = history.last() else {
        return Ok(None);
    };
    let operation = journaled_operation(&history)?;
    match last.state {
        crate::state::OperationState::Planned => return Ok(None),
        crate::state::OperationState::Confirmed | crate::state::OperationState::Failed => {
            return data(serde_json::json!({
                "operation_id": operation_id,
                "status": last.state,
                "recovered": true,
            }))
            .map(Some)
        }
        crate::state::OperationState::Signed
        | crate::state::OperationState::SubmissionPending
        | crate::state::OperationState::Ambiguous => {}
        _ => {
            return Err(Error::Conflict(
                "operation journal is not resumable as a direct EVM execution".into(),
            ))
        }
    }
    let checkpoint = history
        .iter()
        .rev()
        .find(|event| {
            matches!(
                event.state,
                crate::state::OperationState::Signed
                    | crate::state::OperationState::SubmissionPending
            ) && event.detail.get("signed_payload").is_some()
        })
        .ok_or_else(|| Error::Custody("signed EVM payload is missing from journal".into()))?;
    let payload = hex::decode(
        checkpoint.detail["signed_payload"]
            .as_str()
            .ok_or_else(|| Error::Custody("journaled EVM payload is not a string".into()))?,
    )
    .map_err(|_| Error::Custody("journaled EVM payload is not hex".into()))?;
    let transaction_hash = checkpoint.detail["transaction_hash"]
        .as_str()
        .ok_or_else(|| Error::Custody("journaled EVM transaction hash is missing".into()))?;
    let signed_sha256 = checkpoint.detail["signed_transaction_sha256"]
        .as_str()
        .ok_or_else(|| Error::Custody("journaled EVM payload digest is missing".into()))?;
    verify_evm_recovery_payload(&payload, signed_sha256, transaction_hash)?;
    if last.state == crate::state::OperationState::Signed {
        store.append_operation(
            crate::state::OperationEventV1 {
                operation_id: operation_id.into(),
                state: crate::state::OperationState::SubmissionPending,
                detail: checkpoint.detail.clone(),
            },
            None,
        )?;
    }
    let mut receipt = crate::block_on_result(evm.transaction_receipt(transaction_hash))?;
    if receipt.is_none() {
        let returned = crate::block_on_result(evm.send_raw_transaction(&payload))?;
        if returned != transaction_hash {
            store.append_operation(
                crate::state::OperationEventV1 {
                    operation_id: operation_id.into(),
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({
                        "transaction_hash": returned,
                        "expected_transaction_hash": transaction_hash,
                    }),
                },
                None,
            )?;
            return Err(Error::Custody(
                "RPC returned an unexpected transaction hash".into(),
            ));
        }
        receipt = crate::block_on_result(evm.transaction_receipt(transaction_hash))?;
    }
    let terminal = evm_terminal_state(store, &operation, evm, receipt.as_ref())?;
    store.append_operation(
        crate::state::OperationEventV1 {
            operation_id: operation_id.into(),
            state: terminal,
            detail: serde_json::json!({
                "transaction_hash": transaction_hash,
                "receipt": receipt,
                "recovered": true,
            }),
        },
        None,
    )?;
    data(serde_json::json!({
        "transaction_hash": transaction_hash,
        "receipt": receipt,
        "recovered": true,
    }))
    .map(Some)
}

fn execute_stellar_operation(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
    allow_additional_obligation: bool,
) -> Result<CommandData> {
    use crate::stellar::StellarChain as _;
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use sha2::{Digest as _, Sha256};

    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    crate::environment::require_testnet(&state.identity)?;
    let rpc_url = effect.rpc.stellar_url()?.ok_or_else(|| {
        Error::InvalidInput(
            "Stellar execution requires --stellar-rpc-env or --stellar-rpc-file".into(),
        )
    })?;
    let evm_rpc_url = effect.rpc.evm_url()?.ok_or_else(|| {
        Error::InvalidInput(
            "Stellar execution requires the counterparty --evm-rpc-env or --evm-rpc-file".into(),
        )
    })?;
    let secret_env = effect.stellar_secret_env.as_deref().ok_or_else(|| {
        Error::InvalidInput("Stellar execution requires --stellar-secret-env".into())
    })?;
    let sender = crate::layerzero::stellar_operation_authorizer(&state, operation)?;
    let signer = crate::stellar::StellarSecretProviderV1::from_named_env(secret_env)?;
    if signer.public_key() != sender {
        return Err(Error::Policy(format!(
            "Stellar signer {} is not the recorded route owner",
            signer.public_key()
        )));
    }
    let operation_id = canonical_sha256(operation)?;
    let binding = store.derive_phase_a_binding(Vm::Stellar, sender)?;
    let operations_root = effect
        .operation_store_root
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("execution requires --operation-store-root".into()))?;
    let guard = store.acquire_mutation(&binding, operations_root, &operation_id)?;
    let stellar = crate::stellar::HttpStellarChain::new(&rpc_url)?.with_artifact_root(state_path);
    if let Some(result) = recover_stellar_submission(guard.store(), &operation_id, &stellar)? {
        guard.release_authority_if_terminal()?;
        return Ok(result);
    }
    if guard.store().operation_history(&operation_id)?.is_empty() {
        guard.store().append_operation(
            crate::state::OperationEventV1 {
                operation_id: operation_id.clone(),
                state: crate::state::OperationState::Planned,
                detail: serde_json::json!({"operation": operation}),
            },
            None,
        )?;
    }
    let evm = crate::evm::HttpEvmChain::new(&evm_rpc_url)?.with_artifact_root(state_path);
    crate::canary::revalidate_locked_send(
        state_path,
        operation,
        allow_additional_obligation,
        &stellar,
        &evm,
    )?;
    let plan = crate::governance::build_executable_plan(guard.state(), operation, &stellar, &evm)?;
    crate::canary::verify_plan_source_slot(operation, &plan)?;
    if let crate::domain::OperationV1::SendLeg { intent, .. } = operation {
        crate::canary::verify_stellar_plan_fee_ceiling(
            intent,
            plan.stellar
                .as_ref()
                .ok_or_else(|| Error::Chain("Stellar plan omitted its envelope".into()))?,
        )?;
    }
    let stellar_binding = plan
        .stellar
        .ok_or_else(|| Error::Chain("Stellar plan omitted its envelope".into()))?;
    let signed = crate::stellar::sign_envelope(
        &stellar_binding.envelope_xdr,
        &stellar_binding.network_passphrase,
        &signer,
    )?;
    let signed_bytes = BASE64_STANDARD
        .decode(&signed)
        .map_err(|error| Error::Chain(format!("signed Stellar envelope base64 failed: {error}")))?;
    let signed_sha256 = hex::encode(Sha256::digest(signed_bytes));
    let expected_transaction_hash =
        crate::stellar::envelope_transaction_hash(&signed, &stellar_binding.network_passphrase)?;
    guard.reserve_authority(&stellar_binding.sequence, &signed_sha256)?;
    guard.store().append_operation(
        crate::state::OperationEventV1 {
            operation_id: operation_id.clone(),
            state: crate::state::OperationState::Signed,
            detail: serde_json::json!({
                "unsigned_envelope_sha256": stellar_binding.envelope_sha256,
                "signed_envelope_sha256": signed_sha256,
                "signed_transaction_sha256": signed_sha256,
                "transaction_hash": expected_transaction_hash,
                "signed_payload": signed,
            }),
        },
        None,
    )?;
    guard.submission_pending(
        &operation_id,
        &signed_sha256,
        &expected_transaction_hash,
        &signed,
    )?;
    let transaction_hash = match stellar.submit_transaction(&signed) {
        Ok(hash) => hash,
        Err(error) => {
            guard.store().append_operation(
                crate::state::OperationEventV1 {
                    operation_id,
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({"error_code": error.code()}),
                },
                None,
            )?;
            return Err(error);
        }
    };
    if transaction_hash != expected_transaction_hash {
        guard.store().append_operation(
            crate::state::OperationEventV1 {
                operation_id,
                state: crate::state::OperationState::Ambiguous,
                detail: serde_json::json!({
                    "transaction_hash": transaction_hash,
                    "expected_transaction_hash": expected_transaction_hash,
                }),
            },
            None,
        )?;
        return Err(Error::Custody(
            "RPC returned an unexpected transaction hash".into(),
        ));
    }
    let status = match stellar.transaction_status(&transaction_hash) {
        Ok(status) => status,
        Err(error) => {
            guard.store().append_operation(
                crate::state::OperationEventV1 {
                    operation_id,
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({
                        "transaction_hash": transaction_hash,
                        "error_code": error.code(),
                    }),
                },
                None,
            )?;
            return Err(error);
        }
    };
    let terminal = stellar_terminal_state(
        guard.store(),
        operation,
        &stellar,
        &status,
        &transaction_hash,
    )?;
    guard.store().append_operation(
        crate::state::OperationEventV1 {
            operation_id,
            state: terminal,
            detail: serde_json::json!({
                "transaction_hash": transaction_hash,
                "ledger": status.ledger,
            }),
        },
        None,
    )?;
    guard.release_authority_if_terminal()?;
    data(serde_json::json!({
        "transaction_hash": transaction_hash,
        "status": status.status,
        "ledger": status.ledger,
    }))
}

fn execute_evm_operation(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
    allow_additional_obligation: bool,
) -> Result<CommandData> {
    use crate::evm::EvmChain as _;
    use sha2::Digest as _;

    let store = RouteStore::open(state_path)?;
    let state = store.load_state()?;
    crate::environment::require_testnet(&state.identity)?;
    let rpc_url = effect.rpc.evm_url()?.ok_or_else(|| {
        Error::InvalidInput("EVM execution requires --evm-rpc-env or --evm-rpc-file".into())
    })?;
    let stellar_rpc_url = effect.rpc.stellar_url()?.ok_or_else(|| {
        Error::InvalidInput(
            "EVM execution requires the counterparty --stellar-rpc-env or --stellar-rpc-file"
                .into(),
        )
    })?;
    let keystore = effect
        .evm_keystore
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("EVM execution requires --evm-keystore".into()))?;
    let password_file = effect
        .evm_password_file
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("EVM execution requires --evm-password-file".into()))?;
    let sender = crate::layerzero::evm_operation_authorizer(&state, operation)?;
    let sender_address = crate::evm::parse_address(sender)?;
    let password = crate::config::SecretProvider::File(password_file.to_path_buf()).read()?;
    let signer = crate::evm::keystore_signer(keystore, &password, sender_address)?;
    let operation_id = canonical_sha256(operation)?;
    let binding = store.derive_phase_a_binding(Vm::Evm, sender)?;
    let operations_root = effect
        .operation_store_root
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("execution requires --operation-store-root".into()))?;
    let guard = store.acquire_mutation(&binding, operations_root, &operation_id)?;
    let evm = crate::evm::HttpEvmChain::new(&rpc_url)?.with_artifact_root(state_path);
    if let Some(result) = recover_evm_submission(guard.store(), &operation_id, &evm)? {
        guard.release_authority_if_terminal()?;
        return Ok(result);
    }
    if guard.store().operation_history(&operation_id)?.is_empty() {
        guard.store().append_operation(
            crate::state::OperationEventV1 {
                operation_id: operation_id.clone(),
                state: crate::state::OperationState::Planned,
                detail: serde_json::json!({"operation": operation}),
            },
            None,
        )?;
    }
    let stellar =
        crate::stellar::HttpStellarChain::new(&stellar_rpc_url)?.with_artifact_root(state_path);
    crate::canary::revalidate_locked_send(
        state_path,
        operation,
        allow_additional_obligation,
        &stellar,
        &evm,
    )?;
    let plan = crate::governance::build_executable_plan(guard.state(), operation, &stellar, &evm)?;
    crate::canary::verify_plan_source_slot(operation, &plan)?;
    if let crate::domain::OperationV1::SendLeg { intent, .. } = operation {
        crate::canary::verify_evm_plan_fee_ceiling(
            intent,
            plan.evm
                .as_ref()
                .ok_or_else(|| Error::Chain("EVM plan omitted its transaction".into()))?,
        )?;
    }
    let evm_binding = plan
        .evm
        .ok_or_else(|| Error::Chain("EVM plan omitted its transaction".into()))?;
    let signed = crate::evm::sign_eip1559(&evm_binding, &signer)?;
    let signed_sha256 = hex::encode(sha2::Sha256::digest(&signed.encoded));
    guard.reserve_authority(&evm_binding.nonce.to_string(), &signed_sha256)?;
    guard.store().append_operation(
        crate::state::OperationEventV1 {
            operation_id: operation_id.clone(),
            state: crate::state::OperationState::Signed,
            detail: serde_json::json!({
                "unsigned_transaction_sha256": evm_binding.transaction_digest,
                "signed_transaction_sha256": signed_sha256,
                "transaction_hash": signed.transaction_hash,
                "signed_payload": hex::encode(&signed.encoded),
            }),
        },
        None,
    )?;
    guard.submission_pending(
        &operation_id,
        &signed_sha256,
        &signed.transaction_hash,
        &hex::encode(&signed.encoded),
    )?;
    let transaction_hash = match crate::block_on_result(evm.send_raw_transaction(&signed.encoded)) {
        Ok(hash) => hash,
        Err(error) => {
            guard.store().append_operation(
                crate::state::OperationEventV1 {
                    operation_id,
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({"error_code": error.code()}),
                },
                None,
            )?;
            return Err(error);
        }
    };
    if transaction_hash != signed.transaction_hash {
        guard.store().append_operation(
            crate::state::OperationEventV1 {
                operation_id,
                state: crate::state::OperationState::Ambiguous,
                detail: serde_json::json!({
                    "transaction_hash": transaction_hash,
                    "expected_transaction_hash": signed.transaction_hash,
                }),
            },
            None,
        )?;
        return Err(Error::Custody(
            "RPC returned an unexpected transaction hash".into(),
        ));
    }
    let receipt = match crate::block_on_result(evm.transaction_receipt(&transaction_hash)) {
        Ok(receipt) => receipt,
        Err(error) => {
            guard.store().append_operation(
                crate::state::OperationEventV1 {
                    operation_id,
                    state: crate::state::OperationState::Ambiguous,
                    detail: serde_json::json!({
                        "transaction_hash": transaction_hash,
                        "error_code": error.code(),
                    }),
                },
                None,
            )?;
            return Err(error);
        }
    };
    let terminal = evm_terminal_state(guard.store(), operation, &evm, receipt.as_ref())?;
    guard.store().append_operation(
        crate::state::OperationEventV1 {
            operation_id,
            state: terminal,
            detail: serde_json::json!({
                "transaction_hash": transaction_hash,
                "receipt": receipt,
            }),
        },
        None,
    )?;
    guard.release_authority_if_terminal()?;
    data(serde_json::json!({
        "transaction_hash": transaction_hash,
        "receipt": receipt,
    }))
}

fn generic_stellar(args: StellarArgs) -> Result<CommandData> {
    match args.command {
        StellarCommand::SetFee(a) => {
            let operation = match a.remote_eid {
                Some(remote_eid) => OperationV1::SetDestinationFee {
                    remote_eid,
                    bps: a.bps,
                },
                None => OperationV1::SetDefaultFee { bps: a.bps },
            };
            management_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::SetFeeDepositAddress(a) => management_effect(
            &a.state,
            &OperationV1::SetFeeRecipient {
                recipient: a.recipient,
            },
            a.effect,
        ),
        StellarCommand::SetMessageInspector(a) => management_effect(
            &a.state,
            &OperationV1::SetMessageInspector {
                inspector: a.inspector,
            },
            a.effect,
        ),
        StellarCommand::SetRateLimit(a) => {
            let remote_eid = a.remote_eid;
            let limit_raw = a.limit_raw;
            let window_seconds = a.window_seconds;
            let mode = a.mode;
            let operation = match a.direction {
                RateDirectionArg::Inbound => OperationV1::SetInboundRateLimit {
                    remote_eid,
                    limit_raw,
                    window_seconds,
                    mode,
                },
                RateDirectionArg::Outbound => OperationV1::SetOutboundRateLimit {
                    remote_eid,
                    limit_raw,
                    window_seconds,
                    mode,
                },
            };
            management_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::TtlSet(a) => {
            let state = RouteStore::open(&a.state)?.load_state()?;
            let operation = crate::ttl::set_config(
                &state,
                a.instance_threshold,
                a.instance_extend_to,
                a.persistent_threshold,
                a.persistent_extend_to,
            )?;
            management_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::TtlFreeze(a) => {
            let state = RouteStore::open(&a.state)?.load_state()?;
            let operation = crate::ttl::freeze(&state, &a.acknowledge_irreversible)?;
            management_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::TtlExtendInstance(a) => {
            let state = RouteStore::open(&a.state)?.load_state()?;
            let operation = crate::ttl::extend_instance(&state, a.ledgers)?;
            management_effect(&a.state, &operation, a.effect)
        }
        StellarCommand::EmergencyPause(a) => {
            management_effect(&a.state, &OperationV1::PauseEmergency, a.effect)
        }
        StellarCommand::EmergencyUnpause(a) => {
            management_effect(&a.state, &OperationV1::UnpauseEmergency, a.effect)
        }
        StellarCommand::RoleGrant(a) => {
            crate::ttl::require_role(&a.role)?;
            crate::ttl::require_classic_role_address(&a.address)?;
            management_effect(
                &a.state,
                &OperationV1::GrantRole {
                    role: a.role,
                    address: a.address,
                },
                a.effect,
            )
        }
        StellarCommand::RoleRevoke(a) => {
            crate::ttl::require_role(&a.role)?;
            crate::ttl::require_classic_role_address(&a.address)?;
            management_effect(
                &a.state,
                &OperationV1::RevokeRole {
                    role: a.role,
                    address: a.address,
                },
                a.effect,
            )
        }
        StellarCommand::RoleSetAdmin(a) => {
            crate::ttl::require_role(&a.role)?;
            crate::ttl::require_role(&a.admin_role)?;
            management_effect(
                &a.state,
                &OperationV1::SetRoleAdmin {
                    role: a.role,
                    admin_role: a.admin_role,
                },
                a.effect,
            )
        }
    }
}

enum LibraryDirection {
    Send,
    Receive,
}
fn library_operation(
    vm: Vm,
    direction: LibraryDirection,
    remote_eid: u32,
    library: String,
    grace_period_seconds: Option<u64>,
) -> Result<OperationV1> {
    Ok(match (vm, direction) {
        (Vm::Stellar, LibraryDirection::Send) => OperationV1::SetStellarSendLibrary {
            remote_eid,
            library,
        },
        (Vm::Stellar, LibraryDirection::Receive) => OperationV1::SetStellarReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds: grace_period_seconds.ok_or_else(|| {
                Error::InvalidInput("receive library requires --grace-period-seconds".into())
            })?,
        },
        (Vm::Evm, LibraryDirection::Send) => OperationV1::SetEvmSendLibrary {
            remote_eid,
            library,
        },
        (Vm::Evm, LibraryDirection::Receive) => OperationV1::SetEvmReceiveLibrary {
            remote_eid,
            library,
            grace_period_seconds: grace_period_seconds.ok_or_else(|| {
                Error::InvalidInput("receive library requires --grace-period-seconds".into())
            })?,
        },
    })
}

fn config_hash_effect(args: ConfigHashArgs, uln: bool) -> Result<CommandData> {
    if uln {
        match args.direction.as_deref() {
            Some("send" | "receive") => {}
            Some(other) => {
                return Err(Error::InvalidInput(format!(
                    "uln direction must be send or receive, got {other}"
                )))
            }
            None => {
                return Err(Error::InvalidInput(
                    "set-uln requires --direction send|receive".into(),
                ))
            }
        }
    } else if args.direction.is_some() {
        return Err(Error::InvalidInput(
            "set-executor has no --direction".into(),
        ));
    }
    let config: serde_json::Value = read_json(&args.config)?;
    let config_sha256 = canonical_sha256(&config)?;
    let remote_eid = args.remote_eid;
    let vm: Vm = args.vm.into();
    let state = RouteStore::open(&args.state)?.load_state()?;
    let contract = |stellar: &str, evm: &str| {
        state
            .contracts
            .get(match vm {
                Vm::Stellar => stellar,
                Vm::Evm => evm,
            })
            .cloned()
            .ok_or_else(|| {
                Error::Custody(format!("route contract is not recorded: {stellar}/{evm}"))
            })
    };
    let caller = contract("stellar_owner", "evm_owner")?;
    let oapp = contract("stellar_oft", "evm_oft")?;
    let direction = args.direction.unwrap_or_else(|| "receive".into());
    let library_key = match direction.as_str() {
        "send" => crate::route::config_key_send_library(vm, remote_eid),
        _ => crate::route::config_key_receive_library(vm, remote_eid),
    };
    let library = state
        .requested_config
        .get(&library_key)
        .or_else(|| state.effective_config.get(&library_key))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::Custody(format!("route library is not recorded: {library_key}")))?
        .to_string();
    let operation = match (vm, uln) {
        (Vm::Stellar, true) => OperationV1::SetStellarUlnConfig {
            remote_eid,
            direction,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
        (Vm::Evm, true) => OperationV1::SetEvmUlnConfig {
            remote_eid,
            direction,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
        (Vm::Stellar, false) => OperationV1::SetStellarExecutorConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
        (Vm::Evm, false) => OperationV1::SetEvmExecutorConfig {
            remote_eid,
            caller,
            oapp,
            library,
            config_sha256,
            config,
        },
    };
    route_effect(&args.state, &operation, args.effect)
}

fn contain(args: ContainArgs) -> Result<CommandData> {
    match args.command {
        ContainCommand::Inspect(args) => crate::layerzero::containment_status(&args.state),
        ContainCommand::Outbound(args) => {
            if args.limit_raw.is_some_and(|limit| limit != 0) {
                return Err(Error::InvalidInput(
                    "v1 containment is zero-cap only: --limit-raw must be 0".into(),
                ));
            }
            let store = RouteStore::open(&args.state)?;
            let mut state = store.load_state()?;
            let direction: Direction = args.direction.into();
            if args.effect.execute || args.effect.proposal_out.is_some() {
                environment::classify(&state.identity)?;
                if direction == Direction::EvmToStellar {
                    let evm_url = args.effect.rpc.evm_url()?.ok_or_else(|| {
                        Error::InvalidInput("EVM containment requires an EVM RPC provider".into())
                    })?;
                    let evm = crate::evm::HttpEvmChain::new(&evm_url)?;
                    let blocked =
                        crate::route::read_evm_blocked_library(&evm, &state.identity.evm_endpoint)?;
                    let _lock = store.lock()?;
                    state = store.load_state()?;
                    state.effective_config.insert(
                        "endpoint:blocked_library:evm".into(),
                        serde_json::Value::String(blocked),
                    );
                    store.save_state(&state)?;
                }
            }
            let snapshot = crate::layerzero::containment_snapshot(&state, direction)?;
            let operation = OperationV1::ContainOutbound {
                snapshot: Box::new(snapshot),
            };
            containment_effect(&args.state, &operation, args.effect)
        }
        ContainCommand::Restore(args) => {
            let state = RouteStore::open(&args.state)?.load_state()?;
            if args.effect.execute || args.effect.proposal_out.is_some() {
                environment::classify(&state.identity)?;
            }
            let snapshot: crate::domain::ContainmentSnapshotV1 = serde_json::from_value(
                state
                    .effective_config
                    .get(&format!("containment:snapshot:{}", args.snapshot))
                    .cloned()
                    .ok_or_else(|| {
                        Error::Custody("containment snapshot is not recorded in route state".into())
                    })?,
            )?;
            if crate::canonical_sha256(&snapshot)? != args.snapshot {
                return Err(Error::Custody(
                    "containment snapshot digest mismatch".into(),
                ));
            }
            let operation = OperationV1::RestoreOutbound {
                snapshot: Box::new(snapshot),
            };
            containment_effect(&args.state, &operation, args.effect)
        }
    }
}

fn containment_effect(
    state_path: &Path,
    operation: &OperationV1,
    effect: ChainEffectArgs,
) -> Result<CommandData> {
    let (snapshot, restore) = match operation {
        OperationV1::ContainOutbound { snapshot } => (snapshot.as_ref(), false),
        OperationV1::RestoreOutbound { snapshot } => (snapshot.as_ref(), true),
        _ => {
            return Err(Error::InvalidInput(
                "containment_effect requires a containment operation".into(),
            ))
        }
    };
    let before = RouteStore::open(state_path)?.load_state()?;
    let mutation = crate::layerzero::containment_mutation(&before, snapshot, restore)?;
    let execute = effect.execute;
    let stellar_url = if execute {
        Some(effect.rpc.stellar_url()?.ok_or_else(|| {
            Error::InvalidInput("containment execution requires a Stellar RPC provider".into())
        })?)
    } else {
        None
    };
    let evm_url = if execute {
        Some(effect.rpc.evm_url()?.ok_or_else(|| {
            Error::InvalidInput("containment execution requires an EVM RPC provider".into())
        })?)
    } else {
        None
    };
    let result = chain_effect(state_path, operation, effect)?;
    if execute {
        let stellar = crate::stellar::HttpStellarChain::new(
            stellar_url
                .as_deref()
                .ok_or_else(|| Error::InvalidInput("Stellar RPC provider is absent".into()))?,
        )?
        .with_artifact_root(state_path);
        let evm = crate::evm::HttpEvmChain::new(
            evm_url
                .as_deref()
                .ok_or_else(|| Error::InvalidInput("EVM RPC provider is absent".into()))?,
        )?
        .with_artifact_root(state_path);
        let store = RouteStore::open(state_path)?;
        let _lock = store.lock()?;
        let mut state = store.load_state()?;
        crate::route::apply_live_readback(&stellar, &evm, &mut state, &mutation)?;
        let snapshot_sha256 = crate::canonical_sha256(snapshot)?;
        state.effective_config.insert(
            format!("containment:snapshot:{snapshot_sha256}"),
            serde_json::to_value(snapshot)?,
        );
        state.effective_config.insert(
            format!(
                "containment:{}",
                match snapshot.direction {
                    Direction::StellarToEvm => "stellar",
                    Direction::EvmToStellar => "evm",
                }
            ),
            serde_json::json!({
                "snapshot_sha256": snapshot_sha256,
                "status": if restore { "restored" } else { "confirmed" }
            }),
        );
        store.save_state(&state)?;
    }
    Ok(result)
}

fn leg(args: LegArgs, rpc: &RpcArgs) -> Result<CommandData> {
    match args.command {
        LegCommand::Quote(args) => {
            let stellar_url = rpc.stellar_url()?.ok_or_else(|| {
                Error::InvalidInput("leg quote requires a Stellar RPC provider".into())
            })?;
            let evm_url = rpc.evm_url()?.ok_or_else(|| {
                Error::InvalidInput("leg quote requires an EVM RPC provider".into())
            })?;
            let stellar = crate::stellar::HttpStellarChain::new(&stellar_url)?
                .with_artifact_root(&args.state);
            let evm = crate::evm::HttpEvmChain::new(&evm_url)?.with_artifact_root(&args.state);
            crate::canary::quote_live(
                &args.state,
                args.direction.into(),
                args.amount_raw,
                &args.to,
                &args.out,
                &stellar,
                &evm,
            )
        }
        LegCommand::Send(args) => {
            let operation = match (args.effect.rpc.stellar_url()?, args.effect.rpc.evm_url()?) {
                (Some(stellar_url), Some(evm_url)) => {
                    let stellar = crate::stellar::HttpStellarChain::new(&stellar_url)?
                        .with_artifact_root(&args.state);
                    let evm =
                        crate::evm::HttpEvmChain::new(&evm_url)?.with_artifact_root(&args.state);
                    crate::canary::send_operation_live(
                        &args.state,
                        &args.intent,
                        args.allow_additional_obligation,
                        &stellar,
                        &evm,
                    )?
                }
                _ => crate::canary::send_operation(
                    &args.state,
                    &args.intent,
                    args.allow_additional_obligation,
                )?,
            };
            chain_effect_with_send_policy(
                &args.state,
                &operation,
                args.effect,
                args.allow_additional_obligation,
            )
        }
    }
}

fn message(args: MessageArgs) -> Result<CommandData> {
    match args.command {
        MessageCommand::Watch(args) => {
            match args.until.as_deref() {
                None | Some("terminal") => {}
                Some(other) => {
                    return Err(Error::InvalidInput(format!(
                        "watch --until accepts only terminal, got {other}"
                    )))
                }
            }
            let stellar_url = args.rpc.stellar_url()?.ok_or_else(|| {
                Error::InvalidInput("message watch requires a Stellar RPC provider".into())
            })?;
            let evm_url = args.rpc.evm_url()?.ok_or_else(|| {
                Error::InvalidInput("message watch requires an EVM RPC provider".into())
            })?;
            let scan = crate::scan::HttpScanClient::new(&args.scan_url)?;
            let stellar = crate::stellar::HttpStellarChain::new(&stellar_url)?
                .with_artifact_root(&args.state);
            let evm = crate::evm::HttpEvmChain::new(&evm_url)?.with_artifact_root(&args.state);
            let destination = crate::canary::LiveDestinationPacketReader {
                stellar: &stellar,
                evm: &evm,
            };
            crate::canary::watch_with_scan(&args.state, &args.guid, &scan, &destination)
        }
        MessageCommand::Recover(args) => {
            let operation = crate::canary::recovery_operation(&args.state, &args.guid)?;
            let execute = args.effect.execute;
            let (identity, stage) = match &operation {
                OperationV1::CommitVerification { message, .. } => (
                    message.identity(),
                    match message.direction {
                        Direction::StellarToEvm => crate::domain::MessageStageV1::ForwardCommitted,
                        Direction::EvmToStellar => crate::domain::MessageStageV1::ReverseCommitted,
                    },
                ),
                OperationV1::ExecuteReceive { message, .. } => (
                    message.identity(),
                    match message.direction {
                        Direction::StellarToEvm => crate::domain::MessageStageV1::ForwardMinted,
                        Direction::EvmToStellar => crate::domain::MessageStageV1::ReverseUnlocked,
                    },
                ),
                _ => unreachable!(),
            };
            let result = chain_effect(&args.state, &operation, args.effect)?;
            if execute {
                let transaction = result
                    .result
                    .get("transaction_hash")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        Error::Chain("confirmed recovery result has no transaction hash".into())
                    })?
                    .to_string();
                let store = RouteStore::open(&args.state)?;
                let _lock = store.lock()?;
                store.append_message_recovery_event(
                    &identity,
                    transaction,
                    crate::domain::MessageStatusEventV1 {
                        stage,
                        observed_at_unix: crate::now_unix()?,
                        evidence_sha256: crate::canonical_sha256(&result.result)?,
                    },
                )?;
            }
            Ok(result)
        }
    }
}

fn evidence(args: EvidenceArgs) -> Result<CommandData> {
    match args.command {
        EvidenceCommand::Import(args) => {
            crate::canary::import_evidence(&args.state, &args.bundle, args.write)
        }
    }
}

fn draft_config(state_path: &Path, out: &Path) -> Result<CommandData> {
    let state = RouteStore::open(state_path)?.load_state()?;
    let desired = DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: crate::domain::SCHEMA_VERSION,
        route_id: state.route_id.clone(),
        identity: state.identity.clone(),
        asset: state.asset.clone(),
        stellar_owner: state
            .contracts
            .get("stellar_owner")
            .cloned()
            .ok_or_else(|| Error::Custody("route has no recorded stellar_owner".into()))?,
        stellar_delegate: state
            .contracts
            .get("stellar_delegate")
            .cloned()
            .ok_or_else(|| Error::Custody("route has no recorded stellar_delegate".into()))?,
        evm_owner: state
            .contracts
            .get("evm_owner")
            .cloned()
            .ok_or_else(|| Error::Custody("route has no recorded evm_owner".into()))?,
        evm_delegate: state
            .contracts
            .get("evm_delegate")
            .cloned()
            .ok_or_else(|| Error::Custody("route has no recorded evm_delegate".into()))?,
        config: state.requested_config.clone(),
    };
    if canonical_sha256(&desired)? != state.desired_sha256 {
        return Err(Error::Custody(
            "route state cannot reconstruct its desired-route binding".into(),
        ));
    }
    write_create_new_json(out, &desired)?;
    artifact_data("route_config_draft", out.to_path_buf(), &desired, false)
}

fn inspect(path: &Path) -> Result<CommandData> {
    let state = RouteStore::open(path)?.load_state()?;
    let keys: std::collections::BTreeSet<_> = state
        .requested_config
        .keys()
        .chain(state.effective_config.keys())
        .collect();
    let drift: Vec<_> = keys
        .into_iter()
        .filter_map(|key| {
            let requested = state.requested_config.get(key);
            let effective = state.effective_config.get(key);
            (requested != effective).then(|| {
                serde_json::json!({
                    "field": key,
                    "requested": requested,
                    "effective": effective,
                })
            })
        })
        .collect();
    let missing_contracts: Vec<_> = [
        "stellar_owner",
        "stellar_delegate",
        "evm_owner",
        "evm_delegate",
        "stellar_oft",
        "evm_oft",
    ]
    .into_iter()
    .filter(|key| state.contracts.get(*key).is_none_or(String::is_empty))
    .collect();
    data(serde_json::json!({
        "route": state,
        "config_drift": drift,
        "missing_contracts": missing_contracts,
        "converged": drift.is_empty() && missing_contracts.is_empty(),
    }))
}
fn health(state: &Path) -> Result<CommandData> {
    crate::health::command(state)
}

fn read_desired_precontext(path: &Path) -> Result<DesiredRouteV1> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1_048_576 {
        return Err(Error::InvalidInput(
            "desired route must be a real file no larger than 1 MiB".into(),
        ));
    }
    let desired: DesiredRouteV1 = read_json(path)?;
    desired.parse()
}

#[allow(clippy::unnecessary_wraps)]
fn data(result: serde_json::Value) -> Result<CommandData> {
    Ok(CommandData {
        result,
        artifact: None,
    })
}

fn artifact_data<T: Serialize>(
    kind: &str,
    path: PathBuf,
    value: &T,
    authoritative: bool,
) -> Result<CommandData> {
    let artifact = crate::domain::ArtifactRefV1 {
        kind: kind.into(),
        path,
        sha256: canonical_sha256(value)?,
        schema_version: SCHEMA_VERSION,
        authoritative,
    };
    Ok(CommandData {
        result: serde_json::json!({}),
        artifact: Some(artifact),
    })
}

#[cfg(test)]
mod recovery_integrity_tests {
    use super::{verify_evm_recovery_payload, verify_stellar_recovery_payload};
    use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
    use sha2::{Digest as _, Sha256};

    #[test]
    fn stellar_recovery_reuses_only_digest_bound_payload() {
        let payload = BASE64_STANDARD.encode(b"signed-envelope");
        let digest = hex::encode(Sha256::digest(b"signed-envelope"));
        verify_stellar_recovery_payload(&payload, &digest).unwrap();
        assert!(verify_stellar_recovery_payload(&payload, &"0".repeat(64)).is_err());
        assert!(verify_stellar_recovery_payload("not base64", &digest).is_err());
    }

    #[test]
    fn evm_recovery_reuses_only_digest_and_transaction_hash_bound_payload() {
        let payload = b"signed-transaction";
        let digest = hex::encode(Sha256::digest(payload));
        let transaction_hash = format!("0x{}", hex::encode(crate::evm::keccak256_of(payload)));
        verify_evm_recovery_payload(payload, &digest, &transaction_hash).unwrap();
        assert!(verify_evm_recovery_payload(payload, &"0".repeat(64), &transaction_hash).is_err());
        assert!(
            verify_evm_recovery_payload(payload, &digest, &format!("0x{}", "0".repeat(64)))
                .is_err()
        );
    }

    /// Guards the top-level `--stellar-rpc-env` global: it must NOT be
    /// accepted-then-dropped. clap global propagation decides whether a
    /// top-level value reaches a subcommand's own flattened RpcArgs; this
    /// asserts the intended delivery contract stays true.
    #[test]
    fn global_rpc_flag_delivers_to_subcommand_or_is_dropped() {
        use super::{Cli, Command};
        use clap::Parser as _;
        let cli = Cli::try_parse_from([
            "prog",
            "--stellar-rpc-env",
            "MY_STELLAR_URL",
            "init",
            "--desired",
            "desired.json",
            "--state",
            "state",
            "--write",
        ])
        .expect("parse with top-level global rpc flag");
        assert_eq!(
            cli.rpc.stellar_rpc_env.as_deref(),
            Some("MY_STELLAR_URL"),
            "top-level flatten must hold the value"
        );
        match cli.command {
            Command::Init(args) => assert_eq!(
                args.rpc.stellar_rpc_env.as_deref(),
                Some("MY_STELLAR_URL"),
                "clap must propagate the global value into the subcommand RpcArgs"
            ),
            _ => unreachable!(),
        }
    }
}
