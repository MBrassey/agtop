// agtop — terminal UI for monitoring AI coding agents on the system.
//
// `cargo run` for the TUI; `cargo run -- --once` for a one-shot snapshot.

mod aider;
mod cli;
mod claude;
mod codex;
mod collector;
mod format;
mod gemini;
mod generic;
mod goose;
mod matchers;
mod model;
mod omp;
mod pricing;
mod pricing_data;
mod proc_;
mod sessions;
mod plugins;
mod skills;
mod reading_files;
mod net_count;
mod paths;
mod sysbackend;
#[cfg(windows)]
mod wsl_backend;
mod writing_files;
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
