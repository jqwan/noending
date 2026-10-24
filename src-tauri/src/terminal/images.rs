//! Clipboard images become private files whose paths can be pasted as ordinary
//! text. Product-owned images persist with the receiving Session mailbox.
use super::{
    messaging::{error, DeliveryError},
    TerminalRegistry,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
#[cfg(test)]
use std::fs;

const MAX_BYTES: usize = 20 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardImage {
    pub mime: String,
    pub data_base64: String,
}
fn cache_image_file(
    image: ClipboardImage,
) -> Result<crate::platform::runtime_file::RuntimeFile, DeliveryError> {
    if image.data_base64.len() > MAX_BYTES * 4 / 3 + 4 {
        return Err(error("invalid_image", "图片最大支持 20 MiB"));
    }
    let bytes = STANDARD
        .decode(image.data_base64)
        .map_err(|_| error("invalid_image", "图片数据无效"))?;
    if bytes.len() > MAX_BYTES {
        return Err(error("invalid_image", "图片最大支持 20 MiB"));
    }
    let extension = match image.mime.as_str() {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        "image/jpeg" if bytes.starts_with(b"\xff\xd8\xff") => "jpg",
        "image/webp" if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") => "webp",
        _ => return Err(error("invalid_image", "仅支持 PNG、JPEG 和 WebP 图片")),
    };
    crate::platform::runtime_file::RuntimeFile::create(&bytes, extension)
        .map_err(|e| error("image_cache_failed", e))
}
impl TerminalRegistry {
    pub fn cache_image(
        &self,
        terminal: &str,
        image: ClipboardImage,
    ) -> Result<String, DeliveryError> {
        let record = self
            .records
            .lock()
            .map_err(|_| error("internal_error", "终端不可用"))?
            .get(terminal)
            .cloned()
            .ok_or_else(|| error("terminal_not_found", "终端不存在"))?;
        // Do not block PTY output while saving an image.
        let cache = cache_image_file(image)?;
        let session = record
            .lock()
            .map_err(|_| error("internal_error", "终端不可用"))?
            .summary
            .session_id
            .clone()
            .ok_or_else(|| error("unbound_session", "终端尚未绑定会话"))?;
        let app = self
            .app
            .as_ref()
            .ok_or_else(|| error("image_cache_failed", "需要桌面信箱资源目录"))?;
        let home = crate::commands::noending_home(app)
            .ok_or_else(|| error("image_cache_failed", "无法定位 Home"))?;
        persist(&home, &session, cache)
    }
}

fn persist(
    home: &crate::workspace::home::NoEndingHome,
    session: &str,
    cache: crate::platform::runtime_file::RuntimeFile,
) -> Result<String, DeliveryError> {
    if std::path::Path::new(session).components().count() != 1 || session == "." || session == ".."
    {
        return Err(error("invalid_session", "会话标识无效"));
    }
    let root = home.root.join("mailbox-images").join(session);
    std::fs::create_dir_all(&root).map_err(|e| error("image_cache_failed", e))?;
    let destination = root.join(format!(
        "{}.{}",
        crate::storage::new_id(),
        cache.path().extension().unwrap().to_string_lossy()
    ));
    std::fs::copy(cache.path(), &destination).map_err(|e| error("image_cache_failed", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| error("image_cache_failed", e))?;
    }
    let path = destination.to_string_lossy().into_owned();
    Ok(path)
}

/// Inspect only files created in our own mailbox directory, never paths from arbitrary text.
pub(crate) fn cleanup(
    home: &crate::workspace::home::NoEndingHome,
    db: &crate::storage::Db,
    session: &str,
    old_body: Option<&str>,
) {
    if std::path::Path::new(session).components().count() != 1 || session == "." || session == ".."
    {
        return;
    }
    let root = home.root.join("mailbox-images").join(session);
    if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let name = path.to_string_lossy();
        if old_body.is_none_or(|body| body.contains(name.as_ref()))
            && !db.mailbox_references_path(&name).unwrap_or(true)
        {
            let _ = std::fs::remove_file(path);
        }
    }
    let _ = std::fs::remove_dir(root);
}

#[cfg(test)]
pub(super) const TEST_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jD1sAAAAASUVORK5CYII=";
#[cfg(test)]
mod tests {
    use super::*;
    fn png() -> ClipboardImage {
        ClipboardImage {
            mime: "image/png".into(),
            data_base64: TEST_PNG.into(),
        }
    }
    #[test]
    fn cached_image_has_original_bytes_and_is_deleted_when_released() {
        let cache = cache_image_file(png()).unwrap();
        let path = cache.path().to_owned();
        assert_eq!(
            fs::read(&path).unwrap(),
            STANDARD.decode(png().data_base64).unwrap()
        );
        assert_eq!(path.extension().unwrap(), "png");
        drop(cache);
        assert!(!path.exists());
    }

    #[test]
    fn persisted_images_outlive_runtime_handles_and_cleanup_never_deletes_external_files() {
        let root =
            std::env::temp_dir().join(format!("ne-mailbox-images-{}", crate::storage::new_id()));
        let home = crate::workspace::home::NoEndingHome::new(root.to_str().unwrap(), None).unwrap();
        home.ensure_dirs().unwrap();
        let db = crate::storage::Db::open(&home.db_path).unwrap();
        db.write().execute_batch("INSERT INTO sessions(id,agent,root_agent_session_id,source_kind,source_path) VALUES('s','codex','s','jsonl','/s')").unwrap();
        let path = persist(&home, "s", cache_image_file(png()).unwrap()).unwrap();
        assert!(std::path::Path::new(&path).is_file());
        let external = root.join("user.png");
        std::fs::write(&external, b"user-owned").unwrap();
        let body = format!("{}\n{}", path, external.display());
        let mail = db.mailbox_send("s", None, &body).unwrap();
        cleanup(&home, &db, "s", Some(&body));
        assert!(std::path::Path::new(&path).is_file());
        db.mailbox_delete("s", &mail.id, mail.revision).unwrap();
        cleanup(&home, &db, "s", Some(&body));
        assert!(!std::path::Path::new(&path).exists());
        assert_eq!(std::fs::read(&external).unwrap(), b"user-owned");
        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn image_validation_rejects_invalid_data_and_mime_mismatches() {
        assert!(cache_image_file(png()).is_ok());
        let mut image = png();
        image.mime = "image/jpeg".into();
        assert!(cache_image_file(image).is_err());
        let mut image = png();
        image.data_base64 = STANDARD.encode(b"<svg onload='script'>");
        assert!(cache_image_file(image).is_err());
        let mut image = png();
        image.data_base64 = "not base64".into();
        assert!(cache_image_file(image).is_err());
    }
}
