// Curated list of known AI coding-agent CLIs. Each entry maps a label
// (canonical "name" column) to a regex matched against the full command line.
// Order matters: first match wins.

use regex::Regex;

pub struct Matcher {
    pub label: &'static str,
    pub re: Regex,
}

pub struct UserMatcher {
    pub label: String,
    pub re: Regex,
}

pub fn builtin() -> Vec<Matcher> {
    // word-boundary prefix: start, forward slash, backslash (Windows
    // paths), or whitespace.
    const P: &str = r"(^|[\s/\\])";
    // Trailing word-boundary: whitespace, end, or a Windows shim
    // suffix (.exe / .cmd / .ps1 / .bat). On Windows, npm-installed
    // CLIs are exposed as `<name>.cmd` shims and the bare exe shows
    // up as `<name>.exe`; without these the cmdline `claude.exe` or
    // `claude.cmd --print` would never match `claude(\s|$)`.
    const E: &str = r"(\.(exe|cmd|ps1|bat))?(\s|$)";
    let m = |label: &'static str, body: &str| Matcher {
        label,
        re: Regex::new(body).expect("builtin regex"),
    };
    let p = |s: &str| format!("{P}{s}");
    vec![
        m("claude",       &p(&format!(r"claude(-code)?{E}"))),
        // Scoped npm package paths: forward slash on Linux/macOS, but
        // backslash on Windows (`...\node_modules\@anthropic-ai\claude-code\cli.js`).
        // Same for the other scoped agents below.
        m("claude-code",  r"@anthropic-ai[/\\]claude-code"),
        m("codex",        &p(&format!(r"codex{E}"))),
        m("openai-codex", r"@openai[/\\]codex"),
        m("aider",        &p(r"aider(\s|$|\.)")),
        m("cursor-agent", &p(&format!(r"cursor-agent{E}"))),
        m("gemini",       &p(&format!(r"gemini(-cli)?{E}"))),
        m("goose",        &p(&format!(r"goose{E}"))),
        m("continue",     &p(&format!(r"continue(-cli|-agent)?{E}"))),
        m("opencode",     &p(&format!(r"opencode{E}"))),
        m("copilot",      r"gh[\s-]copilot|github-copilot-cli"),
        m("cody",         &p(&format!(r"cody{E}"))),
        m("amp",          r"(^|[\s/\\])amp(\.(exe|cmd|ps1|bat))?(\s|$)|@sourcegraph[/\\]amp"),
        m("crush",        &p(&format!(r"crush{E}"))),
        m("mods",         &p(&format!(r"mods{E}"))),
        m("sgpt",         &p(&format!(r"sgpt{E}"))),
        m("llm",          &p(&format!(r"llm{E}"))),
        m("ollama",       &p(r"ollama(\s+(run|chat|serve)|$)")),
        m("fabric",       &p(&format!(r"fabric{E}"))),
        m("block-goose",  &p(&format!(r"goose-server{E}"))),
    ]
}

pub fn parse_user_matchers(extra: &[String]) -> Vec<UserMatcher> {
    let mut out = Vec::new();
    for spec in extra {
        if let Some((label, pat)) = spec.split_once('=') {
            let label = label.trim().to_string();
            let pat = pat.trim();
            if label.is_empty() || pat.is_empty() {
                continue;
            }
            // Cap regex size to defuse pathological user patterns; without
            // these limits a megabyte-NFA `--match` could OOM the binary.
            let built = regex::RegexBuilder::new(pat)
                .size_limit(1_000_000)
                .dfa_size_limit(1_000_000)
                .build();
            if let Ok(re) = built {
                out.push(UserMatcher { label, re });
            }
        }
    }
    out
}

pub fn classify<'a>(
    cmdline: &str,
    builtins: &'a [Matcher],
    user: &'a [UserMatcher],
) -> Option<&'a str> {
    if cmdline.is_empty() {
        return None;
    }
    // ReDoS defense: cap the regex match input at 16 KiB.  Real
    // agent cmdlines are well under 1 KiB; a hostile co-tenant
    // process with megabyte-scale argv combined with a pathological
    // user-supplied `-m` regex could otherwise spike CPU per tick.
    // 16 KiB is comfortably above any realistic agent invocation.
    const MAX_MATCH_BYTES: usize = 16 * 1024;
    let trimmed = if cmdline.len() > MAX_MATCH_BYTES {
        // Slice to the closest valid utf-8 boundary at or below
        // the cap so regex doesn't see a half-byte sequence.
        let mut end = MAX_MATCH_BYTES;
        while end > 0 && !cmdline.is_char_boundary(end) { end -= 1; }
        &cmdline[..end]
    } else {
        cmdline
    };
    for m in builtins {
        if m.re.is_match(trimmed) {
            return Some(m.label);
        }
    }
    for m in user {
        if m.re.is_match(trimmed) {
            return Some(m.label.as_str());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_agents() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        assert_eq!(classify("/usr/bin/claude --resume", &b, &u), Some("claude"));
        assert_eq!(classify("node /opt/codex/bin/codex chat", &b, &u), Some("codex"));
        assert_eq!(classify("python -m aider --no-git", &b, &u), Some("aider"));
        assert_eq!(classify("/usr/bin/cursor-agent --watch", &b, &u), Some("cursor-agent"));
        assert_eq!(classify("/usr/bin/bash", &b, &u), None);
    }

    #[test]
    fn user_matchers() {
        let b = builtin();
        let u = parse_user_matchers(&["myagent=python.*my_agent\\.py".to_string()]);
        assert_eq!(classify("python /home/x/my_agent.py --foo", &b, &u), Some("myagent"));
        // Builtin wins on its pattern.
        assert_eq!(classify("/usr/bin/claude", &b, &u), Some("claude"));
    }

    #[test]
    fn empty_cmdline_returns_none() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        assert_eq!(classify("", &b, &u), None);
    }

    // Windows paths use backslash separators and CLI shims expose the
    // tool as `<name>.cmd` / `<name>.exe`. Pre-2.4.x these were silent
    // misses and produced an empty Agents pane on Windows even when
    // Claude/Codex were running — see dist/jakeagtop.png for the
    // user-reported repro. Lock the regression in.
    #[test]
    fn windows_npm_global_paths() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        // npm-on-Windows global install: claude-code via node.exe shim.
        assert_eq!(
            classify(
                r"C:\Program Files\nodejs\node.exe C:\Users\jake\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\cli.js",
                &b, &u),
            Some("claude-code"));
        assert_eq!(
            classify(
                r"node.exe C:\Users\jake\AppData\Roaming\npm\node_modules\@openai\codex\dist\cli.js chat",
                &b, &u),
            Some("openai-codex"));
        assert_eq!(
            classify(
                r"node C:\Users\jake\AppData\Roaming\npm\node_modules\@sourcegraph\amp\bin\amp.js",
                &b, &u),
            Some("amp"));
    }

    #[test]
    fn windows_cmd_and_exe_shims() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        assert_eq!(classify(r"C:\Users\jake\AppData\Roaming\npm\claude.cmd --print", &b, &u), Some("claude"));
        assert_eq!(classify(r"C:\Users\jake\AppData\Roaming\npm\codex.cmd chat", &b, &u), Some("codex"));
        assert_eq!(classify(r"C:\bin\claude.exe", &b, &u), Some("claude"));
        assert_eq!(classify(r"C:\bin\gemini.exe --interactive", &b, &u), Some("gemini"));
        assert_eq!(classify(r"goose.exe session", &b, &u), Some("goose"));
    }
}
