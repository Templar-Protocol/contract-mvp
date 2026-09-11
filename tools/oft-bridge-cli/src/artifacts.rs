use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{
    error::{Error, Result},
    output::CommandData,
    state::RouteStore,
};

const LOCK_BYTES: &[u8] = include_bytes!("../artifacts.lock.json");

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArtifactLockV1 {
    pub schema_name: String,
    pub schema_version: u32,
    pub layerzero_source: LayerZeroSourceLockV1,
    pub stellar: StellarArtifactLockV1,
    pub evm: EvmArtifactLockV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayerZeroSourceLockV1 {
    pub remote: String,
    pub commit: String,
    pub source_archive_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StellarArtifactLockV1 {
    pub rust_toolchain: String,
    pub target: String,
    pub soroban_cli: String,
    pub oft_wasm_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EvmArtifactLockV1 {
    pub oft_evm_version: String,
    pub oft_evm_integrity: String,
    pub oapp_evm_version: String,
    pub oapp_evm_integrity: String,
    pub message_lib_version: String,
    pub message_lib_integrity: String,
    pub protocol_version: String,
    pub protocol_integrity: String,
    pub openzeppelin_version: String,
    pub openzeppelin_integrity: String,
    pub openzeppelin_upgradeable_integrity: String,
    pub solc: String,
    pub optimizer: bool,
    pub optimizer_runs: u32,
    pub wrapper_source_sha256: String,
    pub package_json_sha256: String,
    pub foundry_toml_sha256: String,
    pub remappings_sha256: String,
    /// Digest of the operator-supplied build dependency archive: the exact
    /// npm package closure (with registry integrity pins) the build extracts.
    pub build_deps_archive_sha256: String,
    pub creation_bytecode_keccak256: String,
    pub runtime_bytecode_keccak256: String,
}

pub fn embedded_lock() -> Result<ArtifactLockV1> {
    let lock: ArtifactLockV1 = serde_json::from_slice(LOCK_BYTES)?;
    if lock.schema_name != "artifact_lock" || lock.schema_version != 1 {
        return Err(Error::InvalidInput(
            "unsupported artifact lock schema".into(),
        ));
    }
    if lock.evm.oft_evm_version != "4.0.1"
        || lock.evm.oapp_evm_version != "0.4.1"
        || lock.evm.message_lib_version != "3.0.168"
        || lock.evm.protocol_version != "3.0.168"
        || lock.evm.openzeppelin_version != "5.6.1"
        || [
            &lock.evm.oft_evm_integrity,
            &lock.evm.oapp_evm_integrity,
            &lock.evm.message_lib_integrity,
            &lock.evm.protocol_integrity,
            &lock.evm.openzeppelin_integrity,
            &lock.evm.openzeppelin_upgradeable_integrity,
        ]
        .iter()
        .any(|integrity| !integrity.starts_with("sha512-"))
    {
        return Err(Error::Custody(
            "EVM dependency closure must use the frozen exact versions and sha512 integrities"
                .into(),
        ));
    }
    Ok(lock)
}

/// Digest of the embedded artifact lock; binds plans to artifact custody.
pub fn lock_sha256() -> Result<String> {
    Ok(sha256(LOCK_BYTES))
}

pub fn verify_command(state: &Path) -> Result<CommandData> {
    let _ = RouteStore::open(state)?;
    let lock = embedded_lock()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    verify_hash(
        &root.join("evm/src/DisposableOFT.sol"),
        &lock.evm.wrapper_source_sha256,
    )?;
    verify_hash(
        &root.join("evm/package.json"),
        &lock.evm.package_json_sha256,
    )?;
    verify_hash(
        &root.join("evm/foundry.toml"),
        &lock.evm.foundry_toml_sha256,
    )?;
    verify_hash(
        &root.join("evm/remappings.txt"),
        &lock.evm.remappings_sha256,
    )?;
    verify_preserved_artifacts(state, &lock)?;
    Ok(CommandData {
        result: serde_json::json!({"verified": true, "artifact_lock_sha256": sha256(LOCK_BYTES)}),
        artifact: None,
    })
}

pub fn verify_preserved_artifacts(state: &Path, lock: &ArtifactLockV1) -> Result<()> {
    let route_root = state;
    let stellar_path = route_root
        .join(".artifacts")
        .join(format!("stellar-{}.wasm", lock.stellar.oft_wasm_sha256));
    verify_hash(&stellar_path, &lock.stellar.oft_wasm_sha256)?;
    let evm_path = route_root
        .join(".artifacts")
        .join(format!("evm-{}.json", lock.evm.creation_bytecode_keccak256));
    let metadata = fs::symlink_metadata(&evm_path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 16 * 1024 * 1024
    {
        return Err(Error::Custody(
            "preserved EVM artifact must be a regular file no larger than 16 MiB".into(),
        ));
    }
    let artifact: serde_json::Value = serde_json::from_slice(&fs::read(&evm_path)?)?;
    for (field, expected, label) in [
        (
            "bytecode",
            &lock.evm.creation_bytecode_keccak256,
            "creation",
        ),
        (
            "deployedBytecode",
            &lock.evm.runtime_bytecode_keccak256,
            "runtime",
        ),
    ] {
        let encoded = artifact
            .get(field)
            .and_then(|value| value.get("object"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Custody(format!("preserved EVM {label} bytecode is absent")))?;
        let bytes = hex::decode(encoded.trim_start_matches("0x"))
            .map_err(|_| Error::Custody(format!("preserved EVM {label} bytecode is not hex")))?;
        let actual = hex::encode(crate::evm::keccak256_of(&bytes));
        if actual != *expected {
            return Err(Error::Custody(format!(
                "preserved EVM {label} bytecode diverges from the artifact lock"
            )));
        }
    }
    Ok(())
}

pub fn build_command(
    state: &Path,
    out_dir: &Path,
    write: bool,
    deps_archive: Option<&Path>,
    source_archive: Option<&Path>,
) -> Result<CommandData> {
    let lock = embedded_lock()?;
    build_with_executor(
        state,
        out_dir,
        write,
        deps_archive,
        source_archive,
        &lock,
        &crate::process::RealExecutor,
    )
}

pub fn build_with_executor(
    state: &Path,
    out_dir: &Path,
    write: bool,
    deps_archive: Option<&Path>,
    source_archive: Option<&Path>,
    lock: &ArtifactLockV1,
    executor: &dyn crate::process::CommandExecutor,
) -> Result<CommandData> {
    if !write {
        let _ = RouteStore::open(state)?.load_state()?;
        return Ok(CommandData {
            result: serde_json::json!({
                "preview": true,
                "out_dir": out_dir,
                "commands": [
                    "verify wrapper/package/foundry/remappings/deps-archive/source-archive digests",
                    "forge build --root <out_dir>/work",
                    "stellar contract build --manifest-path <out_dir>/stellar-source/Cargo.toml --package oft",
                ],
                "requires_deps_archive": true,
                "requires_source_archive": true,
            }),
            artifact: None,
        });
    }
    let source = source_archive.ok_or_else(|| {
        Error::InvalidInput("artifact build --write requires --source-archive".into())
    })?;
    verify_hash(source, &lock.layerzero_source.source_archive_sha256)?;

    let _route = RouteStore::open(state)?.load_state()?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wrapper = root.join("evm/src/DisposableOFT.sol");
    let package = root.join("evm/package.json");
    let foundry_toml = root.join("evm/foundry.toml");
    let remappings = root.join("evm/remappings.txt");
    if out_dir.exists() {
        return Err(Error::Conflict(format!(
            "artifact output already exists: {}",
            out_dir.display()
        )));
    }
    verify_hash(&wrapper, &lock.evm.wrapper_source_sha256)?;
    verify_hash(&package, &lock.evm.package_json_sha256)?;
    verify_hash(&foundry_toml, &lock.evm.foundry_toml_sha256)?;
    verify_hash(&remappings, &lock.evm.remappings_sha256)?;
    let deps = deps_archive.ok_or_else(|| {
        Error::InvalidInput(
            "artifact build --write requires --deps-archive matching the embedded lock".into(),
        )
    })?;
    verify_hash(deps, &lock.evm.build_deps_archive_sha256)?;
    let work = prepare_work_dir(
        out_dir,
        deps,
        [&wrapper, &package, &foundry_toml, &remappings],
        executor,
    )?;
    run_forge(&work, executor)?;
    let stellar_wasm = build_stellar(out_dir, source, lock, executor)?;
    finalize_build(state, out_dir, &work, lock, &stellar_wasm)
}

/// Runs the pinned Foundry build inside the prepared work dir.
fn run_forge(work: &Path, executor: &dyn crate::process::CommandExecutor) -> Result<()> {
    let work_str = work
        .to_str()
        .ok_or_else(|| Error::InvalidInput("artifact work path must be valid UTF-8".into()))?;
    executor
        .run(
            "forge",
            &[
                "build".to_string(),
                "--root".to_string(),
                work_str.to_string(),
            ],
            &[],
            &[],
        )
        .map_err(|e| Error::Chain(format!("forge build failed: {e}")))?;
    Ok(())
}

fn build_stellar(
    out_dir: &Path,
    source: &Path,
    lock: &ArtifactLockV1,
    executor: &dyn crate::process::CommandExecutor,
) -> Result<std::path::PathBuf> {
    let work = out_dir.join("stellar-source");
    fs::create_dir(&work)?;
    let work_str = work
        .to_str()
        .ok_or_else(|| Error::InvalidInput("Stellar work path must be valid UTF-8".into()))?;
    let source_str = source
        .to_str()
        .ok_or_else(|| Error::InvalidInput("source archive path must be valid UTF-8".into()))?;
    executor.run(
        "tar",
        &[
            "-xzf".into(),
            source_str.into(),
            "-C".into(),
            work_str.into(),
        ],
        &[],
        &[],
    )?;
    let version = executor.run("stellar", &["--version".into()], &[], &[])?;
    if !version.stdout.contains(&lock.stellar.soroban_cli) {
        return Err(Error::Custody(format!(
            "stellar CLI version mismatch: expected {}",
            lock.stellar.soroban_cli
        )));
    }
    let manifest = work.join("Cargo.toml");
    let manifest = manifest
        .to_str()
        .ok_or_else(|| Error::InvalidInput("Stellar manifest path must be valid UTF-8".into()))?;
    executor.run(
        "stellar",
        &[
            "contract".into(),
            "build".into(),
            "--manifest-path".into(),
            manifest.into(),
            "--package".into(),
            "oft".into(),
        ],
        &[],
        &[],
    )?;
    let built = work.join("target/wasm32v1-none/release/oft.wasm");
    verify_hash(&built, &lock.stellar.oft_wasm_sha256)?;
    let destination = out_dir.join("stellar/oft.wasm");
    fs::create_dir(out_dir.join("stellar"))?;
    fs::copy(&built, &destination)?;
    Ok(destination)
}

/// Reads the forge artifact, verifies the frozen bytecode digests, and
/// persists the chained build report.
fn preserve_route_artifact(route: &Path, source: &Path, name: &str) -> Result<PathBuf> {
    let directory = RouteStore::open(route)?.root().join(".artifacts");
    if directory.exists() {
        let metadata = fs::symlink_metadata(&directory)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::Custody(
                "route artifact directory is not a real directory".into(),
            ));
        }
    } else {
        fs::create_dir(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    let destination = directory.join(name);
    let bytes = fs::read(source)?;
    if destination.exists() {
        if fs::read(&destination)? != bytes {
            return Err(Error::Conflict(format!(
                "route artifact {name} already exists with different bytes"
            )));
        }
        return Ok(destination);
    }
    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    if temporary.exists() {
        let metadata = fs::symlink_metadata(&temporary)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Error::Custody(
                "route artifact temporary path is unsafe".into(),
            ));
        }
        fs::remove_file(&temporary)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    match fs::hard_link(&temporary, &destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(&destination)? != bytes {
                fs::remove_file(&temporary)?;
                return Err(Error::Conflict(format!(
                    "route artifact {name} concurrently appeared with different bytes"
                )));
            }
        }
        Err(error) => {
            fs::remove_file(&temporary)?;
            return Err(error.into());
        }
    }
    fs::remove_file(&temporary)?;
    fs::File::open(&directory)?.sync_all()?;
    Ok(destination)
}

fn finalize_build(
    state: &Path,
    out_dir: &Path,
    work: &Path,
    lock: &ArtifactLockV1,
    stellar_wasm: &Path,
) -> Result<CommandData> {
    let evm_artifact = work.join("out/DisposableOFT.sol/DisposableOFT.json");
    let artifact_json: serde_json::Value = serde_json::from_slice(&fs::read(&evm_artifact)?)?;
    let bytecode_hex = artifact_json
        .get("bytecode")
        .and_then(|b| b.get("object"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let runtime_hex = artifact_json
        .get("deployedBytecode")
        .and_then(|b| b.get("object"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if bytecode_hex.len() < 4 || runtime_hex.len() < 4 {
        return Err(Error::Custody(
            "forge artifact produced empty bytecode".into(),
        ));
    }
    let creation_bytes = hex::decode(bytecode_hex.strip_prefix("0x").unwrap_or(bytecode_hex))
        .map_err(|_| Error::Custody("forge creation bytecode is not hex".into()))?;
    let runtime_bytes = hex::decode(runtime_hex.strip_prefix("0x").unwrap_or(runtime_hex))
        .map_err(|_| Error::Custody("forge runtime bytecode is not hex".into()))?;
    let creation = hex::encode(crate::evm::keccak256_of(&creation_bytes));
    let runtime = hex::encode(crate::evm::keccak256_of(&runtime_bytes));
    // The frozen lock is authoritative: a rebuilt artifact that diverges is
    // a custody failure, never a silent re-freeze.
    for (frozen, actual, label) in [
        (
            &lock.evm.creation_bytecode_keccak256,
            creation.as_str(),
            "creation",
        ),
        (
            &lock.evm.runtime_bytecode_keccak256,
            runtime.as_str(),
            "runtime",
        ),
    ] {
        if frozen != actual {
            return Err(Error::Custody(format!(
                "built {label} bytecode diverges from the frozen artifact lock"
            )));
        }
    }
    let stellar_name = format!("stellar-{}.wasm", lock.stellar.oft_wasm_sha256);
    let evm_name = format!("evm-{}.json", lock.evm.creation_bytecode_keccak256);
    preserve_route_artifact(state, stellar_wasm, &stellar_name)?;
    preserve_route_artifact(state, &evm_artifact, &evm_name)?;
    let stellar = serde_json::json!({
        "oft_wasm_sha256": lock.stellar.oft_wasm_sha256,
        "path": format!(".artifacts/{stellar_name}"),
        "source_archive_sha256": lock.layerzero_source.source_archive_sha256,
        "source_commit": lock.layerzero_source.commit,
    });
    let report = serde_json::json!({
        "schema": "artifact_build_report",
        "schema_version": crate::domain::SCHEMA_VERSION,
        "artifact_lock_sha256": sha256(LOCK_BYTES),
        "deps_archive_sha256": lock.evm.build_deps_archive_sha256,
        "evm": {
            "creation_bytecode_keccak256": creation,
            "runtime_bytecode_keccak256": runtime,
            "path": format!(".artifacts/{evm_name}"),
        },
        "stellar": stellar,
    });
    let report_path = out_dir.join("build-report.json");
    crate::state::write_create_new_json(&report_path, &report)?;
    let lock_sha = sha256(LOCK_BYTES);
    RouteStore::open(state)?.append_operation(
        crate::state::OperationEventV1 {
            operation_id: format!("artifact-build-{}", &lock_sha[..16]),
            state: crate::state::OperationState::Planned,
            detail: serde_json::to_value(crate::domain::LocalPreparationV1::BuildArtifact {
                artifact_lock_sha256: lock_sha,
            })?,
        },
        None,
    )?;
    let artifact = crate::domain::ArtifactRefV1 {
        kind: "artifact_build_report".into(),
        path: report_path,
        sha256: crate::canonical_sha256(&report)?,
        schema_version: crate::domain::SCHEMA_VERSION,
        authoritative: true,
    };
    Ok(CommandData {
        result: report,
        artifact: Some(artifact),
    })
}
/// Extracts the digest-verified dependency archive and materializes the build
/// root from the in-crate inputs.
fn prepare_work_dir(
    out_dir: &Path,
    deps: &Path,
    inputs: [&std::path::PathBuf; 4],
    executor: &dyn crate::process::CommandExecutor,
) -> Result<std::path::PathBuf> {
    let [wrapper, package, foundry_toml, remappings] = inputs;
    let work = out_dir.join("work");
    let node_modules = work.join("node_modules");
    fs::create_dir_all(&node_modules)?;
    let deps_str = deps
        .to_str()
        .ok_or_else(|| Error::InvalidInput("deps archive path must be valid UTF-8".into()))?;
    let node_modules_str = node_modules
        .to_str()
        .ok_or_else(|| Error::InvalidInput("artifact work path must be valid UTF-8".into()))?;
    executor
        .run(
            "tar",
            &[
                "-xf".to_string(),
                deps_str.to_string(),
                "-C".to_string(),
                node_modules_str.to_string(),
            ],
            &[],
            &[],
        )
        .map_err(|e| Error::Chain(format!("deps archive extraction failed: {e}")))?;
    // Materialize the build root: wrapper, manifest, and compiler config.
    fs::create_dir_all(work.join("src"))?;
    fs::copy(wrapper, work.join("src/DisposableOFT.sol"))?;
    fs::copy(package, work.join("package.json"))?;
    fs::copy(foundry_toml, work.join("foundry.toml"))?;
    fs::copy(remappings, work.join("remappings.txt"))?;
    Ok(work)
}

fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::Custody(format!(
            "artifact input must be a regular file: {}",
            path.display()
        )));
    }
    let actual = sha256(&fs::read(path)?);
    if actual != expected {
        return Err(Error::Custody(format!(
            "artifact hash mismatch for {}",
            path.display()
        )));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
