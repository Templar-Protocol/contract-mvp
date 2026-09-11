//! Deployment, planning, reconciliation, and recovery workflows.

mod adapters;
mod curator_proxy;
mod plan;
mod reconcile;
mod session;
mod stack;

use crate::{
    artifacts::{ensure_uploaded, ArtifactSpec},
    cli::{DeployArgs, DeployCommand},
    manifest::Manifest,
    stellar::CommandExecutor,
};

use super::{
    context::CommandContext,
    output::{PlanContract, PlanResponse, Response},
};

use adapters::deploy_adapters;
use curator_proxy::deploy_curator_proxy;
use plan::run_deploy_plan;
pub(in crate::commands) use plan::stellar_command_shape;
pub(in crate::commands) use reconcile::run_reconcile;
use session::DeploymentContext;
use stack::{deploy_resume, deploy_stack};

#[cfg(test)]
pub(in crate::commands) use adapters::{parse_runtime_version, validate_adapter_admin};
#[cfg(test)]
pub(in crate::commands) use curator_proxy::record_standard_curator_proxy_initialization_if_missing;
#[cfg(test)]
pub(in crate::commands) use plan::{deploy_adapters_plan, wasm_plan};
#[cfg(test)]
pub(in crate::commands) use reconcile::{
    apply_reconcile_safe_manifest_updates, curator_proxy_needs_version_verification,
    curator_proxy_supports_version_discovery, reconcile_manifest, verify_component_wiring,
};
#[cfg(test)]
pub(in crate::commands) use session::{
    deploy_contract_if_needed, initialize_proxy_if_needed, initialize_vault_if_needed,
    ContractDeployment, InitializationState,
};

pub(in crate::commands) fn run<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &mut Manifest,
    args: &DeployArgs,
) -> anyhow::Result<Response> {
    if context.cli().dry_run {
        return run_dry_plan(context, manifest, args);
    }
    match &args.command {
        DeployCommand::Plan(plan) => run_deploy_plan(context, manifest, plan),
        DeployCommand::Repair(repair) => Ok(run_reconcile(context, manifest, repair)),
        DeployCommand::Stack(stack) => {
            let mut deployment = DeploymentContext::new(context, manifest);
            deploy_stack(&mut deployment, stack)
        }
        DeployCommand::Resume(stack) => {
            let mut deployment = DeploymentContext::new(context, manifest);
            deploy_resume(&mut deployment, stack)
        }
        DeployCommand::Adapters(adapters) => {
            let mut deployment = DeploymentContext::new(context, manifest);
            deploy_adapters(&mut deployment, adapters)
        }
        DeployCommand::CuratorProxy(args) => {
            let mut deployment = DeploymentContext::new(context, manifest);
            deploy_curator_proxy(&mut deployment, args)
        }
        DeployCommand::Wasm(wasm) => {
            let mut deployment = DeploymentContext::new(context, manifest);
            let (context, manifest) = deployment.parts();
            let spec = ArtifactSpec::from_name(wasm.artifact);
            let hash = ensure_uploaded(
                context.stellar(),
                manifest,
                &context.cli().workspace_path,
                spec,
                wasm.build,
            )?;
            context.checkpoint(manifest)?;
            Ok(Response::message(format!("{} wasm hash: {hash}", spec.key)))
        }
    }
}

fn run_dry_plan<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    args: &DeployArgs,
) -> anyhow::Result<Response> {
    match &args.command {
        DeployCommand::Plan(plan) => run_deploy_plan(context, manifest, plan),
        DeployCommand::Repair(repair) => {
            let mut response = PlanResponse::new("repair deployment state", &context.cli().network);
            response.warnings.push(format!(
                "would reconcile manifest state{}; dry-run performs no Stellar queries or manifest writes",
                if repair.skip_view_verification {
                    " without initialized-wiring view verification"
                } else {
                    " including initialized-wiring view verification"
                }
            ));
            response.manifest_mutations.push(
                "mark only chain-verified initialized contracts during a real repair".to_string(),
            );
            Ok(Response::Plan(response))
        }
        DeployCommand::Stack(stack) | DeployCommand::Resume(stack) => Ok(Response::Plan(
            plan::deploy_stack_plan(context.cli(), manifest, stack)?,
        )),
        DeployCommand::Adapters(adapters) => Ok(Response::Plan(plan::deploy_adapters_plan(
            context.cli(),
            manifest,
            adapters,
        )?)),
        DeployCommand::Wasm(wasm) => {
            let spec = ArtifactSpec::from_name(wasm.artifact);
            let mut response = PlanResponse::new("deploy wasm", &context.cli().network);
            response
                .wasm
                .push(plan::wasm_plan(context.cli(), manifest, spec, wasm.build)?);
            Ok(Response::Plan(response))
        }
        DeployCommand::CuratorProxy(args) => {
            let spec = ArtifactSpec::from_name(crate::cli::ArtifactName::CuratorProxy);
            let mut response = PlanResponse::new("deploy curator-proxy", &context.cli().network);
            response
                .wasm
                .push(plan::wasm_plan(context.cli(), manifest, spec, args.build)?);
            response.contracts_to_deploy.push(PlanContract {
                key: "curator_proxy".to_string(),
                contract_id: None,
                reason: "dry-run plans a fresh curator proxy".to_string(),
            });
            response.stellar_commands.push(plan::stellar_command_shape(
                "contract deploy --wasm-hash <curator_proxy_hash> -- --initialization_authority <source-account-address>",
                true,
            ));
            Ok(Response::Plan(response))
        }
    }
}
