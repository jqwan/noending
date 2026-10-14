//! Old system ConPTY omits hidden cursor moves from its rendered output.
//! Read the actual console cursor without attaching the GUI process itself.
use crate::error::{other, Result};
use std::{ffi::c_void, os::windows::process::CommandExt, path::Path};

const HELPER: &str = "--noending-terminal-cursor";
use crate::terminal::TerminalCursor as Cursor;
#[repr(C)]
#[derive(Default)]
struct Coord {
    x: i16,
    y: i16,
}
#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}
#[repr(C)]
#[derive(Default)]
struct Info {
    size: Coord,
    cursor: Coord,
    attributes: u16,
    window: Rect,
    maximum: Coord,
}
#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(pid: u32) -> i32;
    fn FreeConsole() -> i32;
    fn GetStdHandle(kind: u32) -> *mut c_void;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn GetConsoleScreenBufferInfo(handle: *mut c_void, info: *mut Info) -> i32;
    fn WriteFile(
        handle: *mut c_void,
        data: *const u8,
        len: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

pub fn run_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(HELPER) {
        return None;
    }
    Some(
        (|| {
            let pid: u32 = args
                .next()
                .ok_or_else(|| other("missing console pid"))?
                .parse()
                .map_err(|_| other("invalid console pid"))?;
            // AttachConsole replaces standard handles. Keep the caller's pipe so
            // no helper response is written into the Agent's terminal output.
            let pipe = unsafe { GetStdHandle(-11i32 as u32) };
            unsafe {
                FreeConsole();
            }
            if unsafe { AttachConsole(pid) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut info = Info::default();
            // STARTF_USESTDHANDLES can preserve redirected stdout on attach.
            // CONOUT$ explicitly opens the attached console, not that pipe.
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            let console = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    0x80000000,
                    3,
                    std::ptr::null_mut(),
                    3,
                    0,
                    std::ptr::null_mut(),
                )
            };
            let ok = unsafe { GetConsoleScreenBufferInfo(console, &mut info) };
            let error = std::io::Error::last_os_error();
            unsafe {
                CloseHandle(console);
                FreeConsole();
            }
            if ok == 0 {
                return Err(error.into());
            }
            let cursor = Cursor {
                x: (info.cursor.x - info.window.left).max(0) as u16,
                y: (info.cursor.y - info.window.top).max(0) as u16,
                cols: (info.window.right - info.window.left + 1).max(0) as u16,
                rows: (info.window.bottom - info.window.top + 1).max(0) as u16,
            };
            let bytes = serde_json::to_vec(&cursor)?;
            let mut written = 0;
            if unsafe {
                WriteFile(
                    pipe,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            } == 0
                || written as usize != bytes.len()
            {
                return Err(other("failed to return console cursor"));
            }
            Ok::<_, crate::error::AppError>(())
        })()
        .map(|_| 0)
        .unwrap_or(1),
    )
}

pub fn query(pid: u32) -> Result<Cursor> {
    query_with_executable(&std::env::current_exe()?, pid)
}
pub(crate) fn query_with_executable(executable: &Path, pid: u32) -> Result<Cursor> {
    let output = std::process::Command::new(executable)
        .args([HELPER, &pid.to_string()])
        .creation_flags(0x08000000)
        .output()?;
    if !output.status.success() {
        return Err(other("无法读取原生终端光标"));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
