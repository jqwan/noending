use crate::error::Result;
use serde_json::Value;
use std::path::Path;

pub(super) fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
            Ok(String::new())
        }
        Err(e) => Err(e.into()),
    }
}

pub(super) fn key(value: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.as_ref()))
}

/// Reads a JSON file, recording a warning instead of failing the whole scan.
pub(super) fn json(path: &Path, warnings: &mut Vec<String>) -> Option<Value> {
    if !path.is_file() {
        return None;
    }
    match read(path).and_then(|raw| serde_json::from_str(&raw).map_err(Into::into)) {
        Ok(value) => Some(value),
        Err(e) => {
            warnings.push(format!("{}：{e}", path.display()));
            None
        }
    }
}

pub(super) fn revision(parts: &[&str]) -> String {
    key(parts.concat())
}

pub(super) fn check_revision(actual: &str, expected: &str) -> Result<()> {
    if actual != expected {
        return Err(crate::error::other("配置已被修改，请刷新后重试"));
    }
    Ok(())
}

pub(super) fn parse_json(raw: &str) -> Result<serde_json::Value> {
    serde_json::from_str(raw).map_err(|e| crate::error::other(format!("配置无法解析：{e}")))
}

/// Same-directory temp file + rename, so a failed write never leaves a
/// half-written config behind.
pub(super) fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::error::other("配置路径无效"))?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".noending-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, content)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e.into())
        }
    }
}
