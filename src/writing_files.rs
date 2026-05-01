// Per-process writable-file enumeration, native on each OS.
//
// On Linux the original implementation in `proc_::read_writing_files`
// walks `/proc/<pid>/fdinfo` + `/proc/<pid>/fd`.  This module
// dispatches that on Linux and provides equivalent native
// implementations for macOS (libproc / proc_pidinfo) and Windows
// (NtQuerySystemInformation + DuplicateHandle + GetFinalPathNameByHandle).
//
// All implementations:
//   - return an empty `Vec` on any access denial / unknown PID
//     (never panic, never bubble errors)
//   - cap the result at `limit` entries so a process with thousands
//     of open files doesn't dominate one collector tick
//   - exclude pipes / sockets / device nodes / anonymous memory
//     mappings — only real on-disk files

use std::path::PathBuf;

/// Cross-platform entry point.  Returns `Vec<PathBuf>` of files the
/// target PID has open with write access (O_WRONLY / O_RDWR on POSIX,
/// FILE_GENERIC_WRITE on Windows).
pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
    impl_::read(pid, limit)
}

// ── Linux ──────────────────────────────────────────────────────────────────
#[cfg(target_os = "linux")]
mod impl_ {
    use super::*;
    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        crate::proc_::read_writing_files(pid, limit)
    }
}

// ── macOS ──────────────────────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod impl_ {
    use super::*;
    use libproc::libproc::file_info::{pidfdinfo, ListFDs, ProcFDType, VnodeFDInfoWithPath};
    use libproc::libproc::proc_pid::listpidinfo;

    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        let pid = pid as i32;
        // First call sizes the buffer; the libproc wrapper handles the
        // double-call dance internally.  Cap the requested count so a
        // pathological process with millions of FDs doesn't OOM.
        let fds = match listpidinfo::<ListFDs>(pid, 4096) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::with_capacity(limit.min(fds.len()));
        for fd in fds.iter().take(4096) {
            if out.len() >= limit { break; }
            // Only vnode (file-backed) FDs have a path; skip pipes,
            // sockets, kqueues, etc.
            if fd.proc_fdtype != ProcFDType::VNode as u32 { continue; }
            let info: VnodeFDInfoWithPath = match pidfdinfo(pid, fd.proc_fd) {
                Ok(i) => i,
                Err(_) => continue,
            };
            // O_WRONLY = 1, O_RDWR = 2 — same constants as POSIX.
            let flags = info.pfi.fi_openflags as i32;
            if flags & (libc::O_WRONLY | libc::O_RDWR) == 0 { continue; }
            // VnodePathInfo carries a NUL-terminated MAXPATHLEN buffer.
            let path_bytes = &info.pvip.vip_path;
            let nul = path_bytes.iter().position(|&b| b == 0).unwrap_or(path_bytes.len());
            // i8 → u8 reinterpret for the path bytes (libproc types as i8).
            let bytes: Vec<u8> = path_bytes[..nul].iter().map(|&c| c as u8).collect();
            let s = match std::str::from_utf8(&bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Filter device / pseudo paths the same way the Linux impl does.
            if s.starts_with("/dev/") { continue; }
            // macOS doesn't surface pipe:/socket:/anon_inode: as paths
            // the way Linux does (those have empty vip_path), so the
            // remaining filtering is just /dev/.
            out.push(PathBuf::from(s));
        }
        out
    }
}

// ── Windows ────────────────────────────────────────────────────────────────
#[cfg(windows)]
mod impl_ {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE,
        STATUS_INFO_LENGTH_MISMATCH, NTSTATUS,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_GENERIC_WRITE, FILE_NAME_NORMALIZED,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE,
    };
    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation,
    };

    // Manual SystemExtendedHandleInformation = 0x40 (windows-sys
    // doesn't expose the constant for the extended variant).
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 0x40;
    // Object type index for File on Windows is variable across Windows
    // versions; rather than enumerate the type table we filter by
    // GrantedAccess including FILE_GENERIC_WRITE bits.

    #[repr(C)]
    struct SystemHandleInformationEx {
        number_of_handles: usize,
        reserved: usize,
        handles: [SystemHandleTableEntryInfoEx; 1],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SystemHandleTableEntryInfoEx {
        object: *mut std::ffi::c_void,
        unique_process_id: usize,
        handle_value: usize,
        granted_access: u32,
        creator_back_trace_index: u16,
        object_type_index: u16,
        handle_attributes: u32,
        reserved: u32,
    }

    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        let target_pid = pid as usize;
        // Step 1: query the global handle table.  Windows refuses if
        // we don't pre-size the buffer, so loop until the call fits.
        let mut buf: Vec<u8> = vec![0u8; 256 * 1024];
        let table: *const SystemHandleInformationEx = loop {
            let mut needed: u32 = 0;
            let status: NTSTATUS = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_EXTENDED_HANDLE_INFORMATION,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                    &mut needed,
                )
            };
            if status == 0 { break buf.as_ptr() as *const _; }
            if status == STATUS_INFO_LENGTH_MISMATCH {
                // Grow generously — the handle table on a busy box can
                // be many MiB.  Cap at 64 MiB so a runaway query can't
                // OOM agtop.
                let grow = ((needed as usize).max(buf.len() * 2)).min(64 * 1024 * 1024);
                if grow == buf.len() { return Vec::new(); }
                buf.resize(grow, 0);
                continue;
            }
            return Vec::new();
        };

        // Step 2: enumerate, filtering to (a) handles owned by our PID
        // and (b) handles with FILE_GENERIC_WRITE in their granted-access
        // mask.  Open a single dup-source handle on the target process.
        let proc_handle: HANDLE = unsafe {
            OpenProcess(PROCESS_DUP_HANDLE, 0, pid)
        };
        if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
            return Vec::new();
        }

        let header = unsafe { &*table };
        let count = header.number_of_handles;
        let entries: *const SystemHandleTableEntryInfoEx = unsafe {
            (table as *const u8)
                .add(std::mem::size_of::<usize>() * 2) // skip number_of_handles + reserved
                as *const SystemHandleTableEntryInfoEx
        };

        let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
        let me = unsafe { GetCurrentProcess() };

        for i in 0..count {
            if out.len() >= limit { break; }
            let entry = unsafe { *entries.add(i) };
            if entry.unique_process_id != target_pid { continue; }
            // FILE_GENERIC_WRITE = STANDARD_RIGHTS_WRITE | FILE_WRITE_DATA |
            // FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA | SYNCHRONIZE = 0x120116.
            // Match any handle whose grant overlaps the write-data bit.
            if entry.granted_access & FILE_GENERIC_WRITE == 0 { continue; }

            // Step 3: duplicate into our process so we can resolve a path.
            let mut dup: HANDLE = std::ptr::null_mut();
            let ok = unsafe {
                DuplicateHandle(
                    proc_handle,
                    entry.handle_value as HANDLE,
                    me,
                    &mut dup,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if ok == 0 || dup.is_null() { continue; }

            // Step 4: ask Windows for the final path.
            let mut wbuf = [0u16; 32_768];
            let n = unsafe {
                GetFinalPathNameByHandleW(dup, wbuf.as_mut_ptr(), wbuf.len() as u32, FILE_NAME_NORMALIZED)
            };
            unsafe { CloseHandle(dup); }
            if n == 0 || (n as usize) > wbuf.len() { continue; }
            let s = OsString::from_wide(&wbuf[..n as usize]);
            let s = s.to_string_lossy();
            // Strip the `\\?\` long-path prefix Windows returns.
            let trimmed = s.strip_prefix(r"\\?\").unwrap_or(&s);
            // Skip device paths.
            if trimmed.starts_with(r"\Device\") { continue; }
            out.push(PathBuf::from(trimmed));
        }

        unsafe { CloseHandle(proc_handle); }
        out
    }
}

// ── Other Unix (FreeBSD / NetBSD / OpenBSD / DragonFly) ────────────────────
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod impl_ {
    use super::*;
    pub fn read(_pid: u32, _limit: usize) -> Vec<PathBuf> {
        // No portable cross-BSD API for foreign-process FD enumeration.
        // FreeBSD has procstat_getfiles(); NetBSD/OpenBSD have similar.
        // Out of scope until someone needs it.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::fs::OpenOptions;

    /// Cross-platform end-to-end test.  Opens a tempfile for writing
    /// and asserts our writable-FD enumerator finds it in the current
    /// process's open-file set.  Runs identically on Linux, macOS,
    /// and Windows; on FreeBSD it skips with a notice (no impl).
    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn enumerates_self_open_writable_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        // Re-open with explicit write so the OS records us as writer.
        let mut f = OpenOptions::new().write(true).open(&path).unwrap();
        writeln!(f, "agtop-test").unwrap();
        f.sync_all().unwrap();
        // Keep `f` alive for the duration of the read.
        let pid = std::process::id();
        let files = read(pid, 4096);
        // Drop after enumeration so the assertion message can reference path.
        let canon_target = std::fs::canonicalize(&path).unwrap_or(path.clone());
        // Match either the raw or canonicalised path; macOS often
        // returns the resolved /private/var/folders/... path while
        // Linux / Windows return what was opened.
        let found = files.iter().any(|p| {
            p == &path || p == &canon_target
                // Substring fallback for cases like Windows long-path
                // prefixes / macOS /private/var symlink resolution.
                || p.to_string_lossy().contains(path.file_name().unwrap().to_str().unwrap())
        });
        drop(f);
        drop(tmp);
        assert!(found,
            "writing_files::read({}) did not include the test tempfile.\n\
             Opened: {:?}\n\
             Returned ({} entries):\n{}",
            pid, path, files.len(),
            files.iter().take(20).map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n"));
    }
}
