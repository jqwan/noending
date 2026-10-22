//! Desktop refresh after another process commits to the same SQLite database.
use crate::storage::Db;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

pub struct ChangeWatcher {
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl ChangeWatcher {
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}
impl Drop for ChangeWatcher {
    fn drop(&mut self) {
        self.shutdown();
    }
}
pub fn watch_changes(db: Arc<Db>, changed: impl Fn() + Send + 'static) -> ChangeWatcher {
    // Compare on the writer connection, so this process's own commits don't
    // cause duplicate UI notifications. No scan or connection file is needed.
    let version = || {
        db.write()
            .pragma_query_value(None, "data_version", |r| r.get::<_, i64>(0))
    };
    let mut previous = version().ok();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            std::thread::park_timeout(std::time::Duration::from_millis(500));
            if worker_stop.load(Ordering::Acquire) {
                break;
            }
            let current = db
                .write()
                .pragma_query_value(None, "data_version", |r| r.get::<_, i64>(0));
            if let Ok(current) = current {
                if previous.is_some_and(|previous| previous != current) {
                    changed();
                }
                previous = Some(current);
            }
        }
    });
    ChangeWatcher {
        stop,
        worker: Mutex::new(Some(worker)),
    }
}
