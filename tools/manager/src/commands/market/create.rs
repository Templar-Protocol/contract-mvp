use anyhow::Context as _;
use clap::Args;
use serde::Deserialize;
use templar_common::market::MarketConfiguration;
use templar_gateway_methods_spec::market as spec;

use crate::commands::deploy_common::DeployTargetArgs;
use crate::commands::signer::{Authorization, SignerArgs};

/// Deploy a market from a registered version, granting the signer a full access
/// key so the operator retains control of the new account.
#[derive(Args, Debug)]
pub struct Create {
    #[command(flatten)]
    target: DeployTargetArgs,
    /// JSON file with the market init args (`{"configuration": ...}`).
    #[arg(long, value_name = "PATH")]
    init_args_file: std::path::PathBuf,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarketInitArgs {
    configuration: MarketConfiguration,
}

impl Create {
    pub fn try_into_spec(self, authorization: &Authorization) -> anyhow::Result<spec::Create> {
        let file = std::fs::File::open(&self.init_args_file).with_context(|| {
            format!(
                "open market init args from {}",
                self.init_args_file.display()
            )
        })?;
        let init_args: MarketInitArgs =
            serde_json::from_reader(file).context("parse market init args")?;

        Ok(spec::Create {
            target: self.target.resolve(|| authorization.public_key())?,
            configuration: init_args.configuration,
        })
    }
}
