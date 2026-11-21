//! AGENTS.md is the source of the content shown in the project.
use super::ProjectAssistant;
use crate::error::{other, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

fn read_instructions(root: &Path) -> Result<Option<(String, String)>> {
    let path = root.join("AGENTS.md");
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() => return Err(other("智能体档案目录不可用")),
        Ok(_) => {}
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_file() => return Err(other("AGENTS.md 必须是普通文件")),
        Ok(_) => {}
    }
    let instructions = fs::read_to_string(&path)?;
    let revision = format!("{:x}", Sha256::digest(instructions.as_bytes()));
    Ok(Some((instructions, revision)))
}

pub fn inspect(root: &Path) -> ProjectAssistant {
    match read_instructions(root) {
        Ok(Some((instructions, revision))) => ProjectAssistant {
            agents_md: instructions,
            revision: Some(revision),
            agents_md_state: "current",
        },
        result => ProjectAssistant {
            agents_md: String::new(),
            revision: None,
            agents_md_state: if result.is_err() {
                "unavailable"
            } else {
                "missing"
            },
        },
    }
}

pub fn initialize(root: &Path, instructions: &str) -> Result<()> {
    read_instructions(root)?;
    for (name, content) in [("AGENTS.md", instructions), ("CLAUDE.md", "@AGENTS.md\n")] {
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

pub fn save(root: &Path, revision: &str, instructions: &str) -> Result<()> {
    let path = root.join("AGENTS.md");
    let temporary = root.join(format!(".AGENTS.md-{}.tmp", crate::storage::new_id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(instructions.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if !read_instructions(root)?.is_some_and(|(_, current)| current == revision) {
            return Err(other("AGENTS.md 已变化，请重新读取"));
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
