//! Built-in project identity and persistent user instructions.
use noending::{
    assistant,
    domain::{GitDetection, GitWorktreeKind, ProjectKind, ProjectPurpose},
    skills::{self, Roots},
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
    roots: Roots,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("noending-assistant-{}", new_id()));
        let home = NoEndingHome::new(root.to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = Db::open(&home.db_path).unwrap();
        let layer = WorkspaceLayer::new(&home);
        Self {
            roots: Roots {
                home: root.join("agent-home"),
                codex: root.join("agent-home/.codex"),
                claude: root.join("agent-home/.claude"),
                pi: root.join("agent-home/.pi/agent"),
            },
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
    let first = assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    let custom = "我的工作区助手";
    noending::storage::workspace::rename_project_conn(&f.db().write(), &first.project.id, custom)
        .unwrap();
    std::fs::write(
        PathBuf::from(&first.directory).join("USER.md"),
        "用户自己的长期说明",
    )
    .unwrap();
    std::fs::write(PathBuf::from(&first.directory).join("notes.md"), "用户笔记").unwrap();
    let second = assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    assert_eq!(second.project.id, first.project.id);
    assert_eq!(second.project.name, custom);
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&second.directory).join("USER.md")).unwrap(),
        "用户自己的长期说明"
    );
    assert_eq!(
        std::fs::read_to_string(PathBuf::from(&first.directory).join("notes.md")).unwrap(),
        "用户笔记"
    );
    assert!(f
        .home
        .root
        .join("skills/noending-assistant/references/cli.md")
        .is_file());
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
    let recreated = assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    assert_eq!(recreated.project.id, first.project.id);
    assert_eq!(recreated.project.visibility, "archived");
}

#[test]
fn assistant_cli_access_does_not_grant_access_to_application_data_or_other_workspaces() {
    let f = Fixture::new();
    let workspace = assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    assert_eq!(
        assistant::cli_writable_directories(&f.home.runtime_dir, Some(&workspace.directory)),
        vec![f.home.runtime_dir.join("cli").to_string_lossy().to_string()]
    );
    for cwd in [
        None,
        Some(f.home.default_workspace_str()),
        Some(format!("{}-other", workspace.directory)),
        Some(format!("{}/tmp", workspace.directory)),
    ] {
        assert!(
            assistant::cli_writable_directories(&f.home.runtime_dir, cwd.as_deref()).is_empty()
        );
    }
}

#[test]
fn assistant_skill_uses_the_library_and_respects_project_disable_and_uninstall() {
    let f = Fixture::new();
    let workspace = assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    let root = PathBuf::from(&workspace.directory);
    let library = f.home.root.join("skills");
    let file = std::fs::canonicalize(library.join("noending-assistant/SKILL.md")).unwrap();
    assert!(library
        .join("noending-assistant/references/cli.md")
        .is_file());
    assert!(!root.join("skills/noending-assistant").exists());
    // The workspace instructions must not bypass native Skill enablement.
    let entry = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(!entry.contains("SKILL.md"));
    let registry = skills::load_skill_registry(f.db()).unwrap();
    let source = registry
        .sources
        .iter()
        .find(|source| source.name == "noending-assistant")
        .unwrap();
    assert_eq!(std::path::Path::new(&source.path), file);
    let mut inventory = skills::list_catalog(&f.roots, &[root.clone()]);
    skills::merge_skill_sources(&registry, &mut inventory, true, &library);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.id == source.id)
        .unwrap();
    assert!(row.registered);
    assert!(!row.entry_links.is_empty());
    assert!(row.entry_links.iter().all(|link| *link));
    let scopes = skills::project_scopes_from_inventory(
        &f.roots,
        &inventory,
        &[(
            workspace.project.id.clone(),
            workspace.project.name.clone(),
            vec![("assistant-path".into(), root.clone())],
        )],
        &source.id,
    );
    assert!(scopes[0].enabled);
    assert!(scopes[0].full);

    skills::disable_skill_project(&f.roots, &[root.clone()], &source.id).unwrap();
    assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    assert!(!skills::list_catalog(&f.roots, &[root.clone()])
        .resources
        .iter()
        .any(|row| row.id == source.id));
    assert!(file.is_file());
    skills::uninstall_registered_skill(
        &f.roots,
        &[root],
        source,
        &skills::source_revision(source),
        true,
    )
    .unwrap();
    let mut registry = skills::load_skill_registry(f.db()).unwrap();
    skills::remove_source(&mut registry, &source.id);
    skills::save_skill_registry(f.db(), &registry).unwrap();
    assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    assert!(!file.exists());
    assert!(skills::load_skill_registry(f.db())
        .unwrap()
        .sources
        .is_empty());
}

#[test]
fn assistant_skill_relocation_preserves_support_files_and_repoints_existing_project_links() {
    let f = Fixture::new();
    let root = assistant::directory(&f.home);
    let legacy = root.join("skills/noending-assistant");
    std::fs::create_dir_all(legacy.join("references")).unwrap();
    std::fs::write(
        legacy.join("SKILL.md"),
        include_str!("../skills/noending-assistant/SKILL.md"),
    )
    .unwrap();
    std::fs::write(
        legacy.join("references/custom.md"),
        "additional user reference",
    )
    .unwrap();
    std::fs::write(root.join("USER.md"), "custom instructions").unwrap();
    let unrelated = root.join("skills/another/SKILL.md");
    std::fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
    std::fs::write(&unrelated, "another skill").unwrap();
    let old = skills::register_source_dir(&legacy).unwrap();
    let mut registry = skills::SkillRegistry::default();
    let old_id =
        skills::register_source(&mut registry, &old.source_path, &old.name, &old.description)
            .id
            .clone();
    skills::save_skill_registry(f.db(), &registry).unwrap();
    let other_project = f.home.root.join("workspace/existing");
    std::fs::create_dir_all(&other_project).unwrap();
    let projection = f.layer.projection();
    project::ensure_workspace_path(
        f.db(),
        &projection
            .observer()
            .observe(other_project.to_str().unwrap()),
        projection.policy(),
    )
    .unwrap();
    let location = skills::list(&f.roots, Some(&other_project))
        .locations
        .into_iter()
        .find(|loc| {
            loc.scope == "project" && loc.agent == Some(noending::domain::Agent::ClaudeCode)
        })
        .unwrap();
    skills::link_skill(&f.roots, Some(&other_project), &legacy, &location.id).unwrap();
    assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).unwrap();
    let target = f.home.root.join("skills/noending-assistant");
    assert_eq!(
        std::fs::read_to_string(target.join("references/custom.md")).unwrap(),
        "additional user reference"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("USER.md")).unwrap(),
        "custom instructions"
    );
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "another skill");
    assert!(!legacy.exists());
    let new_file = std::fs::canonicalize(target.join("SKILL.md")).unwrap();
    assert_eq!(
        std::fs::canonicalize(
            std::path::Path::new(&location.path).join("noending-assistant/SKILL.md")
        )
        .unwrap(),
        new_file
    );
    let registry = skills::load_skill_registry(f.db()).unwrap();
    assert!(skills::find_source(&registry, &old_id).is_none());
    assert_eq!(registry.sources.len(), 1);
    assert_eq!(std::path::Path::new(&registry.sources[0].path), new_file);
}

#[test]
fn assistant_skill_relocation_does_not_overwrite_a_conflicting_library_source() {
    let f = Fixture::new();
    let legacy = assistant::directory(&f.home).join("skills/noending-assistant");
    let target = f.home.root.join("skills/noending-assistant");
    for dir in [&legacy, &target] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            include_str!("../skills/noending-assistant/SKILL.md"),
        )
        .unwrap();
    }
    std::fs::write(target.join("custom.md"), "keep target").unwrap();
    std::fs::write(legacy.join("custom.md"), "keep original").unwrap();
    assert!(assistant::ensure_workspace(f.db(), &f.home, &f.layer, &f.roots).is_err());
    assert_eq!(
        std::fs::read_to_string(target.join("custom.md")).unwrap(),
        "keep target"
    );
    assert_eq!(
        std::fs::read_to_string(legacy.join("custom.md")).unwrap(),
        "keep original"
    );
}
