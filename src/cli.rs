// Clap-driven entrypoint. Dispatches to the TUI or the --once / --json paths.

use crate::collector::Collector;
use crate::matchers;
use crate::model::Snapshot;
use crate::ui;

use anyhow::Result;
use clap::{ArgAction, Parser};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const LONG_ABOUT: &str = "\
agtop is a terminal UI for monitoring AI coding agents on the system.
Like top, but for Claude Code, Codex, Aider, Cursor, Gemini, Goose, and friends.

It detects the major agent CLIs out of the box and you can teach it about
anything else with a one-line regex via -m / $AGTOP_MATCH.

KEY BINDINGS (TUI):
  q, Ctrl-C   quit
  ?, h        toggle help
  p           pause / resume refresh
  r           refresh now
  s           cycle sort (smart / cpu / mem / tokens / cost / uptime / agent)
  S           reverse sort direction
  x           toggle token metric (cumulative / fresh)
  g           toggle project grouping
  /           filter by substring (Esc to clear)
  j/k, ↓/↑    move selection
  Enter       open / close the detail popup for the selected agent

ENVIRONMENT:
  AGTOP_MATCH   semicolon-separated `label=regex` matchers
                (additive to built-ins)

CONFIG FILE:
  Defaults are read from ~/.config/agtop/config.toml
  ($XDG_CONFIG_HOME respected; %APPDATA%\\agtop\\config.toml on
  Windows).  Flat `key = value` TOML mirroring the flags:
  interval, sort, sort_desc, tokens, theme, no_color, compact,
  match (array of \"label=regex\").  A flag passed on the command
  line always wins.  --config PATH reads a different file;
  --no-config skips loading.

EXAMPLES:
  agtop                       # full TUI
  agtop --once                # one-shot snapshot, top -b -n 1 style
  agtop -1 --top 10           # top-10 active agents and exit
  agtop --json | jq           # machine-readable JSON for scripting
  agtop --json -n 60 -i 1     # NDJSON: 60 snapshots, one JSON object per line
  agtop --watch --json | jq   # NDJSON summary object per tick, forever
  agtop --interval 0.5        # half-second refresh
  agtop --sort tokens         # sort by token consumption (descending)
  agtop --watch               # one summary line per tick (CI-friendly)
  agtop --watch --threshold-tokens-rate 100000   # alert if >100k tok/min
  agtop --prices ~/.config/agtop/prices.toml     # custom model prices
  agtop -m \"myagent=python.*my_agent\\.py\"        # custom matcher
  agtop --no-config           # ignore the config file for this run
";

/// Valid `--sort` keys, shared with the config-file validator.
pub const SORT_KEYS: [&str; 7] =
    ["smart", "cpu", "mem", "tokens", "cost", "uptime", "agent"];
/// Valid `--theme` names, shared with the config-file validator.
pub const THEME_NAMES: [&str; 7] =
    ["default", "light", "dracula", "nord", "gruvbox", "monochrome", "mono"];
/// Valid `--tokens` modes, shared with the config-file validator.
pub const TOKEN_MODES: [&str; 2] = ["cumulative", "fresh"];

#[derive(Parser, Debug)]
#[command(
    name = "agtop",
    version,
    about = "Terminal UI for monitoring AI coding agents — like top, but for agents.",
    long_about = LONG_ABOUT,
    arg_required_else_help = false,
)]
pub struct Args {
    /// Print a one-shot snapshot and exit (no TUI).
    #[arg(short = '1', long)]
    pub once: bool,

    /// Machine-readable JSON snapshot; implies --once.  With -n N (N>1)
    /// or --watch, streams NDJSON: one compact JSON object per line.
    #[arg(short = 'j', long)]
    pub json: bool,

    /// TUI / iteration refresh interval, in seconds.
    #[arg(short = 'i', long, default_value_t = 1.5)]
    pub interval: f64,

    /// With --once, print N snapshots delimited by `---`.  With --json,
    /// N compact snapshots, one per line (NDJSON).
    #[arg(short = 'n', long, default_value_t = 1)]
    pub iterations: u32,

    /// Only show agents whose label / cmdline / cwd / project matches.
    #[arg(short = 'f', long)]
    pub filter: Option<String>,

    /// Sort key.
    #[arg(short = 's', long, default_value = "smart", value_parser = SORT_KEYS)]
    pub sort: String,

    /// Additional agent matchers, repeatable.  e.g. `-m mybot=python.*bot\.py`
    #[arg(short = 'm', long, action = ArgAction::Append)]
    pub r#match: Vec<String>,

    /// Disable ANSI colors — colorless TUI palette, plain --once /
    /// --json output.  Same effect as a non-empty $NO_COLOR.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_color: bool,

    /// Color theme for the TUI.  One of: default, light, dracula,
    /// nord, gruvbox, monochrome.  Honored at startup; stays for the
    /// session.
    #[arg(long, value_name = "NAME", value_parser = THEME_NAMES)]
    pub theme: Option<String>,

    /// With --once, only show top N agents.
    #[arg(long, default_value_t = 0)]
    pub top: u32,

    /// Print the built-in agent matcher list and exit.
    #[arg(long)]
    pub list_builtins: bool,

    /// TOML file overriding / extending the built-in model price table.
    #[arg(long, value_name = "PATH")]
    pub prices: Option<std::path::PathBuf>,

    /// Print one summary line per tick to stdout (no TUI). Pipes cleanly.
    #[arg(long)]
    pub watch: bool,

    /// In --watch mode, exit with code 3 if aggregate CPU% goes above N.
    #[arg(long, value_name = "PERCENT")]
    pub threshold_cpu: Option<f64>,

    /// In --watch mode, exit with code 4 if average token rate (tokens/min)
    /// exceeds N.  Useful for "alert me if I'm burning >100k tok/min".
    #[arg(long, value_name = "TOK_PER_MIN")]
    pub threshold_tokens_rate: Option<f64>,

    /// Open the TUI focused on a specific PID with the detail popup
    /// already showing.  Skips the agent list — useful as a wrapper
    /// from other tooling: `agtop --pid $(pgrep claude)`.  If the
    /// PID isn't a known agent on startup, falls back to the regular
    /// list view.
    #[arg(long, value_name = "PID")]
    pub pid: Option<u32>,

    /// Which "tokens" number to display + sort by:
    ///   cumulative (default) — every turn's reported usage summed
    ///                           across the whole session, including
    ///                           cache_read + cache_write reuse.
    ///                           Matches what the API bills for.
    ///   fresh                 — non-cache input + output only
    ///                           (`tokens_input - tokens_cache_read + tokens_output`).
    ///                           Approximates "tokens you paid full
    ///                           rate for" — useful when long-running
    ///                           sessions with heavy cache reuse
    ///                           dominate the cumulative view.
    #[arg(long, value_name = "MODE", default_value = "cumulative",
          value_parser = TOKEN_MODES)]
    pub tokens: String,

    /// Read defaults from PATH instead of the standard location
    /// (~/.config/agtop/config.toml; %APPDATA%\agtop\config.toml on
    /// Windows).  See CONFIG FILE below for the key list.
    #[arg(long, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// Skip loading the config file entirely.
    #[arg(long)]
    pub no_config: bool,

    // Config-file-only defaults — no CLI flag exists because the TUI
    // toggles them live (`S` reverses the sort, `C` collapses rows).
    // clap skips these; the config loader fills them in post-parse.
    #[arg(skip = true)]
    pub sort_desc: bool,
    #[arg(skip)]
    pub compact: bool,
}

/// Which value-carrying flags were explicitly typed on the command line
/// (vs defaulted), so config-file values only fill the gaps.
pub(crate) fn explicit_flags(m: &clap::ArgMatches) -> crate::config::Explicit {
    use clap::parser::ValueSource;
    let cli = |id: &str| m.value_source(id) == Some(ValueSource::CommandLine);
    crate::config::Explicit {
        interval: cli("interval"),
        sort: cli("sort"),
        tokens: cli("tokens"),
        no_color: cli("no_color"),
    }
}

pub fn run() -> Result<ExitCode> {
    use clap::{CommandFactory, FromArgMatches};
    // Parse via explicit matches (not Args::parse) so the config loader
    // can tell "flag typed on the command line" apart from "clap filled
    // in the default" — comparing values can't distinguish `-i 1.5`
    // from no flag at all.
    let matches = Args::command().get_matches();
    let mut args = Args::from_arg_matches(&matches)?;

    // Config file: built-in defaults < config file < CLI flags.
    // Applied before the theme install below so a `theme` key behaves
    // exactly like --theme.  A missing file at the default location is
    // normal; a missing --config PATH warns.
    if !args.no_config {
        let loaded = match &args.config {
            Some(p) => Some(crate::config::load(p)),
            None => crate::config::default_path()
                .filter(|p| p.is_file())
                .map(|p| crate::config::load(&p)),
        };
        if let Some((cfg, warnings)) = loaded {
            for w in warnings {
                eprintln!("agtop: config: {w}");
            }
            cfg.apply(&mut args, &explicit_flags(&matches));
        }
    }

    // Install the active theme before any draw / print path runs so
    // the very first frame uses the requested palette.  Idempotent
    // — the OnceLock inside theme.rs accepts only the first writer.
    if let Some(name) = &args.theme {
        crate::theme::set_theme(name);
    }
    // NO_COLOR (no-color.org: present AND non-empty) and --no-color
    // drop the TUI to the colorless palette too, not just the
    // printers.  An explicit --theme wins: it was installed above,
    // and the theme OnceLock keeps the first writer.
    if args.no_color || no_color_env() {
        crate::theme::set_theme("no-color");
    }

    if args.list_builtins {
        for m in matchers::builtin() {
            println!("{:<16}  {}", m.label, m.re.as_str());
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut user_extra: Vec<String> = args.r#match.clone();
    if let Ok(env) = std::env::var("AGTOP_MATCH") {
        for s in env.split(';') {
            let s = s.trim();
            if !s.is_empty() {
                user_extra.push(s.to_string());
            }
        }
    }
    let user = matchers::parse_user_matchers(&user_extra);

    let mut pricing = crate::pricing::PriceTable::builtin();
    let prices_path = args.prices.clone()
        .or_else(|| std::env::var("AGTOP_PRICES").ok().map(std::path::PathBuf::from));
    if let Some(p) = prices_path {
        match crate::pricing::PriceTable::load(&p) {
            Ok(t) => pricing = pricing.merge(t),
            Err(e) => eprintln!("agtop: --prices {}: {e:#}", p.display()),
        }
    }
    let mut collector = Collector::new(user, pricing);

    if args.watch {
        return run_watch(&mut collector, &args);
    }
    if args.once || args.json {
        return run_once(&mut collector, &args);
    }

    // Both Linux (/proc) and the sysinfo-backed targets (macOS, Windows,
    // *BSD) populate the full Snapshot via Collector::snapshot, so the
    // TUI renders identically across platforms.  Per-process IO bytes
    // and writable open-file enumeration are still Linux-only and are
    // surfaced as `—` cells on other platforms.
    ui::run(collector, args)?;
    Ok(ExitCode::SUCCESS)
}

/// The token number to display / sort by, honouring `--tokens`.
///   cumulative → the whole-session total (what the API bills)
///   fresh      → non-cache input + output (full-rate tokens only)
fn tokens_for_mode(a: &crate::model::Agent, mode: &str) -> u64 {
    match mode {
        "fresh" => a.tokens_input
            .saturating_sub(a.tokens_cache_read)
            .saturating_add(a.tokens_output),
        _ => a.tokens_total,
    }
}

/// Apply `--filter` and `--sort` to the snapshot's agent list in place, so the
/// non-TUI paths (`--once`, `--json`, `--watch`) honour the same flags the TUI
/// does.  `--top` truncation is left to the caller (it differs per path).
fn apply_view(snap: &mut Snapshot, args: &Args) {
    if let Some(f) = &args.filter {
        let f = f.to_ascii_lowercase();
        if !f.is_empty() {
            snap.agents.retain(|a| {
                a.label.to_ascii_lowercase().contains(&f)
                    || a.cmdline.to_ascii_lowercase().contains(&f)
                    || a.cwd.to_ascii_lowercase().contains(&f)
                    || a.project.to_ascii_lowercase().contains(&f)
            });
        }
    }
    let mode = args.tokens.as_str();
    match args.sort.as_str() {
        "cpu" => snap.agents.sort_by(|a, b|
            b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)),
        "mem" => snap.agents.sort_by_key(|a| std::cmp::Reverse(a.rss)),
        "tokens" => snap.agents.sort_by_key(|a| std::cmp::Reverse(tokens_for_mode(a, mode))),
        "cost" => snap.agents.sort_by(|a, b|
            b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal)),
        "uptime" => snap.agents.sort_by_key(|a| std::cmp::Reverse(a.uptime_sec)),
        "agent" => snap.agents.sort_by(|a, b|
            a.label.cmp(&b.label).then(a.pid.cmp(&b.pid))),
        _ => {} // "smart" — keep the collector's status/project/cpu order
    }
}

fn run_once(collector: &mut Collector, args: &Args) -> Result<ExitCode> {
    let interval = Duration::from_millis((args.interval.max(0.1) * 1000.0) as u64);
    let iters = args.iterations.max(1);
    // First sample has no CPU% delta; warm up silently if a single iteration.
    if iters == 1 {
        let _ = collector.snapshot();
        thread::sleep(Duration::from_millis(400));
    }
    for i in 0..iters {
        let mut snap = collector.snapshot();
        // Honour --filter / --sort in both --once and --json (previously
        // ignored outside the TUI, so `agtop --json -f claude` emitted every
        // agent).
        apply_view(&mut snap, args);
        if args.top > 0 {
            snap.agents.truncate(args.top as usize);
        }
        if args.json {
            if iters > 1 {
                // NDJSON: one compact object per line so `--json -n N`
                // pipes into jq / telemetry as a valid stream (the old
                // concatenated pretty-printed objects were unparseable).
                if !write_line(&serde_json::to_string(&snap)?) {
                    return Ok(ExitCode::SUCCESS);
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            }
        } else {
            print_snapshot(&snap, args);
        }
        if i + 1 < iters {
            if !args.json { println!("---"); }
            thread::sleep(interval);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_watch(collector: &mut Collector, args: &Args) -> Result<ExitCode> {
    let interval = std::time::Duration::from_millis((args.interval.max(0.1) * 1000.0) as u64);
    use crate::format::{bytes, pct, si};
    // Warm-up sample.
    let _ = collector.snapshot();
    std::thread::sleep(std::time::Duration::from_millis(400));
    loop {
        let snap = collector.snapshot();
        let a = &snap.aggregates;
        // Average token rate over the last 20 ticks → tokens/min.
        let recent: Vec<f64> = snap.history.tokens_rate.iter().rev().take(20).copied().collect();
        let rate_per_tick = if !recent.is_empty() {
            recent.iter().sum::<f64>() / recent.len() as f64
        } else { 0.0 };
        let rate_per_min = rate_per_tick * 60.0 / args.interval.max(0.1);
        if args.json {
            // NDJSON: one compact summary object per tick, flushed so a
            // downstream `jq` sees each line as it happens.  A broken
            // pipe (`| head`) ends the stream cleanly.
            if !write_line(&watch_json_line(&snap, rate_per_min)) {
                return Ok(ExitCode::SUCCESS);
            }
        } else {
            let cost = if a.cost_usd > 0.0 { format!("  cost {}", crate::pricing::format_cost(a.cost_usd)) } else { String::new() };
            println!(
                "{}  active={}  busy={}  cpu={}  mem={}  tokens={}  tok/min={}{}",
                chrono::Local::now().format("%H:%M:%S"),
                a.active, a.busy,
                pct(a.cpu), bytes(a.mem_bytes),
                si(a.tokens_total), si(rate_per_min as u64),
                cost,
            );
        }
        // Threshold checks.
        if let Some(t) = args.threshold_cpu {
            if a.cpu > t {
                eprintln!("agtop: cpu {} > threshold {}", pct(a.cpu), pct(t));
                return Ok(ExitCode::from(3));
            }
        }
        if let Some(t) = args.threshold_tokens_rate {
            if rate_per_min > t {
                eprintln!("agtop: token rate {}/min > threshold {}/min",
                    si(rate_per_min as u64), si(t as u64));
                return Ok(ExitCode::from(4));
            }
        }
        std::thread::sleep(interval);
    }
}

/// Write one line to stdout and flush it immediately.  Returns false on
/// any write error — in practice a broken pipe (downstream `head` / `jq`
/// exited) — so streaming loops can stop cleanly instead of panicking
/// inside `println!`.  Unix normally dies of SIGPIPE first (restored to
/// default in main.rs); this is the Windows / signal-blocked fallback.
fn write_line(s: &str) -> bool {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{s}").and_then(|_| out.flush()).is_ok()
}

/// The per-tick `--watch --json` summary object.  Field names match the
/// snapshot schema (`ts` = the snapshot's `now`, epoch milliseconds);
/// `tok_per_min` is the same 20-tick average the human line prints.
fn watch_json_line(snap: &Snapshot, rate_per_min: f64) -> String {
    let a = &snap.aggregates;
    serde_json::json!({
        "ts": snap.now,
        "active": a.active,
        "busy": a.busy,
        "cpu": a.cpu,
        "mem_bytes": a.mem_bytes,
        "tokens_total": a.tokens_total,
        "tok_per_min": rate_per_min,
        "cost_usd": a.cost_usd,
    })
    .to_string()
}

/// no-color.org convention: NO_COLOR disables color only when it is
/// present AND non-empty (`NO_COLOR=` opts back in).
fn no_color_env() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

fn print_snapshot(snap: &Snapshot, args: &Args) {
    use crate::format::{bytes, dur, pct, shorten, si};
    // Honour both --no-color and the no-color.org `NO_COLOR` convention.
    let color = !args.no_color && !no_color_env();
    println!(
        "agtop  active={}  busy={}  subagents={}  waiting={}  completed={}  projects={}  cpu={}  mem={}  tokens={}",
        snap.aggregates.active,
        snap.aggregates.busy,
        snap.aggregates.subagents,
        snap.aggregates.waiting,
        snap.aggregates.completed,
        snap.aggregates.project_count,
        pct(snap.aggregates.cpu),
        bytes(snap.aggregates.mem_bytes),
        si(snap.aggregates.tokens_total),
    );
    // Built from the same widths as the row format below so header and
    // rows can never drift apart.
    println!(
        "{}",
        bold(&format!(
            "{:<8} {:<12} {:>7} {:>6} {:>8} {:>8} {:>4} {:>6}  {:<14}  {}",
            "STATUS", "AGENT", "PID", "CPU%", "MEM", "UP", "SUB", "TOK", "PROJECT", "DOING",
        ), color)
    );
    let take = if args.top > 0 { args.top as usize } else { snap.agents.len() };
    println!(
        "{}",
        paint(
            &format!(
                "prices as of {} ({}) — `--prices PATH` to override",
                crate::pricing::prices_updated(),
                crate::pricing::prices_source(),
            ),
            Color::Gray,
            color,
        ),
    );
    for a in snap.agents.iter().take(take) {
        // Pad each *colored* cell's plain text to its column width first, then
        // wrap it in the ANSI escape — `format!`'s width counts the escape
        // bytes, so painting before padding shifted every colored row out of
        // the grid.  The painted cells therefore use `{}` (no width) below.
        let badge_text = format!("{} {}", a.status.glyph(), a.status.label());
        let badge = paint_status(&format!("{:<8}", badge_text), a.status, color);
        let sub = if a.subagents > 0 {
            paint(&format!("{:>4}", format!("+{}", a.subagents)), Color::Cyan, color)
        } else {
            format!("{:>4}", "-")
        };
        let tok_n = tokens_for_mode(a, &args.tokens);
        let tok = if tok_n > 0 {
            paint(&format!("{:>6}", si(tok_n)), Color::Cyan, color)
        } else {
            format!("{:>6}", "-")
        };
        let doing = describe_doing(a);
        println!(
            "{} {:<12} {:>7} {:>6} {:>8} {:>8} {} {}  {:<14}  {}",
            badge,
            a.label,
            a.pid,
            pct(a.cpu),
            bytes(a.rss),
            dur(a.uptime_sec),
            sub,
            tok,
            shorten(&a.project, 14),
            shorten(&doing, 80),
        );
    }
}

fn describe_doing(a: &crate::model::Agent) -> String {
    if let Some(tool) = &a.current_tool {
        if let Some(t) = &a.current_task {
            return format!("{}: {}", tool, t);
        }
        return tool.clone();
    }
    if let Some(t) = &a.current_task {
        return t.clone();
    }
    if a.status == crate::model::Status::Idle {
        if let Some(age) = a.session_age_ms {
            return format!("(idle {})", crate::format::dur(age / 1000));
        }
    }
    if a.status == crate::model::Status::Waiting   { return "(awaiting input)".into(); }
    if a.status == crate::model::Status::Completed { return "(session ended)".into(); }
    a.cmdline.clone()
}

#[derive(Copy, Clone)]
enum Color { Green, Yellow, Cyan, Magenta, Gray }
fn esc(c: Color) -> &'static str {
    match c {
        Color::Green   => "\x1b[32m",
        Color::Yellow  => "\x1b[33m",
        Color::Cyan    => "\x1b[36m",
        Color::Magenta => "\x1b[35m",
        Color::Gray    => "\x1b[2m",
    }
}
fn paint(s: &str, c: Color, on: bool) -> String {
    if !on { return s.to_string(); }
    format!("{}{}\x1b[0m", esc(c), s)
}
fn paint_status(s: &str, st: crate::model::Status, on: bool) -> String {
    use crate::model::Status::*;
    let c = match st {
        Busy => Color::Green,
        Spawning => Color::Cyan,
        Active => Color::Green,
        Idle => Color::Gray,
        Waiting => Color::Yellow,
        Completed => Color::Magenta,
        Stale => Color::Gray,
    };
    let bold = if matches!(st, Busy | Spawning) { "\x1b[1m" } else { "" };
    if !on { return s.to_string(); }
    format!("{}{}{}\x1b[0m", bold, esc(c), s)
}
fn bold(s: &str, on: bool) -> String {
    if on { format!("\x1b[1m{}\x1b[22m", s) } else { s.to_string() }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_ndjson_is_one_parseable_line() {
        let snap = Snapshot {
            now: 1_777_000_000_000,
            platform: "linux".into(),
            ..Snapshot::default()
        };
        let line = serde_json::to_string(&snap).unwrap();
        assert!(!line.contains('\n'), "compact serialization must be single-line");
        let back: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(back["now"], 1_777_000_000_000u64);
        assert_eq!(back["platform"], "linux");
    }

    #[test]
    fn watch_json_line_is_one_parseable_line() {
        let snap = Snapshot {
            now: 42,
            aggregates: crate::model::Aggregates {
                active: 3,
                busy: 1,
                cpu: 12.5,
                mem_bytes: 1024,
                tokens_total: 999,
                cost_usd: 0.5,
                ..Default::default()
            },
            ..Snapshot::default()
        };
        let line = watch_json_line(&snap, 6000.0);
        assert!(!line.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["ts"], 42);
        assert_eq!(v["active"], 3);
        assert_eq!(v["busy"], 1);
        assert_eq!(v["cpu"], 12.5);
        assert_eq!(v["mem_bytes"], 1024);
        assert_eq!(v["tokens_total"], 999);
        assert_eq!(v["tok_per_min"], 6000.0);
        assert_eq!(v["cost_usd"], 0.5);
    }
}
