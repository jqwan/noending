//! Agent lifecycle, independent of PTY liveness. No terminal bytes are inspected.
//! Working includes thinking, tools and response output, not a token-stream claim.
//! Hook coverage differs: e.g. Claude has no manual-interrupt hook and Codex has
//! no dedicated failure hook. Missing notifications never imply completion.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    #[default]
    Unknown,
    Idle,
    Working,
    Waiting,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivitySignal {
    pub state: ActivityState,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub starts_turn: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TerminalActivity {
    pub state: ActivityState,
    /// Monotonic even across native session switches. Guards late read receipts.
    pub revision: u64,
    pub unread: bool,
    #[serde(skip)]
    turn_id: Option<String>,
    #[serde(skip)]
    last_signal: Option<ActivitySignal>,
}

impl TerminalActivity {
    pub fn reset(&mut self) {
        *self = Self {
            revision: self.revision + 1,
            ..Self::default()
        };
    }

    pub fn apply(&mut self, signal: &ActivitySignal) -> bool {
        if !signal.starts_turn
            && signal.turn_id.is_some()
            && self.turn_id.is_some()
            && signal.turn_id != self.turn_id
        {
            return false;
        }
        if !signal.starts_turn && self.last_signal.as_ref() == Some(signal) {
            return false;
        }
        self.last_signal = Some(signal.clone());
        let new_turn = signal.starts_turn
            && (self.turn_id != signal.turn_id
                || !matches!(self.state, ActivityState::Working | ActivityState::Waiting));
        if self.state == signal.state && !new_turn {
            return false;
        }
        if signal.starts_turn {
            self.turn_id = signal.turn_id.clone();
        }
        self.state = signal.state;
        self.revision += 1;
        if signal.state == ActivityState::Completed {
            self.unread = true;
        }
        true
    }

    pub fn mark_seen(&mut self, revision: u64) -> bool {
        if self.revision != revision {
            return false;
        }
        let reset = matches!(
            self.state,
            ActivityState::Completed | ActivityState::Failed | ActivityState::Interrupted
        );
        if !self.unread && !reset {
            return false;
        }
        self.unread = false;
        if reset {
            self.state = ActivityState::Idle;
        }
        self.revision += 1;
        true
    }
}
