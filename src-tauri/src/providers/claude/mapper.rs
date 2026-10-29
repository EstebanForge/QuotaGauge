use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde_json::Value;

use crate::models::{MetricValue, MetricValueKind, QuotaFormat, QuotaWindow, ValueMetric};

use super::{auth::ClaudeOAuth, ClaudeError};

pub struct ClaudeMappedUsage {
    pub plan: Option<String>,
    pub quotas: Vec<QuotaWindow>,
    pub value_metrics: Vec<ValueMetric>,
}

pub fn map_usage(
    status: StatusCode,
    body: &Value,
    credentials: &ClaudeOAuth,
) -> Result<ClaudeMappedUsage, ClaudeError> {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(ClaudeError::TokenExpired);
    }
    if !status.is_success() {
        return Err(ClaudeError::RequestFailed(status.as_u16()));
    }
    let object = body.as_object().ok_or(ClaudeError::InvalidResponse)?;
    let mut quotas = Vec::new();
    let mut value_metrics = Vec::new();
    append_window(
        &mut quotas,
        "session",
        "Session",
        object.get("five_hour"),
        5 * 60 * 60,
    );
    append_window(
        &mut quotas,
        "weekly",
        "Weekly",
        object.get("seven_day"),
        7 * 24 * 60 * 60,
    );
    append_window(
        &mut quotas,
        "sonnet",
        "Sonnet",
        object.get("seven_day_sonnet"),
        7 * 24 * 60 * 60,
    );
    if let Some(limits) = object.get("limits").and_then(Value::as_array) {
        if let Some(limit) = limits.iter().find(|limit| {
            limit.get("kind").and_then(Value::as_str) == Some("weekly_scoped")
                && limit
                    .pointer("/scope/model/display_name")
                    .and_then(Value::as_str)
                    == Some("Fable")
        }) {
            append_percent(
                &mut quotas,
                "fable",
                "Fable",
                limit.get("percent").and_then(number),
                limit.get("resets_at").and_then(reset_date),
                7 * 24 * 60 * 60,
            );
        }
    }
    if let Some(extra) = object.get("extra_usage").and_then(Value::as_object) {
        if extra.get("is_enabled").and_then(Value::as_bool) == Some(true) {
            if let Some(used_cents) = extra.get("used_credits").and_then(number) {
                let used = used_cents / 100.0;
                let limit = extra
                    .get("monthly_limit")
                    .and_then(number)
                    .map(|v| v / 100.0);
                if limit.is_some_and(|value| value > 0.0) {
                    let limit_value = limit.unwrap_or_default();
                    quotas.push(QuotaWindow {
                        id: "extra".into(),
                        label: "Extra Usage".into(),
                        used_percent: (used / limit_value * 100.0).clamp(0.0, 100.0),
                        resets_at: None,
                        period_seconds: 0,
                        format: QuotaFormat::Dollars,
                        used_value: Some(used),
                        limit_value: Some(limit_value),
                        unit: None,
                        estimated: false,
                        source_note: None,
                    });
                } else if used > 0.0 {
                    value_metrics.push(ValueMetric {
                        id: "extra".into(),
                        label: "Extra Usage".into(),
                        values: vec![MetricValue {
                            number: used,
                            kind: MetricValueKind::Dollars,
                            label: Some("spent".into()),
                            estimated: false,
                        }],
                        expiries_at: Vec::new(),
                    });
                }
            }
        }
    }
    Ok(ClaudeMappedUsage {
        plan: format_plan(
            credentials.subscription_type.as_deref(),
            credentials.rate_limit_tier.as_deref(),
        ),
        quotas,
        value_metrics,
    })
}

fn append_window(
    output: &mut Vec<QuotaWindow>,
    id: &str,
    label: &str,
    value: Option<&Value>,
    period_seconds: u64,
) {
    let Some(object) = value.and_then(Value::as_object) else {
        return;
    };
    append_percent(
        output,
        id,
        label,
        object.get("utilization").and_then(number),
        object.get("resets_at").and_then(reset_date),
        period_seconds,
    );
}

fn append_percent(
    output: &mut Vec<QuotaWindow>,
    id: &str,
    label: &str,
    percent: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
    period_seconds: u64,
) {
    let Some(percent) = percent.filter(|value| value.is_finite()) else {
        return;
    };
    output.push(QuotaWindow {
        id: id.into(),
        label: label.into(),
        used_percent: percent,
        resets_at,
        period_seconds,
        format: QuotaFormat::Percent,
        used_value: None,
        limit_value: None,
        unit: None,
        estimated: false,
        source_note: None,
    });
}

/// Anthropic's one-off usage-limit reset grants (`cedar_ember` block of the usage endpoint).
#[derive(Debug, Clone, PartialEq)]
pub struct ResetGrantStatus {
    pub eligible: bool,
    pub grants: Vec<ResetGrant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResetGrant {
    pub resets_left: i64,
    pub ends_at: Option<DateTime<Utc>>,
}

pub fn parse_reset_grants(body: &Value) -> Option<ResetGrantStatus> {
    let block = body.get("cedar_ember")?.as_object()?;
    let mut grants = Vec::new();
    for entry in block
        .get("grants")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            return None;
        }
        let resets_left = entry
            .get("resets_left")
            .and_then(number)
            .filter(|value| value.is_finite())?;
        grants.push(ResetGrant {
            resets_left: resets_left.floor().max(0.0) as i64,
            ends_at: entry.get("ends_at").and_then(reset_date),
        });
    }
    Some(ResetGrantStatus {
        eligible: block.get("eligible").and_then(Value::as_bool) == Some(true),
        grants,
    })
}

/// A "Rate Limit Resets" values metric counting the still-future grants; elapsed grants drop out.
pub fn reset_grants_metric(status: &ResetGrantStatus, now: DateTime<Utc>) -> Option<ValueMetric> {
    if !status.eligible {
        return None;
    }
    let mut count = 0_i64;
    let mut expiries_at = Vec::new();
    for grant in &status.grants {
        if grant.resets_left < 1 {
            continue;
        }
        if grant.ends_at.is_some_and(|ends_at| ends_at <= now) {
            continue;
        }
        count += grant.resets_left;
        if let Some(ends_at) = grant.ends_at {
            expiries_at.extend(std::iter::repeat_n(ends_at, grant.resets_left as usize));
        }
    }
    Some(ValueMetric {
        id: "rateLimitResets".into(),
        label: "Rate Limit Resets".into(),
        values: vec![MetricValue {
            number: count as f64,
            kind: MetricValueKind::Count,
            label: Some("available".into()),
            estimated: false,
        }],
        expiries_at: {
            expiries_at.sort();
            expiries_at
        },
    })
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        // String-parsed values can be "NaN" or "inf"; neither is a real quota.
        .filter(|number| number.is_finite())
}

fn reset_date(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|date| date.to_utc())
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S%.f")
                    .ok()
                    .map(|date| date.and_utc())
            });
    }
    let raw = number(value)?;
    // Zero is a missing timestamp, not midnight on 1970-01-01.
    if raw <= 0.0 {
        return None;
    }
    let seconds = if raw.abs() < 10_000_000_000.0 {
        raw
    } else {
        raw / 1000.0
    };
    DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
}

fn format_plan(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    let subscription = subscription?.trim();
    if subscription.is_empty() {
        return None;
    }
    let mut plan = subscription
        .to_ascii_lowercase()
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(multiplier) = tier.and_then(|tier| {
        tier.split(|character: char| !character.is_ascii_alphanumeric())
            .find(|part| part.ends_with('x') && part[..part.len() - 1].parse::<u32>().is_ok())
    }) {
        plan.push(' ');
        plan.push_str(multiplier);
    }
    Some(plan)
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use serde_json::Value;

    use super::{map_usage, parse_reset_grants, reset_grants_metric};

    use chrono::TimeZone;

    fn grants_body(block: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "cedar_ember": block })
    }

    use crate::providers::claude::auth::ClaudeOAuth;

    #[test]
    fn maps_live_windows_scoped_limit_and_extra_usage() {
        let body: Value =
            serde_json::from_str(include_str!("fixtures/usage.json")).expect("valid fixture");
        let mapped = map_usage(
            StatusCode::OK,
            &body,
            &ClaudeOAuth {
                subscription_type: Some("pro".into()),
                rate_limit_tier: Some("default_5x".into()),
                ..ClaudeOAuth::default()
            },
        )
        .unwrap();
        assert_eq!(mapped.plan.as_deref(), Some("Pro 5x"));
        assert_eq!(mapped.quotas.len(), 5);
        assert_eq!(mapped.quotas[4].used_percent, 25.0);
        assert_eq!(mapped.quotas[4].used_value, Some(12.5));
        assert!(mapped.value_metrics.is_empty());
    }

    #[test]
    fn maps_uncapped_extra_usage_as_an_unbounded_value() {
        let body = serde_json::json!({
            "extra_usage": {
                "is_enabled": true,
                "used_credits": 123456,
                "monthly_limit": null
            }
        });
        let mapped = map_usage(StatusCode::OK, &body, &ClaudeOAuth::default()).unwrap();
        assert!(mapped.quotas.is_empty());
        assert_eq!(mapped.value_metrics[0].id, "extra");
        assert_eq!(mapped.value_metrics[0].values[0].number, 1234.56);
        assert_eq!(
            mapped.value_metrics[0].values[0].label.as_deref(),
            Some("spent")
        );
    }

    #[test]
    fn maps_microsecond_reset_without_a_timezone_as_utc() {
        let body = serde_json::json!({
            "five_hour": {
                "utilization": 0,
                "resets_at": "2099-06-01T12:00:00.123456"
            }
        });
        let mapped = map_usage(StatusCode::OK, &body, &ClaudeOAuth::default()).unwrap();
        assert_eq!(
            mapped.quotas[0].resets_at.unwrap().to_rfc3339(),
            "2099-06-01T12:00:00.123456+00:00"
        );
    }
    #[test]
    fn reset_grants_parse_counts_future_grants_and_skips_elapsed_ones() {
        let body = grants_body(serde_json::json!({
            "eligible": true,
            "next_grant_id": "g2",
            "grants": [
                {"id": "g1", "resets_left": 2, "ends_at": "2099-01-01T00:00:00Z", "clears": ["five_hour"]},
                {"id": "g2", "resets_left": 1},
                {"id": "g3", "resets_left": 3, "ends_at": "2000-01-01T00:00:00Z"}
            ]
        }));
        let status = parse_reset_grants(&body).unwrap();
        assert!(status.eligible);
        assert_eq!(status.grants.len(), 3);

        let now = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let metric = reset_grants_metric(&status, now).unwrap();
        assert_eq!(metric.id, "rateLimitResets");
        assert_eq!(metric.values[0].number, 3.0);
        assert_eq!(metric.expiries_at.len(), 2);
    }

    #[test]
    fn reset_grants_block_is_rejected_when_a_grant_is_malformed() {
        let malformed_id = grants_body(serde_json::json!({
            "eligible": true,
            "grants": [{"id": "", "resets_left": 1}]
        }));
        assert!(parse_reset_grants(&malformed_id).is_none());

        let missing_count = grants_body(serde_json::json!({
            "eligible": true,
            "grants": [{"id": "g1"}]
        }));
        assert!(parse_reset_grants(&missing_count).is_none());
    }

    #[test]
    fn reset_grants_without_eligibility_or_block_produce_nothing() {
        let now = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();

        let ineligible = grants_body(serde_json::json!({
            "eligible": false,
            "grants": [{"id": "g1", "resets_left": 2}]
        }));
        let status = parse_reset_grants(&ineligible).unwrap();
        assert!(reset_grants_metric(&status, now).is_none());

        assert!(parse_reset_grants(&serde_json::json!({})).is_none());
    }
}
