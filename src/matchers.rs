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
    let m = |label: &'static str, body: &str| Matcher {
        label,
        re: Regex::new(body).expect("builtin regex"),
    };
    let p = |s: &str| format!("{P}{s}");
    vec![
        m("claude",       &p(r"claude(-code)?(\s|$)")),
        m("claude-code",  r"@anthropic-ai/claude-code"),
        m("codex",        &p(r"codex(\s|$)")),
        m("openai-codex", r"@openai/codex"),
        m("aider",        &p(r"aider(\s|$|\.)")),
        m("cursor-agent", &p(r"cursor-agent(\s|$)")),
        m("gemini",       &p(r"gemini(-cli)?(\s|$)")),
        m("goose",        &p(r"goose(\s|$)")),
        m("continue",     &p(r"continue(-cli|-agent)?(\s|$)")),
        m("opencode",     &p(r"opencode(\s|$)")),
        m("copilot",      r"gh[\s-]copilot|github-copilot-cli"),
        m("cody",         &p(r"cody(\s|$)")),
        m("amp",          r"(^|[\s/])amp(\s|$)|@sourcegraph/amp"),
        m("crush",        &p(r"crush(\s|$)")),
        m("mods",         &p(r"mods(\s|$)")),
        m("sgpt",         &p(r"sgpt(\s|$)")),
        m("llm",          &p(r"llm(\s|$)")),
        m("ollama",       &p(r"ollama(\s+(run|chat|serve)|$)")),
        m("fabric",       &p(r"fabric(\s|$)")),
        m("block-goose",  &p(r"goose-server")),
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
    for m in builtins {
        if m.re.is_match(cmdline) {
            return Some(m.label);
        }
    }
    for m in user {
        if m.re.is_match(cmdline) {
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
}
