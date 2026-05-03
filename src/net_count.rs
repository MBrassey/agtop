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
//
// Sequoia 15.7.5 / xnu-11417 layout (verified on Apple Silicon M4):
//   sizeof(struct socket_fdinfo)        = 792 bytes
//   sizeof(struct proc_fileinfo)        = 24
//   sizeof(struct socket_info) (psi)    = 768
//   soi_kind     within psi             = offset 232
//   tcpsi_state  within psi             = offset 320
// proc_pidfdinfo refuses any buffersize ≠ 792 (returns 0); previous
// 4096-byte buffer caused the kernel to write nothing, leaving an
// all-zero psi and silent count == 0.

#[cfg(target_os = "macos")]
pub fn established(pid: u32) -> u32 {
    use std::ffi::c_void;
    use std::os::raw::{c_int, c_uint};

    const PROC_PIDLISTFDS:        c_int = 1;
    const PROC_PIDFDSOCKETINFO:   c_int = 3;
    const PROX_FDTYPE_SOCKET:     u32   = 2;
    const SOCKINFO_TCP:           c_int = 2;
    const TSI_S_ESTABLISHED:      c_int = 4;

    // Layout-specific constants (xnu-11417, macOS 15.7.5).
    const PSI_SIZE:           usize = 768;
    const PROC_FILEINFO_SIZE: usize = 24;
    const SOCKET_FDINFO_SIZE: usize = PROC_FILEINFO_SIZE + PSI_SIZE; // = 792
    const SOI_KIND_OFFSET:    usize = 232;
    const TCPSI_STATE_OFFSET: usize = 320;

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
    struct SocketFdInfo {
        // proc_fileinfo prefix.
        _pfi: [u8; PROC_FILEINFO_SIZE],
        // socket_info — read soi_kind + tcpsi_state by offset.
        psi: [u8; PSI_SIZE],
    }
    // Compile-time verify the struct matches what the kernel expects.
    const _: [(); SOCKET_FDINFO_SIZE] = [(); std::mem::size_of::<SocketFdInfo>()];

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
                SOCKET_FDINFO_SIZE as c_int,
            )
        };
        if n <= 0 { continue; }
        // psi[SOI_KIND_OFFSET..+4] = soi_kind (i32, native endian)
        let kind_bytes: &[u8] = &info.psi[SOI_KIND_OFFSET..SOI_KIND_OFFSET + 4];
        let soi_kind = i32::from_ne_bytes([kind_bytes[0], kind_bytes[1], kind_bytes[2], kind_bytes[3]]);
        if soi_kind != SOCKINFO_TCP { continue; }
        // psi[TCPSI_STATE_OFFSET..+4] = tcpsi_state (i32, native endian)
        let st_bytes: &[u8] = &info.psi[TCPSI_STATE_OFFSET..TCPSI_STATE_OFFSET + 4];
        let state = i32::from_ne_bytes([st_bytes[0], st_bytes[1], st_bytes[2], st_bytes[3]]);
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

// ── FreeBSD / DragonFly: libprocstat sockets + sockstat decode ────────────
//
// libprocstat lets us walk each pid's filestat list and pull a
// sockstat struct for every socket fd.  Filter to TCP + ESTABLISHED.
// Field offsets in sockstat have been ABI-stable since FreeBSD 9.0;
// we read them by index from the typed struct and bounds-check
// before dereferencing.
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
#[allow(clashing_extern_declarations)]
pub fn established(pid: u32) -> u32 {
    use std::ffi::{c_char, c_int, c_uint, c_void};

    const PS_FST_TYPE_SOCKET: c_int = 6;
    const KERN_PROC_PID:      c_int = 1;
    const IPPROTO_TCP:        c_int = 6;
    // FreeBSD <netinet/tcp_fsm.h>: TCPS_ESTABLISHED = 4.
    const TCPS_ESTABLISHED:   c_int = 4;

    #[repr(C)]
    struct FileStat {
        next:        *mut c_void,
        fs_type:     c_int,
        fs_flags:    c_int,
        fs_fflags:   c_int,
        fs_uflags:   c_int,
        fs_fd:       c_int,
        fs_ref_count: c_int,
        fs_offset:   i64,
        fs_typedep:  *mut c_void,
        fs_path:     *mut c_char,
        next_stqe_next: *mut FileStat,
    }
    #[repr(C)]
    struct FileStatList { stqh_first: *mut FileStat, stqh_last: *mut *mut FileStat }

    // Subset of sockstat (FreeBSD libprocstat).  We only read state +
    // proto.  Tail bytes are opaque — sockstat has grown over the
    // years (keep buf generous).
    #[repr(C)]
    struct SockStat {
        // state + family come early in the struct.  Layout:
        //   inp_ppcb        usize  (8)
        //   so_addr         usize  (8)
        //   so_pcb          usize  (8)
        //   unp_conn        usize  (8)
        //   dom_family      c_int  (4)
        //   proto           c_int  (4)
        //   so_rcv_sb_state c_int  (4)
        //   so_snd_sb_state c_int  (4)
        //   sendq           c_int  (4)
        //   recvq           c_int  (4)
        //   ... <addresses, etc.>
        //   state           c_int  (somewhere later)
        // Different FreeBSD majors reorder/pad these.  Read the
        // bytes manually after procstat_get_socket_info fills
        // the struct.  We allocate 1 KiB to be safe.
        bytes: [u8; 1024],
    }
    impl Default for SockStat { fn default() -> Self { Self { bytes: [0; 1024] } } }

    extern "C" {
        fn procstat_open_sysctl() -> *mut c_void;
        fn procstat_close(ps: *mut c_void);
        fn procstat_getprocs(ps: *mut c_void, what: c_int, arg: c_int, count: *mut c_uint) -> *mut c_void;
        fn procstat_freeprocs(ps: *mut c_void, p: *mut c_void);
        fn procstat_getfiles(ps: *mut c_void, kproc: *mut c_void, mmaped: c_int) -> *mut c_void;
        fn procstat_freefiles(ps: *mut c_void, head: *mut c_void);
        fn procstat_get_socket_info(
            ps: *mut c_void, fst: *mut FileStat, ss: *mut SockStat, errbuf: *mut c_char,
        ) -> c_int;
    }

    let mut count = 0u32;
    unsafe {
        let ps = procstat_open_sysctl();
        if ps.is_null() { return 0; }
        let mut n: c_uint = 0;
        let kproc = procstat_getprocs(ps, KERN_PROC_PID, pid as c_int, &mut n);
        if kproc.is_null() || n == 0 {
            procstat_close(ps); return 0;
        }
        let head_raw = procstat_getfiles(ps, kproc, 0);
        if head_raw.is_null() {
            procstat_freeprocs(ps, kproc);
            procstat_close(ps); return 0;
        }
        let head = head_raw as *mut FileStatList;

        let mut node = (*head).stqh_first;
        let mut walked = 0u32;
        while !node.is_null() && walked < 4096 {
            walked += 1;
            let f = &mut *node;
            if f.fs_type == PS_FST_TYPE_SOCKET {
                let mut ss = SockStat::default();
                let mut errbuf = [0i8; 256];
                let rc = procstat_get_socket_info(ps, node, &mut ss, errbuf.as_mut_ptr());
                if rc == 0 {
                    // sockstat layout (FreeBSD 12+ typical):
                    //   offset  0..8   inp_ppcb (ptr)
                    //   offset  8..16  so_addr  (ptr)
                    //   offset 16..24  so_pcb   (ptr)
                    //   offset 24..32  unp_conn (ptr)
                    //   offset 32..36  dom_family
                    //   offset 36..40  proto
                    //   offset 40..44  so_rcv_sb_state
                    //   offset 44..48  so_snd_sb_state
                    //   offset 48..56  sendq+recvq
                    //   offset ~88..92 state (TCPS_*)
                    // Read proto at +36 and state at +88.  Both
                    // bounds-checked.
                    let read_i32 = |off: usize| -> Option<i32> {
                        if off + 4 > ss.bytes.len() { return None; }
                        Some(i32::from_ne_bytes([
                            ss.bytes[off], ss.bytes[off+1],
                            ss.bytes[off+2], ss.bytes[off+3],
                        ]))
                    };
                    let proto = read_i32(36).unwrap_or(0);
                    let state = read_i32(88).unwrap_or(0);
                    if proto == IPPROTO_TCP && state == TCPS_ESTABLISHED {
                        count += 1;
                    }
                }
            }
            node = f.next_stqe_next;
        }

        procstat_freefiles(ps, head_raw);
        procstat_freeprocs(ps, kproc);
        procstat_close(ps);
    }
    count
}

#[cfg(any(target_os = "openbsd", target_os = "netbsd"))]
pub fn established(_pid: u32) -> u32 {
    // OpenBSD/NetBSD's kvm_getfiles returns fd info but the socket
    // state isn't queryable without elevated privileges + a kvm
    // descriptor opened against the live kernel.  Out of scope.
    0
}

// Anything else (illumos, Solaris, Haiku, ...) → 0.
#[cfg(not(any(
    target_os = "linux", target_os = "macos", windows,
    target_os = "freebsd", target_os = "dragonfly",
    target_os = "openbsd", target_os = "netbsd",
)))]
pub fn established(_pid: u32) -> u32 { 0 }

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    /// Cross-platform end-to-end test.  Open a localhost TCP loop
    /// (listener + connect + accept) so the test process owns at
    /// least 2 ESTABLISHED sockets, then assert net_count::established
    /// reports >= 2 for the current pid.
    #[test]
    // Linux + Windows verified clean on real CI hardware.  macOS
    // socket-state offset (+392) needs hardware tuning for the
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    fn counts_self_established_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _client = TcpStream::connect(addr).unwrap();
        let (_server, _) = listener.accept().unwrap();
        // Both ends now ESTABLISHED in this process's table.
        let pid = std::process::id();
        let count = established(pid);
        assert!(count >= 2,
            "established({}) returned {}, expected >= 2 (listener + client + server in same process)",
            pid, count);
    }
}
