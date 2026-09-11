use std::sync::Mutex;

use clap::{error::ErrorKind, CommandFactory, Parser};

use super::cli::{Cli, Command};
use super::commands::proxy_oracle::{CreateProposal, ProxyOracleGovernanceNs, ProxyOracleNs};
use super::commands::signer::{Authorization, Mode, PrintFormat, SignerArgs};

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

/// The authorization the parsed credentials select; a parse test's inputs
/// are valid by construction.
fn authorized(signer: &SignerArgs) -> Authorization {
    Authorization::try_from(signer).expect("parsed credentials resolve")
}

/// Clap's metadata for one of `write`'s flags, by field id.
fn write_arg(id: &str) -> clap::Arg {
    Cli::command()
        .find_subcommand("write")
        .expect("write is a subcommand")
        .get_arguments()
        .find(|arg| arg.get_id() == id)
        .unwrap_or_else(|| panic!("write has no `{id}` flag"))
        .clone()
}

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
            authorized(&call.signer)
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
    assert!(
        !write_arg("pyth_lazer_api_key").is_required_set(),
        "--pyth-lazer-api-key must not be required by `write`"
    );
}

/// ENG-692: clap applies a conflict to an env-sourced value, so an argument
/// that ends up on either side of one fails whenever the variable happens to be
/// exported. Both endpoints are checked, since the conflict binds the pair no
/// matter which of them declares it, as does a group that admits only one
/// member. `overrides_with` has no public getter and is not covered.
#[test]
fn no_argument_pairs_an_env_source_with_a_conflict() {
    fn walk(command: &clap::Command, path: &str) {
        let from_env: Vec<&clap::Id> = command
            .get_arguments()
            .filter(|arg| arg.get_env().is_some())
            .map(clap::Arg::get_id)
            .collect();

        for arg in command.get_arguments() {
            for conflict in command.get_arg_conflicts_with(arg) {
                assert!(
                    arg.get_env().is_none() && !from_env.contains(&conflict.get_id()),
                    "`{path}` conflicts `{}` with `{}`, and one of them reads an env var",
                    arg.get_id(),
                    conflict.get_id(),
                );
            }
        }

        for group in command.get_groups() {
            if group.clone().is_multiple() {
                continue;
            }
            for member in group.get_args() {
                assert!(
                    !from_env.contains(&member),
                    "`{path}` puts the env-sourced `{member}` in the exclusive group `{}`",
                    group.get_id(),
                );
            }
        }

        for sub in command.get_subcommands() {
            walk(sub, &format!("{path} {}", sub.get_name()));
        }
    }

    walk(&Cli::command(), "tmplrmgr");
}

fn in_process() -> Mode {
    Mode::InProcess(Box::new(TEST_SECRET_KEY.parse().expect("valid secret")))
}

/// Every way a write can be authorized, by flags and by an ambient `SECRET_KEY`.
/// The environment is set explicitly per row: an ambient value would otherwise
/// decide the rows that supply none.
#[rstest::rstest]
#[case::nothing(&[], None, Err(ErrorKind::MissingRequiredArgument))]
#[case::the_default_backend_named_without_a_key(
    &["--sign-with", "secret-key"], None, Err(ErrorKind::MissingRequiredArgument)
)]
#[case::secret_key_flag(&["--secret-key", TEST_SECRET_KEY], None, Ok(in_process()))]
#[case::secret_key_env(&[], Some(TEST_SECRET_KEY), Ok(in_process()))]
#[case::the_default_backend_named_with_a_flag(
    &["--sign-with", "secret-key", "--secret-key", TEST_SECRET_KEY], None, Ok(in_process())
)]
#[case::the_default_backend_named_with_env(
    &["--sign-with", "secret-key"], Some(TEST_SECRET_KEY), Ok(in_process())
)]
#[case::print_alone(&["--print", "json"], None, Ok(Mode::Plan(PrintFormat::Json)))]
#[case::print_with_an_unused_flag(
    &["--print", "sputnik", "--secret-key", TEST_SECRET_KEY], None, Ok(Mode::Plan(PrintFormat::Sputnik))
)]
// ENG-692: an ambient `SECRET_KEY` is extremely common and must not block a plan.
#[case::print_with_an_unused_env(
    &["--print", "json"], Some(TEST_SECRET_KEY), Ok(Mode::Plan(PrintFormat::Json))
)]
#[case::keychain_alone(&["--sign-with", "keychain"], None, Ok(Mode::Keychain))]
#[case::keychain_with_an_unused_env(
    &["--sign-with", "keychain"], Some(TEST_SECRET_KEY), Ok(Mode::Keychain)
)]
#[case::keychain_with_print(
    &["--sign-with", "keychain", "--print", "json"], None, Err(ErrorKind::ArgumentConflict)
)]
#[case::the_default_backend_with_print(
    &["--sign-with", "secret-key", "--print", "json"], None, Err(ErrorKind::ArgumentConflict)
)]
fn write_authorization_matrix(
    #[case] flags: &[&str],
    #[case] ambient_secret: Option<&str>,
    #[case] expected: Result<Mode, ErrorKind>,
) {
    let result = with_credential_env(ambient_secret, || {
        try_parse_write(
            ["--signer-id", "dao.near"]
                .into_iter()
                .chain(flags.iter().copied()),
        )
    });

    let actual = result.map(|cli| {
        let Command::Write(call) = cli.command else {
            panic!("expected Write variant");
        };
        authorized(&call.signer)
    });
    assert_eq!(
        actual
            .as_ref()
            .map(Authorization::mode)
            .map_err(clap::Error::kind),
        expected.as_ref().map_err(|kind| *kind),
        "{actual:?}"
    );
}

#[test]
fn help_lists_every_signing_backend() {
    let backends = write_arg("sign_with")
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_owned())
        .collect::<Vec<_>>();

    assert_eq!(backends, ["secret-key", "keychain"]);
}

/// A supplied `--public-key` must never become the full access key on a new
/// account when the signer holds a different secret — that would hand control
/// of the account to a key the operator does not have.
#[test]
fn public_key_cannot_override_the_signing_key() {
    let cli = with_credential_env(None, || {
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
    let error = authorized(&call.signer)
        .public_key()
        .expect_err("a contradicting --public-key must not be honored");

    assert!(
        error.to_string().contains("a key you do not hold"),
        "error should say why: {error}"
    );
}

#[test]
fn public_key_is_not_a_credential() {
    let error = with_credential_env(None, || {
        try_parse_write([
            "--signer-id",
            "signer.testnet",
            "--public-key",
            "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w",
        ])
    })
    .expect_err("--public-key names a key, it does not authorize a write");

    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
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

    assert_eq!(error.kind(), ErrorKind::UnknownArgument);
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
            super::cli::Command::Write(call) => Authorization::try_from(&call.signer)?
                .public_key()
                .map(|_| ()),
            _ => anyhow::bail!("expected Write variant"),
        }
    })();

    restore_env("SIGNER_ID", original_signer);
    restore_env("SECRET_KEY", original_secret);

    result.expect("env-provided credentials should satisfy a write command");
}

/// Run `f` with `SIGNER_ID` cleared and `SECRET_KEY` set to exactly `secret`,
/// environment mutation serialized, then restore the original values.
fn with_credential_env<T>(secret: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().expect("env lock should not be poisoned");
    let original_signer = std::env::var_os("SIGNER_ID");
    let original_secret = std::env::var_os("SECRET_KEY");
    std::env::remove_var("SIGNER_ID");
    match secret {
        Some(secret) => std::env::set_var("SECRET_KEY", secret),
        None => std::env::remove_var("SECRET_KEY"),
    }
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

    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}
