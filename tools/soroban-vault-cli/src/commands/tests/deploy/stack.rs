use super::*;

#[test]
fn deploy_stack_deploys_one_blend_adapter_per_pool() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest.save(&state).expect("save manifest");
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
                blend_pools: vec![
                    CONTRACT.parse().expect("first pool"),
                    ASSET_CONTRACT.parse().expect("second pool"),
                ],
                custodians: Vec::new(),
                adapter_admin: Some(OTHER_CONTRACT.parse().expect("adapter admin")),
                build: true,
                force_new: false,
            })),
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy stack");

    let calls = submitted_calls(&executor.calls());
    let adapter_deploys = calls
        .iter()
        .filter(|(_, args)| args.iter().any(|arg| arg == "--pool"))
        .count();
    assert_eq!(adapter_deploys, 2);
}

#[test]
fn deploy_stack_records_explicit_admin_for_share_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    let state = dir.path().join("manifest.json");
    let cli = Cli {
        workspace_path: dir.path().into(),
        fresh_state: true,
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Stack(Box::new(test_deploy_stack_args(ACCOUNT))),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy stack");

    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    let share_token = manifest
        .contracts
        .get("share_token")
        .expect("share token record");
    assert_eq!(
        share_token
            .constructor_args
            .get("admin")
            .map(String::as_str),
        Some(ACCOUNT)
    );
    assert_eq!(
        share_token
            .constructor_args
            .get("vault")
            .map(String::as_str),
        Some(CONTRACT)
    );
    let calls = submitted_calls(&executor.calls());
    let share_token_deploy = calls
        .iter()
        .find(|(_, args)| args.iter().any(|arg| arg == "--name"))
        .expect("share token deploy call");
    assert!(share_token_deploy
        .1
        .windows(2)
        .any(|pair| pair == ["--admin", ACCOUNT]));
}

#[test]
fn deploy_stack_rejects_vault_as_share_token_admin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest.save(&state).expect("save manifest");
    let cli = Cli {
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Stack(Box::new(test_deploy_stack_args(CONTRACT))),
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("vault-as-admin must fail");

    assert!(error
        .to_string()
        .contains("share-token admin must differ from the vault"));
    assert!(executor.calls().is_empty());
}

#[test]
fn deploy_stack_deploys_one_custodial_adapter_per_custodian() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    manifest
        .contracts
        .insert("vault".to_string(), imported_record(CONTRACT));
    manifest.save(&state).expect("save manifest");
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
                blend_pools: Vec::new(),
                custodians: vec![ACCOUNT.parse().expect("custodian")],
                adapter_admin: Some(ACCOUNT.parse().expect("adapter admin")),
                build: true,
                force_new: false,
            })),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy stack");

    let calls = submitted_calls(&executor.calls());
    let adapter_deploys = calls
        .iter()
        .filter(|(_, args)| args.iter().any(|arg| arg == "--custodian"))
        .count();
    assert_eq!(adapter_deploys, 1);

    let loaded = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert_eq!(
        loaded
            .contracts
            .get("custodial_adapter_0")
            .expect("custodial adapter")
            .constructor_args
            .get("custodian")
            .map(String::as_str),
        Some(ACCOUNT)
    );
    assert_eq!(
        loaded
            .contracts
            .get("custodial_adapter_0")
            .expect("custodial adapter")
            .constructor_args
            .get("asset")
            .map(String::as_str),
        Some(CONTRACT)
    );
    assert!(calls.iter().any(|(_, args)| args
        .windows(2)
        .any(|window| window[0] == "--asset" && window[1] == CONTRACT)));
}

#[test]
fn deploy_stack_checkpoints_manifest_before_initialize_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
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
                blend_pools: Vec::new(),
                custodians: Vec::new(),
                adapter_admin: None,
                build: true,
                force_new: false,
            })),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = FailingInitializeExecutor::new();

    let err = run(&cli, &executor).expect_err("initialize should fail");
    assert!(
        err.to_string().contains("forced initialize failure")
            || err.to_string().contains("preflight simulation failed")
    );

    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    for key in ["vault", "share_token", "governance", "asset_token"] {
        assert!(
            manifest.contracts.contains_key(key),
            "{key} should be checkpointed"
        );
    }
    assert!(
        !manifest
            .contracts
            .get("vault")
            .expect("vault record")
            .initialized
    );
    assert!(
        manifest
            .contracts
            .get("share_token")
            .expect("share token record")
            .initialized
    );
    assert!(
        manifest
            .contracts
            .get("governance")
            .expect("governance record")
            .initialized
    );
    assert!(!manifest.contracts.contains_key("proxy_4626"));
    assert!(manifest.artifacts.contains_key("vault"));
    assert!(manifest.transactions.is_empty());
}

#[test]
fn deploy_stack_without_blend_pools_skips_blend_adapter() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fake_stack_wasms(dir.path());
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
                blend_pools: Vec::new(),
                custodians: Vec::new(),
                adapter_admin: None,
                build: true,
                force_new: false,
            })),
        }),
        ..base_cli(state.clone(), Commands::Status)
    };
    let executor = RecordingExecutor::new();

    run(&cli, &executor).expect("deploy stack without blend pools");

    assert!(!executor
        .calls()
        .iter()
        .any(|(_, args)| args.iter().any(|arg| arg == "--pool")));
    let manifest = Manifest::load_or_new(&state, "testnet", None).expect("load manifest");
    assert!(!manifest
        .contracts
        .keys()
        .any(|key| key.starts_with("blend_adapter_")));
}

#[test]
fn force_new_stack_requires_an_explicit_governance_timelock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = dir.path().join("manifest.json");
    let mut manifest = Manifest::new("testnet", None);
    let mut governance = imported_record(CONTRACT);
    governance
        .constructor_args
        .insert("timelock_ns".to_string(), "1000".to_string());
    manifest
        .contracts
        .insert("governance".to_string(), governance);
    manifest.save(&state).expect("save manifest");
    let mut stack = test_deploy_stack_args(ACCOUNT);
    stack.force_new = true;
    stack.governance_timelock_ns = None;
    let cli = Cli {
        workspace_path: dir.path().into(),
        command: Commands::Deploy(DeployArgs {
            command: DeployCommand::Stack(Box::new(stack)),
        }),
        ..base_cli(state, Commands::Status)
    };
    let executor = RecordingExecutor::new();

    let error = run(&cli, &executor).expect_err("force-new timelock must be explicit");

    assert!(error
        .to_string()
        .contains("new governance deployment requires --governance-timelock-ns"));
    assert!(executor.calls().is_empty());
}
