//! Per-terminal byte acknowledgements for WebView2. portable-pty has no pause
//! API; waiting before the next read lets the bounded OS pipe apply pressure.
//! This lock is independent of TerminalRecord so ACK/close never needs a lock
//! held by a blocked PTY write. Unattached terminals keep their bounded replay.
use std::sync::{Condvar, Mutex};

const HIGH: u64 = 256 * 1024;
const LOW: u64 = 64 * 1024;

#[derive(Default)]
struct State {
    attached: bool,
    closed: bool,
    produced: u64,
    parsed: u64,
}

#[derive(Default)]
pub(super) struct OutputFlow {
    state: Mutex<State>,
    changed: Condvar,
}

impl OutputFlow {
    pub fn attach(&self, replay_start: u64) {
        let mut state = self.state.lock().unwrap();
        state.attached = true;
        state.parsed = state.parsed.max(replay_start.min(state.produced));
        self.changed.notify_all();
    }

    pub fn produced(&self, offset: u64) {
        self.state.lock().unwrap().produced = offset;
    }

    pub fn acknowledge(&self, offset: u64) {
        let mut state = self.state.lock().unwrap();
        // Duplicate/late ACKs cannot move backwards; never grant credit for
        // bytes the backend has not produced.
        state.parsed = state.parsed.max(offset.min(state.produced));
        self.changed.notify_all();
    }

    pub fn wait(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.attached && state.produced.saturating_sub(state.parsed) >= HIGH {
            while !state.closed && state.produced.saturating_sub(state.parsed) > LOW {
                state = self.changed.wait(state).unwrap();
            }
        }
        !state.closed
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn output_backpressure_requires_parsed_bytes_and_close_wakes_reader() {
        let flow = Arc::new(OutputFlow::default());
        flow.produced(HIGH);
        assert!(
            flow.wait(),
            "unattached output must not deadlock startup queries"
        );
        flow.attach(0);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = flow.clone();
        let thread = std::thread::spawn(move || tx.send(reader.wait()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.acknowledge(HIGH - LOW - 1);
        flow.acknowledge(0);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.acknowledge(HIGH - LOW);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        thread.join().unwrap();

        flow.produced(HIGH * 2);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = flow.clone();
        let thread = std::thread::spawn(move || tx.send(reader.wait()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.close();
        assert!(!rx.recv_timeout(Duration::from_secs(2)).unwrap());
        thread.join().unwrap();
    }
}
