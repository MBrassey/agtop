// Hardened, shared file readers for the vendor session parsers
// (claude / codex / gemini / goose / aider).
//
// Two hazards these guard against, both reachable because agtop reads
// files written by *other* programs in directories those programs (and
// the agents they run) control:
//
//  1. A FIFO / socket / device named like a session file (`x.jsonl`).
//     `File::open` on a FIFO issues `open(O_RDONLY)` which *blocks until
//     a writer appears* — and the collector runs synchronously on the UI
//     thread, so one blocking open wedges the entire TUI (input, redraw,
//     quit) forever, and `--once` / `--json` hang outright.  We open with
//     `O_NONBLOCK` on Unix and reject anything that isn't a regular file
//     via fstat, so a non-file can never block us.
//
//  2. An unbounded read of a giant (or adversarially-grown) file OOMing
//     the process.  Every read here is hard-capped.
//
// UTF-8 is decoded lossily: a tail/head window that starts or ends
// mid-multibyte-character yields the readable remainder instead of the
// empty string `read_to_string` returns on invalid UTF-8 (which made a
// whole session's tokens/status blink to zero for a tick — or, for
// codex's fixed-size header read, orphaned a file permanently).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Absolute ceiling on any single read regardless of the caller's
/// request — also keeps the `i64`/`usize` casts below from wrapping on
/// 32-bit targets.
pub const HARD_CAP: u64 = 64 * 1024 * 1024;

/// Open `path` only if it resolves to a *regular file*.  On Unix the file
/// is opened with `O_NONBLOCK` so a FIFO/socket can never block the open,
/// then rejected by fstat (the check is on the opened descriptor, so it
/// is not a stat/open TOCTOU).  On non-Unix targets the blocking-open
/// hazard doesn't exist, so a plain open + `is_file()` check suffices.
/// Returns the handle and its length, or `None` for anything that isn't
/// a plain file or on any I/O error.
fn open_regular(path: &Path) -> Option<(File, u64)> {
    #[cfg(unix)]
    let f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let f = File::open(path).ok()?;
    let md = f.metadata().ok()?;
    if !md.is_file() {
        return None;
    }
    Some((f, md.len()))
}

/// Read up to the last `bytes` bytes of a regular file, UTF-8 lossy.
/// Returns `""` for anything that isn't a regular file or on error.
pub fn tail(path: &Path, bytes: u64) -> String {
    let (mut f, len) = match open_regular(path) {
        Some(v) => v,
        None => return String::new(),
    };
    if len == 0 {
        return String::new();
    }
    let take = bytes.min(len).min(HARD_CAP);
    if f.seek(SeekFrom::End(-(take as i64))).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity(take as usize);
    if f.take(take).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Read up to the first `bytes` bytes of a regular file, UTF-8 lossy.
pub fn head(path: &Path, bytes: u64) -> String {
    let (f, _len) = match open_regular(path) {
        Some(v) => v,
        None => return String::new(),
    };
    let cap = bytes.min(HARD_CAP);
    let mut buf = Vec::with_capacity(cap.min(64 * 1024) as usize);
    if f.take(cap).read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Read an entire regular file but never more than `cap` bytes — the
/// bounded replacement for `read_to_string` on whole-file readers.
pub fn whole_capped(path: &Path, cap: u64) -> String {
    head(path, cap)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_non_regular_file_without_blocking() {
        // A FIFO must be rejected immediately, not block on open.  If this
        // regressed the test would hang rather than fail — that's the bug.
        let dir = std::env::temp_dir().join(format!("agtop_rf_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let fifo = dir.join("x.jsonl");
        let _ = std::fs::remove_file(&fifo);
        let cpath = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
        // SAFETY: mkfifo takes a C string path and a mode; no aliasing.
        let rc = unsafe { libc::mkfifo(cpath.as_ptr(), 0o644) };
        if rc == 0 {
            assert_eq!(tail(&fifo, 4096), "", "FIFO must yield empty, not block");
            assert_eq!(whole_capped(&fifo, 4096), "");
            let _ = std::fs::remove_file(&fifo);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_and_head_are_bounded_and_lossy() {
        let dir = std::env::temp_dir().join(format!("agtop_rf2_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let f = dir.join("s.jsonl");
        {
            let mut fh = File::create(&f).unwrap();
            fh.write_all(b"hello world this is a line").unwrap();
        }
        assert_eq!(tail(&f, 4), "line");
        assert_eq!(head(&f, 5), "hello");
        assert_eq!(whole_capped(&f, 1024), "hello world this is a line");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
