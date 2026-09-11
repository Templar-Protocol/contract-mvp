use clap::Args;
use near_account_id::AccountId;
use templar_gateway_methods_spec::proxy_oracle as spec;

use crate::commands::deploy_common::DeployTargetArgs;
use crate::commands::signer::{Authorization, SignerArgs};

/// Deploy a proxy oracle from a registered version, granting the signer a full
/// access key so the operator retains control of the new account.
///
/// `--owner-id` seats the owner at init, so the oracle can be handed straight to
/// its governance contract. `--skip-abi-check` disables the constructor check
/// that normally rejects a target that ignores this argument.
#[derive(Args, Debug)]
pub struct Create {
    #[command(flatten)]
    target: DeployTargetArgs,
    /// Account that will own the oracle. Defaults to the deployer — the registry.
    #[arg(long, value_name = "ACCOUNT_ID")]
    owner_id: Option<AccountId>,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Create {
    pub fn try_into_spec(self, authorization: &Authorization) -> anyhow::Result<spec::Create> {
        Ok(spec::Create {
            target: self.target.resolve(|| authorization.public_key())?,
            owner_id: self.owner_id,
        })
    }
}
