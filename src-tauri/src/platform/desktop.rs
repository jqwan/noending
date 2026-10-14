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
    use base64::Engine;
    use std::os::windows::process::CommandExt;

    #[test]
    fn windows_presence_requires_a_resolvable_protocol_handler() {
        // Windows PowerShell is required, as for the existing Windows launcher.
        // Use unique per-user keys; never alter a real Agent's registration.
        fn registry_script(script: &str) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(
                script
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            );
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-EncodedCommand", &encoded])
                .creation_flags(0x08000000)
                .output()
                .expect("Windows protocol test requires PowerShell");
            assert!(
                output.status.success(),
                "protocol registry fixture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let prefix = format!("noending-test-{}", crate::storage::new_id());
        let bare = format!("{prefix}-bare");
        let registered = format!("{prefix}-registered");
        let root = "HKCU:\\Software\\Classes";
        // Give the fixture its own executable and explicit friendly name. A
        // system cmd.exe's application registration/MUI varies by runner image.
        let executable = std::env::temp_dir().join(format!("{prefix}.exe"));
        let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        std::fs::copy(
            std::path::PathBuf::from(system_root).join("System32/cmd.exe"),
            &executable,
        )
        .unwrap();
        let application = format!("{root}\\Applications\\{prefix}.exe");
        struct Cleanup(Vec<String>, std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                for key in &self.0 {
                    registry_script(&format!(
                        "$ErrorActionPreference='Stop'; if (Test-Path -LiteralPath '{key}') {{ Remove-Item -LiteralPath '{key}' -Recurse -Force }}"
                    ));
                }
                std::fs::remove_file(&self.1).unwrap();
            }
        }
        let _cleanup = Cleanup(
            vec![
                format!("{root}\\{bare}"),
                format!("{root}\\{registered}"),
                application.clone(),
            ],
            executable.clone(),
        );
        let command = format!("\"{}\" \"%1\"", executable.display());
        let command = crate::platform::launcher::ps_quote(&command);
        registry_script(&format!(
            "$ErrorActionPreference='Stop'; \
             foreach ($scheme in @('{bare}','{registered}')) {{ \
             $key='{root}\\'+$scheme; New-Item -Path $key -Force | Out-Null; \
             New-ItemProperty -LiteralPath $key -Name 'URL Protocol' -Value '' -Force | Out-Null }}; \
             $command='{root}\\{registered}\\shell\\open\\command'; \
             New-Item -Path $command -Force | Out-Null; \
             Set-Item -LiteralPath $command -Value {command}; \
             New-Item -Path '{application}' -Force | Out-Null; \
             New-ItemProperty -LiteralPath '{application}' -Name 'FriendlyAppName' -Value 'NoEnding Protocol Test' -Force | Out-Null"
        ));
        assert!(!app_present("NoEnding Test", &format!("{prefix}-missing")));
        assert!(!app_present("NoEnding Test", &bare));
        assert!(app_present("NoEnding Test", &registered));
    }
}
