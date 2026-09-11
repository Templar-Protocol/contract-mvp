use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use crate::types::{
    AdapterAdminArg, AddressStr, DecimalAmount, GovernanceActionKindArg, RestrictionModeArg,
    ShareDecimalsArg, SourceAccount, SupplyQueueEntryArg, TimelockKindArg, WasmHash,
};

pub const DEFAULT_CONTRACT_SOURCE_REPO: &str = "github:Templar-Protocol/contracts";

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Deploy and operate Templar Soroban vault stacks",
    long_about = "Deploy Templar Soroban vault contracts, reuse previously uploaded WASM where possible, and run typed user, curator, governance, share-token, and adapter operations through the Stellar CLI."
)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "CLI flags are independent switches"
)]
pub struct Cli {
    /// Load defaults from a named public TOML profile
    #[arg(long, env = "TEMPLAR_SOROBAN_VAULT_PROFILE")]
    pub profile: Option<String>,

    /// Stellar network name from the local stellar config
    #[arg(long, env = "SOROBAN_NETWORK", default_value = "testnet")]
    pub network: String,

    /// RPC URL override
    #[arg(long, env = "SOROBAN_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Network passphrase override
    #[arg(
        long,
        env = "SOROBAN_NETWORK_PASSPHRASE",
        default_value = "Test SDF Network ; September 2015"
    )]
    pub network_passphrase: String,

    /// Stellar source account identity alias, public key, or muxed account. Do not pass secret keys or seed phrases here; use Stellar keystore/default identity or STELLAR_ACCOUNT.
    #[arg(long, env = "SOROBAN_IDENTITY")]
    pub source_account: Option<SourceAccount>,

    /// Stellar config directory
    #[arg(long, env = "STELLAR_CONFIG_DIR")]
    pub config_dir: Option<PathBuf>,

    /// Deployment manifest path
    #[arg(
        long,
        env = "TEMPLAR_SOROBAN_VAULT_STATE",
        default_value = "contract/vault/soroban/.deploy-state/manifest.json"
    )]
    pub state: PathBuf,

    /// Require an unused deployment manifest path for a new stack
    #[arg(long)]
    pub fresh_state: bool,

    /// Path to the workspace root
    #[arg(long, env = "WORKSPACE_PATH", default_value = ".")]
    pub workspace_path: PathBuf,

    /// Contract source repository metadata embedded into future WASM builds. Use an empty value to disable.
    #[arg(
        long,
        env = "SOROBAN_CONTRACT_SOURCE_REPO",
        default_value = DEFAULT_CONTRACT_SOURCE_REPO
    )]
    pub contract_source_repo: Option<String>,

    /// Output machine-readable JSON
    #[arg(long)]
    pub json: bool,

    /// Output newline-delimited machine-readable JSON envelopes
    #[arg(long)]
    pub json_lines: bool,

    /// Print commands and manifest decisions without running writes
    #[arg(long)]
    pub dry_run: bool,

    /// Confirm overwrite or guarded production actions
    #[arg(long)]
    pub yes: bool,

    /// Permit mainnet write commands
    #[arg(long)]
    pub allow_mainnet_write: bool,

    /// Permit deploying governance with a zero timelock
    #[arg(long)]
    pub allow_zero_timelock: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Check local operator readiness before deployment or write operations
    Doctor,
    /// Deploy, verify, or reuse vault stack components
    Deploy(DeployArgs),
    /// Reconcile the manifest against chain state and print a repair plan
    Reconcile(ReconcileArgs),
    /// User-facing vault/share operations
    User(UserArgs),
    /// Curator and allocator operations
    Curator(CuratorArgs),
    /// Governance proposal operations
    Governance(GovernanceArgs),
    /// Share token operations and views
    ShareToken(ShareTokenArgs),
    /// Adapter operations and views
    Adapter(AdapterArgs),
    /// Extend TTL for every TTL-capable contract recorded in the deployment manifest
    ExtendTtl(ExtendTtlArgs),
    /// Print deployment status from the manifest
    Status,
    /// Export manifest values as shell environment assignments
    ExportEnv,
    /// Create or manage public CLI profile files
    Profile(ProfileArgs),
    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Generate a roff manpage on stdout
    Man,
}

impl Commands {
    pub fn is_write(&self) -> bool {
        match self {
            Self::Doctor
            | Self::Status
            | Self::ExportEnv
            | Self::Profile(_)
            | Self::Completions { .. }
            | Self::Reconcile(_)
            | Self::Man => false,
            Self::Deploy(args) => args.command.is_write(),
            Self::ExtendTtl(_) => true,
            Self::User(args) => args.command.is_write(),
            Self::Curator(args) => args.command.is_write(),
            Self::Governance(args) => args.command.is_write(),
            Self::ShareToken(args) => args.command.is_write(),
            Self::Adapter(args) => args.command.is_write(),
        }
    }
}

#[derive(Args, Debug)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Subcommand, Debug)]
pub enum ProfileCommand {
    /// Create a public TOML profile template
    Init {
        /// Profile name, for example testnet
        name: String,
        /// Overwrite an existing profile file
        #[arg(long)]
        force: bool,
    },
}

#[derive(Args, Debug)]
pub struct ExtendTtlArgs {
    /// Deprecated compatibility option. Aggregate TTL maintenance no longer
    /// requires a contract-admin caller.
    #[arg(long, env = "SOROBAN_TTL_CALLER")]
    pub caller: Option<AddressStr>,
}

#[derive(Args, Debug)]
pub struct DeployArgs {
    #[command(subcommand)]
    pub command: DeployCommand,
}

#[derive(Args, Debug)]
pub struct ReconcileArgs {
    /// Skip view calls that verify initialized wiring where contract getters are available
    #[arg(long)]
    pub skip_view_verification: bool,
}

#[derive(Subcommand, Debug)]
pub enum DeployCommand {
    /// Plan a deployment without uploading WASM, deploying contracts, or writing the manifest
    Plan(DeployPlanArgs),
    /// Deploy or reuse a full vault stack
    Stack(Box<DeployStackArgs>),
    /// Resume a checkpointed stack deployment after checking recorded chain state
    Resume(Box<DeployStackArgs>),
    /// Print the safe repair/resume plan for the current manifest
    Repair(ReconcileArgs),
    /// Add Blend or custodial adapters to an existing or imported vault deployment
    Adapters(DeployAdaptersArgs),
    /// Deploy a fresh curator proxy for an existing or imported vault deployment
    CuratorProxy(DeployCuratorProxyArgs),
    /// Upload or verify a single WASM artifact
    Wasm(DeployWasmArgs),
}

impl DeployCommand {
    pub const fn is_write(&self) -> bool {
        !matches!(self, Self::Plan(_) | Self::Repair(_))
    }
}

#[derive(Args, Debug)]
pub struct DeployPlanArgs {
    #[command(subcommand)]
    pub command: DeployPlanCommand,
}

#[derive(Subcommand, Debug)]
pub enum DeployPlanCommand {
    /// Plan a full vault stack deployment or reuse flow
    Stack(Box<DeployStackArgs>),
    /// Plan appending Blend or custodial adapters to an existing or imported deployment
    Adapters(DeployAdaptersArgs),
}

#[derive(Args, Debug)]
pub struct DeployStackArgs {
    /// Governance admin, initial vault curator, and share-token admin. Defaults to `stellar keys address <source-account>`.
    #[arg(long, env = "SOROBAN_ADMIN")]
    pub admin: Option<AddressStr>,

    /// Existing asset token contract address. If omitted, native asset SAC is deployed/resolved.
    #[arg(long, env = "SOROBAN_ASSET_TOKEN")]
    pub asset_token: Option<AddressStr>,

    /// Governance timelock in nanoseconds for a new governance contract
    #[arg(long, env = "SOROBAN_GOV_TIMELOCK_NS")]
    pub governance_timelock_ns: Option<u64>,

    /// Initial virtual shares offset
    #[arg(long, env = "SOROBAN_VIRTUAL_SHARES", default_value_t = 0)]
    pub virtual_shares: i128,

    /// Initial virtual assets offset
    #[arg(long, env = "SOROBAN_VIRTUAL_ASSETS", default_value_t = 0)]
    pub virtual_assets: i128,

    /// Share token display name
    #[arg(
        long,
        env = "SOROBAN_SHARE_NAME",
        default_value = "Templar Vault Share"
    )]
    pub share_name: String,

    /// Share token symbol
    #[arg(long, env = "SOROBAN_SHARE_SYMBOL", default_value = "tvSHARE")]
    pub share_symbol: String,

    /// Share token decimals
    #[arg(long, env = "SOROBAN_SHARE_DECIMALS", default_value_t = 7)]
    pub share_decimals: u32,

    /// Blend pool contract address. Repeat the flag, or provide comma-separated BLEND_POOL_ID values, to deploy multiple Blend adapters.
    #[arg(long = "blend-pool", env = "BLEND_POOL_ID", value_delimiter = ',')]
    pub blend_pools: Vec<AddressStr>,

    /// Custodian or multisig address. Repeat the flag, or provide comma-separated CUSTODIAL_ADDRESS values, to deploy custodial adapters.
    #[arg(long = "custodian", env = "CUSTODIAL_ADDRESS", value_delimiter = ',')]
    pub custodians: Vec<AddressStr>,

    /// Admin for every newly deployed Blend or custodial adapter. Required when adapters are requested. Use `vault` only when the runtime advertises companion-upgrade support.
    #[arg(long, env = "SOROBAN_ADAPTER_ADMIN")]
    pub adapter_admin: Option<AdapterAdminArg>,

    /// Build from the checked-out workspace instead of using pinned release artifacts.
    #[arg(long)]
    pub build: bool,

    /// Deploy fresh contract instances instead of reusing manifest ids
    #[arg(long)]
    pub force_new: bool,
}

#[derive(Args, Debug)]
pub struct DeployAdaptersArgs {
    /// Existing vault contract address. Required when the manifest does not already contain `vault`.
    #[arg(long, env = "SOROBAN_VAULT")]
    pub vault: Option<AddressStr>,

    /// Existing governance contract address. Required when the manifest does not already contain `governance`.
    #[arg(long, env = "SOROBAN_GOVERNANCE")]
    pub governance: Option<AddressStr>,

    /// Existing asset token contract address to record for imported deployments.
    #[arg(long, env = "SOROBAN_ASSET_TOKEN")]
    pub asset_token: Option<AddressStr>,

    /// Blend pool contract address. Repeat the flag, or provide comma-separated BLEND_POOL_ID values, to append multiple adapters.
    #[arg(long = "blend-pool", env = "BLEND_POOL_ID", value_delimiter = ',')]
    pub blend_pools: Vec<AddressStr>,

    /// Custodian or multisig address. Repeat the flag, or provide comma-separated CUSTODIAL_ADDRESS values, to append multiple custodial adapters.
    #[arg(long = "custodian", env = "CUSTODIAL_ADDRESS", value_delimiter = ',')]
    pub custodians: Vec<AddressStr>,

    /// Admin for every newly deployed Blend or custodial adapter. Required for this command. Use `vault` only when the runtime advertises companion-upgrade support.
    #[arg(long, env = "SOROBAN_ADAPTER_ADMIN")]
    pub adapter_admin: AdapterAdminArg,

    /// Build from the checked-out workspace instead of using pinned release artifacts.
    #[arg(long)]
    pub build: bool,

    /// Deploy a fresh adapter even if an adapter for the same pool already exists
    #[arg(long)]
    pub force_new: bool,
}

#[derive(Args, Debug)]
pub struct DeployCuratorProxyArgs {
    /// Existing vault contract address. Required when the manifest does not already contain `vault`.
    #[arg(long, env = "SOROBAN_VAULT")]
    pub vault: Option<AddressStr>,

    /// Existing governance contract address. Required when the manifest does not already contain `governance`.
    #[arg(long, env = "SOROBAN_GOVERNANCE")]
    pub governance: Option<AddressStr>,

    /// Verified current vault WASM hash for an approved versionless v1 runtime.
    #[arg(long, env = "SOROBAN_LEGACY_V1_WASM_HASH")]
    pub legacy_v1_wasm_hash: Option<WasmHash>,

    /// Build from the checked-out workspace instead of using the pinned release artifact.
    #[arg(long)]
    pub build: bool,

    /// Abandon a matching checkpointed proxy and deploy a fresh instance.
    #[arg(long)]
    pub force_new: bool,
}

#[derive(Args, Debug)]
pub struct DeployWasmArgs {
    /// Known artifact name
    #[arg(value_enum)]
    pub artifact: ArtifactName,

    /// Build from the checked-out workspace instead of using the pinned release artifact.
    #[arg(long)]
    pub build: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, ValueEnum)]
pub enum ArtifactName {
    Vault,
    Governance,
    ShareToken,
    BlendAdapter,
    CustodialAdapter,
    Proxy4626,
    CuratorProxy,
}

#[derive(Args, Debug)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommand,
}

#[derive(Subcommand, Debug)]
pub enum UserCommand {
    /// Deposit assets through the ERC-4626 proxy when deployed, otherwise through the vault command payload.
    Deposit {
        /// Authorized operator address spending the assets.
        #[arg(long)]
        operator: AddressStr,
        /// Share receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Asset amount in display units, converted using --asset-decimals.
        #[arg(long, conflicts_with = "assets_raw")]
        assets: Option<DecimalAmount>,
        /// Asset amount in raw contract base units.
        #[arg(long)]
        assets_raw: Option<i128>,
        /// Asset token decimals used for --assets.
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
        /// Minimum shares accepted in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "min_shares_out_raw")]
        min_shares_out: Option<DecimalAmount>,
        /// Minimum shares accepted in raw share-token base units.
        #[arg(long, default_value_t = 0)]
        min_shares_out_raw: i128,
        /// Share token decimals for --min-shares-out; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
    /// Mint an exact number of shares through the ERC-4626 proxy.
    Mint {
        /// Authorized operator address spending the assets.
        #[arg(long)]
        operator: AddressStr,
        /// Share receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share amount in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        /// Share amount in raw share-token base units.
        #[arg(long)]
        shares_raw: Option<i128>,
        /// Share token decimals for --shares; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
    /// Queue an asynchronous withdrawal by exact asset amount through the ERC-4626 proxy.
    Withdraw {
        /// Authorized operator address. The current proxy requires --operator to equal --owner.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Asset amount in display units, converted using --asset-decimals.
        #[arg(long, conflicts_with = "assets_raw")]
        assets: Option<DecimalAmount>,
        /// Asset amount in raw contract base units.
        #[arg(long)]
        assets_raw: Option<i128>,
        /// Asset token decimals used for --assets.
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
    },
    /// Queue an asynchronous redemption by exact share amount through the ERC-4626 proxy.
    Redeem {
        /// Authorized operator address. The current proxy requires --operator to equal --owner.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Share amount in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        /// Share amount in raw share-token base units.
        #[arg(long)]
        shares_raw: Option<i128>,
        /// Share token decimals for --shares; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
    /// Atomically withdraw through the ERC-4626 proxy when deployed, otherwise through the vault.
    AtomicWithdraw {
        /// Authorized operator address burning shares.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Asset amount in display units, converted using --asset-decimals.
        #[arg(long, conflicts_with = "assets_raw")]
        assets: Option<DecimalAmount>,
        /// Asset amount in raw contract base units.
        #[arg(long)]
        assets_raw: Option<i128>,
        /// Asset token decimals used for --assets.
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
        /// Maximum shares allowed to burn in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "max_shares_burned_raw")]
        max_shares_burned: Option<DecimalAmount>,
        /// Maximum shares allowed to burn in raw share-token base units. Defaults to raw assets for operator workflows.
        #[arg(long)]
        max_shares_burned_raw: Option<i128>,
        /// Share token decimals for --max-shares-burned; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
    /// Atomically redeem through the ERC-4626 proxy when deployed, otherwise through the vault.
    AtomicRedeem {
        /// Authorized operator address burning shares.
        #[arg(long)]
        operator: AddressStr,
        /// Asset receiver address. Defaults to --operator.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share owner address. Defaults to --operator.
        #[arg(long)]
        owner: Option<AddressStr>,
        /// Share amount in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        /// Share amount in raw share-token base units.
        #[arg(long)]
        shares_raw: Option<i128>,
        /// Share token decimals for --shares; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
        /// Minimum assets accepted in display units, converted using --asset-decimals.
        #[arg(long, conflicts_with = "min_assets_out_raw")]
        min_assets_out: Option<DecimalAmount>,
        /// Minimum assets accepted in raw contract base units.
        #[arg(long, default_value_t = 0)]
        min_assets_out_raw: i128,
        /// Asset token decimals used for --min-assets-out.
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
    },
    /// Queue a delayed withdrawal request directly against the vault.
    RequestWithdraw {
        /// Share owner address.
        #[arg(long)]
        owner: AddressStr,
        /// Asset receiver address. Defaults to --owner.
        #[arg(long)]
        receiver: Option<AddressStr>,
        /// Share amount to burn after cooldown in display units, converted using --share-decimals.
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        /// Share amount to burn after cooldown in raw share-token base units.
        #[arg(long)]
        shares_raw: Option<i128>,
        /// Share token decimals for --shares; `manifest` reads share_token constructor args.
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
        /// Minimum assets accepted when the withdrawal executes in display units.
        #[arg(long, conflicts_with = "min_assets_out_raw")]
        min_assets_out: Option<DecimalAmount>,
        /// Minimum assets accepted when the withdrawal executes in raw contract base units.
        #[arg(long, default_value_t = 0)]
        min_assets_out_raw: i128,
        /// Asset token decimals used for --min-assets-out.
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
    },
    /// Execute the next claimable queued withdrawal as an authorized allocator/keeper.
    ExecuteWithdraw {
        /// Authorized allocator/keeper address executing the queue head.
        #[arg(long)]
        operator: AddressStr,
    },
    /// Read share-token balance for an owner.
    Balance {
        #[arg(long)]
        owner: AddressStr,
    },
    /// Preview vault/proxy conversion and limit values for an owner.
    Preview {
        #[arg(long)]
        owner: AddressStr,
        #[arg(long, conflicts_with = "assets_raw")]
        assets: Option<DecimalAmount>,
        #[arg(long, default_value_t = 0)]
        assets_raw: i128,
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        #[arg(long, default_value_t = 0)]
        shares_raw: i128,
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
    /// Alias for preview that preserves the underlying vault view naming.
    View {
        #[arg(long)]
        owner: AddressStr,
        #[arg(long, conflicts_with = "assets_raw")]
        assets: Option<DecimalAmount>,
        #[arg(long, default_value_t = 0)]
        assets_raw: i128,
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
        #[arg(long, conflicts_with = "shares_raw")]
        shares: Option<DecimalAmount>,
        #[arg(long, default_value_t = 0)]
        shares_raw: i128,
        #[arg(long, default_value = "manifest")]
        share_decimals: ShareDecimalsArg,
    },
}

impl UserCommand {
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::Balance { .. } | Self::Preview { .. } | Self::View { .. }
        )
    }
}

#[derive(Args, Debug)]
pub struct CuratorArgs {
    #[command(subcommand)]
    pub command: CuratorCommand,
}

#[derive(Subcommand, Debug)]
pub enum CuratorCommand {
    /// Supply a positive asset amount to a market through the vault command payload.
    AllocateSupply {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        market: u32,
        #[arg(long, conflicts_with = "amount_raw")]
        amount: Option<DecimalAmount>,
        #[arg(long)]
        amount_raw: Option<i128>,
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
    },
    /// Request a positive asset amount back from a market through the vault command payload.
    AllocateWithdraw {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        market: u32,
        #[arg(long, conflicts_with = "amount_raw")]
        amount: Option<DecimalAmount>,
        #[arg(long)]
        amount_raw: Option<i128>,
        #[arg(long, default_value_t = 7)]
        asset_decimals: u32,
    },
    /// Abort a stale in-flight withdrawal operation and return the vault to idle.
    AbortWithdrawing {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        op_id: u64,
    },
    /// Refresh one or more market ids through the vault command payload.
    RefreshMarkets {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long, value_delimiter = ',')]
        markets: Vec<u32>,
    },
    /// Refresh fee accounting through the vault command payload.
    RefreshFees,
    /// Resynchronize idle asset accounting through the vault command payload.
    ResyncIdle,
    /// Submit a governance proposal for the allowed adapter address set.
    SetAllowedAdapters {
        #[arg(long)]
        admin: AddressStr,
        #[arg(long, value_delimiter = ',')]
        adapters: Vec<AddressStr>,
        #[arg(long)]
        auto_accept: bool,
    },
    /// Submit a governance proposal for the supply queue using typed `SupplyQueueProposalEntry` values.
    SetSupplyQueue {
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
        #[arg(long)]
        auto_accept: bool,
    },
}

impl CuratorCommand {
    pub fn is_write(&self) -> bool {
        true
    }
}

#[derive(Args, Debug)]
pub struct GovernanceArgs {
    #[command(subcommand)]
    pub command: GovernanceCommand,
}

#[derive(Subcommand, Debug)]
pub enum GovernanceCommand {
    /// Plan accepting a pending proposal without submitting a transaction.
    PlanAccept {
        /// Governance admin address that would accept the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Pending proposal id to accept.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Plan a supply queue proposal without submitting a transaction.
    PlanSubmitSetSupplyQueue {
        /// Governance admin address that would submit the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
    },
    /// Plan a timelock proposal without submitting a transaction.
    PlanSubmitSetTimelock {
        /// Governance admin address that would submit the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Timelock kind from the governance contract.
        #[arg(long)]
        kind: TimelockKindArg,
        /// Proposed timelock duration in nanoseconds.
        #[arg(long)]
        timelock_ns: u64,
    },
    /// List pending proposals with decoded readiness when the view output exposes it.
    Queue {
        /// Optional action-kind filter, matched against decoded/raw proposal text.
        #[arg(long)]
        kind: Option<GovernanceActionKindArg>,
    },
    /// Explain one pending proposal with decoded readiness and raw contract output.
    Explain {
        /// Pending proposal id to inspect.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Accept every ready pending proposal, optionally filtered by action kind.
    AcceptReady {
        /// Governance admin address accepting ready proposals.
        #[arg(long)]
        admin: AddressStr,
        /// Optional action-kind filter, matched against decoded/raw proposal text.
        #[arg(long)]
        kind: Option<GovernanceActionKindArg>,
        /// Maximum number of ready proposals to accept.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Submit a typed proposal or wait on an existing proposal id, then accept it when ready.
    SubmitAndWait(GovernanceSubmitAndWaitArgs),
    /// Accept a pending proposal after its timelock has elapsed.
    Accept {
        /// Governance admin address accepting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Pending proposal id to accept.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Revoke a pending proposal before acceptance.
    Revoke {
        /// Governance admin address revoking the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Pending proposal id to revoke.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Print one proposal or list pending proposal ids.
    Pending {
        /// Optional proposal id to inspect. Omit to list pending ids.
        #[arg(long)]
        proposal_id: Option<u64>,
    },
    /// Print the current governance timelock configuration.
    Timelocks,
    /// Submit a proposal to change the governance admin address.
    SubmitSetAdmin {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement governance admin address.
        #[arg(long)]
        new_admin: AddressStr,
    },
    /// Submit a proposal to change the vault curator address.
    SubmitSetCurator {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement vault curator address.
        #[arg(long)]
        new_curator: AddressStr,
    },
    /// Submit a proposal to change the vault governance address.
    SubmitSetGovernance {
        /// Current governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed replacement governance contract address.
        #[arg(long)]
        new_governance: AddressStr,
    },
    /// Submit a timelocked unpause proposal. Omit --paused; Sentinel pause is a direct call.
    SubmitSetPaused {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Leave unset for false/unpause. Setting this flag requests true, which governance rejects.
        #[arg(long)]
        paused: bool,
    },
    /// Submit a proposal to replace the vault supply queue with typed target/adapter entries.
    SubmitSetSupplyQueue {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
    },
    /// Submit a proposal to update fee parameters.
    SubmitSetFees {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Performance fee as a WAD-scaled integer.
        #[arg(long)]
        performance_fee_wad: i128,
        /// Address that receives performance fees.
        #[arg(long)]
        performance_recipient: AddressStr,
        /// Management fee as a WAD-scaled integer.
        #[arg(long)]
        management_fee_wad: i128,
        /// Address that receives management fees.
        #[arg(long)]
        management_recipient: AddressStr,
        /// Optional maximum fee growth rate as a WAD-scaled integer.
        #[arg(long)]
        max_growth_rate_wad: Option<i128>,
    },
    /// Submit a proposal to update transfer restrictions.
    SubmitSetRestrictions {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Restriction mode: none, blacklist, or whitelist.
        #[arg(long)]
        mode: RestrictionModeArg,
        /// Restricted or allowed account addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        accounts: Vec<AddressStr>,
    },
    /// Submit a proposal to update the sentinel address.
    SubmitSetSentinel {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed sentinel address.
        #[arg(long)]
        sentinel: AddressStr,
    },
    /// Submit a proposal to replace allocator addresses.
    SubmitSetAllocators {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Allocator addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        allocators: Vec<AddressStr>,
    },
    /// Submit a proposal to replace allowed adapter addresses.
    SubmitSetAllowedAdapters {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Allowed adapter addresses, comma-separated or repeated.
        #[arg(long, value_delimiter = ',')]
        adapters: Vec<AddressStr>,
    },
    /// Submit a proposal to update one governance timelock kind.
    SubmitSetTimelock {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Timelock kind from the governance contract, for example supply-queue or migration.
        #[arg(long)]
        kind: TimelockKindArg,
        /// Proposed timelock duration in nanoseconds.
        #[arg(long)]
        timelock_ns: u64,
    },
    /// Submit a proposal to update an absolute market cap.
    SubmitSetCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id whose cap should change.
        #[arg(long)]
        market_id: u32,
        /// Proposed absolute cap in asset base units.
        #[arg(long)]
        cap: i128,
    },
    /// Submit a proposal to remove a market from the vault.
    SubmitRemoveMarket {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id to remove.
        #[arg(long)]
        market_id: u32,
    },
    /// Submit a proposal to update an absolute cap for a cap group.
    SubmitSetGroupCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Cap group identifier.
        #[arg(long)]
        group: String,
        /// Proposed group absolute cap in asset base units.
        #[arg(long)]
        cap: i128,
    },
    /// Submit a proposal to update a relative cap for a cap group.
    SubmitSetGroupRelCap {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Cap group identifier.
        #[arg(long)]
        group: String,
        /// Proposed relative cap as a WAD-scaled integer.
        #[arg(long)]
        relative_cap: i128,
    },
    /// Submit a proposal to assign a market to a cap group.
    SubmitSetGroupMember {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Market id whose group should change.
        #[arg(long)]
        market_id: u32,
        /// Cap group identifier to assign.
        #[arg(long)]
        group: String,
    },
    /// Submit a proposal to update the skim recipient.
    SubmitSetSkimRecipient {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed skim recipient address.
        #[arg(long)]
        recipient: AddressStr,
    },
    /// Submit a proposal to skim a token.
    SubmitSkim {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Token contract address to skim.
        #[arg(long)]
        token: AddressStr,
    },
    /// Submit a proposal to update the withdrawal cooldown.
    SubmitSetWithdrawalCooldown {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed withdrawal cooldown in nanoseconds.
        #[arg(long)]
        cooldown_ns: u64,
    },
    /// Submit a proposal to update the idle resync cooldown.
    SubmitSetIdleResyncCooldown {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed idle resync cooldown in nanoseconds.
        #[arg(long)]
        cooldown_ns: u64,
    },
    /// Submit a proposal to upgrade the vault WASM.
    SubmitUpgrade {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposed 32-byte vault WASM hash as hex.
        #[arg(long)]
        wasm_hash: WasmHash,
    },
    /// Submit the governance-controlled migration action.
    SubmitMigrate {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
    },
    /// Submit the governance-controlled cancel-migration action.
    SubmitCancelMigration {
        /// Governance admin address submitting the proposal.
        #[arg(long)]
        admin: AddressStr,
    },
    /// Permanently abdicate one governance action kind.
    Abdicate {
        /// Governance admin address abdicating the action kind.
        #[arg(long)]
        admin: AddressStr,
        /// Governance action kind from the contract enum.
        #[arg(long)]
        kind: GovernanceActionKindArg,
    },
}

#[derive(Args, Debug)]
pub struct GovernanceSubmitAndWaitArgs {
    /// Poll interval in seconds while waiting.
    #[arg(long, default_value_t = 15)]
    pub poll_seconds: u64,
    /// Maximum seconds to wait. Zero checks once and returns if not ready.
    #[arg(long, default_value_t = 0)]
    pub max_wait_seconds: u64,
    #[command(subcommand)]
    pub command: GovernanceSubmitAndWaitCommand,
}

#[derive(Subcommand, Debug)]
pub enum GovernanceSubmitAndWaitCommand {
    /// Wait on an already submitted proposal id, then accept it when ready.
    Proposal {
        /// Governance admin address accepting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Proposal id returned by a submit-* command.
        #[arg(long)]
        proposal_id: u64,
    },
    /// Submit a supply queue proposal, then wait for and accept it when ready.
    SetSupplyQueue {
        /// Governance admin address submitting and accepting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Supply queue entry formatted as `target_id:adapter_address`. Repeat for each queue item.
        #[arg(long = "entry")]
        entries: Vec<SupplyQueueEntryArg>,
    },
    /// Submit a timelock proposal, then wait for and accept it when ready.
    SetTimelock {
        /// Governance admin address submitting and accepting the proposal.
        #[arg(long)]
        admin: AddressStr,
        /// Timelock kind from the governance contract.
        #[arg(long)]
        kind: TimelockKindArg,
        /// Proposed timelock duration in nanoseconds.
        #[arg(long)]
        timelock_ns: u64,
    },
}

impl GovernanceCommand {
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::PlanAccept { .. }
                | Self::PlanSubmitSetSupplyQueue { .. }
                | Self::PlanSubmitSetTimelock { .. }
                | Self::Queue { .. }
                | Self::Explain { .. }
                | Self::Pending { .. }
                | Self::Timelocks
        )
    }
}

#[derive(Args, Debug)]
pub struct ShareTokenArgs {
    #[command(subcommand)]
    pub command: ShareTokenCommand,
}

#[derive(Subcommand, Debug)]
pub enum ShareTokenCommand {
    /// Read share balance for an account.
    Balance {
        #[arg(long)]
        account: AddressStr,
    },
    /// Read total share supply.
    TotalSupply,
    /// Read share-token admin.
    Admin,
    /// Read configured vault address.
    Vault,
    /// Extend share-token instance TTL.
    ExtendTtl {
        #[arg(long)]
        caller: AddressStr,
    },
}

impl ShareTokenCommand {
    pub fn is_write(&self) -> bool {
        matches!(self, Self::ExtendTtl { .. })
    }
}

#[derive(Args, Debug)]
pub struct AdapterArgs {
    /// Indexed Blend adapter to operate on, matching deploy order for repeated --blend-pool flags.
    #[arg(long, default_value_t = 0)]
    pub adapter_index: usize,

    /// Explicit manifest contract key, such as blend_adapter_1.
    #[arg(long)]
    pub adapter_key: Option<String>,

    /// Select the adapter whose constructor pool matches this address.
    #[arg(long)]
    pub adapter_pool: Option<AddressStr>,

    #[command(subcommand)]
    pub command: AdapterCommand,
}

#[derive(Subcommand, Debug)]
pub enum AdapterCommand {
    /// Read adapter total assets for a token address.
    TotalAssets {
        #[arg(long)]
        asset: AddressStr,
    },
    /// Read adapter admin.
    Admin,
    /// Read configured vault address.
    Vault,
    /// Read configured Blend pool address.
    Pool,
    /// Set a pending adapter admin.
    SetAdmin {
        #[arg(long)]
        caller: AddressStr,
        #[arg(long)]
        admin: AddressStr,
    },
    /// Accept a pending adapter admin handoff.
    AcceptAdmin {
        #[arg(long)]
        caller: AddressStr,
    },
    /// Extend adapter instance TTL.
    ExtendTtl {
        #[arg(long)]
        caller: AddressStr,
    },
}

impl AdapterCommand {
    pub fn is_write(&self) -> bool {
        !matches!(
            self,
            Self::TotalAssets { .. } | Self::Admin | Self::Vault | Self::Pool
        )
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{
        Cli, Commands, DeployCommand, ExtendTtlArgs, ReconcileArgs, UserArgs, UserCommand,
    };

    const ADMIN: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
    const POOL: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

    #[test]
    fn parses_deploy_stack_flags() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "--source-account",
            "alice",
            "--fresh-state",
            "deploy",
            "stack",
            "--admin",
            ADMIN,
            "--governance-timelock-ns",
            "1000",
            "--blend-pool",
            POOL,
            "--blend-pool",
            POOL,
            "--custodian",
            ADMIN,
            "--adapter-admin",
            ADMIN,
        ])
        .expect("parse cli");

        assert!(cli.fresh_state);

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Stack(stack) => {
                    assert_eq!(
                        stack.admin.as_ref().map(ToString::to_string).as_deref(),
                        Some(ADMIN)
                    );
                    assert_eq!(stack.governance_timelock_ns, Some(1000));
                    assert_eq!(stack.blend_pools.len(), 2);
                    assert_eq!(stack.custodians.len(), 1);
                    assert_eq!(
                        stack
                            .adapter_admin
                            .as_ref()
                            .map(ToString::to_string)
                            .as_deref(),
                        Some(ADMIN)
                    );
                }
                DeployCommand::Plan(_)
                | DeployCommand::Resume(_)
                | DeployCommand::Repair(_)
                | DeployCommand::Adapters(_)
                | DeployCommand::CuratorProxy(_)
                | DeployCommand::Wasm(_) => panic!("expected deploy stack"),
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn deploy_build_flags_are_explicit_opt_ins() {
        for args in [
            vec!["deploy", "stack", "--admin", ADMIN],
            vec![
                "deploy",
                "adapters",
                "--blend-pool",
                POOL,
                "--adapter-admin",
                ADMIN,
            ],
            vec!["deploy", "curator-proxy", "--vault", POOL],
            vec!["deploy", "wasm", "vault"],
        ] {
            let cli = Cli::try_parse_from(
                std::iter::once("tmplr-soroban-vault").chain(args.iter().copied()),
            )
            .expect("parse default deploy source");
            let build = match cli.command {
                Commands::Deploy(args) => match args.command {
                    DeployCommand::Stack(args) | DeployCommand::Resume(args) => args.build,
                    DeployCommand::Adapters(args) => args.build,
                    DeployCommand::CuratorProxy(args) => args.build,
                    DeployCommand::Wasm(args) => args.build,
                    DeployCommand::Plan(_) | DeployCommand::Repair(_) => {
                        panic!("expected artifact deploy command")
                    }
                },
                _ => panic!("expected deploy command"),
            };
            assert!(!build);

            let cli = Cli::try_parse_from(
                std::iter::once("tmplr-soroban-vault")
                    .chain(args.iter().copied())
                    .chain(std::iter::once("--build")),
            )
            .expect("parse explicit build");
            let build = match cli.command {
                Commands::Deploy(args) => match args.command {
                    DeployCommand::Stack(args) | DeployCommand::Resume(args) => args.build,
                    DeployCommand::Adapters(args) => args.build,
                    DeployCommand::CuratorProxy(args) => args.build,
                    DeployCommand::Wasm(args) => args.build,
                    DeployCommand::Plan(_) | DeployCommand::Repair(_) => {
                        panic!("expected artifact deploy command")
                    }
                },
                _ => panic!("expected deploy command"),
            };
            assert!(build);
        }

        Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "wasm",
            "vault",
            "--build",
            "false",
        ])
        .expect_err("legacy --build false must stay rejected");
    }

    #[test]
    fn parses_comma_delimited_custodians() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "stack",
            "--admin",
            ADMIN,
            "--custodian",
            &format!("{ADMIN},{POOL}"),
            "--adapter-admin",
            "vault",
        ])
        .expect("parse cli");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Stack(stack) => {
                    let custodians = stack
                        .custodians
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>();
                    assert_eq!(custodians, vec![ADMIN, POOL]);
                    assert_eq!(
                        stack.adapter_admin.as_ref().map(ToString::to_string),
                        Some("vault".to_string())
                    );
                }
                DeployCommand::Plan(_)
                | DeployCommand::Resume(_)
                | DeployCommand::Repair(_)
                | DeployCommand::Adapters(_)
                | DeployCommand::CuratorProxy(_)
                | DeployCommand::Wasm(_) => panic!("expected deploy stack"),
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn parses_additive_adapter_deploy_flags() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "adapters",
            "--vault",
            POOL,
            "--governance",
            POOL,
            "--asset-token",
            POOL,
            "--blend-pool",
            POOL,
            "--custodian",
            ADMIN,
            "--adapter-admin",
            ADMIN,
        ])
        .expect("parse cli");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Adapters(args) => {
                    assert_eq!(
                        args.vault.as_ref().map(ToString::to_string).as_deref(),
                        Some(POOL)
                    );
                    assert_eq!(args.blend_pools.len(), 1);
                    assert_eq!(args.custodians.len(), 1);
                    assert_eq!(args.adapter_admin.to_string(), ADMIN);
                }
                DeployCommand::Plan(_)
                | DeployCommand::Stack(_)
                | DeployCommand::Resume(_)
                | DeployCommand::Repair(_)
                | DeployCommand::CuratorProxy(_)
                | DeployCommand::Wasm(_) => panic!("expected deploy adapters"),
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn deploy_adapters_rejects_missing_explicit_admin() {
        let error = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "adapters",
            "--vault",
            POOL,
            "--governance",
            POOL,
            "--blend-pool",
            POOL,
        ])
        .expect_err("adapter admin must be required");

        assert!(error.to_string().contains("--adapter-admin"));
    }

    #[test]
    fn parses_deploy_plan_stack_flags() {
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "plan",
            "stack",
            "--admin",
            ADMIN,
            "--governance-timelock-ns",
            "1000",
        ])
        .expect("parse deploy plan");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::Plan(plan) => match plan.command {
                    super::DeployPlanCommand::Stack(stack) => {
                        assert_eq!(
                            stack.admin.as_ref().map(ToString::to_string).as_deref(),
                            Some(ADMIN)
                        );
                    }
                    super::DeployPlanCommand::Adapters(_) => panic!("expected stack plan"),
                },
                DeployCommand::Stack(_)
                | DeployCommand::Resume(_)
                | DeployCommand::Repair(_)
                | DeployCommand::Adapters(_)
                | DeployCommand::CuratorProxy(_)
                | DeployCommand::Wasm(_) => panic!("expected deploy plan"),
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn parses_deployment_extend_ttl_command() {
        let current =
            Cli::try_parse_from(["tmplr-soroban-vault", "extend-ttl"]).expect("parse current CLI");
        let legacy = Cli::try_parse_from(["tmplr-soroban-vault", "extend-ttl", "--caller", ADMIN])
            .expect("parse legacy CLI");

        assert!(matches!(
            current.command,
            Commands::ExtendTtl(ExtendTtlArgs { caller: None })
        ));
        assert!(matches!(
            legacy.command,
            Commands::ExtendTtl(ExtendTtlArgs { caller: Some(_) })
        ));
    }

    #[test]
    fn parses_targeted_legacy_curator_proxy_deploy() {
        let legacy_hash = "11".repeat(32);
        let cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "curator-proxy",
            "--vault",
            POOL,
            "--governance",
            POOL,
            "--legacy-v1-wasm-hash",
            &legacy_hash,
            "--force-new",
        ])
        .expect("parse targeted curator proxy deploy");

        match cli.command {
            Commands::Deploy(args) => match args.command {
                DeployCommand::CuratorProxy(args) => {
                    assert_eq!(
                        args.vault.as_ref().map(ToString::to_string).as_deref(),
                        Some(POOL)
                    );
                    assert_eq!(
                        args.governance.as_ref().map(ToString::to_string).as_deref(),
                        Some(POOL)
                    );
                    assert_eq!(
                        args.legacy_v1_wasm_hash
                            .as_ref()
                            .map(ToString::to_string)
                            .as_deref(),
                        Some(legacy_hash.as_str())
                    );
                    assert!(!args.build);
                    assert!(args.force_new);
                }
                DeployCommand::Plan(_)
                | DeployCommand::Stack(_)
                | DeployCommand::Resume(_)
                | DeployCommand::Repair(_)
                | DeployCommand::Adapters(_)
                | DeployCommand::Wasm(_) => panic!("expected deploy curator-proxy"),
            },
            _ => panic!("expected deploy command"),
        }
    }

    #[test]
    fn parses_reconcile_and_deploy_recovery_commands() {
        let reconcile =
            Cli::try_parse_from(["tmplr-soroban-vault", "reconcile"]).expect("reconcile");
        assert!(matches!(
            reconcile.command,
            Commands::Reconcile(ReconcileArgs {
                skip_view_verification: false
            })
        ));

        let skip_views = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "reconcile",
            "--skip-view-verification",
        ])
        .expect("reconcile without view checks");
        assert!(matches!(
            skip_views.command,
            Commands::Reconcile(ReconcileArgs {
                skip_view_verification: true
            })
        ));

        let repair = Cli::try_parse_from(["tmplr-soroban-vault", "deploy", "repair"])
            .expect("deploy repair");
        assert!(matches!(
            repair.command,
            Commands::Deploy(args) if matches!(args.command, DeployCommand::Repair(_))
        ));

        let resume = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "deploy",
            "resume",
            "--governance-timelock-ns",
            "1000",
        ])
        .expect("deploy resume");
        assert!(matches!(
            resume.command,
            Commands::Deploy(args) if matches!(args.command, DeployCommand::Resume(_))
        ));
    }

    #[test]
    fn parses_decimal_and_raw_user_amount_flags() {
        let decimal_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "deposit",
            "--operator",
            ADMIN,
            "--assets",
            "1.25",
            "--asset-decimals",
            "7",
            "--min-shares-out-raw",
            "0",
        ])
        .expect("parse decimal deposit");
        assert!(matches!(decimal_cli.command, Commands::User(_)));

        let raw_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "deposit",
            "--operator",
            ADMIN,
            "--assets-raw",
            "12500000",
        ])
        .expect("parse raw deposit");
        assert!(matches!(raw_cli.command, Commands::User(_)));
    }

    #[test]
    fn parses_async_and_atomic_user_exit_commands() {
        let async_withdraw = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "withdraw",
            "--operator",
            ADMIN,
            "--assets-raw",
            "1",
        ])
        .expect("parse async withdraw");
        assert!(matches!(
            async_withdraw.command,
            Commands::User(UserArgs {
                command: UserCommand::Withdraw { .. }
            })
        ));

        let atomic_withdraw = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "atomic-withdraw",
            "--operator",
            ADMIN,
            "--assets-raw",
            "1",
            "--max-shares-burned-raw",
            "1",
        ])
        .expect("parse atomic withdraw");
        assert!(matches!(
            atomic_withdraw.command,
            Commands::User(UserArgs {
                command: UserCommand::AtomicWithdraw { .. }
            })
        ));

        let async_redeem = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "redeem",
            "--operator",
            ADMIN,
            "--shares-raw",
            "1",
        ])
        .expect("parse async redeem");
        assert!(matches!(
            async_redeem.command,
            Commands::User(UserArgs {
                command: UserCommand::Redeem { .. }
            })
        ));

        let atomic_redeem = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "atomic-redeem",
            "--operator",
            ADMIN,
            "--shares-raw",
            "1",
            "--min-assets-out-raw",
            "1",
        ])
        .expect("parse atomic redeem");
        assert!(matches!(
            atomic_redeem.command,
            Commands::User(UserArgs {
                command: UserCommand::AtomicRedeem { .. }
            })
        ));
    }

    #[test]
    fn rejects_invalid_soroban_addresses() {
        let err = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "user",
            "balance",
            "--owner",
            "not-an-address",
        ])
        .expect_err("invalid address should fail");

        assert!(err.to_string().contains("invalid Soroban"));
    }

    #[test]
    fn rejects_secret_source_account_cli_values() {
        let err = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "--source-account",
            "SC36XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
            "status",
        ])
        .expect_err("secret source account should fail");

        assert!(err.to_string().contains("do not pass secret keys"));
    }

    #[test]
    fn rejects_free_form_governance_action_kind() {
        let err = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "abdicate",
            "--admin",
            ADMIN,
            "--kind",
            "whatever",
        ])
        .expect_err("unknown governance kind should fail");

        assert!(err.to_string().contains("unknown governance action kind"));
    }

    #[test]
    fn parses_typed_governance_timelock_and_restriction_commands() {
        let timelock_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "submit-set-timelock",
            "--admin",
            ADMIN,
            "--kind",
            "supply-queue",
            "--timelock-ns",
            "1000",
        ])
        .expect("parse timelock command");
        assert!(matches!(timelock_cli.command, Commands::Governance(_)));

        let restrictions_cli = Cli::try_parse_from([
            "tmplr-soroban-vault",
            "governance",
            "submit-set-restrictions",
            "--admin",
            ADMIN,
            "--mode",
            "whitelist",
            "--accounts",
            ADMIN,
        ])
        .expect("parse restrictions command");
        assert!(matches!(restrictions_cli.command, Commands::Governance(_)));
    }
}
