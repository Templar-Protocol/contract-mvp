//! Command dispatch: map each parsed clap command to gateway reads and writes.
//!
//! Most arms are a direct `ctx.read`/`ctx.write` of a typed spec; the multi-step
//! flows live in focused submodules ([`teardown`], [`proposals`], [`generic`]) so
//! this file stays a readable index of the command surface.

mod aggregate;
pub(crate) mod export;
mod funding;
pub(crate) mod generic;
mod patch;
mod patch_dry_run;
mod patch_export;
mod patch_state;
pub(crate) mod plan;
mod preflight;
mod proposals;
mod reference;
mod teardown;
mod upgrade_preflight;
mod verify;

use crate::cli::Command;
use crate::commands::{
    account::AccountNs,
    contract::ContractNs,
    ft::FtNs,
    market::MarketNs,
    oracle::OracleNs,
    owner::OwnerNs,
    patch::PatchNs,
    proxy_oracle::{ProxyOracleGovernanceNs, ProxyOracleNs},
    pyth::PythNs,
    redstone::RedstoneNs,
    registry::RegistryNs,
    signer::Authorization,
    spec::SpecNs,
    storage::StorageNs,
};
use crate::context::{
    all_sources, lazer_source, print_json, pyth_source, redstone_source, CliContext,
};

/// A kernel price as a plain number.
///
/// Shared by the aggregation dry-run and the reference cross-check, which
/// compare against each other — two copies of this could drift into comparing
/// differently-scaled numbers.
fn scaled(price: &templar_proxy_oracle_kernel::Price) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        reason = "displayed, or compared within a tolerance band; never exact"
    )]
    let value = price.price as f64;
    value * 10f64.powi(price.expo)
}

pub(crate) async fn dispatch(ctx: CliContext, command: Command) -> anyhow::Result<()> {
    match command {
        Command::Account { command } => account(ctx, command).await,
        Command::Contract {
            command: ContractNs::GetVersion(a),
        } => ctx.read(a.into_spec()).await,
        Command::Registry { command } => registry(ctx, command).await,
        Command::Storage { command } => storage(ctx, command).await,
        Command::Ft { command } => ft(ctx, command).await,
        Command::Market { command } => market(ctx, command).await,
        Command::ProxyOracle { command } => proxy_oracle(ctx, command).await,
        Command::Owner { command } => owner(ctx, command).await,
        Command::Oracle { command } => oracle(ctx, command).await,
        Command::Pyth { command } => pyth(ctx, command).await,
        Command::Redstone { command } => redstone(ctx, command).await,
        Command::Patch { command } => patch(ctx, command).await,
        Command::Spec { command } => spec(ctx, command).await,
        Command::RecoverNep141(args) => teardown::recover_nep141(ctx, args).await,
        Command::Read(call) => generic::read(ctx, call).await,
        Command::Write(call) => generic::write(ctx, call).await,
    }
}

async fn spec(ctx: CliContext, ns: SpecNs) -> anyhow::Result<()> {
    match ns {
        SpecNs::Check(a) => preflight::check(ctx, a).await,
        SpecNs::PrintSchema => crate::commands::spec::print_schema(),
    }
}

async fn patch(ctx: CliContext, ns: PatchNs) -> anyhow::Result<()> {
    match ns {
        PatchNs::Plan(args) => patch::plan(ctx, args).await,
        PatchNs::DryRun(args) => patch_dry_run::dry_run(ctx, args).await,
        PatchNs::Apply(args) => patch::apply(ctx, args).await,
        PatchNs::Export(args) => patch_export::export(ctx, args).await,
        PatchNs::Codecs => crate::commands::patch::print_codecs(),
        PatchNs::PrintSchema => crate::commands::patch::print_schema(),
    }
}
async fn account(ctx: CliContext, ns: AccountNs) -> anyhow::Result<()> {
    match ns {
        AccountNs::Get(a) => ctx.read(a.into_spec()).await,
        AccountNs::Delete(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn registry(ctx: CliContext, ns: RegistryNs) -> anyhow::Result<()> {
    match ns {
        RegistryNs::ListVersions(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeployments(a) => ctx.read(a.into_spec()).await,
        RegistryNs::ListDeploymentsByKind(a) => ctx.read(a.into_spec()).await,
        RegistryNs::GetDeployment(a) => ctx.read(a.into_spec()).await,
        RegistryNs::AddVersion(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
        RegistryNs::Deploy(a) => {
            ctx.write_with(a.signer.clone(), |auth| a.try_into_spec(auth))
                .await
        }
        RegistryNs::RemoveVersion(a) => teardown::remove_version(ctx, a).await,
        RegistryNs::Remove(a) => teardown::registry_remove(ctx, a).await,
        RegistryNs::ClearDeployments(a) => teardown::clear_deployments(ctx, a).await,
    }
}

async fn storage(ctx: CliContext, ns: StorageNs) -> anyhow::Result<()> {
    match ns {
        StorageNs::GetBalanceBounds(a) => ctx.read(a.into_spec()).await,
        StorageNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        StorageNs::Deposit(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        StorageNs::Unregister(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        StorageNs::EnsureDeposit(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}

async fn ft(ctx: CliContext, ns: FtNs) -> anyhow::Result<()> {
    match ns {
        FtNs::GetBalanceOf(a) => ctx.read(a.into_spec()).await,
        FtNs::Transfer(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        FtNs::TransferCall(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn market(ctx: CliContext, ns: MarketNs) -> anyhow::Result<()> {
    match ns {
        MarketNs::Create(a) => {
            ctx.write_with(a.signer.clone(), |auth| a.try_into_spec(auth))
                .await
        }
        MarketNs::Export(a) => export::market(ctx, a).await,
        MarketNs::Plan(a) => plan::plan(ctx, a).await,
        MarketNs::Apply(a) => plan::apply(ctx, a).await,
        MarketNs::Verify(a) => verify::market(ctx, a).await,
        MarketNs::Remove(a) => {
            // `market remove` is self-signed: the signer is the market account
            // being torn down.
            let (market, client) = ctx.signing_client_for(&a.signer).await?;
            teardown::remove_market(&ctx, &client, market, a.beneficiary_id(), a.force()).await?;
            print_json(&Removed { removed: true })
        }
    }
}

/// `market remove` has nothing to report but success.
#[derive(serde::Serialize)]
struct Removed {
    removed: bool,
}

async fn proxy_oracle(ctx: CliContext, ns: ProxyOracleNs) -> anyhow::Result<()> {
    match ns {
        ProxyOracleNs::Create(a) => {
            ctx.write_with(a.signer.clone(), |auth| a.try_into_spec(auth))
                .await
        }
        ProxyOracleNs::GetProxy(a) => {
            let oracle_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(oracle_id)).await
        }
        ProxyOracleNs::ListProxies(a) => {
            let oracle_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(oracle_id)).await
        }
        ProxyOracleNs::PriceFeedExists(a) => {
            let oracle_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(oracle_id)).await
        }
        ProxyOracleNs::Preflight(a) => upgrade_preflight::command(ctx, a).await,
        ProxyOracleNs::GetProxyCircuitBreakerSet(a) => {
            let oracle_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(oracle_id)).await
        }
        ProxyOracleNs::UpdatePrices(a) => {
            let oracle_id = a.target.resolve(&ctx).await?;
            ctx.write(a.signer.clone(), a.into_spec(oracle_id)).await
        }
        ProxyOracleNs::Upgrade(a) => {
            let authorization = Authorization::try_from(&a.signer)?;
            let oracle_id = a.target.resolve(&ctx).await?;
            if a.preflight.runs(authorization.mode()) {
                upgrade_preflight::gate(&ctx, &oracle_id, &a.preflight.skip_check).await?;
            }
            ctx.write_authorized(authorization, a.try_into_spec(oracle_id)?)
                .await
        }
        ProxyOracleNs::Governance(a) => proxy_oracle_governance(ctx, a).await,
    }
}

/// The `oracle.*` updates, served by the oracle-updates dispatcher. Each arm layers
/// only the payload sources its method fetches from — see [`CliContext::oracle_write`].
async fn oracle(ctx: CliContext, ns: OracleNs) -> anyhow::Result<()> {
    let network = ctx.network();
    match ns {
        OracleNs::Pyth(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| {
                Ok(pyth_source(base, &sources, network))
            })
            .await
        }
        OracleNs::RedStone(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| {
                redstone_source(base, &sources)
            })
            .await
        }
        OracleNs::Lazer(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| lazer_source(base, &sources))
                .await
        }
        OracleNs::Prices(a) => {
            let (signer, sources) = (a.signer.clone(), a.sources.clone());
            ctx.oracle_write(signer, a.into_spec(), |base| {
                all_sources(base, &sources, network)
            })
            .await
        }
    }
}

async fn pyth(ctx: CliContext, ns: PythNs) -> anyhow::Result<()> {
    match ns {
        PythNs::ListEmaPricesNoOlderThan(a) => ctx.read(a.into_spec()).await,
        PythNs::ListEmaPricesUnsafe(a) => ctx.read(a.into_spec()).await,
        PythNs::UpdatePriceFeeds(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}

async fn owner(ctx: CliContext, ns: OwnerNs) -> anyhow::Result<()> {
    match ns {
        OwnerNs::Get(a) => ctx.read(a.into_spec()).await,
        OwnerNs::GetProposed(a) => ctx.read(a.into_spec()).await,
        OwnerNs::Propose(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        OwnerNs::Accept(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        OwnerNs::Renounce(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
    }
}

async fn proxy_oracle_governance(
    ctx: CliContext,
    ns: ProxyOracleGovernanceNs,
) -> anyhow::Result<()> {
    use templar_gateway_methods_spec::proxy_oracle_governance as gov;

    match ns {
        ProxyOracleGovernanceNs::Create(a) => {
            ctx.write_with(a.signer.clone(), |auth| a.try_into_spec(auth))
                .await
        }
        ProxyOracleGovernanceNs::CreateProposal(a) => proposals::create(ctx, a).await,
        ProxyOracleGovernanceNs::CancelProposal(a) => {
            let governance_id = a.proposal.target.resolve(&ctx).await?;
            ctx.write(a.signer.clone(), a.cancel(governance_id)).await
        }
        ProxyOracleGovernanceNs::ExecuteProposal(a) => proposals::execute(ctx, a).await,
        ProxyOracleGovernanceNs::GetProposal(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(a.get(governance_id)).await
        }
        ProxyOracleGovernanceNs::ListProposals(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(governance_id)).await
        }
        ProxyOracleGovernanceNs::NextProposalId(a) => {
            let governance_id = a.resolve(&ctx).await?;
            ctx.read(gov::NextProposalId { governance_id }).await
        }
        ProxyOracleGovernanceNs::ProposalCount(a) => {
            let governance_id = a.resolve(&ctx).await?;
            ctx.read(gov::ProposalCount { governance_id }).await
        }
        ProxyOracleGovernanceNs::GetGovernancePolicy(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(gov::GetGovernancePolicy { governance_id }).await
        }
        ProxyOracleGovernanceNs::GetProxyOracleId(a) => {
            let governance_id = a.resolve(&ctx).await?;
            ctx.read(gov::GetProxyOracleId { governance_id }).await
        }
        ProxyOracleGovernanceNs::HasRole(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(governance_id)).await
        }
        ProxyOracleGovernanceNs::ListRole(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(governance_id)).await
        }
        ProxyOracleGovernanceNs::GetRoles(a) => {
            let governance_id = a.target.resolve(&ctx).await?;
            ctx.read(a.into_spec(governance_id)).await
        }
    }
}

async fn redstone(ctx: CliContext, ns: RedstoneNs) -> anyhow::Result<()> {
    match ns {
        RedstoneNs::Create(a) => {
            ctx.write_with(a.signer.clone(), |auth| a.try_into_spec(auth))
                .await
        }
        RedstoneNs::GetConfig(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ReadPriceData(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::ListRole(a) => ctx.read(a.into_spec()).await,
        RedstoneNs::SetRole(a) => ctx.write(a.signer.clone(), a.into_spec()).await,
        RedstoneNs::WritePrices(a) => ctx.write(a.signer.clone(), a.try_into_spec()?).await,
    }
}
