// Shared session-enricher types. Each per-vendor module (claude, codex,
// generic, ...) implements `summarise` returning the same SessionsResult so
// the collector can merge them uniformly.

use crate::model::{Session, Sessions};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub struct LiveAgentRef<'a> {
    pub pid: u32,
    pub cwd: &'a str,
    pub label: &'a str,
}

pub struct SessionsResult {
    pub sessions: Sessions,
    pub by_pid: HashMap<u32, Session>,
}

impl SessionsResult {
    pub fn empty() -> Self {
        Self { sessions: Sessions::default(), by_pid: HashMap::new() }
    }
}

/// Merge the results of multiple per-vendor enrichers into a single one.
/// `by_pid` resolves first-wins (the order of enrichers passed in matters);
/// the `sessions` block aggregates session lists and recent-task lists, and
/// recomputes the count fields.
pub fn merge(parts: Vec<SessionsResult>) -> SessionsResult {
    use crate::model::Status;
    let mut by_pid: HashMap<u32, Session> = HashMap::new();
    let mut all_sessions: Vec<Session> = Vec::new();
    let mut all_recent_tasks: Vec<crate::model::RecentTask> = Vec::new();
    for p in parts {
        for (pid, sess) in p.by_pid {
            by_pid.entry(pid).or_insert(sess);
        }
        all_sessions.extend(p.sessions.sessions);
        all_recent_tasks.extend(p.sessions.recent_tasks);
    }
    all_sessions.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    all_recent_tasks.sort_by(|a, b| b.mtime_ms.cmp(&a.mtime_ms));
    if all_recent_tasks.len() > 30 { all_recent_tasks.truncate(30); }

    let waiting   = all_sessions.iter().filter(|s| s.status == Status::Waiting).count() as u32;
    let completed = all_sessions.iter().filter(|s| s.status == Status::Completed).count() as u32;
    let active    = all_sessions.iter()
        .filter(|s| matches!(s.status, Status::Active | Status::Busy | Status::Spawning | Status::Idle))
        .count() as u32;
    let busy      = all_sessions.iter()
        .filter(|s| matches!(s.status, Status::Busy | Status::Spawning))
        .count() as u32;

    SessionsResult {
        sessions: Sessions {
            sessions: all_sessions,
            recent_tasks: all_recent_tasks,
            active, busy, waiting, completed,
        },
        by_pid,
    }
}
