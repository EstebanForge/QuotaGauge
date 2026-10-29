mod auth;
mod client;
mod discovery;
mod mapper;

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};
use thiserror::Error;

use crate::models::{
    MetricDefinition, MetricSection, ProviderDefinition, ProviderSnapshot, QuotaWindow,
};

use self::{
    auth::{load_token, AccessTokenCache},
    client::{AntigravityClient, CloudOutcome, RefreshOutcome},
    discovery::discover,
    mapper::{
        build_legacy_quotas, insert_argon, parse_agy_cli_quota_checked, parse_cloud_models,
        parse_command_model_configs, parse_plan, parse_quota_buckets, parse_quota_summaries,
        parse_quota_summary, parse_user_status,
    },
};

const QUOTA_SUMMARY_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
const RETRIEVE_QUOTA_PATH: &str = "/v1internal:retrieveUserQuota";
/// A warm `agy` answers in a couple of seconds; a cold one needs its own sign-in, which we never
/// drive from a refresh.
const AGY_CLI_TIMEOUT: Duration = Duration::from_secs(15);
const AGY_CLI_PATH_ENV: &str = "ANTIGRAVITY_CLI_PATH";
const AGY_PRINT_SUBCOMMANDS: [&str; 2] = ["/quota", "/usage"];

pub(crate) fn definition() -> ProviderDefinition {
    ProviderDefinition {
        id: "antigravity".into(),
        display_name: "Antigravity".into(),
        short_name: "A".into(),
        fallback_enabled: false,
        links: vec![],
        metrics: vec![
            MetricDefinition::quota(
                "antigravity.geminiPro",
                "Five Hour",
                "geminiPro",
                true,
                true,
                MetricSection::AlwaysVisible,
                true,
                "5H",
            ),
            MetricDefinition::quota(
                "antigravity.geminiWeekly",
                "Weekly",
                "geminiWeekly",
                false,
                true,
                MetricSection::AlwaysVisible,
                true,
                "W",
            ),
            MetricDefinition::quota(
                "antigravity.argonWeekly",
                "Argon Weekly",
                "argonWeekly",
                false,
                true,
                MetricSection::AlwaysVisible,
                false,
                "AW",
            ),
            MetricDefinition::quota(
                "antigravity.claude",
                "Claude",
                "claude",
                true,
                true,
                MetricSection::OnDemand,
                false,
                "C",
            ),
            MetricDefinition::quota(
                "antigravity.claudeWeekly",
                "Claude Weekly",
                "claudeWeekly",
                false,
                true,
                MetricSection::OnDemand,
                false,
                "CW",
            ),
        ],
    }
}

#[derive(Debug, Error)]
pub enum AntigravityError {
    #[error("Start Antigravity or run `agy` and try again.")]
    NotSignedIn,
    #[error("Antigravity sign-in expired. Open Antigravity or run `agy` to refresh.")]
    AuthExpired,
    #[error("Antigravity credentials could not be read from secure storage.")]
    CredentialStoreUnreadable,
    #[error("Antigravity credentials are invalid. Sign in again in Antigravity or `agy`.")]
    InvalidCredentialData,
    #[error("Antigravity usage is temporarily unavailable. Try again shortly.")]
    Unavailable,
}

pub struct AntigravityProvider {
    client: AntigravityClient,
    access_token_cache: AccessTokenCache,
    // A machine without Argon would otherwise spawn agy on every refresh just to
    // re-learn "no Argon"; remember the miss for a few minutes instead.
    argon_miss_cache: std::sync::Mutex<Option<std::time::Instant>>,
}

/// How long a "no Argon from any agy candidate" result stays authoritative.
const ARGON_MISS_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

impl AntigravityProvider {
    pub fn new(cache_path: PathBuf) -> Result<Self, AntigravityError> {
        Ok(Self {
            client: AntigravityClient::new()?,
            access_token_cache: AccessTokenCache::new(cache_path),
            argon_miss_cache: std::sync::Mutex::new(None),
        })
    }

    fn refresh_inner(&self) -> Result<ProviderSnapshot, AntigravityError> {
        if let Some(server) = discover() {
            crate::app_debug!(
                "antigravity",
                "language server discovered (ports={:?})",
                server.ports
            );
            if let Some(summary) = self
                .client
                .call_language_server(&server, "RetrieveUserQuotaSummary")
            {
                if let Some(mut quotas) = parse_quota_summary(&summary) {
                    self.augment_missing_argon(&mut quotas);
                    crate::app_debug!(
                        "antigravity",
                        "language server summary quotas: [{}]",
                        quota_outline(&quotas)
                    );
                    let plan = self
                        .client
                        .call_language_server(&server, "GetUserStatus")
                        .as_ref()
                        .and_then(parse_plan);
                    return Ok(snapshot(plan, quotas));
                }
            }

            if let Some(status) = self.client.call_language_server(&server, "GetUserStatus") {
                if let Some((plan, configs)) = parse_user_status(&status) {
                    let quotas = build_legacy_quotas(configs);
                    if !quotas.is_empty() {
                        return Ok(snapshot(plan, quotas));
                    }
                }
                if let Some(configs) = self
                    .client
                    .call_language_server(&server, "GetCommandModelConfigs")
                    .as_ref()
                    .and_then(parse_command_model_configs)
                {
                    let quotas = build_legacy_quotas(configs);
                    if !quotas.is_empty() {
                        return Ok(snapshot(None, quotas));
                    }
                }
            }
        }

        let keychain = match load_token()? {
            Some(token) => token,
            None => {
                self.access_token_cache.discard();
                return Err(AntigravityError::NotSignedIn);
            }
        };
        let now = Utc::now();
        let cached = self
            .access_token_cache
            .load(keychain.refresh_token.as_deref(), now);
        let (access_tokens, access_is_expired) = access_token_candidates(&keychain, cached, now);

        let mut saw_auth_failure = access_is_expired;
        let mut saw_unavailable = false;
        for access_token in &access_tokens {
            match self.fetch_remote(access_token) {
                Ok(snapshot) => return Ok(snapshot),
                Err(AntigravityError::AuthExpired) => saw_auth_failure = true,
                Err(AntigravityError::Unavailable) => saw_unavailable = true,
                Err(error) => return Err(error),
            }
        }

        let refresh_token = keychain
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty());
        if let Some(refresh_token) = refresh_token.filter(|_| {
            should_refresh_access_token(saw_auth_failure, !access_tokens.is_empty(), true)
        }) {
            match self.client.refresh_google_token(refresh_token) {
                RefreshOutcome::Refreshed {
                    access_token,
                    expires_in_seconds,
                } => {
                    crate::app_info!("auth:antigravity", "token refresh succeeded");
                    self.access_token_cache.store(
                        &access_token,
                        expires_in_seconds,
                        Some(refresh_token),
                        Utc::now(),
                    );
                    return match self.fetch_remote(&access_token) {
                        Err(AntigravityError::AuthExpired) => {
                            self.access_token_cache.discard();
                            Err(AntigravityError::AuthExpired)
                        }
                        result => result,
                    };
                }
                RefreshOutcome::AuthFailed => {
                    self.access_token_cache.discard();
                    return Err(AntigravityError::AuthExpired);
                }
                RefreshOutcome::Unavailable => return Err(AntigravityError::Unavailable),
            }
        }
        Err(credential_failure(
            saw_auth_failure,
            saw_unavailable,
            !access_tokens.is_empty(),
        ))
    }

    fn fetch_remote(&self, token: &str) -> Result<ProviderSnapshot, AntigravityError> {
        let (summaries, summary_auth_failed) =
            self.client
                .cloud_code_all(QUOTA_SUMMARY_PATH, token, json!({}));
        crate::app_debug!(
            "antigravity",
            "cloud quota summaries: {} body(s)",
            summaries.len()
        );
        if !summaries.is_empty() {
            if let Some(mut quotas) = parse_quota_summaries(&summaries) {
                self.augment_missing_session_pool(token, &mut quotas);
                self.augment_missing_argon(&mut quotas);
                crate::app_debug!(
                    "antigravity",
                    "cloud summary quotas: [{}]",
                    quota_outline(&quotas)
                );
                return Ok(snapshot(self.load_remote_plan(token), quotas));
            }
            crate::app_debug!(
                "antigravity",
                "cloud summary bodies did not parse into quotas"
            );
        }
        if summary_auth_failed {
            return Err(AntigravityError::AuthExpired);
        }

        match self.client.cloud_code(FETCH_MODELS_PATH, token, json!({})) {
            CloudOutcome::Ok(value) => {
                let quotas = build_legacy_quotas(parse_cloud_models(&value));
                if !quotas.is_empty() {
                    return Ok(snapshot(self.load_remote_plan(token), quotas));
                }
            }
            CloudOutcome::AuthFailed => return Err(AntigravityError::AuthExpired),
            CloudOutcome::Unavailable => {}
        }

        let (plan, project) = match self
            .client
            .cloud_code(LOAD_CODE_ASSIST_PATH, token, json!({}))
        {
            CloudOutcome::Ok(value) => (
                remote_plan(&value),
                value
                    .get("cloudaicompanionProject")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_owned),
            ),
            CloudOutcome::AuthFailed => return Err(AntigravityError::AuthExpired),
            CloudOutcome::Unavailable => (None, None),
        };

        let body = project
            .as_ref()
            .map(|project| json!({"project": project}))
            .unwrap_or_else(|| json!({}));
        let mut quota = self.client.cloud_code(RETRIEVE_QUOTA_PATH, token, body);
        if matches!(quota, CloudOutcome::Unavailable) && project.is_some() {
            quota = self
                .client
                .cloud_code(RETRIEVE_QUOTA_PATH, token, json!({}));
        }
        match quota {
            CloudOutcome::Ok(value) => {
                let quotas = build_legacy_quotas(parse_quota_buckets(&value));
                if !quotas.is_empty() {
                    return Ok(snapshot(plan, quotas));
                }
            }
            CloudOutcome::AuthFailed => return Err(AntigravityError::AuthExpired),
            CloudOutcome::Unavailable => {}
        }
        Err(AntigravityError::Unavailable)
    }

    fn load_remote_plan(&self, token: &str) -> Option<String> {
        match self
            .client
            .cloud_code(LOAD_CODE_ASSIST_PATH, token, json!({}))
        {
            CloudOutcome::Ok(value) => remote_plan(&value),
            _ => None,
        }
    }

    /// The local language server omits Argon while the IDE is running; the `agy` print report still
    /// carries it. Best effort: a missing CLI or a cold/unresponsive one just keeps the meter absent.
    fn augment_missing_argon(&self, quotas: &mut Vec<crate::models::QuotaWindow>) {
        if quotas.iter().any(|quota| quota.id == "argonWeekly") {
            return;
        }
        if self
            .argon_miss_cache
            .lock()
            .ok()
            .and_then(|miss| *miss)
            .is_some_and(|at| at.elapsed() < ARGON_MISS_TTL)
        {
            crate::app_debug!("antigravity", "Argon miss still cached; skipping agy probe");
            return;
        }
        let cli_path_env =
            std::env::var_os(AGY_CLI_PATH_ENV).map(|path| path.to_string_lossy().into_owned());
        let home = home_directory();
        let home = (!home.as_os_str().is_empty()).then_some(&home);
        let candidates =
            agy_cli_candidates(cli_path_env.as_deref(), home.map(|path| path.as_path()));
        crate::app_debug!(
            "antigravity",
            "Argon missing from the quota snapshot; trying agy ({} {}, candidates={})",
            AGY_CLI_PATH_ENV,
            if cli_path_env.is_some() {
                "set"
            } else {
                "unset"
            },
            candidates.len()
        );
        for program in &candidates {
            if let Some(argon) = argon_from_cli_report(program) {
                let name = program
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| program.to_string_lossy().into_owned());
                crate::app_debug!("antigravity", "Argon supplied by {name}");
                insert_argon(quotas, argon);
                return;
            }
        }
        crate::app_debug!("antigravity", "no agy candidate supplied Argon");
        if let Ok(mut miss) = self.argon_miss_cache.lock() {
            *miss = Some(std::time::Instant::now());
        }
    }

    /// The daily host omits the Gemini session pool; `fetchAvailableModels` still carries the live
    /// rolling quota for it.
    fn augment_missing_session_pool(
        &self,
        token: &str,
        quotas: &mut Vec<crate::models::QuotaWindow>,
    ) {
        if quotas.iter().any(|quota| quota.id == "geminiPro") {
            return;
        }
        if let CloudOutcome::Ok(value) = self.client.cloud_code(FETCH_MODELS_PATH, token, json!({}))
        {
            if let Some(session) = build_legacy_quotas(parse_cloud_models(&value))
                .into_iter()
                .find(|quota| quota.id == "geminiPro")
            {
                quotas.insert(0, session);
            }
        }
    }
}

fn home_directory() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// CodexBar-contract resolution: `ANTIGRAVITY_CLI_PATH` is authoritative, so an empty or
/// unusable value skips the CLI source instead of discovering some other `agy`; otherwise a
/// PATH lookup, then the cask install locations a GUI app's PATH does not carry.
fn agy_cli_candidates(cli_path_env: Option<&str>, home: Option<&Path>) -> Vec<PathBuf> {
    if let Some(path) = cli_path_env.map(str::trim).filter(|path| !path.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    if cli_path_env.is_some() {
        return Vec::new();
    }
    let mut candidates = vec![PathBuf::from("agy")];
    #[cfg(unix)]
    if let Some(home) = home {
        candidates.push(home.join(".local/bin/agy"));
        candidates.push(PathBuf::from("/opt/homebrew/bin/agy"));
        candidates.push(PathBuf::from("/usr/local/bin/agy"));
    }
    candidates
}

/// The slash forms are agy's non-interactive print reports; the bare word is answered as a chat
/// prompt (slow, burns tokens, no groups in the JSON), so it must never be spawned.
fn argon_from_cli_report(program: &Path) -> Option<QuotaWindow> {
    for subcommand in AGY_PRINT_SUBCOMMANDS {
        let mut command = crate::child_process::background_command(program);
        command.env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
        let output = match crate::child_process::output_with_timeout(
            command.args(["-p", subcommand, "--output-format", "json"]),
            AGY_CLI_TIMEOUT,
        ) {
            Ok(output) => output,
            // A hang or a missing binary will not improve on the next subcommand or candidate.
            Err(_) => {
                crate::app_debug!(
                    "antigravity",
                    "agy binary failed to spawn or timed out on -p {subcommand}"
                );
                return None;
            }
        };
        if !output.status.success() {
            crate::app_debug!(
                "antigravity",
                "agy -p {subcommand} exited with {}",
                output.status
            );
            continue;
        }
        let value = match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(value) => value,
            Err(_) => {
                crate::app_debug!(
                    "antigravity",
                    "agy -p {subcommand} printed {} bytes of non-JSON output",
                    output.stdout.len()
                );
                continue;
            }
        };
        match parse_agy_cli_quota_checked(&value) {
            Ok(argon) => return Some(argon),
            Err(reject) => crate::app_debug!(
                "antigravity",
                "agy -p {subcommand} report unusable for Argon: {reject:?}"
            ),
        }
    }
    None
}

fn quota_outline(quotas: &[crate::models::QuotaWindow]) -> String {
    quotas
        .iter()
        .map(|quota| format!("{} {:.0}%", quota.id, quota.used_percent))
        .collect::<Vec<_>>()
        .join(", ")
}

fn access_token_candidates(
    source: &auth::AntigravityToken,
    cached: Option<String>,
    now: chrono::DateTime<Utc>,
) -> (Vec<String>, bool) {
    let access_is_expired = source.access_token.as_ref().is_some_and(|_| {
        source
            .expiry
            .is_some_and(|expiry| expiry <= now + chrono::Duration::seconds(60))
    });
    let mut candidates = Vec::new();
    if !access_is_expired {
        if let Some(access_token) = source
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            candidates.push(access_token.to_owned());
        }
    }
    if let Some(cached) = cached.filter(|token| !token.trim().is_empty()) {
        if !candidates.iter().any(|token| token == &cached) {
            candidates.push(cached);
        }
    }
    (candidates, access_is_expired)
}

fn should_refresh_access_token(
    saw_auth_failure: bool,
    has_access_candidate: bool,
    has_refresh_token: bool,
) -> bool {
    has_refresh_token && (saw_auth_failure || !has_access_candidate)
}

fn credential_failure(
    saw_auth_failure: bool,
    saw_unavailable: bool,
    tried_access_token: bool,
) -> AntigravityError {
    if saw_auth_failure {
        AntigravityError::AuthExpired
    } else if saw_unavailable || tried_access_token {
        AntigravityError::Unavailable
    } else {
        AntigravityError::NotSignedIn
    }
}

fn remote_plan(value: &Value) -> Option<String> {
    // paidTier.id is stabler than the display name Google may localize.
    const PAID_TIER_PLANS: &[(&str, &str)] = &[
        ("g1-pro-tier", "Pro"),
        ("g1-ultra-tier", "Ultra"),
        ("g1-ultra-lite-tier", "Ultra Lite"),
    ];
    if let Some(id) = value.pointer("/paidTier/id").and_then(Value::as_str) {
        if let Some((_, plan)) = PAID_TIER_PLANS.iter().find(|(known, _)| *known == id) {
            return Some((*plan).into());
        }
    }
    let raw = value
        .pointer("/paidTier/name")
        .or_else(|| value.pointer("/currentTier/name"))
        .and_then(Value::as_str)?;
    for tier in ["Ultra", "Pro", "Free"] {
        if raw
            .to_ascii_lowercase()
            .contains(&tier.to_ascii_lowercase())
        {
            return Some(tier.into());
        }
    }
    Some(raw.into())
}

fn snapshot(plan: Option<String>, quotas: Vec<crate::models::QuotaWindow>) -> ProviderSnapshot {
    ProviderSnapshot {
        provider_id: "antigravity".into(),
        plan,
        quotas,
        value_metrics: Vec::new(),
        status_metrics: Vec::new(),
        notices: Vec::new(),
        warnings: Vec::new(),
        refreshed_at: Utc::now(),
    }
}

fn has_refresh_source(
    local_server_available: bool,
    has_credentials: impl FnOnce() -> bool,
) -> bool {
    local_server_available || has_credentials()
}

impl crate::providers::UsageProvider for AntigravityProvider {
    fn definition(&self) -> ProviderDefinition {
        definition()
    }

    fn has_local_credentials(&self) -> bool {
        has_refresh_source(discover().is_some(), auth::has_local_credentials)
    }

    fn refresh(&self) -> Result<ProviderSnapshot, crate::providers::ProviderError> {
        self.refresh_inner().map_err(|error| {
            use crate::models::ProviderErrorKind as Kind;

            let kind = match error {
                AntigravityError::NotSignedIn
                | AntigravityError::AuthExpired
                | AntigravityError::InvalidCredentialData => Kind::Authentication,
                AntigravityError::CredentialStoreUnreadable => Kind::CredentialStorage,
                AntigravityError::Unavailable => Kind::Network,
            };
            crate::providers::ProviderError::from_display(kind, error)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        path::{Path, PathBuf},
    };

    use chrono::{Duration, TimeZone, Utc};

    use super::{
        access_token_candidates, agy_cli_candidates, argon_from_cli_report, auth::AntigravityToken,
        credential_failure, has_refresh_source, remote_plan, should_refresh_access_token,
        AntigravityError,
    };

    #[test]
    fn remote_plan_prefers_the_stable_paid_tier_id() {
        let by_id = remote_plan(&serde_json::json!({
            "paidTier": {"id": "g1-ultra-lite-tier", "name": "Google AI Ultra Lite"}
        }));
        assert_eq!(by_id.as_deref(), Some("Ultra Lite"));

        let unknown_id_falls_back_to_name = remote_plan(&serde_json::json!({
            "paidTier": {"id": "future-tier", "name": "Google AI Pro"}
        }));
        assert_eq!(unknown_id_falls_back_to_name.as_deref(), Some("Pro"));

        let no_paid_tier = remote_plan(&serde_json::json!({
            "currentTier": {"name": "Free"}
        }));
        assert_eq!(no_paid_tier.as_deref(), Some("Free"));
    }

    #[test]
    fn cli_path_env_is_the_authoritative_agy_candidate() {
        assert_eq!(
            agy_cli_candidates(Some("/custom/agy"), Some(Path::new("/home/u"))),
            vec![PathBuf::from("/custom/agy")]
        );
        assert_eq!(
            agy_cli_candidates(Some("  "), Some(Path::new("/home/u"))),
            Vec::<PathBuf>::new()
        );
    }

    #[cfg(unix)]
    #[test]
    fn agy_candidates_fall_back_to_path_then_known_install_locations() {
        assert_eq!(
            agy_cli_candidates(None, Some(Path::new("/home/u"))),
            vec![
                PathBuf::from("agy"),
                PathBuf::from("/home/u/.local/bin/agy"),
                PathBuf::from("/opt/homebrew/bin/agy"),
                PathBuf::from("/usr/local/bin/agy"),
            ]
        );
        assert_eq!(
            agy_cli_candidates(None, None).first(),
            Some(&PathBuf::from("agy"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn argon_print_report_is_parsed_from_a_resolved_cli() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("agy");
        std::fs::write(
            &script,
            concat!(
                "#!/bin/sh\n",
                "if [ \"$2\" != \"/quota\" ]; then exit 1; fi\n",
                "echo '",
                "{\"command\":{\"data\":{\"groups\":[{\"name\":\"Gemini 4 Argon\",\"buckets\":[",
                "{\"bucket_id\":\"argon-weekly\",\"window\":\"weekly\",\"remaining_fraction\":0.19}",
                "]}]}}}'\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let argon = argon_from_cli_report(&script).expect("argon from the print report");
        assert_eq!(argon.id, "argonWeekly");
        assert_eq!(argon.used_percent, 81.0);
    }

    #[cfg(unix)]
    #[test]
    fn usage_backfills_when_quota_is_unknown_and_the_auto_update_guard_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("agy");
        std::fs::write(
            &script,
            concat!(
                "#!/bin/sh\n",
                "if [ \"$2\" != \"/usage\" ]; then exit 1; fi\n",
                "if [ -n \"$AGY_CLI_DISABLE_AUTO_UPDATE\" ]; then fraction=0.81; else fraction=0.99; fi\n",
                "echo '{\"command\":{\"data\":{\"groups\":[{\"name\":\"Gemini 4 Argon\",\"buckets\":[",
                "{\"bucket_id\":\"argon-weekly\",\"window\":\"weekly\",\"remaining_fraction\":'\"$fraction\"'}]}]}}}'\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let argon = argon_from_cli_report(&script).expect("argon via the usage subcommand");
        assert_eq!(argon.used_percent, 19.0);
    }

    #[test]
    fn five_hour_metric_is_named_and_short_labelled_for_the_tray() {
        let session = super::definition()
            .metrics
            .into_iter()
            .find(|metric| metric.id == "antigravity.geminiPro")
            .unwrap();
        assert_eq!(session.label, "Five Hour");
        assert_eq!(
            session.tray.map(|tray| tray.short_label),
            Some("5H".to_string())
        );
    }

    #[test]
    fn keychain_access_token_precedes_a_distinct_cached_token() {
        let now = Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap();
        let source = AntigravityToken {
            access_token: Some("keychain-access".into()),
            refresh_token: Some("refresh".into()),
            expiry: Some(now + Duration::hours(1)),
        };

        let (candidates, expired) =
            access_token_candidates(&source, Some("cached-access".into()), now);
        assert_eq!(candidates, ["keychain-access", "cached-access"]);
        assert!(!expired);

        let (deduplicated, _) =
            access_token_candidates(&source, Some("keychain-access".into()), now);
        assert_eq!(deduplicated, ["keychain-access"]);
    }

    #[test]
    fn expiring_keychain_tokens_are_skipped_but_count_as_auth_evidence() {
        let now = Utc.with_ymd_and_hms(2026, 7, 18, 12, 0, 0).unwrap();
        let source = AntigravityToken {
            access_token: Some("expiring".into()),
            refresh_token: Some("refresh".into()),
            expiry: Some(now + Duration::seconds(30)),
        };

        let (candidates, expired) = access_token_candidates(&source, None, now);
        assert!(candidates.is_empty());
        assert!(expired);
    }

    #[test]
    fn refresh_and_terminal_error_decisions_preserve_auth_network_distinction() {
        assert!(!should_refresh_access_token(false, true, true));
        assert!(should_refresh_access_token(true, true, true));
        assert!(should_refresh_access_token(false, false, true));
        assert!(!should_refresh_access_token(true, true, false));

        assert!(matches!(
            credential_failure(true, false, true),
            AntigravityError::AuthExpired
        ));
        assert!(matches!(
            credential_failure(false, true, true),
            AntigravityError::Unavailable
        ));
        assert!(matches!(
            credential_failure(false, false, false),
            AntigravityError::NotSignedIn
        ));
    }

    #[test]
    fn running_language_server_is_a_refresh_source_without_credentials() {
        assert!(has_refresh_source(false, || true));
        assert!(!has_refresh_source(false, || false));
    }

    #[test]
    fn credentials_skip_language_server_discovery() {
        let discovery_called = Cell::new(false);

        assert!(has_refresh_source(true, || {
            discovery_called.set(true);
            false
        }));
        assert!(!discovery_called.get());
    }
}
