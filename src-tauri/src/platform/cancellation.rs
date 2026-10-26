//! Cooperative cancellation for an explicitly invoked console command. Desktop
//! startup never installs these handlers. Transactions finish or roll back at
//! their normal boundaries; headless children are terminated by exec_runner.
use std::{
    cell::RefCell,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};
static CANCELLED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static REQUEST: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}
static ACTIVE: Mutex<Vec<Weak<AtomicBool>>> = Mutex::new(Vec::new());
pub fn requested() -> bool {
    CANCELLED.load(Ordering::Relaxed)
        || REQUEST.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
        })
}
/// Bind cancellation to a blocking request worker, without cancelling other
/// MCP requests or the next operation on the same pooled thread.
pub(crate) fn with_request<T>(flag: Arc<AtomicBool>, run: impl FnOnce() -> T) -> T {
    struct Restore(Weak<AtomicBool>);
    impl Drop for Restore {
        fn drop(&mut self) {
            REQUEST.with(|slot| {
                slot.take();
            });
            ACTIVE
                .lock()
                .unwrap()
                .retain(|weak| weak.strong_count() > 0 && !weak.ptr_eq(&self.0));
        }
    }
    {
        let mut active = ACTIVE.lock().unwrap();
        active.retain(|weak| weak.strong_count() > 0);
        active.push(Arc::downgrade(&flag));
    }
    let _restore = Restore(Arc::downgrade(&flag));
    let previous = REQUEST.with(|slot| slot.replace(Some(flag)));
    debug_assert!(previous.is_none(), "request workers must not nest");
    drop(previous);
    run()
}
/// Signal every live request worker (an MCP stdio connection has no caller
/// left to answer it), so a bounded shutdown does not wait out its full cap.
pub(crate) fn cancel_all_requests() {
    let mut active = ACTIVE.lock().unwrap();
    active.retain(|weak| match weak.upgrade() {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    });
}
pub fn check() -> io::Result<()> {
    if requested() {
        Err(io::Error::new(io::ErrorKind::Interrupted, "操作已取消"))
    } else {
        Ok(())
    }
}
pub fn install() -> io::Result<()> {
    #[cfg(unix)]
    {
        extern "C" fn cancel(_: std::ffi::c_int) {
            CANCELLED.store(true, Ordering::Relaxed);
        }
        unsafe extern "C" {
            fn signal(sig: std::ffi::c_int, handler: usize) -> usize;
        }
        // SIGINT and SIGTERM have these values on macOS and Linux.
        for sig in [2, 15] {
            if unsafe { signal(sig, cancel as *const () as usize) } == usize::MAX {
                return Err(io::Error::last_os_error());
            }
        }
    }
    #[cfg(windows)]
    {
        unsafe extern "system" fn cancel(event: u32) -> i32 {
            if matches!(event, 0..=2) {
                CANCELLED.store(true, Ordering::Relaxed);
                1
            } else {
                0
            }
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        if unsafe { SetConsoleCtrlHandler(Some(cancel), 1) } == 0 {
            let error = io::Error::last_os_error();
            // A GUI-subsystem CLI entry with redirected pipes may have no console.
            if error.raw_os_error() != Some(6) {
                return Err(error);
            }
        }
    }
    Ok(())
}
