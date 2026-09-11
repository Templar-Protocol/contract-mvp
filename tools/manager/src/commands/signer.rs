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

/// A backend that holds the signing key *outside* this process.
///
/// The in-process path is deliberately not a variant: naming it would let
/// `--sign-with secret-key` satisfy clap's "a backend was chosen" while
/// supplying no key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum SigningBackend {
    /// The OS keychain, looked up by account id.
    Keychain,
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
    /// Hold the signing key outside this process. Omit to use `--secret-key`.
    #[arg(long, value_enum, value_name = "BACKEND", conflicts_with = "print")]
    sign_with: Option<SigningBackend>,
    /// Private key for `--signer-id`, in `ed25519:…` form.
    ///
    /// Required only for the default `secret-key` backend: naming any
    /// `--sign-with` backend lifts the requirement, and `--print` needs no
    /// credential at all.
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
        // `--sign-with` names only external backends, so its presence always
        // means some other credential source was chosen.
        required_unless_present_any = ["print", "sign_with"],
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

    /// The key the operator *asserted* via `--public-key`, unvalidated: only a
    /// resolved signer can check it, so the caller that resolves must.
    pub const fn asserted_public_key(&self) -> Option<CliPublicKey> {
        self.public_key
    }

    /// The requested plan-only output format.
    pub const fn print(&self) -> Option<PrintFormat> {
        self.print
    }

    /// The warning for a credential the selected mode will not use.
    pub(crate) fn ignored_credential_warning(&self) -> Option<String> {
        let mode = match (self.print, self.sign_with) {
            (Some(_), _) => "--print only plans the write, so nothing is signed",
            (None, Some(SigningBackend::Keychain)) => {
                "--sign-with keychain signs with the key the keychain holds"
            }
            (None, None) => return None,
        };
        // Presence only: nothing derived from the key itself may reach a log.
        self.secret_key.is_some().then(|| {
            format!(
                "{mode} for {}; the supplied --secret-key/$SECRET_KEY is ignored.",
                self.signer_id,
            )
        })
    }

    /// The signer's public key, granted full access on accounts a deploy
    /// creates. Only the in-process backend derives it locally.
    pub fn public_key(&self) -> anyhow::Result<PublicKey> {
        // The in-process backend derives from the key it will actually sign
        // with. Returning a supplied `--public-key` here would grant a full
        // access key on the new account to a key the signer does not hold,
        // while the transaction is signed by the secret — so a contradicting
        // value is an error, not an override.
        if self.sign_with.is_none() && self.print.is_none() {
            let derived = PublicKey::from(self.secret()?.public_key());
            if let Some(supplied) = &self.public_key {
                anyhow::ensure!(
                    PublicKey::from(*supplied) == derived,
                    "--public-key names a different key than --secret-key signs with. \
                     The new account would grant full access to a key you do not hold; \
                     drop --public-key to use the signer's own."
                );
            }
            return Ok(derived);
        }

        if let Some(public_key) = &self.public_key {
            return Ok(PublicKey::from(*public_key));
        }
        if self.print.is_some() {
            anyhow::bail!(
                "--public-key is required with --print for writes that embed the signer's key. \
                 --secret-key/$SECRET_KEY cannot stand in: a plan is signed by whoever executes it."
            );
        }
        if self.sign_with.is_some() {
            anyhow::bail!(
                "--public-key is required with --sign-with for writes that embed the signer's key"
            );
        }
        Ok(PublicKey::from(self.secret()?.public_key()))
    }

    /// The signing account's key as a gateway nonce lane, and the key that lane
    /// will sign with.
    ///
    /// Async because the keychain backend discovers the account's keys on chain
    /// before matching them against the OS keystore.
    pub async fn resolve(
        &self,
        network: &NetworkConfig,
    ) -> anyhow::Result<(PooledSigner, CliPublicKey)> {
        if self.print.is_some() {
            anyhow::bail!("--print is not supported for this orchestrated write");
        }
        if let Some(warning) = self.ignored_credential_warning() {
            tracing::warn!("{warning}");
        }

        let signer = match self.sign_with {
            None => Signer::from_secret_key(self.secret()?.clone())
                .context("build a signer from --secret-key")?,
            Some(SigningBackend::Keychain) => {
                Signer::from_keystore_with_search_for_keys(self.signer_id.clone(), network)
                    .await
                    .context("find a usable key for this account in the OS keychain")?
            }
        };

        let public_key = signer
            .get_public_key()
            .await
            .context("ask the signing backend which key it will sign with")?;

        // Both backends produce a single-key signer: `Signer::new` seeds its
        // pool with one entry, the keychain's first matching key.
        let pooled = PooledSigner::from_signer(self.account_id(), signer)
            .await
            .context("register the signing key as a gateway nonce lane")?;

        Ok((pooled, public_key))
    }

    /// Both callers reject print mode before reaching here, so this only has to
    /// account for a credential that clap left absent.
    ///
    /// The source text never reaches the error.
    fn secret(&self) -> anyhow::Result<SecretKey> {
        self.secret_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing --secret-key"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("--secret-key/$SECRET_KEY is not a valid `ed25519:…` key"))
    }
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

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        signer: SignerArgs,
    }

    /// The `secret-key` backend never dials out, so resolution stays offline;
    /// this only satisfies the signature.
    fn offline_network() -> NetworkConfig {
        NetworkConfigBuilder::new(Network::Testnet).build()
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
    #[test]
    fn a_backend_cannot_be_named_alongside_print() {
        assert!(
            Harness::try_parse_from([
                "tmplrmgr",
                "--signer-id",
                "signer.testnet",
                "--sign-with",
                "keychain",
                "--print",
                "json",
            ])
            .is_err(),
            "print mode signs nothing, so a backend is a contradiction"
        );
    }

    /// Built rather than parsed: an ambient `SECRET_KEY` would otherwise decide
    /// the outcome of the cases that supply none.
    #[rstest::rstest]
    #[case::plan_ignores_it(Some(PrintFormat::Json), None, Some(SECRET), true)]
    #[case::keychain_ignores_it(None, Some(SigningBackend::Keychain), Some(SECRET), true)]
    #[case::a_signed_write_uses_it(None, None, Some(SECRET), false)]
    #[case::a_plan_without_one(Some(PrintFormat::Json), None, None, false)]
    fn warns_only_for_a_credential_the_mode_will_not_use(
        #[case] print: Option<PrintFormat>,
        #[case] sign_with: Option<SigningBackend>,
        #[case] secret_key: Option<&str>,
        #[case] warns: bool,
    ) {
        let signer = SignerArgs {
            signer_id: "signer.testnet".parse().expect("valid account"),
            sign_with,
            secret_key: secret_key.map(str::to_owned),
            print,
            public_key: None,
        };

        let Some(warning) = signer.ignored_credential_warning() else {
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

    #[tokio::test]
    async fn plan_mode_rejects_credential_resolution() {
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
            sign_with: None,
            secret_key: None,
            print: Some(PrintFormat::Sputnik),
            public_key: None,
        };

        assert_eq!(
            signer
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
        let signer = SignerArgs {
            signer_id: "dao.near".parse().expect("valid account"),
            sign_with: None,
            secret_key: None,
            print: Some(PrintFormat::Json),
            public_key: None,
        };

        assert!(signer
            .public_key()
            .expect_err("plan must not invent a public key")
            .to_string()
            .contains("--public-key"));
    }

    #[test]
    fn execution_public_key_is_derived_from_typed_secret() {
        let secret_key: SecretKey = SECRET.parse().expect("valid secret");
        let expected = PublicKey::from(secret_key.public_key());
        let signer = SignerArgs {
            signer_id: "signer.testnet".parse().expect("valid account"),
            sign_with: None,
            secret_key: Some(SECRET.to_owned()),
            print: None,
            public_key: None,
        };

        assert_eq!(signer.public_key().expect("derived public key"), expected);
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
        let signer = SignerArgs {
            signer_id: "signer.testnet".parse().expect("valid account"),
            sign_with: Some(SigningBackend::Keychain),
            secret_key: None,
            print: None,
            public_key: None,
        };

        assert!(signer
            .public_key()
            .expect_err("a device key cannot be derived locally")
            .to_string()
            .contains("--public-key"));
    }
}
