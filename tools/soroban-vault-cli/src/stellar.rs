use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use ed25519_dalek::SigningKey;
use ed25519_dalek_bip32::{ChildIndex, ExtendedSigningKey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use stellar_strkey::Strkey;
use tracing::{debug, info, warn};
use zeroize::Zeroize;
use zeroize::Zeroizing;

use crate::cli::Cli;
use crate::types::SourceAccount;

const SUBMITTED_TX_CONFIRMATION_TIMEOUT_SECONDS: u64 = 300;
const SUBMITTED_TX_CONFIRMATION_POLL_SECONDS: u64 = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub struct CommandEnv {
    key: &'static str,
    value: Zeroizing<String>,
    redact: bool,
}

impl CommandEnv {
    fn redacted(key: &'static str, value: String) -> Self {
        Self {
            key,
            value: Zeroizing::new(value),
            redact: true,
        }
    }
}

pub trait CommandExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> anyhow::Result<CommandOutput>;
}

pub struct RealExecutor;

impl CommandExecutor for RealExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> anyhow::Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        for var in env {
            command.env(var.key, var.value.as_str());
        }
        let command_display = display_command(program, args, redacted_args, env);
        debug!(command = %command_display, "running external command");
        let output = command
            .output()
            .with_context(|| format!("run {command_display}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if output.status.success() {
            debug!(command = %command_display, "external command completed");
        } else {
            warn!(
                command = %command_display,
                status = ?output.status.code(),
                "external command failed"
            );
        }
        anyhow::ensure!(
            output.status.success(),
            "command failed: {command_display}\nstdout: {stdout}\nstderr: {stderr}"
        );
        Ok(CommandOutput { stdout, stderr })
    }
}

pub struct Stellar<'a, E: CommandExecutor> {
    cli: &'a Cli,
    executor: &'a E,
}

impl<'a, E: CommandExecutor> Stellar<'a, E> {
    pub fn new(cli: &'a Cli, executor: &'a E) -> Self {
        Self { cli, executor }
    }

    pub fn run(
        &self,
        mut args: Vec<String>,
        redacted_args: &[usize],
        mut env: Vec<CommandEnv>,
    ) -> anyhow::Result<CommandOutput> {
        let confirm_transaction = should_confirm_transaction(&args);
        let preflight = preflight_plan(&args).is_some();
        let command_display = display_command("stellar", &args, redacted_args, &env);
        debug!(
            command = %command_display,
            confirm_transaction,
            preflight,
            dry_run = self.cli.dry_run,
            "preparing stellar command"
        );
        let result = if self.cli.dry_run {
            eprintln!("dry-run: {command_display}");
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        } else {
            if preflight {
                self.preflight_command(&args, redacted_args, &env)?;
            }
            self.executor.run("stellar", &args, redacted_args, &env)
        };
        let result = if confirm_transaction && !self.cli.dry_run {
            self.confirm_transaction_result(result)
        } else {
            result
        };
        zeroize_redacted_args(&mut args, redacted_args);
        zeroize_env(&mut env);
        result
    }

    pub fn network_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        args.extend(["--network".to_string(), self.cli.network.clone()]);
        args.extend([
            "--network-passphrase".to_string(),
            self.cli.network_passphrase.clone(),
        ]);
        if let Some(rpc_url) = &self.cli.rpc_url {
            args.extend(["--rpc-url".to_string(), rpc_url.clone()]);
        }
        if let Some(config_dir) = &self.cli.config_dir {
            args.extend(["--config-dir".to_string(), config_dir.display().to_string()]);
        }
        args
    }

    pub fn source_env(&self) -> Vec<CommandEnv> {
        self.cli
            .source_account
            .as_ref()
            .map(|source| CommandEnv::redacted("STELLAR_ACCOUNT", source.clone_secret()))
            .into_iter()
            .collect()
    }

    /// Resolves the configured signing identity to its public Stellar account address.
    pub fn source_public_address(&self) -> anyhow::Result<String> {
        let stellar_account = if self.cli.source_account.is_none() {
            match std::env::var("STELLAR_ACCOUNT") {
                Ok(value) => Some(Zeroizing::new(value)),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    anyhow::bail!("STELLAR_ACCOUNT must be valid UTF-8")
                }
            }
        } else {
            None
        };
        if let Some(account) = stellar_account.as_ref() {
            if let Some(address) = public_address_from_stellar_identity(account.as_str())? {
                return Ok(address);
            }
        }
        let source = self
            .cli
            .source_account
            .as_ref()
            .map(SourceAccount::as_secret_str)
            .or_else(|| stellar_account.as_ref().map(|account| account.as_str()));
        let (args, redacted_args) =
            keys_address_source_account_args(source, self.cli.config_dir.as_deref());
        let out = self.run(args, &redacted_args, Vec::new())?;
        if self.cli.dry_run {
            return Ok("GDRYRUNSOURCEACCOUNT".to_string());
        }
        anyhow::ensure!(
            !out.stdout.is_empty(),
            "stellar keys address returned no address"
        );
        Ok(out.stdout)
    }

    /// Backward-compatible alias for [`Self::source_public_address`].
    pub fn keys_address_source_account(&self) -> anyhow::Result<String> {
        self.source_public_address()
    }

    pub fn invoke(
        &self,
        contract_id: &str,
        function: &str,
        function_args: Vec<String>,
    ) -> anyhow::Result<CommandOutput> {
        let mut args = vec!["contract".to_string(), "invoke".to_string()];
        args.extend(["--id".to_string(), contract_id.to_string()]);
        args.extend(self.network_args());
        args.push("--".to_string());
        args.push(function.to_string());
        args.extend(function_args);
        self.run(args, &[], self.source_env())
    }

    pub fn invoke_view(
        &self,
        contract_id: &str,
        function: &str,
        function_args: Vec<String>,
    ) -> anyhow::Result<CommandOutput> {
        let mut args = vec!["contract".to_string(), "invoke".to_string()];
        args.extend(["--id".to_string(), contract_id.to_string()]);
        args.extend(["--send".to_string(), "no".to_string()]);
        args.extend(self.network_args());
        args.push("--".to_string());
        args.push(function.to_string());
        args.extend(function_args);
        self.run(args, &[], self.source_env())
    }

    pub fn extend_contract_instance_ttl(
        &self,
        contract_id: &str,
        ledgers_to_extend: u32,
    ) -> anyhow::Result<CommandOutput> {
        self.extend_ttl_entry("--id", contract_id, ledgers_to_extend)
    }

    pub fn extend_contract_code_ttl(
        &self,
        wasm_hash: &str,
        ledgers_to_extend: u32,
    ) -> anyhow::Result<CommandOutput> {
        self.extend_ttl_entry("--wasm-hash", wasm_hash, ledgers_to_extend)
    }

    fn extend_ttl_entry(
        &self,
        selector: &str,
        value: &str,
        ledgers_to_extend: u32,
    ) -> anyhow::Result<CommandOutput> {
        let mut args = vec!["contract".to_string(), "extend".to_string()];
        args.extend([selector.to_string(), value.to_string()]);
        args.extend([
            "--ledgers-to-extend".to_string(),
            ledgers_to_extend.to_string(),
        ]);
        args.extend(self.network_args());
        self.run(args, &[], self.source_env())
    }

    pub fn deploy(&self, wasm_hash: &str, constructor_args: Vec<String>) -> anyhow::Result<String> {
        let mut args = vec!["contract".to_string(), "deploy".to_string()];
        args.extend(["--wasm-hash".to_string(), wasm_hash.to_string()]);
        args.extend(self.network_args());
        if !constructor_args.is_empty() {
            args.push("--".to_string());
            args.extend(constructor_args);
        }
        let out = self.run(args, &[], self.source_env())?;
        if self.cli.dry_run {
            return Ok(format!("CDRYRUN{}", &wasm_hash[..8.min(wasm_hash.len())]));
        }
        parse_contract_id(&out.stdout)
    }

    pub fn upload(&self, wasm_path: &str) -> anyhow::Result<String> {
        anyhow::ensure!(
            !self.cli.dry_run,
            "internal error: upload called during dry-run planning"
        );
        let mut args = vec!["contract".to_string(), "upload".to_string()];
        args.extend(["--wasm".to_string(), wasm_path.to_string()]);
        args.extend(self.network_args());
        let out = self.run(args, &[], self.source_env())?;
        parse_hash(&out.stdout)
    }

    pub fn fetch_wasm_hash(&self, wasm_hash: &str) -> anyhow::Result<bool> {
        let mut args = vec!["contract".to_string(), "fetch".to_string()];
        args.extend(["--wasm-hash".to_string(), wasm_hash.to_string()]);
        args.extend(self.network_args());
        Ok(self.run(args, &[], Vec::new()).is_ok())
    }

    pub fn fetch_contract_wasm_hash(&self, contract_id: &str) -> anyhow::Result<String> {
        anyhow::ensure!(
            !self.cli.dry_run,
            "internal error: contract WASM hash fetch called during dry-run planning"
        );
        let out_file = temp_wasm_path(contract_id);
        let mut args = vec!["contract".to_string(), "fetch".to_string()];
        args.extend(["--id".to_string(), contract_id.to_string()]);
        args.extend(["--out-file".to_string(), out_file.display().to_string()]);
        args.extend(self.network_args());
        let result = self.run(args, &[], Vec::new());
        result?;
        let bytes = fs::read(&out_file)
            .with_context(|| format!("read fetched contract wasm {}", out_file.display()))?;
        if let Err(error) = fs::remove_file(&out_file) {
            warn!(
                path = %out_file.display(),
                error = %error,
                "failed to remove fetched contract wasm temp file"
            );
        }
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub fn deploy_native_asset(&self) -> anyhow::Result<()> {
        let mut args = vec![
            "contract".to_string(),
            "asset".to_string(),
            "deploy".to_string(),
            "--asset".to_string(),
            "native".to_string(),
        ];
        args.extend(self.network_args());
        let _ = self.run(args, &[], self.source_env())?;
        Ok(())
    }

    pub fn asset_contract_id(&self, asset: &str) -> anyhow::Result<String> {
        let mut args = vec![
            "contract".to_string(),
            "id".to_string(),
            "asset".to_string(),
            "--asset".to_string(),
            asset.to_string(),
        ];
        args.extend(self.network_args());
        let out = self.run(args, &[], Vec::new())?;
        if self.cli.dry_run {
            return Ok(if asset == "native" {
                "CDRYRUNNATIVEASSET".to_string()
            } else {
                "CDRYRUNASSETCONTRACT".to_string()
            });
        }
        parse_contract_id(&out.stdout)
    }

    pub fn native_asset_id(&self) -> anyhow::Result<String> {
        self.asset_contract_id("native")
    }

    pub fn contract_interface_functions(
        &self,
        contract_id: &str,
    ) -> anyhow::Result<BTreeSet<String>> {
        let mut args = vec![
            "contract".to_string(),
            "info".to_string(),
            "interface".to_string(),
            "--contract-id".to_string(),
            contract_id.to_string(),
            "--output".to_string(),
            "json".to_string(),
        ];
        args.extend(self.network_args());
        let out = self.run(args, &[], Vec::new())?;
        contract_interface_function_names(&out.stdout)
    }

    pub fn build_package(
        &self,
        workspace_path: &str,
        package: &str,
        out_dir: &str,
    ) -> anyhow::Result<()> {
        let mut args = vec![
            "contract".to_string(),
            "build".to_string(),
            "--manifest-path".to_string(),
            format!("{workspace_path}/Cargo.toml"),
            "--package".to_string(),
            package.to_string(),
            "--optimize".to_string(),
        ];
        if let Some(source_repo) = self
            .cli
            .contract_source_repo
            .as_deref()
            .map(str::trim)
            .filter(|source_repo| !source_repo.is_empty())
        {
            args.extend(["--meta".to_string(), format!("source_repo={source_repo}")]);
        }
        args.extend(["--out-dir".to_string(), out_dir.to_string()]);
        let _ = self.run(args, &[], Vec::new())?;
        Ok(())
    }

    fn confirm_transaction_result(
        &self,
        result: anyhow::Result<CommandOutput>,
    ) -> anyhow::Result<CommandOutput> {
        match result {
            Ok(mut output) => {
                if let Some(hash) = first_tx_hash(&output.stdout, &output.stderr) {
                    info!(tx_hash = %hash, "waiting for submitted transaction confirmation");
                    self.wait_for_transaction_success(&hash)?;
                    append_reconciled_tx_hash(&mut output, &hash);
                }
                Ok(output)
            }
            Err(error) => {
                let message = error.to_string();
                let Some(hash) = first_tx_hash(&message, "") else {
                    return Err(error);
                };
                match self.wait_for_transaction_success(&hash) {
                    Ok(()) => {
                        warn!(
                            tx_hash = %hash,
                            "stellar command errored after submission, but RPC confirmed success"
                        );
                        Ok(CommandOutput {
                            stdout: format!("tx hash: {hash}"),
                            stderr: format!(
                                "stellar command returned an error, but RPC confirmed transaction success: {error}"
                            ),
                        })
                    }
                    Err(wait_error) => Err(error).with_context(|| {
                        format!("could not confirm submitted transaction {hash}: {wait_error}")
                    }),
                }
            }
        }
    }

    fn preflight_command(
        &self,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> anyhow::Result<()> {
        let Some(plan) = preflight_plan(args) else {
            return Ok(());
        };
        let (command_display, output) = match plan {
            PreflightPlan::Invoke(preflight_args) => {
                let command_display =
                    display_command("stellar", &preflight_args, redacted_args, env);
                info!(command = %command_display, "running stellar preflight simulation");
                let output = self
                    .executor
                    .run("stellar", &preflight_args, redacted_args, env)
                    .with_context(|| {
                        format!("preflight simulation failed for {command_display}")
                    })?;
                (command_display, output)
            }
            PreflightPlan::BuildAndSimulate(build_args) => {
                let build_display = display_command("stellar", &build_args, redacted_args, env);
                info!(command = %build_display, "building stellar transaction for preflight simulation");
                let xdr = self
                    .executor
                    .run("stellar", &build_args, redacted_args, env)
                    .with_context(|| format!("preflight build-only failed for {build_display}"))?
                    .stdout
                    .trim()
                    .to_string();
                anyhow::ensure!(
                    !xdr.is_empty(),
                    "preflight build-only produced empty transaction XDR for {build_display}"
                );
                with_preflight_xdr_file(&xdr, |xdr_path| {
                    let mut simulate_args = vec![
                        "tx".to_string(),
                        "simulate".to_string(),
                        xdr_path.display().to_string(),
                    ];
                    simulate_args.extend(self.network_args());
                    let simulate_display = display_command("stellar", &simulate_args, &[], env);
                    info!(command = %simulate_display, "running stellar preflight simulation");
                    let output = self
                        .executor
                        .run("stellar", &simulate_args, &[], env)
                        .with_context(|| {
                            format!("preflight simulation failed for {simulate_display}")
                        })?;
                    Ok((simulate_display, output))
                })?
            }
        };
        eprintln!("Preflight simulation succeeded: {command_display}");
        if !output.stdout.is_empty() {
            eprintln!("{}", output.stdout);
        }
        if !output.stderr.is_empty() {
            eprintln!("{}", output.stderr);
        }
        Ok(())
    }

    fn wait_for_transaction_success(&self, tx_hash: &str) -> anyhow::Result<()> {
        let started = Instant::now();
        let timeout = Duration::from_secs(SUBMITTED_TX_CONFIRMATION_TIMEOUT_SECONDS);
        let poll = Duration::from_secs(SUBMITTED_TX_CONFIRMATION_POLL_SECONDS);
        let mut last_status = "not_found".to_string();
        let mut last_error = None;

        while started.elapsed() < timeout {
            match self.fetch_transaction_status(tx_hash) {
                Ok(TransactionConfirmationStatus::Success) => {
                    info!(tx_hash, "transaction confirmed");
                    return Ok(());
                }
                Ok(TransactionConfirmationStatus::Failed) => {
                    warn!(tx_hash, "transaction failed after submission");
                    anyhow::bail!("transaction {tx_hash} failed after submission")
                }
                Ok(TransactionConfirmationStatus::NotFound) => {
                    debug!(tx_hash, "transaction not found yet");
                    last_status = "not_found".to_string();
                }
                Err(error) => {
                    debug!(tx_hash, error = %error, "transaction fetch failed while polling");
                    last_error = Some(error.to_string());
                }
            }
            thread::sleep(poll);
        }

        if let Some(error) = last_error {
            anyhow::bail!(
                "transaction {tx_hash} was not confirmed before timeout; last status: {last_status}; last error: {error}"
            );
        }
        anyhow::bail!(
            "transaction {tx_hash} was not confirmed before timeout; last status: {last_status}"
        )
    }

    fn fetch_transaction_status(
        &self,
        tx_hash: &str,
    ) -> anyhow::Result<TransactionConfirmationStatus> {
        let mut args = vec![
            "tx".to_string(),
            "fetch".to_string(),
            "result".to_string(),
            "--hash".to_string(),
            tx_hash.to_string(),
            "--output".to_string(),
            "json".to_string(),
        ];
        args.extend(self.network_args());
        match self.executor.run("stellar", &args, &[], &[]) {
            Ok(output) => transaction_status_from_output(&output.stdout).with_context(|| {
                format!(
                    "could not parse transaction status for {tx_hash}; output: {}",
                    output_excerpt(&output.stdout)
                )
            }),
            Err(error) => {
                let message = error.to_string();
                if looks_not_found(&message) {
                    Ok(TransactionConfirmationStatus::NotFound)
                } else {
                    Err(error).with_context(|| format!("fetch transaction {tx_hash}"))
                }
            }
        }
    }
}

pub fn parse_contract_id(stdout: &str) -> anyhow::Result<String> {
    stdout
        .split_whitespace()
        .rev()
        .find(|token| token.starts_with('C') && token.len() >= 56)
        .map(ToString::to_string)
        .context("no contract id found in stellar output")
}

pub fn parse_hash(stdout: &str) -> anyhow::Result<String> {
    stdout
        .split_whitespace()
        .rev()
        .map(|token| token.trim_start_matches("0x"))
        .find(|token| token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_lowercase)
        .context("no wasm hash found in stellar output")
}

fn append_reconciled_tx_hash(output: &mut CommandOutput, tx_hash: &str) {
    if parse_tx_hashes(&output.stdout)
        .into_iter()
        .chain(parse_tx_hashes(&output.stderr))
        .any(|hash| hash == tx_hash)
    {
        return;
    }
    if output.stdout.is_empty() {
        output.stdout = format!("tx hash: {tx_hash}");
    } else {
        let _ = write!(output.stdout, "\ntx hash: {tx_hash}");
    }
}

fn should_confirm_transaction(args: &[String]) -> bool {
    match args {
        [first, second, ..] if first == "tx" && second == "send" => true,
        [first, second, ..]
            if first == "contract"
                && matches!(second.as_str(), "deploy" | "extend" | "invoke" | "upload") =>
        {
            true
        }
        [first, second, third, ..]
            if first == "contract" && second == "asset" && third == "deploy" =>
        {
            true
        }
        _ => false,
    }
}

fn contract_interface_function_names(stdout: &str) -> anyhow::Result<BTreeSet<String>> {
    let entries: Vec<Value> =
        serde_json::from_str(stdout).context("parse Stellar contract interface JSON")?;
    let functions = entries
        .iter()
        .filter_map(|entry| entry.get("function_v0"))
        .filter_map(|function| function.get("name"))
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    anyhow::ensure!(
        !functions.is_empty(),
        "Stellar contract interface did not contain any function names"
    );
    Ok(functions)
}

enum PreflightPlan {
    Invoke(Vec<String>),
    BuildAndSimulate(Vec<String>),
}

fn preflight_plan(args: &[String]) -> Option<PreflightPlan> {
    match args {
        [first, second, ..] if first == "contract" && second == "invoke" => {
            invoke_preflight_args(args).map(PreflightPlan::Invoke)
        }
        // Stellar CLI 26 rejects `contract deploy --wasm-hash ... --build-only`
        // even though the real deploy path accepts the hash and simulates before signing.
        [first, second, ..] if first == "contract" && second == "deploy" => None,
        [first, second, ..]
            if first == "contract" && matches!(second.as_str(), "extend" | "upload") =>
        {
            build_only_preflight_args(args).map(PreflightPlan::BuildAndSimulate)
        }
        [first, second, third, ..]
            if first == "contract" && second == "asset" && third == "deploy" =>
        {
            build_only_preflight_args(args).map(PreflightPlan::BuildAndSimulate)
        }
        _ => None,
    }
}

fn invoke_preflight_args(args: &[String]) -> Option<Vec<String>> {
    if args
        .windows(2)
        .any(|pair| pair[0] == "--send" && pair[1] == "no")
    {
        return None;
    }
    let separator = args.iter().position(|arg| arg == "--")?;
    let mut preflight = Vec::with_capacity(args.len() + 2);
    preflight.extend_from_slice(&args[..separator]);
    preflight.extend(["--send".to_string(), "no".to_string()]);
    preflight.extend_from_slice(&args[separator..]);
    Some(preflight)
}

fn build_only_preflight_args(args: &[String]) -> Option<Vec<String>> {
    if args.iter().any(|arg| arg == "--build-only") {
        return None;
    }
    let mut preflight = Vec::with_capacity(args.len() + 1);
    if let Some(separator) = args.iter().position(|arg| arg == "--") {
        preflight.extend_from_slice(&args[..separator]);
        preflight.push("--build-only".to_string());
        preflight.extend_from_slice(&args[separator..]);
    } else {
        preflight.extend_from_slice(args);
        preflight.push("--build-only".to_string());
    }
    Some(preflight)
}

fn temp_wasm_path(contract_id: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "tmplr-soroban-vault-cli-{}-{nanos}-{contract_id}.wasm",
        std::process::id()
    ))
}

struct TemporaryFileCleanup {
    path: PathBuf,
    active: bool,
}

impl TemporaryFileCleanup {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            active: false,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn activate(&mut self) {
        self.active = true;
    }
}

impl Drop for TemporaryFileCleanup {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Err(error) = fs::remove_file(&self.path) {
            warn!(
                path = %self.path.display(),
                error = %error,
                "failed to remove preflight transaction file"
            );
        }
    }
}

fn with_preflight_xdr_file<T>(
    xdr: &str,
    operation: impl FnOnce(&Path) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut cleanup = TemporaryFileCleanup::new(std::env::temp_dir().join(format!(
        "tmplr-soroban-vault-cli-{}-{nanos}.tx.xdr",
        std::process::id()
    )));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(cleanup.path()).with_context(|| {
        format!(
            "create preflight transaction file {}",
            cleanup.path().display()
        )
    })?;
    cleanup.activate();
    if let Err(error) = file.write_all(xdr.as_bytes()) {
        return Err(error).with_context(|| {
            format!(
                "write preflight transaction file {}",
                cleanup.path().display()
            )
        });
    }
    drop(file);

    operation(cleanup.path())
}

fn first_tx_hash(stdout: &str, stderr: &str) -> Option<String> {
    parse_labeled_tx_hashes(stdout)
        .into_iter()
        .chain(parse_labeled_tx_hashes(stderr))
        .next()
}

pub(crate) fn parse_labeled_tx_hashes(value: &str) -> Vec<String> {
    const MARKERS: [&str; 6] = [
        "signing transaction:",
        "transaction hash:",
        "tx hash:",
        "transaction submitted successfully:",
        "transaction submitted:",
        "/tx/",
    ];

    value
        .lines()
        .flat_map(|line| {
            let normalized = line.to_ascii_lowercase();
            MARKERS
                .iter()
                .filter_map(move |marker| normalized.find(marker).map(|index| index + marker.len()))
                .flat_map(move |start| parse_tx_hashes(&line[start..]))
        })
        .collect()
}

fn parse_tx_hashes(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_ascii_hexdigit())
        .filter(|token| token.len() == 64)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn output_excerpt(value: &str) -> String {
    const MAX_CHARS: usize = 240;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    let mut chars = trimmed.chars();
    let mut excerpt = String::new();
    for _ in 0..MAX_CHARS {
        let Some(ch) = chars.next() else {
            return excerpt;
        };
        excerpt.push(ch);
    }
    if chars.next().is_some() {
        excerpt.push_str("...");
    }
    excerpt
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransactionConfirmationStatus {
    Success,
    Failed,
    NotFound,
}

fn transaction_status_from_output(output: &str) -> Option<TransactionConfirmationStatus> {
    serde_json::from_str::<Value>(output)
        .ok()
        .and_then(|value| find_transaction_status(&value))
        .or_else(|| transaction_status_from_text(output))
}

fn find_transaction_status(value: &Value) -> Option<TransactionConfirmationStatus> {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if let Some(status) = transaction_status_from_result_variant(key) {
                    return Some(status);
                }
                if key.eq_ignore_ascii_case("status") {
                    if let Some(status) = value.as_str().and_then(transaction_status_from_text) {
                        return Some(status);
                    }
                }
                if let Some(status) = find_transaction_status(value) {
                    return Some(status);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_transaction_status),
        Value::String(text) => transaction_status_from_text(text),
        _ => None,
    }
}

fn transaction_status_from_result_variant(key: &str) -> Option<TransactionConfirmationStatus> {
    let normalized = key.to_ascii_lowercase();
    match normalized.as_str() {
        "tx_success" | "tx_fee_bump_inner_success" => Some(TransactionConfirmationStatus::Success),
        "tx_fee_bump_inner_failed"
        | "tx_failed"
        | "tx_too_early"
        | "tx_too_late"
        | "tx_missing_operation"
        | "tx_bad_seq"
        | "tx_bad_auth"
        | "tx_insufficient_balance"
        | "tx_no_account"
        | "tx_insufficient_fee"
        | "tx_bad_auth_extra"
        | "tx_internal_error"
        | "tx_not_supported"
        | "tx_bad_sponsorship"
        | "tx_bad_min_seq_age_or_gap"
        | "tx_malformed"
        | "tx_soroban_invalid"
        | "tx_frozen_key_accessed" => Some(TransactionConfirmationStatus::Failed),
        _ => None,
    }
}

fn transaction_status_from_text(text: &str) -> Option<TransactionConfirmationStatus> {
    let normalized = text.to_ascii_uppercase();
    if normalized.contains("NOT_FOUND") || normalized.contains("NOT FOUND") {
        Some(TransactionConfirmationStatus::NotFound)
    } else if normalized.contains("SUCCESS") {
        Some(TransactionConfirmationStatus::Success)
    } else if normalized.contains("FAILED") || normalized.contains("ERROR") {
        Some(TransactionConfirmationStatus::Failed)
    } else {
        None
    }
}

fn looks_not_found(message: &str) -> bool {
    transaction_status_from_text(message) == Some(TransactionConfirmationStatus::NotFound)
}

pub fn display_command(
    program: &str,
    args: &[String],
    redacted_args: &[usize],
    env: &[CommandEnv],
) -> String {
    let redacted_args = redacted_args.iter().copied().collect::<BTreeSet<_>>();
    env.iter()
        .map(|var| {
            let value = if var.redact {
                "<redacted>".to_string()
            } else {
                shell_escape(var.value.as_str())
            };
            format!("{}={value}", var.key)
        })
        .chain(std::iter::once(program.to_string()))
        .chain(args.iter().enumerate().map(|(index, arg)| {
            if redacted_args.contains(&index) {
                "<redacted>".to_string()
            } else {
                shell_escape(arg)
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

fn zeroize_redacted_args(args: &mut [String], redacted_args: &[usize]) {
    for index in redacted_args {
        if let Some(arg) = args.get_mut(*index) {
            arg.zeroize();
        }
    }
}

fn zeroize_env(env: &mut [CommandEnv]) {
    for var in env {
        if var.redact {
            var.value.zeroize();
        }
    }
}

fn shell_escape(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=,@".contains(c))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(crate) fn keys_address_source_account_args(
    source_account: Option<&str>,
    config_dir: Option<&Path>,
) -> (Vec<String>, Vec<usize>) {
    let mut args = vec!["keys".to_string(), "address".to_string()];
    let mut redacted_args = Vec::new();
    if let Some(source) = source_account {
        redacted_args.push(args.len());
        args.push(source.to_string());
    }
    if let Some(config_dir) = config_dir {
        args.extend(["--config-dir".to_string(), config_dir.display().to_string()]);
    }
    (args, redacted_args)
}

fn public_address_from_stellar_identity(value: &str) -> anyhow::Result<Option<String>> {
    match Strkey::from_string(value) {
        Ok(Strkey::PrivateKeyEd25519(mut private_key)) => {
            let signing_key = SigningKey::from_bytes(&private_key.0);
            private_key.0.zeroize();
            Ok(Some(stellar_public_key(
                signing_key.verifying_key().to_bytes(),
            )))
        }
        Ok(Strkey::PublicKeyEd25519(public_key)) => Ok(Some(public_key.to_string().to_string())),
        Ok(Strkey::MuxedAccountEd25519(muxed_account)) => {
            Ok(Some(stellar_public_key(muxed_account.ed25519)))
        }
        Ok(_) => anyhow::bail!("STELLAR_ACCOUNT is not a supported account source"),
        Err(_) if value.split_whitespace().count() > 1 => {
            let mnemonic = bip39::Mnemonic::parse(value)
                .context("STELLAR_ACCOUNT contains an invalid seed phrase")?;
            let seed = Zeroizing::new(mnemonic.to_seed(""));
            let mut root = ExtendedSigningKey::from_seed(seed.as_ref())
                .context("derive STELLAR_ACCOUNT seed phrase root key")?;
            let path = [
                ChildIndex::Hardened(44),
                ChildIndex::Hardened(148),
                ChildIndex::Hardened(0),
            ];
            let mut derived = root
                .derive(&path)
                .context("derive STELLAR_ACCOUNT default Stellar account")?;
            root.chain_code.zeroize();
            drop(root);
            let public_key = derived.verifying_key().to_bytes();
            derived.chain_code.zeroize();
            drop(derived);
            Ok(Some(stellar_public_key(public_key)))
        }
        Err(_) => {
            anyhow::ensure!(
                !value.is_empty()
                    && value.len() < 56
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()
                            || "-_.".contains(character)),
                "STELLAR_ACCOUNT is neither a valid account key nor a safe Stellar identity name"
            );
            Ok(None)
        }
    }
}

fn stellar_public_key(bytes: [u8; 32]) -> String {
    stellar_strkey::ed25519::PublicKey(bytes)
        .to_string()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_contract_id_from_noisy_output() {
        let id =
            parse_contract_id("logs\nCDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX")
                .expect("parse id");
        assert_eq!(
            id,
            "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX"
        );
    }

    #[test]
    fn parses_hash_from_output() {
        let hash = parse_hash(
            "installed 0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .expect("parse hash");
        assert_eq!(
            hash,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn parses_contract_interface_function_names() {
        let functions = contract_interface_function_names(
            r#"[{"function_v0":{"name":"deposit"}},{"function_v0":{"name":"atomic_withdraw"}},{"udt_error_enum_v0":{"name":"ContractError"}}]"#,
        )
        .expect("parse interface functions");

        assert_eq!(
            functions,
            BTreeSet::from(["atomic_withdraw".to_string(), "deposit".to_string()])
        );
    }

    #[test]
    fn display_command_redacts_sensitive_arguments() {
        let args = vec![
            "contract".to_string(),
            "invoke".to_string(),
            "--source-account".to_string(),
            "SAUCE SECRET SEED".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
        ];

        let display = display_command("stellar", &args, &[3], &[]);

        assert!(display.contains("--source-account <redacted>"));
        assert!(!display.contains("SAUCE SECRET SEED"));
    }

    #[test]
    fn display_command_redacts_sensitive_environment() {
        let display = display_command(
            "stellar",
            &["contract".to_string(), "invoke".to_string()],
            &[],
            &[CommandEnv::redacted(
                "STELLAR_ACCOUNT",
                "SAUCE SECRET SEED".to_string(),
            )],
        );

        assert!(display.contains("STELLAR_ACCOUNT=<redacted>"));
        assert!(!display.contains("SAUCE SECRET SEED"));
    }

    #[test]
    fn derives_env_secret_for_source_address_without_argv() {
        let address = public_address_from_stellar_identity(
            "SBU2RRGLXH3E5CQHTD3ODLDF2BWDCYUSSBLLZ5GNW7JXHDIYKXZWHOKR",
        )
        .expect("derive secret key")
        .expect("public address");

        assert_eq!(
            address,
            "GA3D5KRYM6CB7OWQ6TWYRR3Z4T7GNZLKERYNZGGA5SOAOPIFY6YQHES5"
        );
    }

    #[test]
    fn derives_env_seed_phrase_for_source_address_without_argv() {
        let address = public_address_from_stellar_identity(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("derive seed phrase")
        .expect("public address");

        assert_eq!(
            address,
            "GB3JDWCQJCWMJ3IILWIGDTQJJC5567PGVEVXSCVPEQOTDN64VJBDQBYX"
        );
    }

    #[test]
    fn derives_underlying_account_from_env_muxed_address() {
        let address = public_address_from_stellar_identity(
            "MA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJUAAAAAAAAAAAACJUQ",
        )
        .expect("derive muxed account")
        .expect("public address");

        assert_eq!(
            address,
            "GA7QYNF7SOWQ3GLR2BGMZEHXAVIRZA4KVWLTJJFC7MGXUA74P7UJVSGZ"
        );
    }

    #[test]
    fn accepts_env_keystore_identity_for_redacted_lookup() {
        assert_eq!(
            public_address_from_stellar_identity("operator").expect("accept identity name"),
            None
        );
        let (args, redacted_args) = keys_address_source_account_args(Some("operator"), None);
        assert_eq!(args, vec!["keys", "address", "operator"]);
        assert_eq!(redacted_args, vec![2]);
    }

    #[test]
    fn detects_write_commands_that_can_emit_transaction_hashes() {
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "invoke".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "deploy".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "extend".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "asset".to_string(),
            "deploy".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "tx".to_string(),
            "send".to_string()
        ]));
        assert!(should_confirm_transaction(&[
            "contract".to_string(),
            "upload".to_string()
        ]));
        assert!(!should_confirm_transaction(&[
            "contract".to_string(),
            "fetch".to_string()
        ]));
    }

    #[test]
    fn builds_preflight_args_for_contract_invokes() {
        let args = vec![
            "contract".to_string(),
            "invoke".to_string(),
            "--id".to_string(),
            "CCONTRACT".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
            "--".to_string(),
            "initialize".to_string(),
        ];

        let preflight = invoke_preflight_args(&args).expect("preflight args");

        assert_eq!(
            preflight,
            vec![
                "contract",
                "invoke",
                "--id",
                "CCONTRACT",
                "--network",
                "testnet",
                "--send",
                "no",
                "--",
                "initialize"
            ]
        );
    }

    #[test]
    fn builds_preflight_args_for_contract_ttl_extension() {
        let args = vec![
            "contract".to_string(),
            "extend".to_string(),
            "--id".to_string(),
            "CCONTRACT".to_string(),
            "--ledgers-to-extend".to_string(),
            "3110400".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
        ];

        let Some(PreflightPlan::BuildAndSimulate(preflight)) = preflight_plan(&args) else {
            panic!("expected build-and-simulate preflight plan");
        };

        assert_eq!(
            preflight,
            vec![
                "contract",
                "extend",
                "--id",
                "CCONTRACT",
                "--ledgers-to-extend",
                "3110400",
                "--network",
                "testnet",
                "--build-only"
            ]
        );
    }

    #[test]
    fn skips_preflight_for_view_invokes() {
        let args = vec![
            "contract".to_string(),
            "invoke".to_string(),
            "--id".to_string(),
            "CCONTRACT".to_string(),
            "--send".to_string(),
            "no".to_string(),
            "--".to_string(),
            "vault".to_string(),
        ];

        assert_eq!(invoke_preflight_args(&args), None);
    }

    #[test]
    fn builds_build_only_preflight_args_for_contract_deploys() {
        let args = vec![
            "contract".to_string(),
            "deploy".to_string(),
            "--wasm-hash".to_string(),
            "abc".to_string(),
            "--network".to_string(),
            "testnet".to_string(),
            "--".to_string(),
            "--admin".to_string(),
            "GADMIN".to_string(),
        ];

        let preflight = build_only_preflight_args(&args).expect("preflight args");

        assert_eq!(
            preflight,
            vec![
                "contract",
                "deploy",
                "--wasm-hash",
                "abc",
                "--network",
                "testnet",
                "--build-only",
                "--",
                "--admin",
                "GADMIN"
            ]
        );
        assert!(preflight_plan(&args).is_none());
    }

    #[test]
    fn builds_build_only_preflight_args_for_uploads_and_asset_deploys() {
        let upload = vec![
            "contract".to_string(),
            "upload".to_string(),
            "--wasm".to_string(),
            "contract.wasm".to_string(),
        ];
        assert_eq!(
            build_only_preflight_args(&upload).expect("upload preflight args"),
            vec![
                "contract",
                "upload",
                "--wasm",
                "contract.wasm",
                "--build-only"
            ]
        );

        let asset_deploy = vec![
            "contract".to_string(),
            "asset".to_string(),
            "deploy".to_string(),
            "--asset".to_string(),
            "native".to_string(),
        ];
        assert_eq!(
            build_only_preflight_args(&asset_deploy).expect("asset deploy preflight args"),
            vec![
                "contract",
                "asset",
                "deploy",
                "--asset",
                "native",
                "--build-only"
            ]
        );
    }

    #[test]
    fn stages_large_preflight_transactions_in_a_temporary_file() {
        let xdr = "A".repeat(200_000);
        let mut staged_path = None;

        with_preflight_xdr_file(&xdr, |path| {
            staged_path = Some(path.to_path_buf());
            assert_eq!(fs::read_to_string(path).expect("read staged XDR"), xdr);
            Ok(())
        })
        .expect("stage preflight XDR");

        assert!(!staged_path.expect("staged path").exists());
    }

    #[test]
    fn removes_preflight_transaction_file_when_operation_panics() {
        let staged_path = std::sync::Mutex::new(None);

        let panic = std::panic::catch_unwind(|| {
            with_preflight_xdr_file("AAAA", |path| -> anyhow::Result<()> {
                *staged_path.lock().expect("lock staged path") = Some(path.to_path_buf());
                panic!("forced operation panic");
            })
            .expect("operation should panic before returning");
        });

        assert!(panic.is_err());
        assert!(!staged_path
            .into_inner()
            .expect("staged path mutex")
            .expect("staged path")
            .exists());
    }

    #[test]
    fn inactive_temporary_file_cleanup_preserves_unowned_file() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("existing.tx.xdr");
        fs::write(&path, "existing").expect("write existing file");

        drop(TemporaryFileCleanup::new(path.clone()));

        assert_eq!(
            fs::read_to_string(path).expect("read existing file"),
            "existing"
        );
    }

    #[test]
    fn parses_transaction_hashes_from_command_text() {
        let hashes = parse_tx_hashes(
            "tx hash: 0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
        );

        assert_eq!(
            hashes,
            vec!["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"]
        );
    }

    #[test]
    fn selects_labeled_transaction_hash_over_wasm_hash() {
        let stderr = "ℹ️  Deploying contract using wasm hash 4d24790f3ea2a02e521b84d583dab00bfa246cdfd06ee858f1f656a831cccc83\nℹ️  Signing transaction: 56d4f7c5f5391c6520834c6a53a66a991e786f1693bad90a6c360f07b5386084\n🔗 https://stellar.expert/explorer/testnet/tx/56d4f7c5f5391c6520834c6a53a66a991e786f1693bad90a6c360f07b5386084";

        assert_eq!(
            first_tx_hash(
                "CAUOEITD3BZ6VW6TFQAGI2VWIBXG5YRJL6VOL6V4AWETXUCHX2M3YXSZ",
                stderr
            ),
            Some("56d4f7c5f5391c6520834c6a53a66a991e786f1693bad90a6c360f07b5386084".to_string())
        );
    }

    #[test]
    fn parses_transaction_status_from_json() {
        let status = transaction_status_from_output(r#"{"status":"SUCCESS"}"#);
        assert_eq!(status, Some(TransactionConfirmationStatus::Success));

        let status = transaction_status_from_output(r#"{"result":{"status":"FAILED"}}"#);
        assert_eq!(status, Some(TransactionConfirmationStatus::Failed));

        let status = transaction_status_from_output(
            r#"{"fee_charged":"23048","result":{"tx_success":[{"op_inner":{"invoke_host_function":{"success":"00"}}}]},"ext":"v0"}"#,
        );
        assert_eq!(status, Some(TransactionConfirmationStatus::Success));

        let status = transaction_status_from_output(
            r#"{"fee_charged":"113610","result":{"tx_failed":[{"op_inner":{"invoke_host_function":"trapped"}}]},"ext":"v0"}"#,
        );
        assert_eq!(status, Some(TransactionConfirmationStatus::Failed));

        let status = transaction_status_from_output(
            r#"{"a":{"tx_hash":"0123456789abcdef"},"z":{"tx_success":[]}}"#,
        );
        assert_eq!(status, Some(TransactionConfirmationStatus::Success));
        assert_eq!(transaction_status_from_result_variant("tx_hash"), None);
        assert_eq!(transaction_status_from_result_variant("tx_envelope"), None);

        let status = transaction_status_from_text(
            "command failed: transaction 00 not found on testnet network",
        );
        assert_eq!(status, Some(TransactionConfirmationStatus::NotFound));
    }

    #[test]
    fn output_excerpt_is_bounded_and_marks_empty_output() {
        assert_eq!(output_excerpt(" \n "), "<empty>");
        assert!(output_excerpt(&"x".repeat(400)).len() < 260);
        assert!(output_excerpt(&"x".repeat(400)).ends_with("..."));
    }

    #[test]
    fn appends_reconciled_tx_hash_when_send_output_has_no_hash() {
        let mut output = CommandOutput {
            stdout: "submitted".to_string(),
            stderr: String::new(),
        };

        append_reconciled_tx_hash(
            &mut output,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );

        assert!(output
            .stdout
            .contains("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"));
    }
}
