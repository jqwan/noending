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
