"use strict";

// Linux /proc parser. Reads only what we need, swallows races (PID disappearing
// mid-read), and never throws to the caller for missing files — those just
// produce a null record so the collector can skip them.

const fs = require("fs");
const path = require("path");
const os = require("os");

const PROC = "/proc";
const CLK_TCK = 100; // glibc default on Linux. Reading sysconf would require a native module.
const PAGE_SIZE = 4096;

function safeRead(p) {
  try { return fs.readFileSync(p, "utf8"); } catch { return null; }
}

function safeReadlink(p) {
  try { return fs.readlinkSync(p); } catch { return null; }
}

function listPids() {
  let entries;
  try { entries = fs.readdirSync(PROC); } catch { return []; }
  const out = [];
  for (const e of entries) {
    if (e.length === 0) continue;
    const c = e.charCodeAt(0);
    if (c < 48 || c > 57) continue; // not a digit
    out.push(parseInt(e, 10));
  }
  return out;
}

// /proc/[pid]/stat is space-separated but field 2 ("comm") is in parens and
// can itself contain spaces or parens. Parse defensively.
function parseStat(text) {
  if (!text) return null;
  const lp = text.indexOf("(");
  const rp = text.lastIndexOf(")");
  if (lp < 0 || rp < 0 || rp < lp) return null;
  const pid = parseInt(text.slice(0, lp).trim(), 10);
  const comm = text.slice(lp + 1, rp);
  const rest = text.slice(rp + 2).split(" ");
  // Field indices per `man 5 proc`, 0-based after comm:
  //  0=state 1=ppid ... 11=utime 12=stime ... 19=num_threads 20=itrealvalue
  //  21=starttime 22=vsize 23=rss
  return {
    pid,
    comm,
    state: rest[0],
    ppid: parseInt(rest[1], 10),
    utime: parseInt(rest[11], 10) || 0,
    stime: parseInt(rest[12], 10) || 0,
    num_threads: parseInt(rest[17], 10) || 1,
    starttime: parseInt(rest[19], 10) || 0,
    vsize: parseInt(rest[20], 10) || 0,
    rss_pages: parseInt(rest[21], 10) || 0,
  };
}

function readCmdline(pid) {
  const raw = safeRead(path.join(PROC, String(pid), "cmdline"));
  if (raw == null) return "";
  // /proc/PID/cmdline uses NULs as argv separators.
  return raw.replace(/\0+$/g, "").replace(/\0/g, " ");
}

function readCwd(pid) {
  return safeReadlink(path.join(PROC, String(pid), "cwd"));
}

function readExe(pid) {
  return safeReadlink(path.join(PROC, String(pid), "exe"));
}

function readStatus(pid) {
  const text = safeRead(path.join(PROC, String(pid), "status"));
  if (!text) return null;
  const out = {};
  for (const line of text.split("\n")) {
    const idx = line.indexOf(":");
    if (idx < 0) continue;
    out[line.slice(0, idx)] = line.slice(idx + 1).trim();
  }
  return out;
}

function readIo(pid) {
  const text = safeRead(path.join(PROC, String(pid), "io"));
  if (!text) return null;
  const out = {};
  for (const line of text.split("\n")) {
    const idx = line.indexOf(":");
    if (idx < 0) continue;
    out[line.slice(0, idx)] = parseInt(line.slice(idx + 1).trim(), 10) || 0;
  }
  return out;
}

// Files this process currently has open for writing — useful for "directory
// they are writing to". Walks /proc/[pid]/fdinfo and falls back to /proc/[pid]/fd.
function readWritingFiles(pid, limit = 8) {
  const fdinfoDir = path.join(PROC, String(pid), "fdinfo");
  const fdDir = path.join(PROC, String(pid), "fd");
  let fds = [];
  try { fds = fs.readdirSync(fdinfoDir); } catch { return []; }
  const out = [];
  for (const fd of fds) {
    const info = safeRead(path.join(fdinfoDir, fd));
    if (!info) continue;
    const flagsLine = info.split("\n").find(l => l.startsWith("flags:"));
    if (!flagsLine) continue;
    // flags is octal; bit 0x1 = O_WRONLY, 0x2 = O_RDWR
    const flags = parseInt(flagsLine.split(/\s+/)[1], 8) || 0;
    if ((flags & 0x3) === 0) continue; // O_RDONLY
    const tgt = safeReadlink(path.join(fdDir, fd));
    if (!tgt) continue;
    if (tgt.startsWith("/dev/") || tgt.startsWith("pipe:") || tgt.startsWith("socket:") ||
        tgt.startsWith("anon_inode:") || tgt === "/dev/null") continue;
    out.push(tgt);
    if (out.length >= limit) break;
  }
  return out;
}

function readBootTime() {
  const text = safeRead("/proc/stat");
  if (!text) return 0;
  const line = text.split("\n").find(l => l.startsWith("btime "));
  if (!line) return 0;
  return parseInt(line.split(" ")[1], 10) || 0;
}

function readMemInfo() {
  const text = safeRead("/proc/meminfo");
  if (!text) return { total: 0, available: 0 };
  const get = (k) => {
    const line = text.split("\n").find(l => l.startsWith(k + ":"));
    return line ? parseInt(line.split(/\s+/)[1], 10) * 1024 : 0;
  };
  return { total: get("MemTotal"), available: get("MemAvailable") };
}

function readSystemCpuTotal() {
  const text = safeRead("/proc/stat");
  if (!text) return 0;
  const line = text.split("\n").find(l => l.startsWith("cpu "));
  if (!line) return 0;
  const parts = line.split(/\s+/).slice(1).map(n => parseInt(n, 10) || 0);
  return parts.reduce((a, b) => a + b, 0);
}

function isLinux() {
  return os.platform() === "linux" && fs.existsSync(PROC);
}

module.exports = {
  CLK_TCK,
  PAGE_SIZE,
  isLinux,
  listPids,
  parseStat,
  readCmdline,
  readCwd,
  readExe,
  readStatus,
  readIo,
  readWritingFiles,
  readBootTime,
  readMemInfo,
  readSystemCpuTotal,
};
