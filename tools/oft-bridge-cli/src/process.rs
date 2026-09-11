use std::{fmt, process::Command};

use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Output of an external command with both streams decoded as UTF-8.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
}

/// A single environment variable handed to a subprocess. Values are held
/// zeroized-on-drop and are always rendered redacted in diagnostics.
#[derive(Clone)]
pub struct CommandEnv {
    pub key: &'static str,
    value: Zeroizing<String>,
}

impl CommandEnv {
    pub fn secret(key: &'static str, value: String) -> Self {
        Self {
            key,
            value: Zeroizing::new(value),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for CommandEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}=<redacted>", self.key)
    }
}

/// Injected process boundary. Restricted to official contract builds and the
/// qualified Foundry-keystore signing fallback; never used for transaction-path
/// stdout parsing.
pub trait CommandExecutor {
    /// Runs `program` with `args`. Arguments whose index appears in
    /// `redacted_args` are secrets and are never rendered into error strings.
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> Result<CommandOutput>;
}

pub struct RealExecutor;

impl CommandExecutor for RealExecutor {
    fn run(
        &self,
        program: &str,
        args: &[String],
        redacted_args: &[usize],
        env: &[CommandEnv],
    ) -> Result<CommandOutput> {
        let mut command = Command::new(program);
        command.args(args);
        for var in env {
            command.env(var.key, var.value.as_str());
        }
        let command_display = display_command(program, args, redacted_args, env);
        let output = command
            .output()
            .map_err(|error| Error::Chain(format!("failed to spawn {command_display}: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            return Err(Error::Chain(format!(
                "command failed: {command_display}\nstdout: {stdout}\nstderr: {stderr}"
            )));
        }
        Ok(CommandOutput { stdout, stderr })
    }
}

/// Renders a command line with secret arguments and every environment value
/// redacted, so diagnostics can never leak credentials.
pub fn display_command(
    program: &str,
    args: &[String],
    redacted_args: &[usize],
    env: &[CommandEnv],
) -> String {
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(program.to_string());
    for (index, arg) in args.iter().enumerate() {
        if redacted_args.contains(&index) {
            parts.push("<redacted>".to_string());
        } else {
            parts.push(arg.clone());
        }
    }
    for var in env {
        parts.push(format!("{}=<redacted>", var.key));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoExecutor;

    impl CommandExecutor for EchoExecutor {
        fn run(
            &self,
            _program: &str,
            _args: &[String],
            _redacted_args: &[usize],
            _env: &[CommandEnv],
        ) -> Result<CommandOutput> {
            Ok(CommandOutput {
                stdout: "ok".into(),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn display_redacts_secret_arguments_and_env() {
        let env = vec![CommandEnv::secret("EVM_PASSWORD", "hunter2".into())];
        let rendered = display_command(
            "forge",
            &["build".to_string(), "hunter2".to_string()],
            &[1],
            &env,
        );
        assert_eq!(rendered, "forge build <redacted> EVM_PASSWORD=<redacted>");
        assert!(!rendered.contains("hunter2"));
        assert!(!format!("{env:?}").contains("hunter2"));
        let _ = EchoExecutor;
    }

    #[test]
    fn real_executor_maps_failure_to_chain_error() {
        let error = RealExecutor
            .run("false", &[], &[], &[])
            .expect_err("false always fails");
        assert!(matches!(error, Error::Chain(_)));
        assert!(error.to_string().contains("command failed"));
    }

    #[test]
    fn real_executor_success_round_trip() {
        let output = RealExecutor
            .run("echo", &["-n".to_string(), "ok".to_string()], &[], &[])
            .expect("echo succeeds");
        assert_eq!(output.stdout, "ok");
    }
}
