//! One transport rule for startup and PTY messages, measured in UTF-16 units.
use crate::{
    error::{other, Result},
    platform::runtime_file::RuntimeFile,
};
pub const INLINE_MESSAGE_LIMIT: usize = 2000;
pub struct PreparedMessage {
    pub text: String,
    pub file: Option<RuntimeFile>,
}
pub fn prepare(message: &str) -> Result<PreparedMessage> {
    if message.encode_utf16().count() <= INLINE_MESSAGE_LIMIT {
        return Ok(PreparedMessage {
            text: message.into(),
            file: None,
        });
    }
    let file = RuntimeFile::create(message.as_bytes(), "txt")
        .map_err(|e| other(format!("保存长消息失败：{e}")))?;
    let text = format!(
        "请读取以下 UTF-8 文本文件，将文件全文作为本次用户消息处理：\n{}",
        file.path().display()
    );
    if text.encode_utf16().count() > INLINE_MESSAGE_LIMIT {
        return Err(other("临时文件路径过长，无法传入启动消息"));
    }
    Ok(PreparedMessage {
        text,
        file: Some(file),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_messages_stay_literal_and_long_messages_roundtrip_through_a_private_file() {
        for text in ["中文😀\n$HOME".into(), "a".repeat(2000), "😀".repeat(1000)] {
            let prepared = prepare(&text).unwrap();
            assert_eq!(prepared.text, text);
            assert!(prepared.file.is_none());
        }
        for text in ["a".repeat(2001), "😀".repeat(1001)] {
            let prepared = prepare(&text).unwrap();
            let file = prepared.file.unwrap();
            let path = file.path().to_owned();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert!(prepared.text.contains(path.to_str().unwrap()));
            assert!(prepared.text.encode_utf16().count() <= 2000);
            drop(file);
            assert!(!path.exists());
        }
    }
}
