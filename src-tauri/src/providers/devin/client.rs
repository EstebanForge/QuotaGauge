use std::time::Duration;

use reqwest::Url;

use super::{auth::DevinAuth, DevinError};
use crate::providers::http;

const CLOUD_SERVICE: &str = "exa.seat_management_pb.SeatManagementService";
pub(super) const CLOUD_COMPAT_VERSION: &str = "1.108.2";

pub(super) use crate::providers::http::EndpointResponse as DevinResponse;

pub(super) struct DevinClient {
    client: reqwest::blocking::Client,
}

impl DevinClient {
    pub fn new() -> Result<Self, DevinError> {
        Self::with_timeout(Duration::from_secs(15))
    }

    fn with_timeout(timeout: Duration) -> Result<Self, DevinError> {
        let client = http::build_client(timeout).map_err(|_| DevinError::ConnectionFailed)?;
        Ok(Self { client })
    }

    pub fn fetch_user_status(&self, auth: &DevinAuth) -> Result<DevinResponse, DevinError> {
        let url = user_status_url(auth.effective_api_server_url())?;
        let started = std::time::Instant::now();
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&serde_json::json!({
                "metadata": {
                    "apiKey": auth.api_key.as_str(),
                    "ideName": "devin",
                    "ideVersion": CLOUD_COMPAT_VERSION,
                    "extensionName": "devin",
                    "extensionVersion": CLOUD_COMPAT_VERSION,
                    "locale": "en"
                }
            }))
            .send()
            .map_err(|_| {
                crate::app_warn!("http", "devin user-status request failed (transport)");
                DevinError::ConnectionFailed
            })?;
        http::parse_response(response, started, "devin user-status").map_err(DevinError::from)
    }
}

fn user_status_url(api_server_url: &str) -> Result<Url, DevinError> {
    let base = api_server_url.trim_end_matches('/');
    Url::parse(&format!("{base}/{CLOUD_SERVICE}/GetUserStatus"))
        .map_err(|_| DevinError::InvalidResponse)
}

#[cfg(test)]
impl DevinClient {
    pub fn for_test(timeout: Duration) -> Self {
        Self::with_timeout(timeout).unwrap()
    }
}
