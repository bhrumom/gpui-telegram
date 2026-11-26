use super::*;
use orca_core::workspace_layout::{
    ORCA_YAML_MAX_SHARED_DIRS, SetupAgentStartupPolicy, WORKTREE_INCLUDE_MAX_PATHS,
    WorktreeCopyCandidate, parse_worktree_include, plan_worktree_include_copy,
};
use orca_core::{OrcaIdeErrorCode, WORKTREE_INCLUDE_MAX_BYTES, WORKTREE_INCLUDE_MAX_ENTRIES};
use tempfile::TempDir;

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn folder_workspace_project_group_and_trusted_cwds_never_auto_expand() {
    assert_send_sync::<WorktreeManager>();
    let tmp = TempDir::new().expect("tempdir");
    let mgr = WorktreeManager::open(tmp.path()).expect("open manager");

    // Project group CRUD
    let grp = mgr
        .create_project_group(ProjectGroup {
            id: "grp-1".to_string(),
            name: "Core".to_string(),
            description: Some("Core services".to_string()),
            color: Some("#3366ff".to_string()),
            sort_order: 1,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
        })
        .expect("create group");
    assert_eq!(mgr.list_project_groups().expect("list groups").len(), 1);
    assert_eq!(grp.id, "grp-1");

    // FolderWorkspace validation rejects path traversal
    let bad_fw = FolderWorkspace {
        id: "fw-bad".to_string(),
        name: "Bad".to_string(),
        root_path: "/tmp/../etc".to_string(),
        project_group_id: None,
        default_agent: None,
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        pinned: false,
        checkpoint_comment: None,
        workspace_status: None,
    };
    assert!(mgr.create_folder_workspace(bad_fw).is_err());

    // Valid FolderWorkspace CRUD
    let fw = FolderWorkspace {
        id: "fw-1".to_string(),
        name: "Notes".to_string(),
        root_path: tmp.path().join("notes").to_string_lossy().into_owned(),
        project_group_id: Some("grp-1".to_string()),
        default_agent: Some("claude".to_string()),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        pinned: true,
        checkpoint_comment: Some("initial draft".to_string()),
        workspace_status: Some("in-progress".to_string()),
    };
    mgr.create_folder_workspace(fw.clone()).expect("create fw");
    let fetched = mgr.get_folder_workspace("fw-1").expect("get fw");
    assert_eq!(fetched, fw);

    // Floating Workspace `trusted_cwds`: untrusted path is denied and allowlist stays empty.
    let untrusted = tmp.path().join("untrusted");
    let err = mgr
        .validate_floating_workspace_cwd(&untrusted)
        .expect_err("untrusted cwd must fail");
    assert_eq!(err.code_str(), "permission_denied");
    assert!(mgr.list_trusted_cwds().expect("list trusted").is_empty());

    // Explicit trust succeeds
    mgr.trust_floating_cwd(&untrusted, Some("scratch".into()), "user", 2_000)
        .expect("trust cwd");
    assert!(mgr.validate_floating_workspace_cwd(&untrusted).is_ok());
    assert_eq!(mgr.list_trusted_cwds().expect("list trusted").len(), 1);
}

#[test]
fn creature_naming_lineage_and_first_work_rename_guards() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_root = tmp.path().join("repo");
    let wt_parent_dir = tmp.path().join("wt-parent");
    let wt_child_dir = tmp.path().join("wt-child");
    std::fs::create_dir_all(&repo_root).expect("mkdir repo");
    std::fs::create_dir_all(&wt_parent_dir).expect("mkdir parent");
    std::fs::create_dir_all(&wt_child_dir).expect("mkdir child");

    let mgr = WorktreeManager::open(tmp.path().join("state")).expect("open mgr");
    let parent = mgr
        .create_worktree(CreateWorktreeRequest {
            id: Some("wt-parent".into()),
            repo_root: repo_root.clone(),
            worktree_path: wt_parent_dir,
            branch_name: Some("main".into()),
            title: Some("Main".into()),
            parent_worktree_id: None,
            project_group_id: None,
            created_by_orca: true,
            existing_branches: BTreeSet::new(),
            seed_hint: None,
        })
        .expect("create parent");

    let child = mgr
        .create_worktree(CreateWorktreeRequest {
            id: Some("wt-child".into()),
            repo_root: repo_root.clone(),
            worktree_path: wt_child_dir.clone(),
            branch_name: None,
            title: None,
            parent_worktree_id: Some(parent.id.clone()),
            project_group_id: None,
            created_by_orca: true,
            existing_branches: BTreeSet::new(),
            seed_hint: Some("deterministic-seed-1".into()),
        })
        .expect("create creature child");
    assert!(child.is_creature_named);
    assert!(is_creature_branch_name(&child.branch_name));

    // Collision avoidance on same seed
    let second_name = mgr
        .generate_creature_branch_name(&BTreeSet::new(), Some("deterministic-seed-1"))
        .expect("gen second");
    assert_ne!(second_name, child.branch_name);

    // Lineage verification
    let lineage = mgr.get_worktree_lineage(&child.id).expect("lineage");
    assert_eq!(lineage.len(), 2);
    assert_eq!(lineage[0].id, "wt-parent");
    assert_eq!(lineage[1].id, "wt-child");

    // Guard 1: NotWorking -> Skipped
    let base_ctx = FirstWorkRenameContext {
        worktree_id: child.id.clone(),
        agent_state: AgentRunState::Idle,
        prompt_summary: "Fix OAuth PKCE redirect bug".into(),
        checkout_locked: false,
        upstream_published: false,
        existing_branches: BTreeSet::new(),
        rename_directory_on_disk: true,
        base_backoff_ms: DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS,
        now_ms: 10_000,
    };
    assert_eq!(
        mgr.evaluate_and_apply_first_work_rename(&base_ctx)
            .expect("eval"),
        FirstWorkRenameOutcome::Skipped {
            reason: FirstWorkRenameSkipReason::NotWorking
        }
    );

    // Guard 2: checkout_locked -> Skipped
    let mut locked_ctx = base_ctx.clone();
    locked_ctx.agent_state = AgentRunState::Working;
    locked_ctx.checkout_locked = true;
    assert_eq!(
        mgr.evaluate_and_apply_first_work_rename(&locked_ctx)
            .expect("eval locked"),
        FirstWorkRenameOutcome::Skipped {
            reason: FirstWorkRenameSkipReason::CheckoutLocked
        }
    );

    // Guard 3: upstream_published -> Skipped
    let mut upstream_ctx = base_ctx.clone();
    upstream_ctx.agent_state = AgentRunState::Working;
    upstream_ctx.upstream_published = true;
    assert_eq!(
        mgr.evaluate_and_apply_first_work_rename(&upstream_ctx)
            .expect("eval upstream"),
        FirstWorkRenameOutcome::Skipped {
            reason: FirstWorkRenameSkipReason::UpstreamPublished
        }
    );

    // Guard 4: failure backoff -> Skipped until window passes
    mgr.record_first_work_rename_failure(&child.id, 10_000)
        .expect("record failure");
    let mut backoff_ctx = base_ctx.clone();
    backoff_ctx.agent_state = AgentRunState::Working;
    backoff_ctx.now_ms = 20_000;
    assert!(matches!(
        mgr.evaluate_and_apply_first_work_rename(&backoff_ctx)
            .expect("eval backoff"),
        FirstWorkRenameOutcome::Skipped {
            reason: FirstWorkRenameSkipReason::InFailureBackoff { .. }
        }
    ));

    // All guards clear after backoff window -> Renames branch, folder, and title
    let mut ok_ctx = base_ctx;
    ok_ctx.agent_state = AgentRunState::Working;
    ok_ctx.now_ms = 10_000 + DEFAULT_FIRST_WORK_RENAME_BACKOFF_MS + 1;
    let outcome = mgr
        .evaluate_and_apply_first_work_rename(&ok_ctx)
        .expect("apply rename");
    match outcome {
        FirstWorkRenameOutcome::Renamed {
            new_branch,
            new_path,
            ..
        } => {
            assert_eq!(new_branch, "fix-oauth-pkce-redirect-bug");
            assert!(Path::new(&new_path).is_dir());
            assert!(!wt_child_dir.exists());
        }
        other => panic!("expected Renamed, got {other:?}"),
    }
}

#[test]
fn interrupted_removal_recovery_sparse_presets_and_cleanup_unsaved_guard() {
    let tmp = TempDir::new().expect("tempdir");
    let state_dir = tmp.path().join("state");
    let wt1_dir = tmp.path().join("wt-1");
    let wt2_dir = tmp.path().join("wt-2");
    std::fs::create_dir_all(wt1_dir.join("sub")).expect("mkdir wt1");
    std::fs::write(wt1_dir.join("sub/data.bin"), vec![7u8; 4096]).expect("write data");
    std::fs::create_dir_all(&wt2_dir).expect("mkdir wt2");
    std::fs::write(wt2_dir.join("README.md"), "hello").expect("write readme");

    {
        let mgr = WorktreeManager::open(&state_dir).expect("open mgr");
        mgr.create_worktree(CreateWorktreeRequest {
            id: Some("wt-1".into()),
            repo_root: tmp.path().join("repo"),
            worktree_path: wt1_dir.clone(),
            branch_name: Some("feat-one".into()),
            title: None,
            parent_worktree_id: None,
            project_group_id: None,
            created_by_orca: true,
            existing_branches: BTreeSet::new(),
            seed_hint: None,
        })
        .expect("create wt-1");
        mgr.create_worktree(CreateWorktreeRequest {
            id: Some("wt-2".into()),
            repo_root: tmp.path().join("repo"),
            worktree_path: wt2_dir.clone(),
            branch_name: Some("feat-two".into()),
            title: None,
            parent_worktree_id: None,
            project_group_id: None,
            created_by_orca: true,
            existing_branches: BTreeSet::new(),
            seed_hint: None,
        })
        .expect("create wt-2");

        // Sparse checkout presets reject `..`, absolute paths, control chars, `.git`
        for bad in ["../escape", "/etc/passwd", "crates/.git/config", "a\0b"] {
            let res = mgr.save_sparse_preset(SparseCheckoutPreset {
                id: "sp-bad".into(),
                name: "Bad".into(),
                patterns: vec![bad.into()],
                cone_mode: true,
                created_at_ms: 1,
                updated_at_ms: 1,
            });
            assert!(res.is_err(), "pattern '{bad}' must be rejected");
        }
        let preset = mgr
            .save_sparse_preset(SparseCheckoutPreset {
                id: "sp-core".into(),
                name: "Core Crates".into(),
                patterns: vec!["crates/orca_core".into(), "crates/orca_ide".into()],
                cone_mode: true,
                created_at_ms: 1,
                updated_at_ms: 1,
            })
            .expect("save sparse preset");
        assert_eq!(preset.patterns.len(), 2);

        // Checkpoint + kanban status + disk usage
        let updated = mgr
            .set_worktree_checkpoint(
                "wt-2",
                Some("waiting on CI".into()),
                Some(WorkspaceStatus::InReview),
                5_000,
            )
            .expect("set checkpoint");
        assert_eq!(updated.checkpoint_comment.as_deref(), Some("waiting on CI"));
        assert_eq!(updated.workspace_status.as_deref(), Some("in-review"));

        let usage = mgr.analyze_disk_usage(&wt1_dir).expect("analyze disk");
        assert_eq!(usage.total_bytes, 4096);
        assert_eq!(usage.file_count, 1);

        // Unsaved editor guard refuses cleanup unless force_discard_unsaved == true
        let blocked = mgr.cleanup_worktree(&CleanupWorktreeRequest {
            worktree_id: "wt-2".into(),
            unsaved_editor_tabs: vec!["src/main.rs".into()],
            force_discard_unsaved: false,
            remove_directory_on_disk: true,
            now_ms: 6_000,
        });
        assert!(blocked.is_err());
        assert!(wt2_dir.exists());

        let forced = mgr
            .cleanup_worktree(&CleanupWorktreeRequest {
                worktree_id: "wt-2".into(),
                unsaved_editor_tabs: vec!["src/main.rs".into()],
                force_discard_unsaved: true,
                remove_directory_on_disk: true,
                now_ms: 6_500,
            })
            .expect("force cleanup");
        assert_eq!(forced.discarded_unsaved_count, 1);
        assert!(!wt2_dir.exists());

        // Start removal for wt-1 and simulate crash before completion
        let _ledger = mgr.begin_removal("wt-1", 7_000).expect("begin removal");
    }

    // Re-open manager (simulated boot) and recover interrupted removal
    let reopened = WorktreeManager::open(&state_dir).expect("reopen mgr");
    let recovery = reopened
        .recover_interrupted_removals(8_000)
        .expect("recover removals");
    assert_eq!(recovery.recovered_removal_ids.len(), 1);
    assert!(recovery.failed_removals.is_empty());
    assert!(!wt1_dir.exists());
    assert!(reopened.get_worktree("wt-1").is_err());
}

#[test]
fn orca_yaml_runner_windows_shebang_guard_shared_dirs_and_worktreeinclude_copy() {
    let tmp = TempDir::new().expect("tempdir");
    let src_repo = tmp.path().join("src_repo");
    let dst_wt = tmp.path().join("dst_wt");
    std::fs::create_dir_all(src_repo.join(".git")).expect("mkdir .git");
    std::fs::create_dir_all(src_repo.join("shared_cache")).expect("mkdir cache");
    std::fs::create_dir_all(&dst_wt).expect("mkdir dst");
    std::fs::write(src_repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("git head");
    std::fs::write(src_repo.join(".env.local"), "SECRET=1\n").expect("env local");
    std::fs::write(src_repo.join("keep.log"), "ok\n").expect("keep log");
    std::fs::write(src_repo.join("skip.log"), "no\n").expect("skip log");
    std::fs::write(src_repo.join("tracked.txt"), "tracked\n").expect("tracked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(src_repo.join(".env.local"), src_repo.join("link.env"))
        .expect("symlink");

    // 1. orca.yaml trust hash, WaitForSetup, Windows POSIX `#!` guard, sharedDirectories
    let shared_yaml = b"setup:\n  - \"#!/bin/bash\\necho hi\"\nsetupAgentStartupPolicy: wait-for-setup\nsharedDirectories:\n  - shared_cache\n";
    let parsed = parse_orca_yaml_text(std::str::from_utf8(shared_yaml).expect("utf8"))
        .expect("parse orca yaml");
    assert_eq!(
        parsed.setup_agent_startup_policy,
        SetupAgentStartupPolicy::WaitForSetup
    );

    // Untrusted -> ApprovalRequired
    let untrusted_err = plan_orca_yaml_scripts(&OrcaYamlRunRequest {
        shared_yaml_bytes: Some(shared_yaml.to_vec()),
        local_yaml_bytes: None,
        override_mode: ScriptOverrideMode::SharedOnly,
        phase: OrcaYamlScriptPhase::Setup,
        platform: ScriptPlatform::Unix,
        git_bash_available: false,
        approved_trust_hash: None,
        require_trust_approval: true,
    })
    .expect_err("must require trust");
    assert_eq!(
        untrusted_err.ide_error_code(),
        Some(OrcaIdeErrorCode::ApprovalRequired)
    );

    // Windows without Git Bash refuses POSIX `#!` shebang script
    let win_err = plan_orca_yaml_scripts(&OrcaYamlRunRequest {
        shared_yaml_bytes: Some(shared_yaml.to_vec()),
        local_yaml_bytes: None,
        override_mode: ScriptOverrideMode::SharedOnly,
        phase: OrcaYamlScriptPhase::Setup,
        platform: ScriptPlatform::Windows,
        git_bash_available: false,
        approved_trust_hash: None,
        require_trust_approval: false,
    })
    .expect_err("windows must refuse posix shebang");
    assert_eq!(win_err.code_str(), "permission_denied");

    // Approved RunBoth plan & genuine script execution on Unix
    let local_yaml = b"setup:\n  - \"echo local-ok\"\n";
    let merged_unapproved = plan_orca_yaml_scripts(&OrcaYamlRunRequest {
        shared_yaml_bytes: Some(b"setup:\n  - \"echo shared-ok\"\n".to_vec()),
        local_yaml_bytes: Some(local_yaml.to_vec()),
        override_mode: ScriptOverrideMode::RunBoth,
        phase: OrcaYamlScriptPhase::Setup,
        platform: ScriptPlatform::Unix,
        git_bash_available: true,
        approved_trust_hash: None,
        require_trust_approval: false,
    })
    .expect("plan RunBoth");
    assert_eq!(merged_unapproved.steps.len(), 2);

    let mgr = WorktreeManager::open_in_memory().expect("in-memory mgr");
    mgr.approve_orca_yaml_trust(&src_repo, &merged_unapproved.trust_hash, 1_000)
        .expect("approve trust hash");
    let approved_hash = mgr
        .approved_orca_yaml_trust_hash(&src_repo)
        .expect("read trust hash");
    let approved_plan = plan_orca_yaml_scripts(&OrcaYamlRunRequest {
        shared_yaml_bytes: Some(b"setup:\n  - \"echo shared-ok\"\n".to_vec()),
        local_yaml_bytes: Some(local_yaml.to_vec()),
        override_mode: ScriptOverrideMode::RunBoth,
        phase: OrcaYamlScriptPhase::Setup,
        platform: ScriptPlatform::Unix,
        git_bash_available: true,
        approved_trust_hash: approved_hash,
        require_trust_approval: true,
    })
    .expect("approved plan");
    #[cfg(unix)]
    {
        let exec = execute_orca_yaml_scripts(&dst_wt, &approved_plan, ScriptPlatform::Unix, true)
            .expect("execute setup scripts");
        assert!(exec.succeeded);
        assert_eq!(exec.executed_steps.len(), 2);
        assert_eq!(exec.executed_steps[0].stdout.trim(), "shared-ok");
        assert_eq!(exec.executed_steps[1].stdout.trim(), "local-ok");
    }

    // sharedDirectories symlink creation & cap/traversal checks
    let links = link_shared_directories(&src_repo, &dst_wt, &parsed.shared_directories)
        .expect("link shared dirs");
    assert_eq!(links.len(), 1);
    assert!(links[0].created);
    assert!(dst_wt.join("shared_cache").symlink_metadata().expect("meta").file_type().is_symlink());
    assert!(link_shared_directories(&src_repo, &dst_wt, &["../outside".into()]).is_err());
    let too_many: Vec<String> = (0..=ORCA_YAML_MAX_SHARED_DIRS)
        .map(|i| format!("dir_{i}"))
        .collect();
    assert!(link_shared_directories(&src_repo, &dst_wt, &too_many).is_err());

    // 2. `.worktreeinclude` real copy + skip tracked/negated/symlinks/.git
    let mut ignored = BTreeSet::new();
    ignored.insert(".env.local".to_string());
    ignored.insert("keep.log".to_string());
    ignored.insert("skip.log".to_string());
    ignored.insert("link.env".to_string());
    let report = execute_worktree_include_copy(
        &src_repo,
        &dst_wt,
        ".env.local\n*.log\n!skip.log\n*.env\n",
        &WorktreeIncludeCopyOptions {
            skip_symlinks: true,
            git_ignored_paths: Some(ignored),
        },
    )
    .expect("execute worktreeinclude copy");
    assert!(dst_wt.join(".env.local").is_file());
    assert!(dst_wt.join("keep.log").is_file());
    assert!(!dst_wt.join("skip.log").exists());
    assert!(!dst_wt.join("tracked.txt").exists());
    assert!(!dst_wt.join(".git").exists());
    #[cfg(unix)]
    assert_eq!(report.skipped_symlinks, vec!["link.env".to_string()]);

    // 3. Budget enforcement -> CopyBudgetExceeded (2 GiB / 50k entries / 1k paths)
    let too_many_rules: String = (0..=WORKTREE_INCLUDE_MAX_PATHS)
        .map(|i| format!("f{i}.txt\n"))
        .collect();
    let rule_err = plan_worktree_include_copy(&parse_worktree_include(&too_many_rules), &[])
        .expect_err("over 1000 paths must fail");
    assert_eq!(rule_err.code, OrcaIdeErrorCode::CopyBudgetExceeded);

    let oversize_err = plan_worktree_include_copy(
        &parse_worktree_include("big.bin\n"),
        &[WorktreeCopyCandidate {
            relative_path: "big.bin".into(),
            size_bytes: WORKTREE_INCLUDE_MAX_BYTES + 1,
            is_dir: false,
            is_symlink: false,
            is_git_ignored: true,
            can_clone_cow: false,
        }],
    )
    .expect_err("over 2 GiB must fail");
    assert_eq!(oversize_err.code, OrcaIdeErrorCode::CopyBudgetExceeded);

    let too_many_entries: Vec<WorktreeCopyCandidate> = (0..=WORKTREE_INCLUDE_MAX_ENTRIES)
        .map(|i| WorktreeCopyCandidate {
            relative_path: format!("e/{i}.txt"),
            size_bytes: 1,
            is_dir: false,
            is_symlink: false,
            is_git_ignored: true,
            can_clone_cow: false,
        })
        .collect();
    let entries_err =
        plan_worktree_include_copy(&parse_worktree_include("e/*.txt\n"), &too_many_entries)
            .expect_err("over 50k entries must fail");
    assert_eq!(entries_err.code, OrcaIdeErrorCode::CopyBudgetExceeded);
}
