//! NoEnding — Local Agent Workspace desktop entry.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = noending_lib::terminal::identity::run_hook_helper_if_requested() {
        std::process::exit(code);
    }
    if let Some(code) = noending_lib::cli::run_if_requested() {
        std::process::exit(code);
    }
    noending_lib::run()
}
