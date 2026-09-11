use super::*;

#[test]
fn deploy_adapters_appends_new_pool_to_existing_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_blend_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("asset_token".to_string(), imported_record(CONTRACT));
    manifest.contracts.insert(
        "blend_adapter_0".to_string(),
        ContractRecord {
            contract_id: CONTRACT.to_string(),
            wasm_hash: "hash".to_string(),
            salt: None,
            constructor_args: map_args([("pool", CONTRACT)]),
            deploy_tx: None,
            initialized: true,
        },
    );
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                vault: None,
                governance: None,
                asset_token: Some(CONTRACT.parse().expect("asset token")),
                blend_pools: vec![
                    CONTRACT.parse().expect("existing pool"),
                    ASSET_CONTRACT.parse().expect("new pool"),
                ],
                custodians: Vec::new(),
                adapter_admin: OTHER_CONTRACT.parse().expect("adapter admin"),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy adapters");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(loaded.contracts.contains_key("blend_adapter_0"));
    assert_eq!(
        loaded
            .contracts
            .get("blend_adapter_1")
            .expect("appended adapter")
            .constructor_args
            .get("pool")
            .map(String::as_str),
        Some(ASSET_CONTRACT)
    );
    assert_eq!(
        loaded
            .contracts
            .get("blend_adapter_1")
            .expect("appended adapter")
            .constructor_args
            .get("admin")
            .map(String::as_str),
        Some(OTHER_CONTRACT)
    );
    let adapter_deploys = submitted_calls(&executor.calls())
        .iter()
        .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
        .count();
    assert_eq!(adapter_deploys, 1);
}

#[test]
fn deploy_adapters_accepts_account_blend_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_blend_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                vault: None,
                governance: None,
                asset_token: None,
                blend_pools: vec![CONTRACT.parse().expect("pool")],
                custodians: Vec::new(),
                adapter_admin: ACCOUNT.parse().expect("adapter admin"),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("account admin should be accepted");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert_eq!(
        loaded
            .contracts
            .get("blend_adapter_0")
            .expect("deployed Blend adapter")
            .constructor_args
            .get("admin")
            .map(String::as_str),
        Some(ACCOUNT)
    );
    assert!(submitted_calls(&executor.calls())
        .iter()
        .any(|(_, args)| args
            .windows(2)
            .any(|window| window[0] == "--admin" && window[1] == ACCOUNT)));
}

#[test]
fn adapter_deploy_and_plan_reject_governance_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
    manifest.save(&state).expect("save manifest");

    for plan in [false, true] {
        let adapters = crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: vec![ASSET_CONTRACT.parse().expect("pool")],
            custodians: Vec::new(),
            adapter_admin: OTHER_CONTRACT.parse().expect("adapter admin"),
            build: false,
            force_new: false,
        };
        let command = if plan {
            DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Adapters(adapters),
            })
        } else {
            DeployCommand::Adapters(adapters)
        };
        let cli = Cli {
            command: Commands::Deploy(DeployArgs { command }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        let error = run(&cli, &executor).expect_err("governance admin must be rejected");

        assert!(error
            .to_string()
            .contains("adapter admin must differ from the governance contract"));
        assert!(executor.calls().is_empty());
    }
}

#[test]
fn force_new_stack_deploy_and_plan_reject_recorded_governance_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
    manifest.save(&state).expect("save manifest");

    for plan in [false, true] {
        let mut stack = test_deploy_stack_args(ACCOUNT);
        stack.blend_pools = vec![ASSET_CONTRACT.parse().expect("pool")];
        stack.adapter_admin = Some(OTHER_CONTRACT.parse().expect("adapter admin"));
        stack.force_new = true;
        let command = if plan {
            DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(stack)),
            })
        } else {
            DeployCommand::Stack(Box::new(stack))
        };
        let cli = Cli {
            command: Commands::Deploy(DeployArgs { command }),
            ..base_cli(state.clone(), Commands::Status)
        };
        let executor = RecordingExecutor::new();

        let error = run(&cli, &executor).expect_err("governance admin must be rejected");

        assert!(error
            .to_string()
            .contains("adapter admin must differ from the governance contract"));
        assert!(executor.calls().is_empty());
    }
}

#[test]
fn deploy_adapters_appends_custodial_adapter_to_existing_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_custodial_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("asset_token".to_string(), imported_record(CONTRACT));
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                vault: None,
                governance: None,
                asset_token: None,
                blend_pools: Vec::new(),
                custodians: vec![
                    ACCOUNT.parse().expect("custodian"),
                    ACCOUNT.parse().expect("duplicate custodian"),
                ],
                adapter_admin: ACCOUNT.parse().expect("adapter admin"),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy custodial adapter");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(!loaded.contracts.contains_key("custodial_adapter_1"));
    let adapter = loaded
        .contracts
        .get("custodial_adapter_0")
        .expect("appended custodial adapter");
    assert_eq!(
        adapter
            .constructor_args
            .get("custodian")
            .map(String::as_str),
        Some(ACCOUNT)
    );
    assert_eq!(
        adapter.constructor_args.get("vault").map(String::as_str),
        Some(CONTRACT)
    );
    assert_eq!(
        adapter.constructor_args.get("admin").map(String::as_str),
        Some(ACCOUNT)
    );
    assert_eq!(
        adapter.constructor_args.get("asset").map(String::as_str),
        Some(CONTRACT)
    );
    let calls = submitted_calls(&executor.calls());
    let adapter_deploys = calls
        .iter()
        .filter(|(_, args)| args.iter().any(|arg| arg == "--custodian"))
        .count();
    assert_eq!(adapter_deploys, 1);
    assert!(calls.iter().any(|(_, args)| args
        .windows(2)
        .any(|window| window[0] == "--asset" && window[1] == CONTRACT)));
}

#[test]
fn dry_run_deploy_adapters_does_not_execute_or_write_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let before = fs::read_to_string(&state).expect("read manifest");
    let cache = dir.path().join("release-cache");
    let _cache_env = CacheEnvGuard::set(&cache);
    let cli = Cli {
        workspace_path: dir.path().into(),
        dry_run: true,
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                vault: None,
                governance: None,
                asset_token: None,
                blend_pools: vec![CONTRACT.parse().expect("pool")],
                custodians: Vec::new(),
                adapter_admin: OTHER_CONTRACT.parse().expect("adapter admin"),
                build: false,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("dry-run deploy adapters");

    assert!(executor.calls().is_empty());
    let after = fs::read_to_string(&state).expect("read manifest");
    assert_eq!(before, after);
    assert!(!cache.exists(), "dry-run must not create the release cache");
    assert!(
        !dir.path().join("target").exists(),
        "dry-run must not create build outputs"
    );
}

#[test]
fn deploy_stack_requires_explicit_admin_when_adapters_are_requested() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Stack(Box::new(DeployStackArgs {
                admin: Some(ACCOUNT.parse().expect("admin")),
                asset_token: Some(CONTRACT.parse().expect("asset token")),
                governance_timelock_ns: Some(1_000),
                virtual_shares: 0,
                virtual_assets: 0,
                share_name: "Templar Vault Share".to_string(),
                share_symbol: "tvSHARE".to_string(),
                share_decimals: 7,
                blend_pools: vec![CONTRACT.parse().expect("pool")],
                custodians: Vec::new(),
                adapter_admin: None,
                build: false,
                force_new: false,
            })),
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("adapter admin must be explicit");

    assert!(error.to_string().contains("explicit --adapter-admin"));
    assert!(executor.calls().is_empty());
}

#[test]
fn deploy_adapters_rejects_vault_admin_without_companion_upgrade_capability() {
    for adapter_admin in ["vault", CONTRACT] {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join("manifest.json");
        let mut manifest = Manifest::new("testnet", None);
        manifest
            .contracts
            .insert("vault".to_string(), imported_record(CONTRACT));
        manifest
            .contracts
            .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
        manifest
            .contracts
            .insert("asset_token".to_string(), imported_record(ASSET_CONTRACT));
        let mut blend = imported_record(OTHER_CONTRACT);
        blend.constructor_args = map_args([("pool", CONTRACT)]);
        manifest.contracts.insert("blend_adapter_0".into(), blend);
        let mut custodial = imported_record(OTHER_CONTRACT);
        custodial.constructor_args = map_args([("custodian", ACCOUNT), ("asset", ASSET_CONTRACT)]);
        manifest
            .contracts
            .insert("custodial_adapter_0".into(), custodial);
        manifest.save(&state).expect("save manifest");
        let cli = Cli {
            command: Commands::Deploy(DeployArgs {
                command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                    vault: None,
                    governance: None,
                    asset_token: None,
                    blend_pools: vec![CONTRACT.parse().expect("pool")],
                    custodians: vec![ACCOUNT.parse().expect("custodian")],
                    adapter_admin: adapter_admin.parse().expect("adapter admin"),
                    build: false,
                    force_new: false,
                }),
            }),
            ..base_cli(state, Commands::Status)
        };
        let executor = RecordingExecutor::new();

        let error = run(&cli, &executor).expect_err("vault admin must be capability-gated");

        assert!(error.to_string().contains("companion-upgrade capability"));
        let calls = executor.calls();
        assert!(calls.iter().any(|(_, args)| {
            args.iter().any(|arg| arg == "version")
                && args.windows(2).any(|pair| pair == ["--send", "no"])
        }));
        assert!(!calls.iter().any(|(_, args)| {
            args.iter()
                .any(|arg| arg == "--pool" || arg == "--custodian")
        }));
    }
}

#[test]
fn deploy_adapters_allows_vault_admin_after_companion_upgrade_detection() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_blend_wasm(dir.path());
    write_fake_custodial_wasm(dir.path());
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
    manifest
        .contracts
        .insert("asset_token".to_string(), imported_record(ASSET_CONTRACT));
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Adapters(crate::cli::DeployAdaptersArgs {
                vault: None,
                governance: None,
                asset_token: None,
                blend_pools: vec![CONTRACT.parse().expect("pool")],
                custodians: vec![ACCOUNT.parse().expect("custodian")],
                adapter_admin: "vault".parse().expect("vault adapter admin"),
                build: true,
                force_new: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor =
        RecordingExecutor::with_runtime_feature_flags(0x1f | RUNTIME_FEATURE_COMPANION_UPGRADE);

    run(&cli, &executor).expect("deploy adapters with vault admin");

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    for key in ["blend_adapter_0", "custodial_adapter_0"] {
        assert_eq!(
            loaded
                .contracts
                .get(key)
                .expect("adapter record")
                .constructor_args
                .get("admin")
                .map(String::as_str),
            Some(CONTRACT)
        );
    }
    let calls = executor.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, args)| args.iter().any(|arg| arg == "version"))
            .count(),
        1
    );
    let version_index = calls
        .iter()
        .position(|(_, args)| args.iter().any(|arg| arg == "version"))
        .expect("runtime version query");
    for constructor_flag in ["--pool", "--custodian"] {
        let deploy_index = calls
            .iter()
            .position(|(_, args)| args.iter().any(|arg| arg == constructor_flag))
            .expect("adapter deployment");
        assert!(version_index < deploy_index);
    }
}

#[test]
fn runtime_version_parser_requires_typed_version_and_feature_mask() {
    assert_eq!(
        parse_runtime_version("[\"1.2.3\",64]").expect("runtime version"),
        ("1.2.3".to_string(), 64)
    );
    assert!(parse_runtime_version(CONTRACT).is_err());
    assert!(parse_runtime_version("[\"1.2.3\"]").is_err());
}

#[test]
fn adapter_admin_validation_accepts_accounts_and_rejects_collisions() {
    let account_admin = ACCOUNT.parse().expect("account admin");
    let asset_admin = CONTRACT.parse().expect("asset admin");
    let governance_admin = OTHER_CONTRACT.parse().expect("governance admin");

    assert!(validate_adapter_admin(&account_admin, None, None, None).is_ok());
    assert!(
        validate_adapter_admin(&asset_admin, Some(OTHER_CONTRACT), None, Some(CONTRACT),).is_err()
    );
    assert!(validate_adapter_admin(
        &governance_admin,
        Some(CONTRACT),
        Some(OTHER_CONTRACT),
        None,
    )
    .is_err());
}
