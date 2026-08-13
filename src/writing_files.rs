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
///
/// On Windows the call runs on a single long-lived watchdog thread
/// with a 2s deadline: `GetFinalPathNameByHandleW` can stall on
/// remote/SMB handles even after the FILE_TYPE_DISK pre-filter, and a
/// stuck collector tick would freeze the entire TUI.  A persistently
/// stalled handle keeps getting re-enumerated every tick, so the
/// watchdog must not spawn a fresh thread per call — while the worker
/// is still wedged on an earlier sweep, subsequent calls skip
/// immediately instead of stacking abandoned threads without bound.
#[cfg(windows)]
pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
    use std::sync::OnceLock;
    static DOG: OnceLock<watchdog::Watchdog> = OnceLock::new();
    DOG.get_or_init(watchdog::Watchdog::spawn)
        .run(std::time::Duration::from_secs(2), move || impl_::read(pid, limit))
}
#[cfg(not(windows))]
pub fn read(pid: u32, limit: usize) -> Vec<PathBuf> {
    impl_::read(pid, limit)
}

/// Single-worker deadline runner.  One dedicated thread executes jobs
/// serially; a job that overruns its deadline is abandoned (the caller
/// gets an empty result) but the thread is NOT replaced — further
/// requests are refused until the worker comes back, bounding thread
/// use at one regardless of how long a handle stays wedged.  Late
/// results from abandoned jobs are discarded by sequence number.
///
/// Compiled on all targets so the scheduling logic is covered by tests
/// on any host; only the Windows `read` wrapper routes through it.
#[cfg(any(windows, test))]
pub(crate) mod watchdog {
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    type Job = Box<dyn FnOnce() -> Vec<PathBuf> + Send + 'static>;

    pub struct Watchdog {
        inner: Mutex<Inner>,
    }
    struct Inner {
        req_tx: SyncSender<(u64, Job)>,
        res_rx: Receiver<(u64, Vec<PathBuf>)>,
        seq: u64,
    }

    impl Watchdog {
        pub fn spawn() -> Self {
            // Rendezvous request channel: try_send succeeds only while
            // the worker is parked in recv — i.e. not wedged in a job.
            let (req_tx, req_rx) = sync_channel::<(u64, Job)>(0);
            let (res_tx, res_rx) = channel::<(u64, Vec<PathBuf>)>();
            std::thread::Builder::new()
                .name("agtop-fd-watchdog".into())
                .spawn(move || {
                    while let Ok((seq, job)) = req_rx.recv() {
                        let out = job();
                        if res_tx.send((seq, out)).is_err() { break; }
                    }
                })
                .expect("spawn agtop-fd-watchdog thread");
            Watchdog { inner: Mutex::new(Inner { req_tx, res_rx, seq: 0 }) }
        }

        pub fn run(
            &self,
            timeout: Duration,
            job: impl FnOnce() -> Vec<PathBuf> + Send + 'static,
        ) -> Vec<PathBuf> {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.seq += 1;
            let seq = inner.seq;
            // Rendezvous try_send only lands while the worker is parked in
            // recv.  An idle worker parks within microseconds (retry over a
            // ~20ms window covers the just-spawned race); a worker wedged
            // inside a job keeps the channel Full past the window, and the
            // call is refused — skip this tick rather than queueing behind
            // a dead handle.
            let send_deadline = Instant::now() + Duration::from_millis(20);
            let mut pending: (u64, Job) = (seq, Box::new(job));
            loop {
                use std::sync::mpsc::TrySendError;
                match inner.req_tx.try_send(pending) {
                    Ok(()) => break,
                    Err(TrySendError::Full(p)) => {
                        if Instant::now() >= send_deadline { return Vec::new(); }
                        pending = p;
                        std::thread::yield_now();
                    }
                    Err(TrySendError::Disconnected(_)) => return Vec::new(),
                }
            }
            let deadline = Instant::now() + timeout;
            loop {
                let now = Instant::now();
                if now >= deadline { return Vec::new(); }
                match inner.res_rx.recv_timeout(deadline - now) {
                    Ok((s, v)) if s == seq => return v,
                    Ok(_) => continue, // stale result from an abandoned call
                    Err(_) => return Vec::new(),
                }
            }
        }
    }
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
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_int, c_uint};

    // Apple's libproc.h flavor constants (stable since 10.5).
    const PROC_PIDLISTFDS:           c_int = 1;
    const PROC_PIDFDVNODEPATHINFO:   c_int = 2;
    const PROX_FDTYPE_VNODE:         u32   = 1;

    // libSystem.dylib symbols.  Available on every macOS without an
    // explicit `link` directive — the dynamic linker resolves them.
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

    /// Subset of `proc_fileinfo` from sys/proc_info.h.  We only read
    /// `fi_openflags`; the surrounding fields are present so the
    /// struct size matches what proc_pidfdinfo writes.
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
    struct VnodeInfo {
        // 152-byte stat block; we don't read any of it.
        _opaque: [u8; 152],
    }
    impl Default for VnodeInfo {
        fn default() -> Self { Self { _opaque: [0u8; 152] } }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct VnodeInfoPath {
        vip_vi:   VnodeInfo,
        vip_path: [c_char; 1024],   // MAXPATHLEN, NUL-terminated
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

        // Step 1: probe how big the FD list is.
        let probe = unsafe {
            proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0)
        };
        if probe <= 0 { return Vec::new(); }
        // Cap at 4096 entries to avoid a runaway alloc on a process
        // with millions of fds.
        let needed = (probe as usize).min(4096 * std::mem::size_of::<ProcFdInfo>());
        let entry_count = needed / std::mem::size_of::<ProcFdInfo>();
        let mut buf: Vec<ProcFdInfo> = vec![ProcFdInfo::default(); entry_count];

        // Step 2: fetch the actual FD list.
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

            // Step 3: per-FD path + flags.
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
            // (<sys/file.h>: FREAD=1, FWRITE=2), NOT POSIX O_RDONLY/
            // O_WRONLY/O_RDWR.  The previous mask (libc::O_WRONLY |
            // libc::O_RDWR) coincidentally matched FREAD|FWRITE bits
            // and worked-by-accident for writable detection.  Make the
            // semantics explicit: include when FWRITE is set.
            const FWRITE: u32 = 0x2;
            let flags: u32 = info.pfi.fi_openflags;
            if (flags & FWRITE) == 0 { continue; }

            // vip_path is NUL-terminated c_char (i8 on Apple).
            let path = &info.pvip.vip_path;
            let nul = path.iter().position(|&b| b == 0).unwrap_or(path.len());
            let bytes: Vec<u8> = path[..nul].iter().map(|&c| c as u8).collect();
            let s = match std::str::from_utf8(&bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if s.is_empty() || s.starts_with("/dev/") { continue; }
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
        GetFileType, GetFinalPathNameByHandleW,
        FILE_NAME_NORMALIZED, FILE_TYPE_DISK,
    };

    // The actual data-write access bits (ABI-fixed).  FILE_GENERIC_WRITE
    // (0x120116) is the WRONG mask to test for "is this handle open for
    // writing": it also contains SYNCHRONIZE (0x100000) and READ_CONTROL
    // (0x20000), which every File handle — including read-only handles and
    // directory (FILE_LIST_DIRECTORY) handles — carries, so `& FILE_GENERIC_WRITE`
    // matched essentially everything and the "writing files" panel showed
    // read-only handles.  The reading side documents this exact fix.
    const FILE_WRITE_DATA:  u32 = 0x0002;
    const FILE_APPEND_DATA: u32 = 0x0004;
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
        // Bounded MISMATCH-retry loop: at most 8 grow iterations
        // before we bail.  Defensive against a kernel that keeps
        // reporting MISMATCH with `needed == 0` (would otherwise
        // doubling-spin until the 64 MiB cap, wasting allocator
        // time on each tick).
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
        // Validate the kernel-supplied handle count actually fits in
        // the buffer we allocated.  A racing handle-table mutation
        // between the MISMATCH probe and the populated call could
        // otherwise let us read past `buf.len()` (out-of-bounds in
        // unsafe code).  Clamp `count` to whatever the buffer can
        // hold; better to under-report than to read garbage.
        let header_size = std::mem::size_of::<usize>() * 2;
        let entry_size  = std::mem::size_of::<SystemHandleTableEntryInfoEx>();
        let max_entries = buf.len().saturating_sub(header_size) / entry_size;
        let count = header.number_of_handles.min(max_entries);
        let entries: *const SystemHandleTableEntryInfoEx = unsafe {
            (table as *const u8)
                .add(header_size)
                as *const SystemHandleTableEntryInfoEx
        };

        let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
        let me = unsafe { GetCurrentProcess() };

        for i in 0..count {
            if out.len() >= limit { break; }
            let entry = unsafe { *entries.add(i) };
            if entry.unique_process_id != target_pid { continue; }
            // Match only handles actually granted data-write or append
            // access — not the composite FILE_GENERIC_WRITE, which shares
            // SYNCHRONIZE/READ_CONTROL with every read-only handle.
            if entry.granted_access & (FILE_WRITE_DATA | FILE_APPEND_DATA) == 0 {
                continue;
            }

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

            // Pre-filter to disk-backed handles only.  GetFinalPathNameByHandleW
            // can block indefinitely when called on a pipe/mailslot/socket
            // handle whose other end is a service that never responds —
            // this previously hung agtop's Windows CI for the full 6h max
            // execution budget.  GetFileType is non-blocking and reliably
            // distinguishes disk files (FILE_TYPE_DISK) from pipes / char
            // devices / network endpoints.
            let ftype = unsafe { GetFileType(dup) };
            if ftype != FILE_TYPE_DISK {
                unsafe { CloseHandle(dup); }
                continue;
            }

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

// ── FreeBSD / DragonFly: libprocstat ───────────────────────────────────────
//
// FreeBSD ships libprocstat (since 9.x) which gives us proper per-fd
// path resolution — the same data `fstat -p <pid>` reports.  DragonFly
// has a compatible procstat fork.
//
// OpenBSD / NetBSD don't track per-fd paths in the kernel (their
// kvm_getfiles returns inode + dev only, no path), so FD enumeration
// there can return numbers but not paths — almost useless for agtop's
// "writing files" surface.  Falls through to the empty-Vec stub.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
#[allow(clashing_extern_declarations)]
mod impl_ {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

    // sys/user.h — type tags for libprocstat's filestat.fs_type.
    const PS_FST_TYPE_VNODE:  c_int = 1;
    // sys/user.h — bits in filestat.fs_flags.
    const PS_FST_FFLAG_WRITE: c_int = 0x0002;
    // sys/sysctl.h — what selector for procstat_getprocs.
    const KERN_PROC_PID:      c_int = 1;

    /// Mirror of `struct filestat` from <libprocstat.h>.  Layout has
    /// been stable since FreeBSD 9.0 — fs_path's offset specifically
    /// is part of the public ABI surface.  We only read scalar fields
    /// and the path pointer; we never construct or modify one.
    #[repr(C)]
    struct FileStat {
        fs_type:       c_int,
        fs_flags:      c_int,
        fs_fflags:     c_int,
        fs_uflags:     c_int,
        fs_fd:         c_int,
        fs_ref_count:  c_int,
        fs_offset:     i64,
        fs_typedep:    *mut c_void,
        fs_path:       *mut c_char,
        // STAILQ_ENTRY(filestat) — next pointer at the tail.
        next_stqe_next: *mut FileStat,
    }

    /// Mirror of the STAILQ_HEAD that procstat_getfiles returns.
    #[repr(C)]
    struct FileStatList {
        stqh_first: *mut FileStat,
        stqh_last:  *mut *mut FileStat,
    }

    // libprocstat symbols.  kinfo_proc is opaque to us — we never
    // dereference it, just thread the pointer through the API.
    #[link(name = "procstat")]
    extern "C" {
        fn procstat_open_sysctl() -> *mut c_void;
        fn procstat_close(ps: *mut c_void);
        fn procstat_getprocs(
            ps: *mut c_void, what: c_int, arg: c_int, count: *mut c_uint,
        ) -> *mut c_void;       // returns kinfo_proc *
        fn procstat_freeprocs(ps: *mut c_void, p: *mut c_void);
        fn procstat_getfiles(
            ps: *mut c_void, p: *mut c_void, mmapped: c_int,
        ) -> *mut FileStatList;
        fn procstat_freefiles(ps: *mut c_void, head: *mut FileStatList);
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

            let head = procstat_getfiles(ps, kproc, 0);
            if head.is_null() {
                procstat_freeprocs(ps, kproc);
                procstat_close(ps);
                return Vec::new();
            }

            let mut out: Vec<PathBuf> = Vec::with_capacity(limit);
            let mut node = (*head).stqh_first;
            // Hard iteration cap.  Defends against a kernel /
            // libprocstat ABI drift that places `next.stqe_next`
            // at a different offset than this binding assumes —
            // without this, a bad layout could walk arbitrary
            // memory until segfault.  4096 entries is far more
            // than any realistic process opens.
            let mut walked = 0u32;
            while !node.is_null() && walked < 4096 {
                walked += 1;
                if out.len() >= limit { break; }
                let f = &*node;
                // Vnode entries are the only ones that carry a real
                // filesystem path; pipes / sockets / kqueues have null
                // fs_path even when fs_flags includes WRITE.
                let writable = (f.fs_flags & PS_FST_FFLAG_WRITE) != 0;
                let is_vnode = f.fs_type == PS_FST_TYPE_VNODE;
                if writable && is_vnode && !f.fs_path.is_null() {
                    let cstr = CStr::from_ptr(f.fs_path);
                    if let Ok(s) = cstr.to_str() {
                        if !s.is_empty() && !s.starts_with("/dev/") {
                            out.push(PathBuf::from(s));
                        }
                    }
                }
                node = f.next_stqe_next;
            }

            procstat_freefiles(ps, head);
            procstat_freeprocs(ps, kproc);
            procstat_close(ps);
            out
        }
    }
}

// ── OpenBSD / NetBSD: stub ─────────────────────────────────────────────────
//
// kvm_getfiles returns inode + dev but no path; reconstructing paths
// would require walking every mounted filesystem and reverse-mapping
// inodes, which is both expensive and unreliable.  Out of scope.
#[cfg(any(target_os = "openbsd", target_os = "netbsd"))]
mod impl_ {
    use super::*;
    pub fn read(_pid: u32, _limit: usize) -> Vec<PathBuf> { Vec::new() }
}

// ── Anything else (illumos, Solaris, Haiku, ...) ───────────────────────────
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
mod watchdog_tests {
    use super::watchdog::Watchdog;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n).map(|i| PathBuf::from(format!("/f{i}"))).collect()
    }

    #[test]
    fn fast_job_returns_result() {
        let dog = Watchdog::spawn();
        assert_eq!(dog.run(Duration::from_secs(2), || paths(3)), paths(3));
    }

    #[test]
    fn wedged_worker_is_skipped_not_stacked() {
        let dog = Watchdog::spawn();
        // Wedge the worker well past this test's lifetime budget.
        let t0 = Instant::now();
        let out = dog.run(Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_millis(600));
            paths(1)
        });
        assert!(out.is_empty(), "overrun job must yield empty");
        assert!(t0.elapsed() < Duration::from_millis(500),
            "caller must be released at the deadline");
        // While the worker is still inside the sleeping job, further
        // requests are refused immediately — no second thread, no queue.
        let t1 = Instant::now();
        assert!(dog.run(Duration::from_secs(2), || paths(2)).is_empty());
        assert!(t1.elapsed() < Duration::from_millis(200),
            "busy worker must refuse instantly, not block");
        // Once the worker finishes the abandoned job, service resumes and
        // the stale result is discarded rather than returned to a new call.
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(dog.run(Duration::from_secs(2), || paths(5)), paths(5));
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
