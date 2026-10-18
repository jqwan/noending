//! Native customization contracts, isolated from the user's real Agent homes.
use noending_lib::{
    domain::Agent,
    extensions::{self, Roots},
    storage::Db,
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    root: PathBuf,
    roots: Roots,
    cwd: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("noending-extensions-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let cwd = root.join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        Self {
            roots: Roots {
                codex: home.join(".codex"),
                claude: home.join(".claude"),
                pi: home.join(".pi/agent"),
                home,
            },
            root,
            cwd,
        }
    }
    fn write(&self, path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn source(&self, directory: &str, name: &str) -> PathBuf {
        let path = self.root.join(directory);
        self.write(&path.join("SKILL.md"), &format!("---\nname: {name}\ndescription: |\n  Review source changes.\n  Keep useful findings.\n---\n# Instructions\nRead scripts/check.sh.\n"));
        self.write(&path.join("scripts/check.sh"), "exit 91\n");
        path
    }
    fn mcp(&self, agent: Agent, scope: &str) -> extensions::Location {
        extensions::list(&self.roots, Some(&self.cwd))
            .locations
            .into_iter()
            .find(|l| l.kind == "mcp" && l.agent == Some(agent) && l.scope == scope)
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn project_scopes(
    f: &Fixture,
    paths: &[(&str, PathBuf)],
    id: &str,
) -> Vec<extensions::ProjectSkillScope> {
    let inventory = extensions::merge_inventories(
        paths
            .iter()
            .map(|(_, path)| extensions::list(&f.roots, Some(path)))
            .collect(),
    );
    extensions::project_scopes_from_inventory(
        &f.roots,
        &inventory,
        &[(
            "p".into(),
            "Project".into(),
            paths
                .iter()
                .map(|(workspace, path)| ((*workspace).into(), path.clone()))
                .collect(),
        )],
        id,
    )
}

#[test]
fn skill_install_copies_support_files_and_never_overwrites_or_edits_the_source() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    extensions::install_skill(&f.roots, None, &source, &loc.id).unwrap();
    let installed = Path::new(&loc.path).join("review");
    assert_eq!(
        fs::read_to_string(installed.join("scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
    assert!(source.join("SKILL.md").exists());
    f.write(&installed.join("local.txt"), "keep");
    assert!(extensions::install_skill(&f.roots, None, &source, &loc.id).is_err());
    assert_eq!(
        fs::read_to_string(installed.join("local.txt")).unwrap(),
        "keep"
    );
    let inventory = extensions::list(&f.roots, None);
    let row = inventory
        .resources
        .iter()
        .find(|r| r.kind == "skill")
        .unwrap();
    let detail = extensions::detail(&f.roots, None, &row.id).unwrap();
    assert!(detail.content.contains("# Instructions"));
    assert_eq!(
        row.description.trim(),
        "Review source changes.\nKeep useful findings."
    );
    assert!(!row.entry_links[0]);
    assert!(
        extensions::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision)
            .is_err()
    );
    assert!(installed.join("SKILL.md").exists());
    assert!(extensions::uninstall_skill(&f.roots, &[], &row.id, "old", true).is_err());
    assert!(extensions::uninstall_skill(&f.roots, &[], &row.id, &row.revision, false).is_err());
    extensions::uninstall_skill(&f.roots, &[], &row.id, &row.revision, true).unwrap();
    assert!(!installed.exists());
    assert!(source.join("SKILL.md").exists());
}

#[test]
fn same_name_skills_remain_distinct_and_listing_reports_parse_failures_without_writing() {
    let f = Fixture::new();
    let shared = f.roots.home.join(".agents/skills/review");
    let claude = f.roots.claude.join("skills/review");
    let valid = "---\nname: review\ndescription: Review code\n---\nKeep this content\n";
    f.write(&shared.join("SKILL.md"), valid);
    f.write(&claude.join("SKILL.md"), valid);
    f.write(&f.roots.pi.join("skills/broken/SKILL.md"), "not a skill");
    f.write(&f.roots.pi.join("mcp.json"), "{broken");
    let inventory = extensions::list(&f.roots, None);
    assert_eq!(
        inventory
            .resources
            .iter()
            .filter(|r| r.name == "review")
            .count(),
        2
    );
    assert!(inventory
        .resources
        .iter()
        .find(|r| r.name == "broken")
        .unwrap()
        .error
        .is_some());
    assert_eq!(inventory.warnings.len(), 1);
    assert_eq!(fs::read_to_string(shared.join("SKILL.md")).unwrap(), valid);
    assert_eq!(
        fs::read_to_string(f.roots.pi.join("mcp.json")).unwrap(),
        "{broken"
    );
}

#[cfg(unix)]
#[test]
fn physical_skill_is_deduplicated_and_unlink_preserves_the_external_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let shared = f.roots.home.join(".agents/skills");
    let claude = f.roots.claude.join("skills");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(&claude).unwrap();
    let locations = extensions::list(&f.roots, None).locations;
    let shared_loc = locations
        .iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    let claude_loc = locations
        .iter()
        .find(|l| l.kind == "skill" && l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &shared_loc.id).unwrap();
    extensions::link_skill(&f.roots, None, &source, &claude_loc.id).unwrap();
    let inventory = extensions::list(&f.roots, None);
    assert_eq!(inventory.resources.len(), 1);
    let row = &inventory.resources[0];
    assert_eq!(row.entry_paths.len(), 2);
    assert_eq!(row.entry_links, vec![true, true]);
    let shared_entry = shared
        .join("review/SKILL.md")
        .to_string_lossy()
        .into_owned();
    extensions::disable_skill(&f.roots, None, &row.id, &shared_entry, &row.revision).unwrap();
    assert!(!shared.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    assert!(claude.join("review/SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn uninstall_skill_removes_all_links_and_keeps_the_external_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let shared = f.roots.home.join(".agents/skills");
    let claude = f.roots.claude.join("skills");
    fs::create_dir_all(&shared).unwrap();
    fs::create_dir_all(&claude).unwrap();
    let locations = extensions::list(&f.roots, None).locations;
    let shared_loc = locations
        .iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    let claude_loc = locations
        .iter()
        .find(|l| l.kind == "skill" && l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &shared_loc.id).unwrap();
    extensions::link_skill(&f.roots, None, &source, &claude_loc.id).unwrap();
    let inventory = extensions::list(&f.roots, None);
    let row = &inventory.resources[0];
    assert_eq!(row.entry_links, vec![true, true]);
    extensions::uninstall_skill(&f.roots, &[], &row.id, &row.revision, false).unwrap();
    assert!(!shared.join("review").exists());
    assert!(!claude.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    assert!(extensions::list(&f.roots, None).resources.is_empty());
}

#[cfg(unix)]
#[test]
fn uninstall_skill_can_delete_an_external_source_after_removing_links() {
    let f = Fixture::new();
    let source = f.source("external-delete", "review");
    let shared = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&shared).unwrap();
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let row = extensions::list(&f.roots, None).resources[0].clone();
    assert_eq!(row.entry_links, vec![true]);
    extensions::uninstall_skill(&f.roots, &[], &row.id, &row.revision, true).unwrap();
    assert!(!shared.join("review").exists());
    assert!(!source.exists());
    assert!(extensions::list(&f.roots, None).resources.is_empty());
}

#[cfg(unix)]
#[test]
fn disabling_a_registered_link_keeps_the_source_listed_as_inactive() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed/review", "review");
    let shared = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&shared).unwrap();
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    let installed = extensions::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let mut registry = extensions::SkillRegistry::default();
    extensions::register_source(
        &mut registry,
        &installed.source_path,
        &installed.name,
        &installed.description,
    );
    let row = extensions::list(&f.roots, None).resources[0].clone();
    extensions::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
    assert!(!shared.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    let source_id = extensions::source_id(&installed.source_path);
    let mut inventory = extensions::list(&f.roots, None);
    extensions::merge_skill_sources(&registry, &mut inventory, true, &managed);
    let inactive = inventory
        .resources
        .iter()
        .find(|r| r.id == source_id)
        .unwrap();
    assert!(inactive.registered);
    assert!(inactive.entry_paths.is_empty());
    assert_eq!(inactive.name, "review");
    assert_eq!(inactive.path, installed.source_path);
    let detail =
        extensions::detail_registered(&f.roots, None, &managed, &registry, &registry.sources[0])
            .unwrap();
    assert!(detail.content.contains("# Instructions"));
    let preview =
        extensions::registered_uninstall_preview(&f.roots, &[], &registry.sources[0]).unwrap();
    assert!(!preview.delete_source_required);
    assert_eq!(preview.entry_count, 0);
    assert!(extensions::uninstall_registered_skill(
        &f.roots,
        &[],
        &registry.sources[0],
        "old",
        false
    )
    .is_err());
    extensions::uninstall_registered_skill(
        &f.roots,
        &[],
        &registry.sources[0],
        &inactive.revision,
        false,
    )
    .unwrap();
    assert!(source.join("SKILL.md").exists());
}

#[test]
fn uninstalling_a_registered_physical_source_requires_deleting_it() {
    let f = Fixture::new();
    let dir = f.roots.home.join(".agents/skills/review");
    f.write(
        &dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let row = extensions::list(&f.roots, None).resources[0].clone();
    let mut registry = extensions::SkillRegistry::default();
    extensions::register_source(&mut registry, &row.path, &row.name, &row.description);
    let source = &registry.sources[0];
    let preview = extensions::registered_uninstall_preview(&f.roots, &[], source).unwrap();
    assert!(preview.delete_source_required);
    assert!(
        extensions::uninstall_registered_skill(&f.roots, &[], source, &row.revision, false)
            .is_err()
    );
    assert!(dir.join("SKILL.md").exists());
    extensions::uninstall_registered_skill(&f.roots, &[], source, &row.revision, true).unwrap();
    assert!(!dir.exists());
    assert!(!extensions::list(&f.roots, None)
        .resources
        .iter()
        .any(|r| r.kind == "skill"));
}

#[cfg(unix)]
#[test]
fn enabling_a_source_creates_a_link_without_touching_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_file = source_file.to_string_lossy().into_owned();
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    let id = extensions::source_id(&source_file);
    extensions::enable_skill(&f.roots, None, &id, Some(&source_file), &loc.id).unwrap();
    let entry = f.roots.claude.join("skills/review");
    assert!(entry.is_symlink());
    assert_eq!(
        fs::read_link(&entry).unwrap(),
        fs::canonicalize(&source).unwrap()
    );
    assert!(source.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn enable_all_uses_shared_for_codex_and_pi_and_skips_their_own_dirs() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(source_file.to_string_lossy().as_ref());
    extensions::enable_skill_all(
        &f.roots,
        &[],
        &id,
        Some(source_file.to_string_lossy().as_ref()),
    )
    .unwrap();
    assert!(f.roots.home.join(".agents/skills/review").is_symlink());
    assert!(f.roots.claude.join("skills/review").is_symlink());
    assert!(f
        .roots
        .home
        .join(".gemini/antigravity-cli/skills/review")
        .is_symlink());
    assert!(!f.roots.codex.join("skills/review").exists());
    assert!(!f.roots.pi.join("skills/review").exists());
    assert!(!f.roots.home.join(".gemini/config/skills/review").exists());
    let inventory = extensions::list(&f.roots, None);
    assert_eq!(inventory.resources.len(), 1);
    let row = &inventory.resources[0];
    assert_eq!(row.entry_paths.len(), 3);
    assert!(row.entry_links.iter().all(|link| *link));
    assert!(source.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn enable_all_does_not_add_a_shared_entry_for_an_agent_that_is_already_covered() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(source_file.to_string_lossy().as_ref());
    let pi = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent == Some(Agent::Pi))
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &pi.id).unwrap();
    extensions::enable_skill_all(
        &f.roots,
        &[],
        &id,
        Some(source_file.to_string_lossy().as_ref()),
    )
    .unwrap();
    assert!(f.roots.pi.join("skills/review").is_symlink());
    assert!(f.roots.codex.join("skills/review").is_symlink());
    assert!(!f.roots.home.join(".agents/skills/review").exists());
    assert!(f.roots.claude.join("skills/review").is_symlink());
    assert!(f
        .roots
        .home
        .join(".gemini/antigravity-cli/skills/review")
        .is_symlink());
    let row = &extensions::list(&f.roots, None).resources[0];
    assert_eq!(row.entry_paths.len(), 4);
}

#[cfg(unix)]
#[test]
fn disable_all_removes_user_links_and_keeps_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(source_file.to_string_lossy().as_ref());
    extensions::enable_skill_all(
        &f.roots,
        &[],
        &id,
        Some(source_file.to_string_lossy().as_ref()),
    )
    .unwrap();
    extensions::disable_skill_all(&f.roots, &id).unwrap();
    assert!(!f.roots.home.join(".agents/skills/review").exists());
    assert!(!f.roots.claude.join("skills/review").exists());
    assert!(!f
        .roots
        .home
        .join(".gemini/antigravity-cli/skills/review")
        .exists());
    assert!(source.join("SKILL.md").exists());
    assert!(extensions::list(&f.roots, None).resources.is_empty());
}

#[cfg(unix)]
#[test]
fn a_physical_user_entry_is_skipped_but_other_links_still_toggle() {
    let f = Fixture::new();
    let dir = f.roots.home.join(".agents/skills/review");
    f.write(
        &dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let file = fs::canonicalize(dir.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    extensions::enable_skill_all(&f.roots, &[], &id, Some(file.to_string_lossy().as_ref()))
        .unwrap();
    let claude = f.roots.claude.join("skills/review");
    let antigravity = f.roots.home.join(".gemini/antigravity-cli/skills/review");
    assert!(dir.join("SKILL.md").exists());
    assert!(claude.exists());
    assert!(antigravity.exists());
    let disabled = extensions::disable_skill_all(&f.roots, &id).unwrap();
    assert!(disabled.changed);
    assert!(disabled.message.contains("保持启用"));
    assert!(!claude.exists());
    assert!(!antigravity.exists());
    assert!(dir.join("SKILL.md").exists());
    assert!(
        !extensions::disable_skill_all(&f.roots, &id)
            .unwrap()
            .changed
    );
    assert!(dir.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn user_level_enable_removes_same_source_project_links() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_id = file.to_string_lossy().into_owned();
    let id = extensions::source_id(&source_id);
    extensions::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
    assert!(f.cwd.join(".claude/skills/review").is_symlink());
    let outcome =
        extensions::enable_skill_all(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
    assert!(outcome.message.contains("已清理"));
    assert!(!f.cwd.join(".agents/skills/review").exists());
    assert!(!f.cwd.join(".claude/skills/review").exists());
    assert!(f.roots.claude.join("skills/review").is_symlink());
    assert!(source.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn user_level_enable_keeps_a_physical_project_entry() {
    let f = Fixture::new();
    let physical = f.cwd.join(".claude/skills/review");
    f.write(
        &physical.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let file = fs::canonicalize(physical.join("SKILL.md")).unwrap();
    let source_id = file.to_string_lossy().into_owned();
    let id = extensions::source_id(&source_id);
    let outcome =
        extensions::enable_skill_all(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
    assert!(!outcome.message.contains("已清理"));
    assert!(physical.join("SKILL.md").exists());
    assert!(f.roots.claude.join("skills/review").is_symlink());
}

#[cfg(unix)]
#[test]
fn project_enable_is_rejected_while_user_level_is_active() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_id = file.to_string_lossy().into_owned();
    let id = extensions::source_id(&source_id);
    extensions::enable_skill_all(&f.roots, &[], &id, Some(&source_id)).unwrap();
    let error = extensions::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source_id))
        .err()
        .expect("project enable must be rejected");
    assert!(error.to_string().contains("用户级启用"));
    assert!(!f.cwd.join(".claude/skills/review").exists());
}

#[test]
fn skill_registry_round_trips_through_settings() {
    let root =
        std::env::temp_dir().join(format!("noending-skill-registry-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = Db::open(&root.join("db.sqlite")).unwrap();
    let mut registry = extensions::SkillRegistry::default();
    extensions::register_source(
        &mut registry,
        "/tmp/source/SKILL.md",
        "review",
        "Review changes",
    );
    extensions::save_skill_registry(&db, &registry).unwrap();
    assert_eq!(extensions::load_skill_registry(&db).unwrap(), registry);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mcp_json_write_preserves_unrelated_settings_and_stale_or_invalid_results_do_not_write() {
    let f = Fixture::new();
    let loc = f.mcp(Agent::Pi, "user");
    let original =
        "{\"custom\":{\"keep\":17},\"mcpServers\":{\"other\":{\"command\":\"other-server\"}}}";
    f.write(Path::new(&loc.path), original);
    let revision = extensions::location_revision(&f.roots, None, &loc.id).unwrap();
    extensions::save_mcp(
        &f.roots,
        None,
        &loc.id,
        &revision,
        "docs",
        json!({"url":"https://docs.example/mcp","headers":{"Authorization":"${DOCS_TOKEN}"}}),
        true,
    )
    .unwrap();
    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&loc.path).unwrap()).unwrap();
    assert_eq!(after["custom"]["keep"], 17);
    assert_eq!(after["mcpServers"]["other"]["command"], "other-server");
    assert_eq!(
        after["mcpServers"]["docs"]["headers"]["Authorization"],
        "${DOCS_TOKEN}"
    );
    assert_eq!(
        fs::read_to_string(format!("{}.noending.bak", loc.path)).unwrap(),
        original
    );
    let written = fs::read(&loc.path).unwrap();
    assert!(extensions::save_mcp(
        &f.roots,
        None,
        &loc.id,
        &revision,
        "new",
        json!({"command":"should-not-run"}),
        true
    )
    .is_err());
    let current = extensions::location_revision(&f.roots, None, &loc.id).unwrap();
    assert!(extensions::save_mcp(
        &f.roots,
        None,
        &loc.id,
        &current,
        "bad",
        json!({"type":"sse","url":"https://example/sse"}),
        true
    )
    .is_err());
    assert_eq!(fs::read(&loc.path).unwrap(), written);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&loc.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(format!("{}.noending.bak", loc.path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn codex_toml_round_trip_preserves_comments_and_other_servers_and_removes_only_the_target() {
    let f = Fixture::new();
    let loc = f.mcp(Agent::Codex, "project");
    f.write(Path::new(&loc.path), "# personal comment\nmodel = \"keep-model\"\n[mcp_servers.keep]\ncommand = \"keep-server\"\n");
    let revision = extensions::location_revision(&f.roots, Some(&f.cwd), &loc.id).unwrap();
    extensions::save_mcp(
        &f.roots,
        Some(&f.cwd),
        &loc.id,
        &revision,
        "team.docs",
        json!({"url":"https://example/mcp","http_headers":{"X-Team":"one"},"enabled":false}),
        true,
    )
    .unwrap();
    let raw = fs::read_to_string(&loc.path).unwrap();
    assert!(raw.contains("# personal comment"));
    let value: serde_json::Value = toml_edit::de::from_str(&raw).unwrap();
    assert_eq!(value["model"], "keep-model");
    assert_eq!(value["mcp_servers"]["keep"]["command"], "keep-server");
    assert_eq!(value["mcp_servers"]["team.docs"]["enabled"], false);
    let row = extensions::list(&f.roots, Some(&f.cwd))
        .resources
        .into_iter()
        .find(|r| r.kind == "mcp" && r.name == "team.docs")
        .unwrap();
    extensions::remove_mcp(&f.roots, Some(&f.cwd), &row.id, &row.revision).unwrap();
    let value: serde_json::Value =
        toml_edit::de::from_str(&fs::read_to_string(&loc.path).unwrap()).unwrap();
    assert!(value["mcp_servers"].get("team.docs").is_none());
    assert!(value["mcp_servers"].get("keep").is_some());
}

#[test]
fn project_mcp_is_local_to_its_directory() {
    let f = Fixture::new();
    let other = f.root.join("other");
    fs::create_dir_all(&other).unwrap();
    let user = f.mcp(Agent::Pi, "user");
    extensions::save_mcp(
        &f.roots,
        None,
        &user.id,
        &extensions::location_revision(&f.roots, None, &user.id).unwrap(),
        "tools",
        json!({"command":"tools"}),
        true,
    )
    .unwrap();
    let project = f.mcp(Agent::Pi, "project");
    extensions::save_mcp(
        &f.roots,
        Some(&f.cwd),
        &project.id,
        &extensions::location_revision(&f.roots, Some(&f.cwd), &project.id).unwrap(),
        "tools",
        json!({"enabled":false}),
        true,
    )
    .unwrap();
    assert!(!other.join(".pi/mcp.json").exists());
    let list = extensions::list(&f.roots, Some(&f.cwd));
    assert_eq!(
        list.resources
            .iter()
            .filter(|r| r.kind == "mcp" && r.name == "tools")
            .count(),
        2
    );
    assert_eq!(
        extensions::list(&f.roots, Some(&other))
            .resources
            .iter()
            .filter(|r| r.kind == "mcp" && r.name == "tools")
            .count(),
        1
    );
    assert!(extensions::save_mcp(
        &f.roots,
        Some(&other),
        &project.id,
        &project.id,
        "tools",
        json!({"command":"no"}),
        true
    )
    .is_err());
}

#[test]
fn project_level_enable_covers_every_workspace_and_disable_keeps_the_source() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    let cwds = vec![f.cwd.clone(), second.clone()];
    let skill = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(skill.to_str().unwrap());
    extensions::enable_skill_project(&f.roots, &cwds, &id, Some(skill.to_str().unwrap())).unwrap();
    for cwd in &cwds {
        assert!(cwd.join(".agents/skills/review/SKILL.md").is_file());
        assert!(cwd.join(".claude/skills/review/SKILL.md").is_file());
        assert!(!cwd.join(".pi/skills/review").exists());
    }
    let scopes = project_scopes(&f, &[("a", cwds[0].clone()), ("b", cwds[1].clone())], &id);
    let scope = &scopes[0];
    assert!(scope.enabled);
    assert!(scope.full);
    assert_eq!(
        scope
            .workspaces
            .iter()
            .map(|workspace| workspace.entries.len())
            .collect::<Vec<_>>(),
        vec![2, 2]
    );
    assert!(scope.workspaces[0]
        .entries
        .iter()
        .any(|entry| entry.agents.contains(&Agent::Antigravity)));

    extensions::disable_skill_project(&f.roots, &cwds, &id).unwrap();
    assert!(source.join("SKILL.md").exists());
    for cwd in &cwds {
        assert!(fs::symlink_metadata(cwd.join(".agents/skills/review")).is_err());
        assert!(fs::symlink_metadata(cwd.join(".claude/skills/review")).is_err());
    }
    assert!(!project_scopes(&f, &[("a", cwds[0].clone())], &id)[0].enabled);
}

#[test]
fn a_new_project_directory_inherits_enabled_skills() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    let skill = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(skill.to_str().unwrap());
    extensions::enable_skill_project(
        &f.roots,
        std::slice::from_ref(&f.cwd),
        &id,
        Some(skill.to_str().unwrap()),
    )
    .unwrap();
    assert!(fs::symlink_metadata(second.join(".agents/skills/review")).is_err());

    extensions::inherit_project_skills(
        &f.roots,
        std::slice::from_ref(&f.cwd),
        std::slice::from_ref(&second),
    )
    .unwrap();
    assert!(second.join(".agents/skills/review/SKILL.md").is_file());
    assert!(second.join(".claude/skills/review/SKILL.md").is_file());
}

#[test]
fn project_inventory_merges_entries_from_every_working_directory() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    let cwds = vec![f.cwd.clone(), second.clone()];
    let skill = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(skill.to_str().unwrap());
    extensions::enable_skill_project(&f.roots, &cwds, &id, Some(skill.to_str().unwrap())).unwrap();

    let inventory = extensions::merge_inventories(vec![
        extensions::list(&f.roots, Some(&cwds[0])),
        extensions::list(&f.roots, Some(&cwds[1])),
    ]);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.id == id && row.kind == "skill")
        .unwrap();
    assert_eq!(row.entry_paths.len(), 4);
    assert_eq!(row.locations.len(), 4);
    assert!(row.entry_links.iter().all(|link| *link));
    let detail = extensions::detail_project(&f.roots, &cwds, &id).unwrap();
    assert!(detail.content.contains("# Instructions"));
}

#[cfg(unix)]
#[test]
fn a_physical_project_entry_skips_its_directory_but_still_links_elsewhere() {
    let f = Fixture::new();
    let physical = f.cwd.join(".claude/skills/review");
    f.write(
        &physical.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let file = fs::canonicalize(physical.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let source = file.to_string_lossy().into_owned();
    extensions::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source)).unwrap();
    let shared = f.cwd.join(".agents/skills/review");
    assert!(shared.is_symlink());
    assert!(physical.join("SKILL.md").exists());
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    assert_eq!(
        extensions::find_skill_source(&f.roots, &[f.cwd.clone(), second.clone()], &id),
        Some(file.clone())
    );
    extensions::enable_skill_project(&f.roots, &[second.clone()], &id, Some(&source)).unwrap();
    assert!(second.join(".agents/skills/review").is_symlink());
    assert!(second.join(".claude/skills/review").is_symlink());
    let disabled = extensions::disable_skill_project(&f.roots, &[f.cwd.clone()], &id).unwrap();
    assert!(disabled.changed);
    assert!(!shared.exists());
    assert!(physical.join("SKILL.md").exists());
    assert!(
        !extensions::disable_skill_project(&f.roots, &[f.cwd.clone()], &id)
            .unwrap()
            .changed
    );
    assert!(physical.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn migration_moves_a_physical_user_skill_into_managed_storage() {
    let f = Fixture::new();
    let physical = f.roots.home.join(".claude/skills/review");
    f.write(
        &physical.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    f.write(&physical.join("scripts/check.sh"), "exit 91\n");
    let file = fs::canonicalize(physical.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let shared = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, None, &physical, &shared.id).unwrap();
    let managed = f.root.join("managed");
    let row = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        extensions::skill_source_kind(&row, &managed),
        extensions::SkillSourceKind::Agent
    );

    let preview = extensions::preview_skill_migration(&f.roots, &[], &managed, &id, None).unwrap();
    assert_eq!(preview.entry_count, 2);
    assert_eq!(preview.physical_entries, 1);
    assert_eq!(
        preview.target_path,
        managed.join("review").to_string_lossy()
    );
    assert!(
        extensions::migrate_skill(&f.roots, &[], &managed, &id, None, false).is_err(),
        "a physical source must not be kept after registration"
    );

    let outcome = extensions::migrate_skill(&f.roots, &[], &managed, &id, None, true).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert!(fs::symlink_metadata(&physical)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&physical).unwrap(), target);
    assert_eq!(
        fs::read_link(f.roots.home.join(".agents/skills/review")).unwrap(),
        target
    );
    assert_eq!(
        fs::read_to_string(managed.join("review/scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
    assert_eq!(
        outcome.resource_id,
        extensions::source_id(
            fs::canonicalize(managed.join("review/SKILL.md"))
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
    let inventory = extensions::list(&f.roots, None);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "skill")
        .unwrap();
    assert_eq!(row.id, outcome.resource_id);
    assert!(row.entry_links.iter().all(|link| *link));
    assert!(inventory.resources.iter().all(|row| row.id != id));
}

#[cfg(unix)]
#[test]
fn migration_moves_a_physical_project_skill_and_keeps_the_project_enabled() {
    let f = Fixture::new();
    let physical = f.cwd.join(".claude/skills/review");
    f.write(
        &physical.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let file = fs::canonicalize(physical.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let shared = extensions::list(&f.roots, Some(&f.cwd))
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "project" && loc.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, Some(&f.cwd), &physical, &shared.id).unwrap();
    let managed = f.root.join("managed");

    extensions::migrate_skill(&f.roots, &[f.cwd.clone()], &managed, &id, None, true).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert_eq!(fs::read_link(&physical).unwrap(), target);
    assert_eq!(
        fs::read_link(f.cwd.join(".agents/skills/review")).unwrap(),
        target
    );
    let migrated_id = extensions::source_id(
        fs::canonicalize(managed.join("review/SKILL.md"))
            .unwrap()
            .to_string_lossy()
            .as_ref(),
    );
    let scopes = project_scopes(&f, &[("a", f.cwd.clone())], &migrated_id);
    let scope = &scopes[0];
    assert!(scope.enabled);
    assert!(scope.full);
    assert_eq!(scope.workspaces[0].entries.len(), 2);
}

#[cfg(unix)]
#[test]
fn migration_moves_a_linked_external_source_and_repoints_its_entries() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_path = file.to_string_lossy().into_owned();
    let id = extensions::source_id(&source_path);
    let shared = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &shared.id).unwrap();
    let managed = f.root.join("managed");
    let row = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        extensions::skill_source_kind(&row, &managed),
        extensions::SkillSourceKind::External
    );

    let preview =
        extensions::preview_skill_migration(&f.roots, &[], &managed, &id, Some(&source_path))
            .unwrap();
    assert_eq!(preview.entry_count, 1);
    assert_eq!(preview.physical_entries, 0);

    extensions::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), true).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert_eq!(
        fs::read_link(f.roots.home.join(".agents/skills/review")).unwrap(),
        target
    );
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(managed.join("review/scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
    let inventory = extensions::list(&f.roots, None);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "skill")
        .unwrap();
    assert_eq!(
        extensions::skill_source_kind(row, &managed),
        extensions::SkillSourceKind::Library
    );
    assert!(inventory.resources.iter().all(|row| row.id != id));
}

#[cfg(unix)]
#[test]
fn registration_can_copy_an_external_source_and_keep_the_original() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_path = fs::canonicalize(source.join("SKILL.md"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let id = extensions::source_id(&source_path);
    let shared = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &shared.id).unwrap();
    let managed = f.root.join("managed");

    extensions::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), false).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert_eq!(
        fs::read_link(f.roots.home.join(".agents/skills/review")).unwrap(),
        target
    );
    assert!(source.exists());
    assert_eq!(
        fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
}

/// Legacy installers expose a Skill as a real directory whose files link into
/// the source. That wrapper can be replaced without touching the source.
#[cfg(unix)]
#[test]
fn migration_replaces_a_wrapper_entry_that_links_into_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let shared = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    let wrapper = Path::new(&shared.path).join("review");
    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();
    let managed = f.root.join("managed");

    let row = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(row.entry_links, vec![true]);

    let preview = extensions::preview_skill_migration(&f.roots, &[], &managed, &id, None).unwrap();
    assert_eq!(preview.entry_count, 1);
    assert_eq!(preview.physical_entries, 0);

    let outcome = extensions::migrate_skill(&f.roots, &[], &managed, &id, None, false).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert!(fs::symlink_metadata(&wrapper)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&wrapper).unwrap(), target);
    assert!(source.join("SKILL.md").is_file());
    assert_eq!(
        outcome.resource_id,
        extensions::source_id(
            fs::canonicalize(managed.join("review/SKILL.md"))
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
}

#[cfg(unix)]
#[test]
fn wrapper_entries_can_be_disabled_and_uninstalled_without_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let shared = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.kind == "skill" && loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    let wrapper = Path::new(&shared.path).join("review");
    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();

    let preview = extensions::uninstall_preview(&f.roots, &[], &id).unwrap();
    assert!(!preview.delete_source_required);

    let disabled = extensions::disable_skill_all(&f.roots, &id).unwrap();
    assert!(disabled.changed);
    assert!(!wrapper.exists());
    assert!(source.join("SKILL.md").is_file());

    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();
    let revision = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap()
        .revision;
    extensions::uninstall_skill(&f.roots, &[], &id, &revision, false).unwrap();
    assert!(!wrapper.exists());
    assert!(source.join("SKILL.md").is_file());
}

#[test]
fn registering_an_external_source_adopts_it_into_the_library() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let managed = f.root.join("managed");

    let outcome = extensions::adopt_source(&f.roots, &[], &managed, &source).unwrap();
    assert!(!source.exists());
    let adopted = fs::canonicalize(managed.join("review/SKILL.md")).unwrap();
    assert_eq!(outcome.source_path, adopted.to_string_lossy());
    assert_eq!(outcome.name, "review");
}

#[test]
fn registering_a_library_source_keeps_it_in_place() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let library = f.source("managed/kept", "kept");

    let outcome = extensions::adopt_source(&f.roots, &[], &managed, &library).unwrap();
    assert!(library.exists());
    assert_eq!(
        outcome.source_path,
        fs::canonicalize(library.join("SKILL.md"))
            .unwrap()
            .to_string_lossy()
    );
}

#[test]
fn registering_the_library_root_is_rejected() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let root = f.source("managed", "stray");

    for path in [root.clone(), root.join("SKILL.md")] {
        let error = extensions::adopt_source(&f.roots, &[], &managed, &path)
            .err()
            .expect("the library root is a container, not a Skill");
        assert!(error.to_string().contains("源库根目录"));
    }
    assert!(root.join("SKILL.md").exists());
}

#[test]
fn migration_moves_a_registered_source_without_entries() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_path = file.to_string_lossy().into_owned();
    let id = extensions::source_id(&source_path);
    let managed = f.root.join("managed");

    let preview =
        extensions::preview_skill_migration(&f.roots, &[], &managed, &id, Some(&source_path))
            .unwrap();
    assert_eq!(preview.entry_count, 0);
    assert_eq!(preview.physical_entries, 0);

    let outcome =
        extensions::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), true).unwrap();
    assert!(!source.exists());
    let migrated = fs::canonicalize(managed.join("review/SKILL.md")).unwrap();
    assert_eq!(outcome.source_path, migrated.to_string_lossy());
    assert_eq!(
        outcome.resource_id,
        extensions::source_id(&outcome.source_path)
    );
    assert_eq!(outcome.name, "review");
}

#[test]
fn library_sources_are_discovered_as_registered_sources_without_entries() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    f.write(
        &managed.join("review/SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    fs::create_dir_all(managed.join("notes")).unwrap();
    f.write(
        &managed.join(".backup/SKILL.md"),
        "---\nname: backup\ndescription: Backup\n---\n",
    );
    let mut registry = extensions::SkillRegistry::default();
    assert!(extensions::discover_library_sources(&managed, &mut registry).unwrap());
    assert_eq!(registry.sources.len(), 1);
    let (id, path) = {
        let source = &registry.sources[0];
        assert_eq!(source.name, "review");
        assert_eq!(source.description, "Review changes");
        (source.id.clone(), source.path.clone())
    };
    assert_eq!(
        path,
        fs::canonicalize(managed.join("review/SKILL.md"))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    );
    assert!(!extensions::discover_library_sources(&managed, &mut registry).unwrap());

    let mut inventory = extensions::list(&f.roots, None);
    extensions::merge_skill_sources(&registry, &mut inventory, true, &managed);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "skill")
        .unwrap();
    assert_eq!(row.id, id);
    assert!(row.registered);
    assert!(row.locations.is_empty());
    assert!(row.entry_paths.is_empty());

    f.write(
        &managed.join("review/SKILL.md"),
        "---\nname: review\ndescription: Updated\n---\n# Review\n",
    );
    assert!(extensions::discover_library_sources(&managed, &mut registry).unwrap());
    assert_eq!(registry.sources[0].description, "Updated");
}

#[cfg(unix)]
#[test]
fn a_library_source_is_registered_without_a_registry_row() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed/review", "review");
    fs::create_dir_all(f.roots.home.join(".agents/skills")).unwrap();
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    extensions::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let mut inventory = extensions::list(&f.roots, None);
    extensions::merge_skill_sources(
        &extensions::SkillRegistry::default(),
        &mut inventory,
        true,
        &managed,
    );
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "skill")
        .unwrap();
    assert!(row.registered);
}

#[test]
fn a_location_root_skill_file_is_ignored_while_child_skills_are_listed() {
    let f = Fixture::new();
    let shared = f.roots.home.join(".agents/skills");
    f.write(
        &shared.join("SKILL.md"),
        "---\nname: workspace\ndescription: Root skill\n---\n",
    );
    f.write(
        &shared.join("review/SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n",
    );
    let rows: Vec<_> = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .filter(|row| row.kind == "skill")
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "review");
}

#[test]
fn an_agent_skills_root_cannot_be_imported_as_a_source() {
    let f = Fixture::new();
    let shared = f.roots.home.join(".agents/skills");
    f.write(
        &shared.join("SKILL.md"),
        "---\nname: workspace\ndescription: Root skill\n---\n",
    );
    f.write(
        &shared.join("review/SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n",
    );
    let error = extensions::adopt_source(&f.roots, &[], &f.root.join("managed"), &shared)
        .err()
        .expect("Agent roots are containers, not Skill sources");
    assert!(error.to_string().contains("根目录"));
    assert!(shared.join("SKILL.md").exists());
    assert!(shared.join("review/SKILL.md").exists());
}

#[test]
fn a_registry_row_outside_the_library_is_listed_but_not_registered() {
    let f = Fixture::new();
    let source = f.source("external/review", "review");
    let install = extensions::register_source_dir(&source).unwrap();
    let mut registry = extensions::SkillRegistry::default();
    extensions::register_source(
        &mut registry,
        &install.source_path,
        &install.name,
        &install.description,
    );
    let mut inventory = extensions::list(&f.roots, None);
    extensions::merge_skill_sources(&registry, &mut inventory, true, &f.root.join("managed"));
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "skill")
        .unwrap();
    assert!(!row.registered);
    assert!(row.entry_paths.is_empty());
}

#[test]
fn migration_rejects_a_source_already_in_the_library() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());

    let error = extensions::preview_skill_migration(
        &f.roots,
        &[],
        &managed,
        &id,
        Some(file.to_string_lossy().as_ref()),
    )
    .err()
    .expect("library sources must not be migrated");
    assert!(error.to_string().contains("源库中"));
}

#[test]
fn migration_refuses_to_overwrite_an_existing_managed_skill() {
    let f = Fixture::new();
    let physical = f.roots.home.join(".agents/skills/review");
    f.write(
        &physical.join("SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\n# Review\n",
    );
    let file = fs::canonicalize(physical.join("SKILL.md")).unwrap();
    let id = extensions::source_id(file.to_string_lossy().as_ref());
    let managed = f.root.join("managed");
    f.write(&managed.join("review/SKILL.md"), "existing");

    let error = extensions::preview_skill_migration(&f.roots, &[], &managed, &id, None)
        .err()
        .expect("existing target must be rejected");
    assert!(error.to_string().contains("已存在同名"));
    assert!(extensions::migrate_skill(&f.roots, &[], &managed, &id, None, true).is_err());
    assert!(physical.join("SKILL.md").exists());
    assert_eq!(
        fs::read_to_string(managed.join("review/SKILL.md")).unwrap(),
        "existing"
    );
}

#[cfg(windows)]
#[test]
fn windows_junction_unlink_keeps_skill_source_and_install_rejects_nested_junctions() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let root = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&root).unwrap();
    let junction = root.join("review");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&source)
        .output()
        .expect("Windows cmd is required for junction coverage");
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let row = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|r| r.kind == "skill")
        .unwrap();
    extensions::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
    assert!(!junction.exists());
    assert!(source.join("SKILL.md").exists());
    let nested = source.join("linked-scripts");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&nested)
        .arg(source.join("scripts"))
        .output()
        .unwrap();
    assert!(status.status.success());
    let loc = extensions::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.kind == "skill" && l.agent.is_none())
        .unwrap();
    assert!(extensions::install_skill(&f.roots, None, &source, &loc.id).is_err());
    assert!(!root.join("review").exists());
    fs::remove_dir(nested).unwrap();
}

fn mcp_section(value: &serde_json::Value, agent: Agent) -> &serde_json::Value {
    value
        .get(if agent == Agent::Codex {
            "mcp_servers"
        } else {
            "mcpServers"
        })
        .unwrap()
}

fn read_mcp(f: &Fixture, agent: Agent, scope: &str) -> serde_json::Value {
    let location = f.mcp(agent, scope);
    let raw = fs::read_to_string(&location.path).unwrap();
    if location.path.ends_with(".toml") {
        toml_edit::de::from_str(&raw).unwrap()
    } else {
        serde_json::from_str(&raw).unwrap()
    }
}

#[test]
fn identical_mcp_configurations_collapse_and_definitions_remain_listed() {
    let f = Fixture::new();
    let user = f.mcp(Agent::Pi, "user");
    let project = f.mcp(Agent::Pi, "project");
    f.write(
        Path::new(&user.path),
        r#"{"mcpServers":{"docs":{"url":"https://docs.example/mcp","headers":{"X-Team":"one"}}}}"#,
    );
    f.write(
        Path::new(&project.path),
        r#"{"mcpServers":{"docs":{"serverUrl":"https://docs.example/mcp","type":"http","headers":{"X-Team":"one"}}}}"#,
    );
    let inventory = extensions::list(&f.roots, Some(&f.cwd));
    let rows: Vec<_> = inventory
        .resources
        .iter()
        .filter(|row| row.kind == "mcp")
        .collect();
    assert_eq!(rows.len(), 1, "identical configs are one MCP");
    assert_eq!(rows[0].locations.len(), 2);

    let id = extensions::mcp_identity(
        "docs",
        &json!({"serverUrl":"https://docs.example/mcp","headers":{"X-Team":"one"}}),
    );
    assert_eq!(rows[0].id, id);

    let mut registry = extensions::McpRegistry::default();
    extensions::register_mcp_definition(
        &mut registry,
        "docs",
        "Documentation MCP",
        json!({"url":"https://docs.example/mcp","headers":{"X-Team":"one"}}),
    )
    .unwrap();
    assert_eq!(registry.definitions[0].id, id);

    fs::remove_file(&user.path).unwrap();
    fs::remove_file(&project.path).unwrap();
    let mut inventory = extensions::list(&f.roots, Some(&f.cwd));
    extensions::merge_mcp_definitions(&registry, &mut inventory, true);
    let row = inventory
        .resources
        .iter()
        .find(|row| row.kind == "mcp" && row.id == id)
        .unwrap();
    assert!(row.registered);
    assert!(row.locations.is_empty());
}

#[test]
fn enabling_a_definition_writes_every_agent_and_disable_preserves_other_servers() {
    let f = Fixture::new();
    let mut registry = extensions::McpRegistry::default();
    let id = extensions::register_mcp_definition(
        &mut registry,
        "docs",
        "",
        json!({"url":"https://docs.example/mcp","headers":{"X-Team":"one"}}),
    )
    .unwrap()
    .id
    .clone();
    extensions::enable_mcp_user(&f.roots, &registry, &id, &[]).unwrap();
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        let value = read_mcp(&f, agent, "user");
        let server = &mcp_section(&value, agent)["docs"];
        assert!(server.is_object(), "{agent:?} did not receive docs");
        if agent == Agent::Antigravity {
            assert_eq!(server["serverUrl"], "https://docs.example/mcp");
        }
    }
    extensions::disable_mcp_user(&f.roots, &registry, &id).unwrap();
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        let value = read_mcp(&f, agent, "user");
        assert!(
            mcp_section(&value, agent).get("docs").is_none(),
            "{agent:?} kept docs"
        );
    }
}

#[test]
fn a_definition_enables_into_projects_and_uninstall_removes_every_matching_entry() {
    let f = Fixture::new();
    let mut registry = extensions::McpRegistry::default();
    let id = extensions::register_mcp_definition(
        &mut registry,
        "docs",
        "",
        json!({"url":"https://docs.example/mcp"}),
    )
    .unwrap()
    .id
    .clone();
    extensions::enable_mcp_projects(&f.roots, &registry, &id, std::slice::from_ref(&f.cwd))
        .unwrap();
    let scopes = project_scopes(&f, &[("w", f.cwd.clone())], &id);
    assert!(scopes[0].enabled);
    assert!(scopes[0].full);
    assert_eq!(scopes[0].workspaces[0].entries.len(), 4);
    assert!(scopes[0].workspaces[0]
        .entries
        .iter()
        .all(|entry| entry.agents.len() == 1));
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        let project = read_mcp(&f, agent, "project");
        assert!(mcp_section(&project, agent).get("docs").is_some());
    }
    let enabled =
        extensions::enable_mcp_user(&f.roots, &registry, &id, std::slice::from_ref(&f.cwd))
            .unwrap();
    assert!(enabled.message.contains("已清理"));
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        let project = read_mcp(&f, agent, "project");
        assert!(mcp_section(&project, agent).get("docs").is_none());
        let user = read_mcp(&f, agent, "user");
        assert!(mcp_section(&user, agent).get("docs").is_some());
    }

    extensions::uninstall_mcp_definition(
        &mut registry,
        &f.roots,
        &id,
        std::slice::from_ref(&f.cwd),
    )
    .unwrap();
    assert!(registry.definitions.is_empty());
    for scope in ["user", "project"] {
        for agent in [
            Agent::Codex,
            Agent::ClaudeCode,
            Agent::Pi,
            Agent::Antigravity,
        ] {
            let value = read_mcp(&f, agent, scope);
            assert!(mcp_section(&value, agent).get("docs").is_none());
        }
    }
}

#[test]
fn project_mcp_enable_is_rejected_while_user_level_is_active() {
    let f = Fixture::new();
    let mut registry = extensions::McpRegistry::default();
    let id = extensions::register_mcp_definition(
        &mut registry,
        "docs",
        "",
        json!({"url":"https://docs.example/mcp"}),
    )
    .unwrap()
    .id
    .clone();
    extensions::enable_mcp_user(&f.roots, &registry, &id, &[]).unwrap();
    let error =
        extensions::enable_mcp_projects(&f.roots, &registry, &id, std::slice::from_ref(&f.cwd))
            .unwrap_err();
    assert!(error.to_string().contains("用户级启用"));
    for agent in [
        Agent::Codex,
        Agent::ClaudeCode,
        Agent::Pi,
        Agent::Antigravity,
    ] {
        assert!(!Path::new(&f.mcp(agent, "project").path).exists());
    }
}

#[test]
fn a_name_conflict_does_not_overwrite_a_different_native_configuration() {
    let f = Fixture::new();
    let user = f.mcp(Agent::Pi, "user");
    f.write(
        Path::new(&user.path),
        r#"{"mcpServers":{"docs":{"url":"https://other.example/mcp"}}}"#,
    );
    let mut registry = extensions::McpRegistry::default();
    let id = extensions::register_mcp_definition(
        &mut registry,
        "docs",
        "",
        json!({"url":"https://docs.example/mcp"}),
    )
    .unwrap()
    .id
    .clone();
    let result = extensions::enable_mcp_user(&f.roots, &registry, &id, &[]).unwrap();
    assert!(result.message.contains("同名但不同配置"));
    let value = read_mcp(&f, Agent::Pi, "user");
    assert_eq!(
        mcp_section(&value, Agent::Pi)["docs"]["url"],
        "https://other.example/mcp"
    );
}

#[test]
fn registering_a_skill_source_writes_no_agent_entry() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let outcome = extensions::register_source_dir(&source).unwrap();
    assert_eq!(outcome.name, "review");
    assert!(outcome.source_path.ends_with("SKILL.md"));
    let inventory = extensions::list(&f.roots, None);
    assert!(!inventory
        .resources
        .iter()
        .any(|row| row.kind == "skill" && row.path == outcome.source_path));
}
