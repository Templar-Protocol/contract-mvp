use std::sync::Mutex;

use clap::{CommandFactory, Parser};

use super::cli::{Cli, Command};
use super::commands::proxy_oracle::{CreateProposal, ProxyOracleGovernanceNs, ProxyOracleNs};
use super::commands::signer::PrintFormat;

static ENV_LOCK: Mutex<()> = Mutex::new(());

mod deploy_script;
mod export;
mod ft;
mod market;
mod oracle;
mod patch;
mod plan;
mod plan_file;
mod proxy_oracle;
mod redstone;
mod registry;
mod spec;
mod storage;

#[test]
fn help_lists_all_top_level_commands() {
    let rendered = Cli::command().render_long_help().to_string();
    for command in [
        "account",
        "contract",
        "registry",
        "storage",
        "ft",
        "market",
        "oracle",
        "proxy-oracle",
        "owner",
        "pyth",
        "redstone",
        "patch",
        "spec",
        "recover-nep141",
        "read",
        "write",
    ] {
        assert!(rendered.contains(command), "help is missing `{command}`");
    }
    assert!(
        Cli::command()
            .find_subcommand("proxy-oracle-governance")
            .is_none(),
        "legacy governance command is still top-level"
    );
    assert!(
        !rendered.contains("proxy-oracle-owner"),
        "help still lists the removed `proxy-oracle-owner` command"
    );
}

#[test]
fn owner_uses_concise_subcommands() {
    let command = Cli::command();
    let owner = command
        .find_subcommand("owner")
        .expect("owner command should exist");
    let names = owner
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        ["get", "get-proposed", "propose", "accept", "renounce"]
    );
    assert!(Cli::try_parse_from(["tmplrmgr", "proxy-oracle-owner"]).is_err());
}

#[test]
fn parses_recover_nep141_args() {
    let cli = Cli::try_parse_from(
        [
            "tmplrmgr",
            "recover-nep141",
            "--token-id",
            "usdt.testnet",
            "--beneficiary-id",
            "treasury.testnet",
            "--force",
        ]
        .into_iter()
        .chain(CREDS),
    )
    .expect("recover-nep141 should parse");
    match cli.command {
        super::cli::Command::RecoverNep141(args) => {
            assert_eq!(args.token_id.as_str(), "usdt.testnet");
            assert_eq!(args.beneficiary_id.as_str(), "treasury.testnet");
            assert!(args.force);
        }
        _ => panic!("expected recover-nep141"),
    }
}

#[test]
fn parses_read_fallback_with_json() {
    let cli = Cli::try_parse_from([
        "tmplrmgr",
        "read",
        "contract.getVersion",
        "--json",
        r#"{"contract_id":"market.testnet"}"#,
    ])
    .expect("read fallback should parse");

    match cli.command {
        super::cli::Command::Read(call) => {
            assert_eq!(call.method, "contract.getVersion");
            assert!(call.json.is_some());
        }
        _ => panic!("expected Read variant"),
    }
}

const TEST_SECRET_KEY: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

/// Signer credentials appended to write-command argv in parse tests, so the
/// structural `SignerArgs` are satisfied. Shared by the submodules.
const CREDS: [&str; 4] = [
    "--signer-id",
    "signer.testnet",
    "--secret-key",
    TEST_SECRET_KEY,
];

fn try_parse_write<'a>(args: impl IntoIterator<Item = &'a str>) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(
        [
            "tmplrmgr",
            "write",
            "registry.removeVersion",
            "--json",
            r#"{"registry_id":"registry.testnet","version_key":"v1"}"#,
        ]
        .into_iter()
        .chain(args),
    )
}

fn try_parse_governance<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> Result<ProxyOracleGovernanceNs, clap::Error> {
    let command = Cli::try_parse_from(
        ["tmplrmgr", "proxy-oracle", "governance"]
            .into_iter()
            .chain(args),
    )?
    .command;
    let Command::ProxyOracle {
        command: ProxyOracleNs::Governance(command),
    } = command
    else {
        unreachable!("governance argv prefix always selects the nested namespace");
    };
    Ok(command)
}

fn parse_governance<'a>(args: impl IntoIterator<Item = &'a str>) -> ProxyOracleGovernanceNs {
    try_parse_governance(args).expect("governance command should parse")
}

fn parse_create_proposal<'a>(args: impl IntoIterator<Item = &'a str>) -> CreateProposal {
    // Credentials belong to `create-proposal` and must precede its operation subcommand.
    let ProxyOracleGovernanceNs::CreateProposal(command) =
        parse_governance(["create-proposal"].into_iter().chain(CREDS).chain(args))
    else {
        panic!("expected create-proposal");
    };
    command
}

#[test]
fn parses_write_fallback_with_json() {
    let cli = try_parse_write(CREDS).expect("write fallback should parse");

    match cli.command {
        super::cli::Command::Write(call) => {
            assert_eq!(call.call.method, "registry.removeVersion");
            assert!(call.call.json.is_some());
            call.signer
                .public_key()
                .expect("credentials should resolve");
        }
        _ => panic!("expected Write variant"),
    }
}

/// `write` flattens the oracle source flags because it may dispatch an `oracle.*`
/// update, but only the dispatched method builds a source. The Lazer token must
/// therefore stay optional, or every `write` — `market.borrow` included — would demand
/// it. Asserted against clap's metadata rather than by parsing: a parse test would pass
/// whenever the developer's environment happens to set `PYTH_LAZER_API_KEY`.
#[test]
fn write_fallback_does_not_require_a_lazer_key() {
    let write = Cli::command()
        .find_subcommand("write")
        .expect("write is a subcommand")
        .clone();
    let api_key = write
        .get_arguments()
        .find(|arg| arg.get_id() == "pyth_lazer_api_key")
        .expect("write flattens the oracle source flags");

    assert!(
        !api_key.is_required_set(),
        "--pyth-lazer-api-key must not be required by `write`"
    );
}

/// ENG-692: clap applies a conflict to an env-sourced value, so an arg that
/// declares both fails whenever the variable happens to be exported. Asserted
/// over clap's metadata, since only the exported shell reproduces it.
#[test]
fn no_argument_pairs_an_env_source_with_a_conflict() {
    fn walk(command: &clap::Command, path: &str) {
        for arg in command.get_arguments() {
            assert!(
                arg.get_env().is_none() || command.get_arg_conflicts_with(arg).is_empty(),
                "`{path}` declares both an env source and a conflict on `{}`",
                arg.get_id()
            );
        }
        for sub in command.get_subcommands() {
            walk(sub, &format!("{path} {}", sub.get_name()));
        }
    }

    walk(&Cli::command(), "tmplrmgr");
}

#[test]
fn write_requires_secret_key_or_print() {
    // Omitting both execution credentials and plan mode is a parse error, so no
    // build or network work is reachable.
    let result = with_cleared_credential_env(|| try_parse_write(["--signer-id", "dao.near"]));
    let error = result.expect_err("a write needs --secret-key or --print");
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}

/// The whole point of `--sign-with`: naming a backend that holds the key
/// elsewhere must satisfy the credential requirement with nothing in the
/// environment. Credentials are cleared so an ambient `SECRET_KEY` cannot make
/// this pass for the wrong reason.
#[test]
fn write_with_an_external_backend_needs_no_secret_key() {
    let result = with_cleared_credential_env(|| {
        try_parse_write(["--signer-id", "dao.near", "--sign-with", "keychain"])
    });

    result.expect("--sign-with keychain should satisfy the credential requirement");
}

/// `--sign-with` names only external backends. `secret-key` is not one, so it
/// cannot be typed — an invocation that would parse while supplying no
/// credential is unrepresentable rather than merely discouraged.
#[test]
fn sign_with_cannot_name_the_in_process_backend() {
    let error = with_cleared_credential_env(|| {
        try_parse_write(["--signer-id", "dao.near", "--sign-with", "secret-key"])
    })
    .expect_err("`secret-key` is not a --sign-with backend");

    assert_eq!(error.kind(), clap::error::ErrorKind::InvalidValue);
}

/// A supplied `--public-key` must never become the full access key on a new
/// account when the signer holds a different secret — that would hand control
/// of the account to a key the operator does not have.
#[test]
fn public_key_cannot_override_the_signing_key() {
    let cli = with_cleared_credential_env(|| {
        try_parse_write([
            "--signer-id",
            "signer.testnet",
            "--secret-key",
            TEST_SECRET_KEY,
            "--public-key",
            "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w",
        ])
    })
    .expect("clap accepts the pair; the conflict is semantic");

    let Command::Write(call) = cli.command else {
        panic!("expected Write variant")
    };
    let error = call
        .signer
        .public_key()
        .expect_err("a contradicting --public-key must not be honored");

    assert!(
        error.to_string().contains("a key you do not hold"),
        "error should say why: {error}"
    );
}

/// An ambient `SECRET_KEY` is extremely common. It must not break the documented
/// `--sign-with keychain --public-key …` flow, whose backend ignores it.
#[test]
fn an_ambient_secret_does_not_block_an_external_backend() {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original = std::env::var_os("SECRET_KEY");
    std::env::set_var("SECRET_KEY", TEST_SECRET_KEY);

    let result = try_parse_write([
        "--signer-id",
        "dao.near",
        "--sign-with",
        "keychain",
        "--public-key",
        "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w",
    ]);

    restore_env("SECRET_KEY", original);
    result.expect("an ignored ambient secret must not fail parsing");
}

#[test]
fn write_command_accepts_print_without_secret() {
    let result = with_cleared_credential_env(|| {
        try_parse_write(["--signer-id", "dao.near", "--print", "sputnik"])
    });
    let cli = result.expect("plan-only write should parse without a secret");
    let Command::Write(call) = cli.command else {
        panic!("expected Write variant");
    };
    assert_eq!(call.signer.print(), Some(PrintFormat::Sputnik));
}

#[test]
fn print_accepts_an_explicit_secret_key() {
    let cli = try_parse_write([
        "--signer-id",
        "dao.near",
        "--print",
        "json",
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect("plan-only write should accept an unused credential");

    let Command::Write(call) = cli.command else {
        panic!("expected Write variant");
    };
    assert_eq!(call.signer.print(), Some(PrintFormat::Json));
}

#[test]
fn print_accepts_a_secret_key_from_environment() {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::set_var("SECRET_KEY", TEST_SECRET_KEY);
    let result = try_parse_write(["--signer-id", "dao.near", "--print", "json"]);
    restore_env("SECRET_KEY", original_secret);

    let cli = result.expect("an ambient secret must not block a plan-only write");
    let Command::Write(call) = cli.command else {
        panic!("expected Write variant");
    };
    assert_eq!(call.signer.print(), Some(PrintFormat::Json));
}

#[test]
fn public_key_is_not_a_credential() {
    let error = with_cleared_credential_env(|| {
        try_parse_write([
            "--signer-id",
            "signer.testnet",
            "--public-key",
            "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w",
        ])
    })
    .expect_err("--public-key names a key, it does not authorize a write");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
    assert!(
        error.to_string().contains("--secret-key"),
        "the error should name the missing credential: {error}"
    );
}

#[test]
fn read_command_rejects_credentials() {
    // Reads don't flatten the signer, so credentials are an unexpected argument.
    let error = Cli::try_parse_from([
        "tmplrmgr",
        "read",
        "account.get",
        "--json",
        r#"{"account_id":"signer.testnet"}"#,
        "--secret-key",
        TEST_SECRET_KEY,
    ])
    .expect_err("credentials on a read should fail to parse");

    assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
}

/// Deliberately not a parse error. Clap validates an env value whatever else was
/// passed, and `SECRET_KEY` is a name other tools use — validating it here failed
/// every write, including `--print` and `--sign-with`, which never read it. It is
/// parsed on use instead; see `commands::signer::tests`.
#[test]
fn an_invalid_secret_key_is_not_a_parse_error() {
    let secret = "not-a-real-secret-key";
    let cli = try_parse_write(["--signer-id", "signer.testnet", "--secret-key", secret])
        .expect("an unusable credential must not fail the parse");

    let rendered = format!("{cli:?}");
    assert!(!rendered.contains(secret), "secret leaked: {rendered}");
}

#[test]
fn signer_env_satisfies_write_credentials() {
    // Scripted/CI usage relies on SIGNER_ID/SECRET_KEY env sourcing satisfying the
    // structural credentials with no explicit flags.
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::set_var("SIGNER_ID", "signer.testnet");
    std::env::set_var("SECRET_KEY", TEST_SECRET_KEY);

    let result = (|| {
        let cli = try_parse_write([]).map_err(|error| anyhow::anyhow!(error.to_string()))?;

        match cli.command {
            super::cli::Command::Write(call) => call.signer.public_key().map(|_| ()),
            _ => anyhow::bail!("expected Write variant"),
        }
    })();

    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);

    result.expect("env-provided credentials should satisfy a write command");
}

/// Run `f` with ambient signer credentials cleared and environment mutation
/// serialized, then restore the original values.
fn with_cleared_credential_env<T>(f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::remove_var("SIGNER_ID");
    std::env::remove_var("SECRET_KEY");
    let result = f();
    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);
    result
}

fn restore_env(key: &str, original: Option<std::ffi::OsString>) {
    match original {
        Some(value) => std::env::set_var(key, value),
        None => std::env::remove_var(key),
    }
}

#[test]
fn read_fallback_rejects_missing_json() {
    let error = Cli::try_parse_from(["tmplrmgr", "read", "account.get"])
        .expect_err("read fallback should require --json or --json-file");

    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::MissingRequiredArgument
    );
}
