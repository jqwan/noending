//! Foreground Finder's main/key window without raising its other windows.
use crate::error::{other, Result};
use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
};
use objc2_foundation::{NSArray, NSString};

/// `open -g` has already opened/revealed the target without activating Finder.
/// NSRunningApplication defaults to bringing only the main/key windows forward;
/// unlike application-wide activation, ActivateAllWindows is deliberately absent.
/// https://developer.apple.com/documentation/appkit/nsapplication/activationoptions/activateallwindows
pub(super) fn foreground_target() -> Result<()> {
    // NSRunningApplication is safe to call from any thread. These public
    // AppKit selectors use a bundle identifier and never interpolate a path.
    unsafe {
        let class = AnyClass::get(c"NSRunningApplication")
            .ok_or_else(|| other("无法访问系统文件管理器"))?;
        let identifier = NSString::from_str("com.apple.finder");
        let applications: Retained<NSArray<AnyObject>> =
            msg_send![class,runningApplicationsWithBundleIdentifier:&*identifier];
        let application = applications
            .iter()
            .next()
            .ok_or_else(|| other("Finder 尚未启动"))?;
        // NSApplicationActivateIgnoringOtherApps = 1 << 1. Do not include
        // NSApplicationActivateAllWindows (1 << 0).
        let activated: bool = msg_send![&*application,activateWithOptions:2usize];
        if activated {
            Ok(())
        } else {
            Err(other("无法将目标 Finder 窗口置于前台"))
        }
    }
}
