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
    None
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
    fn formats_small_amounts() {
        assert_eq!(fmt_usd(0.0012), "<$0.01");
        assert_eq!(fmt_usd(1.234), "$1.23");
        assert_eq!(fmt_usd(12.3456), "$12.35");
        assert_eq!(fmt_usd(0.0), "$0.00");
    }
}
