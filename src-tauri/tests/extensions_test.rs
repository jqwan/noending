//! Native customization contracts, isolated from the user's real Agent homes.
use noending_lib::{
    domain::Agent,
    extensions::{self, Roots},
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
            .find(|l| {
                l.kind == "mcp"
                    && l.agent == Some(agent)
                    && l.scope == scope
                    && !l.label.contains("私有")
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
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
    assert!(extensions::remove_skill(&f.roots, None, &row.id, &row.entry_paths[0], "old").is_err());
    extensions::remove_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
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
    let shared_entry = shared
        .join("review/SKILL.md")
        .to_string_lossy()
        .into_owned();
    extensions::remove_skill(&f.roots, None, &row.id, &shared_entry, &row.revision).unwrap();
    assert!(!shared.join("review").exists());
    assert!(source.join("SKILL.md").exists());
    assert!(claude.join("review/SKILL.md").exists());
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
fn project_mcp_is_local_to_its_directory_and_claude_private_scope_keeps_other_projects() {
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
    let local = list
        .locations
        .iter()
        .find(|l| l.kind == "mcp" && l.label.contains("私有"))
        .unwrap();
    f.write(
        Path::new(&local.path),
        "{\"projects\":{\"/other\":{\"keep\":true}},\"theme\":\"dark\"}",
    );
    extensions::save_mcp(
        &f.roots,
        Some(&f.cwd),
        &local.id,
        &extensions::location_revision(&f.roots, Some(&f.cwd), &local.id).unwrap(),
        "docs",
        json!({"type":"http","url":"https://example/mcp"}),
        true,
    )
    .unwrap();
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&local.path).unwrap()).unwrap();
    assert_eq!(stored["projects"]["/other"]["keep"], true);
    assert_eq!(stored["theme"], "dark");
    assert_eq!(
        stored["projects"][f.cwd.to_str().unwrap()]["mcpServers"]["docs"]["type"],
        "http"
    );
}

#[test]
fn package_owned_components_are_read_only_and_scanning_does_not_run_bundled_commands() {
    let f = Fixture::new();
    let root = f.root.join("package");
    f.write(
        &root.join(".claude-plugin/plugin.json"),
        &json!({
            "name": "team",
            "skills": ["./skills/", "skills/../skills"],
            "mcpServers": "./.mcp.json"
        })
        .to_string(),
    );
    f.write(
        &root.join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\nRead source.",
    );
    f.write(
        &root.join(".mcp.json"),
        "{\"mcpServers\":{\"bundled\":{\"command\":\"must-not-be-executed\"}}}",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join(".mcp.json"), root.join("mcp.json")).unwrap();
    f.write(&f.roots.claude.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"team@local":[{"scope":"user","installPath":root.to_str().unwrap(),"version":"1.2.3"}]}}).to_string());
    f.write(
        &f.roots.claude.join("settings.json"),
        "{\"enabledPlugins\":{\"team@local\":false}}",
    );
    let list = extensions::list(&f.roots, None);
    let package = list.resources.iter().find(|r| r.kind == "package").unwrap();
    assert_eq!(package.enabled, Some(false));
    let skill = list.resources.iter().find(|r| r.kind == "skill").unwrap();
    assert!(skill.managed);
    assert_eq!(skill.entry_paths.len(), 1);
    let mcp = list.resources.iter().find(|r| r.kind == "mcp").unwrap();
    assert_eq!(list.resources.iter().filter(|r| r.kind == "mcp").count(), 1);
    assert_eq!(
        Path::new(&mcp.path),
        fs::canonicalize(root.join(".mcp.json")).unwrap()
    );
    let detail = extensions::detail(&f.roots, None, &mcp.id).unwrap();
    assert_eq!(detail.config.unwrap()["command"], "must-not-be-executed");
    assert!(mcp.managed);
    assert_eq!(mcp.enabled, Some(false));
    assert!(extensions::remove_skill(
        &f.roots,
        None,
        &skill.id,
        &skill.entry_paths[0],
        &skill.revision
    )
    .is_err());
    assert!(extensions::remove_mcp(&f.roots, None, &mcp.id, &mcp.revision).is_err());
    let loc = list
        .locations
        .iter()
        .find(|l| mcp.locations.contains(&l.id))
        .unwrap();
    assert!(extensions::save_mcp(
        &f.roots,
        None,
        &loc.id,
        &mcp.revision,
        "bundled",
        json!({"command":"replace"}),
        false
    )
    .is_err());
    assert!(root.join("skills/review/SKILL.md").exists());
}

#[test]
fn codex_skill_switch_edits_only_native_config_and_restores_inheritance() {
    let f = Fixture::new();
    let source = f.roots.home.join(".agents/skills/review");
    f.write(
        &source.join("SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\nKeep unchanged.",
    );
    f.write(&f.roots.codex.join("config.toml"), "# keep this\nmodel = \"existing\"\n[[skills.config]]\npath = \"/unrelated/SKILL.md\"\nenabled = false\n");
    let row = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|r| r.kind == "skill")
        .unwrap();
    assert_eq!(row.skill_controls[0].enabled, None);
    extensions::set_skill_state(
        &f.roots,
        None,
        &row.id,
        "user",
        &row.skill_controls[0].revision,
        Some(false),
    )
    .unwrap();
    let current = extensions::list(&f.roots, None)
        .resources
        .into_iter()
        .find(|r| r.kind == "skill")
        .unwrap();
    assert_eq!(current.skill_controls[0].enabled, Some(false));
    assert!(extensions::set_skill_state(
        &f.roots,
        None,
        &row.id,
        "user",
        &row.skill_controls[0].revision,
        Some(true)
    )
    .is_err());
    extensions::set_skill_state(
        &f.roots,
        None,
        &row.id,
        "user",
        &current.skill_controls[0].revision,
        None,
    )
    .unwrap();
    let raw = fs::read_to_string(f.roots.codex.join("config.toml")).unwrap();
    assert!(raw.contains("# keep this"));
    let value: serde_json::Value = toml_edit::de::from_str(&raw).unwrap();
    assert_eq!(value["model"], "existing");
    assert_eq!(value["skills"]["config"].as_array().unwrap().len(), 1);
    assert_eq!(value["skills"]["config"][0]["path"], "/unrelated/SKILL.md");
    assert!(fs::read_to_string(source.join("SKILL.md"))
        .unwrap()
        .contains("Keep unchanged."));
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
    extensions::remove_skill(&f.roots, None, &row.id, &row.entry_paths[0], &row.revision).unwrap();
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

#[test]
fn codex_project_switches_override_user_resources_without_modifying_user_configuration() {
    let f = Fixture::new();
    let user_config = "model = \"personal\"\n[plugins.\"review@team\"]\nenabled = true\n";
    f.write(&f.roots.codex.join("config.toml"), user_config);
    f.write(
        &f.cwd.join(".codex/config.toml"),
        "# team settings\nmodel = \"project\"\n",
    );
    f.write(
        &f.roots.home.join(".agents/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\nKeep this source.",
    );
    let list = extensions::list(&f.roots, Some(&f.cwd));
    let package = list.resources.iter().find(|r| r.kind == "package").unwrap();
    extensions::package_action(
        &f.roots,
        Some(&f.cwd),
        Agent::Codex,
        "project_disable",
        &package.name,
        Some(&package.id),
        Some(&package.revision),
    )
    .unwrap();
    let value: serde_json::Value =
        toml_edit::de::from_str(&fs::read_to_string(f.cwd.join(".codex/config.toml")).unwrap())
            .unwrap();
    assert_eq!(value["plugins"]["review@team"]["enabled"], false);
    assert_eq!(
        fs::read_to_string(f.roots.codex.join("config.toml")).unwrap(),
        user_config
    );
    let list = extensions::list(&f.roots, Some(&f.cwd));
    let project_package = list
        .resources
        .iter()
        .find(|r| {
            r.kind == "package" && r.path == f.cwd.join(".codex/config.toml").to_string_lossy()
        })
        .unwrap();
    extensions::package_action(
        &f.roots,
        Some(&f.cwd),
        Agent::Codex,
        "inherit",
        &project_package.name,
        Some(&project_package.id),
        Some(&project_package.revision),
    )
    .unwrap();
    let raw = fs::read_to_string(f.cwd.join(".codex/config.toml")).unwrap();
    let value: serde_json::Value = toml_edit::de::from_str(&raw).unwrap();
    assert!(value
        .get("plugins")
        .and_then(|p| p.get("review@team"))
        .is_none());
    assert_eq!(value["model"], "project");
    assert!(raw.contains("# team settings"));
    let skill = extensions::list(&f.roots, Some(&f.cwd))
        .resources
        .into_iter()
        .find(|r| r.kind == "skill")
        .unwrap();
    let control = skill
        .skill_controls
        .iter()
        .find(|c| c.scope == "project")
        .unwrap();
    extensions::set_skill_state(
        &f.roots,
        Some(&f.cwd),
        &skill.id,
        "project",
        &control.revision,
        Some(false),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(f.roots.codex.join("config.toml")).unwrap(),
        user_config
    );
}
