//! Assistant profiles share the normal project lifecycle.
use noending::{
    assistant,
    domain::{Project, WorkspacePath},
    storage::{new_id, Db},
    workspace::{home::NoEndingHome, project, wiring::WorkspaceLayer},
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

    fn project_detail(&self, id: &str) -> Option<project::ProjectDetail> {
        project::project_detail(self.db(), id, self.layer.projection().policy()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        drop(self.db.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
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
struct Profile {
    project: Project,
    workspace_path: WorkspacePath,
    file: assistant::ProjectAssistant,
}

fn create_profile(f: &Fixture, name: &str) -> Profile {
    let project = assistant::create_profile_with_roots(
        f.db(),
        &f.home,
        &f.layer,
        noending::assistant::AssistantCreate {
            name: name.into(),
            agents_md: format!("角色代号：{name}"),
        },
        &profile_roots(f),
    )
    .unwrap();
    let detail = f.project_detail(&project.id).unwrap();
    let assistant = detail.assistant.unwrap();
    let workspace_path = detail
        .workspace_paths
        .into_iter()
        .find(|path| path.id == assistant.workspace_path_id)
        .unwrap();
    Profile {
        project,
        workspace_path,
        file: assistant.file,
    }
}

fn profile_session(f: &Fixture, profile: &Profile, native: &str) -> String {
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
    assert_ne!(first.project.id, second.project.id);
    let conflict =
        project::merge_projects(f.db(), &first.project.id, &second.project.id).unwrap_err();
    assert!(conflict.to_string().contains("不能自动合并"));
    assert!(f
        .project_detail(&second.project.id)
        .unwrap()
        .assistant
        .is_some());
    let root = PathBuf::from(&first.workspace_path.canonical_path);
    std::fs::write(root.join("notes.txt"), "保留用户资料").unwrap();
    let renamed = project::rename_project(f.db(), &first.project.id, "Atlas 改名").unwrap();
    assert_eq!(renamed.id, first.project.id);
    drop(f.db.take());
    f.db = Some(Db::open(&f.home.db_path).unwrap());
    let reopened = f.project_detail(&first.project.id).unwrap();
    assert_eq!(reopened.project.name, "Atlas 改名");
    let assistant = reopened.assistant.unwrap();
    assert_eq!(assistant.file.agents_md, "角色代号：Atlas");
    assert_eq!(assistant.workspace_path_id, first.workspace_path.id);
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "保留用户资料"
    );
    project::reconcile_workspace_paths(f.db(), &f.layer.projection(), usize::MAX).unwrap();
    let reconciled = f.project_detail(&first.project.id).unwrap();
    assert_eq!(
        reconciled.assistant.unwrap().workspace_path_id,
        first.workspace_path.id
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
        noending::commands::project::project_cards(f.db(), f.layer.projection().policy())
            .unwrap()
            .iter()
            .filter(|card| card.is_assistant)
            .count(),
        2
    );
}
#[test]
fn assistant_generated_files_preserve_user_edits() {
    let f = Fixture::new();
    let p = create_profile(&f, "Curator");
    let root = PathBuf::from(&p.workspace_path.canonical_path);
    std::fs::write(root.join("AGENTS.md"), "用户手动修改的文件").unwrap();
    let external = f.project_detail(&p.project.id).unwrap().assistant.unwrap();
    assert_eq!(external.file.agents_md, "用户手动修改的文件");
    assert_ne!(external.file.revision, p.file.revision);
    assert!(assistant::update_profile(
        f.db(),
        &f.home,
        &p.project.id,
        p.file.revision.as_deref().unwrap(),
        assistant::AssistantPatch {
            agents_md: "过期草稿".into()
        },
    )
    .is_err());
    let prepared = assistant::prepare_profile_files(f.db(), &f.home, &p.project.id).unwrap();
    assert_eq!(prepared.agents_md, "用户手动修改的文件");
    let updated = assistant::update_profile(
        f.db(),
        &f.home,
        &p.project.id,
        external.file.revision.as_deref().unwrap(),
        assistant::AssistantPatch {
            agents_md: "在 NoEnding 编辑\n".into(),
        },
    )
    .unwrap();
    assert_eq!(updated.agents_md, "在 NoEnding 编辑\n");
    assert_eq!(
        std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
        "在 NoEnding 编辑\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
        "@AGENTS.md\n"
    );
}
#[test]
fn assistant_project_lists_sessions_by_workspace_and_keeps_existing_task_tracking() {
    use noending::{
        domain::Agent,
        launcher::{LaunchWorkspace, SessionLauncher},
        workspace::workstream,
    };

    let f = Fixture::new();
    let profile = create_profile(&f, "Coordinator");
    let sid = profile_session(&f, &profile, "project-session");
    let additional_dir = f.root.join("additional-workspace");
    std::fs::create_dir(&additional_dir).unwrap();
    let projection = f.layer.projection();
    let additional_path = project::ensure_workspace_path(
        f.db(),
        &projection
            .observer()
            .observe(additional_dir.to_str().unwrap()),
        projection.policy(),
    )
    .unwrap();
    let merged =
        project::merge_projects(f.db(), &profile.project.id, &additional_path.project_id).unwrap();
    assert_eq!(merged.survivor, profile.project.id);
    let other_session = f
        .db()
        .upsert_logical_session_unchecked(
            Agent::Codex,
            "additional-directory-session",
            None,
            Some(&additional_path.canonical_path),
            Some(&additional_path.id),
            None,
            None,
            None,
        )
        .unwrap()
        .0;
    let task = workstream::create_workstream(f.db(), "项目任务", "", &[profile.project.id.clone()])
        .unwrap();
    let other_task = workstream::create_workstream(f.db(), "无项目任务", "", &[]).unwrap();

    let path = &profile.workspace_path;
    let launcher = SessionLauncher {
        runtime_dir: f.home.runtime_dir.clone(),
    };
    let workspace = LaunchWorkspace::from_home(&f.home);
    let prepare = |id: &str| {
        launcher.prepare_new_with_runtime_in(
            f.db(),
            Agent::Codex,
            &[id.to_owned()],
            Some(&path.canonical_path),
            &workspace,
            None,
        )
    };
    assert_eq!(
        prepare(&task.id).unwrap().initial_workstream_ids,
        [task.id.clone()]
    );
    assert!(prepare(&other_task.id)
        .unwrap_err()
        .to_string()
        .contains("未关联"));
    assert!(f
        .db()
        .set_session_tracking(&sid, &other_task.id, true)
        .is_err());
    let session = f.db().get_session(&sid).unwrap().unwrap();
    f.db()
        .inherit_terminal_tracking(&session, &[task.id.clone(), other_task.id.clone()])
        .unwrap();

    let no_cwd = f
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
    let session = f.db().get_session(&no_cwd).unwrap().unwrap();
    f.db()
        .inherit_terminal_tracking(&session, &[task.id.clone()])
        .unwrap();
    let unassigned = f.db().get_session(&no_cwd).unwrap().unwrap();
    assert!(unassigned.project_id.is_none());
    assert!(unassigned.tracked_workstream_ids.is_empty());
    assert_eq!(
        f.project_detail(&profile.project.id)
            .unwrap()
            .sessions
            .into_iter()
            .map(|s| s.id)
            .collect::<std::collections::BTreeSet<_>>(),
        [sid.clone(), other_session].into_iter().collect()
    );

    workstream::set_workstream_projects(f.db(), &task.id, &[]).unwrap();
    let candidates = f
        .db()
        .collaboration_candidate_sessions(&task.id, None, 50)
        .unwrap()
        .items;
    assert!(candidates.contains(&sid));
    assert!(!candidates.contains(&no_cwd));
    assert_eq!(
        f.db()
            .get_session(&sid)
            .unwrap()
            .unwrap()
            .tracked_workstream_ids,
        [task.id]
    );
}

#[test]
fn assistant_project_with_an_archived_session_cannot_be_deleted() {
    let f = Fixture::new();
    let first = create_profile(&f, "Alpha");
    let session = profile_session(&f, &first, "role-session");
    noending::lifecycle::archive_session(f.db(), &session).unwrap();
    project::set_project_archived(f.db(), &first.project.id, true).unwrap();
    let projection = f.layer.projection();
    let error = project::delete_project_permanently(
        f.db(),
        &first.project.id,
        projection.policy(),
        Some(&f.home),
    )
    .unwrap_err();
    assert!(error.to_string().contains("仍有关联会话"));
    assert!(PathBuf::from(&first.workspace_path.canonical_path)
        .join("AGENTS.md")
        .is_file());
}

#[test]
fn assistant_creation_prepares_native_skills_despite_a_single_agent_user_entry() {
    let f = Fixture::new();
    let roots = profile_roots(&f);
    noending::skills::prepare_noending_skill(f.db(), &f.home, &std::env::current_exe().unwrap())
        .unwrap();
    let registry = noending::skills::load_skill_registry(f.db()).unwrap();
    let source = registry
        .sources
        .iter()
        .find(|s| s.name == "noending")
        .unwrap();
    let inventory = noending::skills::list(&roots, None);
    let claude = inventory
        .locations
        .iter()
        .find(|location| {
            location.scope == "user" && location.agent == Some(noending::domain::Agent::ClaudeCode)
        })
        .unwrap();
    noending::skills::enable_skill(&roots, None, &source.id, Some(&source.path), &claude.id)
        .unwrap();
    let user_skill = roots.claude.join("skills/noending/SKILL.md");
    assert!(user_skill.is_file());
    assert!(!roots.home.join(".agents/skills/noending/SKILL.md").exists());

    let profile = create_profile(&f, "Native skills");
    let cwd = PathBuf::from(&profile.workspace_path.canonical_path);
    let shared_skill = cwd.join(".agents/skills/noending/SKILL.md");
    // This native project entry is read by both Codex and Pi.
    assert_eq!(
        std::fs::canonicalize(&shared_skill).unwrap(),
        std::fs::canonicalize(&source.path).unwrap()
    );
    assert!(cwd.join(".claude/skills/noending/SKILL.md").is_file());
    assert!(cwd.join(".workbuddy/skills/noending/SKILL.md").is_file());
    assert!(user_skill.is_file());

    noending::skills::disable_skill_project(&roots, &[cwd.clone()], &source.id).unwrap();
    assistant::prepare_profile_files(f.db(), &f.home, &profile.project.id).unwrap();
    assert!(!shared_skill.exists());
    assert!(!cwd.join(".claude/skills/noending/SKILL.md").exists());
    assert!(user_skill.is_file());
    assert!(std::path::Path::new(&source.path).is_file());
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
            agents_md: String::new(),
        },
        &profile_roots(&f),
    );
    assert!(failure.is_err());
    let projects = f.db().list_projects().unwrap();
    assert_eq!(
        projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<Vec<_>>(),
        [first.project.id.as_str()]
    );
    let published = std::fs::read_dir(assistant::directory(&f.home))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        published,
        [PathBuf::from(&first.workspace_path.canonical_path)
            .file_name()
            .unwrap()
            .to_os_string()]
    );
    assert!(!std::fs::read_dir(&f.home.runtime_dir)
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("assistant-create-")));
}

#[test]
fn missing_generated_working_directory_can_be_recovered_while_the_profile_exists() {
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
    let restored = assistant::prepare_profile_files(f.db(), &f.home, &p.project.id).unwrap();
    assert_eq!(restored.agents_md_state, "current");
    assert!(std::path::Path::new(&p.workspace_path.canonical_path).is_dir());
}

#[test]
fn assistant_project_archive_is_shared_and_deleting_it_removes_only_the_owned_directory() {
    let f = Fixture::new();
    let profile = create_profile(&f, "Curator");
    let peer = create_profile(&f, "Reviewer");
    let project_id = &profile.project.id;
    let projection = f.layer.projection();
    let policy = projection.policy();
    let directory = PathBuf::from(&profile.workspace_path.canonical_path);
    let notes = directory.join("notes.md");
    std::fs::write(&notes, "role notes").unwrap();
    let ordinary_dir = f.root.join("project");
    std::fs::create_dir(&ordinary_dir).unwrap();
    std::fs::write(ordinary_dir.join("keep.txt"), "ordinary data").unwrap();
    let ordinary_path = project::ensure_workspace_path(
        f.db(),
        &projection
            .observer()
            .observe(ordinary_dir.to_str().unwrap()),
        policy,
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&ordinary_dir, directory.join("linked-project")).unwrap();

    let cards = noending::commands::project::project_cards(f.db(), policy).unwrap();
    assert!(cards
        .iter()
        .any(|card| card.id == *project_id && card.is_assistant));
    assert!(cards
        .iter()
        .any(|card| card.id == ordinary_path.project_id && !card.is_assistant));
    let detail = project::project_detail(f.db(), project_id, policy)
        .unwrap()
        .unwrap();
    assert_eq!(
        detail.assistant.unwrap().workspace_path_id,
        profile.workspace_path.id
    );
    assert!(detail.purpose.is_none());
    project::set_project_archived(f.db(), project_id, true).unwrap();
    assert_eq!(
        f.project_detail(project_id).unwrap().project.visibility,
        "archived"
    );
    assert!(notes.is_file());
    project::set_project_archived(f.db(), project_id, false).unwrap();
    assert_eq!(
        f.project_detail(project_id).unwrap().project.visibility,
        "normal"
    );
    project::set_project_archived(f.db(), project_id, true).unwrap();
    f.db()
        .write()
        .execute_batch(
            "CREATE TEMP TRIGGER refuse_project_removal BEFORE DELETE ON projects
         BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;",
        )
        .unwrap();
    assert!(
        project::delete_project_permanently(f.db(), project_id, policy, Some(&f.home)).is_err()
    );
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "role notes");
    assert!(f
        .project_detail(&profile.project.id)
        .unwrap()
        .assistant
        .is_some());
    f.db()
        .write()
        .execute_batch("DROP TRIGGER refuse_project_removal")
        .unwrap();
    let removed =
        project::delete_project_permanently(f.db(), project_id, policy, Some(&f.home)).unwrap();
    assert!(removed.warnings.is_empty());
    assert!(!directory.exists());

    assert!(f.db().get_project(project_id).unwrap().is_none());
    assert!(PathBuf::from(&peer.workspace_path.canonical_path).is_dir());
    assert_eq!(
        std::fs::read_to_string(ordinary_dir.join("keep.txt")).unwrap(),
        "ordinary data"
    );
}

#[cfg(unix)]
#[test]
fn assistant_deletion_rejects_a_replaced_working_directory_without_touching_its_target() {
    let f = Fixture::new();
    let profile = create_profile(&f, "Coordinator");
    let directory = PathBuf::from(&profile.workspace_path.canonical_path);
    let moved = directory.with_extension("moved");
    std::fs::rename(&directory, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &directory).unwrap();
    project::set_project_archived(f.db(), &profile.project.id, true).unwrap();
    let error = project::delete_project_permanently(
        f.db(),
        &profile.project.id,
        f.layer.projection().policy(),
        Some(&f.home),
    )
    .unwrap_err();
    assert!(error.to_string().contains("已被替换"));
    assert!(moved.join("AGENTS.md").is_file());
    assert!(f
        .project_detail(&profile.project.id)
        .unwrap()
        .assistant
        .is_some());
}
