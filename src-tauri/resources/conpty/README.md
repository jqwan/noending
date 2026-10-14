# Bundled Windows ConPTY

The Windows x64 app ships `conpty.dll` and its matching `OpenConsole.exe` from
Microsoft's stable NuGet package `Microsoft.Windows.Console.ConPTY` **1.25.260930003**.
Both files report Windows file version **1.25.2609.30003**.

Source: https://www.nuget.org/packages/Microsoft.Windows.Console.ConPTY/1.25.260930003

Download:
https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/1.25.260930003/microsoft.windows.console.conpty.1.25.260930003.nupkg

Package SHA-256:
`02B07B349AF66D801159BDF9E440D4A1CE78BB951F37FC8609731665AFDAE7EE`

Original package paths:

- `runtimes/win-x64/native/conpty.dll`
- `build/native/runtimes/x64/OpenConsole.exe`

Binary SHA-256:

- `conpty.dll`: `FEEEF341D891643C62D30B6B07800BC70F0BC148F44CB8C3BEE84AA557AE805A`
- `OpenConsole.exe`: `3D66B23D0A71BB8EED2B77EDC8B9DF9BF54CE6C8FB74C863A30E760997F80586`

Microsoft distributes these files under the MIT license. `LICENSE` retains
Microsoft's copyright and permission notice from the source project:
https://github.com/microsoft/terminal/blob/main/LICENSE

`build.rs` copies the pair into `<target>/<profile>/conpty-runtime` for development
and `<target>/<profile>/deps/conpty-runtime` for native test executables. The
Windows-only Tauri configuration installs the same pair into `conpty-bundle`
beside the app; production binaries load that directory. Test executables using
the production configuration also receive a `deps/conpty-bundle` copy.
Keeping dev-loaded files separate from Tauri's resource staging prevents hot
rebuilds from overwriting binaries held open by the running app. Unchanged
runtime files are not overwritten.
Before opening the first PTY, the app loads that DLL by absolute path and
checks the exports used by portable-pty 0.9. Missing/incompatible files produce
an error instead of silently selecting the system ConPTY.

The package is deliberately pinned; updating it requires replacing both files
together and updating their provenance. The current bundled binaries are x64;
Windows ARM64/x86 builds need their matching package binaries and resource mapping.
