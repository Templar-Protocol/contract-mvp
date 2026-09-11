use std::{fs, process::Command};

use serde_json::{json, Value};

const ACCOUNT: &str = "GBRFSXJNPLMYJV7EBFTBZT2PU6KN5WWPX3UKHDAAQQT7BNS7QTFCS3AY";
const VAULT: &str = "CDY3B7IXFN5L4OY4UFFS2FA4MAQWJZLJD76LW37S7HFVWRS3RPQ2SIXX";

#[test]
fn json_dry_run_keeps_stdout_machine_readable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("manifest.json");
    fs::write(
        &manifest,
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "network": "testnet",
            "artifacts": {},
            "contracts": {
                "vault": {
                    "contract_id": VAULT,
                    "wasm_hash": "predeployed",
                    "constructor_args": {},
                    "initialized": true
                }
            },
            "transactions": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_tmplr-soroban-vault"))
        .args([
            "--state",
            manifest.to_str().expect("manifest path"),
            "--source-account",
            "alice",
            "--json",
            "--dry-run",
            "user",
            "deposit",
            "--operator",
            ACCOUNT,
            "--assets-raw",
            "1",
        ])
        .output()
        .expect("run CLI");

    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["type"], "command");
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert!(stderr.contains("dry-run:"));
    assert!(stderr.contains("stellar contract invoke"));
}

#[test]
fn json_deploy_dry_run_reports_release_intent_without_residue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = dir.path().join("state/manifest.json");
    let workspace = dir.path().join("workspace");
    let cache = dir.path().join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_tmplr-soroban-vault"))
        .env("TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE", &cache)
        .args([
            "--state",
            manifest.to_str().expect("manifest path"),
            "--workspace-path",
            workspace.to_str().expect("workspace path"),
            "--json",
            "--dry-run",
            "deploy",
            "wasm",
            "vault",
        ])
        .output()
        .expect("run CLI");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout must be JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["type"], "plan");
    let wasm = &value["data"]["wasm"][0];
    assert_eq!(wasm["key"], "vault");
    assert!(wasm["local_hash"].is_null());
    assert!(wasm["action"]
        .as_str()
        .expect("action")
        .contains("download pinned release soroban-v1.1.1"));
    assert!(!manifest.exists(), "dry-run created manifest");
    assert!(!cache.exists(), "dry-run created cache");
    assert!(
        !workspace.exists(),
        "dry-run created workspace/build directories"
    );
}
