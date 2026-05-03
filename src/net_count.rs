// Per-process count of established TCP connections, native on each OS.
//
// Surfaces "agent is talking to its API / an MCP server / a network
// peer" in the popup's Background-activity section.  Returns 0 on
// error (and on platforms where we have no implementation yet)
// rather than panicking — net stats are advisory, never load-bearing.

#[cfg(target_os = "linux")]
pub fn established(pid: u32) -> u32 {
    crate::proc_::count_net_established(pid)
}

// ── macOS: libproc PROC_PIDFDSOCKETINFO ────────────────────────────────────
#[cfg(target_os = "macos")]
pub fn established(pid: u32) -> u32 {
    use std::ffi::c_void;
    use std::os::raw::{c_int, c_uint};

    const PROC_PIDLISTFDS:        c_int = 1;
    const PROC_PIDFDSOCKETINFO:   c_int = 3;
    const PROX_FDTYPE_SOCKET:     u32   = 2;
    const SOCKINFO_TCP:           c_int = 2;
    const TSI_S_ESTABLISHED:      c_int = 4;

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

    // socket_fdinfo (Apple's sys/proc_info.h) — only the bytes up
    // through tcpsi_state matter for our count.  Keep the layout
    // opaque-tail to match the kernel-defined size; Apple has
    // grown this struct over OS versions, so we allocate generously
    // (4 KiB buffer) and let proc_pidfdinfo write whatever it needs.
    #[repr(C)]
    struct SocketFdInfo {
        // 12 bytes file-info prefix.
        _pfi: [u8; 24],
        // Remainder is `socket_info`.  The first ~700 bytes contain
        // soi_kind + the tcp_sockinfo union fields we need.
        psi: [u8; 4072],
    }

    let pid = pid as c_int;
    let probe = unsafe { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if probe <= 0 { return 0; }
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
    if written <= 0 { return 0; }
    let got = (written as usize) / std::mem::size_of::<ProcFdInfo>();

    let mut count = 0u32;
    for fd in buf.iter().take(got) {
        if fd.proc_fdtype != PROX_FDTYPE_SOCKET { continue; }
        let mut info: Box<SocketFdInfo> = Box::new(unsafe { std::mem::zeroed() });
        let n = unsafe {
            proc_pidfdinfo(
                pid, fd.proc_fd, PROC_PIDFDSOCKETINFO,
                info.as_mut() as *mut _ as *mut c_void,
                std::mem::size_of::<SocketFdInfo>() as c_int,
            )
        };
        if n <= 0 { continue; }
        // psi[0..4] = soi_kind (i32, little-endian).
        let soi_kind = i32::from_le_bytes([info.psi[0], info.psi[1], info.psi[2], info.psi[3]]);
        if soi_kind != SOCKINFO_TCP { continue; }
        // tcpsi_state lives at offset 392 within socket_info on
        // current macOS (XNU/sys/proc_info.h: sizeof(in_sockinfo)
        // = 392, immediately followed by tcp_sockinfo whose first
        // member is tcpsi_state: i32).  Bounds-check to defend
        // against a kernel version where the offset shifted.
        let off = 392usize;
        if off + 4 > info.psi.len() { continue; }
        let state = i32::from_le_bytes([
            info.psi[off], info.psi[off + 1], info.psi[off + 2], info.psi[off + 3],
        ]);
        if state == TSI_S_ESTABLISHED { count += 1; }
    }
    count
}

// ── Windows: GetExtendedTcpTable filtered to ESTABLISHED + pid ────────────
#[cfg(windows)]
pub fn established(pid: u32) -> u32 {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    const MIB_TCP_STATE_ESTAB: u32 = 5;

    let mut buf_size: u32 = 0;
    // Probe size.
    unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(), &mut buf_size, 0, AF_INET as u32,
            TCP_TABLE_OWNER_PID_CONNECTIONS, 0,
        );
    }
    if buf_size == 0 || buf_size > 16 * 1024 * 1024 { return 0; }

    let mut buf: Vec<u8> = vec![0u8; buf_size as usize];
    let rc = unsafe {
        GetExtendedTcpTable(
            buf.as_mut_ptr() as *mut _, &mut buf_size, 0, AF_INET as u32,
            TCP_TABLE_OWNER_PID_CONNECTIONS, 0,
        )
    };
    if rc != 0 { return 0; }

    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
    let n = table.dwNumEntries as usize;
    let rows: *const MIB_TCPROW_OWNER_PID = &table.table as *const _;

    let mut count = 0u32;
    for i in 0..n {
        let row: &MIB_TCPROW_OWNER_PID = unsafe { &*rows.add(i) };
        if row.dwOwningPid == pid && row.dwState == MIB_TCP_STATE_ESTAB {
            count += 1;
        }
    }
    count
}

// ── FreeBSD / DragonFly: libprocstat sockets ──────────────────────────────
//
// libprocstat exposes per-fd socket info but reading the TCP state
// requires a second-pass struct decode.  Keep the implementation
// minimal — count vnode fds whose `fs_type == PS_FST_TYPE_SOCKET`
// and `fs_typedep` decodes as a TCP/ESTABLISHED sockstat.  Skip
// the second pass for now and return 0; this matches OpenBSD/NetBSD
// behaviour and is documented as a gap on FreeBSD.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly",
          target_os = "openbsd", target_os = "netbsd"))]
pub fn established(_pid: u32) -> u32 { 0 }

// Anything else (illumos, Solaris, Haiku, ...) → 0.
#[cfg(not(any(
    target_os = "linux", target_os = "macos", windows,
    target_os = "freebsd", target_os = "dragonfly",
    target_os = "openbsd", target_os = "netbsd",
)))]
pub fn established(_pid: u32) -> u32 { 0 }
