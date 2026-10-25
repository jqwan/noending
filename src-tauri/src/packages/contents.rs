//! Package-root scanning for the detail view. Only an overview is reported:
//! what the package contains, not projections into other features.

use super::{files, PackageContents};
use serde_json::Value;
use std::{fs, path::Path};

pub(super) fn scan(root: &Path, warnings: &mut Vec<String>) -> PackageContents {
    let mut contents = PackageContents::default();
    for skill in skill_dirs(root) {
        contents.skills.push(skill);
    }
    for manifest in [
        "plugin.json",
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
        "package.json",
    ] {
        let path = root.join(manifest);
        if path.is_file() {
            if let Some(value) = files::json(&path, warnings) {
                append_declared_skills(&value, root, &mut contents.skills);
            }
            break;
        }
    }
    contents.skills.sort();
    contents.skills.dedup();
    for file in [".mcp.json", "mcp.json", "mcp_config.json"] {
        if root.join(&file).is_file() {
            contents.mcp_files.push(file.to_string());
        }
    }
    contents
}

fn skill_dirs(root: &Path) -> Vec<String> {
    let base = root.join("skills");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().join("SKILL.md").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn append_declared_skills(manifest: &Value, root: &Path, skills: &mut Vec<String>) {
    let declared = manifest.get("pi").unwrap_or(manifest).get("skills");
    let paths: Vec<String> = match declared {
        Some(Value::String(path)) => vec![path.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    for relative in paths {
        let path = root.join(&relative);
        // Only declare what actually resolves inside the package root; a
        // manifest can point anywhere and unresolvable entries are noise.
        let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if fs::canonicalize(&path)
            .ok()
            .is_some_and(|resolved| resolved.starts_with(&canonical_root))
            && path.join("SKILL.md").is_file()
        {
            let name = relative.trim_end_matches('/');
            skills.push(name.rsplit('/').next().unwrap_or(name).to_string());
        }
    }
}
