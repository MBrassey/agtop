// agtop — terminal UI for monitoring AI coding agents on the system.
//
// `cargo run` for the TUI; `cargo run -- --once` for a one-shot snapshot.

mod cli;
mod claude;
mod collector;
mod format;
mod matchers;
mod model;
mod proc_;
mod theme;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("agtop: {e:#}");
            ExitCode::from(2)
        }
    }
}
