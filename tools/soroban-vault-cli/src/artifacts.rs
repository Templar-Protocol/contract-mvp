use std::{
    fs,
    io::{ErrorKind, Write as _},
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    thread,
    time::Duration,
};

use anyhow::{anyhow, Context};
use sha2::{Digest, Sha256};

use crate::{
    cli::ArtifactName,
    manifest::{ArtifactRecord, Manifest},
    stellar::{CommandExecutor, Stellar},
};

/// Repository whose GitHub Releases host the release artifacts.
pub const RELEASE_REPO: &str = "Templar-Protocol/contracts";

/// Release tag whose assets the resolver pins.
pub const RELEASE_TAG: &str = "soroban-v1.1.1";

/// Exact release-cache root override. It must be nonempty when present.
pub(crate) const CACHE_ENV: &str = "TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE";

/// Download attempts before giving up. Assets are immutable, so a retry can
/// only ever fetch the same bytes.
const DOWNLOAD_ATTEMPTS: u32 = 5;

/// Doubled per attempt: 1 + 2 + 4 + 8 = 15s of backoff. A GitHub 500 on a
/// release asset outlasts a sub-second window.
const RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// Per-request ceiling. Without it a stalled connection hangs a deploy for its
/// whole budget instead of failing into the retry loop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Byte length and SHA-256 pin of one release asset. Both are verified on
/// every read, including of an existing cache entry, so wrong bytes never
/// reach upload or deployment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleasePin {
    pub length: usize,
    pub sha256: &'static str,
}

/// Exhaustive typed model of the artifacts shipped in [`RELEASE_TAG`].
///
/// Every artifact the CLI can deploy is a variant here, and every variant
/// carries its reviewed release pin; resolution and upload verify against it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseArtifact {
    Runtime,
    Governance,
    ShareToken,
    BlendAdapter,
    CustodialAdapter,
    Proxy4626,
    CuratorProxy,
}

impl ReleaseArtifact {
    /// Every artifact in the release, in stack deployment order.
    pub const ALL: [ReleaseArtifact; 7] = [
        Self::Runtime,
        Self::Governance,
        Self::ShareToken,
        Self::Proxy4626,
        Self::CuratorProxy,
        Self::BlendAdapter,
        Self::CustodialAdapter,
    ];

    pub const fn from_name(name: ArtifactName) -> Self {
        match name {
            ArtifactName::Vault => Self::Runtime,
            ArtifactName::Governance => Self::Governance,
            ArtifactName::ShareToken => Self::ShareToken,
            ArtifactName::BlendAdapter => Self::BlendAdapter,
            ArtifactName::CustodialAdapter => Self::CustodialAdapter,
            ArtifactName::Proxy4626 => Self::Proxy4626,
            ArtifactName::CuratorProxy => Self::CuratorProxy,
        }
    }

    /// Reviewed release pin: exact byte length and SHA-256 of the asset.
    pub const fn pin(self) -> ReleasePin {
        match self {
            Self::Runtime => ReleasePin {
                length: 129_499,
                sha256: "4d24790f3ea2a02e521b84d583dab00bfa246cdfd06ee858f1f656a831cccc83",
            },
            Self::Governance => ReleasePin {
                length: 61_598,
                sha256: "7e9b4247082227ddbf8567eb080c1d9c2ff181058e27b1effff60f2513b78b2e",
            },
            Self::ShareToken => ReleasePin {
                length: 11_816,
                sha256: "b0427e26c0b5201be42b2d0577773081b2f5a382507995e1ad9ca7f0ee241da5",
            },
            Self::BlendAdapter => ReleasePin {
                length: 12_112,
                sha256: "ce0024538e49cb726c861bee210f83bebca0a2352fec79f9dbdc25142df647cf",
            },
            Self::CustodialAdapter => ReleasePin {
                length: 11_981,
                sha256: "03c1c749613d67d5ed460bc4fbb12eedb1a043068665a295fbefb774588a796b",
            },
            Self::Proxy4626 => ReleasePin {
                length: 24_142,
                sha256: "b71d716ddda2b6648ece8ea8cacb3abfd0a893d3dc43e44b495c9df072e4ae9d",
            },
            Self::CuratorProxy => ReleasePin {
                length: 50_250,
                sha256: "a2932a58e22d206bd63732d7023321bbc9bb1c629865b4767e1ae589228f7868",
            },
        }
    }

    /// Manifest key for the artifact. Deploy flows look records up by it.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Runtime => "vault",
            Self::Governance => "governance",
            Self::ShareToken => "share_token",
            Self::BlendAdapter => "blend_adapter",
            Self::CustodialAdapter => "custodial_adapter",
            Self::Proxy4626 => "proxy_4626",
            Self::CuratorProxy => "curator_proxy",
        }
    }

    /// Cargo package building the artifact.
    pub const fn package(self) -> &'static str {
        match self {
            Self::Runtime => "templar-soroban-runtime",
            Self::Governance => "templar-soroban-governance",
            Self::ShareToken => "templar-soroban-share-token",
            Self::BlendAdapter => "templar-soroban-blend-adapter",
            Self::CustodialAdapter => "templar-soroban-custodial-adapter",
            Self::Proxy4626 => "templar-4626-proxy-soroban",
            Self::CuratorProxy => "templar-curator-proxy-soroban",
        }
    }

    /// Release asset file name, and the workspace build output name.
    pub const fn wasm_file_name(self) -> &'static str {
        match self {
            Self::Runtime => "templar_soroban_runtime.wasm",
            Self::Governance => "templar_soroban_governance.wasm",
            Self::ShareToken => "templar_soroban_share_token.wasm",
            Self::BlendAdapter => "templar_soroban_blend_adapter.wasm",
            Self::CustodialAdapter => "templar_soroban_custodial_adapter.wasm",
            Self::Proxy4626 => "templar_4626_proxy_soroban.wasm",
            Self::CuratorProxy => "templar_curator_proxy_soroban.wasm",
        }
    }

    /// Asset path relative to the workspace root.
    pub const fn wasm_relative_path(self) -> &'static str {
        match self {
            Self::Runtime => {
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_runtime.wasm"
            }
            Self::Governance => {
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_governance.wasm"
            }
            Self::ShareToken => {
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_share_token.wasm"
            }
            Self::BlendAdapter => {
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_blend_adapter.wasm"
            }
            Self::CustodialAdapter => {
                "target/wasm32-unknown-unknown/release-soroban/templar_soroban_custodial_adapter.wasm"
            }
            Self::Proxy4626 => {
                "target/wasm32-unknown-unknown/release-soroban/templar_4626_proxy_soroban.wasm"
            }
            Self::CuratorProxy => {
                "target/wasm32-unknown-unknown/release-soroban/templar_curator_proxy_soroban.wasm"
            }
        }
    }

    /// Typed spec tying the artifact's deploy metadata to its release pin.
    pub const fn spec(self) -> ArtifactSpec {
        ArtifactSpec {
            key: self.key(),
            package: self.package(),
            wasm_relative_path: self.wasm_relative_path(),
            build_output_dir: "target/wasm32-unknown-unknown/release-soroban",
            release: self,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactSpec {
    pub key: &'static str,
    pub package: &'static str,
    pub wasm_relative_path: &'static str,
    pub build_output_dir: &'static str,
    pub release: ReleaseArtifact,
}

impl ArtifactSpec {
    pub fn from_name(name: ArtifactName) -> Self {
        ReleaseArtifact::from_name(name).spec()
    }

    pub fn stack_artifacts(include_blend: bool, include_custodial: bool) -> Vec<Self> {
        let mut artifacts = vec![
            Self::from_name(ArtifactName::Vault),
            Self::from_name(ArtifactName::Governance),
            Self::from_name(ArtifactName::ShareToken),
            Self::from_name(ArtifactName::Proxy4626),
            Self::from_name(ArtifactName::CuratorProxy),
        ];
        if include_blend {
            artifacts.push(Self::from_name(ArtifactName::BlendAdapter));
        }
        if include_custodial {
            artifacts.push(Self::from_name(ArtifactName::CustodialAdapter));
        }
        artifacts
    }

    pub fn wasm_path(&self, workspace: &Path) -> PathBuf {
        workspace.join(self.wasm_relative_path)
    }

    pub fn output_dir(&self, workspace: &Path) -> PathBuf {
        workspace.join(self.build_output_dir)
    }
}

/// Verified release artifact resolver.
///
/// Sources, in exact precedence order: an isolated `--build`; a valid cache
/// entry; the exact-pin workspace seed; the pinned GitHub release download. A
/// cache entry that exists but fails verification is a hard error, never
/// silently replaced; a workspace file that fails the pin is ignored and
/// resolution falls through to download.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ExistingReleaseArtifact {
    Cache {
        path: PathBuf,
        sha256: String,
    },
    WorkspaceSeed {
        path: PathBuf,
        sha256: String,
    },
    IgnoredWorkspace {
        cache_path: PathBuf,
        workspace_path: PathBuf,
        reason: String,
    },
    Missing {
        cache_path: PathBuf,
    },
}

pub(crate) fn inspect_existing_release_artifact(
    workspace: &Path,
    spec: ArtifactSpec,
) -> anyhow::Result<ExistingReleaseArtifact> {
    let resolver = ArtifactResolver::production()?;
    let cache_path = resolver.cache_file(spec.release);
    match fs::read(&cache_path) {
        Ok(bytes) => {
            let sha256 = verify_release_bytes(spec.release, &bytes, "cache entry")?;
            return Ok(ExistingReleaseArtifact::Cache {
                path: cache_path,
                sha256,
            });
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(anyhow!(
                "read cached release artifact {}: {error}",
                cache_path.display()
            ));
        }
    }

    let workspace_path = spec.wasm_path(workspace);
    if workspace_path.exists() {
        let bytes = fs::read(&workspace_path)
            .with_context(|| format!("read {}", workspace_path.display()))?;
        return match verify_release_bytes(spec.release, &bytes, "workspace") {
            Ok(sha256) => Ok(ExistingReleaseArtifact::WorkspaceSeed {
                path: workspace_path,
                sha256,
            }),
            Err(error) => Ok(ExistingReleaseArtifact::IgnoredWorkspace {
                cache_path,
                workspace_path,
                reason: error.to_string(),
            }),
        };
    }
    Ok(ExistingReleaseArtifact::Missing { cache_path })
}

pub(crate) struct ArtifactResolver {
    cache_root: PathBuf,
    base_url: String,
}

impl ArtifactResolver {
    /// Production resolver: user cache directory (or the nonempty
    /// [`CACHE_ENV`] override) and the reviewed GitHub release base URL.
    pub(crate) fn production() -> anyhow::Result<Self> {
        Ok(Self {
            cache_root: release_cache_root()?,
            base_url: format!("https://github.com/{RELEASE_REPO}/releases/download"),
        })
    }

    fn cache_file(&self, release: ReleaseArtifact) -> PathBuf {
        self.cache_root
            .join(RELEASE_TAG)
            .join(release.wasm_file_name())
    }

    fn asset_url(&self, release: ReleaseArtifact) -> String {
        format!(
            "{}/{}/{}",
            self.base_url,
            RELEASE_TAG,
            release.wasm_file_name()
        )
    }

    /// Resolve and upload-or-reuse one artifact, returning its wasm hash.
    pub(crate) fn ensure_uploaded<E: CommandExecutor>(
        &self,
        stellar: &Stellar<'_, E>,
        manifest: &mut Manifest,
        workspace: &Path,
        spec: ArtifactSpec,
        build: bool,
    ) -> anyhow::Result<String> {
        self.ensure_uploaded_with_pin(
            stellar,
            manifest,
            workspace,
            spec,
            build,
            spec.release.pin(),
        )
    }

    fn ensure_uploaded_with_pin<E: CommandExecutor>(
        &self,
        stellar: &Stellar<'_, E>,
        manifest: &mut Manifest,
        workspace: &Path,
        spec: ArtifactSpec,
        build: bool,
        pin: ReleasePin,
    ) -> anyhow::Result<String> {
        let (wasm_path, resolved_sha) = if build {
            build_artifact(stellar, workspace, spec)?;
            let wasm_path = spec.wasm_path(workspace);
            anyhow::ensure!(
                wasm_path.exists(),
                "artifact {} was not found at {}",
                spec.key,
                wasm_path.display()
            );
            let resolved_sha = sha256_file(&wasm_path)?;
            (wasm_path, resolved_sha)
        } else {
            self.resolve_with_pin(workspace, spec, pin)?
        };
        Self::publish(stellar, manifest, spec, &wasm_path, &resolved_sha)
    }

    /// Cache, then workspace seed, then verified download.
    fn resolve_with_pin(
        &self,
        workspace: &Path,
        spec: ArtifactSpec,
        pin: ReleasePin,
    ) -> anyhow::Result<(PathBuf, String)> {
        let cache_file = self.cache_file(spec.release);
        match fs::read(&cache_file) {
            Ok(bytes) => {
                let resolved_sha = verify_bytes(spec.release, pin, &bytes, "cache entry")?;
                return Ok((cache_file, resolved_sha));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(anyhow!(
                    "read cached release artifact {}: {error}",
                    cache_file.display()
                ));
            }
        }

        let workspace_wasm = spec.wasm_path(workspace);
        if workspace_wasm.exists() {
            let bytes = fs::read(&workspace_wasm)
                .with_context(|| format!("read {}", workspace_wasm.display()))?;
            if let Ok(resolved_sha) = verify_bytes(spec.release, pin, &bytes, "workspace") {
                write_atomically(&cache_file, &bytes)?;
                return Ok((cache_file, resolved_sha));
            }
        }
        let url = self.asset_url(spec.release);
        let bytes = download_with_retries(&url)
            .with_context(|| format!("download release artifact {} from {url}", spec.key))?;
        let resolved_sha = verify_bytes(spec.release, pin, &bytes, &url)?;
        write_atomically(&cache_file, &bytes)?;
        tracing::debug!(
            artifact = spec.key,
            bytes = bytes.len(),
            "downloaded release artifact"
        );
        Ok((cache_file, resolved_sha))
    }

    /// Upload-or-reuse the resolved bytes, gating every network identity on
    /// the resolved SHA.
    fn publish<E: CommandExecutor>(
        stellar: &Stellar<'_, E>,
        manifest: &mut Manifest,
        spec: ArtifactSpec,
        wasm_path: &Path,
        resolved_sha: &str,
    ) -> anyhow::Result<String> {
        let on_chain = stellar.fetch_wasm_hash(resolved_sha)?;

        // The manifest record is reusable only when both of its hashes equal
        // the resolved SHA and the network actually serves that hash.
        if let Some(record) = manifest.artifacts.get(spec.key) {
            if record.local_hash == resolved_sha
                && record.remote_wasm_hash.as_deref() == Some(resolved_sha)
                && on_chain
            {
                return Ok(resolved_sha.to_owned());
            }
        }

        let remote_hash = if on_chain {
            resolved_sha.to_owned()
        } else {
            let uploaded = stellar.upload(&wasm_path.display().to_string())?;
            // The upload-returned hash must equal the resolved SHA before any
            // state is mutated.
            anyhow::ensure!(
                uploaded == resolved_sha,
                "upload of artifact {} returned wasm hash {uploaded} but the resolved release sha is {resolved_sha}",
                spec.key
            );
            uploaded
        };

        manifest.artifacts.insert(
            spec.key.to_string(),
            ArtifactRecord {
                package: spec.package.to_string(),
                wasm_path: wasm_path.to_path_buf(),
                local_hash: resolved_sha.to_owned(),
                remote_wasm_hash: Some(remote_hash.clone()),
                verified: true,
            },
        );
        Ok(remote_hash)
    }
}

/// Resolve and upload-or-reuse an artifact with the production resolver.
pub fn ensure_uploaded<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    manifest: &mut Manifest,
    workspace: &Path,
    spec: ArtifactSpec,
    build: bool,
) -> anyhow::Result<String> {
    ArtifactResolver::production()?.ensure_uploaded(stellar, manifest, workspace, spec, build)
}

/// Root of the release artifact cache: the exact nonempty [`CACHE_ENV`]
/// override, otherwise the CLI-owned directory under the platform cache.
pub(crate) fn release_cache_root() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os(CACHE_ENV) {
        anyhow::ensure!(
            !value.is_empty(),
            "{CACHE_ENV} must be a nonempty absolute path"
        );
        let path = PathBuf::from(value);
        anyhow::ensure!(
            path.is_absolute(),
            "{CACHE_ENV} must be an absolute path, got {}",
            path.display()
        );
        return Ok(path);
    }
    dirs::cache_dir()
        .map(|base| base.join("templar").join("soroban-vault-cli").join("artifacts"))
        .ok_or_else(|| {
            anyhow!(
                "could not determine the release artifact cache directory; set {CACHE_ENV} to an absolute path"
            )
        })
}

fn build_artifact<E: CommandExecutor>(
    stellar: &Stellar<'_, E>,
    workspace: &Path,
    spec: ArtifactSpec,
) -> anyhow::Result<()> {
    let out_dir = spec.output_dir(workspace);
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("create artifact output dir {}", out_dir.display()))?;
    stellar.build_package(
        &workspace.display().to_string(),
        spec.package,
        &out_dir.display().to_string(),
    )?;
    Ok(())
}

/// Verify bytes against the artifact's release pin, returning the hex SHA.
///
/// Length is checked before the digest so a disagreement reports bytes a
/// reader can compare against the release rather than an opaque hash.
fn verify_release_bytes(
    release: ReleaseArtifact,
    bytes: &[u8],
    source: &str,
) -> anyhow::Result<String> {
    verify_bytes(release, release.pin(), bytes, source)
}

fn verify_bytes(
    release: ReleaseArtifact,
    pin: ReleasePin,
    bytes: &[u8],
    source: &str,
) -> anyhow::Result<String> {
    if bytes.len() != pin.length {
        return Err(anyhow!(
            "release artifact {} from {source} has {} bytes but release {RELEASE_TAG} pins {}",
            release.key(),
            bytes.len(),
            pin.length
        ));
    }
    let sha256 = hex::encode(Sha256::digest(bytes));
    if sha256 != pin.sha256 {
        return Err(anyhow!(
            "release artifact {} from {source} hashes to {sha256} but release {RELEASE_TAG} pins {}",
            release.key(),
            pin.sha256
        ));
    }
    Ok(sha256)
}
/// Statuses worth another attempt: rate limiting and server-side faults. Every
/// other status is the server's settled answer and retrying only wastes time.
fn is_retryable(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

enum DownloadAttempt {
    Status(u16),
    Transport(reqwest::Error),
}

fn download_with_retries(url: &str) -> anyhow::Result<Vec<u8>> {
    let client = http_client()?;
    let mut last_status = None;
    let mut last_transport = None;
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        if attempt > 0 {
            thread::sleep(RETRY_BACKOFF.saturating_mul(1u32 << (attempt - 1)));
        }
        match try_download(client, url) {
            Ok(bytes) => return Ok(bytes),
            Err(DownloadAttempt::Status(status)) if is_retryable(status) => {
                last_status = Some(status);
            }
            Err(DownloadAttempt::Status(status)) => {
                return Err(anyhow!("download failed: {url} returned status {status}"));
            }
            Err(DownloadAttempt::Transport(error)) => {
                tracing::debug!(url, error = %error, attempt, "release download transport error");
                last_transport = Some(error);
            }
        }
    }
    if let Some(error) = last_transport {
        return Err(anyhow!(
            "download failed after {DOWNLOAD_ATTEMPTS} attempts: {url}: {error}"
        ));
    }
    let status = last_status.unwrap_or_default();
    Err(anyhow!(
        "download failed after {DOWNLOAD_ATTEMPTS} attempts: {url} kept returning status {status}"
    ))
}

fn try_download(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, DownloadAttempt> {
    let response = client.get(url).send().map_err(DownloadAttempt::Transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(DownloadAttempt::Status(status.as_u16()));
    }
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(DownloadAttempt::Transport)
}

/// One client per process, so repeated downloads reuse connections instead of
/// paying a TLS handshake per asset.
fn http_client() -> anyhow::Result<&'static reqwest::blocking::Client> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| anyhow!("build release download client: {error}"))
}

/// Write through a unique temporary file in the target directory, then publish
/// it with a no-replace hard link. Concurrent writers either publish first or
/// verify the identical winner; neither can replace an accepted cache entry.
fn write_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let with_context = |error| {
        anyhow::Error::new(error).context(format!("write release artifact {}", path.display()))
    };

    let dir = path.parent().unwrap_or(path);
    fs::create_dir_all(dir).map_err(with_context)?;

    let temporary = dir.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("artifact"),
        process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        match fs::hard_link(&temporary, path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if fs::read(path)? == bytes {
                    Ok(())
                } else {
                    Err(std::io::Error::new(
                        ErrorKind::AlreadyExists,
                        format!(
                            "existing cache entry {} has different bytes",
                            path.display()
                        ),
                    ))
                }
            }
            Err(error) => Err(error),
        }
    })();
    let cleanup = fs::remove_file(&temporary);
    if let Err(error) = written {
        return Err(with_context(error));
    }
    cleanup.map_err(with_context)
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let digest = Sha256::digest(bytes);
    Ok(hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
    };

    use clap::Parser as _;

    use super::*;

    fn pinned(bytes: &'static [u8]) -> ReleasePin {
        ReleasePin {
            length: bytes.len(),
            sha256: Box::leak(hex::encode(Sha256::digest(bytes)).into_boxed_str()),
        }
    }

    const TEST_BYTES: &[u8] = b"templar release artifact test bytes";
    const OTHER_BYTES: &[u8] = b"other bytes that must not be reused";

    /// Command executor standing in for the network: configurable on-chain
    /// hash lookup and upload result, recording every call.
    struct ChainMock {
        on_chain: bool,
        upload_hash: Option<String>,
        calls: Mutex<Vec<(&'static str, Vec<String>)>>,
    }

    impl ChainMock {
        fn new(on_chain: bool, upload_hash: Option<String>) -> Self {
            Self {
                on_chain,
                upload_hash,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self, kind: &'static str) -> Vec<Vec<String>> {
            self.calls
                .lock()
                .expect("lock calls")
                .iter()
                .filter(|(call, _)| *call == kind)
                .map(|(_, args)| args.clone())
                .collect()
        }

        fn value_after<'a>(args: &'a [String], wanted: &str) -> Option<&'a str> {
            args.iter()
                .zip(args.iter().skip(1))
                .find_map(|(arg, value)| (arg == wanted).then_some(value.as_str()))
        }
    }

    impl CommandExecutor for ChainMock {
        fn run(
            &self,
            _program: &str,
            args: &[String],
            _redacted_args: &[usize],
            _env: &[crate::stellar::CommandEnv],
        ) -> anyhow::Result<crate::stellar::CommandOutput> {
            let call = |kind| {
                self.calls
                    .lock()
                    .expect("lock calls")
                    .push((kind, args.to_vec()));
            };
            match (
                args.first().map(String::as_str),
                args.get(1).map(String::as_str),
            ) {
                (Some("contract"), Some("fetch")) => {
                    call("fetch");
                    if self.on_chain {
                        Ok(crate::stellar::CommandOutput {
                            stdout: "found\n".to_string(),
                            stderr: String::new(),
                        })
                    } else {
                        Err(anyhow!("not found"))
                    }
                }
                (Some("contract"), Some("upload")) => {
                    let kind = if args.iter().any(|arg| arg == "--build-only") {
                        "upload-build-only"
                    } else {
                        "upload"
                    };
                    call(kind);
                    let wasm_path = Self::value_after(args, "--wasm").expect("upload --wasm");
                    let hash = self
                        .upload_hash
                        .clone()
                        .unwrap_or_else(|| sha256_file(Path::new(wasm_path)).expect("hash upload"));
                    Ok(crate::stellar::CommandOutput {
                        stdout: format!("wasm_hash: {hash}\n"),
                        stderr: String::new(),
                    })
                }
                (Some("contract"), Some("build")) => {
                    call("build");
                    let out_dir = Self::value_after(args, "--out-dir").expect("build --out-dir");
                    let package = Self::value_after(args, "--package").expect("build --package");
                    let out_dir = Path::new(out_dir);
                    fs::create_dir_all(out_dir).expect("create build out dir");
                    fs::write(
                        out_dir
                            .join(package.replace('-', "_"))
                            .join("")
                            .with_file_name(format!("{}.wasm", package.replace('-', "_"))),
                        b"built wasm",
                    )
                    .expect("write built wasm");
                    Ok(crate::stellar::CommandOutput {
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                }
                _ => Ok(crate::stellar::CommandOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                }),
            }
        }
    }

    /// Local HTTP server scripting one response per request, recording hits.
    struct ReleaseServer {
        base_url: String,
        hits: Arc<Mutex<Vec<String>>>,
    }

    struct ScriptedResponse {
        status: u16,
        bytes: Vec<u8>,
    }

    impl ReleaseServer {
        fn spawn(response: ScriptedResponse) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test server");
            let port = listener.local_addr().expect("local addr").port();
            let hits = Arc::new(Mutex::new(Vec::new()));
            let hits_writer = Arc::clone(&hits);
            let response = Arc::new(response);
            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    if let Some(path) = read_request_path(&mut stream) {
                        hits_writer.lock().expect("lock hits").push(path);
                    }
                    write_response(&mut stream, &response);
                }
            });
            Self {
                base_url: format!("http://127.0.0.1:{port}"),
                hits,
            }
        }

        fn hit_count(&self) -> usize {
            self.hits.lock().expect("lock hits").len()
        }

        fn last_path(&self) -> String {
            self.hits
                .lock()
                .expect("lock hits")
                .last()
                .cloned()
                .expect("server was hit")
        }
    }

    fn read_request_path(stream: &mut TcpStream) -> Option<String> {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = stream.read(&mut chunk).ok()?;
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
            if buffer.len() > 16 * 1024 {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buffer);
        head.lines()
            .next()?
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
    }

    fn write_response(stream: &mut TcpStream, response: &ScriptedResponse) {
        let ScriptedResponse { status, bytes } = response;
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        )
        .and_then(|()| stream.write_all(bytes));
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Write);
    }

    fn test_cli() -> crate::cli::Cli {
        crate::cli::Cli::parse_from([
            "tmplr-soroban-vault",
            "--workspace-path",
            "/tmp/templar-artifacts-test",
            "--network",
            "testnet",
            "status",
        ])
    }

    fn resolver(root: &Path, base_url: &str) -> ArtifactResolver {
        ArtifactResolver {
            cache_root: root.to_path_buf(),
            base_url: base_url.to_string(),
        }
    }
    fn resolve_via_pinned(
        stellar: &Stellar<'_, ChainMock>,
        manifest: &mut Manifest,
        workspace: &Path,
        release: ReleaseArtifact,
        resolver: &ArtifactResolver,
        pin: ReleasePin,
    ) -> anyhow::Result<String> {
        resolver.ensure_uploaded_with_pin(stellar, manifest, workspace, release.spec(), false, pin)
    }

    #[test]

    fn release_catalog_matches_reviewed_pins() {
        let expected = [
            (
                ReleaseArtifact::Runtime,
                129_499,
                "4d24790f3ea2a02e521b84d583dab00bfa246cdfd06ee858f1f656a831cccc83",
            ),
            (
                ReleaseArtifact::Governance,
                61_598,
                "7e9b4247082227ddbf8567eb080c1d9c2ff181058e27b1effff60f2513b78b2e",
            ),
            (
                ReleaseArtifact::ShareToken,
                11_816,
                "b0427e26c0b5201be42b2d0577773081b2f5a382507995e1ad9ca7f0ee241da5",
            ),
            (
                ReleaseArtifact::Proxy4626,
                24_142,
                "b71d716ddda2b6648ece8ea8cacb3abfd0a893d3dc43e44b495c9df072e4ae9d",
            ),
            (
                ReleaseArtifact::CuratorProxy,
                50_250,
                "a2932a58e22d206bd63732d7023321bbc9bb1c629865b4767e1ae589228f7868",
            ),
            (
                ReleaseArtifact::BlendAdapter,
                12_112,
                "ce0024538e49cb726c861bee210f83bebca0a2352fec79f9dbdc25142df647cf",
            ),
            (
                ReleaseArtifact::CustodialAdapter,
                11_981,
                "03c1c749613d67d5ed460bc4fbb12eedb1a043068665a295fbefb774588a796b",
            ),
        ];
        assert_eq!(ReleaseArtifact::ALL.len(), expected.len());
        for (release, length, sha256) in expected {
            assert_eq!(release.pin(), ReleasePin { length, sha256 });
        }
    }

    #[test]
    fn valid_cache_skips_http_and_reuses_network_hash() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let resolver = resolver(root.path(), "http://127.0.0.1:9");
        let cache_file = resolver.cache_file(release);
        fs::create_dir_all(cache_file.parent().expect("cache parent")).expect("cache dir");
        fs::write(&cache_file, TEST_BYTES).expect("cache bytes");
        let cli = test_cli();
        let executor = ChainMock::new(true, None);
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        let hash = resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect("resolve cache");

        assert_eq!(hash, pin.sha256);
        assert_eq!(executor.calls("fetch").len(), 1);
        assert!(executor.calls("upload").is_empty());
        assert!(executor.calls("build").is_empty());
    }

    #[test]
    fn corrupt_cache_is_hard_error_before_http_or_upload() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let server = ReleaseServer::spawn(ScriptedResponse {
            status: 200,
            bytes: TEST_BYTES.to_vec(),
        });
        let resolver = resolver(root.path(), &server.base_url);
        let cache_file = resolver.cache_file(release);
        fs::create_dir_all(cache_file.parent().expect("cache parent")).expect("cache dir");
        fs::write(&cache_file, OTHER_BYTES).expect("corrupt cache");
        let cli = test_cli();
        let executor = ChainMock::new(false, None);
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        let error = resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect_err("corrupt cache must fail");

        assert!(error.to_string().contains("cache entry"));
        assert_eq!(server.hit_count(), 0);
        assert!(executor.calls("fetch").is_empty());
        assert!(executor.calls("upload").is_empty());
        assert!(executor.calls("build").is_empty());
    }

    #[test]
    fn exact_workspace_seed_populates_cache_and_uploads_cache_path() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let resolver = resolver(root.path(), "http://127.0.0.1:9");
        let workspace_file = release.spec().wasm_path(workspace.path());
        fs::create_dir_all(workspace_file.parent().expect("workspace parent"))
            .expect("workspace dir");
        fs::write(&workspace_file, TEST_BYTES).expect("workspace seed");
        let cli = test_cli();
        let executor = ChainMock::new(false, Some(pin.sha256.to_string()));
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect("resolve workspace seed");

        let cache_file = resolver.cache_file(release);
        assert_eq!(fs::read(&cache_file).expect("cache seed"), TEST_BYTES);
        let upload = executor.calls("upload");
        assert_eq!(upload.len(), 1);
        assert!(upload[0]
            .iter()
            .any(|value| value == cache_file.to_str().expect("utf8 cache")));
        assert!(executor.calls("build").is_empty());
    }

    #[test]
    fn atomic_write_never_replaces_an_existing_cache_entry() {
        let root = tempfile::tempdir().expect("temp cache");
        let path = root.path().join("artifact.wasm");
        fs::write(&path, TEST_BYTES).expect("existing cache entry");

        write_atomically(&path, TEST_BYTES).expect("identical concurrent winner");
        assert_eq!(fs::read(&path).expect("winner bytes"), TEST_BYTES);

        let error = write_atomically(&path, OTHER_BYTES).expect_err("different bytes must lose");
        assert!(format!("{error:#}").contains("different bytes"));
        assert_eq!(fs::read(&path).expect("preserved winner"), TEST_BYTES);
        assert!(fs::read_dir(root.path())
            .expect("cache directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn mismatching_workspace_downloads_and_caches_verified_bytes() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let server = ReleaseServer::spawn(ScriptedResponse {
            status: 200,
            bytes: TEST_BYTES.to_vec(),
        });
        let resolver = resolver(root.path(), &server.base_url);
        let workspace_file = release.spec().wasm_path(workspace.path());
        fs::create_dir_all(workspace_file.parent().expect("workspace parent"))
            .expect("workspace dir");
        fs::write(&workspace_file, OTHER_BYTES).expect("workspace mismatch");
        let cli = test_cli();
        let executor = ChainMock::new(false, Some(pin.sha256.to_string()));
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect("download release");

        assert_eq!(server.hit_count(), 1);
        assert_eq!(
            server.last_path(),
            format!("/{RELEASE_TAG}/{}", release.wasm_file_name())
        );
        assert_eq!(
            fs::read(resolver.cache_file(release)).expect("cache download"),
            TEST_BYTES
        );
    }

    #[test]
    fn upload_hash_mismatch_fails_before_manifest_mutation() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let resolver = resolver(root.path(), "http://127.0.0.1:9");
        let cache_file = resolver.cache_file(release);
        fs::create_dir_all(cache_file.parent().expect("cache parent")).expect("cache dir");
        fs::write(&cache_file, TEST_BYTES).expect("cache bytes");
        let cli = test_cli();
        let wrong_hash = "f".repeat(64);
        let executor = ChainMock::new(false, Some(wrong_hash.clone()));
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        let error = resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect_err("upload mismatch must fail");

        assert!(format!("{error:#}").contains(&wrong_hash));
        assert!(manifest.artifacts.is_empty());
    }

    #[test]
    fn settled_404_is_not_retried() {
        let root = tempfile::tempdir().expect("temp cache");
        let workspace = tempfile::tempdir().expect("temp workspace");
        let release = ReleaseArtifact::Runtime;
        let pin = pinned(TEST_BYTES);
        let server = ReleaseServer::spawn(ScriptedResponse {
            status: 404,
            bytes: Vec::new(),
        });
        let resolver = resolver(root.path(), &server.base_url);
        let cli = test_cli();
        let executor = ChainMock::new(false, None);
        let stellar = Stellar::new(&cli, &executor);
        let mut manifest = Manifest::new("testnet", None);

        let error = resolve_via_pinned(
            &stellar,
            &mut manifest,
            workspace.path(),
            release,
            &resolver,
            pin,
        )
        .expect_err("404 must fail");

        assert!(format!("{error:#}").contains("status 404"));
        assert_eq!(server.hit_count(), 1);
    }
}
