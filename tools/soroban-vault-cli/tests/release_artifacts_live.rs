//! Opt-in live verification of the fixed Soroban release inventory.
//!
//! This test is intentionally ignored: it downloads the seven fixed GitHub release assets and
//! compares their byte length and SHA-256 against the reviewed pins. It never talks to Stellar and
//! never mutates repository or cache state. Run it explicitly with:
//!
//! ```text
//! cargo test -p templar-soroban-vault-cli --test release_artifacts_live \
//!   release_assets_match_pins -- --ignored --exact --nocapture
//! ```
//!
//! The download uses `curl`, matching the operator image's existing transport tool.

use std::process::Command;

use sha2::{Digest, Sha256};
use templar_soroban_vault_cli::artifacts::{ReleaseArtifact, RELEASE_REPO, RELEASE_TAG};

fn download(url: &str) -> Vec<u8> {
    let output = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--connect-timeout", "15", url])
        .output()
        .unwrap_or_else(|error| panic!("run curl for {url}: {error}"));
    assert!(
        output.status.success(),
        "download {url} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
#[ignore = "requires network access to the fixed GitHub release URL"]
fn release_assets_match_pins() {
    for release in ReleaseArtifact::ALL {
        let asset = release.wasm_file_name();
        let pin = release.pin();
        let url =
            format!("https://github.com/{RELEASE_REPO}/releases/download/{RELEASE_TAG}/{asset}");
        let body = download(&url);
        assert_eq!(body.len(), pin.length, "byte length mismatch for {asset}");
        let digest = hex::encode(Sha256::digest(&body));
        assert_eq!(digest, pin.sha256, "SHA-256 mismatch for {asset}");
    }
}
