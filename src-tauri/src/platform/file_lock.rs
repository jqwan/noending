//! OS-owned locks shared by desktop and short-lived CLI processes.
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

/// Keep the returned handle alive for the entire operation. Never unlink the
/// lock file: another process may already have it open. The OS releases a lock
/// when its owner exits, including after a crash.
fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}
pub fn exclusive(path: &Path) -> io::Result<File> {
    let file = open(path)?;
    try_lock(&file)?;
    Ok(file)
}
pub fn shared(path: &Path) -> io::Result<File> {
    let file = open(path)?;
    file.try_lock_shared().map_err(lock_error)?;
    Ok(file)
}
fn lock_error(error: std::fs::TryLockError) -> io::Error {
    match error {
        std::fs::TryLockError::WouldBlock => {
            io::Error::new(io::ErrorKind::WouldBlock, "操作正在其他进程中运行")
        }
        std::fs::TryLockError::Error(error) => error,
    }
}
pub fn try_lock(file: &File) -> io::Result<()> {
    file.try_lock().map_err(lock_error)
}

/// Bound startup contention without leaving lock ownership in a PID file.
pub fn exclusive_wait(path: &Path, timeout: std::time::Duration) -> io::Result<File> {
    let started = std::time::Instant::now();
    loop {
        match exclusive(path) {
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && started.elapsed() < timeout =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => return result,
        }
    }
}
