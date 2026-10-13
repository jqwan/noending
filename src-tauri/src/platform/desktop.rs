//! Desktop presence: macOS application bundles, Windows URI associations.
//! A data directory or a CLI installation is not evidence of a desktop app.

pub fn app_present(bundle_name: &str, scheme: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = scheme;
        crate::platform::paths::app_bundle_present(bundle_name)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = bundle_name;
        protocol_handler_present(scheme)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (bundle_name, scheme);
        false
    }
}

#[cfg(target_os = "windows")]
fn protocol_handler_present(scheme: &str) -> bool {
    // Query the effective shell association, including per-user defaults and
    // packaged apps. Codex's Store handler has no ordinary open command or exe,
    // but Windows can resolve its friendly application name.
    const ASSOCF_IS_PROTOCOL: u32 = 0x1000;
    const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
    #[link(name = "shlwapi")]
    unsafe extern "system" {
        fn AssocQueryStringW(
            flags: u32,
            query: u32,
            association: *const u16,
            extra: *const u16,
            output: *mut u16,
            length: *mut u32,
        ) -> i32;
    }
    let scheme: Vec<u16> = scheme.encode_utf16().chain(Some(0)).collect();
    let mut length = 0;
    // SAFETY: the scheme is NUL-terminated and length is writable. A null
    // output asks Windows for the required buffer size (including the NUL).
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_FRIENDLYAPPNAME,
            scheme.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut length,
        )
    };
    if result < 0 || length <= 1 {
        return false;
    }
    let mut output = vec![0u16; length as usize];
    // SAFETY: the output buffer has the size supplied by Windows; all pointers
    // stay valid for this synchronous call. Failure is absence, never success.
    let result = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_FRIENDLYAPPNAME,
            scheme.as_ptr(),
            std::ptr::null(),
            output.as_mut_ptr(),
            &mut length,
        )
    };
    result == 0 && output[0] != 0
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn windows_presence_requires_a_resolvable_protocol_handler() {
        // Windows PowerShell is required, as for the existing Windows launcher.
        // Use unique per-user keys; never alter a real Agent's registration.
        fn registry_script(script: &str) {
            let status = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-Command", script])
                .status()
                .expect("Windows protocol test requires PowerShell");
            assert!(status.success(), "protocol registry fixture failed");
        }
        let prefix = format!("noending-test-{}", crate::storage::new_id());
        let bare = format!("{prefix}-bare");
        let registered = format!("{prefix}-registered");
        let root = "HKCU:\\Software\\Classes";
        struct Cleanup(Vec<String>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for key in &self.0 {
                    registry_script(&format!(
                        "$ErrorActionPreference='Stop'; if (Test-Path -LiteralPath '{key}') {{ Remove-Item -LiteralPath '{key}' -Recurse -Force }}"
                    ));
                }
            }
        }
        let _cleanup = Cleanup(vec![
            format!("{root}\\{bare}"),
            format!("{root}\\{registered}"),
        ]);
        registry_script(&format!(
            "$ErrorActionPreference='Stop'; \
             foreach ($scheme in @('{bare}','{registered}')) {{ \
             $key='{root}\\'+$scheme; New-Item -Path $key -Force | Out-Null; \
             New-ItemProperty -LiteralPath $key -Name 'URL Protocol' -Value '' -Force | Out-Null }}; \
             $command='{root}\\{registered}\\shell\\open\\command'; \
             New-Item -Path $command -Force | Out-Null; \
             Set-Item -LiteralPath $command -Value ('\"'+$env:SystemRoot+'\\System32\\cmd.exe\" \"%1\"')"
        ));
        assert!(!app_present("NoEnding Test", &format!("{prefix}-missing")));
        assert!(!app_present("NoEnding Test", &bare));
        assert!(app_present("NoEnding Test", &registered));
    }
}
