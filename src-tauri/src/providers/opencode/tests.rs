use std::fs;
use std::time::Duration;

use chrono::TimeZone;
use tempfile::tempdir;

use crate::models::{MetricSection, ProviderErrorKind};
use crate::providers::UsageProvider;

use super::{client::OpenCodeClient, definition, paths::OpenCodePaths, OpenCodeError};

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap()
}

fn test_client(url: &str) -> OpenCodeClient {
    OpenCodeClient::for_test(url, Duration::from_secs(1))
}

#[test]
fn missing_go_subscription_is_reported_as_permission() {
    let error: crate::providers::ProviderError = OpenCodeError::GoSubscriptionRequired.into();

    assert_eq!(error.kind(), ProviderErrorKind::Permission);
    assert_eq!(error.to_string(), "OpenCode Go subscription required.");
}

#[test]
fn go_usage_rate_limit_is_reported_as_rate_limited() {
    let error: crate::providers::ProviderError = OpenCodeError::RequestFailed(429).into();

    assert_eq!(error.kind(), ProviderErrorKind::RateLimited);
}

#[test]
fn go_usage_server_failure_is_reported_as_network_error() {
    let error: crate::providers::ProviderError = OpenCodeError::RequestFailed(503).into();

    assert_eq!(error.kind(), ProviderErrorKind::Network);
}

#[test]
fn definition_exposes_the_complete_metric_contract() {
    let definition = definition();
    assert_eq!(definition.id, "opencode");
    assert_eq!(definition.display_name, "OpenCode");
    assert_eq!(
        definition
            .metrics
            .iter()
            .map(|metric| metric.id.as_str())
            .collect::<Vec<_>>(),
        ["opencode.session", "opencode.weekly", "opencode.monthly",]
    );
    assert!(definition
        .metrics
        .iter()
        .all(|metric| metric.default_section == MetricSection::AlwaysVisible));
    assert!(definition
        .metrics
        .iter()
        .all(|metric| !metric.default_pinned));
    assert!(definition.metrics[0].source.session_window());
}

#[test]
fn account_usage_endpoint_populates_quotas() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("auth.json"),
        r#"{"opencode-go":{"type":"api","key":"secret-key"}}"#,
    )
    .unwrap();
    let body = serde_json::json!({
        "usage": {
            "rolling": {"percent": 31, "resetsAt": "2026-08-12T12:00:00Z", "status": "ok"},
            "weekly": {"percent": 100, "resetsAt": "2026-08-17T00:00:00Z", "status": "rate-limited"},
            "monthly": {"percent": 72, "resetsAt": "2026-09-05T00:00:00Z", "status": "ok"}
        }
    })
    .to_string();
    let url = crate::providers::test_http::serve_once(200, &[], &body);
    let provider = super::OpenCodeProvider::with_dependencies(
        OpenCodePaths::for_data_directory(directory.path().to_path_buf()),
        test_client(&url),
        now(),
    );

    let snapshot = provider.refresh().unwrap();

    assert_eq!(snapshot.plan.as_deref(), Some("Go"));
    assert_eq!(snapshot.quotas[0].used_percent, 31.0);
    assert_eq!(snapshot.quotas[1].used_percent, 100.0);
}

#[test]
fn missing_go_login_maps_to_authentication() {
    let directory = tempdir().unwrap();
    let provider = super::OpenCodeProvider::with_dependencies(
        OpenCodePaths::for_data_directory(directory.path().to_path_buf()),
        test_client("http://127.0.0.1:1"),
        now(),
    );

    let error = provider.refresh().unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Authentication);
}

#[test]
fn auth_without_a_go_key_maps_to_authentication() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("auth.json"), r#"{"other":true}"#).unwrap();
    let provider = super::OpenCodeProvider::with_dependencies(
        OpenCodePaths::for_data_directory(directory.path().to_path_buf()),
        test_client("http://127.0.0.1:1"),
        now(),
    );

    let error = provider.refresh().unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::Authentication);
}

#[test]
fn transient_account_quota_failure_is_propagated() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("auth.json"),
        r#"{"opencode-go":{"type":"api","key":"secret-key"}}"#,
    )
    .unwrap();
    let url = crate::providers::test_http::serve_once(429, &[], r#"{"type":"error"}"#);
    let provider = super::OpenCodeProvider::with_dependencies(
        OpenCodePaths::for_data_directory(directory.path().to_path_buf()),
        test_client(&url),
        now(),
    );

    let error = provider.refresh().unwrap_err();

    assert_eq!(error.kind(), ProviderErrorKind::RateLimited);
}
