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
  s           cycle sort (smart / cpu / mem / tokens / uptime / agent)
  g           toggle project grouping
  /           filter by substring (Esc to clear)
  j/k, ↓/↑    move selection
  Enter       open / close the detail popup for the selected agent

ENVIRONMENT:
  AGTOP_MATCH   semicolon-separated `label=regex` matchers
                (additive to built-ins)

EXAMPLES:
  agtop                       # full TUI
  agtop --once                # one-shot snapshot, top -b -n 1 style
  agtop -1 --top 10           # top-10 active agents and exit
  agtop --json | jq           # machine-readable JSON for scripting
  agtop --interval 0.5        # half-second refresh
  agtop --sort tokens         # sort by token consumption (descending)
  agtop --watch               # one summary line per tick (CI-friendly)
  agtop --watch --threshold-tokens-rate 100000   # alert if >100k tok/min
  agtop --prices ~/.config/agtop/prices.toml     # custom model prices
  agtop -m \"myagent=python.*my_agent\\.py\"        # custom matcher
";

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

    /// Machine-readable JSON snapshot; implies --once.
    #[arg(short = 'j', long)]
    pub json: bool,

    /// TUI / iteration refresh interval, in seconds.
    #[arg(short = 'i', long, default_value_t = 1.5)]
    pub interval: f64,

    /// With --once, print N snapshots delimited by `---`.
    #[arg(short = 'n', long, default_value_t = 1)]
    pub iterations: u32,

    /// Only show agents whose label / cmdline / cwd / project matches.
    #[arg(short = 'f', long)]
    pub filter: Option<String>,

    /// Sort key.
    #[arg(short = 's', long, default_value = "smart",
          value_parser = ["smart", "cpu", "mem", "tokens", "uptime", "agent"])]
    pub sort: String,

    /// Additional agent matchers, repeatable.  e.g. `-m mybot=python.*bot\.py`
    #[arg(short = 'm', long, action = ArgAction::Append)]
    pub r#match: Vec<String>,

    /// Disable ANSI colors in --once / --json output.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_color: bool,

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
}

pub fn run() -> Result<ExitCode> {
    let args = Args::parse();

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

    if !crate::proc_::is_linux() {
        eprintln!("agtop: live process metrics require Linux /proc.");
        eprintln!("       Falling back to a single Claude-sessions snapshot.");
        let snap = collector.snapshot();
        println!("{}", serde_json::to_string_pretty(&snap.sessions)?);
        return Ok(ExitCode::SUCCESS);
    }

    ui::run(collector, args)?;
    Ok(ExitCode::SUCCESS)
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
        let snap = collector.snapshot();
        if args.json {
            println!("{}", serde_json::to_string_pretty(&snap)?);
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
        let cost = if a.cost_usd > 0.0 { format!("  cost {}", crate::pricing::format_cost(a.cost_usd)) } else { String::new() };
        println!(
            "{}  active={}  busy={}  cpu={}  mem={}  tokens={}  tok/min={}{}",
            chrono::Local::now().format("%H:%M:%S"),
            a.active, a.busy,
            pct(a.cpu), bytes(a.mem_bytes),
            si(a.tokens_total), si(rate_per_min as u64),
            cost,
        );
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

fn print_snapshot(snap: &Snapshot, args: &Args) {
    use crate::format::{bytes, dur, pct, shorten, si};
    let color = !args.no_color;
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
    println!(
        "{}",
        bold("STATUS   AGENT          PID    CPU%      MEM       UP  SUB   TOK  PROJECT         DOING", color)
    );
    let take = if args.top > 0 { args.top as usize } else { snap.agents.len() };
    for a in snap.agents.iter().take(take) {
        let badge_text = format!("{} {}", a.status.glyph(), a.status.label());
        let badge = paint_status(&badge_text, a.status, color);
        let sub = if a.subagents > 0 {
            paint(&format!("+{:>2}", a.subagents), Color::Cyan, color)
        } else {
            "  -".to_string()
        };
        let tok = if a.tokens_total > 0 {
            paint(&si(a.tokens_total), Color::Cyan, color)
        } else {
            "-".to_string()
        };
        let doing = describe_doing(a);
        println!(
            "{:<8} {:<12} {:>7} {:>6} {:>8} {:>8} {:>4} {:>5}  {:<14}  {}",
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
            return format!("(idle {})", crate::format::dur((age / 1000) as u64));
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

