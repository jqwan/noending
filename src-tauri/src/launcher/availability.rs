//! Fresh, advisory terminal-resume checks. No prepared launch or process is created.

use std::collections::HashMap;

use serde::Serialize;

use crate::domain::{Session, SourceAvailability};
use crate::error::{other, Result};
use crate::storage::Db;

/// Shared by the real resume preparation and the UI's availability projection.
pub(crate) fn validate_resume_session(session: &Session) -> Result<()> {
    if session.is_archived() {
        return Err(other("已归档的会话不能继续，请先取消归档"));
    }
    match crate::lifecycle::root_source_status(session)? {
        Some(SourceAvailability::Present) => {}
        Some(SourceAvailability::Missing) => return Err(other("源会话已不存在，无法继续该会话")),
        _ => {
            return Err(other(
                "无法确认源会话当前可用（可能无权限或源不可读），已拒绝继续",
            ))
        }
    }
    if let Some(cwd) = session.cwd.as_deref().filter(|c| !c.trim().is_empty()) {
        if !super::is_usable_directory(cwd) {
            return Err(other(format!(
                "原工作目录不存在或当前不可用（{cwd}），无法继续该会话"
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Serialize)]
pub struct TerminalResumeState {
    pub available: bool,
    pub reason: Option<String>,
}

/// Resolve each Agent installation at most once per batch, including failed lookups.
/// This checks prerequisites only: credentials, networking and process startup can
/// still fail later. Launch always revalidates current state.
pub fn terminal_resume_states(
    db: &Db,
    ids: &[String],
) -> Result<HashMap<String, TerminalResumeState>> {
    let mut installations: HashMap<&str, std::result::Result<(), String>> = HashMap::new();
    let mut states = HashMap::new();
    for id in ids {
        if states.contains_key(id) {
            continue;
        }
        let check = (|| -> Result<()> {
            let session = db.get_session(id)?.ok_or_else(|| other("会话不存在"))?;
            let adapter = crate::adapters::adapter_for(session.agent);
            if !adapter.has_terminal_cli()
                || !matches!(
                    adapter.continue_route(&session),
                    crate::adapters::ResumeRoute::Terminal
                )
            {
                return Err(other("该会话格式不支持在终端中继续"));
            }
            validate_resume_session(&session)?;
            let install = installations
                .entry(session.agent.as_str())
                .or_insert_with(|| {
                    super::resolve_install(db, session.agent)
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
            install.as_ref().map_err(|reason| other(reason.clone()))?;
            crate::agent_runtime::runtime_overrides_for_launch(db, session.agent)?;
            Ok(())
        })();
        states.insert(
            id.clone(),
            TerminalResumeState {
                available: check.is_ok(),
                reason: check.err().map(|e| e.to_string()),
            },
        );
    }
    Ok(states)
}
