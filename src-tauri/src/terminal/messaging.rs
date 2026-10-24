//! Message submission is separate from raw xterm input. Never infer readiness
//! from terminal output. PTY writes confirm transport only, not Agent acceptance.

#[derive(Debug, Clone, Copy, Default)]
pub(super) enum MessageDelivery {
    #[default]
    Queue,
    Steer,
}

/// Native default shortcuts, independent of asynchronous lifecycle snapshots.
/// Antigravity's Enter follows its saved preference; Alt+M selects the opposite.
pub(super) fn submit_key(
    agent: crate::domain::Agent,
    delivery: MessageDelivery,
) -> Result<&'static str, DeliveryError> {
    let settings = if agent == crate::domain::Agent::Antigravity {
        let path = crate::platform::paths::resolve_antigravity_cli_dir()
            .ok_or_else(|| error("agent_settings_unavailable", "无法定位 Antigravity 配置"))?
            .join("settings.json");
        match std::fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice::<serde_json::Value>(&bytes).map_err(
                |_| {
                    error(
                        "agent_settings_unavailable",
                        "无法读取 Antigravity 排队设置",
                    )
                },
            )?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                return Err(error(
                    "agent_settings_unavailable",
                    "无法读取 Antigravity 排队设置",
                ))
            }
        }
    } else {
        None
    };
    native_submit_key(agent, delivery, cfg!(windows), settings.as_ref())
}
fn native_submit_key(
    agent: crate::domain::Agent,
    delivery: MessageDelivery,
    windows: bool,
    settings: Option<&serde_json::Value>,
) -> Result<&'static str, DeliveryError> {
    use crate::domain::Agent;
    let queue = matches!(delivery, MessageDelivery::Queue);
    Ok(match agent {
        Agent::Codex => {
            if queue {
                "\t"
            } else {
                "\r"
            }
        }
        Agent::ClaudeCode => {
            if queue {
                "\x18\r"
            } else {
                "\x18\x13"
            }
        }
        Agent::Pi => {
            if queue {
                if windows {
                    "\x11"
                } else {
                    "\x1b\r"
                }
            } else {
                "\r"
            }
        }
        Agent::Antigravity => {
            let enter_queues = match settings.and_then(|s| s.get("queuedMessages")) {
                None => true,
                Some(serde_json::Value::String(v)) if v == "queue" => true,
                Some(serde_json::Value::String(v)) if v == "send-immediately" => false,
                _ => {
                    return Err(error(
                        "agent_settings_unavailable",
                        "无法识别 Antigravity 排队设置",
                    ))
                }
            };
            if queue == enter_queues {
                "\r"
            } else {
                "\x1bm"
            }
        }
        _ => return Err(error("unsupported_agent", "此 Agent 不支持终端消息投递")),
    })
}

#[derive(Debug)]
pub struct DeliveryError {
    pub code: &'static str,
    pub message: String,
}
pub(super) fn error(code: &'static str, message: impl ToString) -> DeliveryError {
    DeliveryError {
        code,
        message: message.to_string(),
    }
}
/// Replies to terminal protocol queries do not represent text in the editor.
/// Arrow/history keys and bracketed paste deliberately remain user input.
pub(super) fn terminal_reply(data: &[u8]) -> bool {
    if data.starts_with(b"\x1b]") && (data.ends_with(b"\x07") || data.ends_with(b"\x1b\\")) {
        return true;
    }
    let Some(body) = data.strip_prefix(b"\x1b[") else {
        return false;
    };
    let Some((&last, parameters)) = body.split_last() else {
        return false;
    };
    matches!(last, b'c' | b'n' | b'R' | b't' | b'I' | b'O')
        && parameters.iter().all(|byte| (0x20..=0x3f).contains(byte))
}

/// Keep multi-key chords in separate terminal input events (Ink otherwise
/// treats a coalesced chord/paste as text). This is key framing, not a turn wait.
pub(super) fn write_submit(
    writer: &mut (impl std::io::Write + ?Sized),
    key: &str,
) -> std::io::Result<()> {
    if let Some(rest) = key.strip_prefix('\x18') {
        writer.write_all(b"\x18")?;
        writer.flush()?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        writer.write_all(rest.as_bytes())?;
    } else {
        writer.write_all(key.as_bytes())?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Agent::*;
    #[test]
    fn native_queue_and_steer_shortcuts_follow_platform_and_antigravity_preference() {
        use MessageDelivery::*;
        for (agent, queue, steer) in [
            (Codex, "\t", "\r"),
            (ClaudeCode, "\x18\r", "\x18\x13"),
            (Pi, "\x1b\r", "\r"),
            (Antigravity, "\r", "\x1bm"),
        ] {
            assert_eq!(native_submit_key(agent, Queue, false, None).unwrap(), queue);
            assert_eq!(native_submit_key(agent, Steer, false, None).unwrap(), steer);
        }
        assert_eq!(native_submit_key(Pi, Queue, true, None).unwrap(), "\x11");
        let immediate = serde_json::json!({"queuedMessages":"send-immediately"});
        assert_eq!(
            native_submit_key(Antigravity, Queue, true, Some(&immediate)).unwrap(),
            "\x1bm"
        );
        assert_eq!(
            native_submit_key(Antigravity, Steer, true, Some(&immediate)).unwrap(),
            "\r"
        );
        assert!(native_submit_key(
            Antigravity,
            Queue,
            false,
            Some(&serde_json::json!({"queuedMessages":"invalid"}))
        )
        .is_err());
    }
}
