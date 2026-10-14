fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        copy_conpty();
    }
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut attributes = tauri_build::Attributes::new();
    if windows_msvc {
        // Tauri's resource manifest reaches app binaries only. Embed it through
        // the linker instead so lib/integration test executables also select
        // Common Controls v6 (required by the dialog APIs' TaskDialogIndirect).
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}

fn copy_conpty() {
    assert_eq!(
        std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86_64"),
        "the bundled ConPTY binaries currently support Windows x64 only"
    );
    let source = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("resources/conpty");
    // OUT_DIR is <target>/<profile>/build/<package>/out, including when Cargo
    // uses a custom target directory or an explicit target triple.
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let profile = output.ancestors().nth(3).expect("Cargo profile directory");
    // Tauri recopies bundle.resources on every build. Dev binaries must load
    // a separate copy so that hot rebuilds never overwrite an in-use DLL.
    for destination in [
        profile.join("conpty-runtime"),
        profile.join("deps/conpty-runtime"),
        profile.join("deps/conpty-bundle"),
    ] {
        std::fs::create_dir_all(&destination).expect("create ConPTY output directory");
        for name in ["conpty.dll", "OpenConsole.exe", "LICENSE"] {
            let file = source.join(name);
            println!("cargo:rerun-if-changed={}", file.display());
            let target = destination.join(name);
            let bytes = std::fs::read(&file).expect("read bundled ConPTY");
            // tauri dev can rebuild while the previous process still holds
            // these binaries open. Leave unchanged files in place.
            if std::fs::read(&target).ok().as_deref() != Some(bytes.as_slice()) {
                std::fs::write(&target, bytes).expect("copy bundled ConPTY");
            }
        }
    }
}
