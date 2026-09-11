use std::fs;

use templar_oft_bridge_cli::config::{public_origin, read_headers_file, SecretProvider};

#[test]
fn rpc_file_provider_reads_mode_0600_and_strips_secret_origin_parts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rpc");
    fs::write(
        &path,
        "https://user:secret@example.test:8443/private?api_key=secret\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    let value = SecretProvider::File(path).read().unwrap();
    assert_eq!(public_origin(&value).unwrap(), "https://example.test:8443");
}

#[cfg(unix)]
#[test]
fn rpc_file_provider_rejects_insecure_mode_and_symlink() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("rpc");
    fs::write(&path, "https://example.test").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(SecretProvider::File(path.clone()).read().is_err());

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let link = directory.path().join("rpc-link");
    symlink(&path, &link).unwrap();
    assert!(SecretProvider::File(link).read().is_err());
}

#[cfg(unix)]
#[test]
fn rpc_headers_are_string_only_and_kept_in_zeroizing_values() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("headers.json");
    fs::write(
        &path,
        r#"{"Authorization":"Bearer secret","X-Api-Key":"secret"}"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let headers = read_headers_file(&path).unwrap();
    assert_eq!(headers.len(), 2);
    assert!(headers
        .iter()
        .any(|(name, value)| name == "Authorization" && value.as_str() == "Bearer secret"));

    fs::write(&path, r#"{"Authorization":7}"#).unwrap();
    assert!(read_headers_file(&path).is_err());
}
