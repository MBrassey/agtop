// Curated list of known AI coding-agent CLIs. Each entry maps a label
// (canonical "name" column) to a regex. Bare-name matchers run against the
// command *head* (argv[0], plus an interpreter's script/module target) so a
// bystander that merely mentions an agent in its arguments (`grep -r
// claude .`, `journalctl -u codex`) is never classified as one; matchers
// flagged `wide` (scoped npm package paths) still see the full command line.
// Order matters: first match wins.

use regex::Regex;

pub struct Matcher {
    pub label: &'static str,
    pub re: Regex,
    /// Match against the full command line instead of the command head —
    /// needed for package-path matchers (`.../@openai/codex/cli.js`)
    /// where an interpreter is argv[0] and the path may fall anywhere.
    pub wide: bool,
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
        wide: false,
    };
    let w = |label: &'static str, body: &str| Matcher {
        label,
        re: Regex::new(body).expect("builtin regex"),
        wide: true,
    };
    let p = |s: &str| format!("{P}{s}");
    vec![
        m("claude",       &p(&format!(r"claude(-code)?{E}"))),
        // Scoped npm package paths: forward slash on Linux/macOS, but
        // backslash on Windows (`...\node_modules\@anthropic-ai\claude-code\cli.js`).
        // Same for the other scoped agents below.
        w("claude-code",  r"@anthropic-ai[/\\]claude-code"),
        m("codex",        &p(&format!(r"codex{E}"))),
        w("openai-codex", r"@openai[/\\]codex"),
        m("aider",        &p(r"aider(\s|$|\.)")),
        m("cursor-agent", &p(&format!(r"cursor-agent{E}"))),
        m("gemini",       &p(&format!(r"gemini(-cli)?{E}"))),
        // Gemini CLI's primary distribution is `npm i -g @google/gemini-cli`.
        // On Windows the cmdline is the resolved package path
        // (`...\node_modules\@google\gemini-cli\dist\index.js`), where
        // "gemini-cli" is followed by `\` so the bare matcher's trailing
        // word-boundary fails — the agent was invisible.  Match the scoped
        // path like the other npm-only agents.
        w("gemini-cli",   r"@google[/\\]gemini-cli"),
        m("goose",        &p(&format!(r"goose{E}"))),
        m("continue",     &p(&format!(r"continue(-cli|-agent)?{E}"))),
        m("opencode",     &p(&format!(r"opencode{E}"))),
        m("copilot",      r"gh[\s-]copilot|github-copilot-cli"),
        m("cody",         &p(&format!(r"cody{E}"))),
        m("amp",          &p(&format!(r"amp{E}"))),
        w("amp",          r"@sourcegraph[/\\]amp"),
        m("crush",        &p(&format!(r"crush{E}"))),
        m("mods",         &p(&format!(r"mods{E}"))),
        m("sgpt",         &p(&format!(r"sgpt{E}"))),
        m("llm",          &p(&format!(r"llm{E}"))),
        m("ollama",       &p(r"ollama(\s+(run|chat|serve)|$)")),
        m("fabric",       &p(&format!(r"fabric{E}"))),
        m("block-goose",  &p(&format!(r"goose-server{E}"))),
    ]
}

/// The classification-relevant head of a command line: argv[0]; plus the
/// script/module target when argv[0] is an interpreter (`node .../codex`,
/// `python -m aider`); plus the first subcommand for launchers whose
/// agent-ness depends on it (`gh copilot`, `ollama run`).  Bare-name
/// matchers run against this instead of the whole cmdline, so agent names
/// appearing only in a process's *arguments* never classify it.
fn match_head(cmdline: &str) -> String {
    let mut toks = cmdline.split_whitespace();
    let argv0 = match toks.next() { Some(t) => t, None => return String::new() };
    let mut base = argv0.rsplit(['/', '\\']).next().unwrap_or(argv0).to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".ps1", ".bat"] {
        if let Some(s) = base.strip_suffix(ext) {
            base = s.to_string();
            break;
        }
    }
    let mut head = argv0.to_string();
    let is_interp = base.starts_with("python")
        || matches!(base.as_str(),
            "node" | "nodejs" | "bun" | "deno" | "ruby" | "perl"
            | "sh" | "bash" | "zsh" | "dash" | "fish");
    if is_interp {
        // First non-flag argument is the script path; `-m mod` (python)
        // and `-c cmd` (shells) name the target in the next token.
        let mut take_next = false;
        for t in toks {
            if take_next || !t.starts_with('-') {
                head.push(' ');
                head.push_str(t);
                break;
            }
            if t == "-m" || t == "-c" { take_next = true; }
        }
    } else if base == "gh" || base == "ollama" {
        if let Some(t) = toks.find(|t| !t.starts_with('-')) {
            head.push(' ');
            head.push_str(t);
        }
    }
    head
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
    let head = match_head(trimmed);
    for m in builtins {
        let hay = if m.wide { trimmed } else { head.as_str() };
        if m.re.is_match(hay) {
            return Some(m.label);
        }
    }
    // User matchers keep the documented contract: regex over the full
    // command line.
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
        // Gemini CLI is npm-only; on Windows the resolved package path is
        // what shows up and the bare `gemini-cli\` boundary fails without
        // the scoped matcher.
        assert_eq!(
            classify(
                r"node.exe C:\Users\jake\AppData\Roaming\npm\node_modules\@google\gemini-cli\dist\index.js",
                &b, &u),
            Some("gemini-cli"));
    }

    // Agent names in a process's *arguments* must not classify it — a
    // `grep claude` run inside a project directory used to become a Busy
    // "claude" row paired to that project's freshest session JSONL.
    #[test]
    fn bystanders_mentioning_agents_are_not_agents() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        assert_eq!(classify("grep -r claude .", &b, &u), None);
        assert_eq!(classify("less claude", &b, &u), None);
        assert_eq!(classify("journalctl -u codex", &b, &u), None);
        assert_eq!(classify("vim /home/x/notes/claude-todo.md", &b, &u), None);
        assert_eq!(classify("/usr/bin/vi /home/x/src/goose apply", &b, &u), None);
        assert_eq!(classify(r"C:\Windows\notepad.exe C:\Users\x\claude.txt", &b, &u), None);
        assert_eq!(classify("tail -f /home/x/.codex/sessions/rollout.jsonl", &b, &u), None);
        assert_eq!(classify("man aider", &b, &u), None);
    }

    // ...while interpreter wrappers and subcommand launchers stay detected.
    #[test]
    fn wrappers_and_launchers_still_detected() {
        let b = builtin();
        let u: Vec<UserMatcher> = vec![];
        assert_eq!(classify("sh -c claude --resume", &b, &u), Some("claude"));
        assert_eq!(classify("python3 -m aider.main --model gpt-4", &b, &u), Some("aider"));
        assert_eq!(classify("node /usr/local/lib/claude/claude --chat", &b, &u), Some("claude"));
        assert_eq!(classify("gh copilot suggest", &b, &u), Some("copilot"));
        assert_eq!(classify("ollama run llama3", &b, &u), Some("ollama"));
        assert_eq!(classify("ollama list", &b, &u), None);
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
