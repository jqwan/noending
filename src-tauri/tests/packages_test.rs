//! Plugin package discovery contracts, isolated from the user's real Agent
//! homes. Native registry files are the only install evidence; the tests fix
//! their shapes so silent upstream format changes fail here first.
use noending_lib::{
    packages::{self, Capabilities, Origin},
    skills::Roots,
};
use std::{fs, path::PathBuf};

struct Fixture {
    root: PathBuf,
    roots: Roots,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("noending-packages-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        Self {
            roots: Roots {
                codex: home.join(".codex"),
                claude: home.join(".claude"),
                pi: home.join(".pi/agent"),
                home,
            },
            root,
        }
    }

    fn discover(&self) -> packages::PackageInventory {
        packages::discover(&self.roots)
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn agent_key(agent: &str) -> String {
    match agent {
        "claude_code" => "claudecode".into(),
        other => other.into(),
    }
}

fn entry<'a>(
    inventory: &'a packages::PackageInventory,
    agent: &str,
    name: &str,
) -> &'a packages::PackageEntry {
    inventory
        .packages
        .iter()
        .find(|entry| {
            entry
                .agent
                .as_ref()
                .map(|value| format!("{value:?}").to_lowercase() == agent_key(agent))
                .unwrap_or(false)
                && entry.name == name
        })
        .unwrap_or_else(|| panic!("package {agent}/{name} not found"))
}

#[test]
fn codex_reads_the_config_registry_and_resolves_single_version_roots() {
    let f = Fixture::new();
    let cache_version = f.dir("home/.codex/plugins/cache/openai-curated/review/1.0.0");
    f.write(
        "home/.codex/config.toml",
        "[plugins.\"review@openai-curated\"]\nenabled = true\n\n[plugins.\"chrome@openai-bundled\"]\nenabled = false\n\n[plugins.\"no-flag@openai-curated\"]\n",
    );
    let inventory = f.discover();
    let review = entry(&inventory, "codex", "review");
    assert_eq!(review.origin, Origin::Market);
    assert_eq!(review.marketplace.as_deref(), Some("openai-curated"));
    assert_eq!(review.scopes.len(), 1);
    assert_eq!(review.scopes[0].state, packages::ToggleState::Enabled);
    assert_eq!(
        review.install_path.as_deref(),
        Some(cache_version.to_str().unwrap())
    );
    assert!(review.capabilities.uninstall);
    assert!(review.capabilities.toggle);

    let chrome = entry(&inventory, "codex", "chrome");
    assert_eq!(chrome.origin, Origin::Bundled);
    assert_eq!(chrome.scopes[0].state, packages::ToggleState::Disabled);
    // Bundled plugins can be toggled natively but not removed...
    assert!(!chrome.capabilities.uninstall && chrome.capabilities.toggle);
    // ...and the toggle actually works: bundled markets are ordinary
    // config.toml entries, writing `enabled` is the Agent's own switch.
    noending_lib::packages::set_enabled(&inventory, &chrome.id, &chrome.revision, true).unwrap();
    let raw = fs::read_to_string(f.root.join("home/.codex/config.toml")).unwrap();
    let doc = raw.parse::<toml_edit::Document>().unwrap();
    assert_eq!(
        doc["plugins"]["chrome@openai-bundled"]["enabled"].as_bool(),
        Some(true)
    );
    // Re-discovery reports the re-enabled state.
    assert_eq!(
        entry(&f.discover(), "codex", "chrome").scopes[0].state,
        packages::ToggleState::Enabled
    );

    let unset = entry(&inventory, "codex", "no-flag");
    assert_eq!(unset.scopes[0].state, packages::ToggleState::Unset);
}

#[test]
fn codex_ambiguous_cache_versions_report_no_root() {
    let f = Fixture::new();
    f.dir("home/.codex/plugins/cache/openai-curated/review/1.0.0");
    f.dir("home/.codex/plugins/cache/openai-curated/review/2.0.0");
    f.write(
        "home/.codex/config.toml",
        "[plugins.\"review@openai-curated\"]\nenabled = true\n",
    );
    let inventory = f.discover();
    assert_eq!(entry(&inventory, "codex", "review").install_path, None);
}

#[test]
fn claude_registry_is_the_install_evidence_and_settings_hold_the_toggle() {
    let f = Fixture::new();
    let install_path =
        f.dir("home/.claude/plugins/cache/claude-plugins-official/superpowers/5.1.0");
    // A cache directory without a registry entry must not count as installed.
    f.dir("home/.claude/plugins/cache/claude-plugins-official/ghost/1.0.0");
    f.write(
        "home/.claude/plugins/installed_plugins.json",
        r#"{"version": 2, "plugins": {
            "superpowers@claude-plugins-official": [
                {"scope": "user", "installPath": "CLAUDE_INSTALL", "version": "5.1.0",
                 "installedAt": "2026-05-31T14:58:21.246Z", "lastUpdated": "2026-06-01T10:00:00.000Z"},
                {"scope": "local", "projectPath": "/tmp/team-repo", "installPath": "CLAUDE_LOCAL", "version": "5.1.0"}
            ]}}"#,
    );
    f.write(
        "home/.claude/settings.json",
        r#"{"enabledPlugins": {"superpowers@claude-plugins-official": true}}"#,
    );
    let inventory = f.discover();
    let user = inventory
        .packages
        .iter()
        .find(|entry| entry.name == "superpowers" && entry.scopes[0].scope == packages::Scope::User)
        .expect("user-scope entry");
    assert_eq!(user.marketplace.as_deref(), Some("claude-plugins-official"));
    assert_eq!(user.version, "5.1.0");
    assert_eq!(user.install_path.as_deref(), Some("CLAUDE_INSTALL"));
    assert_eq!(user.scopes[0].state, packages::ToggleState::Enabled);
    assert!(user.capabilities.toggle);

    // Project/local installs stay Agent-managed and are not discovered.
    assert!(inventory
        .packages
        .iter()
        .all(|entry| entry.scopes[0].scope == packages::Scope::User));
    assert!(inventory.packages.iter().all(|entry| entry.name != "ghost"));
    let _ = install_path;
}

#[test]
fn claude_unset_toggle_reports_unset_not_disabled() {
    let f = Fixture::new();
    f.write(
        "home/.claude/plugins/installed_plugins.json",
        r#"{"version": 2, "plugins": {"quiet@claude-plugins-official": [
            {"scope": "user", "installPath": "P", "version": "1.0"}]}}"#,
    );
    let inventory = f.discover();
    assert_eq!(
        entry(&inventory, "claude_code", "quiet").scopes[0].state,
        packages::ToggleState::Unset
    );
}

#[test]
fn pi_packages_split_remote_and_local_sources_without_toggles() {
    let f = Fixture::new();
    // Relative sources resolve against the settings directory.
    let local = f.dir("home/.pi/agent/local-toolkit");
    f.write(
        "home/.pi/agent/settings.json",
        r#"{"packages": ["npm:team-tools", "./local-toolkit"]}"#,
    );
    let inventory = f.discover();
    let npm = entry(&inventory, "pi", "npm:team-tools");
    assert_eq!(npm.origin, Origin::Market);
    assert!(!npm.capabilities.toggle, "Pi has no native package toggle");
    assert_eq!(npm.scopes[0].state, packages::ToggleState::Enabled);
    let relative = entry(&inventory, "pi", "./local-toolkit");
    assert_eq!(relative.origin, Origin::Local);
    assert_eq!(
        relative.install_path.as_deref(),
        Some(local.to_str().unwrap())
    );
}

#[test]
fn antigravity_installed_from_records_the_marketplace_origin() {
    let f = Fixture::new();
    f.dir("home/.gemini/antigravity-cli/plugins/review");
    f.write(
        "home/.gemini/antigravity-cli/plugins/review/plugin.json",
        r#"{"name": "review"}"#,
    );
    f.write(
        "home/.gemini/config/config.json",
        r#"{"plugins": {"review": {"enabled": true, "installedFrom": {"id": "antigravity-plugins-official/plugin/review", "marketplace": "antigravity-plugins-official", "version": "1.0.6"}}}}"#,
    );
    let inventory = f.discover();
    let review = entry(&inventory, "antigravity", "review");
    assert_eq!(review.origin, Origin::Market);
    assert_eq!(
        review.marketplace.as_deref(),
        Some("antigravity-plugins-official")
    );
    // The manifest carries no version; the registry fallback supplies it.
    assert_eq!(review.version, "1.0.6");

    // Without installedFrom the install stays a local-directory origin.
    f.dir("home/.gemini/antigravity-cli/plugins/local-tool");
    f.write(
        "home/.gemini/antigravity-cli/plugins/local-tool/plugin.json",
        r#"{"name": "local-tool"}"#,
    );
    let inventory = f.discover();
    assert_eq!(
        entry(&inventory, "antigravity", "local-tool").origin,
        Origin::Local
    );
}

#[test]
fn antigravity_reads_plugin_dirs_and_marks_noending_managed_readonly() {
    let f = Fixture::new();
    f.dir("home/.gemini/antigravity-cli/plugins/review-plugin");
    f.write(
        "home/.gemini/antigravity-cli/plugins/review-plugin/plugin.json",
        r#"{"name": "review-plugin", "version": "0.3.0", "description": "Review helpers"}"#,
    );
    f.dir("home/.gemini/antigravity-cli/plugins/noending-terminal-identity");
    f.write(
        "home/.gemini/antigravity-cli/plugins/noending-terminal-identity/plugin.json",
        r#"{"name": "noending-terminal-identity"}"#,
    );
    f.write(
        "home/.gemini/config/config.json",
        r#"{"plugins": {"review-plugin": {"enabled": true}}}"#,
    );
    let inventory = f.discover();
    let review = entry(&inventory, "antigravity", "review-plugin");
    assert_eq!(review.version, "0.3.0");
    assert_eq!(review.description, "Review helpers");
    assert_eq!(review.scopes[0].state, packages::ToggleState::Enabled);
    assert!(review.capabilities.uninstall);
    let managed = entry(&inventory, "antigravity", "noending-terminal-identity");
    assert_eq!(managed.origin, Origin::NoEndingManaged);
    assert!(
        noending_lib::packages::set_enabled(&inventory, &managed.id, &managed.revision, true)
            .is_err()
    );
    assert_eq!(
        managed.capabilities,
        Capabilities {
            uninstall: false,
            toggle: false
        }
    );
    // The built-in ships enabled and never writes the switch: an absent key
    // IS the default-on state. Only an explicit false reads as disabled.
    assert_eq!(managed.scopes[0].state, packages::ToggleState::Enabled);
    f.dir("home/.gemini/antigravity-cli/plugins/noending-disabled-experiment");
    f.write(
        "home/.gemini/antigravity-cli/plugins/noending-disabled-experiment/plugin.json",
        r#"{"name": "noending-disabled-experiment"}"#,
    );
    f.write(
        "home/.gemini/config/config.json",
        r#"{"plugins": {"noending-disabled-experiment": {"enabled": false}}}"#,
    );
    let inventory = f.discover();
    assert_eq!(
        entry(&inventory, "antigravity", "noending-disabled-experiment").scopes[0].state,
        packages::ToggleState::Disabled
    );
}

#[test]
fn workbuddy_discovery_is_readonly_and_marks_builtin_market() {
    let f = Fixture::new();
    f.write(
        "home/.workbuddy/plugins/installed_plugins.json",
        r#"{"version": 2, "plugins": {
            "find-skills@codebuddy-plugins-official": [{"scope": "user", "installPath": "WB1", "version": "1.0.0"}],
            "welcomemode-code@workbuddy-builtin": [{"scope": "user", "installPath": "WB2", "version": "2.0.0"}]}}"#,
    );
    f.write(
        "home/.workbuddy/plugins/configured-plugins.json",
        r#"{"version": 1, "builtInMarketPlugins": [{"name": "welcomemode-code", "marketplaceName": "workbuddy-builtin", "scope": "user"}]}"#,
    );
    let inventory = f.discover();
    let find_skills = entry(&inventory, "workbuddy", "find-skills");
    assert_eq!(find_skills.origin, Origin::Market);
    // No enablement evidence in this registry: the Agent owns the state.
    assert_eq!(find_skills.scopes[0].state, packages::ToggleState::Unknown);
    assert_eq!(
        find_skills.capabilities,
        Capabilities {
            uninstall: false,
            toggle: false
        }
    );
    let builtin = entry(&inventory, "workbuddy", "welcomemode-code");
    assert_eq!(builtin.origin, Origin::Bundled);
}

#[test]
fn detail_scans_package_contents_for_the_overview() {
    let f = Fixture::new();
    let root = f.dir("home/.claude/plugins/cache/m/review/1.0.0");
    f.write(
        "home/.claude/plugins/cache/m/review/1.0.0/.claude-plugin/plugin.json",
        r#"{"name": "review"}"#,
    );
    f.write(
        "home/.claude/plugins/cache/m/review/1.0.0/skills/pr-check/SKILL.md",
        "x",
    );
    f.write("home/.claude/plugins/cache/m/review/1.0.0/.mcp.json", "{}");
    f.write(
        "home/.claude/plugins/installed_plugins.json",
        r#"{"version": 2, "plugins": {"review@m": [{"scope": "user", "installPath": "ROOT", "version": "1.0.0"}]}}"#,
    );
    let mut inventory = packages::discover(&f.roots);
    let install_path = root.to_string_lossy().into_owned();
    inventory.packages.iter_mut().for_each(|entry| {
        if entry.name == "review" {
            entry.install_path = Some(install_path.clone());
        }
    });
    let detail = packages::detail(&inventory, &inventory.packages[0].id)
        .unwrap()
        .expect("detail");
    assert_eq!(detail.contents.skills, vec!["pr-check"]);
    assert_eq!(detail.contents.mcp_files, vec![".mcp.json"]);
}

#[test]
fn cli_args_pin_the_native_commands_per_agent() {
    use noending_lib::domain::Agent;
    use noending_lib::packages::{cli_args, PackageAction};
    let install = |agent, target| cli_args(agent, PackageAction::Install, target);
    let uninstall = |agent, target| cli_args(agent, PackageAction::Uninstall, target);
    // Install args exist only for local-directory agents; NoEnding never
    // triggers a marketplace download through Codex/Claude selectors.
    assert!(install(Agent::Codex, "review@openai-curated").is_err());
    assert!(install(Agent::ClaudeCode, "review@claude-plugins-official").is_err());
    assert_eq!(
        install(Agent::Pi, "./local-toolkit").unwrap(),
        ["install", "./local-toolkit"]
    );
    assert_eq!(
        install(Agent::Antigravity, "./toolkit").unwrap(),
        ["plugin", "install", "./toolkit"]
    );
    assert_eq!(
        uninstall(Agent::Codex, "review@openai-curated").unwrap(),
        ["plugin", "remove", "review@openai-curated", "--json"]
    );
    assert_eq!(
        uninstall(Agent::ClaudeCode, "review@claude-plugins-official").unwrap(),
        [
            "plugin",
            "uninstall",
            "review@claude-plugins-official",
            "--scope",
            "user"
        ]
    );
    assert!(cli_args(Agent::WorkBuddy, PackageAction::Install, "x").is_err());
    assert!(install(Agent::Pi, "-rm-rf").is_err());
    assert!(install(Agent::Pi, "").is_err());
    assert!(install(Agent::Pi, "a\nb").is_err());
}

#[test]
fn toggle_writes_the_native_flag_and_preserves_unrelated_config() {
    let f = Fixture::new();
    f.write(
        "home/.codex/config.toml",
        "# user comment\nmodel = \"gpt-5\"\n[plugins.\"review@openai-curated\"]\nenabled = true\n\n[mcp_servers.x]\ncommand = \"run\"\n",
    );
    let inventory = f.discover();
    let review = entry(&inventory, "codex", "review");
    noending_lib::packages::set_enabled(&inventory, &review.id, &review.revision, false).unwrap();
    let raw = fs::read_to_string(f.root.join("home/.codex/config.toml")).unwrap();
    assert!(raw.contains("# user comment"));
    assert!(raw.contains("model = \"gpt-5\""));
    assert!(raw.contains("[mcp_servers.x]"));
    let doc = raw.parse::<toml_edit::Document>().unwrap();
    assert_eq!(
        doc["plugins"]["review@openai-curated"]["enabled"].as_bool(),
        Some(false)
    );

    // Stale revision is rejected instead of overwriting external edits.
    assert!(noending_lib::packages::set_enabled(&inventory, &review.id, "stale", true).is_err());

    // Re-discovery reports the disabled state.
    let inventory = f.discover();
    assert_eq!(
        entry(&inventory, "codex", "review").scopes[0].state,
        packages::ToggleState::Disabled
    );
}

#[test]
fn claude_and_antigravity_toggles_map_to_native_cli_commands() {
    use noending_lib::domain::Agent;
    use noending_lib::packages::{cli_args, PackageAction};
    // Claude and Antigravity enable/disable ride the Agents' own CLI
    // commands; NoEnding never hand-writes their settings files. Args are
    // pinned here as a pure contract — running set_enabled in a test would
    // launch the real binary (see the "mixed" pollution incident).
    assert_eq!(
        cli_args(Agent::ClaudeCode, PackageAction::Enable, "review@m").unwrap(),
        ["plugin", "enable", "review@m", "--json"]
    );
    assert_eq!(
        cli_args(Agent::ClaudeCode, PackageAction::Disable, "review@m").unwrap(),
        ["plugin", "disable", "review@m", "--json"]
    );
    // agy keys enable/disable on the bare plugin name (its own list output).
    assert_eq!(
        cli_args(Agent::Antigravity, PackageAction::Enable, "review").unwrap(),
        ["plugin", "enable", "review"]
    );
    assert_eq!(
        cli_args(Agent::Antigravity, PackageAction::Disable, "review").unwrap(),
        ["plugin", "disable", "review"]
    );
    // Codex has no native toggle subcommand; the mapping refuses instead of
    // silently writing files behind a fake command.
    assert!(cli_args(Agent::Codex, PackageAction::Enable, "x@m").is_err());
    assert!(cli_args(Agent::Pi, PackageAction::Enable, "x").is_err());
}

#[test]
fn import_preview_lists_one_option_per_installable_manifest() {
    let f = Fixture::new();
    let source = f.dir("external/toolkit");
    f.write(
        "external/toolkit/plugin.json",
        r#"{"name": "toolkit", "description": "Preview me"}"#,
    );
    let preview = packages::preview_import(source.to_str().unwrap()).unwrap();
    assert_eq!(preview.options.len(), 1);
    let option = &preview.options[0];
    assert_eq!(option.name, "toolkit");
    assert_eq!(option.description, "Preview me");
    assert_eq!(option.manifest, "plugin.json");
    assert_eq!(option.agent, noending_lib::domain::Agent::Antigravity);

    // package.json at the root is a Pi package.
    let pi_dir = f.dir("external/pi-tool");
    f.write("external/pi-tool/package.json", r#"{"name": "pi-tool"}"#);
    f.dir("external/pi-tool/skills/greet");
    assert_eq!(
        packages::preview_import(pi_dir.to_str().unwrap())
            .unwrap()
            .options[0]
            .agent,
        noending_lib::domain::Agent::Pi
    );

    // Recognized formats NoEnding cannot install from a directory are named
    // in the rejection instead of a generic miss.
    let claude = f.dir("external/claude-tool");
    f.write(
        "external/claude-tool/.claude-plugin/plugin.json",
        r#"{"name": "claude-tool"}"#,
    );
    let message = format!(
        "{}",
        packages::preview_import(claude.to_str().unwrap()).unwrap_err()
    );
    assert!(message.contains("Claude"), "{message}");
    let codex = f.dir("external/codex-tool");
    f.write(
        "external/codex-tool/.codex-plugin/plugin.json",
        r#"{"name": "codex-tool"}"#,
    );
    assert!(packages::preview_import(codex.to_str().unwrap()).is_err());

    // A dual-format directory lists BOTH targets; the dialog offers them as
    // multi-select and each manifest carries its own metadata.
    let mixed = f.dir("external/mixed-tool");
    f.write("external/mixed-tool/plugin.json", r#"{"name": "a-plugin"}"#);
    f.write(
        "external/mixed-tool/package.json",
        r#"{"name": "a-package"}"#,
    );
    f.dir("external/mixed-tool/extensions");
    let preview = packages::preview_import(mixed.to_str().unwrap()).unwrap();
    let by_manifest: std::collections::BTreeMap<&str, &packages::ImportOption> = preview
        .options
        .iter()
        .map(|option| (option.manifest.as_str(), option))
        .collect();
    assert_eq!(by_manifest.len(), 2);
    assert_eq!(
        by_manifest["plugin.json"].agent,
        noending_lib::domain::Agent::Antigravity
    );
    assert_eq!(by_manifest["plugin.json"].name, "a-plugin");
    assert_eq!(
        by_manifest["package.json"].agent,
        noending_lib::domain::Agent::Pi
    );
    assert_eq!(by_manifest["package.json"].name, "a-package");

    // An unsupported manifest next to an installable one is inert: only the
    // installable format is offered, with its own metadata.
    let npm_of_claude = f.dir("external/npm-of-claude");
    f.write(
        "external/npm-of-claude/.claude-plugin/plugin.json",
        r#"{"name": "claude-side"}"#,
    );
    f.write(
        "external/npm-of-claude/package.json",
        r#"{"name": "pi-side"}"#,
    );
    f.dir("external/npm-of-claude/skills");
    let preview = packages::preview_import(npm_of_claude.to_str().unwrap()).unwrap();
    assert_eq!(preview.options.len(), 1);
    assert_eq!(preview.options[0].agent, noending_lib::domain::Agent::Pi);
    assert_eq!(preview.options[0].manifest, "package.json");
    assert_eq!(preview.options[0].name, "pi-side");

    // A manifest nested deeper than the known formats is not a package.
    let nested = f.dir("external/nested");
    f.write("external/nested/inner/plugin.json", r#"{"name": "nested"}"#);
    assert!(packages::preview_import(nested.to_str().unwrap()).is_err());

    assert!(packages::preview_import("/nonexistent/path").is_err());
    let empty = f.dir("external/not-a-package");
    assert!(packages::preview_import(empty.to_str().unwrap()).is_err());
}

#[test]
fn package_json_options_require_real_pi_or_dsh_evidence() {
    let f = Fixture::new();
    // Pi evidence = the `pi` key or a conventional resource directory.
    let plain = f.dir("external/pi-only");
    f.write("external/pi-only/package.json", r#"{"name": "pi-only"}"#);
    f.dir("external/pi-only/skills/greet");
    let preview = packages::preview_import(plain.to_str().unwrap()).unwrap();
    assert_eq!(preview.options.len(), 1);
    assert_eq!(preview.options[0].agent, noending_lib::domain::Agent::Pi);

    // The official DSH marker (dsh-app-boot: dsh.bundle) WITHOUT Pi
    // resources yields the DSH option alone — installing a bundle into Pi
    // would only create an empty entry.
    let bundle = f.dir("external/dsh-bundle");
    f.write(
        "external/dsh-bundle/package.json",
        r#"{"name": "dsh-bundle", "dsh": {"bundle": {"patch": "./cordis.patch.yml"}}}"#,
    );
    let preview = packages::preview_import(bundle.to_str().unwrap()).unwrap();
    assert_eq!(preview.options.len(), 1);
    assert_eq!(preview.options[0].agent, noending_lib::domain::Agent::Dsh);

    // A genuinely dual-purpose package offers both.
    f.dir("external/dsh-bundle/skills");
    let preview = packages::preview_import(bundle.to_str().unwrap()).unwrap();
    let agents: Vec<noending_lib::domain::Agent> =
        preview.options.iter().map(|o| o.agent).collect();
    assert!(agents.contains(&noending_lib::domain::Agent::Pi));
    assert!(agents.contains(&noending_lib::domain::Agent::Dsh));

    // A dsh key that is NOT a bundle marker (profile tooling) does not
    // count; a package.json with neither evidence is not installable.
    let fake = f.dir("external/fake-dsh");
    f.write(
        "external/fake-dsh/package.json",
        r#"{"name": "fake", "dsh": {"profile": {}}}"#,
    );
    f.dir("external/fake-dsh/skills");
    let preview = packages::preview_import(fake.to_str().unwrap()).unwrap();
    assert!(preview
        .options
        .iter()
        .all(|o| o.agent != noending_lib::domain::Agent::Dsh));
    let bare = f.dir("external/bare-npm");
    f.write("external/bare-npm/package.json", r#"{"name": "bare"}"#);
    let message = format!(
        "{}",
        packages::preview_import(bare.to_str().unwrap()).unwrap_err()
    );
    assert!(message.contains("Pi 资源"), "{message}");
    assert!(message.contains("dsh.bundle"), "{message}");
}

#[test]
fn dsh_install_and_uninstall_pin_the_desktop_plugin_command() {
    use noending_lib::domain::Agent;
    use noending_lib::packages::{cli_args, PackageAction};

    assert_eq!(
        cli_args(Agent::Dsh, PackageAction::Install, "/external/bundle").unwrap(),
        [
            "plugin",
            "--profile",
            "desktop",
            "add",
            "link:/external/bundle"
        ]
    );
    assert_eq!(
        cli_args(
            Agent::Dsh,
            PackageAction::Uninstall,
            "@deepseek-ai/dsh-base"
        )
        .unwrap(),
        [
            "plugin",
            "--profile",
            "desktop",
            "remove",
            "@deepseek-ai/dsh-base"
        ]
    );
    // The install guard accepts the DSH option the marker detected; running
    // it in a test would launch the real CLI, so only the guard-rejection
    // side is exercised (Pi-only directory refuses DSH).
    let f = Fixture::new();
    let plain = f.dir("external/pi-only");
    f.write("external/pi-only/package.json", r#"{"name": "pi-only"}"#);
    assert!(packages::install_local(plain.to_str().unwrap(), &[Agent::Dsh]).is_err());
}

#[test]
fn import_preview_enumerates_local_marketplace_plugins() {
    let f = Fixture::new();
    let market = f.dir("external/my-claude-market");
    f.write(
        "external/my-claude-market/.claude-plugin/marketplace.json",
        r#"{"name": "my-claude-market", "description": "Local collection",
            "plugins": [
                {"name": "review", "description": "Review helpers"},
                {"name": "docs", "description": "Doc tools"}
            ]}"#,
    );
    let preview = packages::preview_import(market.to_str().unwrap()).unwrap();
    assert!(matches!(preview.kind, packages::ImportKind::Marketplace));
    assert_eq!(preview.options.len(), 1);
    let option = &preview.options[0];
    assert_eq!(option.agent, noending_lib::domain::Agent::ClaudeCode);
    assert_eq!(option.name, "my-claude-market");
    assert_eq!(option.manifest, ".claude-plugin/marketplace.json");
    let names: Vec<&str> = option.plugins.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["review", "docs"]);
    assert_eq!(option.plugins[0].description, "Review helpers");

    // A codex marketplace lives at .agents/plugins/marketplace.json; a
    // directory carrying both formats offers both Agents.
    let dual = f.dir("external/dual-market");
    f.write(
        "external/dual-market/.claude-plugin/marketplace.json",
        r#"{"name": "dual-market", "plugins": [{"name": "a"}]}"#,
    );
    f.write(
        "external/dual-market/.agents/plugins/marketplace.json",
        r#"{"name": "dual-market", "plugins": [{"name": "b"}]}"#,
    );
    let preview = packages::preview_import(dual.to_str().unwrap()).unwrap();
    assert_eq!(preview.options.len(), 2);

    // A marketplace root wins over stray single-package manifests.
    let market_with_stray = f.dir("external/market-with-stray");
    f.write(
        "external/market-with-stray/.claude-plugin/marketplace.json",
        r#"{"name": "m", "plugins": [{"name": "x"}]}"#,
    );
    f.write(
        "external/market-with-stray/plugin.json",
        r#"{"name": "stray"}"#,
    );
    let preview = packages::preview_import(market_with_stray.to_str().unwrap()).unwrap();
    assert!(matches!(preview.kind, packages::ImportKind::Marketplace));
    assert!(preview
        .options
        .iter()
        .all(|option| option.agent != noending_lib::domain::Agent::Antigravity));
}

#[test]
fn marketplace_commands_and_selection_validation_are_pure() {
    use noending_lib::packages::{marketplace_add_args, marketplace_install_args};

    // Args pinned without launching anything.
    assert_eq!(
        marketplace_add_args("/external/my-market"),
        ["plugin", "marketplace", "add", "/external/my-market"]
    );
    assert_eq!(
        marketplace_install_args(
            noending_lib::domain::Agent::ClaudeCode,
            "review",
            "/external/my-market",
            "my-market"
        )
        .unwrap(),
        [
            "plugin",
            "install",
            "review",
            "--marketplace",
            "/external/my-market",
            "--json"
        ]
    );
    assert_eq!(
        marketplace_install_args(
            noending_lib::domain::Agent::Codex,
            "review",
            "/external/my-market",
            "my-market"
        )
        .unwrap(),
        ["plugin", "add", "review@my-market"]
    );
    assert!(marketplace_install_args(noending_lib::domain::Agent::Pi, "x", "/p", "m").is_err());
    assert!(marketplace_install_args(noending_lib::domain::Agent::Codex, "-x", "/p", "m").is_err());

    // Selection validation refuses unknown plugins, empty selections and
    // agents whose marketplace format is absent — all before any CLI runs.
    let f = Fixture::new();
    let market = f.dir("external/my-claude-market");
    f.write(
        "external/my-claude-market/.claude-plugin/marketplace.json",
        r#"{"name": "my-claude-market", "plugins": [{"name": "review"}]}"#,
    );
    let unknown = packages::install_local_marketplace(
        market.to_str().unwrap(),
        &[(
            noending_lib::domain::Agent::ClaudeCode,
            vec!["ghost".into()],
        )],
    );
    assert!(unknown.is_err());
    assert!(format!("{}", unknown.unwrap_err()).contains("ghost"));
    assert!(packages::install_local_marketplace(market.to_str().unwrap(), &[]).is_err());
    assert!(packages::install_local_marketplace(
        market.to_str().unwrap(),
        &[(noending_lib::domain::Agent::ClaudeCode, vec![])]
    )
    .is_err());
    assert!(packages::install_local_marketplace(
        market.to_str().unwrap(),
        &[(noending_lib::domain::Agent::Codex, vec!["review".into()])]
    )
    .is_err());
    // A manifest without a name cannot be registered.
    let anon = f.dir("external/anon-market");
    f.write(
        "external/anon-market/.claude-plugin/marketplace.json",
        r#"{"plugins": [{"name": "x"}]}"#,
    );
    assert!(packages::install_local_marketplace(
        anon.to_str().unwrap(),
        &[(noending_lib::domain::Agent::ClaudeCode, vec!["x".into()])]
    )
    .is_err());
}

#[test]
fn qoder_registry_discovery_is_readonly_and_marks_the_bundler_market_bundled() {
    let f = Fixture::new();
    f.write(
        "home/.qoder-cn/plugins/installed_plugins_v2.json",
        r#"{"version": 2, "plugins": {
            "qoder-context@qoderapp-bundler": [{"scope": "user", "installPath": "Q1", "version": "1.0.84",
                "displayName": "Qoder Context", "userVisible": false}]}}"#,
    );
    let inventory = f.discover();
    let context = entry(&inventory, "qoder", "qoder-context");
    assert_eq!(context.origin, Origin::Bundled);
    assert_eq!(context.description, "Qoder Context");
    assert_eq!(context.version, "1.0.84");
    assert_eq!(
        context.capabilities,
        Capabilities {
            uninstall: false,
            toggle: false
        }
    );
}

#[test]
fn zcode_runtime_data_dirs_are_the_install_evidence() {
    let f = Fixture::new();
    f.dir("home/.zcode/cli/plugins/data/documents@zcode-plugins-official");
    f.dir("home/.zcode/cli/plugins/cache/zcode-plugins-official/documents/0.5.1");
    // Cache without a data dir is not an install.
    f.dir("home/.zcode/cli/plugins/cache/zcode-plugins-official/ghost/0.1.0");
    let inventory = f.discover();
    let documents = entry(&inventory, "zcode", "documents");
    assert_eq!(documents.origin, Origin::Market);
    assert_eq!(
        documents.marketplace.as_deref(),
        Some("zcode-plugins-official")
    );
    assert!(documents
        .install_path
        .as_deref()
        .unwrap()
        .ends_with("0.5.1"));
    assert_eq!(
        documents.capabilities,
        Capabilities {
            uninstall: false,
            toggle: false
        }
    );
    assert!(inventory.packages.iter().all(|entry| entry.name != "ghost"));
    // No client config yet: every plugin ships default-enabled, and the
    // config file is reported as not yet created.
    assert_eq!(documents.scopes[0].state, packages::ToggleState::Enabled);
    assert!(!documents.scopes[0].config_exists);
}

#[test]
fn zcode_enablement_follows_the_client_config_plugins_section() {
    let f = Fixture::new();
    f.dir("home/.zcode/cli/plugins/data/documents@zcode-plugins-official");
    f.dir("home/.zcode/cli/plugins/data/browser@zcode-plugins-official");
    f.dir("home/.zcode/cli/plugins/data/pdf@zcode-plugins-official");
    // Explicit entries win; absent entries stay default-enabled; the master
    // switch (when false) disables everything; suppressedBuiltins hides a
    // built-in from discovery entirely. Schema per the client's own reader.
    f.write(
        "home/.zcode/cli/config.json",
        r#"{"plugins": {
            "enabledPlugins": {
                "documents@zcode-plugins-official": false,
                "browser@zcode-plugins-official": true
            },
            "suppressedBuiltins": ["pdf@zcode-plugins-official"]
        }}"#,
    );
    let inventory = f.discover();
    assert_eq!(
        entry(&inventory, "zcode", "documents").scopes[0].state,
        packages::ToggleState::Disabled
    );
    assert_eq!(
        entry(&inventory, "zcode", "browser").scopes[0].state,
        packages::ToggleState::Enabled
    );
    // Suppressed = the client's uninstalled marker: not listed at all.
    assert!(inventory.packages.iter().all(|entry| entry.name != "pdf"));

    // Master switch off: every remaining plugin reports disabled.
    f.write(
        "home/.zcode/cli/config.json",
        r#"{"plugins": {"enabled": false}}"#,
    );
    let inventory = f.discover();
    assert!(inventory
        .packages
        .iter()
        .all(|entry| entry.scopes[0].state == packages::ToggleState::Disabled));
}

#[test]
fn dsh_discovery_reads_only_the_desktop_profile_with_uninstall_capability() {
    let f = Fixture::new();
    // The desktop profile is the only one NoEnding manages; other profiles
    // (web) and the shared node_modules folder are not discovery inputs.
    f.write(
        "home/.dsh/profiles/desktop/package.json",
        r#"{"name": "dsh-profile-desktop", "dsh": {"profile": {"bundles": ["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-web-app"]}}}"#,
    );
    f.write(
        "home/.dsh/profiles/web/package.json",
        r#"{"name": "dsh-profile-web", "dsh": {"profile": {"bundles": ["@deepseek-ai/dsh-web-only"]}}}"#,
    );
    f.write(
        "home/.dsh/profiles/node_modules/@deepseek-ai/dsh-base/package.json",
        r#"{"name": "@deepseek-ai/dsh-base", "version": "1.2.0", "description": "Base bundle"}"#,
    );

    let inventory = f.discover();
    let base = entry(&inventory, "dsh", "@deepseek-ai/dsh-base");
    assert_eq!(base.version, "1.2.0");
    assert_eq!(base.description, "Base bundle");
    assert_eq!(base.origin, packages::Origin::Profile);
    assert_eq!(base.marketplace.as_deref(), Some("desktop"));
    assert!(base.install_path.is_some());
    // Whole-bundle uninstall; no native toggle.
    assert!(base.capabilities.uninstall && !base.capabilities.toggle);
    assert_eq!(base.scopes[0].state, packages::ToggleState::Unknown);
    // web-only bundles and unresolved declarations are simply absent.
    assert!(inventory
        .packages
        .iter()
        .all(|entry| entry.name != "@deepseek-ai/dsh-web-only"));
    assert!(inventory
        .packages
        .iter()
        .all(|entry| entry.name != "@deepseek-ai/dsh-web-app" || entry.install_path.is_none()));
}

#[test]
fn dsh_bundle_declarations_edit_the_desktop_profile_safely() {
    let f = Fixture::new();
    f.write(
        "home/.dsh/profiles/desktop/package.json",
        r#"{"name": "dsh-profile-desktop", "dsh": {"profile": {"bundles": ["@deepseek-ai/dsh-base"]}}}"#,
    );
    let roots = f.roots.clone();
    // Declare is idempotent and appends; undeclare removes and tolerates a
    // missing entry.
    noending_lib::packages::dsh::declare_bundle(&roots, "@deepseek-ai/dsh-pi-agent").unwrap();
    noending_lib::packages::dsh::declare_bundle(&roots, "@deepseek-ai/dsh-pi-agent").unwrap();
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("home/.dsh/profiles/desktop/package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        value["dsh"]["profile"]["bundles"],
        serde_json::json!(["@deepseek-ai/dsh-base", "@deepseek-ai/dsh-pi-agent"])
    );
    noending_lib::packages::dsh::undeclare_bundle(&roots, "@deepseek-ai/dsh-pi-agent").unwrap();
    noending_lib::packages::dsh::undeclare_bundle(&roots, "@deepseek-ai/dsh-ghost").unwrap();
    let value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.join("home/.dsh/profiles/desktop/package.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        value["dsh"]["profile"]["bundles"],
        serde_json::json!(["@deepseek-ai/dsh-base"])
    );
    // A profile without the bundles block refuses declare (never invents
    // structure) and undeclare is a no-op.
    f.write(
        "home/.dsh/profiles/desktop/package.json",
        r#"{"name": "x"}"#,
    );
    assert!(noending_lib::packages::dsh::declare_bundle(&roots, "y").is_err());
    noending_lib::packages::dsh::undeclare_bundle(&roots, "y").unwrap();
}

#[test]
fn serialized_origin_uses_the_snake_case_contract_the_frontend_labels_on() {
    let f = Fixture::new();
    f.dir("home/.gemini/antigravity-cli/plugins/noending-terminal-identity");
    f.write(
        "home/.gemini/antigravity-cli/plugins/noending-terminal-identity/plugin.json",
        r#"{"name": "noending-terminal-identity"}"#,
    );
    let inventory = f.discover();
    let entry = entry(&inventory, "antigravity", "noending-terminal-identity");
    let json = serde_json::to_value(entry).unwrap();
    // The serialization contract the frontend labels key on — snake_case
    // hump-splitting would otherwise yield `no_ending_managed`.
    assert_eq!(json["origin"], "noending_managed");
    // The built-in's absent switch serializes as its default-on state.
    assert_eq!(json["scopes"][0]["state"], "enabled");
}
