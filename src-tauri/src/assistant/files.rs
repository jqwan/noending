//! Generated entry points are replaceable only while their previous bytes match
//! the ownership manifest. User notes and edits are never silently overwritten.
use super::AssistantProfile;
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};
const MANIFEST: &str = ".noending-assistant-files.json";
#[derive(Serialize, Deserialize)]
struct Manifest {
    assistant_id: String,
    hashes: BTreeMap<String, String>,
    #[serde(default)]
    pending: BTreeMap<String, String>,
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn entries(p: &AssistantProfile) -> BTreeMap<String, String> {
    let agents=format!("<!-- NoEnding managed role: {} -->\n# {}\n\n{}\n\n{}\n\n## NoEnding\n\n此目录是智能体的主目录。智能体 ID：{}。使用 noending 技能查询明确负责的项目和任务；实际工程执行使用工程项目的普通会话。会话发信身份仍通过 session identity 确认，智能体 ID 不能作为会话 ID。普通闲聊不自动创建任务或派工。\n",p.id,p.name,p.description,p.instructions,p.id);
    BTreeMap::from([
        ("AGENTS.md".into(), agents),
        (
            "CLAUDE.md".into(),
            format!("<!-- NoEnding managed role: {} -->\n@AGENTS.md\n", p.id),
        ),
    ])
}
fn manifest(root: &Path, p: &AssistantProfile) -> Result<Option<Manifest>> {
    let file = root.join(MANIFEST);
    let metadata = match fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file() {
        return Err(other("智能体生成入口清单不能是符号链接"));
    }
    let m: Manifest = serde_json::from_slice(&fs::read(file)?)?;
    if m.assistant_id != p.id {
        return Err(other("智能体目录已由其他档案拥有"));
    }
    Ok(Some(m))
}
pub fn state(root: &Path, p: &AssistantProfile) -> Result<String> {
    if !root.is_dir() {
        return Ok("missing".into());
    }
    let m = match manifest(root, p) {
        Ok(v) => v,
        Err(_) => return Ok("conflict".into()),
    };
    let mut status = "current";
    for (name, content) in entries(p) {
        let file = root.join(&name);
        match fs::symlink_metadata(&file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if status == "current" {
                    status = "missing";
                }
            }
            Err(e) => return Err(e.into()),
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    return Ok("conflict".into());
                }
                let hash = digest(&fs::read(file)?);
                let owned = m.as_ref().is_some_and(|m| {
                    m.hashes.get(&name) == Some(&hash) || m.pending.get(&name) == Some(&hash)
                });
                if !owned {
                    return Ok("conflict".into());
                }
                if hash != digest(content.as_bytes()) && status == "current" {
                    status = "outdated";
                }
            }
        }
    }
    Ok(status.into())
}
fn replace(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let staging = root.join(format!(".{}-{}.tmp", name, crate::storage::new_id()));
    fs::write(&staging, bytes)?;
    let result = fs::rename(&staging, root.join(name));
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result?;
    Ok(())
}
pub fn prepare(root: &Path, p: &AssistantProfile) -> Result<()> {
    prepare_with(root, p, &mut replace)
}
fn prepare_with(
    root: &Path,
    p: &AssistantProfile,
    write: &mut impl FnMut(&Path, &str, &[u8]) -> Result<()>,
) -> Result<()> {
    if !root.is_dir() {
        return Err(other("智能体主目录不存在，请先恢复目录"));
    }
    if state(root, p)? == "conflict" {
        return Err(other("智能体指令入口存在用户修改或文件冲突，已保留原内容"));
    }
    let content = entries(p);
    let mut journal = manifest(root, p)?.unwrap_or_else(|| Manifest {
        assistant_id: p.id.clone(),
        hashes: BTreeMap::new(),
        pending: BTreeMap::new(),
    });
    for (name, text) in content {
        let hash = digest(text.as_bytes());
        // Persist both the old committed hash and the next candidate before
        // replacing an entry. Recovery recognizes either side of the rename.
        journal.pending.insert(name.clone(), hash.clone());
        write(root, MANIFEST, &serde_json::to_vec_pretty(&journal)?)?;
        write(root, &name, text.as_bytes())?;
        journal.hashes.insert(name.clone(), hash);
        journal.pending.remove(&name);
        write(root, MANIFEST, &serde_json::to_vec_pretty(&journal)?)?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_entry_publication_recovers_without_overwriting_user_edits() {
        let root =
            std::env::temp_dir().join(format!("ne-role-journal-{}", crate::storage::new_id()));
        fs::create_dir(&root).unwrap();
        let mut profile = AssistantProfile {
            id: "profile".into(),
            name: "Old".into(),
            avatar: String::new(),
            description: String::new(),
            instructions: "Read the assigned task".into(),
            agent: crate::domain::Agent::Codex,
            runtime_override: None,
            workspace_path_id: String::new(),
            visibility: "normal".into(),
            revision: 1,
            created_at: String::new(),
            updated_at: String::new(),
        };
        prepare(&root, &profile).unwrap();
        profile.name = "New".into();
        let mut target_written = false;
        let result = prepare_with(&root, &profile, &mut |root, name, bytes| {
            if name == MANIFEST && target_written {
                return Err(other("simulated interrupted manifest commit"));
            }
            replace(root, name, bytes)?;
            if name == "AGENTS.md" {
                target_written = true;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert!(fs::read_to_string(root.join("AGENTS.md"))
            .unwrap()
            .contains("# New"));
        prepare(&root, &profile).unwrap();
        assert_eq!(state(&root, &profile).unwrap(), "current");
        fs::write(root.join("AGENTS.md"), "User-authored content").unwrap();
        assert!(prepare(&root, &profile).is_err());
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "User-authored content"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
