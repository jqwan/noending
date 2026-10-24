//! UserPromptSubmit precedes the persisted user turn. Follow new native user
//! records to schedule history ingestion, excluding queue metadata and tools.
//! The hook must return before Claude can write that turn.
use super::*;
use std::io::{Seek, SeekFrom};

#[derive(Default)]
pub(super) struct InputTracker {
    stream: Option<Transcript>,
}
struct Transcript {
    path: PathBuf,
    active_record: PathBuf,
    event: NativeIdentityEvent,
    offset: u64,
    line: Vec<u8>,
    oversized: bool,
    pending: VecDeque<String>,
}
impl InputTracker {
    pub(super) fn arm(&mut self, event: &NativeIdentityEvent, active: &Path) {
        let (Some(input), Some(path)) = (&event.input, &event.source_path) else {
            return;
        };
        let path = PathBuf::from(path);
        if self.stream.as_ref().is_none_or(|s| {
            s.path != path || s.event.native_id != event.native_id || s.pending.is_empty()
        }) {
            self.stream = Some(Transcript {
                offset: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                path,
                active_record: active.to_owned(),
                event: event.clone(),
                line: Vec::new(),
                oversized: false,
                pending: VecDeque::new(),
            });
        }
        let stream = self.stream.as_mut().unwrap();
        if stream.pending.len() < MAX_PENDING_EVENTS {
            stream
                .pending
                .push_back(input.id.clone().unwrap_or_default());
        }
    }
    pub(super) fn poll(&mut self, callback: &IdentityCallback) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        if stream.pending.is_empty() {
            return;
        }
        // A switched/closed native session cannot confirm an older input.
        let Some(current) = active_record_event(&stream.active_record, &stream.event) else {
            // The PID record is written asynchronously; a transient missing or
            // partial read must not lose the outstanding prompt.
            return;
        };
        if current.native_id != stream.event.native_id {
            self.stream = None;
            return;
        }
        let Ok(mut file) = std::fs::File::open(&stream.path) else {
            return;
        };
        if file.metadata().is_ok_and(|m| m.len() < stream.offset) {
            self.stream = None;
            return;
        }
        if file.seek(SeekFrom::Start(stream.offset)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file.take(64 * 1024).read_to_end(&mut bytes).is_err() {
            return;
        }
        stream.offset += bytes.len() as u64;
        for byte in bytes {
            if byte != b'\n' {
                if !stream.oversized {
                    if stream.line.len() < 32 * 1024 * 1024 {
                        stream.line.push(byte);
                    } else {
                        stream.line.clear();
                        stream.oversized = true;
                    }
                }
                continue;
            }
            if !stream.oversized {
                if let Some(input_event) = user_input(&stream.line, &stream.event.native_id) {
                    if !stream.pending.is_empty() {
                        stream.pending.pop_front();
                        let mut event = stream.event.clone();
                        event.source = Some("user_message".into());
                        event.input = Some(input_event);
                        event.activity = Some(ActivitySignal {
                            state: ActivityState::Working,
                            turn_id: None,
                            starts_turn: true,
                        });
                        callback(event);
                    }
                }
            }
            stream.line.clear();
            stream.oversized = false;
        }
    }
}
fn user_input(line: &[u8], native_id: &str) -> Option<NativeInput> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if value["type"] != "user"
        || value["message"]["role"] != "user"
        || value["isMeta"] == true
        || value["isSidechain"] == true
        || value["sessionId"]
            .as_str()
            .is_some_and(|id| id != native_id)
    {
        return None;
    }
    let content = &value["message"]["content"];
    let text = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        content
            .as_array()?
            .iter()
            .filter(|part| part["type"] == "text")
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("")
    };
    if text.trim().is_empty() {
        return None;
    }
    Some(NativeInput {
        id: Some(value["uuid"].as_str()?.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_prompts_wait_for_new_complete_native_user_records_and_stop_on_identity_switch() {
        let root = std::env::temp_dir().join(crate::storage::new_id());
        create_private_dir(&root).unwrap();
        let path = root.join("native.jsonl");
        let active = root.join("123.json");
        let record = |id: &str| {
            serde_json::to_vec(&serde_json::json!({"pid":123,"kind":"interactive","sessionId":id}))
                .unwrap()
        };
        std::fs::write(&active, record("native")).unwrap();
        let row = |kind: &str, id: &str| {
            format!(
                "{}\n",
                serde_json::json!({"type":kind,"uuid":id,"sessionId":"native","message":{"role":"user","content":"same input"}})
            )
        };
        std::fs::write(&path, row("user", "old")).unwrap();
        let event = NativeIdentityEvent {
            native_id: "native".into(),
            source_path: Some(path.to_string_lossy().into()),
            cwd: None,
            source: None,
            activity: None,
            input: Some(NativeInput { id: None }),
        };
        let mut tracker = InputTracker::default();
        tracker.arm(&event, &active);
        let (tx, rx) = std::sync::mpsc::channel();
        let callback: IdentityCallback = Arc::new(move |event| {
            tx.send(event).unwrap();
        });
        tracker.poll(&callback);
        assert!(
            rx.try_recv().is_err(),
            "old identical prompts cannot confirm this write"
        );
        let append = |bytes: &[u8]| {
            std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(bytes)
                .unwrap()
        };
        append(row("queue-operation", "queued").as_bytes());
        let user = row("user", "new");
        append(&user.as_bytes()[..user.len() - 1]);
        tracker.poll(&callback);
        assert!(
            rx.try_recv().is_err(),
            "queued and partial records cannot confirm delivery"
        );
        append(b"\n");
        std::fs::write(&active, b"{").unwrap();
        tracker.poll(&callback);
        assert!(rx.try_recv().is_err());
        std::fs::write(&active, record("native")).unwrap();
        tracker.poll(&callback);
        let input_event = rx.try_recv().unwrap();
        assert_eq!(input_event.input.unwrap().id.as_deref(), Some("new"));
        assert_eq!(input_event.source.as_deref(), Some("user_message"));
        tracker.arm(&event, &active);
        std::fs::write(&active, record("switched")).unwrap();
        append(row("user", "after-switch").as_bytes());
        tracker.poll(&callback);
        assert!(rx.try_recv().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
