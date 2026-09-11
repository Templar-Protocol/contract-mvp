use super::*;

use crate::artifacts::ExistingReleaseArtifact;
use crate::commands::doctor::{artifact_check, artifact_doctor_checks};

#[test]
fn doctor_checks_stellar_and_source_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("write Cargo.toml");
    let config_dir = dir.path().join("stellar-config");
    let cli = Cli {
        workspace_path: dir.path().into(),
        config_dir: Some(config_dir.clone()),
        command: Commands::Doctor,
        ..base_cli(dir.path().join("manifest.json"), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("run doctor");

    let calls = executor.calls();
    assert!(calls
        .iter()
        .any(|(_, args)| args == &["--version".to_string()]));
    assert!(calls.iter().any(|(_, args)| args
        == &[
            "keys".to_string(),
            "address".to_string(),
            "alice".to_string(),
            "--config-dir".to_string(),
            config_dir.display().to_string(),
        ]));
}

#[test]
fn manifest_writable_probe_ignores_a_stale_pid_only_probe_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stale_probe = dir.path().join(format!(
        ".tmplr-soroban-vault-cli-write-test-{}",
        std::process::id()
    ));
    fs::write(&stale_probe, "stale").expect("write stale probe");

    let check = manifest_writable_check(&dir.path().join("manifest.json"));

    assert_eq!(check.status, DoctorStatus::Pass);
    assert!(stale_probe.exists());
}

#[test]
fn artifact_check_reports_a_verified_cache_entry_as_pass() {
    let spec = ArtifactSpec::from_name(ArtifactName::ShareToken);
    let existing = ExistingReleaseArtifact::Cache {
        path: std::path::PathBuf::from("/cache/soroban-v1.1.1/templar_soroban_share_token.wasm"),
        sha256: "abc123".to_string(),
    };

    let check = artifact_check(&spec, &existing);

    assert_eq!(check.status, DoctorStatus::Pass);
    assert!(check
        .message
        .contains("/cache/soroban-v1.1.1/templar_soroban_share_token.wasm"));
    assert!(check.message.contains("sha256 abc123"));
}

#[test]
fn artifact_check_reports_a_verified_workspace_seed_as_pass() {
    let spec = ArtifactSpec::from_name(ArtifactName::ShareToken);
    let existing = ExistingReleaseArtifact::WorkspaceSeed {
        path: std::path::PathBuf::from(
            "/workspace/target/wasm32-unknown-unknown/release-soroban/templar_soroban_share_token.wasm",
        ),
        sha256: "abc123".to_string(),
    };

    let check = artifact_check(&spec, &existing);

    assert_eq!(check.status, DoctorStatus::Pass);
    assert!(check.message.contains("sha256 abc123"));
}

#[test]
fn artifact_check_warns_when_the_release_wasm_is_unresolved() {
    let spec = ArtifactSpec::from_name(ArtifactName::ShareToken);
    let existing = ExistingReleaseArtifact::Missing {
        cache_path: std::path::PathBuf::from(
            "/cache/soroban-v1.1.1/templar_soroban_share_token.wasm",
        ),
    };

    let check = artifact_check(&spec, &existing);

    assert_eq!(check.status, DoctorStatus::Warn);
    assert!(check.message.contains("is not cached"));
    assert!(check.message.contains("downloads the pinned release"));
    assert!(check.message.contains(spec.package));
}

#[test]
fn artifact_check_warns_when_workspace_bytes_do_not_match_release() {
    let spec = ArtifactSpec::from_name(ArtifactName::ShareToken);
    let existing = ExistingReleaseArtifact::IgnoredWorkspace {
        cache_path: std::path::PathBuf::from(
            "/cache/soroban-v1.1.1/templar_soroban_share_token.wasm",
        ),
        workspace_path: std::path::PathBuf::from(
            "/workspace/target/wasm32-unknown-unknown/release-soroban/templar_soroban_share_token.wasm",
        ),
        reason: "has 5 bytes but release pins 11816".to_string(),
    };

    let check = artifact_check(&spec, &existing);

    assert_eq!(check.status, DoctorStatus::Warn);
    assert!(check.message.contains("will be ignored"));
    assert!(check.message.contains("has 5 bytes"));
    assert!(check.message.contains("soroban-v1.1.1"));
}

#[test]
fn artifact_checks_fail_on_a_corrupt_cache_entry_without_building_or_residue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_root = dir.path().join("cache");
    let entry = cache_root
        .join("soroban-v1.1.1")
        .join("templar_soroban_share_token.wasm");
    fs::create_dir_all(entry.parent().expect("parent")).expect("create cache dir");
    fs::write(&entry, "short").expect("write corrupt entry");
    let _cache_env = CacheEnvGuard::set(&cache_root);
    let workspace = tempfile::tempdir().expect("tempdir");
    fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").expect("write manifest");
    let cli = Cli {
        workspace_path: workspace.path().into(),
        command: Commands::Doctor,
        ..base_cli(workspace.path().join("manifest.json"), Commands::Status)
    };

    let checks = artifact_doctor_checks(&cli);

    let failures: Vec<_> = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "exactly the corrupt entry fails: {checks:?}"
    );
    assert_eq!(failures[0].name, "artifact_share_token");
    assert!(failures[0].message.contains("from cache entry"));
    // Read-only probe: no build outputs created under the workspace.
    assert!(
        !workspace.path().join("target").exists(),
        "probe created build outputs"
    );
}

#[test]
fn artifact_checks_warn_without_failing_when_nothing_is_cached() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache_root = dir.path().join("cache");
    let _cache_env = CacheEnvGuard::set(&cache_root);
    let workspace = tempfile::tempdir().expect("tempdir");
    fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").expect("write manifest");
    let cli = Cli {
        workspace_path: workspace.path().into(),
        command: Commands::Doctor,
        ..base_cli(workspace.path().join("manifest.json"), Commands::Status)
    };

    let checks = artifact_doctor_checks(&cli);

    assert_eq!(
        checks.len(),
        ArtifactSpec::stack_artifacts(true, true).len()
    );
    assert!(checks
        .iter()
        .all(|check| check.status == DoctorStatus::Warn));
    assert!(checks
        .iter()
        .all(|check| check.message.contains("downloads the pinned release")));
    // Zero residue: the probe created neither the cache root nor build outputs.
    assert!(!cache_root.exists(), "probe created the cache root");
    assert!(
        !workspace.path().join("target").exists(),
        "probe created build outputs"
    );
}
