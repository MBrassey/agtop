// Token → $ pricing.  Embeds a default table for the most common Anthropic
// and OpenAI model SKUs and exposes `--prices PATH` so users can override
// or add new models via TOML.
//
// Lookup is suffix-tolerant: `claude-sonnet-4-7-20260101` resolves to
// `claude-sonnet-4-7` (then `claude-sonnet-4`, then `claude-sonnet`, etc.)
// so we don't have to track every dated revision.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ModelPrice {
    /// USD per 1,000,000 input tokens.
    pub input_per_mtok:  f64,
    /// USD per 1,000,000 output tokens.
    pub output_per_mtok: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PriceTable {
    #[serde(default)]
    pub models: HashMap<String, ModelPrice>,
}

impl PriceTable {
    /// Built-in defaults — illustrative pricing as of early 2026.  Override
    /// with `--prices prices.toml` when these drift.
    pub fn builtin() -> Self {
        let mut m = HashMap::new();
        let put = |m: &mut HashMap<String, ModelPrice>, k: &str, i: f64, o: f64| {
            m.insert(k.into(), ModelPrice { input_per_mtok: i, output_per_mtok: o });
        };
        // Anthropic
        put(&mut m, "claude-sonnet-4-5", 3.00, 15.00);
        put(&mut m, "claude-sonnet-4-6", 3.00, 15.00);
        put(&mut m, "claude-sonnet-4-7", 3.00, 15.00);
        put(&mut m, "claude-opus-4-1",  15.00, 75.00);
        put(&mut m, "claude-opus-4-7",  15.00, 75.00);
        put(&mut m, "claude-haiku-4-5",  0.80,  4.00);
        put(&mut m, "claude-3-5-sonnet", 3.00, 15.00);
        put(&mut m, "claude-3-5-haiku",  0.80,  4.00);
        put(&mut m, "claude-3-opus",    15.00, 75.00);
        // OpenAI
        put(&mut m, "gpt-5",          1.25, 10.00);
        put(&mut m, "gpt-5-mini",     0.25,  2.00);
        put(&mut m, "gpt-5-nano",     0.05,  0.40);
        put(&mut m, "gpt-4o",         2.50, 10.00);
        put(&mut m, "gpt-4o-mini",    0.15,  0.60);
        put(&mut m, "gpt-4-turbo",   10.00, 30.00);
        put(&mut m, "o1",            15.00, 60.00);
        put(&mut m, "o1-mini",        1.10,  4.40);
        put(&mut m, "o3",             2.00,  8.00);
        put(&mut m, "o3-mini",        1.10,  4.40);
        // Google
        put(&mut m, "gemini-2.0-flash",  0.10,  0.40);
        put(&mut m, "gemini-1.5-pro",    1.25,  5.00);
        put(&mut m, "gemini-1.5-flash",  0.075, 0.30);
        Self { models: m }
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

    /// Suffix-tolerant lookup: walks `-`-separated suffixes off the right.
    pub fn lookup(&self, model: &str) -> Option<ModelPrice> {
        if let Some(p) = self.models.get(model) { return Some(*p); }
        let mut s = model;
        while let Some(i) = s.rfind('-') {
            s = &s[..i];
            if let Some(p) = self.models.get(s) { return Some(*p); }
        }
        None
    }

    pub fn cost(&self, model: &str, in_tok: u64, out_tok: u64) -> f64 {
        match self.lookup(model) {
            Some(p) => (in_tok as f64 / 1_000_000.0) * p.input_per_mtok
                     + (out_tok as f64 / 1_000_000.0) * p.output_per_mtok,
            None => 0.0,
        }
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
        // claude-sonnet-4-7: in $3/MTok, out $15/MTok
        // 1M in + 0 out = $3
        let c = t.cost("claude-sonnet-4-7", 1_000_000, 0);
        assert!((c - 3.0).abs() < 1e-6);
        // 0 in + 1M out = $15
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
}
