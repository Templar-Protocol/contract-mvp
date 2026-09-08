use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use near_account_id::AccountId;
use templar_common::oracle::{pyth::PriceIdentifier, redstone};
use templar_gateway_core::{
    client::{proxy_oracle::UpdatePricesArgs, ContractWriteOptions},
    plan_pyth_lazer_update, plan_pyth_update, plan_redstone_write_prices, query_oracle_kind,
    resolve_price_dependencies, GatewayError, GatewayResult, HasNearClient, OperationPlan,
    OraclePayloadSource, PlanWrite, PlannedTransaction,
};
use templar_gateway_oracle_updates_spec::oracle::{
    UpdateLazer, UpdatePrices, UpdatePyth, UpdateRedStone,
};
use templar_gateway_types::{ManagedAccountId, OracleContractKind};
use templar_proxy_oracle_near_common::request::{LazerRequest, OracleRequest};

use crate::{Dispatch, ProvidesLazerSource, ProvidesPythSource, ProvidesRedStoneSource};

#[async_trait]
impl<C> PlanWrite<UpdatePyth, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePyth>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        if body.price_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updatePyth requested with no price ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        plan_pyth_feed_update(
            &ctx,
            request.signer_account_id,
            body.oracle_id,
            &body.price_ids,
        )
        .await
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdateRedStone, C> for Dispatch
where
    C: HasNearClient + ProvidesRedStoneSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdateRedStone>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        // Nothing to fetch and nothing to write. A step-less plan settles as a terminal
        // no-op, matching `oracle.updatePrices` with no price ids.
        if body.feed_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updateRedStone requested with no feed ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        tracing::debug!(
            oracle_id = %body.oracle_id,
            feed_count = body.feed_ids.len(),
            "fetching RedStone payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &body.feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        plan_redstone_write_prices(
            ctx.near_client(),
            request.signer_account_id,
            body.oracle_id,
            body.feed_ids,
            payload,
        )
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdateLazer, C> for Dispatch
where
    C: HasNearClient + ProvidesLazerSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdateLazer>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let body = request.body;
        if body.feed_ids.is_empty() {
            tracing::warn!(
                oracle_id = %body.oracle_id,
                "oracle.updateLazer requested with no feed ids; nothing to update"
            );
            return Ok(OperationPlan { steps: Vec::new() });
        }
        plan_lazer_feed_update(
            &ctx,
            request.signer_account_id,
            body.oracle_id,
            &body.feed_ids,
        )
        .await
        .map(OperationPlan::from)
    }
}

#[async_trait]
impl<C> PlanWrite<UpdatePrices, C> for Dispatch
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + ProvidesLazerSource,
{
    async fn plan(
        request: templar_gateway_types::common::WriteRequest<UpdatePrices>,
        ctx: C,
    ) -> GatewayResult<OperationPlan> {
        let signer_account_id = request.signer_account_id;
        let oracle_id = request.body.oracle_id;
        let price_ids = request.body.price_ids;

        let (kind, requests) = resolve_update_requests(&ctx, oracle_id.clone(), &price_ids).await?;

        let mut plan = plan_grouped_updates(&ctx, signer_account_id.clone(), requests).await?;

        // When the target is a proxy oracle, re-aggregate its cached prices after the
        // underlying updates. Steps execute sequentially, so the proxy read sees the fresh
        // underlying prices this same operation just wrote.
        if matches!(kind, OracleContractKind::Proxy) {
            // The underlying oracle updates are mutually independent: one reverting must
            // not cancel the others, nor the re-aggregation. Mark them continue_on_failure
            // so a revert is recorded and tolerated while the operation advances. The
            // re-aggregation step appended below stays non-fallible — it always runs (after
            // whatever underlying prices did land) and its outcome is the operation's verdict.
            // A direct/LST oracle takes neither branch, so its single underlying update stays
            // all-or-nothing.
            for step in &mut plan.steps {
                step.continue_on_failure = true;
            }
            plan.steps
                .push(ctx.near_client().proxy_oracle(oracle_id).update_prices(
                    ContractWriteOptions::new(signer_account_id).tgas(100),
                    UpdatePricesArgs { price_ids },
                )?);
        }

        Ok(plan)
    }
}

/// Fetch the VAA covering `price_ids` from Hermes and plan the adapter write.
async fn plan_pyth_feed_update<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    price_ids: &[PriceIdentifier],
) -> GatewayResult<PlannedTransaction>
where
    C: HasNearClient + ProvidesPythSource,
{
    tracing::debug!(
        %oracle_id,
        price_count = price_ids.len(),
        "fetching Pyth payload for gateway oracle update"
    );
    let vaa = OraclePayloadSource::fetch_payload(ctx.pyth_source(), price_ids)
        .await
        .map_err(|error| GatewayError::HttpRequest(error.to_string()))?;
    plan_pyth_update(ctx.near_client(), signer_account_id, oracle_id, vaa)
}

/// Fetch the payload covering `feed_ids` from the Lazer stream and plan the adapter write.
async fn plan_lazer_feed_update<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    oracle_id: AccountId,
    feed_ids: &[u32],
) -> GatewayResult<PlannedTransaction>
where
    C: HasNearClient + ProvidesLazerSource,
{
    tracing::debug!(
        %oracle_id,
        feed_count = feed_ids.len(),
        "fetching Pyth Lazer payload for gateway oracle update"
    );
    let payload = OraclePayloadSource::fetch_payload(ctx.lazer_source(), feed_ids)
        .await
        .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
    plan_pyth_lazer_update(ctx.near_client(), signer_account_id, oracle_id, payload)
}

async fn plan_grouped_updates<C>(
    ctx: &C,
    signer_account_id: ManagedAccountId,
    requests: Vec<OracleRequest>,
) -> GatewayResult<OperationPlan>
where
    C: HasNearClient + ProvidesPythSource + ProvidesRedStoneSource + ProvidesLazerSource,
{
    let mut steps = Vec::new();
    let mut pyth_updates = BTreeMap::<AccountId, BTreeSet<PriceIdentifier>>::new();
    let mut redstone_updates = BTreeMap::<AccountId, BTreeSet<redstone::FeedId>>::new();
    let mut lazer_updates = BTreeMap::<AccountId, BTreeSet<u32>>::new();

    for request in requests {
        match request {
            OracleRequest::Pyth(request) => {
                pyth_updates
                    .entry(request.oracle_id)
                    .or_default()
                    .insert(request.price_id);
            }
            OracleRequest::RedStone(request) => {
                redstone_updates
                    .entry(request.oracle_id)
                    .or_default()
                    .insert(request.price_id);
            }
            OracleRequest::Lazer(LazerRequest { oracle_id, feed_id }) => {
                lazer_updates.entry(oracle_id).or_default().insert(feed_id);
            }
        }
    }

    tracing::debug!(
        pyth_oracle_count = pyth_updates.len(),
        redstone_oracle_count = redstone_updates.len(),
        lazer_oracle_count = lazer_updates.len(),
        "resolved oracle update dependencies"
    );

    for (oracle_id, price_ids) in pyth_updates {
        let price_ids = price_ids.into_iter().collect::<Vec<_>>();
        steps.push(
            plan_pyth_feed_update(ctx, signer_account_id.clone(), oracle_id, &price_ids).await?,
        );
    }

    for (oracle_id, feed_ids) in redstone_updates {
        let feed_ids = feed_ids.into_iter().collect::<Vec<_>>();
        tracing::debug!(
            %oracle_id,
            feed_count = feed_ids.len(),
            "fetching RedStone payload for gateway oracle update"
        );
        let payload = OraclePayloadSource::fetch_payload(ctx.redstone_source(), &feed_ids)
            .await
            .map_err(|error| GatewayError::ExternalService(error.to_string()))?;
        steps.push(plan_redstone_write_prices(
            ctx.near_client(),
            signer_account_id.clone(),
            oracle_id,
            feed_ids,
            payload,
        )?);
    }

    for (oracle_id, feed_ids) in lazer_updates {
        let feed_ids: Vec<u32> = feed_ids.into_iter().collect();
        steps.push(
            plan_lazer_feed_update(ctx, signer_account_id.clone(), oracle_id, &feed_ids).await?,
        );
    }

    Ok(OperationPlan { steps })
}

/// Resolve every requested `price_id` on `oracle_id` into the underlying source updates,
/// returning the oracle's kind alongside so the caller can append the proxy re-aggregation
/// step. Delegates to the shared `gateway_core` resolution so writes agree with reads and
/// `oracle.getPriceResolutionDependencies` on how each oracle resolves.
async fn resolve_update_requests<C: HasNearClient>(
    ctx: &C,
    oracle_id: AccountId,
    price_ids: &[PriceIdentifier],
) -> GatewayResult<(OracleContractKind, Vec<OracleRequest>)> {
    let kind = query_oracle_kind(ctx, oracle_id.clone()).await?;
    let mut requests = BTreeSet::new();

    for &price_id in price_ids {
        requests.extend(resolve_price_dependencies(ctx, oracle_id.clone(), price_id, &kind).await?);
    }

    Ok((kind, requests.into_iter().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use near_api::types::transaction::actions::Action;
    use near_api::NetworkConfig;
    use rstest::rstest;
    use std::sync::{Arc, Mutex};
    use templar_gateway_core::NearClient;
    use templar_gateway_types::common::WriteRequest;
    use thiserror::Error;

    // Every source error carries `err.to_string()` into its gateway variant, so one
    // variant exercises that path; the tests differ by entry point, not error kind.
    #[derive(Debug, Clone, Error)]
    #[error("fake source unavailable")]
    struct FakeError;

    #[derive(Clone)]
    struct FakeLazerSource {
        outcome: Result<Vec<u8>, FakeError>,
        calls: Arc<Mutex<Vec<Vec<u32>>>>,
    }

    impl FakeLazerSource {
        fn new(outcome: Result<Vec<u8>, FakeError>) -> Self {
            Self {
                outcome,
                calls: Arc::default(),
            }
        }

        fn calls(&self) -> Vec<Vec<u32>> {
            self.calls
                .lock()
                .expect("calls mutex must not be poisoned")
                .clone()
        }
    }

    #[async_trait]
    impl OraclePayloadSource for FakeLazerSource {
        type PriceId = u32;
        type Error = FakeError;

        async fn fetch_payload(&self, price_ids: &[u32]) -> Result<Vec<u8>, FakeError> {
            self.calls
                .lock()
                .expect("calls mutex must not be poisoned")
                .push(price_ids.to_vec());
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct FakePythSource {
        outcome: Result<Vec<u8>, FakeError>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakePythSource {
        type PriceId = PriceIdentifier;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _price_ids: &[PriceIdentifier],
        ) -> Result<Vec<u8>, FakeError> {
            self.outcome.clone()
        }
    }

    #[derive(Clone)]
    struct FakeRedStoneSource {
        payload: Vec<u8>,
    }

    #[async_trait]
    impl OraclePayloadSource for FakeRedStoneSource {
        type PriceId = redstone::FeedId;
        type Error = FakeError;

        async fn fetch_payload(
            &self,
            _feed_ids: &[redstone::FeedId],
        ) -> Result<Vec<u8>, FakeError> {
            Ok(self.payload.clone())
        }
    }

    #[derive(Clone)]
    struct TestCtx {
        near_client: NearClient,
        pyth_source: FakePythSource,
        redstone_source: FakeRedStoneSource,
        lazer_source: FakeLazerSource,
    }

    impl HasNearClient for TestCtx {
        fn near_client(&self) -> &NearClient {
            &self.near_client
        }
    }

    impl ProvidesPythSource for TestCtx {
        type PythSource = FakePythSource;

        fn pyth_source(&self) -> &FakePythSource {
            &self.pyth_source
        }
    }

    impl ProvidesRedStoneSource for TestCtx {
        type RedStoneSource = FakeRedStoneSource;

        fn redstone_source(&self) -> &FakeRedStoneSource {
            &self.redstone_source
        }
    }

    impl ProvidesLazerSource for TestCtx {
        type LazerSource = FakeLazerSource;

        fn lazer_source(&self) -> &FakeLazerSource {
            &self.lazer_source
        }
    }

    fn test_client() -> NearClient {
        NearClient::new(NetworkConfig::from_rpc_url(
            "test",
            "https://example.test".parse().expect("valid url"),
        ))
    }

    fn signer_id() -> ManagedAccountId {
        ManagedAccountId("relayer.near".parse().expect("valid account id"))
    }

    fn unpack_function_call(step: &PlannedTransaction) -> (String, Vec<u8>) {
        assert_eq!(
            step.actions.len(),
            1,
            "planner must produce exactly one action, got {}",
            step.actions.len()
        );
        match &step.actions[0] {
            Action::FunctionCall(fc) => (fc.method_name.clone(), fc.args.clone()),
            other => panic!("expected FunctionCall action, got {other:?}"),
        }
    }

    /// A `TestCtx` whose every source succeeds with an empty payload. Override the one
    /// source under test with struct-update syntax.
    fn inert_ctx() -> TestCtx {
        TestCtx {
            near_client: test_client(),
            pyth_source: FakePythSource {
                outcome: Ok(Vec::new()),
            },
            redstone_source: FakeRedStoneSource {
                payload: Vec::new(),
            },
            lazer_source: FakeLazerSource::new(Ok(Vec::new())),
        }
    }

    fn pyth_ctx(outcome: Result<Vec<u8>, FakeError>) -> TestCtx {
        TestCtx {
            pyth_source: FakePythSource { outcome },
            ..inert_ctx()
        }
    }

    fn lazer_ctx(outcome: Result<Vec<u8>, FakeError>) -> TestCtx {
        TestCtx {
            lazer_source: FakeLazerSource::new(outcome),
            ..inert_ctx()
        }
    }

    fn write_request<B>(body: B) -> WriteRequest<B> {
        WriteRequest {
            signer_account_id: signer_id(),
            idempotency_key: None,
            body,
        }
    }

    #[tokio::test]
    async fn update_pyth_writes_the_vaa_it_fetched() {
        let vaa = vec![0x01, 0x02, 0x03, 0x04];
        let oracle_id: AccountId = "pyth.near".parse().expect("valid account id");
        let request = write_request(UpdatePyth {
            oracle_id: oracle_id.clone(),
            price_ids: vec![PriceIdentifier([0xAA; 32])],
        });

        let plan =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Ok(vaa.clone())))
                .await
                .expect("UpdatePyth plan must succeed with a fresh payload");

        assert_eq!(plan.steps.len(), 1, "UpdatePyth must plan exactly one step");
        let step = &plan.steps[0];
        assert_eq!(step.receiver_id, oracle_id);

        let (method, args_bytes) = unpack_function_call(step);
        assert_eq!(method, "update_price_feeds");
        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert_eq!(
            args_json["data"],
            hex::encode(&vaa),
            "the adapter must receive the fetched VAA as hex; got: {args_json}"
        );
    }

    #[tokio::test]
    async fn update_pyth_with_no_price_ids_plans_nothing() {
        let request = write_request(UpdatePyth {
            oracle_id: "pyth.near".parse().unwrap(),
            price_ids: Vec::new(),
        });

        let plan =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Err(FakeError)))
                .await
                .expect("an empty UpdatePyth must not reach the source");

        assert!(plan.steps.is_empty(), "expected a step-less plan");
    }

    #[tokio::test]
    async fn update_pyth_propagates_source_error() {
        let request = write_request(UpdatePyth {
            oracle_id: "pyth.near".parse().unwrap(),
            price_ids: vec![PriceIdentifier([0xAA; 32])],
        });

        let error =
            <Dispatch as PlanWrite<UpdatePyth, TestCtx>>::plan(request, pyth_ctx(Err(FakeError)))
                .await
                .expect_err("a Hermes error must surface as a plan error");

        assert!(
            matches!(error, GatewayError::HttpRequest(ref msg) if msg.contains("unavailable")),
            "expected HttpRequest carrying the source-error detail, got {error:?}"
        );
    }

    #[rstest]
    #[case::single_feed(vec![7])]
    #[case::multiple_feeds(vec![7, 8])]
    #[tokio::test]
    async fn update_lazer_plans_one_adapter_write(#[case] feed_ids: Vec<u32>) {
        let payload = vec![0xAA, 0xBB, 0xCC, 0xDD];
        let ctx = lazer_ctx(Ok(payload.clone()));
        // Shares the recorder with the context `plan` consumes.
        let source = ctx.lazer_source.clone();
        let oracle_id: AccountId = "pyth-lazer.near".parse().expect("valid account id");
        let request = write_request(UpdateLazer {
            oracle_id: oracle_id.clone(),
            feed_ids: feed_ids.clone(),
        });

        let plan = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect("UpdateLazer plan must succeed with a fresh payload");

        assert_eq!(
            source.calls(),
            vec![feed_ids],
            "every requested feed must reach the source in a single fetch"
        );
        assert_eq!(
            plan.steps.len(),
            1,
            "UpdateLazer must plan exactly one step for the whole feed set"
        );
        let step = &plan.steps[0];
        assert_eq!(step.receiver_id, oracle_id);

        let (method, args_bytes) = unpack_function_call(step);
        assert_eq!(method, "update_price_feeds");

        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert!(
            args_json.get("payload").is_some(),
            "UpdateLazer args MUST carry `payload` (base64); got: {args_json}"
        );
        assert!(
            args_json.get("data").is_none(),
            "UpdateLazer args MUST NOT carry `data`; got: {args_json}"
        );

        let payload_b64 = args_json["payload"]
            .as_str()
            .expect("`payload` must be a json string");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(payload_b64)
            .expect("payload must be valid base64");
        assert_eq!(
            decoded, payload,
            "payload must round-trip to the fetched bytes"
        );
    }

    #[tokio::test]
    async fn update_lazer_with_no_feed_ids_plans_nothing() {
        let ctx = lazer_ctx(Err(FakeError));
        let request = write_request(UpdateLazer {
            oracle_id: "pyth-lazer.near".parse().unwrap(),
            feed_ids: Vec::new(),
        });

        let plan = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect("an empty UpdateLazer must not reach the source");

        assert!(plan.steps.is_empty(), "expected a step-less plan");
    }

    #[tokio::test]
    async fn update_lazer_propagates_source_error() {
        let ctx = lazer_ctx(Err(FakeError));
        let request = write_request(UpdateLazer {
            oracle_id: "pyth-lazer.near".parse().unwrap(),
            feed_ids: vec![7],
        });

        let error = <Dispatch as PlanWrite<UpdateLazer, TestCtx>>::plan(request, ctx)
            .await
            .expect_err("a Lazer source error must surface as a plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("unavailable")),
            "expected ExternalService carrying the source-error detail, got {error:?}"
        );
    }

    #[tokio::test]
    async fn update_prices_groups_lazer_separately() {
        let pyth_payload = vec![0x11_u8; 16];
        let lazer_payload = vec![0x22_u8; 24];
        let ctx = TestCtx {
            pyth_source: FakePythSource {
                outcome: Ok(pyth_payload.clone()),
            },
            lazer_source: FakeLazerSource::new(Ok(lazer_payload.clone())),
            ..inert_ctx()
        };
        let pyth_oracle: AccountId = "pyth.near".parse().unwrap();
        let lazer_oracle: AccountId = "pyth-lazer.near".parse().unwrap();
        let price_id = PriceIdentifier([0xAA; 32]);

        let requests = vec![
            OracleRequest::pyth(pyth_oracle.clone(), price_id),
            OracleRequest::lazer(lazer_oracle.clone(), 42),
        ];

        let plan = plan_grouped_updates(&ctx, signer_id(), requests)
            .await
            .expect("mixed Pyth + Lazer grouping must succeed");

        assert_eq!(
            plan.steps.len(),
            2,
            "mixed grouping must produce one step per oracle"
        );
        assert!(
            plan.steps.iter().all(|step| !step.continue_on_failure),
            "plan_grouped_updates must leave steps all-or-nothing; marking underlying \
             updates continue_on_failure is the proxy branch's job in UpdatePrices::plan"
        );

        let mut found_pyth_data_step = false;
        let mut found_lazer_payload_step = false;
        for step in &plan.steps {
            let (method, args_bytes) = unpack_function_call(step);
            assert_eq!(method, "update_price_feeds");
            let args_json: serde_json::Value =
                serde_json::from_slice(&args_bytes).expect("step args must be valid json");

            if step.receiver_id == pyth_oracle {
                assert!(
                    args_json.get("data").is_some(),
                    "classic Pyth step MUST carry `data` (hex); got: {args_json}"
                );
                assert!(
                    args_json.get("payload").is_none(),
                    "classic Pyth step MUST NOT carry `payload`; got: {args_json}"
                );
                found_pyth_data_step = true;
            } else if step.receiver_id == lazer_oracle {
                assert!(
                    args_json.get("payload").is_some(),
                    "Lazer step MUST carry `payload` (base64); got: {args_json}"
                );
                assert!(
                    args_json.get("data").is_none(),
                    "Lazer step MUST NOT carry `data`; got: {args_json}"
                );
                found_lazer_payload_step = true;
            }
        }
        assert!(
            found_pyth_data_step,
            "mixed plan must include a classic Pyth `data` step"
        );
        assert!(
            found_lazer_payload_step,
            "mixed plan must include a Lazer `payload` step"
        );
    }

    #[tokio::test]
    async fn update_prices_lazer_source_error_is_hard_error() {
        let ctx = lazer_ctx(Err(FakeError));
        let lazer_oracle: AccountId = "pyth-lazer.near".parse().unwrap();
        let requests = vec![OracleRequest::lazer(lazer_oracle, 7)];

        let error = plan_grouped_updates(&ctx, signer_id(), requests)
            .await
            .expect_err("a Lazer source error must be a hard plan error");

        assert!(
            matches!(error, GatewayError::ExternalService(ref msg) if msg.contains("unavailable")),
            "expected ExternalService carrying the source-error detail, got {error:?}"
        );
    }

    // The proxy re-aggregation step `UpdatePrices::plan` appends for a proxy oracle must
    // call the proxy's own `update_prices` with the requested (proxy-level) price ids. The
    // kind gating itself needs a live `contract.getKind` query and is covered by the
    // gateway sandbox test `oracle_update_prices_endpoint_resolves_and_updates_dependencies`.
    #[test]
    fn proxy_reaggregation_step_calls_proxy_update_prices() {
        let ctx = inert_ctx();
        let oracle_id: AccountId = "proxy.near".parse().expect("valid account id");
        let price_ids = vec![PriceIdentifier([0x11; 32]), PriceIdentifier([0x22; 32])];

        let step = ctx
            .near_client()
            .proxy_oracle(oracle_id.clone())
            .update_prices(
                ContractWriteOptions::new(signer_id()).tgas(100),
                UpdatePricesArgs {
                    price_ids: price_ids.clone(),
                },
            )
            .expect("proxy re-aggregation step must build");

        assert_eq!(step.receiver_id, oracle_id);
        let (method, args_bytes) = unpack_function_call(&step);
        assert_eq!(method, "update_prices");
        let args_json: serde_json::Value =
            serde_json::from_slice(&args_bytes).expect("args must be valid json");
        assert_eq!(
            args_json["price_ids"].as_array().map(Vec::len),
            Some(price_ids.len()),
            "proxy step must carry every requested price id; got: {args_json}"
        );
    }
}
