// agtop — terminal UI for monitoring AI coding agents on the system.
//
// `cargo run` for the TUI; `cargo run -- --once` for a one-shot snapshot.

mod cli;
mod claude;
mod codex;
mod collector;
mod format;
mod generic;
mod matchers;
mod model;
mod proc_;
mod sessions;
mod theme;
mod ui;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Restore default SIGPIPE behavior so `agtop --json | head` exits cleanly
    // (status 141) instead of panicking inside Rust's stdout writer.
    #[cfg(unix)]
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }

    match cli::run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("agtop: {e:#}");
            ExitCode::from(2)
        }
    }
}
