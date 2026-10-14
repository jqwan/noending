//! Pin the Windows terminal backend to the ConPTY shipped with NoEnding.
//! portable-pty 0.9 prefers an already loaded conpty.dll over system ConPTY.
use crate::error::{other, Result};
use std::{ffi::c_void, os::windows::ffi::OsStrExt, sync::OnceLock};

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> i32;
}

// Retain the module for the process lifetime: portable-pty caches its function
// pointers globally, and active PTYs must never outlive the DLL.
static MODULE: OnceLock<std::result::Result<usize, String>> = OnceLock::new();

pub(crate) fn ensure_loaded() -> Result<()> {
    MODULE
        .get_or_init(load)
        .as_ref()
        .map(|_| ())
        .map_err(|error| other(error.clone()))
}

fn load() -> std::result::Result<usize, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let directory = executable
        .parent()
        .ok_or_else(|| "无法确定 ConPTY 资源目录".to_string())?
        .join(if cfg!(dev) {
            "conpty-runtime"
        } else {
            "conpty-bundle"
        });
    let dll = directory.join("conpty.dll");
    for name in ["conpty.dll", "OpenConsole.exe"] {
        if !directory.join(name).is_file() {
            return Err(format!(
                "缺少随应用发布的 ConPTY 文件：{}",
                directory.join(name).display()
            ));
        }
    }
    let path: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
    // Search dependencies in the DLL's own directory and trusted default
    // directories, without depending on the user's current working directory.
    let module = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), 0x100 | 0x1000) };
    if module.is_null() {
        return Err(format!(
            "加载 ConPTY 失败：{}：{}",
            dll.display(),
            std::io::Error::last_os_error()
        ));
    }
    // These are the compatibility exports portable-pty 0.9 resolves. Do not
    // silently fall back to an old system backend if the packaged DLL is wrong.
    for name in [
        b"CreatePseudoConsole\0".as_slice(),
        b"ResizePseudoConsole\0".as_slice(),
        b"ClosePseudoConsole\0".as_slice(),
    ] {
        if unsafe { GetProcAddress(module, name.as_ptr()) }.is_null() {
            unsafe {
                FreeLibrary(module);
            }
            return Err("随应用发布的 ConPTY 与 portable-pty 不兼容".to_string());
        }
    }
    Ok(module as usize)
}
