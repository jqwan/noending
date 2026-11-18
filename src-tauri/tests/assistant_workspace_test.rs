//! Empty Assistant workspaces retain project identity and user-created files.
use noending::{
    assistant,
    domain::{GitDetection, GitWorktreeKind, ProjectKind, ProjectPurpose},
    storage::{new_id, Db},
    workspace::{home::NoEndingHome, path_identity, project, wiring::WorkspaceLayer},
};
use noending_lib as noending;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    db: Option<Db>,
    home: NoEndingHome,
    layer: WorkspaceLayer,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("assistant-workspace-{}", new_id()));
        let home = NoEndingHome::new(root.to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Db::open(&home.db_path).unwrap();
        let layer = WorkspaceLayer::new(&home);
        Self {
            root,
            db: Some(db),
            home,
            layer,
        }
    }
    fn db(&self) -> &Db {
        self.db.as_ref().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn assistant_project_keeps_identity_after_rename_and_empty_registry_sweep() {
    let f = Fixture::new();
    let first = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    let custom = "我的工作区助手";
    noending::storage::workspace::rename_project_conn(&f.db().write(), &first.project.id, custom)
        .unwrap();
    std::fs::write(
        PathBuf::from(&first.directory).join("instructions.md"),
        "用户自己的长期说明",
    )
    .unwrap();
    std::fs::write(PathBuf::from(&first.directory).join("notes.md"), "用户笔记").unwrap();
    let second = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    assert_eq!(second.project.id, first.project.id);
    assert_eq!(second.project.name, custom);
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&second.directory).join("instructions.md")).unwrap(),
        "用户自己的长期说明"
    );
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&first.directory).join("notes.md")).unwrap(),
        "用户笔记"
    );
    let projection = f.layer.projection();
    let root_id = path_identity(&second.directory);
    project::reconcile_workspace_paths(f.db(), &projection, usize::MAX).unwrap();
    assert!(f.db().get_workspace_path(&root_id).unwrap().is_some());
    let child = PathBuf::from(&second.directory).join("conversations");
    std::fs::create_dir(&child).unwrap();
    let child_path = project::ensure_workspace_path(
        f.db(),
        &projection.observer().observe(child.to_str().unwrap()),
        projection.policy(),
    )
    .unwrap();
    assert_ne!(child_path.project_id, first.project.id);
    let child_detail = project::project_detail(f.db(), &child_path.project_id, projection.policy())
        .unwrap()
        .unwrap();
    assert_eq!(child_detail.kind, ProjectKind::SingleDirectory);
    assert_eq!(child_detail.purpose, None);
    let default = project::ensure_workspace_path(
        f.db(),
        &projection
            .observer()
            .observe(&f.home.default_workspace_str()),
        projection.policy(),
    )
    .unwrap();
    assert_ne!(default.project_id, first.project.id);
    let detail = project::project_detail(f.db(), &first.project.id, projection.policy())
        .unwrap()
        .unwrap();
    assert_eq!(detail.kind, ProjectKind::SingleDirectory);
    assert_eq!(detail.purpose, Some(ProjectPurpose::Assistant));
    assert!(detail.sessions.is_empty());
    let mut observation = projection.observer().observe(&second.directory);
    observation.git = GitDetection::Detected {
        common_dir: PathBuf::from(&second.directory)
            .join(".git")
            .to_string_lossy()
            .to_string(),
        toplevel: Some(second.directory.clone()),
        kind: GitWorktreeKind::Main,
        worktrees: vec![],
        remotes: vec![],
    };
    let upgraded =
        project::ensure_workspace_path(f.db(), &observation, projection.policy()).unwrap();
    assert_eq!(upgraded.project_id, first.project.id);
    let git_detail = project::project_detail(f.db(), &first.project.id, projection.policy())
        .unwrap()
        .unwrap();
    assert_eq!(git_detail.kind, ProjectKind::Git);
    assert_eq!(git_detail.purpose, Some(ProjectPurpose::Assistant));
    observation.git = GitDetection::None;
    project::ensure_workspace_path(f.db(), &observation, projection.policy()).unwrap();
    let returned = project::project_detail(f.db(), &first.project.id, projection.policy())
        .unwrap()
        .unwrap();
    assert_eq!(returned.kind, ProjectKind::SingleDirectory);
    assert_eq!(returned.purpose, Some(ProjectPurpose::Assistant));
    assert_eq!(returned.project.name, custom);

    // Missing directories stay registered, and built-in purpose still forbids deletion.
    std::fs::remove_dir_all(&child).unwrap();
    std::fs::remove_dir_all(&second.directory).unwrap();
    project::reconcile_workspace_paths(f.db(), &projection, 100).unwrap();
    let retained = f.db().get_workspace_path(&root_id).unwrap().unwrap();
    assert!(!retained.exists);
    assert!(f.db().get_project(&first.project.id).unwrap().is_some());
    project::set_project_archived(f.db(), &first.project.id, true).unwrap();
    assert!(
        project::delete_project_permanently(f.db(), &first.project.id, projection.policy())
            .is_err()
    );
    let recreated = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    assert_eq!(recreated.project.id, first.project.id);
    assert_eq!(recreated.project.visibility, "archived");
}

#[test]
fn assistant_workspace_stays_empty_across_reopening_until_users_add_files() {
    let mut f = Fixture::new();
    let first = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    let root = PathBuf::from(&first.directory);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    drop(f.db.take());
    f.db = Some(Db::open(&f.home.db_path).unwrap());
    let reopened = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    assert_eq!(reopened.project.id, first.project.id);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    std::fs::write(root.join("AGENTS.md"), "User project instructions").unwrap();
    assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "User project instructions"
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}

fn profile_roots(f: &Fixture) -> noending::skills::Roots {
    let home = f.root.join("native-home");
    std::fs::create_dir_all(&home).unwrap();
    noending::skills::Roots {
        codex: home.join(".codex"),
        claude: home.join(".claude"),
        pi: home.join(".pi/agent"),
        home,
    }
}
fn create_profile(f: &Fixture, name: &str) -> noending::assistant::AssistantDetail {
    noending::assistant::create_profile_with_roots(
        f.db(),
        &f.home,
        &f.layer,
        noending::assistant::AssistantCreate {
            name: name.into(),
            agent: "codex".into(),
            avatar: "assistant".into(),
            description: "管理明确分配的任务".into(),
            instructions: format!("角色代号：{name}"),
            runtime_override: None,
        },
        &profile_roots(f),
    )
    .unwrap()
}
fn profile_session(
    f: &Fixture,
    profile: &noending::assistant::AssistantDetail,
    native: &str,
) -> String {
    f.db()
        .upsert_logical_session_unchecked(
            noending::domain::Agent::Codex,
            native,
            Some("Manager"),
            Some(&profile.workspace_path.canonical_path),
            Some(&profile.workspace_path.id),
            None,
            None,
            None,
        )
        .unwrap()
        .0
}
#[test]
fn assistant_profiles_keep_independent_projects_and_survive_rename_reopen_and_reconcile() {
    let mut f = Fixture::new();
    let legacy = assistant::ensure_workspace(f.db(), &f.home, &f.layer).unwrap();
    let first = create_profile(&f, "Atlas");
    let private = f.root.join("atlas-only");
    std::fs::create_dir(&private).unwrap();
    std::fs::write(
        private.join("SKILL.md"),
        "---\nname: atlas-only\ndescription: Atlas private capability\n---\nUse only in Atlas.\n",
    )
    .unwrap();
    let source = std::fs::canonicalize(private.join("SKILL.md"))
        .unwrap()
        .to_string_lossy()
        .to_string();
    noending::skills::enable_skill_project(
        &profile_roots(&f),
        &[PathBuf::from(&first.workspace_path.canonical_path)],
        &noending::skills::source_id(&source),
        Some(&source),
    )
    .unwrap();
    let second = create_profile(&f, "Muse");
    let first_skills = noending::skills::list(
        &profile_roots(&f),
        Some(std::path::Path::new(&first.workspace_path.canonical_path)),
    );
    let second_skills = noending::skills::list(
        &profile_roots(&f),
        Some(std::path::Path::new(&second.workspace_path.canonical_path)),
    );
    assert!(first_skills
        .resources
        .iter()
        .any(|s| s.name == "atlas-only" && s.error.is_none()));
    assert!(!second_skills
        .resources
        .iter()
        .any(|s| s.name == "atlas-only"));
    assert!(second_skills.resources.iter().any(|s| s.name == "noending"
        && s.error.is_none()
        && std::path::Path::new(&s.path).is_file()));
    assert_ne!(first.project.id, second.project.id);
    assert_ne!(first.project.id, legacy.project.id);
    assert_eq!(first.instructions_state, "current");
    let root = PathBuf::from(&first.workspace_path.canonical_path);
    std::fs::write(root.join("notes.txt"), "保留用户资料").unwrap();
    let renamed = assistant::update_profile(
        f.db(),
        &f.home,
        &first.profile.id,
        first.profile.revision,
        assistant::AssistantPatch {
            name: Some("Atlas 改名".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(renamed.workspace_path.id, first.workspace_path.id);
    assert_eq!(renamed.instructions_state, "outdated");
    assert!(assistant::update_profile(
        f.db(),
        &f.home,
        &first.profile.id,
        first.profile.revision,
        assistant::AssistantPatch {
            name: Some("覆盖并发编辑".into()),
            ..Default::default()
        }
    )
    .is_err());
    let fixed = assistant::prepare_profile_files(
        f.db(),
        &f.home,
        &renamed.profile.id,
        renamed.profile.revision,
    )
    .unwrap();
    assert_eq!(fixed.instructions_state, "current");
    assert!(std::fs::read_to_string(root.join("AGENTS.md"))
        .unwrap()
        .contains("Atlas 改名"));
    drop(f.db.take());
    f.db = Some(Db::open(&f.home.db_path).unwrap());
    let reopened = assistant::detail(f.db(), &first.profile.id).unwrap();
    assert_eq!(reopened.profile.name, "Atlas 改名");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "保留用户资料"
    );
    project::reconcile_workspace_paths(f.db(), &f.layer.projection(), usize::MAX).unwrap();
    assert_eq!(
        assistant::detail(f.db(), &first.profile.id)
            .unwrap()
            .project
            .id,
        first.project.id
    );
    let sibling = PathBuf::from(&first.workspace_path.canonical_path).join("resources");
    std::fs::create_dir(&sibling).unwrap();
    project::ensure_workspace_path(
        f.db(),
        &f.layer
            .projection()
            .observer()
            .observe(sibling.to_str().unwrap()),
        f.layer.projection().policy(),
    )
    .unwrap();
    assert_eq!(
        assistant::list_profiles(f.db(), assistant::AssistantListQuery::default())
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(
        assistant::ensure_workspace(f.db(), &f.home, &f.layer)
            .unwrap()
            .project
            .id
            == legacy.project.id
    );
}
#[test]
fn assistant_generated_files_preserve_user_edits_and_runtime_patch_distinguishes_missing_null_and_empty(
) {
    let f = Fixture::new();
    let p = create_profile(&f, "Curator");
    let root = PathBuf::from(&p.workspace_path.canonical_path);
    let with_runtime = assistant::update_profile(
        f.db(),
        &f.home,
        &p.profile.id,
        p.profile.revision,
        serde_json::from_value(serde_json::json!({"runtime_override":{"model":"custom-model"}}))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        with_runtime
            .profile
            .runtime_override
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("custom-model")
    );
    let omitted = assistant::update_profile(
        f.db(),
        &f.home,
        &p.profile.id,
        with_runtime.profile.revision,
        serde_json::from_value(serde_json::json!({"description":"用户说明"})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        omitted.profile.runtime_override,
        with_runtime.profile.runtime_override
    );
    let empty = assistant::update_profile(
        f.db(),
        &f.home,
        &p.profile.id,
        omitted.profile.revision,
        serde_json::from_value(serde_json::json!({"runtime_override":{}})).unwrap(),
    )
    .unwrap();
    assert_eq!(empty.profile.runtime_override, Some(Default::default()));
    let inherited = assistant::update_profile(
        f.db(),
        &f.home,
        &p.profile.id,
        empty.profile.revision,
        serde_json::from_value(serde_json::json!({"runtime_override":null})).unwrap(),
    )
    .unwrap();
    assert_eq!(inherited.profile.runtime_override, None);
    std::fs::write(root.join("AGENTS.md"), "用户手动修改的角色规则").unwrap();
    assert_eq!(
        assistant::detail(f.db(), &p.profile.id)
            .unwrap()
            .instructions_state,
        "conflict"
    );
    assert!(assistant::prepare_profile_files(
        f.db(),
        &f.home,
        &p.profile.id,
        inherited.profile.revision
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "用户手动修改的角色规则"
    );
    assert!(std::fs::read_to_string(root.join("CLAUDE.md"))
        .unwrap()
        .contains("@AGENTS.md"));
}
#[test]
fn assistant_management_tasks_do_not_expand_task_projects_or_grant_unassigned_sessions_access() {
    use noending::{domain::Agent, workspace::workstream};
    let f = Fixture::new();
    let p = create_profile(&f, "Manager");
    let other = create_profile(&f, "Unassigned");
    let manager = profile_session(&f, &p, "manager-native");
    let outsider = profile_session(&f, &other, "outsider-native");
    let task = workstream::create_workstream(f.db(), "工程任务", "验收结果", &[]).unwrap();
    assert!(f
        .db()
        .set_session_tracking(&manager, &task.id, true)
        .is_err());
    let assigned = assistant::set_responsibilities(
        f.db(),
        &f.home,
        &p.profile.id,
        p.profile.revision,
        &[],
        &[task.id.clone()],
    )
    .unwrap();
    assert!(f
        .db()
        .set_session_tracking(&outsider, &task.id, true)
        .is_err());
    let ordinary = f
        .db()
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "ordinary",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .0;
    assert!(f
        .db()
        .set_session_tracking(&ordinary, &task.id, true)
        .is_err());
    f.db()
        .set_session_tracking(&manager, &task.id, true)
        .unwrap();
    assert!(f
        .db()
        .collaboration_candidate_sessions(&task.id, None, 50)
        .unwrap()
        .items
        .contains(&manager));
    assert_eq!(
        f.db()
            .get_session(&manager)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        [task.id.clone()]
    );
    assert!(
        noending::storage::workstream_projects::project_ids_for_workstream(
            &f.db().read(),
            &task.id
        )
        .unwrap()
        .is_empty()
    );
    let (profile, launch_path, _) =
        assistant::profile_launch(f.db(), &f.home, &p.profile.id, &[task.id.clone()], None)
            .unwrap();
    assert_eq!(profile.id, p.profile.id);
    assert_eq!(launch_path.id, p.workspace_path.id);
    let path = launch_path.canonical_path.clone();
    let draft = noending::launcher::SessionLauncher {
        runtime_dir: f.home.runtime_dir.clone(),
    };
    // Reuse the same public preview validation without requiring a real installed CLI.
    let install = noending::platform::exec_resolver::AgentInstallation {
        agent: Agent::Codex,
        executable_path: "test-codex".into(),
        version: None,
    };
    f.db().save_installation(&install).unwrap();
    let preview = draft
        .prepare_new_with_runtime_in(
            f.db(),
            Agent::Codex,
            &[task.id.clone()],
            Some(&path),
            &noending::launcher::LaunchWorkspace::from_home(&f.home),
            None,
        )
        .unwrap();
    assert_eq!(preview.initial_workstream_ids, [task.id.clone()]);
    let unknown_cwd = f
        .db()
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "native-no-cwd",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .0;
    let session = f.db().get_session(&unknown_cwd).unwrap().unwrap();
    f.db()
        .inherit_terminal_tracking_for_assistant(&session, &[task.id.clone()], Some(&p.profile.id))
        .unwrap();
    assert_eq!(
        f.db()
            .get_session(&unknown_cwd)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        [task.id.clone()]
    );
    assert_eq!(
        noending::storage::assistant_profiles::profile_for_session(&f.db().read(), &unknown_cwd)
            .unwrap()
            .unwrap()
            .id,
        p.profile.id
    );
    let removed = assistant::set_responsibilities(
        f.db(),
        &f.home,
        &p.profile.id,
        assigned.profile.revision,
        &[],
        &[],
    )
    .unwrap();
    let next = profile_session(&f, &p, "next-manager");
    assert!(f.db().set_session_tracking(&next, &task.id, true).is_err());
    assert!(!f
        .db()
        .collaboration_candidate_sessions(&task.id, None, 50)
        .unwrap()
        .items
        .contains(&manager));
    assert_eq!(
        f.db()
            .get_session(&manager)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        [task.id.clone()]
    );
    let archived = assistant::set_archived(
        f.db(),
        &f.home,
        &p.profile.id,
        removed.profile.revision,
        true,
    )
    .unwrap();
    assert!(assistant::profile_launch(f.db(), &f.home, &p.profile.id, &[], None).is_err());
    assert!(draft
        .prepare_new_with_runtime_in(
            f.db(),
            Agent::Codex,
            &[],
            Some(&path),
            &noending::launcher::LaunchWorkspace::from_home(&f.home),
            None
        )
        .is_err());
    assert_eq!(
        assistant::list_profile_sessions(
            f.db(),
            &p.profile.id,
            assistant::AssistantListQuery::default()
        )
        .unwrap()
        .items
        .len(),
        3
    );
    let restored = assistant::set_archived(
        f.db(),
        &f.home,
        &p.profile.id,
        archived.profile.revision,
        false,
    )
    .unwrap();
    assert_eq!(restored.profile.visibility, "normal");
    assert_eq!(restored.workspace_path.id, p.workspace_path.id);
}
#[test]
fn assistant_profile_pages_preserve_filters_and_archive_visibility() {
    let f = Fixture::new();
    let first = create_profile(&f, "Alpha");
    let second = create_profile(&f, "Beta");
    let _third = create_profile(&f, "Gamma");
    let one = assistant::list_profiles(
        f.db(),
        assistant::AssistantListQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(one.items.len(), 1);
    assert!(!one.end);
    let two = assistant::list_profiles(
        f.db(),
        assistant::AssistantListQuery {
            limit: Some(1),
            cursor: one.next_cursor.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(one.items[0].id, two.items[0].id);
    assert!(assistant::list_profiles(
        f.db(),
        assistant::AssistantListQuery {
            cursor: one.next_cursor,
            scope: Some("all".into()),
            ..Default::default()
        }
    )
    .is_err());
    assistant::set_archived(
        f.db(),
        &f.home,
        &first.profile.id,
        first.profile.revision,
        true,
    )
    .unwrap();
    let live = assistant::list_profiles(f.db(), assistant::AssistantListQuery::default()).unwrap();
    assert_eq!(live.items.len(), 2);
    assert!(live.items.iter().any(|p| p.id == second.profile.id));
    let archived = assistant::list_profiles(
        f.db(),
        assistant::AssistantListQuery {
            scope: Some("archived".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        archived
            .items
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        [first.profile.id.as_str()]
    );
}

#[test]
fn assistant_failed_skill_preparation_leaves_no_published_role_or_directory() {
    let f = Fixture::new();
    let first = create_profile(&f, "Existing");
    // Simulate an explicit user uninstall of this fixture's managed Skill.
    std::fs::remove_dir_all(f.home.root.join("skills/noending")).unwrap();
    let failure = assistant::create_profile_with_roots(
        f.db(),
        &f.home,
        &f.layer,
        assistant::AssistantCreate {
            name: "Unpublished".into(),
            agent: "codex".into(),
            avatar: String::new(),
            description: String::new(),
            instructions: String::new(),
            runtime_override: None,
        },
        &profile_roots(&f),
    );
    assert!(failure.is_err());
    let profiles =
        assistant::list_profiles(f.db(), assistant::AssistantListQuery::default()).unwrap();
    assert_eq!(
        profiles
            .items
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        [first.profile.id.as_str()]
    );
    let published = std::fs::read_dir(assistant::directory(&f.home))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(published, [std::ffi::OsString::from(&first.profile.id)]);
    assert!(!std::fs::read_dir(&f.home.runtime_dir)
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("assistant-create-")));
}

#[test]
fn missing_assistant_main_directory_remains_registered_and_its_project_cannot_be_purged() {
    let f = Fixture::new();
    let p = create_profile(&f, "Retained role");
    std::fs::remove_dir_all(&p.workspace_path.canonical_path).unwrap();
    project::reconcile_workspace_paths(f.db(), &f.layer.projection(), usize::MAX).unwrap();
    let before = project::project_detail(f.db(), &p.project.id, f.layer.projection().policy())
        .unwrap()
        .unwrap();
    assert!(!before.workspace_paths[0].exists);
    assert!(before.cleanable_missing_path_ids.is_empty());
    assert_eq!(
        project::clean_missing_project_paths(f.db(), &p.project.id).unwrap(),
        0
    );
    assert!(f
        .db()
        .get_workspace_path(&p.workspace_path.id)
        .unwrap()
        .is_some());
    project::set_project_archived(f.db(), &p.project.id, true).unwrap();
    let reason =
        project::delete_project_permanently(f.db(), &p.project.id, f.layer.projection().policy())
            .unwrap_err();
    assert!(reason.to_string().contains("智能体档案"), "{reason}");
    assert!(f.db().get_project(&p.project.id).unwrap().is_some());
    assert_eq!(
        assistant::detail(f.db(), &p.profile.id)
            .unwrap()
            .workspace_path
            .id,
        p.workspace_path.id
    );
    let restored =
        assistant::prepare_profile_files(f.db(), &f.home, &p.profile.id, p.profile.revision)
            .unwrap();
    assert_eq!(restored.instructions_state, "current");
    assert!(std::path::Path::new(&p.workspace_path.canonical_path).is_dir());
}
