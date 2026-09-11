//! Recovery capability matrix and governance production-policy tests.

use std::collections::BTreeMap;

use base64::Engine as _;
use sha2::Digest;
use stellar_baselib::{
    account::{Account, AccountBehavior as _},
    keypair::{Keypair, KeypairBehavior as _},
    transaction::{Transaction, TransactionBehavior as _},
    transaction_builder::{TransactionBuilder, TransactionBuilderBehavior as _},
    xdr::{Limits, WriteXdr as _},
};
use templar_oft_bridge_cli::domain::Vm;
use templar_oft_bridge_cli::domain::{Environment, ExecutablePlanV1, ProposalV1, SCHEMA_VERSION};
use templar_oft_bridge_cli::error::Error;
use templar_oft_bridge_cli::governance::{
    attach_signature, build_proposal, recovery_capability, signature_verification_data,
    CheckedGovernancePolicy, GovernancePolicyAdapter, RecoveryCapabilityV1, RecoveryMechanism,
    RecoveryScenario,
};

const SECRET: &str = "SD7X7LEHBNMUIKQGKPARG5TDJNBHKC346OUARHGZL5ITC6IJPXHILY36";
const SIGNER: &str = "GDFQVQCYYB7GKCGSCUSIQYXTPLV5YJ3XWDMWGQMDNM4EAXAL7LITIBQ7";
const PASSPHRASE: &str = "Test SDF Network ; September 2015";
const MAINNET_PASSPHRASE: &str = "Public Global Stellar Network ; September 2015";

fn envelope_for(passphrase: &str) -> String {
    let mut account = Account::new(SIGNER, "6").unwrap();
    TransactionBuilder::new(&mut account, passphrase, None)
        .fee(100u32)
        .build_for_simulation()
        .to_envelope()
        .unwrap()
        .to_xdr_base64(Limits::none())
        .unwrap()
}

fn signature(proposal: &ProposalV1) -> String {
    let passphrase = &proposal.plan.stellar.as_ref().unwrap().network_passphrase;
    let transaction = Transaction::from_xdr_envelope(
        &proposal.plan.stellar.as_ref().unwrap().envelope_xdr,
        passphrase,
    );
    hex::encode(
        Keypair::from_secret(SECRET)
            .unwrap()
            .sign(&transaction.hash())
            .unwrap(),
    )
}

fn plan() -> ExecutablePlanV1 {
    plan_for(PASSPHRASE)
}

fn plan_for(passphrase: &str) -> ExecutablePlanV1 {
    let envelope = envelope_for(passphrase);
    ExecutablePlanV1 {
        schema_name: "executable_plan".to_string(),
        schema_version: SCHEMA_VERSION,
        route_id: "route-recovery".to_string(),
        desired_sha256: "desired".to_string(),
        operation: templar_oft_bridge_cli::domain::OperationV1::SetDefaultFee { bps: 10 },
        artifact_lock_sha256: "artifact-lock".to_string(),
        simulation_sha256: "sim".to_string(),
        expires_at_unix: u64::MAX,
        stellar: Some(templar_oft_bridge_cli::domain::StellarPlanBindingV1 {
            network_passphrase: passphrase.to_string(),
            source_account: SIGNER.to_string(),
            sequence: "7".to_string(),
            min_ledger: 1,
            max_ledger: 100,
            auth_entries: Vec::new(),
            envelope_xdr: envelope.clone(),
            envelope_sha256: hex::encode(sha2::Sha256::digest(
                base64::engine::general_purpose::STANDARD
                    .decode(envelope)
                    .unwrap(),
            )),
            simulation_ledger: 50,
            signer_weights: BTreeMap::from([(SIGNER.to_string(), 1)]),
            required_threshold_weight: 1,
            threshold_level: "medium".to_string(),
        }),
        evm: None,
        continuation_sha256: String::new(),
    }
}

fn proposal() -> ProposalV1 {
    ProposalV1 {
        schema_name: "proposal".to_string(),
        schema_version: SCHEMA_VERSION,
        plan: plan(),
        signatures: BTreeMap::new(),
    }
}

#[test]
fn contained_outbound_is_restore_authorized() {
    let capability = recovery_capability(Vm::Stellar, RecoveryScenario::OutboundContained);
    assert_eq!(
        capability,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRestore,
            requires_source_account: true,
        }
    );
}

#[test]
fn undelivered_packets_are_resend_authorized() {
    for vm in [Vm::Stellar, Vm::Evm] {
        let capability = recovery_capability(vm, RecoveryScenario::PacketUndelivered);
        assert_eq!(
            capability,
            RecoveryCapabilityV1::Authorized {
                mechanism: RecoveryMechanism::OperatorResend,
                requires_source_account: true,
            }
        );
    }
}

#[test]
fn delivered_unacknowledged_is_unavailable() {
    for vm in [Vm::Stellar, Vm::Evm] {
        let capability = recovery_capability(vm, RecoveryScenario::PacketDeliveredUnacknowledged);
        match capability {
            RecoveryCapabilityV1::Unavailable { reason } => {
                assert_eq!(reason, "await_native_acknowledgement_v1");
            }
            other @ RecoveryCapabilityV1::Authorized { .. } => {
                panic!("expected unavailable, got {other:?}")
            }
        }
    }
}

#[test]
fn acknowledged_unsettled_splits_by_vm() {
    let stellar = recovery_capability(Vm::Stellar, RecoveryScenario::PacketAcknowledgedUnsettled);
    assert_eq!(
        stellar,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorRefund,
            requires_source_account: true,
        }
    );
    let evm = recovery_capability(Vm::Evm, RecoveryScenario::PacketAcknowledgedUnsettled);
    assert_eq!(
        evm,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OwnerTimelockWithdraw,
            requires_source_account: false,
        }
    );
}

#[test]
fn mainnet_recovery_can_be_planned_for_external_authorization() {
    let capability = CheckedGovernancePolicy
        .plan_recovery(
            Environment::StellarMainnetEthereum,
            Vm::Stellar,
            RecoveryScenario::PacketUndelivered,
        )
        .expect("mainnet recovery plan");
    assert_eq!(
        capability,
        RecoveryCapabilityV1::Authorized {
            mechanism: RecoveryMechanism::OperatorResend,
            requires_source_account: true,
        }
    );
}

#[test]
fn mainnet_proposal_and_external_signature_paths_are_allowed() {
    let environment = Environment::StellarMainnetEthereum;
    let built =
        build_proposal(environment, plan_for(MAINNET_PASSPHRASE)).expect("mainnet proposal");
    let signature = signature(&built);
    let attached = attach_signature(environment, &built, SIGNER, &signature)
        .expect("verified external mainnet signature");
    let verified =
        signature_verification_data(environment, &attached).expect("mainnet verification data");
    assert_eq!(verified.attached_weight, 1);
    assert!(verified.threshold_satisfied);
}

#[test]
fn proposal_environment_mismatch_fails_closed() {
    let error = build_proposal(Environment::StellarMainnetEthereum, plan())
        .expect_err("testnet-bound plan must not become a mainnet proposal");
    assert!(matches!(error, Error::Policy(_)));
}

#[test]
fn testnet_proposal_attach_and_verification_are_deterministic() {
    let environment = Environment::StellarTestnetSepolia;
    let first = build_proposal(environment, plan()).unwrap();
    let second = build_proposal(environment, plan()).unwrap();
    assert_eq!(first, second);
    assert!(first.signatures.is_empty());

    let signature = signature(&first);
    let attached = attach_signature(environment, &first, SIGNER, &signature).unwrap();
    assert_eq!(attached.signatures.get(SIGNER), Some(&signature));
    let again = attach_signature(environment, &attached, SIGNER, &signature).unwrap();
    assert_eq!(attached, again);

    let data = signature_verification_data(environment, &again).unwrap();
    let repeated = signature_verification_data(environment, &again).unwrap();
    assert_eq!(data, repeated);
    assert_eq!(data.route_id, "route-recovery");
    assert_eq!(data.sender, SIGNER);
    assert_eq!(data.sequence_or_nonce, "7");
    assert_eq!(data.unsigned_payload, envelope_for(PASSPHRASE));
    assert_eq!(data.unsigned_payload_sha256.len(), 64);
    assert_eq!(data.plan_sha256.len(), 64);
    assert_eq!(data.expires_at_unix, u64::MAX);
    assert_eq!(data.attached_weight, 1);
    assert_eq!(data.required_weight, 1);
    assert!(data.threshold_satisfied);
}

#[test]
fn signature_attachment_refuses_empty_inputs() {
    let environment = Environment::StellarTestnetSepolia;
    let proposal = proposal();
    let empty_signer = attach_signature(environment, &proposal, " ", "sig").unwrap_err();
    assert!(matches!(empty_signer, Error::Policy(_)));
    let empty_signature = attach_signature(environment, &proposal, SIGNER, "").unwrap_err();
    assert!(matches!(empty_signature, Error::InvalidInput(_)));
}

#[test]
fn expired_proposal_cannot_accept_or_verify_signatures() {
    let environment = Environment::StellarTestnetSepolia;
    let mut proposal = proposal();
    proposal.plan.expires_at_unix = 0;
    let signature = signature(&proposal);
    assert!(matches!(
        attach_signature(environment, &proposal, SIGNER, &signature),
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        signature_verification_data(environment, &proposal),
        Err(Error::Conflict(_))
    ));
}

#[test]
fn checked_governance_trait_matches_free_functions() {
    let adapter = CheckedGovernancePolicy;
    let environment = Environment::StellarTestnetSepolia;
    let built = adapter.build_proposal(environment, plan()).unwrap();
    let signature = signature(&built);
    let attached = adapter
        .attach_signature(environment, &built, SIGNER, &signature)
        .unwrap();
    assert_eq!(
        adapter
            .signature_verification_data(environment, &attached)
            .unwrap(),
        signature_verification_data(environment, &attached).unwrap()
    );
    assert_eq!(
        adapter
            .plan_recovery(
                environment,
                Vm::Stellar,
                RecoveryScenario::OutboundContained
            )
            .unwrap(),
        recovery_capability(Vm::Stellar, RecoveryScenario::OutboundContained)
    );
}
