//! Per-operation signer inputs: the account authorizing every write, and either
//! the credential that signs it or the plan-only output format that replaces it.
//!
//! Credentials resolve to a signer rather than a [`SecretKey`], so a backend
//! holding its key outside this process is expressible.

use std::{ffi::OsStr, fmt};

use anyhow::Context as _;
use clap::{builder::TypedValueParser, error::ErrorKind, Args, ValueEnum};
use near_account_id::AccountId;
use near_api::{NetworkConfig, PublicKey as CliPublicKey, SecretKey, Signer};
use templar_gateway_core::PooledSigner;
use templar_gateway_types::{primitive::PublicKey, ManagedAccountId};

/// Placeholder for the secret key in `Debug` output, so `{:?}` on a command that
/// flattens these args never echoes credentials.
const REDACTED: &str = "<redacted>";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum PrintFormat {
    Json,
    Sputnik,
}

/// Where the signing key lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum SigningBackend {
    /// The key passed as `--secret-key`/`$SECRET_KEY`, held in this process.
    SecretKey,
    /// The OS keychain, looked up by account id.
    Keychain,
}

/// How a write is signed: planned for someone else, or here by a named backend.
///
/// `Debug` is hand-written to redact the key; do not derive it.
#[derive(PartialEq, Eq)]
pub(crate) enum Mode {
    Plan(PrintFormat),
    InProcess(Box<SecretKey>),
    Keychain,
}

impl fmt::Debug for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plan(format) => f.debug_tuple("Plan").field(format).finish(),
            Self::InProcess(_) => f.debug_tuple("InProcess").field(&REDACTED).finish(),
            Self::Keychain => f.write_str("Keychain"),
        }
    }
}

/// The account authorizing a write and how the write is signed, resolved once
/// from [`SignerArgs`] per invocation.
#[derive(Debug)]
pub(crate) struct Authorization {
    account_id: ManagedAccountId,
    /// The key the operator asserted via `--public-key`, unvalidated: only a
    /// resolved signer can check it, so the caller that resolves must.
    asserted_public_key: Option<CliPublicKey>,
    mode: Mode,
}

/// The account authorizing a write and either its execution credential or its
/// plan-only output format.
///
/// `Debug` is hand-written to redact `secret_key`; do not derive it.
#[derive(Args, Clone)]
pub struct SignerArgs {
    /// Account that signs the transaction, or the DAO account that will execute a plan.
    #[arg(long, env = "SIGNER_ID", value_name = "ACCOUNT_ID")]
    signer_id: AccountId,
    /// Backend that signs the transaction. Defaults to `secret-key`.
    #[arg(long, value_enum, value_name = "BACKEND", conflicts_with = "print")]
    sign_with: Option<SigningBackend>,
    /// Private key for `--signer-id`, in `ed25519:…` form.
    ///
    /// Required by the `secret-key` backend, named or defaulted; other backends
    /// and `--print` need no credential.
    ///
    /// Held as text and parsed on use. Clap validates an env value during
    /// parsing whatever else was passed, and `SECRET_KEY` is a name other tools
    /// use — an unrelated ambient value would otherwise fail every write,
    /// including the ones that never touch it.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY",
        required_unless_present_any = ["print", "sign_with"],
        required_if_eq("sign_with", "secret-key"),
    )]
    secret_key: Option<String>,
    /// Plan the write without executing it, then print the selected representation.
    #[arg(long, value_enum, value_name = "FORMAT")]
    print: Option<PrintFormat>,
    /// Public key embedded by deploy/create writes. Deliberately not in conflict
    /// with `--secret-key`: clap's conflicts fire on env values, so an ambient
    /// `SECRET_KEY` would break the documented keychain flow at parse time.
    #[arg(long, value_name = "PUBLIC_KEY")]
    public_key: Option<CliPublicKey>,
}

impl fmt::Debug for SignerArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignerArgs")
            .field("secret_key", &self.secret_key.as_ref().map(|_| REDACTED))
            .field("signer_id", &self.signer_id)
            .field("sign_with", &self.sign_with)
            .field("print", &self.print)
            .field("public_key", &self.public_key)
            .finish()
    }
}

impl SignerArgs {
    /// The signing account.
    pub fn account_id(&self) -> ManagedAccountId {
        ManagedAccountId::from(self.signer_id.clone())
    }

    /// The warning for a credential `mode` will not use.
    fn ignored_credential_warning(&self, mode: &Mode) -> Option<String> {
        let mode = match mode {
            Mode::Plan(_) => "--print only plans the write, so nothing is signed",
            Mode::Keychain => "--sign-with keychain signs with the key the keychain holds",
            Mode::InProcess(_) => return None,
        };
        // Presence only: nothing derived from the key itself may reach a log.
        self.secret_key.is_some().then(|| {
            format!(
                "{mode} for {}; the supplied --secret-key/$SECRET_KEY is ignored.",
                self.signer_id,
            )
        })
    }

    /// The source text never reaches the error.
    fn secret(&self) -> anyhow::Result<SecretKey> {
        self.secret_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing --secret-key"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("--secret-key/$SECRET_KEY is not a valid `ed25519:…` key"))
    }
}

impl TryFrom<&SignerArgs> for Authorization {
    type Error = anyhow::Error;

    /// The one place plan mode and the backend are decided; warns here, once,
    /// about a credential the chosen mode will not use.
    fn try_from(args: &SignerArgs) -> anyhow::Result<Self> {
        let backend = args.sign_with.unwrap_or(SigningBackend::SecretKey);
        let mode = match (args.print, backend) {
            (Some(format), _) => Mode::Plan(format),
            (None, SigningBackend::SecretKey) => Mode::InProcess(Box::new(args.secret()?)),
            (None, SigningBackend::Keychain) => Mode::Keychain,
        };
        if let Some(warning) = args.ignored_credential_warning(&mode) {
            tracing::warn!("{warning}");
        }
        let authorization = Self {
            account_id: args.account_id(),
            asserted_public_key: args.public_key,
            mode,
        };
        if let Mode::InProcess(secret) = &authorization.mode {
            ensure_asserted_key_is_held(
                &authorization.account_id,
                authorization.asserted_public_key,
                secret.public_key(),
            )?;
        }
        Ok(authorization)
    }
}

impl Authorization {
    /// The signing account.
    pub(crate) const fn account_id(&self) -> &ManagedAccountId {
        &self.account_id
    }

    pub(crate) const fn mode(&self) -> &Mode {
        &self.mode
    }

    /// The signer's public key, granted full access on accounts a deploy
    /// creates. Only the in-process backend derives it locally; the others
    /// need `--public-key`.
    pub(crate) fn public_key(&self) -> anyhow::Result<PublicKey> {
        let required_with = match &self.mode {
            Mode::InProcess(secret) => return Ok(PublicKey::from(secret.public_key())),
            Mode::Plan(_) => {
                "--print for writes that embed the signer's key. \
                 --secret-key/$SECRET_KEY cannot stand in: a plan is signed by whoever executes it"
            }
            Mode::Keychain => "--sign-with for writes that embed the signer's key",
        };
        self.asserted_public_key
            .map(PublicKey::from)
            .ok_or_else(|| anyhow::anyhow!("--public-key is required with {required_with}"))
    }

    /// The signing account's key as a gateway nonce lane, and the key that lane
    /// will sign with.
    ///
    /// Async because the keychain backend discovers the account's keys on chain
    /// before matching them against the OS keystore.
    pub(crate) async fn resolve(
        self,
        network: &NetworkConfig,
    ) -> anyhow::Result<(PooledSigner, CliPublicKey)> {
        let Self {
            account_id,
            asserted_public_key,
            mode,
        } = self;
        let signer = match mode {
            Mode::Plan(_) => anyhow::bail!("--print is not supported for this orchestrated write"),
            Mode::InProcess(secret) => {
                Signer::from_secret_key(*secret).context("build a signer from --secret-key")?
            }
            Mode::Keychain => {
                Signer::from_keystore_with_search_for_keys(account_id.0.clone(), network)
                    .await
                    .context("find a usable key for this account in the OS keychain")?
            }
        };

        let public_key = signer
            .get_public_key()
            .await
            .context("ask the signing backend which key it will sign with")?;
        // An external backend can only be checked once it says which key it holds.
        ensure_asserted_key_is_held(&account_id, asserted_public_key, public_key)?;

        // Both backends produce a single-key signer: `Signer::new` seeds its
        // pool with one entry, the keychain's first matching key.
        let pooled = PooledSigner::from_signer(account_id, signer)
            .await
            .context("register the signing key as a gateway nonce lane")?;

        Ok((pooled, public_key))
    }
}

/// A deploy embeds `--public-key` as the full access key on the account it
/// creates, so an asserted key the signer does not hold hands the account to
/// someone else.
fn ensure_asserted_key_is_held(
    account_id: &ManagedAccountId,
    asserted: Option<CliPublicKey>,
    held: CliPublicKey,
) -> anyhow::Result<()> {
    if let Some(asserted) = asserted {
        anyhow::ensure!(
            asserted == held,
            "--public-key is `{asserted}`, but `{}` will sign with `{held}`. \
             A deploy embeds `--public-key` as the full access key on the account \
             it creates, so this would hand control to a key you do not hold. \
             Drop --public-key to use the signing key, or pass the one the backend holds.",
            **account_id,
        );
    }
    Ok(())
}

/// A secret key with no bound account — for teardown flows (e.g. `registry
/// clear-deployments`) that sign many discovered accounts with one authorized
/// key, so there is no single `--signer-id`.
///
/// `Debug` is hand-written to redact `secret_key`; do not derive it.
#[derive(Args, Clone)]
pub struct SecretKeyArgs {
    /// Private key that signs each discovered account, in `ed25519:…` form.
    #[arg(
        long,
        env = "SECRET_KEY",
        hide_env_values = true,
        value_name = "SECRET_KEY",
        value_parser = SecretKeyParser
    )]
    secret_key: Box<SecretKey>,
}

impl fmt::Debug for SecretKeyArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKeyArgs")
            .field("secret_key", &REDACTED)
            .finish()
    }
}

impl SecretKeyArgs {
    /// The parsed secret key.
    pub fn secret(&self) -> SecretKey {
        self.secret_key.as_ref().clone()
    }
}

/// Parse a secret key at the clap boundary without placing its source text in
/// the validation error.
#[derive(Clone)]
struct SecretKeyParser;

impl TypedValueParser for SecretKeyParser {
    type Value = Box<SecretKey>;

    fn parse_ref(
        &self,
        command: &clap::Command,
        _argument: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        value
            .to_str()
            .and_then(|value| value.parse().ok())
            .map(Box::new)
            .ok_or_else(|| {
                clap::Error::raw(ErrorKind::ValueValidation, "invalid --secret-key")
                    .with_cmd(command)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use templar_gateway_client::{Network, NetworkConfigBuilder};

    const SECRET: &str = "ed25519:2vVTQWpoZvYZBS4HYFZtzU2rxpoQSrhyFWdaHLqSdyaEfgjefbSKiFpuVatuRqax3HFvVq2tkkqWH2h7tso2nK8q";

    #[derive(Parser, Debug)]
    struct Harness {
        #[command(flatten)]
        signer: SignerArgs,
    }

    /// The `secret-key` backend never dials out, so resolution stays offline;
    /// this only satisfies the signature.
    fn offline_network() -> NetworkConfig {
        NetworkConfigBuilder::new(Network::Testnet).build()
    }

    /// Built rather than parsed: an ambient `SECRET_KEY` would otherwise decide
    /// the outcome of the cases that supply none.
    fn signer_args(
        signer_id: &str,
        sign_with: Option<SigningBackend>,
        secret_key: Option<&str>,
        print: Option<PrintFormat>,
    ) -> SignerArgs {
        SignerArgs {
            signer_id: signer_id.parse().expect("valid account"),
            sign_with,
            secret_key: secret_key.map(str::to_owned),
            print,
            public_key: None,
        }
    }

    #[test]
    fn debug_redacts_secret_key() {
        let harness = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--secret-key",
            SECRET,
        ])
        .expect("signer args should parse");
        let rendered = format!("{:?}", harness.signer);
        assert!(
            !rendered.contains(SECRET),
            "secret leaked in Debug: {rendered}"
        );
        assert!(
            rendered.contains(REDACTED),
            "no redaction marker: {rendered}"
        );
        // The account id stays visible for diagnostics.
        assert!(
            rendered.contains("signer.testnet"),
            "signer id missing: {rendered}"
        );
    }

    #[test]
    fn authorization_debug_redacts_secret_key() {
        let signer = signer_args("signer.testnet", None, Some(SECRET), None);
        let rendered = format!("{:?}", Authorization::try_from(&signer).expect("resolves"));
        // The rendering is deliberately kept out of the messages: on failure it
        // would be the leaked secret.
        assert!(!rendered.contains(SECRET), "secret leaked in Debug");
        assert!(rendered.contains(REDACTED), "no redaction marker");
    }

    /// `SECRET_KEY` is a name other tools use. Parsing it eagerly meant an
    /// unrelated ambient value failed every write, including the ones that never
    /// read it.
    #[test]
    fn an_ambient_invalid_secret_key_does_not_block_an_external_backend() {
        let harness = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--sign-with",
            "keychain",
            "--secret-key",
            "not-a-near-key",
        ])
        .expect("the keychain backend never reads the secret");

        assert_eq!(
            Authorization::try_from(&harness.signer)
                .expect("the keychain backend must not parse the unused credential")
                .mode(),
            &Mode::Keychain
        );

        let error = harness
            .signer
            .secret()
            .expect_err("but asking for it still fails, and says why")
            .to_string();

        assert!(error.contains("not a valid"), "{error}");
        assert!(
            !error.contains("not-a-near-key"),
            "the source text must not reach the error: {error}"
        );
    }

    /// Print mode wins over `--sign-with`, so accepting both silently ignored
    /// the backend the operator named.
    #[rstest::rstest]
    #[case::keychain("keychain")]
    #[case::secret_key("secret-key")]
    fn a_backend_cannot_be_named_alongside_print(#[case] backend: &str) {
        let error = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--sign-with",
            backend,
            "--print",
            "json",
        ])
        .expect_err("print mode signs nothing, so a backend is a contradiction");

        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[rstest::rstest]
    #[case::plan_ignores_it(Some(PrintFormat::Json), None, Some(SECRET), true)]
    #[case::keychain_ignores_it(None, Some(SigningBackend::Keychain), Some(SECRET), true)]
    #[case::a_signed_write_uses_it(None, None, Some(SECRET), false)]
    #[case::the_named_default_uses_it(None, Some(SigningBackend::SecretKey), Some(SECRET), false)]
    #[case::a_plan_without_one(Some(PrintFormat::Json), None, None, false)]
    fn warns_only_for_a_credential_the_mode_will_not_use(
        #[case] print: Option<PrintFormat>,
        #[case] sign_with: Option<SigningBackend>,
        #[case] secret_key: Option<&str>,
        #[case] warns: bool,
    ) {
        let signer = signer_args("signer.testnet", sign_with, secret_key, print);
        let authorization = Authorization::try_from(&signer).expect("resolves");

        let Some(warning) = signer.ignored_credential_warning(authorization.mode()) else {
            assert!(!warns, "an unused credential must not pass silently");
            return;
        };

        assert!(warns, "the credential this write signs with is not ignored");
        assert!(warning.contains("--secret-key/$SECRET_KEY"), "{warning}");
        assert!(warning.contains("signer.testnet"), "{warning}");
        assert!(
            !warning.contains(SECRET),
            "the warning must not echo the credential"
        );
    }

    #[test]
    fn the_in_process_backend_needs_a_parseable_key() {
        let signer = signer_args("signer.testnet", None, Some("not-a-near-key"), None);

        let error = Authorization::try_from(&signer)
            .expect_err("an in-process write cannot sign with an unusable key")
            .to_string();

        assert!(error.contains("not a valid"), "{error}");
        assert!(!error.contains("not-a-near-key"), "{error}");
    }

    #[tokio::test]
    async fn plan_mode_rejects_credential_resolution() {
        let signer = signer_args("dao.near", None, None, Some(PrintFormat::Sputnik));

        assert_eq!(
            Authorization::try_from(&signer)
                .expect("a plan resolves without a credential")
                .resolve(&offline_network())
                .await
                // `Signer` is not `Debug`, so discard the Ok value before asserting.
                .map(|_| ())
                .expect_err("plan mode has no execution credentials")
                .to_string(),
            "--print is not supported for this orchestrated write"
        );
    }

    #[test]
    fn plan_write_that_embeds_a_key_requires_public_key() {
        let signer = signer_args("dao.near", None, None, Some(PrintFormat::Json));

        assert!(Authorization::try_from(&signer)
            .expect("a plan resolves without a credential")
            .public_key()
            .expect_err("plan must not invent a public key")
            .to_string()
            .contains("--public-key"));
    }

    /// Naming the default backend must change nothing about how the key is
    /// derived.
    #[rstest::rstest]
    #[case::defaulted(None)]
    #[case::named(Some(SigningBackend::SecretKey))]
    fn execution_public_key_is_derived_from_typed_secret(
        #[case] sign_with: Option<SigningBackend>,
    ) {
        let secret_key: SecretKey = SECRET.parse().expect("valid secret");
        let expected = PublicKey::from(secret_key.public_key());
        let signer = signer_args("signer.testnet", sign_with, Some(SECRET), None);

        let derived = Authorization::try_from(&signer)
            .expect("in-process resolves from the credential")
            .public_key()
            .expect("derived public key");
        assert_eq!(derived, expected);
    }

    /// Naming the default backend must change nothing about which key signs.
    #[rstest::rstest]
    #[case::defaulted(None)]
    #[case::named(Some(SigningBackend::SecretKey))]
    #[tokio::test]
    async fn execution_signs_with_the_typed_secret(#[case] sign_with: Option<SigningBackend>) {
        let secret_key: SecretKey = SECRET.parse().expect("valid secret");
        let signer = signer_args("signer.testnet", sign_with, Some(SECRET), None);

        let (pooled, public_key) = Authorization::try_from(&signer)
            .expect("in-process resolves from the credential")
            .resolve(&offline_network())
            .await
            .expect("the in-process backend resolves offline");

        assert_eq!(public_key, secret_key.public_key());
        assert_eq!(*pooled.account_id(), signer.account_id());
    }

    /// The signing key, not the operator's assertion, is what a deploy embeds
    /// as the new account's full access key. Caught before any work is done.
    #[test]
    fn an_asserted_key_the_signer_does_not_hold_is_rejected() {
        let signer = SignerArgs {
            public_key: Some(
                "ed25519:5TMKtTtD5uuMF28ovo7vVge7oAu58eXjySJWTrwcEB5w"
                    .parse()
                    .expect("valid public key"),
            ),
            ..signer_args("signer.testnet", None, Some(SECRET), None)
        };

        let error = Authorization::try_from(&signer)
            .expect_err("a key the signer does not hold must not be embedded")
            .to_string();

        assert!(error.contains("a key you do not hold"), "{error}");
    }

    /// The point of the flag: an operator can sign without a key in the
    /// environment, so naming a backend must lift the `--secret-key` demand.
    #[test]
    fn external_backend_parses_without_a_secret_key() {
        let harness = Harness::try_parse_from([
            "tmplrmgr",
            "--signer-id",
            "signer.testnet",
            "--sign-with",
            "keychain",
        ])
        .expect("external backend should parse with no credential");

        assert_eq!(harness.signer.sign_with, Some(SigningBackend::Keychain));
    }

    /// A key held in the keychain or on a device cannot be derived in-process,
    /// so writes that embed the signer's key must be told what it is.
    #[test]
    fn external_backend_requires_an_explicit_public_key() {
        let signer = signer_args("signer.testnet", Some(SigningBackend::Keychain), None, None);

        assert!(Authorization::try_from(&signer)
            .expect("the keychain backend resolves without a credential")
            .public_key()
            .expect_err("a device key cannot be derived locally")
            .to_string()
            .contains("--public-key"));
    }
}
