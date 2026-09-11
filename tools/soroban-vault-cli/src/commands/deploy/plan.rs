//! Read-only deployment planning and Stellar command-shape projection.

use std::collections::BTreeSet;

use anyhow::Context;
use templar_soroban_shared_types::RUNTIME_FEATURE_COMPANION_UPGRADE;

use crate::{
    artifacts::{
        inspect_existing_release_artifact, ArtifactSpec, ExistingReleaseArtifact, RELEASE_TAG,
    },
    cli::{Cli, DeployPlanCommand},
    manifest::Manifest,
    stellar::CommandExecutor,
    types::{AdapterAdminArg, AddressStr},
};

use super::{
    super::{
        context::CommandContext,
        inventory::{
            blend_adapter_by_pool, blend_adapter_key, contract_id, custodial_adapter_by_custodian,
            custodial_adapter_key, next_blend_adapter_index, next_custodial_adapter_index,
        },
        output::{PlanContract, PlanResponse, PlanWasm, Response},
    },
    adapters::{validate_adapter_deployment_request, validated_stack_adapter_admin},
    stack::resolve_governance_timelock_ns,
};

pub(in crate::commands) fn run_deploy_plan<E: CommandExecutor>(
    context: &CommandContext<'_, E>,
    manifest: &Manifest,
    args: &crate::cli::DeployPlanArgs,
) -> anyhow::Result<Response> {
    let cli = context.cli();
    let plan = match &args.command {
        DeployPlanCommand::Stack(stack) => deploy_stack_plan(cli, manifest, stack)?,
        DeployPlanCommand::Adapters(adapters) => deploy_adapters_plan(cli, manifest, adapters)?,
    };
    Ok(Response::Plan(plan))
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps the full stack plan ordered to match deployment execution"
)]
pub(in crate::commands) fn deploy_stack_plan(
    cli: &Cli,
    manifest: &Manifest,
    args: &crate::cli::DeployStackArgs,
) -> anyhow::Result<PlanResponse> {
    let mut plan = PlanResponse::new("deploy stack", &cli.network);
    if args.governance_timelock_ns == Some(0) && !cli.allow_zero_timelock {
        plan.warnings.push(
            "zero governance timelock would be blocked without --allow-zero-timelock".to_string(),
        );
    }
    let _ = resolve_governance_timelock_ns(manifest, args)?;
    plan.required_signers.push(
        args.admin
            .as_ref()
            .map_or_else(default_source_label, |admin| admin.to_string()),
    );

    let include_blend = !args.blend_pools.is_empty();
    let include_custodial = !args.custodians.is_empty();
    let requested_adapter_admin = validated_stack_adapter_admin(manifest, args)?;
    let adapter_admin = if include_blend || include_custodial {
        let vault = (!args.force_new)
            .then(|| contract_id(manifest, "vault"))
            .flatten();
        Some(plan_adapter_admin(
            &mut plan,
            requested_adapter_admin.context("adapter deployment requires --adapter-admin")?,
            vault,
        ))
    } else {
        None
    };
    for spec in ArtifactSpec::stack_artifacts(include_blend, include_custodial) {
        plan.wasm.push(wasm_plan(cli, manifest, spec, args.build)?);
    }

    for key in [
        "vault",
        "share_token",
        "governance",
        "proxy_4626",
        "curator_proxy",
    ] {
        push_contract_plan(&mut plan, manifest, key, args.force_new);
    }
    ensure_provided_contract_matches_manifest(manifest, "asset_token", args.asset_token.as_ref())?;
    if let Some(asset_token) = &args.asset_token {
        if let Some(existing) = contract_id(manifest, "asset_token") {
            plan.contracts_to_reuse.push(PlanContract {
                key: "asset_token".to_string(),
                contract_id: Some(existing.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
        } else {
            plan.manifest_mutations.push(format!(
                "record provided asset_token contract {asset_token}"
            ));
        }
    } else if let Some(asset_token) = contract_id(manifest, "asset_token") {
        plan.contracts_to_reuse.push(PlanContract {
            key: "asset_token".to_string(),
            contract_id: Some(asset_token.to_string()),
            reason: "already recorded in manifest".to_string(),
        });
    } else {
        plan.manifest_mutations
            .push("record native asset token contract id".to_string());
        plan.stellar_commands.push(stellar_command_shape(
            "contract asset deploy --asset native",
            true,
        ));
    }
    let mut next_blend_index = next_blend_adapter_index(manifest);
    for pool in &args.blend_pools {
        if !args.force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("blend adapter for pool {pool}"),
                contract_id: blend_adapter_by_pool(manifest, pool).map(ToString::to_string),
                reason: "adapter for pool is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: blend_adapter_key(next_blend_index),
                contract_id: None,
                reason: format!("new adapter for pool {pool}"),
            });
            next_blend_index += 1;
            plan.manifest_mutations
                .push(format!("record new Blend adapter for pool {pool}"));
            plan.stellar_commands.push(stellar_command_shape(
                &format!(
                    "contract deploy --wasm-hash <blend_adapter_hash> -- --admin {} --vault <vault> --pool <pool>",
                    adapter_admin
                        .as_deref()
                        .context("adapter deployment requires --adapter-admin <address|vault>")?
                ),
                true,
            ));
        }
    }
    plan_custodial_adapters(
        &mut plan,
        manifest,
        &args.custodians,
        adapter_admin.as_deref(),
        args.force_new,
    )?;
    plan.manifest_mutations
        .push("mark initialized contracts after successful initialize calls".to_string());
    Ok(plan)
}

pub(in crate::commands) fn deploy_adapters_plan(
    cli: &Cli,
    manifest: &Manifest,
    args: &crate::cli::DeployAdaptersArgs,
) -> anyhow::Result<PlanResponse> {
    let mut plan = PlanResponse::new("deploy adapters", &cli.network);
    anyhow::ensure!(
        !args.blend_pools.is_empty() || !args.custodians.is_empty(),
        "deploy adapters requires at least one --blend-pool or --custodian"
    );
    validate_adapter_deployment_request(manifest, args)?;
    let vault =
        contract_id(manifest, "vault").or_else(|| args.vault.as_ref().map(AddressStr::as_str));
    let adapter_admin = plan_adapter_admin(&mut plan, &args.adapter_admin, vault);
    plan.required_signers.push(default_source_label());
    if !args.blend_pools.is_empty() {
        plan.wasm.push(wasm_plan(
            cli,
            manifest,
            ArtifactSpec::from_name(crate::cli::ArtifactName::BlendAdapter),
            args.build,
        )?);
    }
    if !args.custodians.is_empty() {
        plan.wasm.push(wasm_plan(
            cli,
            manifest,
            ArtifactSpec::from_name(crate::cli::ArtifactName::CustodialAdapter),
            args.build,
        )?);
    }

    for (key, provided) in [
        ("vault", args.vault.as_ref()),
        ("governance", args.governance.as_ref()),
        ("asset_token", args.asset_token.as_ref()),
    ] {
        ensure_provided_contract_matches_manifest(manifest, key, provided)?;
        if let Some(existing) = contract_id(manifest, key) {
            plan.contracts_to_reuse.push(PlanContract {
                key: key.to_string(),
                contract_id: Some(existing.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
        } else if let Some(provided) = provided {
            plan.manifest_mutations
                .push(format!("record imported {key} contract {provided}"));
        } else if key != "asset_token" {
            plan.warnings.push(format!(
                "{key} is missing from manifest and must be passed for deploy adapters"
            ));
        } else if !args.custodians.is_empty() {
            plan.warnings.push(
                "asset_token is missing from manifest and must be passed for custodial adapters"
                    .to_string(),
            );
        }
    }

    let mut next_blend_index = next_blend_adapter_index(manifest);
    for pool in &args.blend_pools {
        if !args.force_new && blend_adapter_by_pool(manifest, pool).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("blend adapter for pool {pool}"),
                contract_id: blend_adapter_by_pool(manifest, pool).map(ToString::to_string),
                reason: "adapter for pool is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: blend_adapter_key(next_blend_index),
                contract_id: None,
                reason: format!("new adapter for pool {pool}"),
            });
            next_blend_index += 1;
            plan.manifest_mutations
                .push(format!("record new Blend adapter for pool {pool}"));
            plan.stellar_commands.push(stellar_command_shape(
                &format!(
                    "contract deploy --wasm-hash <blend_adapter_hash> -- --admin {adapter_admin} --vault <vault> --pool <pool>"
                ),
                true,
            ));
        }
    }
    plan_custodial_adapters(
        &mut plan,
        manifest,
        &args.custodians,
        Some(&adapter_admin),
        args.force_new,
    )?;
    Ok(plan)
}

fn ensure_provided_contract_matches_manifest(
    manifest: &Manifest,
    key: &str,
    provided: Option<&AddressStr>,
) -> anyhow::Result<()> {
    if let (Some(existing), Some(provided)) = (contract_id(manifest, key), provided) {
        anyhow::ensure!(
            existing == provided.as_str(),
            "{key} already recorded as {existing}; refusing to overwrite with {provided}"
        );
    }
    Ok(())
}

pub(in crate::commands) fn plan_adapter_admin(
    plan: &mut PlanResponse,
    admin: &AdapterAdminArg,
    vault: Option<&str>,
) -> String {
    let resolved = admin.resolve(vault.unwrap_or("<vault>"));
    if admin.targets_vault(vault.unwrap_or("<vault>")) {
        plan.stellar_commands.push(stellar_command_shape(
            &format!("contract invoke --id {resolved} --send no -- version"),
            false,
        ));
        plan.warnings.push(format!(
            "using the vault as adapter admin requires version() to advertise companion-upgrade capability {RUNTIME_FEATURE_COMPANION_UPGRADE:#x}"
        ));
    }
    resolved.to_string()
}

pub(in crate::commands) fn plan_custodial_adapters(
    plan: &mut PlanResponse,
    manifest: &Manifest,
    custodians: &[AddressStr],
    adapter_admin: Option<&str>,
    force_new: bool,
) -> anyhow::Result<()> {
    let mut next_custodial_index = next_custodial_adapter_index(manifest);
    let mut planned_custodians = BTreeSet::new();
    for custodian in custodians {
        if !planned_custodians.insert(custodian.as_str().to_string()) {
            continue;
        }
        if !force_new && custodial_adapter_by_custodian(manifest, custodian).is_some() {
            plan.contracts_to_reuse.push(PlanContract {
                key: format!("custodial adapter for custodian {custodian}"),
                contract_id: custodial_adapter_by_custodian(manifest, custodian)
                    .map(ToString::to_string),
                reason: "adapter for custodian is already recorded in manifest".to_string(),
            });
        } else {
            plan.contracts_to_deploy.push(PlanContract {
                key: custodial_adapter_key(next_custodial_index),
                contract_id: None,
                reason: format!("new adapter for custodian {custodian}"),
            });
            next_custodial_index += 1;
            plan.manifest_mutations.push(format!(
                "record new custodial adapter for custodian {custodian}"
            ));
            plan.stellar_commands.push(stellar_command_shape(
                &format!(
                    "contract deploy --wasm-hash <custodial_adapter_hash> -- --admin {} --vault <vault> --custodian <custodian> --asset <asset_token>",
                    adapter_admin
                        .context("adapter deployment requires --adapter-admin <address|vault>")?
                ),
                true,
            ));
        }
    }
    Ok(())
}

pub(in crate::commands) fn push_contract_plan(
    plan: &mut PlanResponse,
    manifest: &Manifest,
    key: &str,
    force_new: bool,
) {
    if !force_new {
        if let Some(contract_id) = contract_id(manifest, key) {
            plan.contracts_to_reuse.push(PlanContract {
                key: key.to_string(),
                contract_id: Some(contract_id.to_string()),
                reason: "already recorded in manifest".to_string(),
            });
            return;
        }
    }
    plan.contracts_to_deploy.push(PlanContract {
        key: key.to_string(),
        contract_id: None,
        reason: if force_new {
            "--force-new requested".to_string()
        } else {
            "not recorded in manifest".to_string()
        },
    });
    plan.manifest_mutations
        .push(format!("record deployed {key} contract id"));
    let command = if key == "curator_proxy" {
        format!(
            "contract deploy --wasm-hash <{key}_hash> -- --initialization_authority <source-account-address>"
        )
    } else {
        format!("contract deploy --wasm-hash <{key}_hash>")
    };
    plan.stellar_commands
        .push(stellar_command_shape(&command, true));
}

pub(in crate::commands) fn wasm_plan(
    cli: &Cli,
    manifest: &Manifest,
    spec: ArtifactSpec,
    build: bool,
) -> anyhow::Result<PlanWasm> {
    let workspace_path = spec.wasm_path(&cli.workspace_path);
    // Mirrors resolver precedence without side effects. Only verified bytes at
    // the reported path produce a local hash; projected builds/downloads do not.
    let (artifact_path, local_hash, source_action, source_includes_network) = if build {
        (
            workspace_path,
            None,
            "build artifact from the checked-out workspace".to_string(),
            false,
        )
    } else {
        match inspect_existing_release_artifact(&cli.workspace_path, spec)? {
            ExistingReleaseArtifact::Cache { path, sha256 } => (
                path,
                Some(sha256),
                "use verified release cache bytes".to_string(),
                false,
            ),
            ExistingReleaseArtifact::WorkspaceSeed { path, sha256 } => (
                path,
                Some(sha256),
                "seed verified workspace release bytes into the cache; fetch the pinned hash and upload the cached copy if missing remotely".to_string(),
                true,
            ),
            ExistingReleaseArtifact::IgnoredWorkspace {
                cache_path,
                workspace_path,
                reason,
            } => (
                cache_path,
                None,
                format!(
                    "ignore unreleased workspace bytes at {} ({reason}); download pinned release {RELEASE_TAG} artifact",
                    workspace_path.display()
                ),
                false,
            ),
            ExistingReleaseArtifact::Missing { cache_path } => (
                cache_path,
                None,
                format!("download pinned release {RELEASE_TAG} artifact"),
                false,
            ),
        }
    };
    let recorded_remote_hash = manifest
        .artifacts
        .get(spec.key)
        .and_then(|record| record.remote_wasm_hash.clone());
    let network_action = match (&local_hash, &recorded_remote_hash) {
        (Some(local), Some(remote)) if local == remote => {
            "reuse recorded remote hash after fetch verification"
        }
        _ if source_includes_network => "",
        (Some(_), _) => "fetch local hash and upload if missing remotely",
        (None, _) if build => "then fetch/upload the resulting hash",
        (None, _) => "then upload if missing remotely",
    };
    let action = if network_action.is_empty() {
        source_action
    } else {
        format!("{source_action}; {network_action}")
    };
    Ok(PlanWasm {
        key: spec.key.to_string(),
        package: spec.package.to_string(),
        path: artifact_path.display().to_string(),
        local_hash,
        recorded_remote_hash,
        action,
    })
}

pub(in crate::commands) fn stellar_command_shape(command: &str, uses_source: bool) -> String {
    if uses_source {
        format!("STELLAR_ACCOUNT=<redacted-if-overridden> stellar {command}")
    } else {
        format!("stellar {command}")
    }
}

pub(in crate::commands) fn default_source_label() -> String {
    "Stellar default identity/keystore or STELLAR_ACCOUNT".to_string()
}
