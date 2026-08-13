// Config-file defaults.
//
// agtop reads a flat `key = value` TOML file at startup and uses it to
// pre-populate the same fields the CLI flags set.  Precedence is
// strictly: built-in defaults < config file < CLI flags — a flag the
// user actually typed always wins (decided via clap's ValueSource, not
// by comparing against default values, so `--interval 1.5` still counts
// as explicit).
//
// Locations probed (first match wins):
//   1. --config PATH            (explicit override; missing file warns)
//   2. $XDG_CONFIG_HOME/agtop/config.toml   (Unix; ~/.config fallback)
//      %APPDATA%\agtop\config.toml          (Windows)
// --no-config skips loading entirely.
//
// Error policy: a malformed file or a bad value never aborts startup —
// each problem is a one-line stderr warning and the affected key falls
// back to its built-in default.  Unknown keys warn too (they're usually
// typos) but are otherwise ignored, so configs stay forward-compatible
// with older binaries.

use crate::cli::{Args, SORT_KEYS, THEME_NAMES, TOKEN_MODES};
use std::path::{Path, PathBuf};

/// Every field is `Option`: `None` means "the file didn't set it", so
/// `apply` can leave the built-in / CLI value untouched.
#[derive(Debug, Default, PartialEq)]
pub struct Config {
    pub interval: Option<f64>,
    pub sort: Option<String>,
    pub sort_desc: Option<bool>,
    pub tokens: Option<String>,
    pub theme: Option<String>,
    pub no_color: Option<bool>,
    pub compact: Option<bool>,
    /// Extra `label=regex` matchers, additive to built-ins — same
    /// semantics as repeating `-m` or setting `$AGTOP_MATCH`.
    pub matchers: Option<Vec<String>>,
}

/// Which value-carrying flags the user explicitly passed on the command
/// line.  Built from `ArgMatches::value_source` — flags that are
/// `Option<T>` in `Args` (theme) don't need an entry, `None` already
/// means "not passed".
#[derive(Debug, Default)]
pub struct Explicit {
    pub interval: bool,
    pub sort: bool,
    pub tokens: bool,
    pub no_color: bool,
}

/// Standard config path: `$XDG_CONFIG_HOME/agtop/config.toml` (or
/// `~/.config/agtop/config.toml`) on Unix, `%APPDATA%\agtop\config.toml`
/// on Windows.  Deliberately not `dirs::config_dir()` on macOS — that
/// resolves to `~/Library/Application Support`, while everything else
/// agtop documents (`--prices`, goose sessions) lives under `~/.config`.
pub fn default_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::config_dir().map(|d| d.join("agtop").join("config.toml"))
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
        Some(base.join("agtop").join("config.toml"))
    }
}

/// Read + parse `path`.  IO failure (missing file, unreadable) is a
/// warning, not an error — callers only reach here with an explicit
/// `--config` or an existing default-location file.
pub fn load(path: &Path) -> (Config, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) => (
            Config::default(),
            vec![format!("{}: {e}; using built-in defaults", path.display())],
        ),
    }
}

/// Parse the flat key/value schema, collecting warnings instead of
/// failing.  A TOML syntax error abandons the whole file (there is no
/// safe partial read); a bad value or unknown key skips just that key.
pub fn parse(text: &str) -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            // toml's Display is multi-line (caret + span); flatten so the
            // warning stays a single stderr line.
            let msg = e.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
            warnings.push(format!("{msg}; using built-in defaults"));
            return (Config::default(), warnings);
        }
    };
    let mut cfg = Config::default();
    for (key, val) in table {
        match key.as_str() {
            "interval" => match val {
                toml::Value::Float(f) if f > 0.0 => cfg.interval = Some(f),
                toml::Value::Integer(i) if i > 0 => cfg.interval = Some(i as f64),
                other => warnings.push(format!(
                    "interval: expected a positive number, got {}", describe(&other))),
            },
            "sort" => cfg.sort = choice(val, &SORT_KEYS, "sort", &mut warnings),
            "tokens" => cfg.tokens = choice(val, &TOKEN_MODES, "tokens", &mut warnings),
            "theme" => cfg.theme = choice(val, &THEME_NAMES, "theme", &mut warnings),
            "sort_desc" => cfg.sort_desc = boolean(val, "sort_desc", &mut warnings),
            "no_color" => cfg.no_color = boolean(val, "no_color", &mut warnings),
            "compact" => cfg.compact = boolean(val, "compact", &mut warnings),
            "match" => match val {
                toml::Value::Array(items) => {
                    let mut out = Vec::new();
                    for it in items {
                        match it {
                            toml::Value::String(s) => out.push(s),
                            other => warnings.push(format!(
                                "match: expected string entries, got {}", describe(&other))),
                        }
                    }
                    cfg.matchers = Some(out);
                }
                // A single bare string is a common way to write a
                // one-entry list; accept it.
                toml::Value::String(s) => cfg.matchers = Some(vec![s]),
                other => warnings.push(format!(
                    "match: expected an array of \"label=regex\" strings, got {}",
                    describe(&other))),
            },
            other => warnings.push(format!("unknown key `{other}` (ignored)")),
        }
    }
    (cfg, warnings)
}

impl Config {
    /// Fold the file's values into the parsed `Args`, respecting
    /// precedence: only fields the user did NOT pass on the command
    /// line are overwritten.  Matchers are additive (like `-m` and
    /// `$AGTOP_MATCH`), never a replacement.
    pub fn apply(&self, args: &mut Args, cli: &Explicit) {
        if !cli.interval {
            if let Some(v) = self.interval { args.interval = v; }
        }
        if !cli.sort {
            if let Some(s) = &self.sort { args.sort = s.clone(); }
        }
        if !cli.tokens {
            if let Some(t) = &self.tokens { args.tokens = t.clone(); }
        }
        if args.theme.is_none() {
            if let Some(t) = &self.theme { args.theme = Some(t.clone()); }
        }
        if !cli.no_color {
            if let Some(v) = self.no_color { args.no_color = v; }
        }
        // TUI-only defaults: no CLI flag exists (the TUI toggles these
        // live with `S` / `C`), so the file value always lands.
        if let Some(v) = self.sort_desc { args.sort_desc = v; }
        if let Some(v) = self.compact { args.compact = v; }
        if let Some(ms) = &self.matchers {
            args.r#match.extend(ms.iter().cloned());
        }
    }
}

fn choice(
    val: toml::Value,
    allowed: &[&str],
    key: &str,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match val {
        toml::Value::String(s) if allowed.contains(&s.as_str()) => Some(s),
        toml::Value::String(s) => {
            warnings.push(format!(
                "{key}: unknown value `{s}` (expected one of: {})", allowed.join(", ")));
            None
        }
        other => {
            warnings.push(format!("{key}: expected a string, got {}", describe(&other)));
            None
        }
    }
}

fn boolean(val: toml::Value, key: &str, warnings: &mut Vec<String>) -> Option<bool> {
    match val {
        toml::Value::Boolean(b) => Some(b),
        other => {
            warnings.push(format!("{key}: expected true/false, got {}", describe(&other)));
            None
        }
    }
}

// toml's Value Display sits behind a feature agtop doesn't enable; the
// value for strings / the type name otherwise is enough to point at the
// offending line.
fn describe(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("string `{s}`"),
        other => other.type_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches};

    fn parse_args(argv: &[&str]) -> (Args, Explicit) {
        let m = Args::command().get_matches_from(argv);
        let args = Args::from_arg_matches(&m).unwrap();
        let explicit = crate::cli::explicit_flags(&m);
        (args, explicit)
    }

    #[test]
    fn full_file_parses_clean() {
        let (cfg, warn) = parse(concat!(
            "interval = 0.5\n",
            "sort = \"tokens\"\n",
            "sort_desc = false\n",
            "tokens = \"fresh\"\n",
            "theme = \"dracula\"\n",
            "no_color = true\n",
            "compact = true\n",
            "match = [\"mybot=python.*bot\\\\.py\"]\n",
        ));
        assert!(warn.is_empty(), "unexpected warnings: {warn:?}");
        assert_eq!(cfg.interval, Some(0.5));
        assert_eq!(cfg.sort.as_deref(), Some("tokens"));
        assert_eq!(cfg.sort_desc, Some(false));
        assert_eq!(cfg.tokens.as_deref(), Some("fresh"));
        assert_eq!(cfg.theme.as_deref(), Some("dracula"));
        assert_eq!(cfg.no_color, Some(true));
        assert_eq!(cfg.compact, Some(true));
        assert_eq!(cfg.matchers, Some(vec!["mybot=python.*bot\\.py".to_string()]));
    }

    #[test]
    fn integer_interval_accepted() {
        let (cfg, warn) = parse("interval = 2\n");
        assert!(warn.is_empty());
        assert_eq!(cfg.interval, Some(2.0));
    }

    #[test]
    fn unknown_key_warns_but_keeps_known_keys() {
        let (cfg, warn) = parse("intervall = 2.0\nsort = \"cpu\"\n");
        assert_eq!(cfg.sort.as_deref(), Some("cpu"));
        assert_eq!(cfg.interval, None);
        assert_eq!(warn.len(), 1);
        assert!(warn[0].contains("intervall"), "warning names the key: {warn:?}");
    }

    #[test]
    fn malformed_file_falls_back_to_defaults() {
        let (cfg, warn) = parse("sort = \"cpu\"\nthis is not toml [\n");
        assert_eq!(cfg, Config::default());
        assert_eq!(warn.len(), 1);
        assert!(warn[0].contains("defaults"), "warning mentions fallback: {warn:?}");
    }

    #[test]
    fn bad_values_warn_and_are_ignored() {
        let (cfg, warn) = parse(concat!(
            "interval = -1\n",
            "sort = \"sideways\"\n",
            "tokens = 7\n",
            "compact = \"yes\"\n",
        ));
        assert_eq!(cfg, Config::default());
        assert_eq!(warn.len(), 4);
        assert!(warn.iter().any(|w| w.contains("sideways")));
    }

    #[test]
    fn single_string_match_becomes_one_entry() {
        let (cfg, warn) = parse("match = \"bot=python\"\n");
        assert!(warn.is_empty());
        assert_eq!(cfg.matchers, Some(vec!["bot=python".to_string()]));
    }

    #[test]
    fn config_fills_in_unpassed_flags() {
        let (mut args, explicit) = parse_args(&["agtop"]);
        let (cfg, _) = parse(concat!(
            "interval = 0.5\nsort = \"mem\"\ntokens = \"fresh\"\n",
            "theme = \"nord\"\nno_color = true\nsort_desc = false\ncompact = true\n",
            "match = [\"a=b\"]\n",
        ));
        cfg.apply(&mut args, &explicit);
        assert_eq!(args.interval, 0.5);
        assert_eq!(args.sort, "mem");
        assert_eq!(args.tokens, "fresh");
        assert_eq!(args.theme.as_deref(), Some("nord"));
        assert!(args.no_color);
        assert!(!args.sort_desc);
        assert!(args.compact);
        assert_eq!(args.r#match, vec!["a=b".to_string()]);
    }

    #[test]
    fn cli_flags_beat_config() {
        // --interval matching the built-in default must STILL win over
        // the file: explicitness comes from ValueSource, not the value.
        let (mut args, explicit) = parse_args(&[
            "agtop", "--interval", "1.5", "--sort", "cpu",
            "--tokens", "cumulative", "--theme", "light", "-m", "cli=x",
        ]);
        let (cfg, _) = parse(concat!(
            "interval = 9.0\nsort = \"mem\"\ntokens = \"fresh\"\n",
            "theme = \"nord\"\nmatch = [\"file=y\"]\n",
        ));
        cfg.apply(&mut args, &explicit);
        assert_eq!(args.interval, 1.5);
        assert_eq!(args.sort, "cpu");
        assert_eq!(args.tokens, "cumulative");
        assert_eq!(args.theme.as_deref(), Some("light"));
        // Matchers are additive, CLI entries first.
        assert_eq!(args.r#match, vec!["cli=x".to_string(), "file=y".to_string()]);
    }

    #[test]
    fn empty_config_leaves_builtin_defaults() {
        let (mut args, explicit) = parse_args(&["agtop"]);
        Config::default().apply(&mut args, &explicit);
        assert_eq!(args.interval, 1.5);
        assert_eq!(args.sort, "smart");
        assert_eq!(args.tokens, "cumulative");
        assert!(args.sort_desc);
        assert!(!args.compact);
        assert!(args.theme.is_none());
    }

    #[test]
    fn load_missing_file_warns() {
        let (cfg, warn) = load(Path::new("/nonexistent/agtop-config.toml"));
        assert_eq!(cfg, Config::default());
        assert_eq!(warn.len(), 1);
    }

    #[test]
    fn default_path_ends_with_agtop_config_toml() {
        if let Some(p) = default_path() {
            assert!(p.ends_with(Path::new("agtop").join("config.toml")));
        }
    }
}
