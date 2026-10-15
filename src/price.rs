//! List-price estimates for the models herdr-fleet reads from transcripts.
//!
//! USD per 1M tokens, dated 2026-10-13. These are dashboard estimates, not
//! billing truth. Anthropic rates are the published first-party ones (cache
//! reads at 0.1x input, cache writes at 1.25x). DeepSeek rates are its
//! off-peak ones; its peak window (UTC 01:00-04:00 and 06:00-10:00, Mon-Fri)
//! is 2x, so evening US usage can cost more than shown.

const M: f64 = 1_000_000.0;

/// USD per 1M tokens for one model's four token kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rates {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Rates {
    fn anthropic(input: f64, output: f64) -> Rates {
        Rates {
            input,
            output,
            cache_read: input * 0.10,
            cache_write: input * 1.25,
        }
    }
}

/// Where a cost figure comes from. Shown next to every figure (KO).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Priced from the live OpenRouter models table (openrouter.ai/api/v1/models).
    Api,
    /// Priced from static list prices (Anthropic/DeepSeek published, or a dated
    /// builtin snapshot when the live table is unreachable).
    Est,
    /// Subscription billing (Codex/Luna): tokens are shown, no USD figure.
    Plan,
}

impl Basis {
    pub fn as_str(self) -> &'static str {
        match self {
            Basis::Api => "api",
            Basis::Est => "est",
            Basis::Plan => "plan",
        }
    }
}

/// True for subscription-billed families (Codex/Luna): cost is tokens only.
pub fn plan_family(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    let m = m.strip_prefix("meta/").unwrap_or(&m).to_string();
    let base = m.rsplit('/').next().unwrap_or(&m);
    base.starts_with("gpt-")
        || base.starts_with("o1")
        || base.starts_with("o3")
        || base.starts_with("o4")
        || base.contains("codex")
        || base.contains("luna")
}

/// One decimal-string-per-token price object to per-1M [`Rates`].
fn rates_from_pricing(pricing: &serde_json::Value) -> Option<Rates> {
    let parse = |v: &serde_json::Value| {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
    };
    let (Some(ps), Some(cs)) = (pricing.get("prompt"), pricing.get("completion")) else {
        return None;
    };
    let (Some(prompt), Some(completion)) = (parse(ps), parse(cs)) else {
        return None;
    };
    let cache_read = pricing
        .get("input_cache_read")
        .and_then(parse)
        .unwrap_or(prompt);
    Some(Rates {
        input: prompt * M,
        output: completion * M,
        cache_read: cache_read * M,
        // OpenRouter quotes no cache-write price; creating cache costs a
        // normal input token.
        cache_write: prompt * M,
    })
}

/// Live per-model rates from the OpenRouter models table.
#[derive(Debug, Clone, Default)]
pub struct OrTable {
    rates: std::collections::HashMap<String, Rates>,
    /// Model id -> default-route provider name (from the endpoints API).
    routes: std::collections::HashMap<String, String>,
}

impl OrTable {
    /// Parse an openrouter.ai/api/v1/models body. Prices there are decimal
    /// strings per token; they are stored per 1M tokens.
    pub fn parse_models_json(text: &str) -> OrTable {
        let mut rates = std::collections::HashMap::new();
        let v: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => return OrTable::default(),
        };
        let empty = vec![];
        let list = v.get("data").and_then(|d| d.as_array()).unwrap_or(&empty);
        for m in list {
            let Some(id) = m.get("id").and_then(|i| i.as_str()) else {
                continue;
            };
            let Some(pricing) = m.get("pricing") else {
                continue;
            };
            if let Some(r) = rates_from_pricing(pricing) {
                rates.insert(id.to_ascii_lowercase(), r);
            }
        }
        OrTable {
            rates,
            routes: std::collections::HashMap::new(),
        }
    }

    /// Pick the default-route provider's rates from a
    /// `/models/:id/endpoints` body: the cheapest prompt price among fully
    /// healthy providers (`uptime_last_30m == 100`), else the cheapest prompt
    /// overall. OpenRouter publishes no default route, but two independent
    /// default-routed MiMo calls (2026-10-14, 2026-10-15) both billed at the
    /// cheapest-healthy provider's price (Darkbloom), while the headline table
    /// overprices MiMo prompts ~40% — so this tracks billing far closer.
    /// Returns the provider name and its rates.
    pub fn parse_endpoints_json(text: &str) -> Option<(String, Rates)> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let empty = vec![];
        let list = v
            .pointer("/data/endpoints")
            .and_then(|e| e.as_array())
            .unwrap_or(&empty);
        let mut best: Option<(String, Rates)> = None;
        let mut best_healthy: Option<(String, Rates)> = None;
        for e in list {
            let (Some(name), Some(pricing)) = (
                e.get("provider_name").and_then(|n| n.as_str()),
                e.get("pricing"),
            ) else {
                continue;
            };
            let Some(r) = rates_from_pricing(pricing) else {
                continue;
            };
            if best.as_ref().map_or(true, |(_, b)| r.input < b.input) {
                best = Some((name.to_string(), r));
            }
            let healthy = e
                .get("uptime_last_30m")
                .and_then(|u| u.as_f64())
                .is_some_and(|u| u >= 100.0);
            if healthy
                && best_healthy
                    .as_ref()
                    .map_or(true, |(_, b)| r.input < b.input)
            {
                best_healthy = Some((name.to_string(), r));
            }
        }
        best_healthy.or(best)
    }

    /// Override one model's rates with its default-route provider price,
    /// recording the provider for evidence.
    pub fn set_route(&mut self, model: &str, provider: String, rates: Rates) {
        self.rates.insert(model.to_ascii_lowercase(), rates);
        self.routes.insert(model.to_ascii_lowercase(), provider);
    }

    /// The default-route provider recorded for a model, if any.
    pub fn route_for(&self, model: &str) -> Option<&str> {
        let m = model.trim().to_ascii_lowercase();
        self.routes.get(&m).map(String::as_str)
    }

    /// Rates for a model id from the live table, or `None`.
    /// Matches `xiaomi/mimo-v2.6-flash` exactly, a bare `mimo-v2.6-flash`
    /// against any provider suffix, and vice versa.
    pub fn rates_for(&self, model: &str) -> Option<Rates> {
        let m = model.trim().to_ascii_lowercase();
        let m = m.strip_suffix("[1m]").unwrap_or(&m);
        if let Some(r) = self.rates.get(m) {
            return Some(*r);
        }
        // Bare id: match the unique provider-qualified entry ending in it.
        if !m.contains('/') {
            let mut hit = None;
            for (id, r) in &self.rates {
                if id.rsplit('/').next() == Some(m) {
                    if hit.is_some() {
                        return None; // ambiguous across providers
                    }
                    hit = Some(*r);
                }
            }
            if hit.is_some() {
                return hit;
            }
        }
        // Qualified id whose provider prefix differs (e.g. a `meta/`-less transcript).
        if let Some(bare) = m.rsplit('/').next() {
            for (id, r) in &self.rates {
                if id.rsplit('/').next() == Some(bare) {
                    return Some(*r);
                }
            }
        }
        None
    }
}

/// Quote one session's summed token counts: the USD figure (if priceable) and
/// where it came from. The live table wins; static list prices are the
/// fallback; subscription families price nothing (KO).
pub fn quote(
    table: Option<&OrTable>,
    model: &str,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> (Option<f64>, Option<Basis>) {
    let cost = |r: Rates| {
        (input as f64 * r.input
            + cache_read as f64 * r.cache_read
            + cache_write as f64 * r.cache_write
            + output as f64 * r.output)
            / M
    };
    if let Some(t) = table {
        if let Some(r) = t.rates_for(model) {
            return (Some(cost(r)), Some(Basis::Api));
        }
    }
    if let Some(r) = rates_for(model) {
        return (Some(cost(r)), Some(Basis::Est));
    }
    if plan_family(model) {
        return (None, Some(Basis::Plan));
    }
    (None, None)
}

/// Rates for a model id, or `None` when it is not a model we know.
pub fn rates_for(model: &str) -> Option<Rates> {
    let m = model.trim().to_ascii_lowercase();
    // Anthropic. Most specific first, so "claude-opus-5-5" does not fall into
    // the wider "claude-opus" branch.
    if m.starts_with("claude-fable-5") || m.starts_with("claude-mythos-5") {
        return Some(Rates::anthropic(10.0, 50.0));
    }
    if m.starts_with("claude-opus-5-5") {
        return Some(Rates::anthropic(4.0, 20.0));
    }
    if m.starts_with("claude-opus") {
        return Some(Rates::anthropic(5.0, 25.0));
    }
    if m.starts_with("claude-sonnet-5") {
        return Some(Rates::anthropic(2.0, 10.0));
    }
    if m.starts_with("claude-sonnet-4-6") {
        return Some(Rates::anthropic(3.0, 15.0));
    }
    if m.starts_with("claude-haiku-5-5") {
        return Some(Rates::anthropic(0.10, 0.50));
    }
    if m.starts_with("claude-haiku-4-5") {
        return Some(Rates::anthropic(1.0, 5.0));
    }
    // DeepSeek (off-peak; peak is 2x).
    if m.starts_with("deepseek") && m.contains("pro") {
        return Some(Rates {
            input: 0.66,
            output: 1.98,
            cache_read: 0.022,
            cache_write: 0.66,
        });
    }
    if m.starts_with("deepseek") {
        return Some(Rates {
            input: 0.15,
            output: 0.60,
            cache_read: 0.003,
            cache_write: 0.15,
        });
    }
    // Dated builtin snapshot (OpenRouter table 2026-10-15) for the models the
    // fleet actually runs. The live table above wins whenever it is reachable;
    // these keep COST working offline. Units: USD per 1M tokens.
    let bare = m.rsplit('/').next().unwrap_or(&m);
    if bare.starts_with("mimo-v2.6-flash") || bare.starts_with("mimo-v2.5") {
        return Some(Rates {
            input: 0.14,
            output: 0.28,
            cache_read: 0.0028,
            cache_write: 0.14,
        });
    }
    if bare.starts_with("mimo-v2.6-pro") {
        return Some(Rates {
            input: 0.435,
            output: 0.87,
            cache_read: 0.0036,
            cache_write: 0.435,
        });
    }
    if bare.starts_with("mimo-v2.6-pro-ultraspeed") {
        return Some(Rates {
            input: 4.35,
            output: 8.70,
            cache_read: 0.036,
            cache_write: 4.35,
        });
    }
    if bare.starts_with("muse-spark") {
        return Some(Rates {
            input: 0.10,
            output: 0.20,
            cache_read: 0.002,
            cache_write: 0.10,
        });
    }
    if bare.starts_with("muse-wonder") {
        return Some(Rates {
            input: 4.0,
            output: 20.0,
            cache_read: 0.20,
            cache_write: 4.0,
        });
    }
    None
}

/// Maximum age of the cached OpenRouter models table before a refresh is tried.
const TABLE_TTL_SECS: u64 = 24 * 3600;

/// Where the fetched OpenRouter models body is cached:
/// `$XDG_CACHE_HOME/herdr-fleet/openrouter-models.json`, else under `~/.cache`.
pub fn cache_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| std::path::PathBuf::from(".cache"));
    base.join("herdr-fleet").join("openrouter-models.json")
}

/// Fleet models whose billed price is their default-route provider's, not the
/// headline table's (measured: MiMo Flash bills at the cheapest-healthy
/// provider, ~40% under the headline on prompts).
const ROUTED_MODELS: [&str; 1] = ["xiaomi/mimo-v2.6-flash"];

/// Fetch one URL with `curl`, best effort.
fn curl_fetch(url: &str) -> Option<Vec<u8>> {
    let o = std::process::Command::new("curl")
        .args(["-s", "-m", "10", url])
        .output()
        .ok()?;
    (o.status.success() && !o.stdout.is_empty()).then_some(o.stdout)
}

/// Load the live price table: the cache file when present, refreshing it with
/// `curl` when it is older than [`TABLE_TTL_SECS`]. Never fails hard — an
/// unreachable API or missing `curl` just yields `None` (static estimates apply).
pub fn load_table() -> Option<OrTable> {
    let path = cache_path();
    let stale = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.elapsed()
                .map(|d| d.as_secs() > TABLE_TTL_SECS)
                .unwrap_or(true)
        })
        .unwrap_or(true);
    if stale {
        // Best effort: one quick fetch, kept out of the hot path by the TTL.
        if let Some(body) = curl_fetch("https://openrouter.ai/api/v1/models") {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, &body);
        }
    }
    let mut table = std::fs::read_to_string(&path)
        .map(|t| OrTable::parse_models_json(&t))
        .ok()?;
    // Default-route provider prices for the fleet's OpenRouter models, cached
    // per model beside the table under the same TTL.
    if let Some(dir) = path.parent() {
        for model in ROUTED_MODELS {
            let slug: String = model
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let epath = dir.join(format!("endpoints-{slug}.json"));
            let estale = std::fs::metadata(&epath)
                .and_then(|m| m.modified())
                .map(|t| {
                    t.elapsed()
                        .map(|d| d.as_secs() > TABLE_TTL_SECS)
                        .unwrap_or(true)
                })
                .unwrap_or(true);
            if estale {
                let url = format!("https://openrouter.ai/api/v1/models/{model}/endpoints");
                if let Some(body) = curl_fetch(&url) {
                    let _ = std::fs::write(&epath, &body);
                }
            }
            if let Ok(text) = std::fs::read_to_string(&epath) {
                if let Some((provider, rates)) = OrTable::parse_endpoints_json(&text) {
                    table.set_route(model, provider, rates);
                }
            }
        }
    }
    Some(table)
}

/// Estimated USD for one session's summed token counts, or `None` for an
/// unknown model.
pub fn cost_usd(
    model: &str,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
) -> Option<f64> {
    let r = rates_for(model)?;
    Some(
        (input as f64 * r.input
            + cache_read as f64 * r.cache_read
            + cache_write as f64 * r.cache_write
            + output as f64 * r.output)
            / M,
    )
}

/// `1.235` -> `$1.23`; a tiny positive amount -> `<$0.01`.
pub fn fmt_usd(v: f64) -> String {
    // Spend is never negative; a `-0.00` here came from a fold over no rows.
    let v = v.max(0.0);
    if v > 0.0 && v < 0.005 {
        "<$0.01".to_string()
    } else {
        format!("${v:.2}")
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn prices_the_known_families() {
        assert_eq!(
            rates_for("claude-opus-5-5"),
            Some(Rates::anthropic(4.0, 20.0))
        );
        assert_eq!(
            rates_for("claude-opus-5-5[1m]"),
            Some(Rates::anthropic(4.0, 20.0))
        );
        assert_eq!(
            rates_for("deepseek-flash[1m]"),
            Some(Rates {
                input: 0.15,
                output: 0.60,
                cache_read: 0.003,
                cache_write: 0.15,
            })
        );
        assert_eq!(rates_for("some-random-model"), None);
    }

    #[test]
    fn cost_adds_the_four_kinds() {
        let c = cost_usd("deepseek-flash[1m]", 1_000_000, 1_000_000, 0, 1_000_000).unwrap();
        assert!((c - (0.15 + 0.003 + 0.60)).abs() < 1e-9);
        assert_eq!(cost_usd("nope", 1, 0, 0, 0), None);
    }

    #[test]
    fn live_table_beats_static_and_matches_bare_ids() {
        let table = OrTable::parse_models_json(
            r#"{"data":[
            {"id":"xiaomi/mimo-v2.6-flash","pricing":{"prompt":"0.00000014","completion":"0.00000028","input_cache_read":"0.0000000028"}},
            {"id":"meta/muse-spark-1.3-contributor","pricing":{"prompt":"0.0000001","completion":"0.0000002"}}
        ]}"#,
        );
        // Exact qualified id -> api basis at live prices.
        let (c, b) = quote(Some(&table), "xiaomi/mimo-v2.6-flash", 1_000_000, 0, 0, 0);
        assert_eq!(b, Some(Basis::Api));
        assert!((c.unwrap() - 0.14).abs() < 1e-9);
        // Bare transcript id resolves against the qualified entry.
        let (c2, b2) = quote(Some(&table), "mimo-v2.6-flash", 1_000_000, 0, 0, 0);
        assert_eq!((c2, b2), (c, b));
        // meta/-less transcript id resolves too.
        let (_, b3) = quote(Some(&table), "muse-spark-1.3-contributor", 1, 0, 0, 0);
        assert_eq!(b3, Some(Basis::Api));
        // A model in no table falls back to static estimates.
        let (c4, b4) = quote(Some(&table), "deepseek-flash[1m]", 1_000_000, 0, 0, 0);
        assert_eq!(b4, Some(Basis::Est));
        assert!((c4.unwrap() - 0.15).abs() < 1e-9);
        // Subscription families price nothing but keep their basis.
        assert_eq!(
            quote(Some(&table), "gpt-6-luna", 100, 0, 0, 100),
            (None, Some(Basis::Plan))
        );
        assert_eq!(
            quote(None, "codex-thing", 100, 0, 0, 100),
            (None, Some(Basis::Plan))
        );
        // Unknown models stay unpriced with no basis.
        assert_eq!(quote(None, "nope", 1, 0, 0, 0), (None, None));
    }

    #[test]
    fn endpoints_pick_the_cheapest_healthy_provider() {
        let mut table = OrTable::parse_models_json(
            r#"{"data":[{"id":"xiaomi/mimo-v2.6-flash","pricing":{"prompt":"0.00000014","completion":"0.00000028"}}]}"#,
        );
        let (provider, rates) = OrTable::parse_endpoints_json(
            r#"{"data":{"endpoints":[
            {"provider_name":"Darkbloom","uptime_last_30m":100,"pricing":{"prompt":"0.0000001","completion":"0.00000028"}},
            {"provider_name":"Novita","uptime_last_30m":93.33,"pricing":{"prompt":"0.00000005","completion":"0.00000028"}},
            {"provider_name":"Venice","uptime_last_30m":100,"pricing":{"prompt":"0.000000175","completion":"0.00000035"}}
        ]}}"#,
        )
        .expect("a route");
        // Novita is cheaper but unhealthy; Darkbloom wins among the healthy.
        assert_eq!(provider, "Darkbloom");
        table.set_route("xiaomi/mimo-v2.6-flash", provider, rates);
        assert_eq!(table.route_for("xiaomi/mimo-v2.6-flash"), Some("Darkbloom"));
        let (c, b) = quote(Some(&table), "xiaomi/mimo-v2.6-flash", 1_000_000, 0, 0, 0);
        assert_eq!(b, Some(Basis::Api));
        assert!((c.unwrap() - 0.10).abs() < 1e-9);
    }

    #[test]
    fn static_snapshot_covers_the_fleet_offline() {
        // No live table: the dated builtins still price MiMo and Muse.
        let (c, b) = quote(None, "xiaomi/mimo-v2.6-flash", 1_000_000, 0, 0, 1_000_000);
        assert_eq!(b, Some(Basis::Est));
        assert!((c.unwrap() - (0.14 + 0.28)).abs() < 1e-9);
        let (c2, b2) = quote(None, "muse-spark-1.3-contributor", 1_000_000, 0, 0, 0);
        assert_eq!(b2, Some(Basis::Est));
        assert!((c2.unwrap() - 0.10).abs() < 1e-9);
    }

    #[test]
    fn formats_small_amounts() {
        assert_eq!(fmt_usd(0.0012), "<$0.01");
        assert_eq!(fmt_usd(1.234), "$1.23");
        assert_eq!(fmt_usd(12.3456), "$12.35");
        assert_eq!(fmt_usd(0.0), "$0.00");
        assert_eq!(fmt_usd(-0.0), "$0.00", "a fold over no rows gives -0.0");
        assert_eq!(fmt_usd(-0.001), "$0.00");
    }
}
