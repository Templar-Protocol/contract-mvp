//! RPC-level coverage for the Pyth Lazer oracle update path.
//!
//! These tests exercise the same `OracleUpdatesDispatch` the JSON-RPC layer
//! registers, against a test context that provides a deterministic
//! [`FakeLazerSource`]. They cover the three Lazer-specific behaviors that the
//! RPC layer must guarantee:
//!
//! 1. `oracle.updateLazer` plans an adapter write carrying the payload as
//!    base64 `payload` (never classic-Pyth hex `data`). The feed set reaching
//!    the source is pinned one layer down, where the fake records its calls.
//! 2. A proxy `oracle.updatePrices` for a Lazer-backed feed routes through the
//!    Lazer source (the canned payload shows up in the planned adapter write).
//! 3. A Lazer cache-miss surfaces as a structured gateway error through the RPC
//!    layer instead of silently falling back to Hermes.

use super::*;
use base64::Engine as _;

use near_api::types::transaction::actions::Action;
use near_api::NetworkConfig;
use templar_gateway_core::{GatewayContext, NearClient, PlannedTransaction};
use templar_gateway_oracle_updates_dispatch::Dispatch as OracleUpdatesDispatch;
use templar_gateway_types::ManagedAccountId;

use fake_lazer_source::{FakeLazerError, FakeLazerSource, WithFakeLazerSource};

/// Build a minimal context (no sandbox) that only satisfies the
/// `HasNearClient + ProvidesLazerSource` bound `PlanWrite<UpdateLazer>` needs.
/// Pyth/RedStone are intentionally absent: `updateLazer` must never touch them.
fn lazer_only_context(source: FakeLazerSource) -> WithFakeLazerSource<GatewayContext> {
    let network = NetworkConfig::from_rpc_url("test", "https://example.test".parse().unwrap());
    WithFakeLazerSource::new(
        GatewayContext::from_near_client(NearClient::new(network)),
        source,
    )
}

/// Pull the single function-call action out of a planned transaction as
/// `(method_name, args_bytes)`. The single-action shape is part of the contract
/// being pinned: each Lazer step is exactly one `update_price_feeds` call.
fn unpack_function_call(plan: &PlannedTransaction) -> (String, Vec<u8>) {
    assert_eq!(
        plan.actions.len(),
        1,
        "Lazer plan must produce exactly one action, got {}",
        plan.actions.len()
    );
    match &plan.actions[0] {
        Action::FunctionCall(fc) => (fc.method_name.clone(), fc.args.clone()),
        other => panic!("expected FunctionCall action, got {other:?}"),
    }
}

/// Assert the parsed args look like a Pyth Lazer adapter write: method
/// `update_price_feeds`, a base64 `payload` field decoding to `expected`, and
/// no classic-Pyth `data` field.
fn assert_pyth_lazer_adapter_args(args_bytes: &[u8], expected_payload: &[u8]) {
    let args_json: serde_json::Value =
        serde_json::from_slice(args_bytes).expect("Lazer step args must be valid JSON");
    assert!(
        args_json.get("payload").is_some(),
        "Lazer step MUST carry `payload` (base64); got: {args_json}"
    );
    assert!(
        args_json.get("data").is_none(),
        "Lazer step MUST NOT carry classic-Pyth `data`; got: {args_json}"
    );
    let payload_b64 = args_json["payload"]
        .as_str()
        .expect("`payload` must be a JSON string");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload_b64)
        .expect("payload must be valid base64");
    assert_eq!(
        decoded, expected_payload,
        "payload must round-trip to the bytes the Lazer source returned"
    );
}

#[tokio::test]
async fn oracle_update_lazer_plan_carries_payload_base64_not_data() -> Result<()> {
    let payload = vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    let ctx = lazer_only_context(FakeLazerSource::with_payload(payload.clone()));

    let oracle_id: near_account_id::AccountId = "pyth-lazer.near".parse()?;
    let request = WriteRequest {
        signer_account_id: ManagedAccountId("relayer.near".parse()?),
        idempotency_key: None,
        body: oracle_updates::UpdateLazer {
            oracle_id: oracle_id.clone(),
            feed_ids: vec![7],
        },
    };

    let plan =
        <OracleUpdatesDispatch as PlanWrite<oracle_updates::UpdateLazer, _>>::plan(request, ctx)
            .await
            .expect("UpdateLazer plan must succeed when the Lazer source returns a payload");

    assert_eq!(
        plan.steps.len(),
        1,
        "UpdateLazer must plan exactly one step"
    );
    let step = &plan.steps[0];
    assert_eq!(step.receiver_id, oracle_id);
    let (method, args_bytes) = unpack_function_call(step);
    assert_eq!(method, "update_price_feeds");
    assert_pyth_lazer_adapter_args(&args_bytes, &payload);

    Ok(())
}

#[tokio::test]
async fn oracle_update_prices_routes_lazer_backed_proxy_through_lazer_source() -> Result<()> {
    // A Lazer-backed proxy feed must route through the Lazer source. The fake
    // returns a distinctive payload; if the plan surfaces it, routing is correct.
    let payload = vec![0x22_u8; 24];
    let stack = TestStack::start_with_lazer(
        "https://hermes-unused.example.test".parse()?,
        FakeLazerSource::with_payload(payload.clone()),
    )
    .await?;

    let lazer_oracle_id: near_account_id::AccountId = "pyth-lazer.near".parse()?;
    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;
    let price_id = PriceIdentifier([0x33; 32]);
    let feed_id = 42u32;

    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            price_id,
            Some(Proxy::median_low(
                [OracleRequest::lazer(lazer_oracle_id.clone(), feed_id).into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;

    // Plan through the registered dispatch (the same code the RPC layer calls)
    // so we can inspect the planned args without submitting on-chain: no
    // pyth-lazer adapter is deployed in the sandbox.
    let plan =
        <OracleUpdatesDispatch as PlanWrite<oracle_updates::UpdatePrices, TestContext>>::plan(
            WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: oracle_updates::UpdatePrices {
                    oracle_id: proxy_oracle_id.clone(),
                    price_ids: vec![price_id],
                },
            },
            stack.context.clone(),
        )
        .await
        .expect("UpdatePrices plan for a Lazer-backed proxy must succeed");

    // A proxy `updatePrices` plans the underlying write(s) first, then re-aggregates the
    // proxy's cached prices: [Lazer adapter write, proxy re-aggregation].
    assert_eq!(
        plan.steps.len(),
        2,
        "a Lazer-backed proxy feed must plan the underlying Lazer write plus the proxy re-aggregation"
    );
    let lazer_step = &plan.steps[0];
    assert_eq!(
        lazer_step.receiver_id, lazer_oracle_id,
        "the underlying write must target the Pyth Lazer adapter, not the proxy oracle"
    );
    let (method, args_bytes) = unpack_function_call(lazer_step);
    assert_eq!(method, "update_price_feeds");
    assert_pyth_lazer_adapter_args(&args_bytes, &payload);

    let reaggregation_step = &plan.steps[1];
    assert_eq!(
        reaggregation_step.receiver_id, proxy_oracle_id,
        "the trailing re-aggregation must target the proxy oracle"
    );
    let (method, _) = unpack_function_call(reaggregation_step);
    assert_eq!(method, "update_prices");

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_prices_rejects_bare_pyth_lazer_adapter_as_standalone_oracle() -> Result<()> {
    // A Pyth Lazer adapter is Lazer-native and not a standalone oracle. Targeting one directly
    // with `oracle.updatePrices` must fail loudly — directing the operator to wrap it in a
    // proxy oracle with a Lazer source — rather than planning a doomed classic-Pyth write.
    let stack = TestStack::start_with_lazer(
        "https://hermes-unused.example.test".parse()?,
        FakeLazerSource::with_payload(vec![0x5A_u8; 32]),
    )
    .await?;

    let price_id = PriceIdentifier([0x44; 32]);
    let adapter_id = stack
        .harness
        .deploy_pyth_lazer_adapter("pyth-lazer-direct")
        .await?;

    let error =
        <OracleUpdatesDispatch as PlanWrite<oracle_updates::UpdatePrices, TestContext>>::plan(
            WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: oracle_updates::UpdatePrices {
                    oracle_id: adapter_id.clone(),
                    price_ids: vec![price_id],
                },
            },
            stack.context.clone(),
        )
        .await
        .expect_err("a bare Pyth Lazer adapter must not be usable as a standalone oracle");

    let error_string = format!("{error}");
    assert!(
        error_string.contains("standalone oracle") && error_string.contains("proxy"),
        "error must direct the operator to wrap the adapter in a proxy; got: {error_string}"
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_prices_rejects_pyth_lazer_adapter_as_classic_pyth_proxy_source() -> Result<()>
{
    // A proxy must reference a Pyth Lazer adapter as a `Lazer` source, never a classic `Pyth`
    // source: the gateway would otherwise plan a Pyth VAA write the adapter's ABI rejects.
    // Resolution must reject the misconfiguration up front, naming Lazer.
    let stack = TestStack::start_with_lazer(
        "https://hermes-unused.example.test".parse()?,
        FakeLazerSource::with_payload(vec![0x5A_u8; 32]),
    )
    .await?;

    let proxy_oracle_id = stack.harness.deploy_proxy_oracle().await?;
    let price_id = PriceIdentifier([0x44; 32]);
    let adapter_id = stack
        .harness
        .deploy_pyth_lazer_adapter("pyth-lazer-source")
        .await?;
    stack
        .harness
        .admin_set_proxy(
            proxy_oracle_id.clone(),
            price_id,
            Some(Proxy::median_low(
                [OracleRequest::pyth(adapter_id.clone(), price_id).into()],
                FreshnessFilter::empty(),
            )),
        )
        .await?;

    let error =
        <OracleUpdatesDispatch as PlanWrite<oracle_updates::UpdatePrices, TestContext>>::plan(
            WriteRequest {
                signer_account_id: stack.harness.gateway_signer_account_id.clone(),
                idempotency_key: None,
                body: oracle_updates::UpdatePrices {
                    oracle_id: proxy_oracle_id.clone(),
                    price_ids: vec![price_id],
                },
            },
            stack.context.clone(),
        )
        .await
        .expect_err("a proxy Pyth source pointing at a Pyth Lazer adapter must be rejected");

    let error_string = format!("{error}");
    assert!(
        error_string.contains("Lazer"),
        "error must direct the operator to use a Lazer source; got: {error_string}"
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_lazer_cache_miss_returns_structured_gateway_error() -> Result<()> {
    // Hermes is mocked and would happily return a VAA, but `updateLazer` must
    // only consult the Lazer source. A cache-miss there must surface as a
    // structured gateway error rather than a silent Hermes fallback.
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_lazer(
        hermes.uri().parse()?,
        FakeLazerSource::failing(FakeLazerError::CacheMiss),
    )
    .await?;

    let error = stack
        .controller
        .request::<oracle_updates::UpdateLazer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdateLazer {
                oracle_id: "pyth-lazer.near".parse()?,
                feed_ids: vec![7],
            },
        })
        .await
        .expect_err(
            "a Lazer cache-miss must surface as an RPC error, not a silent Hermes fallback",
        );

    let error_string = format!("{error}");
    assert!(
        error_string.contains("cache miss"),
        "RPC error must carry the Lazer cache-miss detail; got: {error_string}"
    );
    assert!(
        error_string.contains("-32000"),
        "RPC error must surface as a structured gateway server error; got: {error_string}"
    );

    stack.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn oracle_update_lazer_stale_payload_is_a_hard_error() -> Result<()> {
    // A stale Lazer payload is a hard error at plan time: the gateway must not
    // silently drop the feed or retry via Hermes.
    let hermes = start_mock_hermes_server("cafebabe").await?;
    let stack = TestStack::start_with_lazer(
        hermes.uri().parse()?,
        FakeLazerSource::failing(FakeLazerError::Stale),
    )
    .await?;

    let error = stack
        .controller
        .request::<oracle_updates::UpdateLazer>(&WriteRequest {
            signer_account_id: stack.harness.gateway_signer_account_id.clone(),
            idempotency_key: None,
            body: oracle_updates::UpdateLazer {
                oracle_id: "pyth-lazer.near".parse()?,
                feed_ids: vec![7],
            },
        })
        .await
        .expect_err("a stale Lazer payload must be a hard RPC error");

    let error_string = format!("{error}");
    assert!(
        error_string.contains("stale"),
        "RPC error must carry the Lazer stale-payload detail; got: {error_string}"
    );

    stack.shutdown().await;
    Ok(())
}
