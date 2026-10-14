//! Per-terminal byte acknowledgements for WebView2. portable-pty has no pause
//! API; waiting before the next read lets the bounded OS pipe apply pressure.
//! This lock is independent of TerminalRecord so ACK/close never needs a lock
//! held by a blocked PTY write. Unattached terminals keep their bounded replay.
use std::sync::{Condvar, Mutex};

const HIGH: u64 = 256 * 1024;
const LOW: u64 = 64 * 1024;

#[derive(Default)]
struct State {
    consumer: Option<String>,
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
    pub fn attach(&self, consumer: &str, replay_start: u64) {
        let mut state = self.state.lock().unwrap();
        state.consumer = Some(consumer.to_owned());
        state.parsed = state.parsed.max(replay_start.min(state.produced));
        self.changed.notify_all();
    }

    pub fn produced(&self, offset: u64) {
        self.state.lock().unwrap().produced = offset;
    }

    pub fn acknowledge(&self, consumer: &str, offset: u64) {
        let mut state = self.state.lock().unwrap();
        if state.consumer.as_deref() != Some(consumer) {
            return;
        }
        // Duplicate/late ACKs cannot move backwards; never grant credit for
        // bytes the backend has not produced.
        state.parsed = state.parsed.max(offset.min(state.produced));
        self.changed.notify_all();
    }

    pub fn wait(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.consumer.is_some() && state.produced.saturating_sub(state.parsed) >= HIGH {
            while !state.closed
                && state.consumer.is_some()
                && state.produced.saturating_sub(state.parsed) > LOW
            {
                state = self.changed.wait(state).unwrap();
            }
        }
        !state.closed
    }

    pub fn release(&self, consumer: &str) {
        let mut state = self.state.lock().unwrap();
        if state.consumer.as_deref() == Some(consumer) {
            state.consumer = None;
            self.changed.notify_all();
        }
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
        flow.attach("view", 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = flow.clone();
        let thread = std::thread::spawn(move || tx.send(reader.wait()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.acknowledge("view", HIGH - LOW - 1);
        flow.acknowledge("view", 0);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.acknowledge("view", HIGH - LOW);
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

    #[test]
    fn output_release_wakes_reader_and_stale_consumer_cannot_release_or_ack_new_owner() {
        let flow = Arc::new(OutputFlow::default());
        flow.produced(HIGH);
        flow.attach("old", 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = flow.clone();
        let thread = std::thread::spawn(move || tx.send(reader.wait()).unwrap());
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.attach("new", 0);
        flow.release("old");
        flow.acknowledge("old", HIGH);
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        flow.release("new");
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        thread.join().unwrap();
        flow.acknowledge("new", HIGH);
        flow.produced(HIGH * 2);
        assert!(
            flow.wait(),
            "released receivers must not pause background output"
        );
    }
}
