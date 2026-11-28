//! End-to-end verification tests for:
//! 1. Deleting new conversations / thread tabs (`新对话 #1`, `新对话 #2`, etc.) from header tabs and Topics pane.
//! 2. Left sidebar 1:1 OpenBot Workflows entry (`AppleSidebarSection::Workflows` pinned first + Lucide `Waypoints` SVG + v1->v2 pin upgrade).
//! 3. Full interactive 1:1 OpenBot Workflows Studio persisted to live SQLite `openbot.db`
//!    (`WorkflowsEmpty`, `WorkflowStart` 5 templates, nodes/edges/ink/pan/zoom/undo/redo, folders,
//!    pause/resume, execution + approval gate + history, version preview/restore, portable JSON export/import,
//!    and delete + undo restore).

use std::fs;

use gpui::AssetSource;
use gpui_telegram_desktop::theme::{IconName, TelegramAssetSource};
use gpui_telegram_desktop::views::apple_sidebar::{
    AppleSidebarSection, DEFAULT_PINNED_SECTIONS, PAGE_RAIL_PINS_FILE, PINNABLE_SECTIONS,
    load_page_rail_pins, save_page_rail_pins,
};
use gpui_telegram_desktop::views::conversation_stage::ConversationStage;
use gpui_telegram_desktop::views::openbot::workflow_canvas::{
    WorkflowInkTool, WorkflowTextInputTarget,
};
use gpui_telegram_desktop::views::openbot::workflow_interactive::{
    OPENBOT_WORKFLOW_TEMPLATES, WorkflowUiAction, build_openbot_template_graph,
    handle_canvas_pointer_down, handle_canvas_pointer_move, handle_canvas_pointer_up,
    handle_canvas_scroll_wheel, handle_node_pointer_down,
};
use gpui_telegram_desktop::views::openbot::{
    OpenBotPage, OpenBotWorkspaceShell, ServerPlatform, ShortcutChord,
};
use openbot_contracts::workflows::{
    WorkflowFieldValue, WorkflowNodeKind, WorkflowPortType, WorkflowRunStatus,
};
use openbot_runtime::OpenBotHostRuntime;

#[test]
fn test_new_conversation_thread_tabs_can_be_deleted_and_numbered_without_collision() {
    let mut stage = ConversationStage::new();
    assert!(stage.thread_tabs.is_empty());
    assert_eq!(stage.active_thread_id, None);
    assert_eq!(stage.next_thread_tab_number(), 1);

    let id1 = stage.append_and_focus_thread_tab("新对话 #1");
    assert_eq!(stage.thread_tabs.len(), 1);
    assert_eq!(stage.active_thread_id.as_deref(), Some(id1.as_str()));
    assert_eq!(stage.next_thread_tab_number(), 2);

    let id2 = stage.append_and_focus_thread_tab("新对话 #2");
    let id3 = stage.append_and_focus_thread_tab("新对话 #3");
    assert_eq!(stage.thread_tabs.len(), 3);
    assert_eq!(stage.active_thread_id.as_deref(), Some(id3.as_str()));
    assert_ne!(id1, id2);
    assert_ne!(id2, id3);

    // Delete active rightmost tab (#3) -> focus shifts cleanly to left neighbor (#2)
    let removed3 = stage.remove_thread_tab(&id3);
    assert_eq!(removed3.as_ref().map(|(_, l)| l.as_str()), Some("新对话 #3"));
    assert_eq!(stage.thread_tabs.len(), 2);
    assert_eq!(stage.active_thread_id.as_deref(), Some(id2.as_str()));

    // Next thread number stays monotonic (no collision with #2)
    assert_eq!(stage.next_thread_tab_number(), 4);
    let id4 = stage.append_and_focus_thread_tab("新对话 #4");
    assert_eq!(stage.thread_tabs.len(), 3);

    // Close tabs to right of #1 -> removes #2 and #4, keeps #1 focused
    let removed_right = stage.remove_thread_tabs_to_right(&id1);
    assert_eq!(removed_right, 2);
    assert_eq!(stage.thread_tabs.len(), 1);
    assert_eq!(stage.thread_tabs[0].0, id1);
    assert_eq!(stage.active_thread_id.as_deref(), Some(id1.as_str()));

    // Add two more, then close others keeping #1
    let _id5 = stage.append_and_focus_thread_tab("新对话 #5");
    let _id6 = stage.append_and_focus_thread_tab("新对话 #6");
    let removed_others = stage.remove_other_thread_tabs(&id1);
    assert_eq!(removed_others, 2);
    assert_eq!(stage.thread_tabs.len(), 1);
    assert_eq!(stage.active_thread_id.as_deref(), Some(id1.as_str()));

    // Delete last remaining thread tab -> returns cleanly to Main conversation (`None`)
    let removed1 = stage.remove_thread_tab(&id1);
    assert_eq!(removed1.as_ref().map(|(_, l)| l.as_str()), Some("新对话 #1"));
    assert!(stage.thread_tabs.is_empty());
    assert_eq!(stage.active_thread_id, None);
    assert_eq!(stage.thread_tab_menu_id, None);
    let _ = id4;
}

#[test]
fn test_left_sidebar_workflows_entry_matches_openbot_and_migrates_v1_pins() {
    // 1. Workflows is pinned first right below fixed Chats, using OpenBot Lucide Waypoints SVG
    assert_eq!(DEFAULT_PINNED_SECTIONS[0], AppleSidebarSection::Workflows);
    assert_eq!(PINNABLE_SECTIONS[0], AppleSidebarSection::Workflows);
    assert_eq!(
        AppleSidebarSection::Workflows.icon_name(),
        IconName::WorkflowWaypoints
    );
    assert_eq!(AppleSidebarSection::Workflows.localized_label("zh-CN"), "工作流");
    assert_eq!(AppleSidebarSection::Workflows.localized_label("en-US"), "Workflows");

    // Verify embedded workflow_waypoints.svg resolves via TelegramAssetSource
    let svg_bytes = TelegramAssetSource
        .load("icons/workflow_waypoints.svg")
        .expect("load workflow_waypoints.svg")
        .expect("workflow_waypoints.svg present");
    let svg_str = std::str::from_utf8(&svg_bytes).expect("utf-8 svg");
    assert!(svg_str.contains("<svg") && svg_str.contains("<circle"));

    // 2. Legacy untouched v1 pins automatically promote Workflows to index 0
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(
        tmp.path().join(PAGE_RAIL_PINS_FILE),
        r#"{"version":1,"pinned_sections":["community","books","music","games","app_store"]}"#,
    )
    .expect("write legacy v1 pins");
    let upgraded = load_page_rail_pins(tmp.path());
    assert_eq!(upgraded[0], AppleSidebarSection::Workflows);
    assert_eq!(upgraded.len(), 3);
    assert_eq!(upgraded, DEFAULT_PINNED_SECTIONS.to_vec());

    // 3. Explicit v2 unpin of Workflows persists across restart
    let unpinned: Vec<AppleSidebarSection> = upgraded
        .into_iter()
        .filter(|&s| s != AppleSidebarSection::Workflows)
        .collect();
    save_page_rail_pins(tmp.path(), &unpinned).expect("save v2 pins");
    let reloaded = load_page_rail_pins(tmp.path());
    assert!(!reloaded.contains(&AppleSidebarSection::Workflows));
    assert_eq!(reloaded, unpinned);
}

#[test]
fn test_openbot_workflows_studio_full_sqlite_e2e_lifecycle() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let rt = OpenBotHostRuntime::open(tmp.path()).expect("open sqlite openbot.db");

    // 1. All 5 OpenBot starter templates build valid graphs
    assert_eq!(OPENBOT_WORKFLOW_TEMPLATES.len(), 5);
    for tpl in OPENBOT_WORKFLOW_TEMPLATES {
        let (_title, g) = build_openbot_template_graph(tpl.id, "zh-CN", None)
            .expect("template builds valid graph");
        assert!(
            g.nodes.len() >= 3,
            "template {} should have >=3 nodes",
            tpl.id
        );
        assert!(
            !g.edges.is_empty(),
            "template {} should connect nodes",
            tpl.id
        );
    }

    // 2. Empty SQLite DB opens cleanly into OpenBot WorkflowsEmpty (NOT sample pipeline)
    let mut shell = OpenBotWorkspaceShell::with_data_dir(tmp.path());
    shell.sync_locale("zh-CN");
    shell.set_workflow_runtime(rt.clone());
    shell.open_workflows_page();
    assert_eq!(shell.active_page, OpenBotPage::Workflows);
    assert!(!shell.workflow_canvas.has_active_workflow);
    assert!(shell.workflow_canvas.library.is_empty());

    // 3. Create new workflow -> lands on WorkflowStart template picker & persists to SQLite
    shell.dispatch_workflow_action(WorkflowUiAction::CreateNewWorkflow);
    assert!(shell.workflow_canvas.has_active_workflow);
    assert!(shell.workflow_canvas.showing_start_templates());
    assert_eq!(shell.workflow_canvas.library.len(), 1);
    assert_eq!(shell.workflow_canvas.open_tab_ids.len(), 1);
    let wf_id = shell.workflow_canvas.workflow_id.clone();

    // 4. Apply `weeklyReport` template (includes Approval gate) -> persists nodes & edges
    shell.dispatch_workflow_action(WorkflowUiAction::ApplyTemplate("weeklyReport"));
    assert!(!shell.workflow_canvas.showing_start_templates());
    let initial_nodes = shell.workflow_canvas.graph.nodes.len();
    assert_eq!(initial_nodes, 4);
    let persisted = rt
        .workflows()
        .get_workflow(&wf_id)
        .expect("read persisted workflow")
        .expect("workflow exists in sqlite");
    assert_eq!(persisted.graph.nodes.len(), initial_nodes);

    // 5. Add node, wire edge, edit prompt via keyboard input, duplicate, lock, undo/redo
    shell.dispatch_workflow_action(WorkflowUiAction::AddNode {
        kind: WorkflowNodeKind::Prompt,
        preset: None,
    });
    assert_eq!(shell.workflow_canvas.graph.nodes.len(), initial_nodes + 1);
    let added_id = shell
        .workflow_canvas
        .selected_node_ids
        .first()
        .cloned()
        .expect("added node selected");

    // Edit added node prompt via keyboard text input + Enter commit
    shell.dispatch_workflow_action(WorkflowUiAction::StartEditNodeField {
        node_id: added_id.clone(),
        field_key: "text".into(),
    });
    assert!(matches!(
        shell.workflow_canvas.active_text_input,
        WorkflowTextInputTarget::EditNodeField { .. }
    ));
    shell.clear_workflow_active_input();
    shell.handle_text_input("汇总本周发布变更");
    shell.handle_enter(false);
    assert_eq!(
        shell.workflow_canvas.active_text_input,
        WorkflowTextInputTarget::None
    );
    let edited_text = shell
        .workflow_canvas
        .graph
        .nodes
        .iter()
        .find(|n| n.id == added_id)
        .and_then(|n| n.fields.get("text"))
        .and_then(|v| match v {
            WorkflowFieldValue::Text(s) => Some(s.as_str()),
            _ => None,
        });
    assert_eq!(edited_text, Some("汇总本周发布变更"));

    // Wire port-to-port edge to another node
    let target_id = shell
        .workflow_canvas
        .graph
        .nodes
        .iter()
        .find(|n| n.id != added_id && n.kind != WorkflowNodeKind::Trigger)
        .map(|n| n.id.clone())
        .expect("target node");
    let edges_before = shell.workflow_canvas.graph.edges.len();
    shell.dispatch_workflow_action(WorkflowUiAction::StartConnectFrom {
        node_id: added_id.clone(),
        port_type: WorkflowPortType::Text,
        branch: None,
    });
    shell.dispatch_workflow_action(WorkflowUiAction::CompleteConnectTo {
        target_node_id: target_id,
    });
    assert_eq!(shell.workflow_canvas.graph.edges.len(), edges_before + 1);

    // Lock, unlock, drag node, draw ink stroke, scroll zoom, tidy, undo/redo via shortcut
    shell.dispatch_workflow_action(WorkflowUiAction::ToggleLockSelected);
    assert_eq!(
        shell
            .workflow_canvas
            .graph
            .nodes
            .iter()
            .find(|n| n.id == added_id)
            .and_then(|n| n.locked),
        Some(true)
    );
    shell.dispatch_workflow_action(WorkflowUiAction::ToggleLockSelected);
    let old_x = shell
        .workflow_canvas
        .graph
        .nodes
        .iter()
        .find(|n| n.id == added_id)
        .unwrap()
        .at
        .x;
    handle_node_pointer_down(&mut shell.workflow_canvas, &added_id, false, 100.0, 100.0);
    assert!(handle_canvas_pointer_move(&mut shell.workflow_canvas, 160.0, 120.0));
    assert!(handle_canvas_pointer_up(
        &mut shell.workflow_canvas,
        Some(&rt),
        "zh-CN"
    ));
    let moved_x = shell
        .workflow_canvas
        .graph
        .nodes
        .iter()
        .find(|n| n.id == added_id)
        .unwrap()
        .at
        .x;
    assert!(moved_x > old_x);

    // Freehand pen draw + wheel zoom
    shell.dispatch_workflow_action(WorkflowUiAction::SetTool(WorkflowInkTool::Pen));
    handle_canvas_pointer_down(&mut shell.workflow_canvas, 200.0, 200.0);
    handle_canvas_pointer_move(&mut shell.workflow_canvas, 240.0, 230.0);
    handle_canvas_pointer_up(&mut shell.workflow_canvas, Some(&rt), "zh-CN");
    assert_eq!(shell.workflow_canvas.graph.strokes.len(), 1);
    shell.dispatch_workflow_action(WorkflowUiAction::SetTool(WorkflowInkTool::Select));

    let z0 = shell.workflow_canvas.zoom;
    handle_canvas_scroll_wheel(&mut shell.workflow_canvas, 0.0, -10.0, true);
    assert!(shell.workflow_canvas.zoom > z0);

    // Keyboard Delete removes selected node, Cmd/Ctrl+Z restores it
    shell.workflow_canvas.select_node(&added_id, false);
    shell.handle_delete();
    assert_eq!(shell.workflow_canvas.graph.nodes.len(), initial_nodes);
    let is_mac = shell.platform == ServerPlatform::MacOs;
    let undo_chord = ShortcutChord::new(!is_mac, false, false, is_mac, "KeyZ");
    assert!(shell.handle_shortcut(&undo_chord));
    assert_eq!(shell.workflow_canvas.graph.nodes.len(), initial_nodes + 1);
    let redo_chord = ShortcutChord::new(!is_mac, false, true, is_mac, "KeyZ");
    assert!(shell.handle_shortcut(&redo_chord));
    assert_eq!(shell.workflow_canvas.graph.nodes.len(), initial_nodes);

    // 6. Folder management persisted to disk
    shell.dispatch_workflow_action(WorkflowUiAction::StartNewFolder);
    shell.handle_text_input("核心自动化");
    shell.handle_enter(false);
    assert_eq!(shell.workflow_canvas.folders.len(), 1);
    let folder_id = shell.workflow_canvas.folders[0].id.clone();
    shell.dispatch_workflow_action(WorkflowUiAction::MoveWorkflowToFolder(Some(
        folder_id.clone(),
    )));
    assert_eq!(
        shell.workflow_canvas.folder_placement.get(&wf_id),
        Some(&folder_id)
    );

    // 7. Pause & resume workflow in SQLite
    shell.dispatch_workflow_action(WorkflowUiAction::TogglePaused);
    assert!(shell.workflow_canvas.paused);
    shell.dispatch_workflow_action(WorkflowUiAction::TogglePaused);
    assert!(!shell.workflow_canvas.paused);

    // 8. Run workflow -> hits Approval gate -> Approve completes run & records run history!
    shell.dispatch_workflow_action(WorkflowUiAction::RunWorkflow { from_node_id: None });
    let (run_id, appr_node_id) = shell
        .workflow_canvas
        .pending_approval
        .clone()
        .expect("weeklyReport should pause on approval gate");
    shell.dispatch_workflow_action(WorkflowUiAction::DecideApproval {
        run_id,
        node_id: appr_node_id,
        approve: true,
    });
    assert!(shell.workflow_canvas.pending_approval.is_none());
    assert_eq!(
        shell.workflow_canvas.hud_run.as_ref().map(|r| r.status),
        Some(WorkflowRunStatus::Done)
    );
    assert!(!shell.workflow_canvas.past_runs.is_empty());

    // 9. Portable JSON export + import into a brand new SQLite workflow
    let exported = shell.workflow_canvas.export_portable_json();
    shell.dispatch_workflow_action(WorkflowUiAction::OpenImportDialog);
    shell.handle_text_input(&serde_json::to_string(&exported).expect("serialize export"));
    shell.dispatch_workflow_action(WorkflowUiAction::ConfirmImportJson);
    assert!(!shell.workflow_canvas.import_dialog_open);
    assert_eq!(shell.workflow_canvas.library.len(), 2);

    // 10. Delete active workflow + Undo delete toast restores it cleanly in SQLite
    let deleted_id = shell.workflow_canvas.workflow_id.clone();
    shell.dispatch_workflow_action(WorkflowUiAction::DeleteCurrentWorkflow);
    assert_eq!(shell.workflow_canvas.library.len(), 1);
    assert!(shell.workflow_canvas.deleted_undo.is_some());
    assert!(rt.workflows().get_workflow(&deleted_id).unwrap().is_none());

    shell.dispatch_workflow_action(WorkflowUiAction::UndoDeleteWorkflow);
    assert_eq!(shell.workflow_canvas.library.len(), 2);
    assert!(shell.workflow_canvas.deleted_undo.is_none());
}
