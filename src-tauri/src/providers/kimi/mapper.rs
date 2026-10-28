use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{QuotaFormat, QuotaWindow};

use super::KimiError;

const MONTHLY_PERIOD_SECONDS: u64 = 30 * 24 * 60 * 60;
const DEFAULT_WINDOW_PERIOD_SECONDS: u64 = 5 * 60 * 60;

#[derive(Debug, PartialEq)]
pub struct KimiMappedUsage {
    pub plan: Option<String>,
    pub quotas: Vec<QuotaWindow>,
}

pub fn map_usage(body: &Value) -> Result<KimiMappedUsage, KimiError> {
    Ok(KimiMappedUsage {
        plan: plan_name(body),
        quotas: map_quotas(body)?,
    })
}

fn plan_name(body: &Value) -> Option<String> {
    let raw = body
        .get("user")?
        .get("membership")?
        .get("level")?
        .as_str()?;
    let stripped = raw.trim().strip_prefix("LEVEL_").unwrap_or(raw).trim();
    if stripped.is_empty() {
        None
    } else {
        Some(title_case(stripped))
    }
}

fn title_case(value: &str) -> String {
    let mut out = String::new();
    let mut new_word = true;
    for ch in value.chars() {
        if ch == '_' {
            new_word = true;
            out.push(' ');
            continue;
        }
        if new_word {
            out.extend(ch.to_uppercase());
            new_word = false;
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn map_quotas(body: &Value) -> Result<Vec<QuotaWindow>, KimiError> {
    // Kimi exposes two windows: the rolling 5-hour session and the monthly pool (their console
    // shows the same pair). The weekly window and the legacy count-based "usage" section are
    // gone from Kimi and are ignored here.
    let mut quotas = Vec::new();
    let session = ratio_pool_quota(
        body,
        "limit_5h",
        "session",
        "Session",
        DEFAULT_WINDOW_PERIOD_SECONDS,
    )
    .or_else(|| session_quota(body).ok().flatten());
    if let Some(session) = session {
        quotas.push(session);
    }
    if let Some(monthly) = ratio_pool_quota(
        body,
        "limit_month_total",
        "monthly",
        "Monthly",
        MONTHLY_PERIOD_SECONDS,
    ) {
        quotas.push(monthly);
    }
    (!quotas.is_empty())
        .then_some(quotas)
        .ok_or(KimiError::InvalidResponse)
}

fn ratio_pool_quota(
    body: &Value,
    pool_name: &str,
    id: &str,
    label: &str,
    period_seconds: u64,
) -> Option<QuotaWindow> {
    let pool = body
        .get("usages")
        .and_then(|usages| usages.get(pool_name))
        .and_then(Value::as_object)?;
    let used_ratio = number(pool.get("used_ratio")).filter(|value| *value >= 0.0)?;
    Some(QuotaWindow {
        id: id.into(),
        label: label.into(),
        used_percent: (used_ratio * 100.0).clamp(0.0, 100.0),
        resets_at: iso_time(pool.get("reset_time")),
        period_seconds,
        format: QuotaFormat::Percent,
        used_value: None,
        limit_value: None,
        unit: None,
        estimated: false,
        source_note: None,
    })
}

fn session_quota(body: &Value) -> Result<Option<QuotaWindow>, KimiError> {
    let Some(entry) = body
        .get("limits")
        .and_then(Value::as_array)
        .and_then(|limits| {
            limits
                .iter()
                .find(|entry| window_period_seconds(entry) == Some(DEFAULT_WINDOW_PERIOD_SECONDS))
        })
    else {
        return Ok(None);
    };
    let detail = entry
        .get("detail")
        .and_then(Value::as_object)
        .ok_or(KimiError::InvalidResponse)?;
    let limit = number(detail.get("limit"))
        .filter(|value| *value >= 0.0)
        .ok_or(KimiError::InvalidResponse)?;
    let remaining = number(detail.get("remaining"))
        .filter(|value| *value >= 0.0)
        .ok_or(KimiError::InvalidResponse)?;
    let used = (limit - remaining).max(0.0);
    let used_percent = if limit > 0.0 {
        (used / limit * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let period_seconds = window_period_seconds(entry).unwrap_or(DEFAULT_WINDOW_PERIOD_SECONDS);

    Ok(Some(QuotaWindow {
        id: "session".into(),
        label: "Session".into(),
        used_percent,
        resets_at: iso_time(detail.get("resetTime")),
        period_seconds,
        format: QuotaFormat::Percent,
        used_value: None,
        limit_value: None,
        unit: None,
        estimated: false,
        source_note: None,
    }))
}

fn window_period_seconds(entry: &Value) -> Option<u64> {
    let window = entry.get("window")?;
    let duration = number(window.get("duration")).filter(|value| *value > 0.0)?;
    let factor = match window.get("timeUnit").and_then(Value::as_str) {
        Some("TIME_UNIT_HOUR") => 3600.0,
        Some("TIME_UNIT_SECOND") => 1.0,
        Some("TIME_UNIT_MINUTE") => 60.0,
        _ => return None,
    };
    Some((duration * factor) as u64)
}

fn number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .filter(|value| value.is_finite())
}

fn iso_time(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let text = value?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{map_usage, plan_name, DEFAULT_WINDOW_PERIOD_SECONDS};
    use crate::models::QuotaFormat;

    fn captured() -> Value {
        serde_json::json!({
            "user": {"membership":{"level":"LEVEL_BASIC"}},
            "usage": {"limit":"100","used":"25","resetTime":"2026-08-10T02:17:43.139020Z"},
            "limits": [{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                        "detail":{"limit":"100","remaining":"80",
                                  "resetTime":"2026-08-07T06:17:43.139020Z"}}],
            "parallel": {"limit":"10"}
        })
    }

    #[test]
    fn captured_payload_maps_usage_window_and_plan() {
        let mapped = map_usage(&captured()).unwrap();

        assert_eq!(mapped.plan.as_deref(), Some("Basic"));
        assert_eq!(
            mapped
                .quotas
                .iter()
                .map(|quota| quota.id.as_str())
                .collect::<Vec<_>>(),
            ["session"]
        );

        let session = &mapped.quotas[0];
        assert_eq!(session.used_percent, 20.0);
        assert_eq!(session.period_seconds, 5 * 60 * 60);
        assert_eq!(session.format, QuotaFormat::Percent);
        assert_eq!(session.used_value, None);
        assert_eq!(session.limit_value, None);
        assert_eq!(session.unit, None);
    }

    #[test]
    fn ratio_pool_payload_maps_session_and_monthly_quotas() {
        let mapped = map_usage(&json!({
            "user": {"membership": {"level": "LEVEL_BASIC"}},
            "usages": {
                "limit_5h": {
                    "used_ratio": 0.2,
                    "reset_time": "2026-10-28T15:00:00Z"
                },
                "limit_month_total": {
                    "used_ratio": 0.4,
                    "reset_time": "2026-11-15T00:00:00Z"
                }
            }
        }))
        .unwrap();

        assert_eq!(mapped.plan.as_deref(), Some("Basic"));
        assert_eq!(
            mapped
                .quotas
                .iter()
                .map(|quota| (quota.id.as_str(), quota.used_percent))
                .collect::<Vec<_>>(),
            [("session", 20.0), ("monthly", 40.0)]
        );
        assert!(mapped.quotas.iter().all(|quota| quota.resets_at.is_some()));
        let monthly = mapped
            .quotas
            .iter()
            .find(|quota| quota.id == "monthly")
            .unwrap();
        assert_eq!(monthly.period_seconds, 30 * 24 * 60 * 60);
    }

    #[test]
    fn the_retired_weekly_window_is_ignored_wherever_it_appears() {
        let with_ratio_pool = map_usage(&json!({
            "usages": {
                "limit_5h": {"used_ratio": 0.1, "reset_time": "2026-10-28T15:00:00Z"},
                "limit_7d": {"used_ratio": 0.9, "reset_time": "2026-11-01T00:00:00Z"},
                "limit_month_total": {"used_ratio": 0.5, "reset_time": "2026-11-15T00:00:00Z"},
                "limit_month_code": {"used_ratio": 0.4, "reset_time": "2026-11-15T00:00:00Z"}
            }
        }))
        .unwrap();
        assert_eq!(
            with_ratio_pool
                .quotas
                .iter()
                .map(|quota| quota.id.as_str())
                .collect::<Vec<_>>(),
            ["session", "monthly"]
        );

        // The legacy count-based usage section used to carry the weekly window.
        let legacy = map_usage(&json!({
            "user": {"membership": {"level": "LEVEL_PRO"}},
            "usage": {"limit": "50", "used": "0", "resetTime": null},
            "limits": [
                {"window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"},
                 "detail": {"limit": "100", "remaining": "75"}}
            ]
        }))
        .unwrap();
        assert_eq!(
            legacy
                .quotas
                .iter()
                .map(|quota| quota.id.as_str())
                .collect::<Vec<_>>(),
            ["session"]
        );
    }

    #[test]
    fn out_of_range_ratio_pools_degrade_safely() {
        let mapped = map_usage(&json!({
            "usages": {"limit_5h": {"used_ratio": 1.5, "reset_time": "2026-10-28T15:00:00Z"}}
        }))
        .unwrap();
        assert_eq!(mapped.quotas[0].used_percent, 100.0);

        let unusable = map_usage(&json!({
            "usages": {"limit_5h": {"used_ratio": null}}
        }))
        .unwrap_err();
        assert_eq!(unusable, super::super::KimiError::InvalidResponse);
    }

    #[test]
    fn a_payload_with_no_usable_window_is_rejected() {
        assert!(map_usage(&json!({"limits":[]})).is_err());

        // The legacy usage section alone is no longer a window source.
        assert!(map_usage(&json!({
            "user": {"membership": {"level": "LEVEL_PRO"}},
            "usage": {"limit": "50", "used": "0"}
        }))
        .is_err());
    }

    #[test]
    fn session_quota_selects_the_five_hour_window_regardless_of_order() {
        let mapped = map_usage(&json!({
            "user": {"membership": {"level": "LEVEL_PRO"}},
            "usage": {"limit": "100", "used": "0"},
            "limits": [
                {"window": {"duration": 1, "timeUnit": "TIME_UNIT_HOUR"},
                 "detail": {"limit": "100", "remaining": "0"}},
                {"window": {"duration": 5, "timeUnit": "TIME_UNIT_HOUR"},
                 "detail": {"limit": "100", "remaining": "75"}}
            ]
        }))
        .unwrap();
        let session = mapped
            .quotas
            .iter()
            .find(|quota| quota.id == "session")
            .unwrap();
        assert_eq!(session.used_percent, 25.0);
        assert_eq!(session.period_seconds, DEFAULT_WINDOW_PERIOD_SECONDS);
    }

    #[test]
    fn an_invalid_session_section_yields_invalid_response() {
        let mut body = captured();
        body["limits"][0]["detail"]["remaining"] = json!("not-a-number");

        assert_eq!(
            map_usage(&body).unwrap_err(),
            super::super::KimiError::InvalidResponse
        );
    }

    #[test]
    fn plan_level_is_optional_and_title_cased() {
        assert_eq!(
            plan_name(&json!({"user":{"membership":{"level":"LEVEL_BASIC"}}})).as_deref(),
            Some("Basic")
        );
        assert_eq!(
            plan_name(&json!({"user":{"membership":{"level":"LEVEL_YEARLY_PRO"}}})).as_deref(),
            Some("Yearly Pro")
        );
        assert_eq!(plan_name(&json!({"user":{"membership":{}}})), None);
        assert_eq!(plan_name(&json!({})), None);
    }
}
