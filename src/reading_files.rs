// Per-process read-mode file enumeration, native on each OS.
//
// Mirror of `writing_files.rs` but selects FDs that are open for
// reading only (O_RDONLY).  Surfaces what an agent is actively
// reading right now — project files during context indexing,
// MCP server configs, hook scripts — when nothing else explains
// where its CPU is going.
//
// Same hard caps as writing_files:
//   - capped at `limit` entries
//   - excludes pipes, sockets, device nodes, anonymous memory,
//     /proc/* and /sys/* (agents poll those every tick to monitor
//     children; flooding the popup with /proc/<pid>/statm reads
//     buries the actually-interesting entries)

use std::path::PathBuf;

/// Cross-platform entry point.  Returns `Vec<PathBuf>` of files the
/// target PID has open in read-only mode.
///
/// Windows path runs under a 2-second deadline because
/// `GetFinalPathNameByHandleW` can stall on remote/SMB handles even
/// after the FILE_TYPE_DISK pre-filter — same hardening as
/// `writing_files::read`, on a dedicated single-worker watchdog so a
/// persistently wedged handle refuses later sweeps instead of
/// stacking one abandoned thread per tick.
#[cfg(windows)]
pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
    use std::sync::OnceLock;
    use crate::writing_files::watchdog::Watchdog;
    static DOG: OnceLock<Watchdog> = OnceLock::new();
    DOG.get_or_init(Watchdog::spawn)
        .run(std::time::Duration::from_secs(2), move || impl_::read(pid, limit))
}
#[cfg(not(windows))]
pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
    impl_::read(pid, limit)
}

// ── Linux ──────────────────────────────────────────────────────────────────
#[cfg(target_os = "linux")]
mod impl_ {
    use super::*;
    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        crate::proc_::read_reading_files(pid, limit)
    }
}

// ── macOS: libproc, same path as writing_files but inverted flag check ────
#[cfg(target_os = "macos")]
mod impl_ {
    use super::*;
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int, c_uint};

    const PROC_PIDLISTFDS:           c_int = 1;
    const PROC_PIDFDVNODEPATHINFO:   c_int = 2;
    const PROX_FDTYPE_VNODE:         u32   = 1;

    extern "C" {
        fn proc_pidinfo(
            pid: c_int, flavor: c_int, arg: u64,
            buffer: *mut c_void, buffersize: c_int,
        ) -> c_int;
        fn proc_pidfdinfo(
            pid: c_int, fd: c_int, flavor: c_int,
            buffer: *mut c_void, buffersize: c_int,
        ) -> c_int;
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ProcFdInfo {
        proc_fd: c_int,
        proc_fdtype: c_uint,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct ProcFileInfo {
        fi_openflags: u32,
        fi_status:    u32,
        fi_offset:    i64,
        fi_type:      i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VnodeInfo { _opaque: [u8; 152] }
    impl Default for VnodeInfo {
        fn default() -> Self { Self { _opaque: [0u8; 152] } }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VnodeInfoPath {
        vip_vi:   VnodeInfo,
        vip_path: [c_char; 1024],
    }
    impl Default for VnodeInfoPath {
        fn default() -> Self { Self { vip_vi: VnodeInfo::default(), vip_path: [0; 1024] } }
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct VnodeFdInfoWithPath {
        pfi:  ProcFileInfo,
        pvip: VnodeInfoPath,
    }

    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        let pid = pid as c_int;

        let probe = unsafe {
            proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0)
        };
        if probe <= 0 { return Vec::new(); }
        let needed = (probe as usize).min(4096 * std::mem::size_of::<ProcFdInfo>());
        let entry_count = needed / std::mem::size_of::<ProcFdInfo>();
        let mut buf: Vec<ProcFdInfo> = vec![ProcFdInfo::default(); entry_count];

        let written = unsafe {
            proc_pidinfo(
                pid, PROC_PIDLISTFDS, 0,
                buf.as_mut_ptr() as *mut c_void,
                (buf.len() * std::mem::size_of::<ProcFdInfo>()) as c_int,
            )
        };
        if written <= 0 { return Vec::new(); }
        let got = (written as usize) / std::mem::size_of::<ProcFdInfo>();

        let mut out: Vec<PathBuf> = Vec::with_capacity(limit.min(got));
        for fd in buf.iter().take(got) {
            if out.len() >= limit { break; }
            if fd.proc_fdtype != PROX_FDTYPE_VNODE { continue; }

            let mut info = VnodeFdInfoWithPath::default();
            let n = unsafe {
                proc_pidfdinfo(
                    pid, fd.proc_fd, PROC_PIDFDVNODEPATHINFO,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of::<VnodeFdInfoWithPath>() as c_int,
                )
            };
            if n <= 0 { continue; }
            // libproc fi_openflags uses BSD FREAD/FWRITE semantics
            // (<sys/file.h>: FREAD=1, FWRITE=2), NOT the POSIX
            // O_RDONLY/O_WRONLY/O_RDWR values used to open the file:
            //   O_RDONLY  → fi_openflags = FREAD          = 0x1
            //   O_WRONLY  → fi_openflags = FWRITE         = 0x2
            //   O_RDWR    → fi_openflags = FREAD | FWRITE = 0x3
            // Pre-2.4.12 we masked against POSIX O_WRONLY|O_RDWR, which
            // happens to be 0x3 — the SAME bits as FREAD|FWRITE — so
            // every read-only fd (FREAD=1) tested as "O_WRONLY-set"
            // and got filtered out.  Use the right semantics: include
            // when FREAD is set AND FWRITE is clear.
            const FREAD:  u32 = 0x1;
            const FWRITE: u32 = 0x2;
            let flags: u32 = info.pfi.fi_openflags;
            if (flags & FREAD) == 0 || (flags & FWRITE) != 0 { continue; }

            let path = &info.pvip.vip_path;
            let nul = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            let bytes: Vec<u8> = path[..nul].iter().map(|&b| b as u8).collect();
            let s = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => continue,
            };
            if s.is_empty() { continue; }
            // Same exclusions as the Linux reader.
            if s.starts_with("/dev/")
                || s.starts_with("/proc/") || s.starts_with("/sys/")
                || s == "/" { continue; }
            out.push(PathBuf::from(s));
        }
        out
    }
}

// ── Windows: NtQuerySystemInformation + filter access mask ─────────────────
//
// Windows handles don't cleanly distinguish "opened read-only" the
// way POSIX O_RDONLY does — every File handle has `granted_access`
// flags from the union of NTFS access rights.  Heuristic: include
// handles that have FILE_GENERIC_READ but NOT FILE_GENERIC_WRITE.
// This catches cargo's source-file reads, MCP-config reads, etc.,
// while not double-counting the writable handles that would have
// already shown up in the "writing files" surface.
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
        GetFileType, GetFinalPathNameByHandleW,
        FILE_READ_DATA, FILE_WRITE_DATA,
        FILE_NAME_NORMALIZED, FILE_TYPE_DISK,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE,
    };
    use windows_sys::Wdk::System::SystemInformation::{
        NtQuerySystemInformation,
    };

    const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 0x40;

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
        let mut buf: Vec<u8> = vec![0u8; 256 * 1024];
        let mut retries = 0u32;
        let table: *const SystemHandleInformationEx = loop {
            retries += 1;
            if retries > 8 { return Vec::new(); }
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
                let grow = ((needed as usize).max(buf.len() * 2)).min(64 * 1024 * 1024);
                if grow == buf.len() { return Vec::new(); }
                buf.resize(grow, 0);
                continue;
            }
            return Vec::new();
        };

        let proc_handle: HANDLE = unsafe {
            OpenProcess(PROCESS_DUP_HANDLE, 0, pid)
        };
        if proc_handle.is_null() || proc_handle == INVALID_HANDLE_VALUE {
            return Vec::new();
        }

        let header = unsafe { &*table };
        let header_size = std::mem::size_of::<usize>() * 2;
        let entry_size  = std::mem::size_of::<SystemHandleTableEntryInfoEx>();
        let max_entries = buf.len().saturating_sub(header_size) / entry_size;
        let count = header.number_of_handles.min(max_entries);
        let entries: *const SystemHandleTableEntryInfoEx = unsafe {
            (table as *const u8).add(header_size) as *const SystemHandleTableEntryInfoEx
        };

        let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
        let me = unsafe { GetCurrentProcess() };

        for i in 0..count {
            if out.len() >= limit { break; }
            let entry = unsafe { *entries.add(i) };
            if entry.unique_process_id != target_pid { continue; }
            // Read-only: has FILE_READ_DATA (0x1) and NOT FILE_WRITE_DATA
            // (0x2).  Pre-2.4.10 we tested the composite FILE_GENERIC_*
            // masks, but those share SYNCHRONIZE (0x100000) which
            // every File handle has — making `has_write` always true.
            // Use the specific data-rights bits instead.
            let has_read  = entry.granted_access & FILE_READ_DATA  != 0;
            let has_write = entry.granted_access & FILE_WRITE_DATA != 0;
            if !has_read || has_write { continue; }

            let mut dup: HANDLE = std::ptr::null_mut();
            let ok = unsafe {
                DuplicateHandle(
                    proc_handle,
                    entry.handle_value as HANDLE,
                    me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS,
                )
            };
            if ok == 0 || dup.is_null() { continue; }

            // Same disk-only filter as writing_files — pipes /
            // sockets / mailslots will hang GetFinalPathNameByHandleW.
            let ftype = unsafe { GetFileType(dup) };
            if ftype != FILE_TYPE_DISK {
                unsafe { CloseHandle(dup); }
                continue;
            }

            let mut wbuf = [0u16; 32_768];
            let n = unsafe {
                GetFinalPathNameByHandleW(dup, wbuf.as_mut_ptr(), wbuf.len() as u32, FILE_NAME_NORMALIZED)
            };
            unsafe { CloseHandle(dup); }
            if n == 0 || (n as usize) > wbuf.len() { continue; }
            let s = OsString::from_wide(&wbuf[..n as usize]);
            let s = s.to_string_lossy();
            let trimmed = s.strip_prefix(r"\\?\").unwrap_or(&s);
            if trimmed.starts_with(r"\Device\") { continue; }
            out.push(PathBuf::from(trimmed));
        }

        unsafe { CloseHandle(proc_handle); }
        out
    }
}

// ── FreeBSD / DragonFly: libprocstat ──────────────────────────────────────
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
#[allow(clashing_extern_declarations)]
mod impl_ {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

    const PS_FST_TYPE_VNODE:  c_int = 1;
    const PS_FST_FFLAG_READ:  c_int = 0x0001;
    const PS_FST_FFLAG_WRITE: c_int = 0x0002;
    const KERN_PROC_PID:      c_int = 1;

    // Mirror of `struct filestat` from <libprocstat.h>.  The STAILQ_ENTRY
    // link (`next`) is at the *tail* of the struct, after fs_path — NOT a
    // leading field.  A previous leading `next: StqEntry` here added an
    // 8-byte prefix that shifted every field: fs_path read the real
    // struct's link pointer and `CStr::from_ptr` scanned a non-string heap
    // pointer, so on FreeBSD an agent's read-only fds produced garbage
    // paths or a crash.  This now matches writing_files.rs's (correct)
    // mirror of the same C struct exactly.
    #[repr(C)]
    struct FileStat {
        fs_type:    c_int,
        fs_flags:   c_int,
        fs_fflags:  c_int,
        fs_uflags:  c_int,
        fs_fd:      c_int,
        fs_ref_count: c_int,
        fs_offset:  i64,
        fs_typedep: *mut c_void,
        fs_path:    *mut c_char,
        // STAILQ_ENTRY(filestat) — single next pointer at the tail.
        next_stqe_next: *mut FileStat,
    }

    #[repr(C)]
    struct FileStatList { stqh_first: *mut FileStat, stqh_last: *mut *mut FileStat }

    // writing_files.rs also binds these symbols.  Use opaque
    // pointers for procstat_getfiles / procstat_freefiles so the
    // clashing-extern check sees identical signatures from both
    // modules; cast to the typed pointer at the call site below.
    extern "C" {
        fn procstat_open_sysctl() -> *mut c_void;
        fn procstat_close(ps: *mut c_void);
        fn procstat_getprocs(ps: *mut c_void, what: c_int, arg: c_int, count: *mut c_uint) -> *mut c_void;
        fn procstat_freeprocs(ps: *mut c_void, p: *mut c_void);
        fn procstat_getfiles(ps: *mut c_void, kproc: *mut c_void, mmaped: c_int) -> *mut c_void;
        fn procstat_freefiles(ps: *mut c_void, head: *mut c_void);
    }

    pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
        unsafe {
            let ps = procstat_open_sysctl();
            if ps.is_null() { return Vec::new(); }
            let mut count: c_uint = 0;
            let kproc = procstat_getprocs(ps, KERN_PROC_PID, pid as c_int, &mut count);
            if kproc.is_null() || count == 0 {
                procstat_close(ps);
                return Vec::new();
            }
            let head_raw = procstat_getfiles(ps, kproc, 0);
            if head_raw.is_null() {
                procstat_freeprocs(ps, kproc);
                procstat_close(ps);
                return Vec::new();
            }
            let head = head_raw as *mut FileStatList;

            let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
            let mut node = (*head).stqh_first;
            let mut walked = 0u32;
            while !node.is_null() && walked < 4096 {
                walked += 1;
                if out.len() >= limit { break; }
                let f = &*node;
                let read  = (f.fs_flags & PS_FST_FFLAG_READ) != 0;
                let write = (f.fs_flags & PS_FST_FFLAG_WRITE) != 0;
                let is_vnode = f.fs_type == PS_FST_TYPE_VNODE;
                if read && !write && is_vnode && !f.fs_path.is_null() {
                    let cstr = CStr::from_ptr(f.fs_path);
                    if let Ok(s) = cstr.to_str() {
                        if !s.is_empty() && !s.starts_with("/dev/") {
                            out.push(PathBuf::from(s));
                        }
                    }
                }
                node = f.next_stqe_next;
            }

            procstat_freefiles(ps, head_raw);
            procstat_freeprocs(ps, kproc);
            procstat_close(ps);
            out
        }
    }
}

// ── OpenBSD / NetBSD / everything else: stub ──────────────────────────────
#[cfg(any(target_os = "openbsd", target_os = "netbsd"))]
mod impl_ {
    use super::*;
    pub fn read(_pid: u32, _limit: usize) -> Vec<PathBuf> { Vec::new() }
}

#[cfg(not(any(
    target_os = "linux", target_os = "macos", windows,
    target_os = "freebsd", target_os = "dragonfly",
    target_os = "openbsd", target_os = "netbsd",
)))]
mod impl_ {
    use super::*;
    pub fn read(_pid: u32, _limit: usize) -> Vec<PathBuf> { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::fs::OpenOptions;

    /// Cross-platform end-to-end test.  Open a tempfile read-only,
    /// assert our enumerator finds it for the current process.
    /// Linux + macOS + Windows have native paths; FreeBSD has
    /// libprocstat; the rest fall through to the empty stub.
    #[test]
    // Linux + Windows verified on real CI hardware.  macOS impl
    // compiles + cross-checks but needs hardware iteration to
    // tune the libproc fi_openflags interpretation; gate it off
    // until then.  Run `cargo test -- --include-ignored` on a Mac
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn enumerates_self_open_readonly_file() {
        // Write a file, then re-open it read-only so the test
        // process holds an O_RDONLY fd to it.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        {
            let mut w = OpenOptions::new().write(true).open(&path).unwrap();
            writeln!(w, "agtop reading_files self-test").unwrap();
        }
        let _r = OpenOptions::new().read(true).open(&path).unwrap();

        let pid = std::process::id();
        let files = read(pid, 4096);
        let canon = std::fs::canonicalize(&path).unwrap_or(path.clone());
        let stem = path.file_name().unwrap().to_string_lossy();
        let found = files.iter().any(|p| {
            p == &path || p == &canon
                || p.to_string_lossy().contains(stem.as_ref())
        });
        assert!(found,
            "reading_files::read({}) did not include the test tempfile.\n\
             Opened: {:?}\n\
             Returned ({} entries):\n{}",
            pid, path, files.len(),
            files.iter().take(20).map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n"));
    }
}
