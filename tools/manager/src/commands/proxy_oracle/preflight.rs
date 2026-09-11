use clap::{ArgGroup, Args};
use near_account_id::AccountId;

use crate::commands::signer::Mode;
use crate::resolve::OracleTarget;

/// Read-only pre-upgrade checks over a deployed proxy oracle's stored state.
///
/// A wider target group than [`OracleTarget`]: `--registry-id` sweeps every proxy oracle a registry
/// has deployed, which is how the fleet is checked before anyone composes an upgrade.
#[derive(Args, Debug)]
#[command(group(ArgGroup::new("preflight_target").required(true).args(["oracle_id", "market_id", "registry_id"])))]
pub struct Preflight {
    /// Proxy-oracle account to check.
    #[arg(long, value_name = "ACCOUNT_ID")]
    oracle_id: Option<AccountId>,
    /// Market whose configured proxy oracle is checked.
    #[arg(long, value_name = "ACCOUNT_ID")]
    market_id: Option<AccountId>,
    /// Check every proxy oracle this registry has deployed.
    #[arg(long, value_name = "ACCOUNT_ID")]
    pub(crate) registry_id: Option<AccountId>,
    /// Check id to skip, repeatable (e.g. `upgrade.pending_rearm`).
    #[arg(long, value_name = "CHECK_ID")]
    pub(crate) skip_check: Vec<String>,
}

impl Preflight {
    /// The single-oracle target, for the paths `--registry-id` does not cover.
    pub(crate) fn target(&self) -> OracleTarget {
        OracleTarget::from_parts(self.oracle_id.clone(), self.market_id.clone())
    }
}

/// The pre-upgrade preflight's opt-out, flattened into every command that puts new oracle code
/// on chain.
#[derive(Args, Debug, Clone)]
pub struct PreflightArgs {
    /// Proceed without checking the oracle's stored state against what the new code requires.
    #[arg(long)]
    pub skip_preflight: bool,
    /// Preflight check id to skip, repeatable (e.g. `upgrade.pending_rearm`).
    #[arg(long, value_name = "CHECK_ID")]
    pub skip_check: Vec<String>,
}

impl PreflightArgs {
    /// Whether the preflight should run at all. A plan builds a payload without submitting, so it
    /// stays offline.
    pub(crate) fn runs(&self, mode: &Mode) -> bool {
        !self.skip_preflight && !matches!(mode, Mode::Plan(_))
    }
}
