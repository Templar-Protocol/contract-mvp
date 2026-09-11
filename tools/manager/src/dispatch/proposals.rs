//! Governance proposal planning and orchestration: resolving proposal inputs,
//! rendering plans, creating proposals, and optionally waiting for maturity
//! before execution.

use anyhow::Context as _;
use near_account_id::AccountId;
use serde::Serialize;
use templar_common::oracle::pyth::PriceIdentifier;
use templar_gateway_client::Client;
use templar_gateway_methods_spec::proxy_oracle_governance as gov;
use templar_gateway_types::{common::WriteOperationResult, ManagedAccountId};

use crate::commands::proxy_oracle::{CreateProposal, ExecuteProposalArgs};
use crate::commands::signer::{Authorization, Mode};
use crate::context::{print_json, CliContext};

/// Plan or create a governance proposal. Resolves the proposal id (fetching the
/// governance contract's next id when `--id` was omitted) and, for an
/// `oracle add-circuit-breaker` proposal without `--breaker-id`, the set's next breaker
/// id. Print mode emits the selected plan representation without sending.
/// Execution mode logs the resolved id, and `--execute-when-ready` waits for the
/// proposal's TTL to elapse before executing it.
pub(super) async fn create(ctx: CliContext, mut args: CreateProposal) -> anyhow::Result<()> {
    let execute_when_ready = args.execute_when_ready();
    let authorization = Authorization::try_from(&args.signer)?;
    let preflight = args.preflight.clone();
    let requires_upgrade_preflight = args.requires_upgrade_preflight();
    let governance_id = args.target.resolve(&ctx).await?;

    // An upgrade proposal is gated before it is even queued, so a deployment that cannot survive
    // the new code is caught while the fix is still cheap.
    let preflight_runs = requires_upgrade_preflight && preflight.runs(authorization.mode());
    if preflight_runs {
        super::upgrade_preflight::gate_governed_oracle(&ctx, &governance_id, &preflight).await?;
    }

    // Auto-fill the next breaker id for oracle add-circuit-breaker, resolving the proxy
    // oracle (whose set holds the breakers) through the governance contract. This
    // reads the committed set's next id; if a concurrent proposal advances it
    // before this one executes, the contract rejects the stale id (no corruption)
    // and the proposal can simply be retried.
    if let Some(price_id) = args.unresolved_breaker_price_id() {
        let next_id = next_breaker_id(&ctx, &governance_id, price_id).await?;
        tracing::info!(breaker_id = next_id, "auto-fetched next breaker id");
        args.set_breaker_id(next_id);
    }

    let id = match args.id() {
        Some(id) => id,
        None => {
            ctx.client
                .read(gov::NextProposalId {
                    governance_id: governance_id.clone(),
                })
                .await?
        }
    };

    let create_spec = args.try_into_spec(governance_id.clone(), id)?;
    if let Mode::Plan(_) = authorization.mode() {
        return ctx.write_authorized(authorization, create_spec).await;
    }

    let (signer, client, _) = ctx.signing_client_and_key(authorization).await?;
    let create = client.execute_as(signer.clone(), create_spec).await?;
    // Fail fast if the create reverted, before waiting on / executing a proposal
    // that was never created.
    ctx.report_checked(&create)?;
    // Emit the id now so it survives a later wait/execute failure below.
    tracing::info!(proposal_id = id, "created proposal");

    let execute = if execute_when_ready {
        wait_for_maturity(&ctx, &governance_id, id).await?;
        // Re-checked after the wait: the timelock is exactly the window in which stored state can
        // drift away from what the queued code expects.
        if preflight_runs {
            super::upgrade_preflight::gate_governed_oracle(&ctx, &governance_id, &preflight)
                .await?;
        }
        let result = execute_now(&ctx, &client, &signer, &governance_id, id).await?;
        Some(result)
    } else {
        None
    };

    print_json(&CreateProposalOutput {
        id,
        create,
        execute,
    })
}

/// Plan or execute a governance proposal. In execution mode, `--when-ready`
/// waits for its TTL to elapse, so an early call blocks instead of failing on
/// an immature proposal.
pub(super) async fn execute(ctx: CliContext, args: ExecuteProposalArgs) -> anyhow::Result<()> {
    let authorization = Authorization::try_from(&args.signer)?;
    let governance_id = args.target.resolve(&ctx).await?;
    if args.when_ready() {
        wait_for_maturity(&ctx, &governance_id, args.id()).await?;
    }
    if args.preflight.runs(authorization.mode()) {
        super::upgrade_preflight::gate_queued_upgrade(
            &ctx,
            &governance_id,
            args.id(),
            &args.preflight,
        )
        .await?;
    }
    ctx.write_authorized(authorization, args.into_spec(governance_id))
        .await
}

/// Execute proposal `id` on its own (no idempotency key), signed as `signer`
/// through `client`, reporting the tx link.
async fn execute_now(
    ctx: &CliContext,
    client: &Client,
    signer: &ManagedAccountId,
    governance_id: &AccountId,
    id: u32,
) -> anyhow::Result<WriteOperationResult> {
    let result = client
        .execute_as(
            signer.clone(),
            gov::ExecuteProposal {
                governance_id: governance_id.clone(),
                id,
            },
        )
        .await?;
    ctx.report_checked(&result)?;
    Ok(result)
}

/// The next breaker id for `price_id` on the proxy oracle administered by
/// `governance_id`: resolve the oracle, read its circuit breaker set, and take
/// the set's next id (0 when no set exists yet).
async fn next_breaker_id(
    ctx: &CliContext,
    governance_id: &AccountId,
    price_id: PriceIdentifier,
) -> anyhow::Result<u32> {
    use templar_gateway_methods_spec::proxy_oracle;

    let oracle_id = ctx
        .client
        .read(gov::GetProxyOracleId {
            governance_id: governance_id.clone(),
        })
        .await?
        .proxy_oracle_id;
    let set = ctx
        .client
        .read(proxy_oracle::GetProxyCircuitBreakerSet {
            oracle_id,
            id: price_id,
        })
        .await?
        .circuit_breaker_set;
    Ok(set.map_or(0, |set| set.next_id()))
}

/// Machine-readable result of an execution-mode `create-proposal` run. `id` is
/// always present (resolved even when auto-fetched); `execute` is present only
/// when the proposal was executed via `--execute-when-ready`.
#[derive(Serialize)]
struct CreateProposalOutput {
    id: u32,
    create: WriteOperationResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    execute: Option<WriteOperationResult>,
}

/// Block until proposal `id` is executable (`now - created_at >= ttl`), reading
/// its effective TTL back from the governance contract.
async fn wait_for_maturity(
    ctx: &CliContext,
    governance_id: &AccountId,
    id: u32,
) -> anyhow::Result<()> {
    use std::time::Duration;

    let proposal = ctx
        .client
        .read(gov::GetProposal {
            governance_id: governance_id.clone(),
            id,
        })
        .await?
        .proposal
        .context("created proposal not found when waiting for maturity")?;

    let maturity_ns = proposal
        .created_at
        .as_ns()
        .saturating_add(proposal.ttl.as_ns());
    let now_ns = crate::spec::wall_clock().as_ns();

    if maturity_ns > now_ns {
        // Small buffer so block time (the chain's authoritative clock) has caught
        // up to the local wall clock before we submit the execute.
        let wait = Duration::from_nanos(maturity_ns - now_ns) + Duration::from_secs(2);
        eprintln!(
            "Waiting {}s for proposal {id} to mature before executing...",
            wait.as_secs()
        );
        tokio::time::sleep(wait).await;
    }

    Ok(())
}
