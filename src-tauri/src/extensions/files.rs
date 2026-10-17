use crate::error::{other, Result};
use serde_json::Value;
use std::{fs, path::Path};

pub(super) fn read(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
            Ok(String::new())
        }
        Err(e) => Err(e.into()),
    }
}

pub(super) fn revision(path: &Path) -> Result<String> {
    Ok(super::key(read(path)?))
}

pub(super) fn json(raw: &str) -> Result<Value> {
    let value = if raw.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(raw)?
    };
    if !value.is_object() {
        return Err(other("配置文件顶层必须是对象"));
    }
    Ok(value)
}

/// Serialize only after parsing succeeds. A sibling backup retains the previous
/// bytes; permissions never expose credentials to other local users.
pub(super) fn write(path: &Path, expected: &str, text: &str) -> Result<()> {
    let path = if path.is_symlink() {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    super::check_revision(&revision(&path)?, expected)?;
    let parent = path.parent().ok_or_else(|| other("配置路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".noending-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        super::check_revision(&revision(&path)?, expected)?;
        let backup = parent.join(format!(
            "{}.noending.bak",
            path.file_name().unwrap().to_string_lossy()
        ));
        if path.exists() {
            // Backup is replaced before the original; never truncate the live file.
            let bytes = fs::read(&path)?;
            let mut backup_options = fs::OpenOptions::new();
            backup_options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
                backup_options.mode(0o600);
                if backup.exists() {
                    fs::set_permissions(&backup, fs::Permissions::from_mode(0o600))?;
                }
            }
            let mut backup_file = backup_options.open(&backup)?;
            backup_file.write_all(&bytes)?;
            backup_file.sync_all()?;
        }
        #[cfg(not(windows))]
        fs::rename(&temp, &path)?;
        #[cfg(windows)]
        {
            // std::fs::rename cannot replace an existing file on Windows. Keep a
            // recoverable old path until the new file has taken its place.
            let old = parent.join(format!(".noending-{}.old", uuid::Uuid::new_v4()));
            let existed = path.exists();
            if existed {
                fs::rename(&path, &old)?;
            }
            if let Err(error) = fs::rename(&temp, &path) {
                if existed {
                    let _ = fs::rename(&old, &path);
                }
                return Err(error.into());
            }
            if existed {
                let _ = fs::remove_file(old);
            }
        }
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
