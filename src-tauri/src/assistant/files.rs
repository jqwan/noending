//! The archive files are the source of the content shown in the project.
use super::{AssistantFile, AssistantPatch, ProjectAssistant};
use crate::error::{other, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

const AGENTS: &str = "AGENTS.md";
const SOUL: &str = "SOUL.md";
const ROLE: &str = "ROLE.md";
const STANDARD_ENTRY: &str = include_str!("templates/AGENTS.md");

fn read_document(root: &Path, name: &str) -> Result<Option<(String, String)>> {
    let path = root.join(name);
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() => return Err(other("智能体档案目录不可用")),
        Ok(_) => {}
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_file() => return Err(other(format!("{name} 必须是普通文件"))),
        Ok(_) => {}
    }
    let instructions = fs::read_to_string(&path)?;
    let revision = format!("{:x}", Sha256::digest(instructions.as_bytes()));
    Ok(Some((instructions, revision)))
}

pub fn inspect(root: &Path) -> ProjectAssistant {
    ProjectAssistant {
        guide_state: match read_document(root, AGENTS) {
            Ok(Some((content, _))) if content == STANDARD_ENTRY => "current",
            Ok(Some(_)) => "outdated",
            Ok(None) => "missing",
            Err(_) => "unavailable",
        },
        soul: inspect_document(root, SOUL),
        role: inspect_document(root, ROLE),
    }
}

fn inspect_document(root: &Path, name: &str) -> AssistantFile {
    match read_document(root, name) {
        Ok(Some((content, revision))) => AssistantFile {
            content,
            revision: Some(revision),
            state: "current",
        },
        result => AssistantFile {
            content: String::new(),
            revision: None,
            state: if result.is_err() {
                "unavailable"
            } else {
                "missing"
            },
        },
    }
}

pub fn initialize(root: &Path, soul: Option<&str>, role: Option<&str>) -> Result<()> {
    if let Some((content, revision)) = read_document(root, AGENTS)? {
        if content != STANDARD_ENTRY {
            save_document(root, AGENTS, &revision, STANDARD_ENTRY)?;
        }
    }
    for (name, content) in [
        (AGENTS, STANDARD_ENTRY),
        (SOUL, soul.unwrap_or(include_str!("templates/SOUL.md"))),
        (ROLE, role.unwrap_or_default()),
        ("CLAUDE.md", "@AGENTS.md\n"),
    ] {
        let path = root.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(content.as_bytes()) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(error.into());
                }
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn save(root: &Path, revision: &str, patch: AssistantPatch) -> Result<()> {
    let (name, content) = match patch {
        AssistantPatch::Soul { content } => (SOUL, content),
        AssistantPatch::Role { content } => (ROLE, content),
    };
    save_document(root, name, revision, &content)
}

fn save_document(root: &Path, name: &str, revision: &str, content: &str) -> Result<()> {
    let path = root.join(name);
    let temporary = root.join(format!(".{name}-{}.tmp", crate::storage::new_id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if !read_document(root, name)?.is_some_and(|(_, current)| current == revision) {
            return Err(other(format!("{name} 已变化，请重新读取")));
        }
        fs::set_permissions(&temporary, fs::symlink_metadata(&path)?.permissions())?;
        fs::rename(&temporary, &path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
