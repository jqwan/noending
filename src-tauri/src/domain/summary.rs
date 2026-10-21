//! Current summary content, grouped into freely reorganizable topics.
use crate::error::{other, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub items: Vec<SummaryItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryItem {
    pub title: String,
    pub description: String,
    pub details: Vec<String>,
}

impl Summary {
    pub fn single(title: impl Into<String>) -> Self {
        Self {
            items: vec![SummaryItem {
                title: title.into(),
                description: String::new(),
                details: Vec::new(),
            }],
        }
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Normalize surrounding whitespace, rejecting malformed or empty output.
    /// An empty summary is represented by absence, never a successful extraction.
    pub fn validated(mut self) -> Result<Self> {
        if self.is_empty() {
            return Err(other("摘要至少需要一个事项"));
        }
        for item in &mut self.items {
            item.title = item.title.trim().into();
            item.description = item.description.trim().into();
            if item.title.is_empty() || item.title.contains(['\n', '\r']) {
                return Err(other("事项标题必须是非空单行文字"));
            }
            for detail in &mut item.details {
                *detail = detail.trim().into();
                if detail.is_empty() {
                    return Err(other("事项细则不能为空"));
                }
            }
        }
        Ok(self)
    }
    /// Markdown is a read/export projection, not another persisted copy.
    pub fn markdown(&self) -> String {
        self.items
            .iter()
            .map(|item| {
                let mut parts = vec![format!("### {}", item.title)];
                if !item.description.is_empty() {
                    parts.push(item.description.clone());
                }
                if !item.details.is_empty() {
                    parts.push(
                        item.details
                            .iter()
                            .map(|detail| format!("- {}", detail.replace('\n', "\n  ")))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                parts.join("\n\n")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
