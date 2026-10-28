mod auth;
mod client;
mod local_credentials;
mod mapper;

use std::sync::Arc;

use chrono::Utc;
use reqwest::StatusCode;
use thiserror::Error;

use crate::models::{
    ApiKeyStatus, MetricDefinition, MetricSection, ProviderDefinition, ProviderErrorKind,
    ProviderLink, ProviderSnapshot,
};

use self::{
    auth::KimiAuthStore,
    client::{EndpointResponse, KimiClient},
    mapper::map_usage,
};

use super::{ProviderError, UsageProvider};

pub(crate) fn definition() -> ProviderDefinition {
    ProviderDefinition {
        id: "kimi".into(),
        display_name: "Kimi".into(),
        short_name: "K".into(),
        fallback_enabled: false,
        links: vec![
            ProviderLink::new("Dashboard", "https://www.kimi.ai/code/console"),
            ProviderLink::new("API Keys", "https://www.kimi.ai/code/console"),
        ],
        metrics: vec![
            MetricDefinition::quota(
                "kimi.session",
                "Session",
                "session",
                false,
                true,
                MetricSection::AlwaysVisible,
                true,
                "S",
            ),
            MetricDefinition::quota(
                "kimi.weekly",
                "Weekly",
                "weekly",
                false,
                true,
                MetricSection::AlwaysVisible,
                true,
                "W",
            ),
            MetricDefinition::quota(
                "kimi.monthly",
                "Monthly",
                "monthly",
                false,
                true,
                MetricSection::AlwaysVisible,
                true,
                "M",
            ),
        ],
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum KimiError {
    #[error(
        "Add a Kimi API key in Customize, sign in with the Kimi CLI (kimi), or connect Kimi in pi."
    )]
    MissingKey,
    #[error("The Kimi sign-in expired. Run the Kimi CLI (kimi) or pi to renew it.")]
    SigninExpired,
    #[error("The Kimi sign-in was rejected. Run the Kimi CLI (kimi) or pi to sign in again.")]
    SigninRejected,
    #[error("The Kimi API key is invalid. Check it in the Kimi Code console.")]
    InvalidKey,
    #[error("Could not reach Kimi. Check your internet connection.")]
    ConnectionFailed,
    #[error("Kimi usage data is temporarily unavailable.")]
    InvalidResponse,
    #[error("Kimi request failed (HTTP {0}).")]
    RequestFailed(u16),
    #[error("The Kimi API key could not be read or updated.")]
    CredentialStorage,
}

impl From<crate::providers::http::HttpError> for KimiError {
    fn from(error: crate::providers::http::HttpError) -> Self {
        match error {
            crate::providers::http::HttpError::Transport => Self::ConnectionFailed,
            crate::providers::http::HttpError::Body => Self::InvalidResponse,
        }
    }
}

impl From<KimiError> for ProviderError {
    fn from(error: KimiError) -> Self {
        let kind = match error {
            KimiError::MissingKey
            | KimiError::InvalidKey
            | KimiError::SigninExpired
            | KimiError::SigninRejected => ProviderErrorKind::Authentication,
            KimiError::ConnectionFailed => ProviderErrorKind::Network,
            KimiError::RequestFailed(429) => ProviderErrorKind::RateLimited,
            KimiError::RequestFailed(401) => ProviderErrorKind::Authentication,
            KimiError::RequestFailed(403) => ProviderErrorKind::Permission,
            KimiError::RequestFailed(_) | KimiError::InvalidResponse => {
                ProviderErrorKind::InvalidResponse
            }
            KimiError::CredentialStorage => ProviderErrorKind::CredentialStorage,
        };
        ProviderError::new(kind, error.to_string())
    }
}

pub struct KimiProvider {
    auth: KimiAuthStore,
    client: Arc<KimiClient>,
    /// Test seam; None resolves the home directory from the environment.
    home: Option<std::path::PathBuf>,
}

impl KimiProvider {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(Self {
            auth: KimiAuthStore::new(),
            client: Arc::new(KimiClient::new().map_err(ProviderError::from)?),
            home: None,
        })
    }

    #[cfg(test)]
    fn with_dependencies(auth: KimiAuthStore, client: KimiClient) -> Self {
        Self {
            auth,
            client: Arc::new(client),
            home: None,
        }
    }

    #[cfg(test)]
    fn with_local_home(mut self, home: std::path::PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    fn local_home(&self) -> Option<std::path::PathBuf> {
        self.home.clone().or_else(local_credentials::env_home)
    }

    fn refresh_snapshot(
        &self,
        api_key: &str,
        source: CredentialSource,
    ) -> Result<ProviderSnapshot, ProviderError> {
        let response = required_response(self.client.fetch(api_key), source)?;
        let mapped = map_usage(&response.body)?;
        Ok(ProviderSnapshot {
            provider_id: "kimi".into(),
            plan: mapped.plan,
            quotas: mapped.quotas,
            value_metrics: Vec::new(),
            status_metrics: Vec::new(),
            notices: Vec::new(),
            warnings: Vec::new(),
            refreshed_at: Utc::now(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    ApiKey,
    LocalSignin,
}

impl UsageProvider for KimiProvider {
    fn definition(&self) -> ProviderDefinition {
        definition()
    }

    fn has_local_credentials(&self) -> bool {
        self.auth.has_local_credentials()
            || self
                .local_home()
                .is_some_and(|home| local_credentials::load_from_home(&home).is_some())
    }

    fn refresh(&self) -> Result<ProviderSnapshot, ProviderError> {
        if let Some(key) = self.auth.load().map_err(ProviderError::from)? {
            return self.refresh_snapshot(key.as_str(), CredentialSource::ApiKey);
        }
        let home = self
            .local_home()
            .ok_or_else(|| ProviderError::from(KimiError::MissingKey))?;
        match local_credentials::load_from_home(&home) {
            Some(local_credentials::LocalKimiCredential::Fresh(access)) => {
                self.refresh_snapshot(&access, CredentialSource::LocalSignin)
            }
            Some(local_credentials::LocalKimiCredential::Expired) => {
                Err(ProviderError::from(KimiError::SigninExpired))
            }
            None => Err(ProviderError::from(KimiError::MissingKey)),
        }
    }

    fn api_key_status(&self) -> Option<Result<ApiKeyStatus, ProviderError>> {
        Some(self.auth.status().map_err(ProviderError::from))
    }

    fn supports_api_key_configuration(&self) -> bool {
        true
    }

    fn save_api_key(&self, value: &str) -> Result<(), ProviderError> {
        self.auth.save(value).map_err(ProviderError::from)
    }

    fn delete_api_key(&self) -> Result<(), ProviderError> {
        self.auth.delete().map_err(ProviderError::from)
    }
}

fn required_response(
    response: Result<EndpointResponse, KimiError>,
    source: CredentialSource,
) -> Result<EndpointResponse, KimiError> {
    let response = response?;
    if matches!(response.status, StatusCode::UNAUTHORIZED) {
        return Err(match source {
            CredentialSource::ApiKey => KimiError::InvalidKey,
            CredentialSource::LocalSignin => KimiError::SigninRejected,
        });
    }
    if !response.status.is_success() {
        return Err(KimiError::RequestFailed(response.status.as_u16()));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use tempfile::tempdir;

    use crate::{
        models::ProviderErrorKind,
        providers::{api_key::*, test_http, UsageProvider},
    };

    use super::{auth::KimiAuthStore, client::KimiClient, definition, KimiProvider};

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);

    impl SecretBackend for MemorySecrets {
        fn read(&self, account: &str) -> Result<Option<SecretBytes>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(account)
                .cloned()
                .map(SecretBytes::new))
        }
        fn write(&self, account: &str, value: &[u8]) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(account.to_owned(), value.to_vec());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(account);
            Ok(())
        }
    }

    struct Environment(HashMap<String, String>);
    impl EnvironmentReader for Environment {
        fn value(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn auth(key: Option<&str>) -> KimiAuthStore {
        KimiAuthStore::with_store(ApiKeyStore::with_backends(
            "kimi",
            "KIMI_API_KEY",
            Arc::new(MemorySecrets::default()),
            Arc::new(Environment(
                key.map(|value| HashMap::from([("KIMI_API_KEY".into(), value.into())]))
                    .unwrap_or_default(),
            )),
        ))
    }

    const QUOTA_BODY: &str = r#"{"user":{"membership":{"level":"LEVEL_BASIC"}},
        "usage":{"limit":"100","used":"25","resetTime":"2026-08-10T02:17:43.139020Z"},
        "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
        "detail":{"limit":"100","remaining":"80","resetTime":"2026-08-07T06:17:43.139020Z"}}]}"#;

    #[test]
    fn refresh_maps_usage_and_window() {
        let url = test_http::serve_once(200, &[], QUOTA_BODY);
        let provider = KimiProvider::with_dependencies(
            auth(Some("secret")),
            KimiClient::for_test(&url, Duration::from_secs(1)),
        );

        let snapshot = provider.refresh().unwrap();
        assert_eq!(snapshot.provider_id, "kimi");
        assert_eq!(snapshot.plan.as_deref(), Some("Basic"));
        assert_eq!(
            snapshot
                .quotas
                .iter()
                .map(|quota| quota.id.as_str())
                .collect::<Vec<_>>(),
            ["session", "weekly"]
        );
    }

    #[test]
    fn local_cli_and_pi_credentials_work_without_an_api_key() {
        let future = (chrono::Utc::now().timestamp_millis() + 3_600_000) / 1_000;
        for (relative_path, payload) in [
            (
                ".kimi-code/credentials/kimi-code.json",
                format!(
                    r#"{{"access_token":"local-access","refresh_token":"r","expires_at":{future}}}"#
                ),
            ),
            (
                ".pi/agent/auth.json",
                format!(
                    r#"{{"kimi-coding":{{"type":"oauth","access":"local-access","refresh":"r","expires":{}}}}}"#,
                    future * 1_000
                ),
            ),
        ] {
            let home = tempdir().unwrap();
            let path = home.path().join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, payload).unwrap();

            let provider = KimiProvider::with_dependencies(
                auth(None),
                KimiClient::for_test(
                    &test_http::serve_once(200, &[], QUOTA_BODY),
                    Duration::from_secs(1),
                ),
            )
            .with_local_home(home.path().to_path_buf());

            let snapshot = provider.refresh().unwrap();
            assert_eq!(snapshot.provider_id, "kimi");
            assert!(provider.has_local_credentials());
        }
    }

    #[test]
    fn expired_local_credentials_still_require_a_key() {
        let stale = chrono::Utc::now().timestamp_millis();
        let home = tempdir().unwrap();
        let path = home.path().join(".kimi-code/credentials/kimi-code.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(
                r#"{{"access_token":"local-access","expires_at":{}}}"#,
                stale / 1_000
            ),
        )
        .unwrap();

        let error = KimiProvider::with_dependencies(
            auth(None),
            KimiClient::for_test(
                &test_http::serve_once(200, &[], QUOTA_BODY),
                Duration::from_secs(1),
            ),
        )
        .with_local_home(home.path().to_path_buf())
        .refresh()
        .unwrap_err();
        assert_eq!(error.kind(), ProviderErrorKind::Authentication);
    }

    #[test]
    fn missing_invalid_and_rate_limited_keys_are_distinct() {
        let missing = KimiProvider::with_dependencies(
            auth(None),
            KimiClient::for_test(
                &test_http::serve_once(200, &[], QUOTA_BODY),
                Duration::from_secs(1),
            ),
        )
        // An empty home keeps the real machine's local Kimi credentials out of the test.
        .with_local_home(tempdir().unwrap().path().to_path_buf())
        .refresh()
        .unwrap_err();
        assert_eq!(missing.kind(), ProviderErrorKind::Authentication);

        let invalid = KimiProvider::with_dependencies(
            auth(Some("bad-key")),
            KimiClient::for_test(
                &test_http::serve_once(401, &[], "{}"),
                Duration::from_secs(1),
            ),
        )
        .refresh()
        .unwrap_err();
        assert_eq!(invalid.kind(), ProviderErrorKind::Authentication);
        assert!(!invalid.to_string().contains("bad-key"));

        let forbidden = KimiProvider::with_dependencies(
            auth(Some("secret")),
            KimiClient::for_test(
                &test_http::serve_once(403, &[], "{}"),
                Duration::from_secs(1),
            ),
        )
        .refresh()
        .unwrap_err();
        assert_eq!(forbidden.kind(), ProviderErrorKind::Permission);

        let rate_limited = KimiProvider::with_dependencies(
            auth(Some("secret")),
            KimiClient::for_test(
                &test_http::serve_once(429, &[], "{}"),
                Duration::from_secs(1),
            ),
        )
        .refresh()
        .unwrap_err();
        assert_eq!(rate_limited.kind(), ProviderErrorKind::RateLimited);
    }

    #[test]
    fn definition_exposes_expected_identity_and_metrics() {
        let definition = definition();
        assert_eq!(definition.id, "kimi");
        assert_eq!(definition.display_name, "Kimi");
        assert_eq!(
            definition
                .links
                .iter()
                .map(|link| link.label.as_str())
                .collect::<Vec<_>>(),
            ["Dashboard", "API Keys"]
        );
    }
}
