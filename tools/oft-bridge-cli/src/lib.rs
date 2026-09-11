pub mod artifacts;
pub mod canary;
pub mod cli;
pub mod codec;
pub mod config;
pub mod deployment;
pub mod domain;
pub mod environment;
pub mod error;
pub mod evm;
pub mod governance;
pub mod health;
pub mod layerzero;
pub mod output;
pub mod process;
pub mod reconcile;
pub mod route;
pub mod scan;
pub mod state;
pub mod stellar;
pub mod ttl;
pub mod wrap;

use std::future::Future;
use std::process::ExitCode;
use std::sync::LazyLock;

use clap::{error::ErrorKind, Parser as _};

use crate::{cli::Cli, error::Error};
pub fn main_entry() -> ExitCode {
    let json_requested = std::env::args_os().any(|argument| argument == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(parse_error)
            if matches!(
                parse_error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = parse_error.print();
            return ExitCode::SUCCESS;
        }
        Err(parse_error) if json_requested => {
            let error = Error::InvalidInput(parse_error.to_string());
            if let Err(output_error) = output::failure("parse", "local_read", &error) {
                eprintln!("failed to emit error envelope: {output_error}");
            }
            return error.exit_code();
        }
        Err(parse_error) => {
            let _ = parse_error.print();
            return ExitCode::from(2);
        }
    };
    let command = cli.command_name();
    let effect = cli.effect();
    match cli.run() {
        Ok(data) => match output::success(&command, effect, data) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => error.exit_code(),
        },
        Err(error) => {
            if let Err(output_error) = output::failure(&command, effect, &error) {
                eprintln!("failed to emit error envelope: {output_error}");
            }
            error.exit_code()
        }
    }
}

/// Runs one adapter future on the shared crate runtime. Only adapter reads
/// use this; no command holds a runtime across user interaction.
pub(crate) fn block_on<F: Future>(future: F) -> error::Result<F::Output> {
    static RUNTIME: LazyLock<error::Result<tokio::runtime::Runtime>> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|error| error::Error::Chain(format!("tokio runtime unavailable: {error}")))
    });
    match &*RUNTIME {
        Ok(runtime) => Ok(runtime.block_on(future)),
        Err(_) => Err(error::Error::Chain("tokio runtime unavailable".into())),
    }
}

/// Runs an adapter future whose output is already a crate [`Result`],
/// flattening the nested boundary in one place.
pub(crate) fn block_on_result<T, F>(future: F) -> error::Result<T>
where
    F: Future<Output = error::Result<T>>,
{
    block_on(future).and_then(|inner| inner)
}

/// Canonical JSON bytes for hashing boundaries.
pub(crate) fn canonical_bytes<T: serde::Serialize>(value: &T) -> error::Result<Vec<u8>> {
    Ok(serde_json_canonicalizer::to_vec(value)?)
}

pub fn canonical_sha256<T: serde::Serialize>(value: &T) -> error::Result<String> {
    use sha2::{Digest as _, Sha256};
    let bytes = serde_json_canonicalizer::to_vec(value)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Current Unix time in seconds, refusing a system clock before the epoch.
pub(crate) fn now_unix() -> error::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            crate::error::Error::Custody(format!("system clock precedes Unix epoch: {error}"))
        })?
        .as_secs())
}

pub fn policy_error(message: impl Into<String>) -> Error {
    Error::Policy(message.into())
}
