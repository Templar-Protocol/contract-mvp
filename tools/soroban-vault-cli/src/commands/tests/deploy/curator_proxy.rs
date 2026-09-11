use super::*;

#[test]
fn targeted_curator_proxy_deploy_uses_standard_initializer_and_verifies_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: None,
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy curator proxy");

    let calls = executor.calls();
    assert!(calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
            && args.windows(2).any(|pair| {
                pair == ["--initialization_authority", ACCOUNT]
            })
    }));
    assert!(calls.iter().any(|(_, args)| {
        args.iter().any(|arg| arg == "initialize")
            && args
                .windows(2)
                .any(|pair| pair == ["--vault_address", CONTRACT])
            && args
                .windows(2)
                .any(|pair| pair == ["--governance_address", CONTRACT])
    }));
    assert!(calls.iter().any(|(_, args)| {
        args.iter().any(|arg| arg == "vault_version")
            && args.windows(2).any(|pair| pair == ["--send", "no"])
    }));

    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("curator proxy record");
    assert!(proxy.initialized);
    assert_eq!(
        proxy
            .constructor_args
            .get(CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG),
        Some(&ACCOUNT.to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_INITIALIZER_ARG),
        Some(&"initialize".to_string())
    );
    assert_eq!(
        proxy
            .constructor_args
            .get(CURATOR_PROXY_VERSION_DISCOVERY_ARG),
        Some(&"true".to_string())
    );
    assert!(!proxy
        .constructor_args
        .contains_key(CURATOR_PROXY_LEGACY_V1_HASH_ARG));
}

#[test]
fn targeted_curator_proxy_reuses_fully_verified_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: None,
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state, Commands::Status)
    };
    run(&cli, &RecordingExecutor::new()).expect("deploy curator proxy");

    let retry_executor = RecordingExecutor::new();
    run(&cli, &retry_executor).expect("reuse verified curator proxy");

    let retry_calls = retry_executor.calls();
    assert!(!retry_calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
    }));
    assert!(!retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "initialize")));
    assert!(retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));
}

#[test]
fn targeted_curator_proxy_records_verified_imported_targets_without_provenance() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: None,
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    run(&cli, &TtlRecordingExecutor::new()).expect("seed initialized curator proxy");

    let mut imported = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let proxy = imported
        .contracts
        .get_mut("curator_proxy")
        .expect("curator proxy record");
    proxy.constructor_args.clear();
    imported.save(&state).expect("save imported manifest");

    let legacy_hash = "11".repeat(32);
    let retry_cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: Some(legacy_hash.parse().expect("legacy v1 Wasm hash")),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let retry_executor = TtlRecordingExecutor::new();
    run(&retry_cli, &retry_executor).expect("record imported curator proxy targets");

    let retry_calls = retry_executor.calls();
    assert!(!retry_calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
    }));
    assert!(!retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "initialize")));
    for function in ["vault", "governance", "vault_version"] {
        assert!(retry_calls
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == function)));
    }
    let backfilled = Manifest::load_or_new(&state, "testnet", None).expect("load backfill");
    let proxy = backfilled
        .contracts
        .get("curator_proxy")
        .expect("backfilled curator proxy");
    assert!(!proxy
        .constructor_args
        .contains_key(CURATOR_PROXY_INITIALIZER_ARG));
    assert!(!proxy
        .constructor_args
        .contains_key(CURATOR_PROXY_LEGACY_V1_HASH_ARG));
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_VAULT_ARG),
        Some(&CONTRACT.to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_GOVERNANCE_ARG),
        Some(&CONTRACT.to_string())
    );
    assert!(curator_proxy_supports_version_discovery(proxy));
}

#[test]
fn targeted_curator_proxy_deploy_imports_targets_and_uses_legacy_initializer() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    let legacy_hash = format!("{:x}", Sha256::digest(CONTRACT.as_bytes()));
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: Some(CONTRACT.parse().expect("vault")),
                governance: Some(CONTRACT.parse().expect("governance")),
                legacy_v1_wasm_hash: Some(legacy_hash.parse().expect("legacy hash")),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = TtlRecordingExecutor::new();

    run(&cli, &executor).expect("deploy legacy curator proxy");

    let calls = executor.calls();
    assert!(calls.iter().any(|(_, args)| {
        args.iter().any(|arg| arg == "initialize_legacy_v1")
            && args
                .windows(2)
                .any(|pair| pair == ["--legacy_v1_wasm_hash", legacy_hash.as_str()])
    }));
    assert!(calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));

    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(manifest.contracts.contains_key("vault"));
    assert!(manifest.contracts.contains_key("governance"));
    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("curator proxy record");
    assert_eq!(
        proxy
            .constructor_args
            .get(CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG),
        Some(&ACCOUNT.to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_INITIALIZER_ARG),
        Some(&"initialize_legacy_v1".to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_LEGACY_V1_HASH_ARG),
        Some(&legacy_hash)
    );
}

#[test]
fn targeted_legacy_curator_proxy_rejects_wrong_current_vault_hash_before_deploy() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: Some("11".repeat(32).parse().expect("legacy hash")),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = TtlRecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("legacy hash mismatch must fail");

    assert!(error.to_string().contains("legacy v1 WASM hash mismatch"));
    assert!(!executor.calls().iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
    }));
}

#[test]
fn targeted_curator_proxy_checkpoint_stays_uninitialized_when_initialize_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: None,
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = FailingInitializeExecutor::new();

    let error = run(&cli, &executor).expect_err("initialize must fail");

    assert!(
        error.to_string().contains("forced initialize failure")
            || error.to_string().contains("preflight simulation failed")
    );
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("deployed proxy must be checkpointed");
    assert!(!proxy.initialized);
    assert_eq!(
        proxy
            .constructor_args
            .get(CURATOR_PROXY_INITIALIZATION_AUTHORITY_ARG),
        Some(&ACCOUNT.to_string())
    );
    assert!(!proxy
        .constructor_args
        .contains_key(CURATOR_PROXY_INITIALIZER_ARG));

    let retry_executor = RecordingExecutor::new();
    run(&cli, &retry_executor).expect("retry checkpointed curator proxy");
    let retry_calls = retry_executor.calls();
    assert!(!retry_calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
    }));
    assert!(retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "initialize")));
    let retried = Manifest::load_or_new(&state, "testnet", None).expect("load retried manifest");
    let proxy = retried
        .contracts
        .get("curator_proxy")
        .expect("retried curator proxy");
    assert!(proxy.initialized);
    assert!(curator_proxy_supports_version_discovery(proxy));
}

#[test]
fn targeted_curator_proxy_preserves_initialization_provenance_when_version_check_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: None,
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = FailingVaultVersionExecutor::new();

    let error = run(&cli, &executor).expect_err("vault version check must fail");

    assert!(error.to_string().contains("forced vault_version failure"));
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("curator proxy record");
    assert!(proxy.initialized);
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_INITIALIZER_ARG),
        Some(&"initialize".to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_VAULT_ARG),
        Some(&CONTRACT.to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_GOVERNANCE_ARG),
        Some(&CONTRACT.to_string())
    );
    assert!(!curator_proxy_supports_version_discovery(proxy));

    let retry_executor = RecordingExecutor::new();
    run(&cli, &retry_executor).expect("retry curator proxy version verification");
    let retry_calls = retry_executor.calls();
    assert!(!retry_calls.iter().any(|(_, args)| {
        matches!(args.as_slice(), [contract, deploy, ..] if contract == "contract" && deploy == "deploy")
    }));
    assert!(!retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "initialize")));
    assert!(retry_calls
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "vault_version")));
    let retried = Manifest::load_or_new(&state, "testnet", None).expect("load retried manifest");
    assert!(curator_proxy_supports_version_discovery(
        retried
            .contracts
            .get("curator_proxy")
            .expect("retried curator proxy")
    ));
}

#[test]
fn targeted_legacy_proxy_preserves_pinned_hash_when_version_check_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_curator_proxy_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let legacy_hash = format!("{:x}", Sha256::digest(CONTRACT.as_bytes()));
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::CuratorProxy(DeployCuratorProxyArgs {
                vault: None,
                governance: None,
                legacy_v1_wasm_hash: Some(legacy_hash.parse().expect("legacy v1 Wasm hash")),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = FailingVaultVersionExecutor::new();

    run(&cli, &executor).expect_err("vault version check must fail");

    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let proxy = manifest
        .contracts
        .get("curator_proxy")
        .expect("curator proxy record");
    assert!(proxy.initialized);
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_INITIALIZER_ARG),
        Some(&"initialize_legacy_v1".to_string())
    );
    assert_eq!(
        proxy.constructor_args.get(CURATOR_PROXY_LEGACY_V1_HASH_ARG),
        Some(&legacy_hash)
    );
    assert!(!curator_proxy_supports_version_discovery(proxy));
}
