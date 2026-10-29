use chrono::DateTime;
use serde_json::Value;

use crate::models::{QuotaFormat, QuotaWindow};

const MODEL_BLACKLIST: [&str; 9] = [
    "MODEL_CHAT_20706",
    "MODEL_CHAT_23310",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
    "MODEL_GOOGLE_GEMINI_2_5_PRO",
    "MODEL_PLACEHOLDER_M19",
    "MODEL_PLACEHOLDER_M9",
    "MODEL_PLACEHOLDER_M12",
];

#[derive(Debug)]
pub struct ModelConfig {
    label: String,
    model_id: Option<String>,
    // None when the host omitted quotaInfo; such a model must not vote on the
    // pool minimum, or a missing field reads as 100% used.
    remaining_fraction: Option<f64>,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
}

const ARGON_GROUP_NAME: &str = "Gemini 4 Argon";

const BUCKETS: [(&str, &str, &str, u64); 4] = [
    ("gemini-5h", "geminiPro", "Five Hour", 5 * 60 * 60),
    ("gemini-weekly", "geminiWeekly", "Weekly", 7 * 24 * 60 * 60),
    ("3p-5h", "claude", "Claude", 5 * 60 * 60),
    (
        "3p-weekly",
        "claudeWeekly",
        "Claude Weekly",
        7 * 24 * 60 * 60,
    ),
];

/// Why an Argon window could not be built; logged at debug level so live payload drift is
/// diagnosable instead of silently dropping the meter.
#[derive(Debug, PartialEq)]
pub enum ArgonReject {
    NoGroups,
    NoArgonGroup,
    WeeklyBucketCount(usize),
    MissingBucketId,
    SharedBucketId,
    BucketDisabled,
    MissingFraction,
}

pub fn parse_quota_summary(value: &Value) -> Option<Vec<QuotaWindow>> {
    parse_quota_summaries(std::slice::from_ref(value))
}

/// One or more `RetrieveUserQuotaSummary` bodies merged by bucket id: the first usable bucket for
/// each id wins, because a single host can omit a whole pool (the daily host has returned only the
/// Claude group). Argon is judged per body so two hosts' Argon groups never read as one.
pub fn parse_quota_summaries(values: &[Value]) -> Option<Vec<QuotaWindow>> {
    let group_lists = values
        .iter()
        .filter_map(|value| {
            value
                .pointer("/response/groups")
                .or_else(|| value.get("groups"))?
                .as_array()
        })
        .collect::<Vec<_>>();
    if group_lists.is_empty() {
        return None;
    }

    let mut found = std::collections::HashMap::new();
    let mut argon = None;
    for groups in &group_lists {
        let mut argon_buckets = Vec::new();
        let mut seen_argon_group = false;
        for group in groups.iter() {
            let argon_group =
                group.get("displayName").and_then(Value::as_str) == Some(ARGON_GROUP_NAME);
            seen_argon_group |= argon_group;
            for bucket in group
                .get("buckets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if argon_group {
                    argon_buckets.push(bucket);
                    continue;
                }
                pool_bucket(bucket, &mut found);
            }
        }
        if argon.is_none() {
            if !seen_argon_group {
                crate::app_debug!(
                    "antigravity",
                    "summary body has no '{}' group (groups: {})",
                    ARGON_GROUP_NAME,
                    group_outline(groups)
                );
            } else {
                argon = argon_window_checked(&argon_buckets)
                    .inspect_err(|_| {
                        crate::app_debug!(
                            "antigravity",
                            "summary body Argon buckets: {}",
                            bucket_outline(&argon_buckets)
                        );
                    })
                    .ok();
            }
        }
    }

    let mut quotas: Vec<QuotaWindow> = BUCKETS
        .iter()
        .filter_map(|(bucket_id, quota_id, label, period_seconds)| {
            let (remaining, resets_at) = found.get(*bucket_id)?;
            Some(quota_window(
                quota_id,
                label,
                *remaining,
                *resets_at,
                *period_seconds,
            ))
        })
        .collect();
    if let Some(argon) = argon {
        insert_argon(&mut quotas, argon);
    }
    // A body that answered with groups but no known bucket (signed out, renamed
    // buckets) must read as "no data" so the caller falls back to the other
    // sources instead of reporting an empty-but-healthy quota list.
    if quotas.is_empty() {
        return None;
    }
    Some(quotas)
}

/// Argon Weekly slots in between the Gemini and third-party pools.
pub fn insert_argon(quotas: &mut Vec<QuotaWindow>, argon: QuotaWindow) {
    let index = quotas
        .iter()
        .position(|quota| quota.id.starts_with("claude"))
        .unwrap_or(quotas.len());
    quotas.insert(index, argon);
}

/// `agy -p "/quota" --output-format json` nests the groups under `command.data` and renames the
/// canonical fields (`name`, `remaining_fraction`); normalize into the summary bucket shape.
pub fn parse_agy_cli_quota_checked(value: &Value) -> Result<QuotaWindow, ArgonReject> {
    let groups = value
        .pointer("/command/data/groups")
        .or_else(|| value.pointer("/response/groups"))
        .or_else(|| value.get("groups"))
        .and_then(Value::as_array)
        .ok_or(ArgonReject::NoGroups)?;
    let argon_groups = groups
        .iter()
        .filter(|group| {
            group
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| group.get("displayName").and_then(Value::as_str))
                == Some(ARGON_GROUP_NAME)
        })
        .collect::<Vec<_>>();
    if argon_groups.is_empty() {
        crate::app_debug!(
            "antigravity",
            "agy report has no '{}' group (groups: {})",
            ARGON_GROUP_NAME,
            group_outline(groups)
        );
        return Err(ArgonReject::NoArgonGroup);
    }
    let buckets = argon_groups
        .iter()
        .filter_map(|group| group.get("buckets").and_then(Value::as_array))
        .flatten()
        .map(|bucket| {
            let mut normalized = bucket.clone();
            if let Some(fraction) = bucket.get("remaining_fraction").and_then(number) {
                normalized["remainingFraction"] = Value::from(fraction);
            }
            if let Some(id) = bucket
                .get("bucket_id")
                .or_else(|| bucket.get("id"))
                .and_then(Value::as_str)
            {
                normalized["bucketId"] = Value::from(id);
            }
            normalized
        })
        .collect::<Vec<_>>();
    let refs: Vec<&Value> = buckets.iter().collect();
    argon_window_checked(&refs)
}

fn pool_bucket(
    bucket: &Value,
    found: &mut std::collections::HashMap<String, (f64, Option<DateTime<chrono::Utc>>)>,
) {
    let Some(id) = bucket.get("bucketId").and_then(Value::as_str) else {
        return;
    };
    if !BUCKETS.iter().any(|spec| spec.0 == id) || found.contains_key(id) {
        return;
    }
    let Some(fraction) = bucket.get("remainingFraction").and_then(number) else {
        return;
    };
    let reset = bucket
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.to_utc());
    found.insert(id.to_owned(), (fraction, reset));
}

/// Argon ships its own quota group instead of a stable bucket id, so the group name plus exactly
/// one weekly bucket identifies the window; anything ambiguous is dropped rather than guessed.
fn argon_window_checked(buckets: &[&Value]) -> Result<QuotaWindow, ArgonReject> {
    let weekly = buckets
        .iter()
        .filter(|bucket| bucket.get("window").and_then(Value::as_str) == Some("weekly"))
        .collect::<Vec<_>>();
    if weekly.len() != 1 {
        crate::app_debug!(
            "antigravity",
            "Argon needs exactly one weekly bucket, found {}",
            weekly.len()
        );
        return Err(ArgonReject::WeeklyBucketCount(weekly.len()));
    }
    let bucket = weekly[0];
    if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
        crate::app_debug!("antigravity", "Argon weekly bucket is disabled");
        return Err(ArgonReject::BucketDisabled);
    }
    let id = bucket
        .get("bucketId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            crate::app_debug!("antigravity", "Argon weekly bucket has no bucketId");
            ArgonReject::MissingBucketId
        })?;
    if BUCKETS.iter().any(|spec| spec.0 == id) {
        crate::app_debug!(
            "antigravity",
            "Argon weekly bucket id '{id}' collides with a standard pool bucket"
        );
        return Err(ArgonReject::SharedBucketId);
    }
    let fraction = bucket
        .get("remainingFraction")
        .and_then(number)
        .ok_or_else(|| {
            crate::app_debug!(
                "antigravity",
                "Argon weekly bucket '{id}' has no usable remainingFraction"
            );
            ArgonReject::MissingFraction
        })?;
    let reset = bucket
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
        .map(|date| date.to_utc());
    crate::app_debug!(
        "antigravity",
        "Argon weekly accepted: bucket '{id}', {:.1}% remaining, reset {}",
        fraction * 100.0,
        reset
            .map(|reset| reset.to_rfc3339())
            .unwrap_or_else(|| "n/a".into())
    );
    Ok(quota_window(
        "argonWeekly",
        "Argon Weekly",
        fraction,
        reset,
        7 * 24 * 60 * 60,
    ))
}

fn group_outline(groups: &[Value]) -> String {
    let names = groups
        .iter()
        .map(|group| {
            group
                .get("displayName")
                .or_else(|| group.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("?")
        })
        .collect::<Vec<_>>();
    if names.is_empty() {
        "none".to_owned()
    } else {
        format!("[{}]", names.join(", "))
    }
}

fn bucket_outline(buckets: &[&Value]) -> String {
    if buckets.is_empty() {
        return "none".to_owned();
    }
    let items = buckets
        .iter()
        .map(|bucket| {
            format!(
                "{}/{}/{}{}",
                bucket
                    .get("bucketId")
                    .or_else(|| bucket.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                bucket.get("window").and_then(Value::as_str).unwrap_or("?"),
                bucket
                    .get("remainingFraction")
                    .or_else(|| bucket.get("remaining_fraction"))
                    .and_then(number)
                    .map(|fraction| format!("{fraction:.2}"))
                    .unwrap_or_else(|| "?".to_owned()),
                if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
                    "/disabled"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

fn quota_window(
    quota_id: &str,
    label: &str,
    remaining: f64,
    resets_at: Option<DateTime<chrono::Utc>>,
    period_seconds: u64,
) -> QuotaWindow {
    QuotaWindow {
        id: quota_id.into(),
        label: label.into(),
        used_percent: ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).round(),
        resets_at,
        period_seconds,
        format: QuotaFormat::Percent,
        used_value: None,
        limit_value: None,
        unit: None,
        estimated: false,
        source_note: None,
    }
}

pub fn parse_plan(value: &Value) -> Option<String> {
    let raw = value
        .pointer("/userStatus/userTier/name")
        .or_else(|| value.pointer("/userStatus/planStatus/planInfo/planName"))
        .and_then(Value::as_str)?
        .trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(value) = raw.strip_prefix("Google AI ") {
        return Some(title_case(value));
    }
    for tier in ["Ultra", "Pro", "Free"] {
        if raw
            .to_ascii_lowercase()
            .contains(&tier.to_ascii_lowercase())
        {
            return Some(tier.into());
        }
    }
    Some(title_case(raw))
}

pub fn parse_user_status(value: &Value) -> Option<(Option<String>, Vec<ModelConfig>)> {
    value.get("userStatus")?;
    let configs = value
        .pointer("/userStatus/cascadeModelConfigData/clientModelConfigs")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(model_from_ls).collect())
        .unwrap_or_default();
    Some((parse_plan(value), configs))
}

pub fn parse_command_model_configs(value: &Value) -> Option<Vec<ModelConfig>> {
    Some(
        value
            .get("clientModelConfigs")?
            .as_array()?
            .iter()
            .filter_map(model_from_ls)
            .collect(),
    )
}

pub fn parse_cloud_models(value: &Value) -> Vec<ModelConfig> {
    let Some(models) = value.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|(key, model)| {
            if model.get("isInternal").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            let label = model
                .get("displayName")
                .and_then(Value::as_str)
                .and_then(trimmed)
                .or_else(|| model.get("label").and_then(Value::as_str).and_then(trimmed))?;
            let model_id = model
                .get("model")
                .and_then(Value::as_str)
                .and_then(trimmed)
                .unwrap_or(key)
                .to_owned();
            Some(model_config(label, Some(model_id), model.get("quotaInfo")))
        })
        .collect()
}

pub fn parse_quota_buckets(value: &Value) -> Vec<ModelConfig> {
    value
        .get("buckets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bucket| {
            let id = bucket
                .get("modelId")
                .and_then(Value::as_str)
                .and_then(trimmed)?
                .to_owned();
            Some(model_config(&id, Some(id.clone()), Some(bucket)))
        })
        .collect()
}

pub fn build_legacy_quotas(configs: Vec<ModelConfig>) -> Vec<QuotaWindow> {
    let mut gemini: Option<(f64, Option<chrono::DateTime<chrono::Utc>>)> = None;
    let mut claude: Option<(f64, Option<chrono::DateTime<chrono::Utc>>)> = None;
    for config in configs {
        if config
            .model_id
            .as_deref()
            .is_some_and(|id| MODEL_BLACKLIST.contains(&id))
        {
            continue;
        }
        let Some(fraction) = config.remaining_fraction else {
            continue;
        };
        let pool = if config.label.to_ascii_lowercase().contains("gemini") {
            &mut gemini
        } else {
            &mut claude
        };
        if pool
            .as_ref()
            .is_none_or(|(remaining, _)| fraction < *remaining)
        {
            *pool = Some((fraction, config.resets_at));
        }
    }
    [
        ("geminiPro", "Five Hour", gemini),
        ("claude", "Claude", claude),
    ]
    .into_iter()
    .filter_map(|(id, label, pool)| {
        let (remaining, resets_at) = pool?;
        Some(quota_window(id, label, remaining, resets_at, 5 * 60 * 60))
    })
    .collect()
}

fn model_from_ls(value: &Value) -> Option<ModelConfig> {
    let label = value
        .get("label")
        .and_then(Value::as_str)
        .and_then(trimmed)?;
    let model_id = value
        .pointer("/modelOrAlias/model")
        .and_then(Value::as_str)
        .and_then(trimmed)
        .map(str::to_owned);
    Some(model_config(label, model_id, value.get("quotaInfo")))
}

fn model_config(label: &str, model_id: Option<String>, quota: Option<&Value>) -> ModelConfig {
    ModelConfig {
        label: label.to_owned(),
        model_id,
        remaining_fraction: quota
            .and_then(|value| value.get("remainingFraction"))
            .and_then(number),
        resets_at: quota
            .and_then(|value| value.get("resetTime"))
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|date| date.to_utc()),
    }
}

fn trimmed(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn title_case(value: &str) -> String {
    value
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        build_legacy_quotas, parse_agy_cli_quota_checked, parse_cloud_models,
        parse_command_model_configs, parse_quota_buckets, parse_quota_summaries,
        parse_quota_summary, parse_user_status,
    };

    #[test]
    fn legacy_bucket_id_wins_over_the_newer_id_field() {
        let parsed = parse_agy_cli_quota_checked(&json!({
            "groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"bucket_id": "legacy-id", "id": "newer-id", "window": "weekly",
                     "remaining_fraction": 0.19}
                ]}
            ]
        }));

        let argon = parsed.unwrap();
        assert_eq!(argon.id, "argonWeekly");
    }

    #[test]
    fn maps_authoritative_pool_summary_in_fixed_order() {
        let value: Value = serde_json::from_str(include_str!("fixtures/quota_summary.json"))
            .expect("valid fixture");
        let quotas = parse_quota_summary(&value).unwrap();
        assert_eq!(quotas.len(), 2);
        assert_eq!(quotas[0].id, "geminiPro");
        assert_eq!(quotas[0].used_percent, 20.0);
        assert_eq!(quotas[1].id, "claudeWeekly");
    }

    #[test]
    fn gemini_five_hour_window_is_named_like_the_antigravity_app() {
        let value = summary_with_argon(argon_group(json!([
            {"bucketId": "gemini-4-argon-weekly", "window": "weekly", "remainingFraction": 1.0}
        ])));
        let quotas = parse_quota_summary(&value).unwrap();
        let gemini = quotas.iter().find(|quota| quota.id == "geminiPro").unwrap();
        assert_eq!(gemini.label, "Five Hour");
        let claude = quotas.iter().find(|quota| quota.id == "claude").unwrap();
        assert_eq!(claude.label, "Claude");
    }

    fn summary_with_argon(argon_group: Value) -> Value {
        json!({"groups": [
            {"displayName": "Gemini Models", "buckets": [
                {"bucketId": "gemini-5h", "remainingFraction": 0.8, "window": "5h"},
                {"bucketId": "gemini-weekly", "remainingFraction": 0.5, "window": "weekly"}
            ]},
            argon_group,
            {"displayName": "Claude and GPT models", "buckets": [
                {"bucketId": "3p-5h", "remainingFraction": 0.7, "window": "5h"},
                {"bucketId": "3p-weekly", "remainingFraction": 0.6, "window": "weekly"}
            ]}
        ]})
    }

    fn argon_group(buckets: Value) -> Value {
        json!({"displayName": "Gemini 4 Argon", "buckets": buckets})
    }

    #[test]
    fn summary_orders_session_weekly_argon_then_claude() {
        let value = summary_with_argon(argon_group(json!([
            {"bucketId": "gemini-4-argon-weekly", "window": "weekly", "remainingFraction": 0.19}
        ])));
        let quotas = parse_quota_summary(&value).unwrap();
        assert_eq!(
            quotas.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            [
                "geminiPro",
                "geminiWeekly",
                "argonWeekly",
                "claude",
                "claudeWeekly"
            ]
        );
        assert_eq!(quotas[2].used_percent, 81.0);
        assert_eq!(quotas[2].period_seconds, 7 * 24 * 60 * 60);
    }

    #[test]
    fn ambiguous_argon_buckets_are_dropped() {
        let two_weekly = summary_with_argon(argon_group(json!([
            {"bucketId": "a", "window": "weekly", "remainingFraction": 0.9},
            {"bucketId": "b", "window": "weekly", "remainingFraction": 0.8}
        ])));
        assert!(parse_quota_summary(&two_weekly)
            .unwrap()
            .iter()
            .all(|quota| quota.id != "argonWeekly"));

        let disabled = summary_with_argon(argon_group(json!([
            {"bucketId": "a", "window": "weekly", "remainingFraction": 0.9, "disabled": true}
        ])));
        assert!(parse_quota_summary(&disabled)
            .unwrap()
            .iter()
            .all(|quota| quota.id != "argonWeekly"));

        let shared_id = summary_with_argon(argon_group(json!([
            {"bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.9}
        ])));
        let quotas = parse_quota_summary(&shared_id).unwrap();
        assert_eq!(quotas.iter().filter(|q| q.id == "geminiWeekly").count(), 1);
        assert!(quotas.iter().all(|quota| quota.id != "argonWeekly"));
    }

    #[test]
    fn agy_cli_print_payload_maps_with_snake_case_fields() {
        let value = json!({
            "response": "Gemini 4 Argon Weekly Limit: 81%",
            "command": {"data": {"groups": [
                {"name": "Gemini Models", "buckets": [
                    {"bucket_id": "gemini-5h", "remaining_fraction": 0.9}
                ]},
                {"name": "Gemini 4 Argon", "buckets": [
                    {"id": "gemini-argon-wtu-weekly", "window": "weekly", "remaining_fraction": 0.19}
                ]}
            ]}}
        });
        let argon = parse_agy_cli_quota_checked(&value).unwrap();
        assert_eq!(argon.id, "argonWeekly");
        assert_eq!(argon.used_percent, 81.0);

        assert!(
            parse_agy_cli_quota_checked(&json!({"command": {"data": {"groups": []}}})).is_err()
        );
        assert!(parse_agy_cli_quota_checked(&json!({})).is_err());
    }

    #[test]
    fn argon_rejections_carry_the_reason_a_bucket_was_dropped() {
        use super::{parse_agy_cli_quota_checked, ArgonReject};

        assert_eq!(
            parse_agy_cli_quota_checked(&json!({})),
            Err(ArgonReject::NoGroups)
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini Models", "buckets": []}
            ]})),
            Err(ArgonReject::NoArgonGroup)
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"id": "a", "window": "weekly", "remaining_fraction": 0.9},
                    {"id": "b", "window": "weekly", "remaining_fraction": 0.8}
                ]}
            ]})),
            Err(ArgonReject::WeeklyBucketCount(2))
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"id": "a", "window": "weekly", "remaining_fraction": 0.9, "disabled": true}
                ]}
            ]})),
            Err(ArgonReject::BucketDisabled)
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.9}
                ]}
            ]})),
            Err(ArgonReject::SharedBucketId)
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"id": "argon-x", "window": "weekly"}
                ]}
            ]})),
            Err(ArgonReject::MissingFraction)
        );
        assert_eq!(
            parse_agy_cli_quota_checked(&json!({"groups": [
                {"name": "Gemini 4 Argon", "buckets": [
                    {"window": "weekly", "remaining_fraction": 0.9}
                ]}
            ]})),
            Err(ArgonReject::MissingBucketId)
        );
    }

    #[test]
    fn summaries_merge_across_hosts_with_first_wins_and_body_local_argon() {
        let daily = json!({"groups": [
            {"displayName": "Gemini Models", "buckets": [
                {"bucketId": "gemini-5h", "remainingFraction": 0.8}
            ]},
            {"displayName": "Claude and GPT models", "buckets": [
                {"bucketId": "3p-5h", "remainingFraction": 0.7}
            ]}
        ]});
        let production = json!({"groups": [
            {"displayName": "Gemini Models", "buckets": [
                {"bucketId": "gemini-5h", "remainingFraction": 0.1},
                {"bucketId": "gemini-weekly", "remainingFraction": 0.5}
            ]},
            argon_group(json!([
                {"bucketId": "argon-weekly", "window": "weekly", "remainingFraction": 0.3}
            ]))
        ]});
        let quotas = parse_quota_summaries(&[daily, production]).unwrap();
        assert_eq!(
            quotas.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(),
            ["geminiPro", "geminiWeekly", "argonWeekly", "claude"]
        );
        assert_eq!(quotas[0].used_percent, 20.0);
    }

    #[test]
    fn pools_language_server_models_by_the_worst_remaining_fraction() {
        let value = json!({"userStatus": {
            "userTier": {"name": "Google AI Pro"},
            "cascadeModelConfigData": {"clientModelConfigs": [
                {"label": "Gemini 3 Pro", "quotaInfo": {"remainingFraction": 0.5}},
                {"label": "Gemini Flash", "quotaInfo": {"remainingFraction": 0.8}},
                {"label": "Claude Sonnet", "quotaInfo": {"remainingFraction": 0.7}}
            ]}
        }});
        let (plan, configs) = parse_user_status(&value).unwrap();
        let quotas = build_legacy_quotas(configs);
        assert_eq!(plan.as_deref(), Some("Pro"));
        assert_eq!(
            quotas
                .iter()
                .map(|quota| quota.id.as_str())
                .collect::<Vec<_>>(),
            ["geminiPro", "claude"]
        );
        assert_eq!(quotas[0].used_percent, 50.0);
        assert_eq!(quotas[1].used_percent, 30.0);
    }

    #[test]
    fn command_models_cloud_models_and_quota_buckets_share_pooling_rules() {
        let command = json!({"clientModelConfigs": [
            {"label": "Claude Opus", "quotaInfo": {"remainingFraction": 0.25}}
        ]});
        assert_eq!(
            build_legacy_quotas(parse_command_model_configs(&command).unwrap())[0].id,
            "claude"
        );

        let cloud = json!({"models": {
            "good": {"model": "MODEL_GOOD", "displayName": "Gemini 3 Pro", "quotaInfo": {"remainingFraction": 0.4}},
            "internal": {"displayName": "Claude Internal", "isInternal": true},
            "blacklisted": {"model": "MODEL_GOOGLE_GEMINI_2_5_PRO", "displayName": "Gemini 2.5 Pro"}
        }});
        let quotas = build_legacy_quotas(parse_cloud_models(&cloud));
        assert_eq!(quotas.len(), 1);
        assert_eq!(quotas[0].used_percent, 60.0);

        let buckets = json!({"buckets": [
            {"modelId": "gemini-3-pro", "remainingFraction": 0.5},
            {"modelId": "gemini-3-flash", "remainingFraction": 0.9}
        ]});
        let quotas = build_legacy_quotas(parse_quota_buckets(&buckets));
        assert_eq!(quotas[0].id, "geminiPro");
        assert_eq!(quotas[0].used_percent, 50.0);
    }
}
