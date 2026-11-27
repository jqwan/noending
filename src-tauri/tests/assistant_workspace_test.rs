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
        dsh: home.join(".dsh"),
        home,
    }
}
struct Profile {
    project: Project,
    workspace_path: WorkspacePath,
    files: assistant::ProjectAssistant,
}

fn create_profile(f: &Fixture, name: &str) -> Profile {
    let project = assistant::create_profile_with_roots(
        f.db(),
        &f.home,
        &f.layer,
        noending::assistant::AssistantCreate {
            name: name.into(),
            soul_md: Some(format!("角色代号：{name}")),
            role_md: None,
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
        files: assistant.files,
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
    let identity_path = root.join("IDENTITY.md");
    let identity_id = f
        .project_detail(&first.project.id)
        .unwrap()
        .assistant
        .unwrap()
        .id;
    assert_eq!(
        std::fs::read_to_string(&identity_path).unwrap(),
        format!("# 身份\n\n- 名称：Atlas\n- 智能体 ID：`{identity_id}`\n")
    );
    std::fs::write(root.join("notes.txt"), "保留用户资料").unwrap();
    let renamed =
        project::rename_project(f.db(), &f.home, &first.project.id, "Atlas 改名").unwrap();
    assert_eq!(renamed.id, first.project.id);
    drop(f.db.take());
    f.db = Some(Db::open(&f.home.db_path).unwrap());
    let reopened = f.project_detail(&first.project.id).unwrap();
    assert_eq!(reopened.project.name, "Atlas 改名");
    let assistant = reopened.assistant.unwrap();
    assert_eq!(assistant.id, identity_id);
    assert_eq!(
        std::fs::read_to_string(&identity_path).unwrap(),
        format!("# 身份\n\n- 名称：Atlas 改名\n- 智能体 ID：`{identity_id}`\n")
    );
    assert_eq!(assistant.files.soul.content, "角色代号：Atlas");
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
fn reset_restores_the_managed_guide_and_fills_missing_archive_files() {
    let f = Fixture::new();
    let profile = create_profile(&f, "Curator");
    let peer = create_profile(&f, "Reviewer");
    let root = PathBuf::from(&profile.workspace_path.canonical_path);
    let guide = root.join("AGENTS.md");
    let identity = root.join("IDENTITY.md");
    let expected_identity = std::fs::read_to_string(&identity).unwrap();
    let claude_entry = root.join("CLAUDE.md");
    let guide_template = std::fs::read_to_string(&guide).unwrap();
    assert!(profile.files.guide_is_default);
    assert_eq!(profile.files.agents.content, guide_template);
    assert!(!profile.files.claude_md_missing);
    assert_eq!(
        guide_template,
        std::fs::read_to_string(
            PathBuf::from(&peer.workspace_path.canonical_path).join("AGENTS.md")
        )
        .unwrap()
    );
    assert_eq!(profile.files.soul.content, "角色代号：Curator");
    assert_eq!(profile.files.role.state, "current");
    assert_eq!(std::fs::read_to_string(root.join("ROLE.md")).unwrap(), "");
    assert_eq!(
        std::fs::read_to_string(&claude_entry).unwrap(),
        "@AGENTS.md\n"
    );

    let custom_guide = "先查看发布验收任务。\n";
    let guide_revision = profile.files.agents.revision.as_deref().unwrap();
    let edited_guide = assistant::update_profile(
        f.db(),
        &f.home,
        &profile.project.id,
        guide_revision,
        assistant::AssistantPatch::Agents {
            content: custom_guide.into(),
        },
    )
    .unwrap();
    assert_eq!(edited_guide.agents.content, custom_guide);
    assert!(assistant::update_profile(
        f.db(),
        &f.home,
        &profile.project.id,
        guide_revision,
        assistant::AssistantPatch::Agents {
            content: "过期草稿".into()
        },
    )
    .is_err());
    std::fs::remove_file(root.join("ROLE.md")).unwrap();
    std::fs::remove_file(&claude_entry).unwrap();
    std::fs::remove_file(&identity).unwrap();
    let external = f
        .project_detail(&profile.project.id)
        .unwrap()
        .assistant
        .unwrap()
        .files;
    assert!(!external.guide_is_default);
    assert_eq!(external.agents.state, "current");
    assert_eq!(external.agents.content, custom_guide);
    assert!(external.claude_md_missing);
    assert_eq!(external.role.state, "missing");
    let prepared = assistant::prepare_profile_files(f.db(), &f.home, &profile.project.id).unwrap();
    assert!(prepared.guide_is_default);
    assert_eq!(
        std::fs::read_to_string(&identity).unwrap(),
        expected_identity
    );
    assert!(!prepared.claude_md_missing);
    assert_eq!(
        std::fs::read_to_string(&claude_entry).unwrap(),
        "@AGENTS.md\n"
    );
    assert_eq!(std::fs::read_to_string(&guide).unwrap(), guide_template);
    assert_eq!(prepared.role.state, "current");
    assert!(prepared.role.content.is_empty());
    assert_eq!(prepared.soul.revision, profile.files.soul.revision);

    std::fs::write(root.join("SOUL.md"), "外部更新自我定义").unwrap();
    assert!(assistant::update_profile(
        f.db(),
        &f.home,
        &profile.project.id,
        prepared.soul.revision.as_deref().unwrap(),
        assistant::AssistantPatch::Soul {
            content: "过期草稿".into()
        },
    )
    .is_err());
    let edited = assistant::update_profile(
        f.db(),
        &f.home,
        &profile.project.id,
        prepared.role.revision.as_deref().unwrap(),
        assistant::AssistantPatch::Role {
            content: "接手后核对交付\n".into(),
        },
    )
    .unwrap();
    let repaired = assistant::prepare_profile_files(f.db(), &f.home, &profile.project.id).unwrap();
    assert_eq!(repaired.role.content, "接手后核对交付\n");
    assert_eq!(repaired.soul.content, "外部更新自我定义");
    assert_eq!(repaired.role.revision, edited.role.revision);

    let cleared = assistant::update_profile(
        f.db(),
        &f.home,
        &profile.project.id,
        edited.role.revision.as_deref().unwrap(),
        assistant::AssistantPatch::Role {
            content: String::new(),
        },
    )
    .unwrap();
    assert_eq!(cleared.role.state, "current");
    assert_eq!(std::fs::read_to_string(root.join("ROLE.md")).unwrap(), "");
}

#[test]
fn an_identity_write_failure_rolls_back_the_rename() {
    let f = Fixture::new();
    let profile = create_profile(&f, "原名称");
    let identity = PathBuf::from(&profile.workspace_path.canonical_path).join("IDENTITY.md");
    std::fs::remove_file(&identity).unwrap();
    std::fs::create_dir(&identity).unwrap();

    let error =
        project::rename_project(f.db(), &f.home, &profile.project.id, "新名称").unwrap_err();
    assert!(error.to_string().contains("IDENTITY.md"));
    assert_eq!(
        f.db()
            .get_project(&profile.project.id)
            .unwrap()
            .unwrap()
            .name,
        "原名称"
    );
    assert!(identity.is_dir());
}

#[test]
fn assistant_written_documents_are_read_from_disk_and_survive_guide_reset() {
    let f = Fixture::new();
    let profile = create_profile(&f, "Curator");
    let project_id = &profile.project.id;
    let root = PathBuf::from(&profile.workspace_path.canonical_path);
    let user_path = root.join("USER.md");
    let intro_path = root.join("INTRO.md");
    assert_eq!(profile.files.user.state, "missing");
    assert_eq!(profile.files.intro.state, "missing");
    let prepared = assistant::prepare_profile_files(f.db(), &f.home, project_id).unwrap();
    assert_eq!(prepared.user.state, "missing");
    assert_eq!(prepared.intro.state, "missing");
    assert!(!user_path.exists());
    assert!(!intro_path.exists());

    let impressions = "用户多次先问交付结论，再追问风险。\n";
    let introduction = "## 什么时候找我\n\n我可以帮你核对发布验收。\n";
    std::fs::write(&user_path, impressions).unwrap();
    std::fs::write(&intro_path, introduction).unwrap();
    let files = f
        .project_detail(project_id)
        .unwrap()
        .assistant
        .unwrap()
        .files;
    assert_eq!(files.user.state, "current");
    assert_eq!(files.user.content, impressions);
    assert_eq!(files.intro.state, "current");
    assert_eq!(files.intro.content, introduction);
    let prepared = assistant::prepare_profile_files(f.db(), &f.home, project_id).unwrap();
    assert_eq!(prepared.user.revision, files.user.revision);
    assert_eq!(prepared.intro.revision, files.intro.revision);
    assert_eq!(std::fs::read_to_string(&user_path).unwrap(), impressions);
    assert_eq!(std::fs::read_to_string(&intro_path).unwrap(), introduction);
}

#[test]
fn assistant_project_lists_sessions_by_workspace_and_supplies_collaboration_candidates() {
    use noending::{domain::Agent, workspace::workstream};

    let f = Fixture::new();
    let profile = create_profile(&f, "Coordinator");
    let sid = profile_session(&f, &profile, "project-session");
    let alternate_session = f
        .db()
        .upsert_logical_session_unchecked(
            Agent::Pi,
            "second-archive-session",
            None,
            Some(&profile.workspace_path.canonical_path),
            Some(&profile.workspace_path.id),
            None,
            None,
            None,
        )
        .unwrap()
        .0;
    assert_ne!(sid, alternate_session);
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
    let archive = f
        .project_detail(&profile.project.id)
        .unwrap()
        .assistant
        .unwrap();
    let resolve = |path: &str| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ne"))
            .env("NOENDING_HOME", &f.home.root)
            .args(["project", "resolve", "--path", path])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(output.status.success(), "{value}");
        assert_eq!(value["data"]["project"]["id"], profile.project.id);
        value["data"].clone()
    };
    let resolved = resolve(&profile.workspace_path.canonical_path);
    assert_eq!(resolved["assistant"]["id"], archive.id.to_string());
    assert_ne!(resolved["assistant"]["id"], profile.project.id);
    assert_eq!(
        resolved["assistant"]["archive_directory"],
        profile.workspace_path.canonical_path
    );
    assert!(resolve(&additional_path.canonical_path)
        .get("assistant")
        .is_none());

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
    let unassigned = f.db().get_session(&no_cwd).unwrap().unwrap();
    assert!(unassigned.project_id.is_none());
    assert_eq!(
        f.project_detail(&profile.project.id)
            .unwrap()
            .sessions
            .into_iter()
            .map(|s| s.id)
            .collect::<std::collections::BTreeSet<_>>(),
        [
            sid.clone(),
            alternate_session.clone(),
            other_session.clone()
        ]
        .into_iter()
        .collect()
    );

    let mut candidates = f
        .db()
        .collaboration_candidate_sessions(&task.id, None, 50, None, None)
        .unwrap()
        .items;
    candidates.sort();
    let mut expected = [sid, alternate_session, other_session];
    expected.sort();
    assert_eq!(candidates, expected);

    workstream::set_workstream_projects(f.db(), &task.id, &[]).unwrap();
    assert!(f
        .db()
        .collaboration_candidate_sessions(&task.id, None, 50, None, None)
        .unwrap()
        .items
        .is_empty());
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
            soul_md: None,
            role_md: None,
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
    assistant::prepare_profile_files(f.db(), &f.home, &p.project.id).unwrap();
    assert!(std::path::Path::new(&p.workspace_path.canonical_path)
        .join("AGENTS.md")
        .is_file());
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

#[test]
fn assistant_mail_has_one_handler_and_survives_detached_conversations() {
    use noending::storage::assistant_mailbox::{Action, Change, Filter};
    use std::sync::{Arc, Barrier};

    let f = Fixture::new();
    let profile = create_profile(&f, "发布协调员");
    let assistant = f
        .project_detail(&profile.project.id)
        .unwrap()
        .assistant
        .unwrap()
        .id
        .to_string();
    let first = profile_session(&f, &profile, "inbox-first");
    let second = profile_session(&f, &profile, "inbox-second");
    let ordinary = f
        .db()
        .upsert_logical_session_unchecked(
            noending::domain::Agent::Codex,
            "outside-inbox",
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap()
        .0;
    let letter = f
        .db()
        .assistant_mailbox_send(&f.home, &assistant, Some(&ordinary), "核对发布结果")
        .unwrap();
    let another = f
        .db()
        .assistant_mailbox_send(&f.home, &assistant, None, "准备下次交付")
        .unwrap();
    assert!(f
        .db()
        .assistant_mailbox_list(&f.home, &assistant, Some(&ordinary), Filter::All, None, 10)
        .is_err());
    assert!(f
        .db()
        .assistant_mailbox_send(&f.home, &assistant, Some(&first), "循环")
        .is_err());
    let page = f
        .db()
        .assistant_mailbox_list(&f.home, &assistant, Some(&first), Filter::Pending, None, 1)
        .unwrap();
    assert_eq!(page.items[0].id, another.id);
    assert_eq!(page.unread, 2);
    let second_page = f
        .db()
        .assistant_mailbox_list(
            &f.home,
            &assistant,
            Some(&second),
            Filter::Pending,
            page.next_cursor,
            1,
        )
        .unwrap();
    assert_eq!(second_page.items[0].id, letter.id);
    assert_eq!(second_page.unread, 2);

    let change = |operation, revision, target_session| Change {
        message_id: letter.id.clone(),
        operation,
        revision,
        target_session,
    };
    let claim = change(Action::Claim, 1, None);
    let barrier = Arc::new(Barrier::new(2));
    let claims = std::thread::scope(|scope| {
        let handles = [&first, &second].map(|session| {
            let barrier = barrier.clone();
            let claim = &claim;
            let assistant = &assistant;
            let home = &f.home;
            scope.spawn(move || {
                let db = Db::open(&home.db_path).unwrap();
                barrier.wait();
                db.assistant_mailbox_change(home, assistant, Some(session), claim)
            })
        });
        handles.map(|handle| handle.join().unwrap())
    });
    assert_eq!(claims.iter().filter(|result| result.is_ok()).count(), 1);
    let claimed = claims.into_iter().find_map(Result::ok).unwrap();
    assert!(claimed.read_at.is_none());
    let handler = claimed.handler_session_id.unwrap();
    let other = if handler == first { &second } else { &first };
    assert!(f
        .db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            Some(other),
            &change(Action::Complete, 2, None)
        )
        .is_err());
    let read = f
        .db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            Some(other),
            &change(Action::Read, 2, None),
        )
        .unwrap();
    assert!(read.read_at.is_some());
    assert_eq!(read.handler_session_id.as_deref(), Some(handler.as_str()));

    assert!(f
        .db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            None,
            &change(Action::Transfer, 3, Some(ordinary))
        )
        .is_err());
    let transferred = f
        .db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            None,
            &change(Action::Transfer, 3, Some(other.clone())),
        )
        .unwrap();
    assert_eq!(
        transferred.handler_session_id.as_deref(),
        Some(other.as_str())
    );
    let completed = f
        .db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            Some(other),
            &change(Action::Complete, 4, None),
        )
        .unwrap();
    assert!(completed.completed_at.is_some());
    assert!(f
        .db()
        .assistant_mailbox_change(&f.home, &assistant, None, &change(Action::Release, 5, None))
        .is_err());

    f.db()
        .assistant_mailbox_change(
            &f.home,
            &assistant,
            Some(&handler),
            &Change {
                message_id: another.id.clone(),
                operation: Action::Claim,
                revision: 1,
                target_session: None,
            },
        )
        .unwrap();
    f.db()
        .write()
        .execute("DELETE FROM sessions WHERE id=?1", [&handler])
        .unwrap();
    let released = f
        .db()
        .assistant_mailbox_read(&f.home, &assistant, Some(other), &another.id)
        .unwrap();
    assert!(released.handler_session_id.is_none());
    assert_eq!(released.revision, 3);
    let renamed =
        project::rename_project(f.db(), &f.home, &profile.project.id, "发布负责人").unwrap();
    assert_ne!(renamed.name, profile.project.name);
    let page = f
        .db()
        .assistant_mailbox_list(
            &f.home,
            &assistant,
            Some(other),
            Filter::Completed,
            None,
            10,
        )
        .unwrap();
    assert_eq!(
        page.items[0].handler_session_id.as_deref(),
        Some(other.as_str())
    );
    assert_eq!(page.unread, 1);
    assert_eq!(f.db().mailbox_unread(other).unwrap(), 0);
    let mut survivor = renamed;
    survivor.id = new_id();
    survivor.created_at = "2000-01-01T00:00:00Z".into();
    f.db().upsert_project(&survivor).unwrap();
    let merged = project::merge_projects(f.db(), &survivor.id, &profile.project.id).unwrap();
    assert_eq!(merged.survivor, survivor.id);
    assert_eq!(
        f.db()
            .assistant_mailbox_read(&f.home, &assistant, Some(other), &letter.id)
            .unwrap()
            .completed_at,
        completed.completed_at
    );
}
