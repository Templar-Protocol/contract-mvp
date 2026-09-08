use clap::Args;
use near_account_id::AccountId;
use templar_gateway_oracle_updates_dispatch::LazerSourceArgs;
use templar_gateway_oracle_updates_spec::oracle as spec;

use crate::commands::signer::SignerArgs;

#[derive(Args, Debug)]
pub struct UpdateLazer {
    /// Pyth Lazer adapter account to update.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: AccountId,
    /// Pyth Lazer feed ids to fetch and update; repeat the flag per feed.
    /// All feeds are covered by a single payload and written in one call.
    #[arg(long = "feed-id", value_name = "FEED_ID", required = true)]
    feed_ids: Vec<u32>,
    #[command(flatten)]
    pub(crate) sources: LazerSourceArgs,
    #[command(flatten)]
    pub(crate) signer: SignerArgs,
}

impl UpdateLazer {
    pub fn into_spec(self) -> spec::UpdateLazer {
        spec::UpdateLazer {
            oracle_id: self.oracle_id,
            feed_ids: self.feed_ids,
        }
    }
}
