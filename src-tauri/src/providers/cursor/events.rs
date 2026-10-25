use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::pricing::{ModelPricing, TokenBreakdown};

#[derive(Debug, Clone, PartialEq)]
pub struct CursorUsageRow {
    pub date: DateTime<Utc>,
    pub model: String,
    pub tokens: TokenBreakdown,
    pub estimated_cost_usd: Option<f64>,
}

pub fn parse_usage_events(body: &Value, pricing: &ModelPricing) -> Vec<CursorUsageRow> {
    body.get("usageEventsDisplay")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(|event| usage_row(event, pricing))
                .collect()
        })
        .unwrap_or_default()
}

fn usage_row(event: &Value, pricing: &ModelPricing) -> Option<CursorUsageRow> {
    let date = event_timestamp(event.get("timestamp")?)?;
    let model = event.get("model")?.as_str()?.trim();
    if model.is_empty() {
        return None;
    }
    let usage = event.get("tokenUsage");
    let token = |key: &str| -> u64 {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(|value| value.as_i64())
            .filter(|value| *value >= 0)
            .unwrap_or(0) as u64
    };
    let tokens = TokenBreakdown {
        input: token("inputTokens"),
        cache_write_5m: token("cacheWriteTokens"),
        cache_read: token("cacheReadTokens"),
        output: token("outputTokens"),
        ..TokenBreakdown::default()
    };
    Some(CursorUsageRow {
        date,
        model: model.to_owned(),
        tokens,
        estimated_cost_usd: pricing.estimated_cost_dollars(model, tokens, false),
    })
}

fn event_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    let millis = match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }?;
    if millis < 0 {
        return None;
    }
    DateTime::from_timestamp_millis(millis)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::pricing::{test_bundled_pricing, ModelRates, PricingCatalog, PricingSupplement};

    use super::*;

    fn pricing() -> ModelPricing {
        ModelPricing::new(
            PricingSupplement::default(),
            PricingCatalog {
                entries: HashMap::from([("composer-1".into(), ModelRates::new(2.0, 10.0))]),
                retrieved_at: None,
            },
            PricingCatalog::default(),
        )
    }

    fn events_body(events: Value) -> Value {
        serde_json::json!({ "totalUsageEventsCount": 1, "usageEventsDisplay": [events] })
    }

    #[test]
    fn events_map_tokens_and_pricing_with_missing_counts_as_zero() {
        let body = events_body(serde_json::json!({
            "timestamp": "1784116800000",
            "model": "composer-1",
            "tokenUsage": {"inputTokens": 300, "outputTokens": 100}
        }));
        let rows = parse_usage_events(&body, &pricing());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tokens.input, 300);
        assert_eq!(rows[0].tokens.output, 100);
        assert_eq!(rows[0].tokens.cache_read, 0);
        assert_eq!(
            rows[0].date,
            DateTime::from_timestamp_millis(1_784_116_800_000).unwrap()
        );
        assert!((rows[0].estimated_cost_usd.unwrap() - 0.001_6).abs() < 0.000_001);
    }

    #[test]
    fn cursor_columns_are_priced_without_long_context_uplift() {
        let body = events_body(serde_json::json!({
            "timestamp": 1_784_116_800_000_i64,
            "model": "composer-1",
            "tokenUsage": {"inputTokens": 300000, "cacheWriteTokens": 0, "cacheReadTokens": 0, "outputTokens": 1000000}
        }));
        let rows = parse_usage_events(&body, &pricing());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tokens.input, 300_000);
        assert!((rows[0].estimated_cost_usd.unwrap() - 10.6).abs() < 0.000_001);
    }

    #[test]
    fn router_model_labels_use_the_routed_models_rates() {
        let event = |timestamp: i64, model: &str| {
            serde_json::json!({
                "timestamp": timestamp,
                "model": model,
                "tokenUsage": {"inputTokens": 1000000, "outputTokens": 1000000}
            })
        };
        let body = serde_json::json!({
            "totalUsageEventsCount": 3,
            "usageEventsDisplay": [
                event(1_786_454_400_000_i64, "Opus 5 (Auto Balanced)"),
                event(1_786_454_460_000_i64, "Kimi K3 (Auto Intelligence)"),
                event(1_786_454_520_000_i64, "Grok 4.6 Fast (Auto Balanced)")
            ]
        });

        let rows = parse_usage_events(&body, &test_bundled_pricing());

        assert_eq!(rows.len(), 3);
        for (row, expected) in rows.iter().zip([30.0, 18.0, 16.0]) {
            assert_eq!(row.estimated_cost_usd, Some(expected), "{}", row.model);
        }
    }

    #[test]
    fn unknown_pricing_keeps_tokens_and_marks_cost_missing() {
        let body = events_body(serde_json::json!({
            "timestamp": 1_784_116_800_000_i64,
            "model": "unknown",
            "tokenUsage": {"inputTokens": 100}
        }));
        let rows = parse_usage_events(&body, &pricing());
        assert_eq!(rows[0].tokens.total_tokens(), 100);
        assert!(rows[0].estimated_cost_usd.is_none());
    }

    #[test]
    fn malformed_events_are_skipped_without_dropping_good_ones() {
        let body = serde_json::json!({
            "totalUsageEventsCount": 4,
            "usageEventsDisplay": [
                {"timestamp": "not-a-number", "model": "composer-1"},
                {"timestamp": 1_784_116_800_000_i64},
                {"timestamp": 1_784_116_800_000_i64, "model": "   "},
                {"timestamp": "-5", "model": "composer-1"},
                {"timestamp": 1_784_116_800_001_i64, "model": "composer-1",
                 "tokenUsage": {"inputTokens": -10}}
            ]
        });
        let rows = parse_usage_events(&body, &pricing());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tokens.input, 0);
    }

    #[test]
    fn missing_or_malformed_envelopes_yield_no_rows() {
        assert!(parse_usage_events(&serde_json::json!({}), &pricing()).is_empty());
        assert!(parse_usage_events(
            &serde_json::json!({"usageEventsDisplay": "nope"}),
            &pricing()
        )
        .is_empty());
    }
}
