//! Native customization contracts, isolated from the user's real Agent homes.
use noending_lib::{
    domain::Agent,
    skills::{self, Roots},
    storage::Db,
};
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
        let root = std::env::temp_dir().join(format!("noending-skills-{}", uuid::Uuid::new_v4()));
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
) -> Vec<skills::ProjectSkillScope> {
    let inventory = skills::merge_inventories(
        paths
            .iter()
            .map(|(_, path)| skills::list(&f.roots, Some(path)))
            .collect(),
    );
    skills::project_scopes_from_inventory(
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
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent.is_none())
        .unwrap();
    skills::install_skill(&f.roots, None, &source, &loc.id).unwrap();
    let installed = Path::new(&loc.path).join("review");
    assert_eq!(
        fs::read_to_string(installed.join("scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
    assert!(source.join("SKILL.md").exists());
    f.write(&installed.join("local.txt"), "keep");
    assert!(skills::install_skill(&f.roots, None, &source, &loc.id).is_err());
    assert_eq!(
        fs::read_to_string(installed.join("local.txt")).unwrap(),
        "keep"
    );
    let inventory = skills::list(&f.roots, None);
    let row = inventory.resources.iter().next().unwrap();
    let detail = skills::detail(&f.roots, None, &row.id).unwrap();
    assert!(detail.content.contains("# Instructions"));
    assert_eq!(
        row.description.trim(),
        "Review source changes.\nKeep useful findings."
    );
    assert!(!row.entry_links[0]);
    assert!(
        skills::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).is_err()
    );
    assert!(installed.join("SKILL.md").exists());
    assert!(skills::uninstall_skill(&f.roots, &[], &row.id, "old", true).is_err());
    assert!(skills::uninstall_skill(&f.roots, &[], &row.id, &row.revision, false).is_err());
    skills::uninstall_skill(&f.roots, &[], &row.id, &row.revision, true).unwrap();
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
    let inventory = skills::list(&f.roots, None);
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
    assert!(inventory.warnings.is_empty());
    assert_eq!(fs::read_to_string(shared.join("SKILL.md")).unwrap(), valid);
}

#[test]
fn skill_discovery_and_uninstall_leave_native_agent_configs_untouched() {
    let f = Fixture::new();
    let configs = [
        f.roots.codex.join("config.toml"),
        f.roots.home.join(".claude.json"),
        f.roots.pi.join("mcp.json"),
        f.roots.home.join(".gemini/config/mcp_config.json"),
        f.cwd.join(".codex/config.toml"),
        f.cwd.join(".mcp.json"),
        f.cwd.join(".pi/mcp.json"),
        f.cwd.join(".agents/mcp_config.json"),
    ];
    // Invalid configuration would cause warnings if Skill discovery parsed it.
    for path in &configs {
        f.write(path, "invalid native config: { [");
    }
    let source = f.source("input", "review");
    let location = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|location| location.agent == Some(Agent::ClaudeCode))
        .unwrap();
    skills::install_skill(&f.roots, None, &source, &location.id).unwrap();
    let inventory = skills::list_catalog(&f.roots, &[f.cwd.clone()]);
    assert!(inventory.warnings.is_empty(), "{:?}", inventory.warnings);
    assert_eq!(inventory.resources.len(), 1);
    assert!(inventory
        .locations
        .iter()
        .all(|location| !configs.contains(&PathBuf::from(&location.path))));
    let row = &inventory.resources[0];
    let detail = skills::detail_in_catalog(&inventory, &row.id)
        .unwrap()
        .unwrap();
    assert!(detail.content.contains("# Instructions"));
    skills::uninstall_skill(&f.roots, &[f.cwd.clone()], &row.id, &row.revision, true).unwrap();
    assert!(source.join("SKILL.md").is_file());
    assert!(skills::list_catalog(&f.roots, &[f.cwd.clone()])
        .resources
        .is_empty());
    for path in &configs {
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "invalid native config: { ["
        );
        assert!(!PathBuf::from(format!("{}.noending.bak", path.display())).exists());
    }
}

#[test]
fn skill_scan_skips_plain_file_load_roots_without_following_or_modifying_them() {
    let f = Fixture::new();
    let user_skill = f.roots.home.join(".agents/skills/user-review/SKILL.md");
    let project_skill = f.cwd.join(".agents/skills/project-review/SKILL.md");
    f.write(
        &user_skill,
        "---\nname: user-review\ndescription: User skill\n---\nInstructions\n",
    );
    f.write(
        &project_skill,
        "---\nname: project-review\ndescription: Project skill\n---\nInstructions\n",
    );
    // Git with core.symlinks=false checks out this directory link as a file.
    let user_entry = f.roots.claude.join("skills");
    let project_entry = f.cwd.join(".claude/skills");
    for entry in [&user_entry, &project_entry] {
        f.write(entry, "../.agents/skills");
    }

    let inventory = skills::list_catalog(&f.roots, &[f.cwd.clone()]);
    assert!(inventory.warnings.is_empty(), "{:?}", inventory.warnings);
    let mut found: Vec<_> = inventory
        .resources
        .iter()
        .map(|row| row.path.clone())
        .collect();
    found.sort();
    let mut expected: Vec<_> = [&user_skill, &project_skill]
        .iter()
        .map(|path| {
            fs::canonicalize(path)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    expected.sort();
    assert_eq!(found, expected);
    for row in &inventory.resources {
        for id in &row.locations {
            let location = inventory
                .locations
                .iter()
                .find(|loc| &loc.id == id)
                .unwrap();
            assert_ne!(
                location.agent,
                Some(Agent::ClaudeCode),
                "plain files cannot enable Claude Skills"
            );
        }
    }
    for entry in [&user_entry, &project_entry] {
        assert!(entry.is_file());
        assert_eq!(fs::read_to_string(entry).unwrap(), "../.agents/skills");
    }
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
    let locations = skills::list(&f.roots, None).locations;
    let shared_loc = locations.iter().find(|l| l.agent.is_none()).unwrap();
    let claude_loc = locations
        .iter()
        .find(|l| l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &shared_loc.id).unwrap();
    skills::link_skill(&f.roots, None, &source, &claude_loc.id).unwrap();
    let inventory = skills::list(&f.roots, None);
    assert_eq!(inventory.resources.len(), 1);
    let row = &inventory.resources[0];
    assert_eq!(row.entry_paths.len(), 2);
    assert_eq!(row.entry_links, vec![true, true]);
    let shared_entry = shared
        .join("review/SKILL.md")
        .to_string_lossy()
        .into_owned();
    skills::disable_skill(&f.roots, None, &row.id, &shared_entry, &row.revision).unwrap();
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
    let locations = skills::list(&f.roots, None).locations;
    let shared_loc = locations.iter().find(|l| l.agent.is_none()).unwrap();
    let claude_loc = locations
        .iter()
        .find(|l| l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &shared_loc.id).unwrap();
    skills::link_skill(&f.roots, None, &source, &claude_loc.id).unwrap();
    let inventory = skills::list(&f.roots, None);
    let row = &inventory.resources[0];
    assert_eq!(row.entry_links, vec![true, true]);
    skills::uninstall_skill(&f.roots, &[], &row.id, &row.revision, false).unwrap();
    assert!(!shared.join("review").exists());
    assert!(!claude.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    assert!(skills::list(&f.roots, None).resources.is_empty());
}

#[cfg(unix)]
#[test]
fn uninstall_skill_can_delete_an_external_source_after_removing_links() {
    let f = Fixture::new();
    let source = f.source("external-delete", "review");
    let shared = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&shared).unwrap();
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let row = skills::list(&f.roots, None).resources[0].clone();
    assert_eq!(row.entry_links, vec![true]);
    skills::uninstall_skill(&f.roots, &[], &row.id, &row.revision, true).unwrap();
    assert!(!shared.join("review").exists());
    assert!(!source.exists());
    assert!(skills::list(&f.roots, None).resources.is_empty());
}

#[cfg(unix)]
#[test]
fn disabling_a_registered_link_keeps_the_source_listed_as_inactive() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed/review", "review");
    let shared = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&shared).unwrap();
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent.is_none())
        .unwrap();
    let installed = skills::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let mut registry = skills::SkillRegistry::default();
    skills::register_source(
        &mut registry,
        &installed.source_path,
        &installed.name,
        &installed.description,
    );
    let row = skills::list(&f.roots, None).resources[0].clone();
    skills::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
    assert!(!shared.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    let source_id = skills::source_id(&installed.source_path);
    let mut inventory = skills::list(&f.roots, None);
    skills::merge_skill_sources(&registry, &mut inventory, true, &managed);
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
        skills::detail_registered(&f.roots, None, &managed, &registry, &registry.sources[0])
            .unwrap();
    assert!(detail.content.contains("# Instructions"));
    let preview =
        skills::registered_uninstall_preview(&f.roots, &[], &registry.sources[0]).unwrap();
    assert!(!preview.delete_source_required);
    assert_eq!(preview.entry_count, 0);
    assert!(
        skills::uninstall_registered_skill(&f.roots, &[], &registry.sources[0], "old", false)
            .is_err()
    );
    skills::uninstall_registered_skill(
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
    let row = skills::list(&f.roots, None).resources[0].clone();
    let mut registry = skills::SkillRegistry::default();
    skills::register_source(&mut registry, &row.path, &row.name, &row.description);
    let source = &registry.sources[0];
    let preview = skills::registered_uninstall_preview(&f.roots, &[], source).unwrap();
    assert!(preview.delete_source_required);
    assert!(
        skills::uninstall_registered_skill(&f.roots, &[], source, &row.revision, false).is_err()
    );
    assert!(dir.join("SKILL.md").exists());
    skills::uninstall_registered_skill(&f.roots, &[], source, &row.revision, true).unwrap();
    assert!(!dir.exists());
    assert!(!skills::list(&f.roots, None)
        .resources
        .iter()
        .next()
        .is_some());
}

#[cfg(unix)]
#[test]
fn enabling_a_source_creates_a_link_without_touching_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_file = source_file.to_string_lossy().into_owned();
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent == Some(Agent::ClaudeCode))
        .unwrap();
    let id = skills::source_id(&source_file);
    skills::enable_skill(&f.roots, None, &id, Some(&source_file), &loc.id).unwrap();
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
    let id = skills::source_id(source_file.to_string_lossy().as_ref());
    skills::enable_skill_all(
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
    let inventory = skills::list(&f.roots, None);
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
    let id = skills::source_id(source_file.to_string_lossy().as_ref());
    let pi = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent == Some(Agent::Pi))
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &pi.id).unwrap();
    skills::enable_skill_all(
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
    let row = &skills::list(&f.roots, None).resources[0];
    assert_eq!(row.entry_paths.len(), 4);
}

#[cfg(unix)]
#[test]
fn disable_all_removes_user_links_and_keeps_the_source() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = skills::source_id(source_file.to_string_lossy().as_ref());
    skills::enable_skill_all(
        &f.roots,
        &[],
        &id,
        Some(source_file.to_string_lossy().as_ref()),
    )
    .unwrap();
    skills::disable_skill_all(&f.roots, &id).unwrap();
    assert!(!f.roots.home.join(".agents/skills/review").exists());
    assert!(!f.roots.claude.join("skills/review").exists());
    assert!(!f
        .roots
        .home
        .join(".gemini/antigravity-cli/skills/review")
        .exists());
    assert!(source.join("SKILL.md").exists());
    assert!(skills::list(&f.roots, None).resources.is_empty());
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    skills::enable_skill_all(&f.roots, &[], &id, Some(file.to_string_lossy().as_ref())).unwrap();
    let claude = f.roots.claude.join("skills/review");
    let antigravity = f.roots.home.join(".gemini/antigravity-cli/skills/review");
    assert!(dir.join("SKILL.md").exists());
    assert!(claude.exists());
    assert!(antigravity.exists());
    let disabled = skills::disable_skill_all(&f.roots, &id).unwrap();
    assert!(disabled.changed);
    assert!(disabled.message.contains("保持启用"));
    assert!(!claude.exists());
    assert!(!antigravity.exists());
    assert!(dir.join("SKILL.md").exists());
    assert!(!skills::disable_skill_all(&f.roots, &id).unwrap().changed);
    assert!(dir.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn user_level_enable_removes_same_source_project_links() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let source_id = file.to_string_lossy().into_owned();
    let id = skills::source_id(&source_id);
    skills::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
    assert!(f.cwd.join(".claude/skills/review").is_symlink());
    let outcome =
        skills::enable_skill_all(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
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
    let id = skills::source_id(&source_id);
    let outcome =
        skills::enable_skill_all(&f.roots, &[f.cwd.clone()], &id, Some(&source_id)).unwrap();
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
    let id = skills::source_id(&source_id);
    skills::enable_skill_all(&f.roots, &[], &id, Some(&source_id)).unwrap();
    let error = skills::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source_id))
        .err()
        .expect("project enable must be rejected");
    assert!(error.to_string().contains("用户级启用"));
    assert!(!f.cwd.join(".claude/skills/review").exists());
}

#[test]
fn project_level_enable_covers_every_workspace_and_disable_keeps_the_source() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    let cwds = vec![f.cwd.clone(), second.clone()];
    let skill = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = skills::source_id(skill.to_str().unwrap());
    skills::enable_skill_project(&f.roots, &cwds, &id, Some(skill.to_str().unwrap())).unwrap();
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

    skills::disable_skill_project(&f.roots, &cwds, &id).unwrap();
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
    let id = skills::source_id(skill.to_str().unwrap());
    skills::enable_skill_project(
        &f.roots,
        std::slice::from_ref(&f.cwd),
        &id,
        Some(skill.to_str().unwrap()),
    )
    .unwrap();
    assert!(fs::symlink_metadata(second.join(".agents/skills/review")).is_err());

    skills::inherit_project_skills(
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
    let id = skills::source_id(skill.to_str().unwrap());
    skills::enable_skill_project(&f.roots, &cwds, &id, Some(skill.to_str().unwrap())).unwrap();

    let inventory = skills::merge_inventories(vec![
        skills::list(&f.roots, Some(&cwds[0])),
        skills::list(&f.roots, Some(&cwds[1])),
    ]);
    let row = inventory.resources.iter().find(|row| row.id == id).unwrap();
    assert_eq!(row.entry_paths.len(), 4);
    assert_eq!(row.locations.len(), 4);
    assert!(row.entry_links.iter().all(|link| *link));
    let detail = skills::detail_project(&f.roots, &cwds, &id).unwrap();
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let source = file.to_string_lossy().into_owned();
    skills::enable_skill_project(&f.roots, &[f.cwd.clone()], &id, Some(&source)).unwrap();
    let shared = f.cwd.join(".agents/skills/review");
    assert!(shared.is_symlink());
    assert!(physical.join("SKILL.md").exists());
    let second = f.root.join("repo-two");
    fs::create_dir_all(&second).unwrap();
    assert_eq!(
        skills::find_skill_source(&f.roots, &[f.cwd.clone(), second.clone()], &id),
        Some(file.clone())
    );
    skills::enable_skill_project(&f.roots, &[second.clone()], &id, Some(&source)).unwrap();
    assert!(second.join(".agents/skills/review").is_symlink());
    assert!(second.join(".claude/skills/review").is_symlink());
    let disabled = skills::disable_skill_project(&f.roots, &[f.cwd.clone()], &id).unwrap();
    assert!(disabled.changed);
    assert!(!shared.exists());
    assert!(physical.join("SKILL.md").exists());
    assert!(
        !skills::disable_skill_project(&f.roots, &[f.cwd.clone()], &id)
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let shared = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, None, &physical, &shared.id).unwrap();
    let managed = f.root.join("managed");
    let row = skills::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        skills::skill_source_kind(&row, &managed),
        skills::SkillSourceKind::Agent
    );

    let preview = skills::preview_skill_migration(&f.roots, &[], &managed, &id, None).unwrap();
    assert_eq!(preview.entry_count, 2);
    assert_eq!(preview.physical_entries, 1);
    assert_eq!(
        preview.target_path,
        managed.join("review").to_string_lossy()
    );
    assert!(
        skills::migrate_skill(&f.roots, &[], &managed, &id, None, false).is_err(),
        "a physical source must not be kept after registration"
    );

    let outcome = skills::migrate_skill(&f.roots, &[], &managed, &id, None, true).unwrap();
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
        skills::source_id(
            fs::canonicalize(managed.join("review/SKILL.md"))
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
    );
    let inventory = skills::list(&f.roots, None);
    let row = inventory.resources.iter().next().unwrap();
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let shared = skills::list(&f.roots, Some(&f.cwd))
        .locations
        .into_iter()
        .find(|loc| loc.scope == "project" && loc.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, Some(&f.cwd), &physical, &shared.id).unwrap();
    let managed = f.root.join("managed");

    skills::migrate_skill(&f.roots, &[f.cwd.clone()], &managed, &id, None, true).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert_eq!(fs::read_link(&physical).unwrap(), target);
    assert_eq!(
        fs::read_link(f.cwd.join(".agents/skills/review")).unwrap(),
        target
    );
    let migrated_id = skills::source_id(
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
    let id = skills::source_id(&source_path);
    let shared = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &shared.id).unwrap();
    let managed = f.root.join("managed");
    let row = skills::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(
        skills::skill_source_kind(&row, &managed),
        skills::SkillSourceKind::External
    );

    let preview =
        skills::preview_skill_migration(&f.roots, &[], &managed, &id, Some(&source_path)).unwrap();
    assert_eq!(preview.entry_count, 1);
    assert_eq!(preview.physical_entries, 0);

    skills::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), true).unwrap();
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
    let inventory = skills::list(&f.roots, None);
    let row = inventory.resources.iter().next().unwrap();
    assert_eq!(
        skills::skill_source_kind(row, &managed),
        skills::SkillSourceKind::Library
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
    let id = skills::source_id(&source_path);
    let shared = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &shared.id).unwrap();
    let managed = f.root.join("managed");

    skills::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), false).unwrap();
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

#[cfg(unix)]
#[test]
fn migration_rejects_external_relative_links_before_changing_the_source_or_entries() {
    for delete_source in [false, true] {
        let f = Fixture::new();
        let source = f.source("input", "review");
        f.write(&f.root.join("shared.txt"), "shared reference");
        std::os::unix::fs::symlink("../shared.txt", source.join("ref.txt")).unwrap();
        std::os::unix::fs::symlink("scripts/check.sh", source.join("check.sh")).unwrap();
        let shared = skills::list(&f.roots, None)
            .locations
            .into_iter()
            .find(|loc| loc.agent.is_none())
            .unwrap();
        skills::link_skill(&f.roots, None, &source, &shared.id).unwrap();
        let entry = Path::new(&shared.path).join("review");
        let original_entry = fs::read_link(&entry).unwrap();
        let source_path = fs::canonicalize(source.join("SKILL.md"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let id = skills::source_id(&source_path);
        let managed = f.root.join("managed");
        assert!(
            skills::preview_skill_migration(&f.roots, &[], &managed, &id, Some(&source_path))
                .is_err()
        );
        assert!(skills::migrate_skill(
            &f.roots,
            &[],
            &managed,
            &id,
            Some(&source_path),
            delete_source
        )
        .is_err());
        assert!(!managed.join("review").exists());
        assert_eq!(fs::read_link(&entry).unwrap(), original_entry);
        assert_eq!(
            fs::read_to_string(source.join("ref.txt")).unwrap(),
            "shared reference"
        );

        // An explicit external absolute link and an internal relative link
        // remain valid in both copy and move modes.
        fs::remove_file(source.join("ref.txt")).unwrap();
        std::os::unix::fs::symlink(
            fs::canonicalize(f.root.join("shared.txt")).unwrap(),
            source.join("ref.txt"),
        )
        .unwrap();
        skills::migrate_skill(
            &f.roots,
            &[],
            &managed,
            &id,
            Some(&source_path),
            delete_source,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(managed.join("review/ref.txt")).unwrap(),
            "shared reference"
        );
        assert_eq!(
            fs::read_to_string(managed.join("review/check.sh")).unwrap(),
            "exit 91\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn failed_skill_copy_leaves_no_partial_library_resource_and_can_be_retried() {
    let f = Fixture::new();
    let source = f.source("input", "review");
    // macOS's default temporary path can exceed the Unix socket path limit.
    // Bind at a short path, then move the socket into the source package.
    let socket = PathBuf::from("/tmp").join(format!("ne-{}", uuid::Uuid::new_v4()));
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    fs::rename(&socket, source.join("server.sock")).unwrap();
    let file = fs::canonicalize(source.join("SKILL.md"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let id = skills::source_id(&file);
    let managed = f.root.join("managed");
    assert!(skills::migrate_skill(&f.roots, &[], &managed, &id, Some(&file), false).is_err());
    assert!(source.join("SKILL.md").is_file());
    assert_eq!(fs::read_dir(&managed).unwrap().count(), 0);

    fs::remove_file(source.join("server.sock")).unwrap();
    skills::migrate_skill(&f.roots, &[], &managed, &id, Some(&file), false).unwrap();
    assert_eq!(
        fs::read_to_string(managed.join("review/scripts/check.sh")).unwrap(),
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let shared = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    let wrapper = Path::new(&shared.path).join("review");
    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();
    let managed = f.root.join("managed");

    let row = skills::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(row.entry_links, vec![true]);

    let preview = skills::preview_skill_migration(&f.roots, &[], &managed, &id, None).unwrap();
    assert_eq!(preview.entry_count, 1);
    assert_eq!(preview.physical_entries, 0);

    let outcome = skills::migrate_skill(&f.roots, &[], &managed, &id, None, false).unwrap();
    let target = fs::canonicalize(managed.join("review")).unwrap();
    assert!(fs::symlink_metadata(&wrapper)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&wrapper).unwrap(), target);
    assert!(source.join("SKILL.md").is_file());
    assert_eq!(
        outcome.resource_id,
        skills::source_id(
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let shared = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|loc| loc.scope == "user" && loc.agent.is_none())
        .unwrap();
    let wrapper = Path::new(&shared.path).join("review");
    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();

    let preview = skills::uninstall_preview(&f.roots, &[], &id).unwrap();
    assert!(!preview.delete_source_required);

    let disabled = skills::disable_skill_all(&f.roots, &id).unwrap();
    assert!(disabled.changed);
    assert!(!wrapper.exists());
    assert!(source.join("SKILL.md").is_file());

    fs::create_dir_all(&wrapper).unwrap();
    std::os::unix::fs::symlink(&file, wrapper.join("SKILL.md")).unwrap();
    let revision = skills::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|row| row.id == id)
        .unwrap()
        .revision;
    skills::uninstall_skill(&f.roots, &[], &id, &revision, false).unwrap();
    assert!(!wrapper.exists());
    assert!(source.join("SKILL.md").is_file());
}

#[test]
fn registering_an_external_source_adopts_it_into_the_library() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let managed = f.root.join("managed");

    let outcome = skills::adopt_source(&f.roots, &[], &managed, &source).unwrap();
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

    let outcome = skills::adopt_source(&f.roots, &[], &managed, &library).unwrap();
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
        let error = skills::adopt_source(&f.roots, &[], &managed, &path)
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
    let id = skills::source_id(&source_path);
    let managed = f.root.join("managed");

    let preview =
        skills::preview_skill_migration(&f.roots, &[], &managed, &id, Some(&source_path)).unwrap();
    assert_eq!(preview.entry_count, 0);
    assert_eq!(preview.physical_entries, 0);

    let outcome =
        skills::migrate_skill(&f.roots, &[], &managed, &id, Some(&source_path), true).unwrap();
    assert!(!source.exists());
    let migrated = fs::canonicalize(managed.join("review/SKILL.md")).unwrap();
    assert_eq!(outcome.source_path, migrated.to_string_lossy());
    assert_eq!(outcome.resource_id, skills::source_id(&outcome.source_path));
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
    let mut registry = skills::SkillRegistry::default();
    assert!(skills::discover_library_sources(&managed, &mut registry).unwrap());
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
    assert!(!skills::discover_library_sources(&managed, &mut registry).unwrap());

    let mut inventory = skills::list(&f.roots, None);
    skills::merge_skill_sources(&registry, &mut inventory, true, &managed);
    let row = inventory.resources.iter().next().unwrap();
    assert_eq!(row.id, id);
    assert!(row.registered);
    assert!(row.locations.is_empty());
    assert!(row.entry_paths.is_empty());

    f.write(
        &managed.join("review/SKILL.md"),
        "---\nname: review\ndescription: Updated\n---\n# Review\n",
    );
    assert!(skills::discover_library_sources(&managed, &mut registry).unwrap());
    assert_eq!(registry.sources[0].description, "Updated");
}

#[cfg(unix)]
#[test]
fn a_library_source_is_registered_without_a_registry_row() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed/review", "review");
    fs::create_dir_all(f.roots.home.join(".agents/skills")).unwrap();
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent.is_none())
        .unwrap();
    skills::link_skill(&f.roots, None, &source, &loc.id).unwrap();
    let mut inventory = skills::list(&f.roots, None);
    skills::merge_skill_sources(
        &skills::SkillRegistry::default(),
        &mut inventory,
        true,
        &managed,
    );
    let row = inventory.resources.iter().next().unwrap();
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
    let rows: Vec<_> = skills::list(&f.roots, None).resources.into_iter().collect();
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
    let error = skills::adopt_source(&f.roots, &[], &f.root.join("managed"), &shared)
        .err()
        .expect("Agent roots are containers, not Skill sources");
    assert!(error.to_string().contains("根目录"));
    assert!(
        skills::prepare_source(&f.roots, &[], &f.root.join("managed"), &shared, false).is_err()
    );
    assert!(shared.join("SKILL.md").exists());
    assert!(shared.join("review/SKILL.md").exists());
}

#[test]
fn referencing_an_agent_source_preserves_its_identity_files_and_native_enablement() {
    for project_level in [false, true] {
        let f = Fixture::new();
        let source = if project_level {
            f.cwd.join(".claude/skills/review")
        } else {
            f.roots.claude.join("skills/review")
        };
        f.write(
            &source.join("SKILL.md"),
            "---\nname: review\ndescription: Original source\n---\n# Original\n",
        );
        f.write(&source.join("scripts/check.sh"), "keep original bytes\n");
        let source_file = fs::canonicalize(source.join("SKILL.md")).unwrap();
        let id = skills::source_id(source_file.to_string_lossy().as_ref());
        // A library name conflict must not prevent referencing an external source.
        let managed = f.root.join("managed");
        f.write(&managed.join("review/SKILL.md"), "existing library source");
        let cwds = vec![f.cwd.clone()];
        let preview =
            skills::preview_skill_registration(&f.roots, &cwds, &managed, &id, None).unwrap();
        assert_eq!(preview.physical_entries, 1);
        assert_eq!(preview.entry_count, 1);
        let outcome = skills::prepare_source(&f.roots, &cwds, &managed, &source, false).unwrap();
        assert_eq!(outcome.source_path, source_file.to_string_lossy());
        let db = Db::open(&f.root.join("registry.db")).unwrap();
        let mut registry = skills::SkillRegistry::default();
        skills::register_source(
            &mut registry,
            &outcome.source_path,
            &outcome.name,
            &outcome.description,
        );
        skills::save_skill_registry(&db, &registry).unwrap();
        let saved = skills::load_skill_registry(&db).unwrap();
        let mut inventory = skills::list_catalog(&f.roots, &cwds);
        skills::merge_skill_sources(&saved, &mut inventory, true, &managed);
        let row = inventory.resources.iter().find(|row| row.id == id).unwrap();
        assert!(row.registered);
        assert_eq!(row.entry_links, vec![false]);
        assert_eq!(
            skills::skill_source_kind(row, &managed),
            skills::SkillSourceKind::Agent
        );
        let disabled = if project_level {
            skills::disable_skill_project(&f.roots, &cwds, &id)
        } else {
            skills::disable_skill_all(&f.roots, &id)
        }
        .unwrap();
        assert!(!disabled.changed);
        assert!(source.is_dir());
        assert!(!fs::symlink_metadata(&source)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
            "keep original bytes\n"
        );
        assert_eq!(
            fs::read_to_string(managed.join("review/SKILL.md")).unwrap(),
            "existing library source"
        );
        assert!(skills::list_catalog(&f.roots, &cwds)
            .resources
            .iter()
            .any(|row| row.id == id));
        if project_level {
            let scopes = project_scopes(&f, &[("w", f.cwd.clone())], &id);
            assert!(scopes[0].enabled);
        }
    }
}

#[test]
fn a_registered_external_source_is_managed_without_copying_or_moving() {
    let f = Fixture::new();
    let source = f.source("external/review", "review");
    let managed = f.root.join("managed");
    let install = skills::prepare_source(&f.roots, &[], &managed, &source, false).unwrap();
    let mut registry = skills::SkillRegistry::default();
    skills::register_source(
        &mut registry,
        &install.source_path,
        &install.name,
        &install.description,
    );
    let mut inventory = skills::list(&f.roots, None);
    skills::merge_skill_sources(&registry, &mut inventory, true, &f.root.join("managed"));
    let row = inventory.resources.iter().next().unwrap();
    assert!(row.registered);
    assert_eq!(
        Path::new(&row.path),
        fs::canonicalize(source.join("SKILL.md")).unwrap()
    );
    assert!(!managed.exists());
    assert!(source.join("scripts/check.sh").exists());
    assert!(row.error.is_none());
    assert!(row.entry_paths.is_empty());
    fs::remove_dir_all(&source).unwrap();
    let mut missing = skills::list(&f.roots, None);
    skills::merge_skill_sources(&registry, &mut missing, true, &managed);
    assert!(missing.resources[0].registered);
    assert!(missing.resources[0].error.is_some());
}

#[test]
fn migration_rejects_a_source_already_in_the_library() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let id = skills::source_id(file.to_string_lossy().as_ref());

    let error = skills::preview_skill_migration(
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
    let id = skills::source_id(file.to_string_lossy().as_ref());
    let managed = f.root.join("managed");
    f.write(&managed.join("review/SKILL.md"), "existing");

    let error = skills::preview_skill_migration(&f.roots, &[], &managed, &id, None)
        .err()
        .expect("existing target must be rejected");
    assert!(error.to_string().contains("已存在同名"));
    assert!(skills::migrate_skill(&f.roots, &[], &managed, &id, None, true).is_err());
    assert!(physical.join("SKILL.md").exists());
    assert_eq!(
        fs::read_to_string(managed.join("review/SKILL.md")).unwrap(),
        "existing"
    );
}

/// mklink treats a forward slash as an option prefix, so a path built with
/// `.join(".agents/skills")` makes it read `/skills` as an unknown switch.
#[cfg(windows)]
fn create_junction(link: &Path, target: &Path) -> std::process::Output {
    let link = link.to_string_lossy().replace('/', "\\");
    let target = target.to_string_lossy().replace('/', "\\");
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J", link.as_str(), target.as_str()])
        .output()
        .expect("Windows cmd is required for junction coverage")
}

#[cfg(windows)]
#[test]
fn windows_junction_unlink_keeps_skill_source_and_install_rejects_nested_junctions() {
    let f = Fixture::new();
    let source = f.source("external", "review");
    let root = f.roots.home.join(".agents/skills");
    fs::create_dir_all(&root).unwrap();
    let junction = root.join("review");
    let status = create_junction(&junction, &source);
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let row = skills::list(&f.roots, None)
        .resources
        .into_iter()
        .next()
        .unwrap();
    skills::disable_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
    assert!(!junction.exists());
    assert!(source.join("SKILL.md").exists());
    let nested = source.join("linked-scripts");
    let status = create_junction(&nested, &source.join("scripts"));
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let loc = skills::list(&f.roots, None)
        .locations
        .into_iter()
        .find(|l| l.agent.is_none())
        .unwrap();
    assert!(skills::install_skill(&f.roots, None, &source, &loc.id).is_err());
    assert!(!root.join("review").exists());
    fs::remove_dir(nested).unwrap();
}

#[test]
fn unregistering_an_inactive_external_source_persists_without_deleting_files() {
    for remove_links in [false, true] {
        let f = Fixture::new();
        let source = f.source("external/review", "review");
        let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
        let db = Db::open(&f.root.join("registry.db")).unwrap();
        let mut registry = skills::SkillRegistry::default();
        let id = skills::register_source(&mut registry, file.to_str().unwrap(), "review", "Review")
            .id
            .clone();
        skills::save_skill_registry(&db, &registry).unwrap();
        let mut registry = skills::load_skill_registry(&db).unwrap();
        skills::unregister_skill(
            &f.roots,
            &[],
            &f.root.join("managed"),
            &mut registry,
            &id,
            remove_links,
        )
        .unwrap();
        skills::save_skill_registry(&db, &registry).unwrap();
        assert!(skills::load_skill_registry(&db).unwrap().sources.is_empty());
        assert_eq!(
            fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
            "exit 91\n"
        );
        let mut inventory = skills::list_catalog(&f.roots, &[]);
        skills::merge_skill_sources(&registry, &mut inventory, true, &f.root.join("managed"));
        assert!(!inventory.resources.iter().any(|row| row.id == id));
    }
}

#[test]
fn unregister_rejects_library_and_unregistered_sources_without_changing_them() {
    let f = Fixture::new();
    let managed = f.root.join("managed");
    let source = f.source("managed/review", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let mut registry = skills::SkillRegistry::default();
    let id = skills::register_source(&mut registry, file.to_str().unwrap(), "review", "Review")
        .id
        .clone();
    for remove_links in [false, true] {
        assert!(skills::unregister_skill(
            &f.roots,
            &[],
            &managed,
            &mut registry,
            &id,
            remove_links
        )
        .is_err());
        assert!(skills::find_source(&registry, &id).is_some());
        assert!(skills::unregister_skill(
            &f.roots,
            &[],
            &managed,
            &mut registry,
            "unknown",
            remove_links
        )
        .is_err());
    }
    assert_eq!(
        fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
}

#[cfg(unix)]
#[test]
fn unregister_preserves_physical_entries_and_ancestor_links_and_optionally_removes_skill_links() {
    use std::os::unix::fs::symlink;
    for remove_links in [false, true] {
        let f = Fixture::new();
        let source = f.source("home/.claude/skills/review", "review");
        let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
        let ancestor = f.roots.home.join(".agents/skills");
        fs::create_dir_all(ancestor.parent().unwrap()).unwrap();
        symlink(source.parent().unwrap(), &ancestor).unwrap();
        let links = [
            f.roots.pi.join("skills/review"),
            f.cwd.join(".claude/skills/review"),
            f.root.join("repo-two/.agents/skills/review"),
        ];
        for link in &links {
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&source, link).unwrap();
        }
        let wrapper = f.cwd.join(".pi/skills/review");
        fs::create_dir_all(&wrapper).unwrap();
        symlink(source.join("SKILL.md"), wrapper.join("SKILL.md")).unwrap();
        symlink(source.join("scripts"), wrapper.join("scripts")).unwrap();
        let unrelated = f.source("other", "other");
        let unrelated_link = f.cwd.join(".claude/skills/other");
        symlink(&unrelated, &unrelated_link).unwrap();
        let mut registry = skills::SkillRegistry::default();
        let id = skills::register_source(&mut registry, file.to_str().unwrap(), "review", "Review")
            .id
            .clone();
        let other_id = skills::register_source(
            &mut registry,
            fs::canonicalize(unrelated.join("SKILL.md"))
                .unwrap()
                .to_str()
                .unwrap(),
            "other",
            "Other",
        )
        .id
        .clone();
        let cwds = [f.cwd.clone(), f.root.join("repo-two")];
        skills::unregister_skill(
            &f.roots,
            &cwds,
            &f.root.join("managed"),
            &mut registry,
            &id,
            remove_links,
        )
        .unwrap();
        assert!(skills::find_source(&registry, &id).is_none());
        assert!(skills::find_source(&registry, &other_id).is_some());
        for link in links.iter().chain(std::iter::once(&wrapper)) {
            assert_eq!(fs::symlink_metadata(link).is_ok(), !remove_links);
        }
        assert!(ancestor.is_symlink());
        assert!(unrelated_link.is_symlink());
        assert_eq!(
            fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
            "exit 91\n"
        );
        let mut inventory = skills::list_catalog(&f.roots, &cwds);
        skills::merge_skill_sources(&registry, &mut inventory, true, &f.root.join("managed"));
        let row = inventory.resources.iter().find(|row| row.id == id).unwrap();
        assert!(!row.registered);
        assert!(row.entry_links.iter().any(|link| !link));
        assert_eq!(row.entry_links.iter().any(|link| *link), !remove_links);
    }
}

#[cfg(unix)]
#[test]
fn unregister_cleans_dangling_source_links_and_rejects_a_library_alias() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let source = f.source("external/review", "review");
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let load_root = f.cwd.join(".claude/skills");
    fs::create_dir_all(&load_root).unwrap();
    let link = load_root.join("review");
    // A relative link must still be recognized after the source is deleted.
    symlink("../../../external/review", &link).unwrap();
    assert!(link.join("SKILL.md").is_file());
    let mut registry = skills::SkillRegistry::default();
    let id = skills::register_source(&mut registry, file.to_str().unwrap(), "review", "Review")
        .id
        .clone();
    fs::remove_dir_all(&source).unwrap();
    skills::unregister_skill(
        &f.roots,
        &[f.cwd.clone()],
        &f.root.join("managed"),
        &mut registry,
        &id,
        true,
    )
    .unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(registry.sources.is_empty());

    let library_source = f.source("managed/review", "review");
    let alias = f.root.join("alias");
    symlink(&library_source, &alias).unwrap();
    let id = skills::register_source(
        &mut registry,
        alias.join("SKILL.md").to_str().unwrap(),
        "review",
        "Review",
    )
    .id
    .clone();
    assert!(skills::unregister_skill(
        &f.roots,
        &[],
        &f.root.join("managed"),
        &mut registry,
        &id,
        true
    )
    .is_err());
    assert!(skills::find_source(&registry, &id).is_some());
    assert!(library_source.join("SKILL.md").is_file());
}

#[cfg(windows)]
#[test]
fn windows_unregister_removes_skill_junctions_but_keeps_source_and_ancestor_junctions() {
    let f = Fixture::new();
    let source = f.source("home/.claude/skills/review", "review");
    let ancestor = f.roots.home.join(".agents/skills");
    fs::create_dir_all(ancestor.parent().unwrap()).unwrap();
    let status = create_junction(&ancestor, source.parent().unwrap());
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let link = f.cwd.join(".claude/skills/review");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    let status = create_junction(&link, &source);
    assert!(
        status.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let file = fs::canonicalize(source.join("SKILL.md")).unwrap();
    let mut registry = skills::SkillRegistry::default();
    let id = skills::register_source(&mut registry, file.to_str().unwrap(), "review", "Review")
        .id
        .clone();
    skills::unregister_skill(
        &f.roots,
        &[f.cwd.clone()],
        &f.root.join("managed"),
        &mut registry,
        &id,
        true,
    )
    .unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(ancestor.join("review/SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(source.join("scripts/check.sh")).unwrap(),
        "exit 91\n"
    );
    // Remove the ancestor junction explicitly so fixture cleanup cannot follow it.
    fs::remove_dir(ancestor).unwrap();
}
