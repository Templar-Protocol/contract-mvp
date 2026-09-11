// Tests fail by panicking; Result handling in assertions is noise.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{cell::RefCell, collections::BTreeMap, fs, path::Path};

use sha2::{Digest as _, Sha256};
use templar_oft_bridge_cli::{
    artifacts::{
        build_with_executor as combined_build_with_executor, ArtifactLockV1, EvmArtifactLockV1,
        LayerZeroSourceLockV1, StellarArtifactLockV1,
    },
    domain::{AssetKind, AssetPolicyV1, ChainIdentityV1, DesiredRouteV1, Environment},
    error::Error,
    process::{CommandEnv, CommandExecutor, CommandOutput},
    state::RouteStore,
};

fn sha256(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

fn desired() -> DesiredRouteV1 {
    DesiredRouteV1 {
        schema_name: "desired_route".into(),
        schema_version: 1,
        route_id: "route-build".into(),
        identity: ChainIdentityV1 {
            environment: Environment::StellarTestnetSepolia,
            stellar_passphrase: "Test SDF Network ; September 2015".into(),
            stellar_eid: 40_600,
            stellar_endpoint: "CALTBA5S6GRJEHAXFP45LGGLKWWAF7HTZCPNUBUJF2HWWRRLQNV35AIV".into(),
            stellar_endpoint_code_hash: "1".repeat(64),
            evm_chain_id: 11_155_111,
            evm_eid: 40_161,
            evm_endpoint: "0x6EDCE65403992e310A62460808c4b910D972f10f".into(),
            evm_endpoint_code_hash: "2".repeat(64),
        },
        asset: AssetPolicyV1 {
            kind: AssetKind::NativeSac,
            asset_id: "native".into(),
            local_decimals: 7,
            issuer_custodian_evidence_sha256: None,
            destination_acceptance_evidence_sha256: None,
            custody_risk_acceptance_sha256: None,
            forbidden_classic_issuer: None,
            evidence: BTreeMap::default(),
        },
        stellar_owner: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        stellar_delegate: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".into(),
        evm_owner: "0x0000000000000000000000000000000000000001".into(),
        evm_delegate: "0x0000000000000000000000000000000000000001".into(),
        config: BTreeMap::default(),
    }
}

/// Lock whose file digests match the real in-crate build inputs; the deps
/// archive digest is caller-supplied.
fn lock_with_deps(deps_sha: &str) -> ArtifactLockV1 {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    ArtifactLockV1 {
        schema_name: "artifact_lock".into(),
        schema_version: 1,
        layerzero_source: LayerZeroSourceLockV1 {
            remote: "https://github.com/LayerZero-Labs/monorepo-external".into(),
            commit: "a877e3fb45f3a0629e8332073f9d7a41260a9b08".into(),
            source_archive_sha256: "source-unused".into(),
        },
        stellar: StellarArtifactLockV1 {
            rust_toolchain: "1.86.0".into(),
            target: "wasm32v1-none".into(),
            soroban_cli: "23.1.4".into(),
            oft_wasm_sha256: "ae7116b1f3f5e32bebc416d2441756fc79a50e3f67ab90ab96898b78c2f81ca2"
                .into(),
        },
        evm: EvmArtifactLockV1 {
            oft_evm_version: "4.0.1".into(),
            oft_evm_integrity: "sha512-oft".into(),
            oapp_evm_version: "0.4.1".into(),
            oapp_evm_integrity: "sha512-oapp".into(),
            message_lib_version: "3.0.168".into(),
            message_lib_integrity: "sha512-message-lib".into(),
            protocol_version: "3.0.168".into(),
            protocol_integrity: "sha512-protocol".into(),
            openzeppelin_version: "5.6.1".into(),
            openzeppelin_integrity: "sha512-openzeppelin".into(),
            openzeppelin_upgradeable_integrity: "sha512-openzeppelin-upgradeable".into(),
            solc: "0.8.23".into(),
            optimizer: true,
            optimizer_runs: 200,
            wrapper_source_sha256: sha256(&root.join("evm/src/DisposableOFT.sol")),
            package_json_sha256: sha256(&root.join("evm/package.json")),
            foundry_toml_sha256: sha256(&root.join("evm/foundry.toml")),
            remappings_sha256: sha256(&root.join("evm/remappings.txt")),
            build_deps_archive_sha256: deps_sha.into(),
            // The fake forge emits 0x6001/0x6002; freeze their keccaks.
            creation_bytecode_keccak256: hex::encode(templar_oft_bridge_cli::evm::keccak256_of(&[
                0x60, 0x01,
            ])),

            runtime_bytecode_keccak256: hex::encode(templar_oft_bridge_cli::evm::keccak256_of(&[
                0x60, 0x02,
            ])),
        },
    }
}
fn build_with_executor(
    state: &Path,
    out_dir: &Path,
    write: bool,
    deps_archive: Option<&Path>,
    lock: &ArtifactLockV1,
    executor: &dyn CommandExecutor,
) -> templar_oft_bridge_cli::error::Result<templar_oft_bridge_cli::output::CommandData> {
    if !write {
        return combined_build_with_executor(
            state,
            out_dir,
            false,
            deps_archive,
            None,
            lock,
            executor,
        );
    }
    let source = out_dir.with_extension("source.tgz");
    fs::write(&source, b"source")?;
    let mut lock = lock.clone();
    lock.layerzero_source.source_archive_sha256 = sha256(&source);
    let wasm = out_dir.with_extension("wasm");
    fs::write(&wasm, b"fake-stellar-wasm")?;
    lock.stellar.oft_wasm_sha256 = sha256(&wasm);
    combined_build_with_executor(
        state,
        out_dir,
        true,
        deps_archive,
        Some(&source),
        &lock,
        executor,
    )
}

/// Fake boundary: `tar` is a no-op (contents are digest-gated upstream) and
/// `forge` materializes a deterministic artifact under the `--root` work dir.
struct FakeBuild {
    calls: RefCell<Vec<String>>,
}

impl FakeBuild {
    fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl CommandExecutor for FakeBuild {
    fn run(
        &self,
        program: &str,
        args: &[String],
        _redacted_args: &[usize],
        _env: &[CommandEnv],
    ) -> templar_oft_bridge_cli::error::Result<CommandOutput> {
        self.calls
            .borrow_mut()
            .push(format!("{program} {}", args.join(" ")));
        let mut stdout = String::new();
        if program == "forge" {
            let root = args
                .iter()
                .position(|a| a == "--root")
                .and_then(|i| args.get(i + 1))
                .unwrap();
            let out = Path::new(root).join("out/DisposableOFT.sol");
            fs::create_dir_all(&out).unwrap();
            fs::write(
                out.join("DisposableOFT.json"),
                r#"{"bytecode":{"object":"0x6001"},"deployedBytecode":{"object":"0x6002"}}"#,
            )
            .unwrap();
        } else if program == "stellar" && args == ["--version"] {
            stdout = "stellar 23.1.4".into();
        } else if program == "stellar" && args.first().map(String::as_str) == Some("contract") {
            let manifest = args
                .iter()
                .position(|arg| arg == "--manifest-path")
                .and_then(|index| args.get(index + 1))
                .unwrap();
            let out = Path::new(manifest)
                .parent()
                .unwrap()
                .join("target/wasm32v1-none/release");
            fs::create_dir_all(&out).unwrap();
            fs::write(out.join("oft.wasm"), b"fake-stellar-wasm").unwrap();
        }
        Ok(CommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("route");
    let (_store, _) = RouteStore::create(&root, desired()).unwrap();
    (directory, root)
}

#[test]
fn preview_does_not_write() {
    let (directory, root) = fixture();
    let out_dir = directory.path().join("out");
    let lock = lock_with_deps("unused");
    let result =
        build_with_executor(&root, &out_dir, false, None, &lock, &FakeBuild::new()).unwrap();
    assert_eq!(result.result["preview"], true);
    assert!(!out_dir.exists());
}

#[test]
fn write_requires_deps_archive() {
    let (directory, root) = fixture();
    let lock = lock_with_deps("unused");
    let error = build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        None,
        &lock,
        &FakeBuild::new(),
    )
    .expect_err("deps archive required");
    assert!(matches!(error, Error::InvalidInput(_)));
}

#[test]
fn wrong_deps_digest_is_refused() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tar");
    fs::write(&deps, b"deps").unwrap();
    let lock = lock_with_deps(&"0".repeat(64));
    let error = build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        &lock,
        &FakeBuild::new(),
    )
    .expect_err("digest mismatch");
    assert!(matches!(error, Error::Custody(_)));
}

#[test]
fn existing_output_dir_is_refused() {
    let (directory, root) = fixture();
    let out_dir = directory.path().join("out");
    fs::create_dir(&out_dir).unwrap();
    let lock = lock_with_deps("unused");
    let error = build_with_executor(&root, &out_dir, true, None, &lock, &FakeBuild::new())
        .expect_err("existing out dir");
    assert!(matches!(error, Error::Conflict(_)));
}

#[test]
fn write_builds_and_freezes_report() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tar");
    fs::write(&deps, b"deps").unwrap();
    let lock = lock_with_deps(&sha256(&deps));
    let out_dir = directory.path().join("out");
    let build = FakeBuild::new();
    let result = build_with_executor(&root, &out_dir, true, Some(&deps), &lock, &build).unwrap();
    let mut verified_lock = lock.clone();
    verified_lock.stellar.oft_wasm_sha256 = sha256(&out_dir.with_extension("wasm"));

    let creation = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(&[0x60, 0x01]));
    let runtime = hex::encode(templar_oft_bridge_cli::evm::keccak256_of(&[0x60, 0x02]));
    assert_eq!(
        result.result["evm"]["creation_bytecode_keccak256"],
        creation
    );
    assert_eq!(result.result["evm"]["runtime_bytecode_keccak256"], runtime);
    assert_eq!(result.result["schema"], "artifact_build_report");
    assert!(out_dir.join("build-report.json").is_file());
    let stellar_path = result.result["stellar"]["path"].as_str().unwrap();
    let evm_path = result.result["evm"]["path"].as_str().unwrap();
    assert_eq!(
        fs::read(root.join(stellar_path)).unwrap(),
        b"fake-stellar-wasm"
    );
    assert!(root.join(evm_path).is_file());
    assert_eq!(
        stellar_path,
        format!(
            ".artifacts/stellar-{}.wasm",
            verified_lock.stellar.oft_wasm_sha256
        )
    );
    assert_eq!(
        evm_path,
        format!(
            ".artifacts/evm-{}.json",
            verified_lock.evm.creation_bytecode_keccak256
        )
    );
    templar_oft_bridge_cli::artifacts::verify_preserved_artifacts(&root, &verified_lock).unwrap();
    fs::write(root.join(stellar_path), b"corrupt").unwrap();
    assert!(
        templar_oft_bridge_cli::artifacts::verify_preserved_artifacts(&root, &verified_lock)
            .is_err()
    );
    let calls = build.calls.borrow();
    assert!(calls.iter().any(|c| c.starts_with("tar -xf ")));
    assert!(calls.iter().any(|c| c.starts_with("forge build --root ")));

    // The artifact build event is chained into the operation log.
    let log = fs::read_to_string(root.join("operations.jsonl")).unwrap();
    assert!(log.contains("build_artifact"));
}

#[test]
fn forge_failure_fails_closed() {
    struct FailingForge;
    impl CommandExecutor for FailingForge {
        fn run(
            &self,
            program: &str,
            _args: &[String],
            _redacted_args: &[usize],
            _env: &[CommandEnv],
        ) -> templar_oft_bridge_cli::error::Result<CommandOutput> {
            if program == "forge" {
                return Err(Error::Chain("forge exploded".into()));
            }
            Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tar");
    fs::write(&deps, b"deps").unwrap();
    let lock = lock_with_deps(&sha256(&deps));
    let error = build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        &lock,
        &FailingForge,
    )
    .expect_err("forge failure");
    assert!(error.to_string().contains("forge build failed"));
}

#[test]
fn frozen_lock_divergence_fails_closed() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tar");
    fs::write(&deps, b"deps").unwrap();
    let mut lock = lock_with_deps(&sha256(&deps));
    // Frozen lock claims a different creation hash than the build produces.
    lock.evm.creation_bytecode_keccak256 = "f".repeat(64);
    let error = build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        &lock,
        &FakeBuild::new(),
    )
    .expect_err("divergence refused");
    assert!(matches!(error, Error::Custody(_)));
    assert!(error.to_string().contains("diverges from the frozen"));
}

#[test]
fn combined_build_binds_source_archive_and_stellar_toolchain() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tgz");
    let source = directory.path().join("source.tgz");
    fs::write(&deps, b"deps").unwrap();
    fs::write(&source, b"source").unwrap();
    let mut lock = lock_with_deps(&sha256(&deps));
    lock.layerzero_source.source_archive_sha256 = sha256(&source);
    let wasm = directory.path().join("wasm");
    fs::write(&wasm, b"fake-stellar-wasm").unwrap();
    lock.stellar.oft_wasm_sha256 = sha256(&wasm);
    let out = directory.path().join("out");
    let executor = FakeBuild::new();

    let result = combined_build_with_executor(
        &root,
        &out,
        true,
        Some(&deps),
        Some(&source),
        &lock,
        &executor,
    )
    .unwrap();

    assert_eq!(
        result.result["stellar"]["source_commit"],
        lock.layerzero_source.commit
    );
    assert_eq!(
        fs::read(out.join("stellar/oft.wasm")).unwrap(),
        b"fake-stellar-wasm"
    );
    let calls = executor.calls.borrow().join("\n");
    assert!(calls.contains("stellar --version"));
    assert!(calls.contains("stellar contract build --manifest-path"));
    assert!(!calls.contains("fake-stellar-wasm"));
}

#[test]
fn combined_build_rejects_source_archive_drift_before_spawning_tools() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tgz");
    let source = directory.path().join("source.tgz");
    fs::write(&deps, b"deps").unwrap();
    fs::write(&source, b"source").unwrap();
    let mut lock = lock_with_deps(&sha256(&deps));
    lock.layerzero_source.source_archive_sha256 = "0".repeat(64);
    let executor = FakeBuild::new();

    assert!(combined_build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        Some(&source),
        &lock,
        &executor,
    )
    .is_err());
    assert!(executor.calls.borrow().is_empty());
}

#[test]
fn combined_build_rejects_stellar_cli_version_mismatch() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tgz");
    let source = directory.path().join("source.tgz");
    fs::write(&deps, b"deps").unwrap();
    fs::write(&source, b"source").unwrap();
    let mut lock = lock_with_deps(&sha256(&deps));
    lock.layerzero_source.source_archive_sha256 = sha256(&source);
    lock.stellar.soroban_cli = "25.1.1".into();

    let error = combined_build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        Some(&source),
        &lock,
        &FakeBuild::new(),
    )
    .expect_err("version mismatch");
    assert!(error.to_string().contains("version mismatch"));
}

#[test]
fn combined_build_rejects_one_byte_stellar_wasm_drift() {
    let (directory, root) = fixture();
    let deps = directory.path().join("deps.tgz");
    let source = directory.path().join("source.tgz");
    fs::write(&deps, b"deps").unwrap();
    fs::write(&source, b"source").unwrap();
    let mut lock = lock_with_deps(&sha256(&deps));
    lock.layerzero_source.source_archive_sha256 = sha256(&source);
    lock.stellar.oft_wasm_sha256 = sha256(&source);

    let error = combined_build_with_executor(
        &root,
        &directory.path().join("out"),
        true,
        Some(&deps),
        Some(&source),
        &lock,
        &FakeBuild::new(),
    )
    .expect_err("WASM drift");
    assert!(error.to_string().contains("hash mismatch"), "{error}");
}
