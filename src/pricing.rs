// Token → $ pricing.  The bulk of the price table is auto-generated from
// LiteLLM's community registry (see scripts/sync_prices.py and the nightly
// sync workflow).  On top of that we keep a small curated overlay for:
//
//   - local-only models (Ollama, llama.cpp, vLLM, LM Studio):
//     these have *no* API cost — they run on the user's hardware — so
//     `cost()` short-circuits to $0 with a `local` note rather than
//     returning a guess based on a coincidental name match.
//
//   - the latest Anthropic/OpenAI/Google SKUs that may not yet be in the
//     LiteLLM snapshot we shipped with (curated wins over generated).
//
// Lookup is suffix-tolerant: `claude-sonnet-4-7-20260101` resolves to
// `claude-sonnet-4-7` (then `claude-sonnet-4`, then `claude-sonnet`, etc.)
// so we don't have to track every dated revision.
//
// `--prices PATH` lets the user override anything via TOML; user values win.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::pricing_data;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ModelPrice {
    /// USD per 1,000,000 input tokens.
    pub input_per_mtok:  f64,
    /// USD per 1,000,000 output tokens.
    pub output_per_mtok: f64,
    /// Maximum input-window size in tokens.  Sourced from LiteLLM's
    /// `max_input_tokens` field; `None` when the registry doesn't list
    /// one (rare; older closed-source models).  Used by the TUI to
    /// render a per-agent "Context: X% used (used/limit)" indicator
    /// and to warn when a session is approaching compaction.
    #[serde(default)]
    pub max_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PriceTable {
    #[serde(default)]
    pub models: HashMap<String, ModelPrice>,
}

/// Date the bundled price table was last synced from LiteLLM.  Surfaced
/// in `--once` footer, the help overlay, and the detail popup so the
/// user knows the cost number is a snapshot, not a live quote.
pub fn prices_updated() -> &'static str { pricing_data::PRICES_UPDATED }

/// Human label for the data source.  Shown next to the date.
pub fn prices_source()  -> &'static str { pricing_data::PRICES_SOURCE  }

/// Local-model name patterns: substrings that, if present in the model
/// string, signal a local runtime with no API cost.  Conservative —
/// only matches well-known local-only naming conventions, not every
/// open-weights model that *could* be served via a paid API.
const LOCAL_MARKERS: &[&str] = &[
    "ollama/", "ollama_chat/", "ollama:",
    "lmstudio/", "lm-studio/", "vllm/", "llama-cpp/", "llamacpp/",
    "localhost:", "127.0.0.1:", "huggingface/",
];

/// True for sonnet-4-5/6/7 and opus-4-5/6/7 ids with any prefix or date
/// suffix — the families whose standard window is 200K.  4-8 and later
/// minors don't match, and the `[1m]`/`-1m` markers are checked before
/// this rule ever runs.
fn is_sonnet_opus_4x_standard(lower: &str) -> bool {
    for fam in ["sonnet-4-", "opus-4-"] {
        if let Some(i) = lower.find(fam) {
            let rest = &lower.as_bytes()[i + fam.len()..];
            if matches!(rest.first(), Some(b'5' | b'6' | b'7'))
                && !matches!(rest.get(1), Some(b'0'..=b'9')) {
                return true;
            }
        }
    }
    false
}

/// Returns true if the model string identifies a local (no-API-cost)
/// runtime.  Used to short-circuit cost() to $0 unambiguously.
pub fn is_local_model(model: &str) -> bool {
    if model.is_empty() { return false; }
    let lower = model.to_ascii_lowercase();
    LOCAL_MARKERS.iter().any(|m| lower.contains(m))
}

impl PriceTable {
    /// Built-in defaults: LiteLLM snapshot + curated Anthropic/OpenAI/
    /// Google overlay.  Curated entries win over generated when keys
    /// collide.
    pub fn builtin() -> Self {
        let mut m: HashMap<String, ModelPrice> = HashMap::with_capacity(
            pricing_data::GENERATED.len() + 32,
        );
        // Layer 1: LiteLLM-derived registry.
        for (k, p) in pricing_data::GENERATED {
            m.insert((*k).to_string(), *p);
        }
        // Layer 2: curated overlay for the SKUs we want canonical
        // pricing on regardless of what LiteLLM happens to ship.
        let put = |m: &mut HashMap<String, ModelPrice>, k: &str, i: f64, o: f64, ctx: u64| {
            m.insert(k.into(), ModelPrice {
                input_per_mtok: i,
                output_per_mtok: o,
                max_input_tokens: Some(ctx),
            });
        };
        // Anthropic — current Claude families.  Prices are USD per
        // million tokens (input / output); the prompt-cache read/write
        // multipliers (0.1× read, 1.25× 5-min write, 2× 1-hour write)
        // are applied in `cost_with_cache()`.  Context windows: the
        // 5-line (fable/mythos/sonnet/opus) and opus-4-8 ship 1M;
        // sonnet/opus 4.5–4.7 default to 200K — their opt-in 1M SKUs
        // carry an explicit `[1m]`/`-1m` marker in the model id, which
        // `context_limit()` resolves before the table lookup — and
        // Haiku 4.5 plus the older 3.x SKUs are 200K.  These curated
        // values win over the LiteLLM snapshot so the current default
        // models (fable-5, opus-4-8) are never left unpriced or
        // mispriced by a stale registry.
        put(&mut m, "claude-fable-5",    10.00, 50.00, 1_000_000);
        put(&mut m, "claude-mythos-5",   10.00, 50.00, 1_000_000);
        put(&mut m, "claude-opus-5",      5.00, 25.00, 1_000_000);
        put(&mut m, "claude-opus-4-8",    5.00, 25.00, 1_000_000);
        put(&mut m, "claude-opus-4-7",    5.00, 25.00,   200_000);
        put(&mut m, "claude-opus-4-6",    5.00, 25.00,   200_000);
        put(&mut m, "claude-opus-4-5",    5.00, 25.00,   200_000);
        put(&mut m, "claude-opus-4-1",   15.00, 75.00,   200_000);
        put(&mut m, "claude-sonnet-5",    3.00, 15.00, 1_000_000);
        put(&mut m, "claude-sonnet-4-7",  3.00, 15.00,   200_000);
        put(&mut m, "claude-sonnet-4-6",  3.00, 15.00,   200_000);
        put(&mut m, "claude-sonnet-4-5",  3.00, 15.00,   200_000);
        put(&mut m, "claude-haiku-4-5",   1.00,  5.00,   200_000);
        put(&mut m, "claude-3-5-sonnet",  3.00, 15.00,   200_000);
        put(&mut m, "claude-3-5-haiku",   0.80,  4.00,   200_000);
        put(&mut m, "claude-3-opus",     15.00, 75.00,   200_000);
        // OpenAI
        put(&mut m, "gpt-5",          1.25, 10.00, 256_000);
        put(&mut m, "gpt-5-mini",     0.25,  2.00, 256_000);
        put(&mut m, "gpt-5-nano",     0.05,  0.40, 256_000);
        put(&mut m, "gpt-4o",         2.50, 10.00, 128_000);
        put(&mut m, "gpt-4o-mini",    0.15,  0.60, 128_000);
        put(&mut m, "gpt-4-turbo",   10.00, 30.00, 128_000);
        put(&mut m, "o1",            15.00, 60.00, 200_000);
        put(&mut m, "o1-mini",        1.10,  4.40, 128_000);
        put(&mut m, "o3",             2.00,  8.00, 200_000);
        put(&mut m, "o3-mini",        1.10,  4.40, 200_000);
        // Google
        put(&mut m, "gemini-2.0-flash",  0.10,  0.40, 1_000_000);
        put(&mut m, "gemini-1.5-pro",    1.25,  5.00, 2_000_000);
        put(&mut m, "gemini-1.5-flash",  0.075, 0.30, 1_000_000);
        Self { models: m }
    }

    /// Look up the model's input-context window in tokens, falling back
    /// to a conservative 200K when the model isn't in the registry.
    /// Used by the TUI to render context-fill bars.
    pub fn context_limit(&self, model: &str) -> u64 {
        // The price table can't distinguish standard SKUs from their
        // 1M-context variants — both report `claude-opus-4-7` /
        // `claude-sonnet-4-7` in the session JSONL.  Heuristic: if the
        // model id explicitly mentions a long-context flag, treat it
        // as the larger window.  The collector also auto-promotes the
        // limit when an observed prompt exceeds it (see
        // collector::enrich_context).
        let lower = model.to_ascii_lowercase();
        // `[1m]` is the marker Claude Code itself uses (`sonnet[1m]`).
        if lower.contains("-1m") || lower.contains("[1m]")
            || lower.contains("1m-context") || lower.contains("-1000k") {
            return 1_000_000;
        }
        if lower.contains("-2m") {
            return 2_000_000;
        }
        // The sonnet/opus 4.5–4.7 line defaults to 200K — the 1M window
        // is an opt-in SKU caught by the marker check above.  Some
        // LiteLLM snapshots list the 1M window for dated revisions of
        // these ids (exact-match hits that bypass the curated overlay),
        // which would understate context fill ~5×, so the family rule
        // wins over the registry here.  A genuine 1M session still
        // self-corrects via the collector's upward calibration.
        if lower.contains("claude") && is_sonnet_opus_4x_standard(&lower) {
            return 200_000;
        }
        self.lookup(model)
            .and_then(|p| p.max_input_tokens)
            .unwrap_or(200_000)
    }

    /// Read user overrides from a TOML file.  Format:
    ///
    /// ```toml
    /// [models."my-model-2026"]
    /// input_per_mtok = 0.50
    /// output_per_mtok = 2.00
    /// ```
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = fs::read_to_string(path)?;
        let parsed: PriceTable = toml::from_str(&text)?;
        Ok(parsed)
    }

    /// Merge another table on top, user values winning.
    pub fn merge(mut self, other: PriceTable) -> Self {
        for (k, v) in other.models {
            self.models.insert(k, v);
        }
        self
    }

    /// Suffix-tolerant lookup: walks up to 4 `-`-separated suffixes off the
    /// right.  Capped so a user-defined `[models."claude"]` doesn't silently
    /// shadow every Claude SKU ever released — the lookup still walks past
    /// dated revisions (`claude-sonnet-4-7-20260101` → `claude-sonnet-4-7`)
    /// but stops well short of the model family root.
    pub fn lookup(&self, model: &str) -> Option<ModelPrice> {
        if let Some(p) = self.models.get(model) { return Some(*p); }
        let mut s = model;
        for _ in 0..4 {
            let Some(i) = s.rfind('-') else { break };
            s = &s[..i];
            if let Some(p) = self.models.get(s) { return Some(*p); }
        }
        None
    }

    /// Estimated USD cost for `(in_tok, out_tok)` of `model`.
    ///
    /// Returns `0.0` (not None) for both unknown models *and* models
    /// classified as local.  The UI distinguishes the two via
    /// `is_local_model()` — local rows render as `local` instead of `—`.
    /// Legacy two-arg cost (treats every input token at the standard
    /// rate).  Kept for callers that don't track cache splits — but
    /// agent enrichers should prefer `cost_with_cache` so cache hits
    /// don't get billed at full price.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn cost(&self, model: &str, in_tok: u64, out_tok: u64) -> f64 {
        self.cost_with_cache(model, in_tok, out_tok, 0, 0)
    }

    /// Cost with Anthropic-style prompt-cache rate adjustments:
    ///   - cache-read tokens billed at 0.1× input rate
    ///   - cache-write tokens billed at 1.25× input rate
    ///   - everything else billed at the standard input / output rate
    ///
    /// `in_tok` is the *total* input bucket (including cache_read +
    /// cache_write); the formula subtracts the cached portion before
    /// applying the standard rate.  Returns 0.0 for local models and
    /// for unknown SKUs.
    pub fn cost_with_cache(
        &self, model: &str,
        in_tok: u64, out_tok: u64,
        cache_read: u64, cache_write: u64,
    ) -> f64 {
        if is_local_model(model) { return 0.0; }
        let p = match self.lookup(model) { Some(p) => p, None => return 0.0 };
        let cached = cache_read.saturating_add(cache_write);
        let raw_input = in_tok.saturating_sub(cached);
        const M: f64 = 1_000_000.0;
        (raw_input    as f64 / M) * p.input_per_mtok
            + (cache_read  as f64 / M) * p.input_per_mtok * 0.10
            + (cache_write as f64 / M) * p.input_per_mtok * 1.25
            + (out_tok     as f64 / M) * p.output_per_mtok
    }
}

/// Format a USD cost for the UI: $0.04, $1.23, $42.10, $1.2k.
pub fn format_cost(usd: f64) -> String {
    if usd <= 0.0 { return "—".into(); }
    if usd < 0.01 { return "<$0.01".into(); }
    if usd < 10.0  { return format!("${:.2}", usd); }
    if usd < 1000.0 { return format!("${:.1}", usd); }
    if usd < 1_000_000.0 { return format!("${:.1}k", usd / 1000.0); }
    format!("${:.1}M", usd / 1_000_000.0)
}

/// Cost cell variant for an agent.  `Local` carries no $ amount because
/// the runtime is on the user's hardware — the UI should label it
/// `local` (or the user can run `agtop --json` and look at `cost_usd`
/// which will be `0.0` plus a `cost_basis: "local"` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostBasis {
    /// API-priced model with a known per-token rate.
    Api,
    /// Local-runtime model (Ollama / llama.cpp / vLLM / LM Studio).
    Local,
    /// Model name didn't match anything in the price table.
    Unknown,
}

/// Classify a model into one of the three buckets above.  Useful for
/// the UI and for the JSON `cost_basis` field.
pub fn cost_basis(table: &PriceTable, model: &str) -> CostBasis {
    if is_local_model(model) { return CostBasis::Local; }
    if model.is_empty()      { return CostBasis::Unknown; }
    if table.lookup(model).is_some() { CostBasis::Api } else { CostBasis::Unknown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_strips_date_suffixes() {
        let t = PriceTable::builtin();
        let p = t.lookup("claude-sonnet-4-7-20260101").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
    }

    #[test]
    fn cost_math_is_per_million() {
        let t = PriceTable::builtin();
        let c = t.cost("claude-sonnet-4-7", 1_000_000, 0);
        assert!((c - 3.0).abs() < 1e-6);
        let c = t.cost("claude-sonnet-4-7", 0, 1_000_000);
        assert!((c - 15.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_model_is_zero_cost() {
        let t = PriceTable::builtin();
        assert_eq!(t.cost("totally-made-up-model", 999_999, 999_999), 0.0);
    }

    #[test]
    fn format_cost_buckets() {
        assert_eq!(format_cost(0.0), "—");
        assert_eq!(format_cost(0.001), "<$0.01");
        assert_eq!(format_cost(0.04), "$0.04");
        assert_eq!(format_cost(1.23), "$1.23");
        assert_eq!(format_cost(42.10), "$42.1");
        assert_eq!(format_cost(1234.0), "$1.2k");
    }

    #[test]
    fn local_models_short_circuit_to_zero() {
        let t = PriceTable::builtin();
        assert!(is_local_model("ollama/llama3"));
        assert!(is_local_model("Ollama:codellama"));
        assert!(is_local_model("vllm/mistral-7b"));
        assert_eq!(t.cost("ollama/llama3", 5_000_000, 5_000_000), 0.0);
        assert_eq!(cost_basis(&t, "ollama/llama3"), CostBasis::Local);
    }

    #[test]
    fn cost_basis_classifies_three_buckets() {
        let t = PriceTable::builtin();
        assert_eq!(cost_basis(&t, "claude-sonnet-4-7"), CostBasis::Api);
        assert_eq!(cost_basis(&t, "ollama/llama3"),     CostBasis::Local);
        assert_eq!(cost_basis(&t, "totally-made-up"),   CostBasis::Unknown);
        assert_eq!(cost_basis(&t, ""),                  CostBasis::Unknown);
    }

    #[test]
    fn current_flagship_models_are_priced() {
        // Regression: the stale curated overlay left the current default
        // models (fable-5, opus-4-8) unpriced/mispriced — fable-5 fell
        // through to $0 and opus-4-8 suffix-walked to a 15/75 entry.
        let t = PriceTable::builtin();
        let f = t.lookup("claude-fable-5").expect("fable-5 must be priced");
        assert_eq!((f.input_per_mtok, f.output_per_mtok), (10.0, 50.0));
        let o = t.lookup("claude-opus-4-8").expect("opus-4-8 must be priced");
        assert_eq!((o.input_per_mtok, o.output_per_mtok), (5.0, 25.0));
        // Date-suffixed variants resolve via the suffix walk.
        assert!(t.lookup("claude-opus-4-8-20260101").is_some());
        assert!(t.cost("claude-fable-5", 1_000_000, 0) > 0.0);
        assert_eq!(t.context_limit("claude-opus-4-8"), 1_000_000);
        // haiku-4-5 corrected to 1.00/5.00 (was 0.80/4.00).
        let h = t.lookup("claude-haiku-4-5").unwrap();
        assert_eq!((h.input_per_mtok, h.output_per_mtok), (1.0, 5.0));
    }

    #[test]
    fn opus_5_priced_with_1m_window() {
        // Regression: claude-opus-5 was absent from both tables — the
        // suffix walk degraded to nothing, so real sessions priced $0
        // with a 200K fallback window.
        let t = PriceTable::builtin();
        let p = t.lookup("claude-opus-5").expect("opus-5 must be priced");
        assert_eq!((p.input_per_mtok, p.output_per_mtok), (5.0, 25.0));
        assert_eq!(t.context_limit("claude-opus-5"), 1_000_000);
        assert!(t.lookup("claude-opus-5-20260901").is_some());
        assert!(t.cost("claude-opus-5", 1_000_000, 0) > 0.0);
    }

    #[test]
    fn sonnet_opus_4x_windows_are_200k_unless_1m_flagged() {
        // The 4.5–4.7 sonnet/opus SKUs are 200K models; 1M is the opt-in
        // variant with an explicit marker in the id.  An overstated limit
        // can never be corrected at runtime (calibration only promotes
        // upward), so these must not default to 1M.
        let t = PriceTable::builtin();
        for m in ["claude-sonnet-4-5", "claude-sonnet-4-6", "claude-sonnet-4-7",
                  "claude-opus-4-5", "claude-opus-4-6", "claude-opus-4-7"] {
            assert_eq!(t.context_limit(m), 200_000, "{m} is a 200K model");
        }
        // Dated revisions resolve to 200K even when the LiteLLM snapshot
        // carries the 1M beta window for the exact id.
        assert_eq!(t.context_limit("claude-opus-4-7-20260416"), 200_000);
        assert_eq!(t.context_limit("claude-opus-4-6-20260205"), 200_000);
        // Explicit 1M markers win — both spellings.
        assert_eq!(t.context_limit("claude-sonnet-4-5[1m]"), 1_000_000);
        assert_eq!(t.context_limit("claude-sonnet-4-7-1m"), 1_000_000);
        // The genuinely-1M SKUs keep their window.
        assert_eq!(t.context_limit("claude-opus-4-8"), 1_000_000);
        assert_eq!(t.context_limit("claude-fable-5"), 1_000_000);
        assert_eq!(t.context_limit("claude-opus-4-1"), 200_000);
    }

    #[test]
    fn generated_table_has_substantial_coverage() {
        // Sanity check that the LiteLLM sync produced a real dataset.
        // 500 is well below the ~1800 we get today but high enough to
        // catch a regression where the sync silently produced an empty
        // file and someone committed it.
        assert!(pricing_data::GENERATED.len() > 500,
                "generated table only has {} models", pricing_data::GENERATED.len());
    }
}
