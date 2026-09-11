//! CLI gateway context and the shared helpers for reading, planning, executing,
//! reporting, and printing. Execution credentials and plan output selection
//! remain on each write command rather than in this context.

use anyhow::Context as _;
use near_api::{types::transaction::actions::Action, NetworkConfig, SecretKey};
use near_sdk::json_types::Base64VecU8;
use serde::Serialize;
use std::io::Write as _;
use templar_gateway_client::{Client, Network, NetworkConfigBuilder};
use templar_gateway_core::{
    DispatchRead, GatewayContext, GatewayContextBuilder, OperationPlan, PlanWrite,
    PlannedTransaction,
};
use templar_gateway_methods_dispatch::Dispatch;
use templar_gateway_oracle_updates_dispatch::{
    build_oracle_updates_context, Dispatch as OracleUpdatesDispatch,
    GatewayContextBuilderOracleExt as _, LazerSourceArgs, OracleSourceArgs, OracleUpdatesContext,
    PythSourceArgs, RedStoneSourceArgs, WithLazerSource, WithPythSource, WithRedStoneSource,
};
use templar_gateway_types::{
    common::{WriteOperationResult, WriteRequest},
    operation::ReceiptStatus,
    ManagedAccountId, MethodSpec, OperationStatus,
};

use crate::cli::Cli;
use crate::commands::signer::{Authorization, Mode, PrintFormat, SignerArgs};

pub(crate) struct CliContext {
    /// An unsigned client for reads and ordinary write plans. Executed writes
    /// build a per-operation signing client from their command's credentials.
    pub(crate) client: Client,
    network: NetworkConfig,
    /// The chain the CLI was pointed at. Kept alongside the built config so a
    /// command can compare it against what a spec declares — reading a mainnet
    /// spec against testnet reports every account as missing.
    selected_network: Network,
    transaction_url_prefix: String,
    /// Whether `-q` silenced the human report. Diagnostics-only: it never
    /// affects the JSON on stdout.
    quiet: bool,
}

impl CliContext {
    /// The chain the CLI was pointed at.
    pub(crate) const fn network(&self) -> Network {
        self.selected_network
    }
    pub(crate) fn network_config(&self) -> &NetworkConfig {
        &self.network
    }
    pub(crate) fn final_client(&self) -> anyhow::Result<Client> {
        Client::builder(self.network.clone())
            .finality_policy(templar_gateway_core::FinalityPolicy::Final)
            .build()
            .context("build final-state gateway client")
    }

    /// A reporter for this run's human output, carrying the ids to suppress.
    pub(crate) fn reporter(&self, skip: &[String]) -> crate::report::Reporter {
        crate::report::Reporter::new(skip, self.quiet)
    }

    /// Build a single-signer client for `account_id` from a bare `secret_key`.
    ///
    /// For the teardown flows that sign many *discovered* accounts with one
    /// authorized key (`registry clear-deployments`), where there is no single
    /// `SignerArgs` to resolve. Writes that carry their own credentials use
    /// [`Self::signing_client_for`] instead.
    pub(crate) fn signing_client(
        &self,
        account_id: impl Into<ManagedAccountId>,
        secret_key: SecretKey,
    ) -> anyhow::Result<Client> {
        Client::builder(self.network.clone())
            .secret_key(account_id, secret_key)?
            .build()
            .context("build signing client")
    }

    /// Resolve a write command's credentials through its selected backend and
    /// build a single-signer client from them.
    ///
    /// The signer stays behind `Arc<Signer>` rather than being unwrapped to a
    /// secret key, so a backend that never surrenders one still works here.
    pub(crate) async fn signing_client_for(
        &self,
        signer: &SignerArgs,
    ) -> anyhow::Result<(ManagedAccountId, Client)> {
        let (account_id, client, _) = self
            .signing_client_and_key(Authorization::try_from(signer)?)
            .await?;
        Ok((account_id, client))
    }

    /// A signing client and the key it will sign with, from one resolution:
    /// resolving twice can prompt twice and yield two different keys.
    pub(crate) async fn signing_client_and_key(
        &self,
        authorization: Authorization,
    ) -> anyhow::Result<(ManagedAccountId, Client, near_api::PublicKey)> {
        let (signing, public_key) = authorization.resolve(&self.network).await?;
        let account_id = signing.account_id().clone();
        let client = Client::builder(self.network.clone())
            .with_signer(signing)
            .build()
            .context("build signing client")?;
        Ok((account_id, client, public_key))
    }

    /// Dispatch a read and print its JSON result.
    pub(crate) async fn read<S>(&self, request: S) -> anyhow::Result<()>
    where
        S: MethodSpec,
        Dispatch: DispatchRead<S, GatewayContext>,
    {
        let output = self.client.read(request).await?;
        print_json(&output)
    }

    /// Plan and print a write when `signer` selects a format; otherwise execute
    /// it, report the tx link, print the result, and require on-chain success.
    pub(crate) async fn write<S>(&self, signer: SignerArgs, body: S) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        self.write_with(signer, |_| Ok(body)).await
    }

    /// [`Self::write`] for a spec that embeds the signer's public key: the
    /// authorization is resolved before the spec is built, then signs it.
    pub(crate) async fn write_with<S>(
        &self,
        signer: SignerArgs,
        into_spec: impl FnOnce(&Authorization) -> anyhow::Result<S>,
    ) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        let authorization = Authorization::try_from(&signer)?;
        let body = into_spec(&authorization)?;
        self.write_authorized(authorization, body).await
    }

    /// [`Self::write`] for a caller that already resolved the authorization.
    pub(crate) async fn write_authorized<S>(
        &self,
        authorization: Authorization,
        body: S,
    ) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Dispatch: PlanWrite<S, GatewayContext>,
    {
        if let Mode::Plan(format) = *authorization.mode() {
            let plan = self
                .client
                .plan_request(WriteRequest {
                    signer_account_id: authorization.account_id().clone(),
                    idempotency_key: None,
                    body,
                })
                .await?;
            return print_plan(format, plan);
        }

        let (account_id, client, _) = self.signing_client_and_key(authorization).await?;
        let output = client.execute_as(account_id, body).await?;
        self.finish_write(&output)
    }

    /// Plan or execute an `oracle.*` write through [`OracleUpdatesDispatch`],
    /// whose context carries the in-process payload sources the method fetches.
    pub(crate) async fn oracle_write<S, Ctx>(
        &self,
        signer: SignerArgs,
        body: S,
        layer_sources: impl FnOnce(GatewayContext) -> anyhow::Result<Ctx>,
    ) -> anyhow::Result<()>
    where
        S: MethodSpec<Output = WriteOperationResult>,
        Ctx: Clone,
        OracleUpdatesDispatch: PlanWrite<S, Ctx>,
    {
        let authorization = Authorization::try_from(&signer)?;
        if let Mode::Plan(format) = *authorization.mode() {
            let context = layer_sources(GatewayContext::new(self.network.clone())?)?;
            let plan = <OracleUpdatesDispatch as PlanWrite<S, Ctx>>::plan(
                WriteRequest {
                    signer_account_id: authorization.account_id().clone(),
                    idempotency_key: None,
                    body,
                },
                context,
            )
            .await?;
            return print_plan(format, plan);
        }

        let (signing, _) = authorization.resolve(&self.network).await?;
        let account_id = signing.account_id().clone();
        let (base_context, driver, signer_account_ids) = Client::builder(self.network.clone())
            .with_signer(signing)
            .build_parts()
            .context("build oracle-updates client")?;
        let client: Client<OracleUpdatesDispatch, Ctx> =
            Client::from_parts(layer_sources(base_context)?, driver, signer_account_ids);
        let output = client.execute_as(account_id, body).await?;
        self.finish_write(&output)
    }

    /// Report the tx link, print the machine-readable result, then fail unless the
    /// operation succeeded on chain. Printing precedes the status check so a reverted
    /// operation still emits its JSON on stdout.
    pub(crate) fn finish_write(&self, output: &WriteOperationResult) -> anyhow::Result<()> {
        self.report_tx(output);
        print_json(output)?;
        check_operation_status(output)
    }

    /// Log the explorer link for a completed write to stderr (the JSON result,
    /// carrying every step's hash, still goes to stdout).
    pub(crate) fn report_tx(&self, result: &WriteOperationResult) {
        if let Some(tx_hash) = result.operation.latest_tx_hash() {
            tracing::info!("tx: {}{}", self.transaction_url_prefix, tx_hash);
        }
    }

    /// Report an intermediate write's tx link, then fail if it reverted — the
    /// multi-step counterpart to [`write`](CliContext::write) (which also prints
    /// the JSON), so a reverted step in a teardown/proposal flow aborts instead of
    /// being treated as success.
    pub(crate) fn report_checked(&self, result: &WriteOperationResult) -> anyhow::Result<()> {
        self.report_tx(result);
        check_operation_status(result)
    }
}

/// `layer_sources` for `oracle.updatePyth`.
pub(crate) fn pyth_source(
    base: GatewayContext,
    args: &PythSourceArgs,
    network: Network,
) -> WithPythSource<GatewayContext> {
    GatewayContextBuilder::new(base)
        .with_pyth_source(args.hermes_url(network.hermes_url()))
        .build()
}

/// `layer_sources` for `oracle.updateRedStone`.
pub(crate) fn redstone_source(
    base: GatewayContext,
    args: &RedStoneSourceArgs,
) -> anyhow::Result<WithRedStoneSource<GatewayContext>> {
    Ok(GatewayContextBuilder::new(base)
        .with_redstone_source(&args.redstone_node_path)?
        .build())
}

/// `layer_sources` for `oracle.updateLazer`.
pub(crate) fn lazer_source(
    base: GatewayContext,
    args: &LazerSourceArgs,
) -> anyhow::Result<WithLazerSource<GatewayContext>> {
    Ok(GatewayContextBuilder::new(base)
        .with_lazer_source(args.build()?)
        .build())
}

/// `layer_sources` for `oracle.updatePrices`, which resolves a proxy oracle's
/// dependencies at plan time and so may reach any of the three sources.
pub(crate) fn all_sources(
    base: GatewayContext,
    args: &OracleSourceArgs,
    network: Network,
) -> anyhow::Result<OracleUpdatesContext> {
    Ok(build_oracle_updates_context(
        base,
        args.build(network.hermes_url())?,
    )?)
}

/// Succeed only when a submitted write reached a `Succeeded` terminal status.
/// A `Failed` operation (RPC round-trip fine, but reverted on chain) errors with
/// a concise stderr diagnostic naming the failed receipt(s); a non-terminal
/// `Pending`/`InProgress` operation also errors, because the CLI's transient
/// in-memory store has no later resume, so an unconfirmed outcome must not read
/// as success. Either way the process exits non-zero — letting driver scripts
/// stop instead of continuing past a failed or unknown step. Callers print the
/// machine-readable JSON first.
pub(crate) fn check_operation_status(result: &WriteOperationResult) -> anyhow::Result<()> {
    let operation = &result.operation;
    match operation.status {
        OperationStatus::Succeeded => Ok(()),
        OperationStatus::Failed => {
            let failed_contracts: Vec<&str> = operation
                .final_outcome()
                .map(|outcome| {
                    outcome
                        .receipts
                        .iter()
                        .filter(|receipt| receipt.status == ReceiptStatus::Failed)
                        .map(|receipt| receipt.contract_id.as_str())
                        .collect()
                })
                .unwrap_or_default();

            if failed_contracts.is_empty() {
                anyhow::bail!("operation {} failed on chain", operation.id.0);
            }
            anyhow::bail!(
                "operation {} failed on chain; reverted receipt(s) on: {}",
                operation.id.0,
                failed_contracts.join(", ")
            )
        }
        status @ (OperationStatus::Pending | OperationStatus::InProgress) => anyhow::bail!(
            "operation {} did not reach a terminal state (status: {status:?}); \
             its on-chain outcome is unknown",
            operation.id.0,
        ),
    }
}

fn print_plan(format: PrintFormat, plan: OperationPlan) -> anyhow::Result<()> {
    let transaction = single_transaction(plan)?;
    match format {
        PrintFormat::Json => print_json(&transaction),
        PrintFormat::Sputnik => print_json(&sputnik_function_call(transaction)?),
    }
}

pub(crate) fn single_transaction(plan: OperationPlan) -> anyhow::Result<PlannedTransaction> {
    let [transaction] = plan.steps.try_into().map_err(|steps: Vec<_>| {
        anyhow::anyhow!(
            "--print requires exactly one planned transaction; planned {}",
            steps.len()
        )
    })?;
    Ok(transaction)
}

fn sputnik_function_call(
    transaction: PlannedTransaction,
) -> anyhow::Result<sputnikdao2::ProposalKind> {
    let actions = transaction
        .actions
        .into_iter()
        .map(|action| {
            let Action::FunctionCall(action) = action else {
                anyhow::bail!("--print sputnik supports FunctionCall actions only");
            };
            // `ActionCall` is public but its fields are private, so construct
            // each action through the contract's JSON boundary.
            serde_json::from_value(serde_json::json!({
                "method_name": action.method_name,
                "args": Base64VecU8(action.args),
                "deposit": action.deposit,
                "gas": action.gas,
            }))
            .context("build Sputnik FunctionCall action")
        })
        .collect::<anyhow::Result<Vec<sputnikdao2::proposals::ActionCall>>>()?;

    Ok(sputnikdao2::ProposalKind::FunctionCall {
        receiver_id: transaction.receiver_id,
        actions,
    })
}

/// Serialize `output` as a single line of JSON to stdout — the machine-readable
/// result channel (diagnostics go to stderr via tracing).
pub(crate) fn print_json(output: &impl Serialize) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer(&mut lock, output)?;
    writeln!(lock)?;
    Ok(())
}

/// Build the CLI context: the selected network and an unsigned gateway client
/// backed by a transient in-memory operation store. Reads and ordinary plans use
/// that client; executed writes build a signing client per operation.
pub(crate) fn build_context(cli: &Cli) -> anyhow::Result<CliContext> {
    let network = NetworkConfigBuilder::new(cli.network)
        .rpc_url(cli.rpc_url.as_deref())
        .context("invalid RPC URL")?
        .api_key(cli.rpc_api_key.clone())
        .build();

    Ok(CliContext {
        client: Client::read_only(network.clone())?,
        network,
        selected_network: cli.network,
        transaction_url_prefix: cli.transaction_url_prefix(),
        quiet: cli.quiet > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{check_operation_status, single_transaction, sputnik_function_call};
    use near_account_id::AccountId;
    use near_api::types::transaction::actions::{Action, FunctionCallAction, TransferAction};
    use serde_json::json;
    use templar_gateway_core::{OperationPlan, PlannedTransaction};
    use templar_gateway_types::{
        common::WriteOperationResult, ManagedAccountId, NearGas, NearToken,
    };

    /// A single-step write result with the given operation status and a reverted
    /// step whose first receipt has `receipt_status`.
    fn result(status: &str, receipt_status: &str) -> WriteOperationResult {
        serde_json::from_value(json!({
            "operation": {
                "id": "op-1",
                "signer_account_id": "signer.testnet",
                "status": status,
                "steps": [{
                    "index": 0,
                    "status": { "Reverted": {
                        "tx_hash": "3DeTHGEZzdG5Vpj5b972u45DSRKTBsV87a1eGLCcFQY2",
                        "outcome": {
                            "tokens_burnt": "120081144700600000000",
                            "total_gas_burnt": "1647176572006",
                            "receipts": [
                                {"contract_id": "po-market.signer.testnet", "status": receipt_status, "logs": []},
                                {"contract_id": "signer.testnet", "status": "Succeeded", "logs": []}
                            ],
                            "return_value": null
                        }
                    }}
                }]
            }
        }))
        .expect("valid WriteOperationResult json")
    }

    fn function_call_transaction() -> PlannedTransaction {
        PlannedTransaction::single_action(
            ManagedAccountId::from("dao.near".parse::<AccountId>().expect("valid signer")),
            "governance.testnet".parse().expect("valid receiver"),
            Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "execute_proposal".to_owned(),
                args: br#"{"id":4}"#.to_vec(),
                gas: NearGas::from_tgas(300),
                deposit: NearToken::from_yoctonear(1),
            })),
        )
    }

    #[test]
    fn sputnik_renders_paste_ready_function_call_kind() {
        let transaction = function_call_transaction();
        let value = serde_json::to_value(
            sputnik_function_call(transaction).expect("FunctionCall plan should render"),
        )
        .expect("Sputnik kind should serialize");

        assert_eq!(
            value,
            json!({
                "FunctionCall": {
                    "receiver_id": "governance.testnet",
                    "actions": [{
                        "method_name": "execute_proposal",
                        "args": "eyJpZCI6NH0=",
                        "deposit": "1",
                        "gas": "300000000000000"
                    }]
                }
            })
        );
    }

    #[test]
    fn sputnik_rejects_non_function_call_actions() {
        let transaction = PlannedTransaction::single_action(
            ManagedAccountId::from("dao.near".parse::<AccountId>().expect("valid signer")),
            "receiver.testnet".parse().expect("valid receiver"),
            Action::Transfer(TransferAction {
                deposit: NearToken::from_yoctonear(1),
            }),
        );

        let json_serializes = serde_json::to_value(&transaction).is_ok();
        let Err(error) = sputnik_function_call(transaction) else {
            panic!("Sputnik FunctionCall cannot represent a transfer");
        };
        assert_eq!(
            error.to_string(),
            "--print sputnik supports FunctionCall actions only"
        );
        assert!(
            json_serializes,
            "the same transaction remains valid JSON output"
        );
    }

    #[test]
    fn print_requires_exactly_one_planned_transaction() {
        let transaction = function_call_transaction();
        assert_eq!(
            single_transaction(OperationPlan::single(transaction.clone()))
                .expect("single step")
                .receiver_id,
            transaction.receiver_id
        );

        for plan in [
            OperationPlan { steps: vec![] },
            OperationPlan {
                steps: vec![transaction.clone(), transaction.clone()],
            },
        ] {
            let error = single_transaction(plan).expect_err("non-single plan must not be rendered");
            assert!(
                error
                    .to_string()
                    .starts_with("--print requires exactly one planned transaction"),
                "{error}"
            );
        }
    }

    #[test]
    fn failed_operation_errors_and_names_reverted_receipt() {
        let error = check_operation_status(&result("Failed", "Failed"))
            .expect_err("a failed operation must map to an error (non-zero exit)");
        let message = error.to_string();
        assert!(message.contains("op-1"), "message: {message}");
        assert!(
            message.contains("po-market.signer.testnet"),
            "message should name the reverted receipt: {message}"
        );
    }

    #[test]
    fn succeeded_operation_is_ok() {
        check_operation_status(&result("Succeeded", "Succeeded"))
            .expect("a succeeded operation must not error");
    }

    #[test]
    fn non_terminal_operation_errors() {
        // A non-terminal status is an unknown outcome, not a success: with the
        // CLI's transient store there is no later resume to confirm it.
        for status in ["Pending", "InProgress"] {
            let error = check_operation_status(&result(status, "Succeeded"))
                .expect_err("a non-terminal operation must error");
            assert!(
                error.to_string().contains("did not reach a terminal state"),
                "status {status}: {error}"
            );
        }
    }
}
