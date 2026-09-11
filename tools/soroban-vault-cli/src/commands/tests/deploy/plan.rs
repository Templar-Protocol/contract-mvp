use super::*;

#[test]
fn deploy_plan_does_not_execute_or_write_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = dir.path().join("cache");
    let _cache_env = CacheEnvGuard::set(&cache);
    write_fake_stack_wasms(dir.path());
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let before = fs::read_to_string(&state).expect("read manifest");
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: crate::cli::DeployPlanCommand::Stack(Box::new(DeployStackArgs {
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
                    adapter_admin: Some(OTHER_CONTRACT.parse().expect("adapter admin")),
                    build: false,
                    force_new: false,
                })),
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy plan");

    assert!(executor.calls().is_empty());
    let after = fs::read_to_string(&state).expect("read manifest");
    assert_eq!(before, after);
}

#[test]
fn repair_dry_run_does_not_query_stellar_or_write_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let cli = Cli {
        dry_run: true,
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Repair(crate::cli::ReconcileArgs {
                skip_view_verification: false,
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("dry repair plan");
    assert!(executor.calls().is_empty());
    assert!(!state.exists());
}

#[test]
fn wasm_plan_distinguishes_cache_workspace_seed_and_build_sources() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = dir.path().join("cache");
    let _cache_env = CacheEnvGuard::set(&cache);
    let state = dir.path().join("manifest.json");
    let cli = base_cli(state, Commands::Status);
    let manifest = Manifest::new("testnet", None);
    let spec = ArtifactSpec::from_name(ArtifactName::Vault);
    let pin = spec.release.pin();

    let missing = wasm_plan(&cli, &manifest, spec, false).expect("missing plan");
    assert!(missing.local_hash.is_none());
    assert!(missing
        .action
        .contains("download pinned release soroban-v1.1.1"));

    let workspace_path = spec.wasm_path(&cli.workspace_path);
    fs::create_dir_all(workspace_path.parent().expect("parent")).expect("create workspace path");
    fs::write(&workspace_path, "wrong bytes").expect("write mismatching workspace");
    let ignored = wasm_plan(&cli, &manifest, spec, false).expect("ignored workspace plan");
    assert!(ignored.local_hash.is_none());
    assert!(ignored.action.contains("ignore unreleased workspace bytes"));

    fs::remove_file(&workspace_path).expect("remove mismatch");
    let cache_path = cache
        .join(crate::artifacts::RELEASE_TAG)
        .join(spec.release.wasm_file_name());
    fs::create_dir_all(cache_path.parent().expect("cache parent")).expect("create cache path");
    fs::write(&cache_path, vec![0; pin.length]).expect("write invalid cache length-only fixture");
    let error = wasm_plan(&cli, &manifest, spec, false).expect_err("invalid cache must fail");
    assert!(error.to_string().contains("hashes to"));

    fs::remove_dir_all(&cache).expect("remove invalid cache");
    let build = wasm_plan(&cli, &manifest, spec, true).expect("build plan");
    assert!(build.local_hash.is_none());
    assert!(build.action.contains("checked-out workspace"));
}

#[test]
fn force_new_stack_plan_requires_explicit_governance_timelock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    manifest_with_governance_and_vault(&state);
    let mut stack = test_deploy_stack_args(ACCOUNT);
    stack.force_new = true;
    stack.governance_timelock_ns = None;
    let cli = base_cli(
        state,
        Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(stack)),
            }),
        }),
    );
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("invalid force-new plan must fail");

    assert!(error
        .to_string()
        .contains("new governance deployment requires --governance-timelock-ns"));
    assert!(executor.calls().is_empty());
}

#[test]
fn deploy_stack_plan_rejects_conflicting_asset_token_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("asset_token".to_string(), imported_record(ASSET_CONTRACT));
    manifest.save(&state).expect("save manifest");
    let before = fs::read_to_string(&state).expect("read manifest before plan");
    let mut args = test_deploy_stack_args(ACCOUNT);
    args.asset_token = Some(CONTRACT.parse().expect("conflicting asset token"));
    let cli = base_cli(
        state.clone(),
        Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(args)),
            }),
        }),
    );
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("conflicting asset token must fail planning");

    assert!(error.to_string().contains(&format!(
        "asset_token already recorded as {ASSET_CONTRACT}; refusing to overwrite with {CONTRACT}"
    )));
    assert!(executor.calls().is_empty());
    let after = fs::read_to_string(&state).expect("read manifest after plan");
    assert_eq!(before, after);
}

#[test]
fn deploy_adapters_plan_rejects_conflicting_import_ids() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    for (key, contract_id) in [
        ("vault", CONTRACT),
        ("governance", OTHER_CONTRACT),
        ("asset_token", ASSET_CONTRACT),
    ] {
        manifest
            .contracts
            .insert(key.to_string(), imported_record(contract_id));
    }
    let cli = base_cli(state, Commands::Status);
    let cases = [
        ("vault", Some(OTHER_CONTRACT), None, None, CONTRACT),
        ("governance", None, Some(CONTRACT), None, OTHER_CONTRACT),
        ("asset_token", None, None, Some(CONTRACT), ASSET_CONTRACT),
    ];

    for (key, vault, governance, asset_token, existing) in cases {
        let args = crate::cli::DeployAdaptersArgs {
            vault: vault.map(|value| value.parse().expect("vault")),
            governance: governance.map(|value| value.parse().expect("governance")),
            asset_token: asset_token.map(|value| value.parse().expect("asset token")),
            blend_pools: vec![ACCOUNT.parse().expect("pool")],
            custodians: Vec::new(),
            adapter_admin: "vault".parse().expect("vault adapter admin"),
            build: false,
            force_new: false,
        };
        let provided = vault.or(governance).or(asset_token).expect("provided id");

        let error = deploy_adapters_plan(&cli, &manifest, &args)
            .expect_err("conflicting imported contract must fail planning");

        assert!(error.to_string().contains(&format!(
            "{key} already recorded as {existing}; refusing to overwrite with {provided}"
        )));
    }
}

#[test]
fn deploy_plan_uses_unique_keys_for_multiple_custodians() {
    let dir = tempfile::tempdir().expect("tempdir");
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
    let cli = base_cli(state, Commands::Status);
    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: Vec::new(),
            custodians: vec![
                ACCOUNT.parse().expect("first custodian"),
                ACCOUNT.parse().expect("duplicate custodian"),
                CONTRACT.parse().expect("second custodian"),
            ],
            adapter_admin: ACCOUNT.parse().expect("adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    let custodial_keys = plan
        .contracts_to_deploy
        .iter()
        .filter_map(|contract| {
            contract
                .key
                .starts_with("custodial_adapter_")
                .then_some(contract.key.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        custodial_keys,
        vec!["custodial_adapter_0", "custodial_adapter_1"]
    );
}

#[test]
fn deploy_plan_uses_unique_keys_for_multiple_blend_pools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(CONTRACT));
    let cli = base_cli(state, Commands::Status);
    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: vec![
                ACCOUNT.parse().expect("first pool"),
                CONTRACT.parse().expect("second pool"),
            ],
            custodians: Vec::new(),
            adapter_admin: OTHER_CONTRACT.parse().expect("adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    let blend_keys = plan
        .contracts_to_deploy
        .iter()
        .filter_map(|contract| {
            contract
                .key
                .starts_with("blend_adapter_")
                .then_some(contract.key.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(blend_keys, vec!["blend_adapter_0", "blend_adapter_1"]);
}

#[test]
fn deploy_plan_records_companion_upgrade_check_for_vault_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest
        .contracts
        .insert("governance".to_string(), imported_record(OTHER_CONTRACT));
    let cli = base_cli(state, Commands::Status);

    let plan = deploy_adapters_plan(
        &cli,
        &manifest,
        &crate::cli::DeployAdaptersArgs {
            vault: None,
            governance: None,
            asset_token: None,
            blend_pools: vec![ACCOUNT.parse().expect("pool")],
            custodians: Vec::new(),
            adapter_admin: "vault".parse().expect("vault adapter admin"),
            build: false,
            force_new: false,
        },
    )
    .expect("plan adapters");

    assert!(plan
        .stellar_commands
        .iter()
        .any(|command| command.contains("--send no -- version")));
    assert!(plan
        .stellar_commands
        .iter()
        .any(|command| command.contains(&format!("--admin {CONTRACT}"))));
    assert!(plan
        .warnings
        .iter()
        .any(|warning| warning.contains("companion-upgrade capability 0x40")));
}

#[test]
fn fresh_plan_rejects_existing_manifest_without_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    Manifest::new("testnet", None)
        .save(&state)
        .expect("save existing manifest");
    let cli = Cli {
        fresh_state: true,
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Plan(crate::cli::DeployPlanArgs {
                command: DeployPlanCommand::Stack(Box::new(test_deploy_stack_args(ACCOUNT))),
            }),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("existing path must be rejected");

    assert!(error
        .to_string()
        .contains("fresh deployment requires an unused --state path"));
    assert!(executor.calls().is_empty());
}
