use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, ValueEnum};
use near_account_id::AccountId;
use templar_common::oracle::redstone::{config, Config};
use templar_gateway_methods_spec::redstone as spec;

use crate::commands::deploy_common::DeployTargetArgs;
use crate::commands::load_json_file;
use crate::commands::signer::{Authorization, SignerArgs};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Preset {
    Prod,
    Test,
}

/// Deploy a RedStone price adapter from a registered version, granting the
/// signer a full access key so the operator retains control of the new account.
///
/// The configuration is typed either way: `--preset` names a built-in table and
/// `--config-file` is parsed into it. Init args this tool cannot model belong to
/// `registry deploy` instead.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("redstone_config")
        .args(["preset", "config_file"])
        .required(true)
        .multiple(false)
))]
pub struct Create {
    #[command(flatten)]
    target: DeployTargetArgs,
    /// Built-in RedStone configuration to use.
    #[arg(long, value_enum)]
    preset: Option<Preset>,
    /// JSON file with the adapter's `Config` (signers, thresholds, timestamp bounds).
    #[arg(long, value_name = "PATH")]
    config_file: Option<PathBuf>,
    /// Account to seed with the adapter's administration roles.
    #[arg(long, value_name = "ACCOUNT_ID")]
    admin_id: AccountId,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl Create {
    pub fn try_into_spec(self, authorization: &Authorization) -> anyhow::Result<spec::Create> {
        let config: Config = match self.preset {
            Some(Preset::Prod) => config::prod(),
            Some(Preset::Test) => config::test(),
            None => {
                let path = self
                    .config_file
                    .as_deref()
                    .context("provide --preset or --config-file")?;
                load_json_file(path).context("parse RedStone config")?
            }
        };

        Ok(spec::Create {
            target: self.target.resolve(|| authorization.public_key())?,
            config,
            admin_id: self.admin_id,
        })
    }
}
