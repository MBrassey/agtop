// Detect Claude Code plugins enabled for the host.
//
// Claude Code stores plugin marketplaces and per-marketplace plugin
// installs in `~/.claude/plugins/`:
//
//   ~/.claude/settings.json                   {"enabledPlugins":{...}}
//   ~/.claude/plugins/installed_plugins.json  {"plugins":{name@market:[...]}}
//
// We surface the union of (installed AND enabled) plugin names so the
// detail popup can show "plugins: 3 enabled — caveman, frontend-design,
// wakatime" the same way it shows skills.  Plugins are user-global
// (not project-scoped), so this returns the same list for every
// Claude session on the host.
//
// Cheap by design: parse two small JSON files, no fs walks.  Silent
// on any read / parse error — plugins are advisory information, never
// load-bearing for agent monitoring.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// Both JSON files live in `~/.claude/`, a directory writable by the
/// monitored agents themselves — same adversary model as the session
/// files, so they get the same hardened reader (O_NONBLOCK open,
/// regular-file check, bounded read) instead of a raw
/// `read_to_string` that a FIFO could block forever.
const PLUGIN_JSON_CAP: u64 = 1024 * 1024;

/// Return the sorted list of plugin display names (the part before `@`
/// in `name@marketplace`) that are both installed and enabled in the
/// user's Claude Code settings.  Empty if either file is missing /
/// malformed.
pub fn enabled_plugins() -> Vec<String> {
    let home = match dirs::home_dir() { Some(h) => h, None => return Vec::new() };
    let settings_path: PathBuf = home.join(".claude").join("settings.json");
    let installed_path: PathBuf = home.join(".claude").join("plugins").join("installed_plugins.json");

    let enabled: BTreeSet<String> =
        parse_enabled(&crate::readfile::whole_capped(&settings_path, PLUGIN_JSON_CAP));
    let installed: BTreeSet<String> =
        parse_installed(&crate::readfile::whole_capped(&installed_path, PLUGIN_JSON_CAP));

    // Intersect: only surface plugins that are both installed and
    // enabled.  Strip the `@<marketplace>` suffix for display.
    let mut out: BTreeSet<String> = BTreeSet::new();
    for full in enabled.intersection(&installed) {
        let display = full.split('@').next().unwrap_or(full).to_string();
        let clean = crate::format::sanitize_control(&display);
        if !clean.is_empty() { out.insert(clean); }
    }
    out.into_iter().collect()
}

fn parse_enabled(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v, Err(_) => return out,
    };
    let map = match v.get("enabledPlugins").and_then(|x| x.as_object()) {
        Some(m) => m, None => return out,
    };
    for (k, val) in map {
        // Only include `true` entries — `false` means user disabled it.
        if val.as_bool() == Some(true) {
            out.insert(k.to_string());
        }
    }
    out
}

fn parse_installed(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v, Err(_) => return out,
    };
    let map = match v.get("plugins").and_then(|x| x.as_object()) {
        Some(m) => m, None => return out,
    };
    for k in map.keys() {
        out.insert(k.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enabled_map() {
        let s = r#"{"enabledPlugins":{"caveman@caveman":true,"old@m":false}}"#;
        let got = parse_enabled(s);
        assert!(got.contains("caveman@caveman"));
        assert!(!got.contains("old@m"));
    }

    #[test]
    fn parses_installed_map() {
        let s = r#"{"version":2,"plugins":{"caveman@caveman":[{}],"x@y":[{}]}}"#;
        let got = parse_installed(s);
        assert!(got.contains("caveman@caveman"));
        assert!(got.contains("x@y"));
    }

    #[test]
    fn empty_on_missing_keys() {
        assert!(parse_enabled("{}").is_empty());
        assert!(parse_installed("{}").is_empty());
        assert!(parse_enabled("not json").is_empty());
        assert!(parse_installed("not json").is_empty());
    }
}
