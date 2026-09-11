use std::path::PathBuf;

use anyhow::Context as _;
use clap::Args;
use near_account_id::AccountId;
use templar_common::Nanoseconds;
use templar_gateway_methods_spec::proxy_oracle_governance as spec;
use templar_proxy_oracle_near_governance_common::GovernancePolicy;

use crate::commands::deploy_common::DeployTargetArgs;
use crate::commands::duration::parse_duration;
use crate::commands::load_json_file;
use crate::commands::signer::{Authorization, SignerArgs};

/// Create (deploy-from-registry) a governance contract, building its
/// `new(proxy_oracle_id, admin_id, policy)` init args from typed flags.
///
/// A governance contract administers exactly one proxy oracle and must be that
/// oracle's owner. Prefer naming it as the oracle's `owner_id` at deploy time;
/// an oracle that is already owned by someone else has to hand ownership over
/// instead (propose-owner to it, then have it execute an `oracle call --method
/// own_accept_owner` proposal).
#[derive(Args, Debug)]
pub struct GovernanceCreate {
    #[command(flatten)]
    target: DeployTargetArgs,
    /// The proxy oracle account this governance contract will administer
    #[arg(long, value_name = "ACCOUNT_ID")]
    proxy_oracle_id: AccountId,
    /// The account granted the Admin role
    #[arg(long, value_name = "ACCOUNT_ID")]
    admin_id: AccountId,
    /// Default proposal TTL applied to every reflexive kind and the target default (e.g. `10s`, `100ns`).
    #[arg(long, value_name = "DURATION", default_value = "0ns", value_parser = parse_duration, conflicts_with = "policy_file")]
    ttl_default: Nanoseconds,
    /// Full `GovernancePolicy` JSON, overriding --ttl-default with an explicit policy table.
    #[arg(long, value_name = "PATH")]
    policy_file: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl GovernanceCreate {
    pub fn try_into_spec(self, authorization: &Authorization) -> anyhow::Result<spec::Create> {
        let policy: GovernancePolicy = match self.policy_file {
            Some(path) => load_json_file(&path).context("parse GovernancePolicy")?,
            None => {
                GovernancePolicy::uniform(self.ttl_default).context("build --ttl-default policy")?
            }
        };

        Ok(spec::Create {
            target: self.target.resolve(|| authorization.public_key())?,
            proxy_oracle_id: self.proxy_oracle_id,
            admin_id: self.admin_id,
            policy,
        })
    }
}
