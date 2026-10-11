//! Console-subsystem entry on Windows; the desktop app remains `noending`.
fn main() {
    if let Some(code) = noending::terminal::identity::run_hook_helper_if_requested() {
        std::process::exit(code);
    }
    std::process::exit(noending::cli::run(std::env::args().skip(1).collect()));
}
