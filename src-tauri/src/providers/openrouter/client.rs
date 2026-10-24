use std::time::Duration;

use super::OpenRouterError;
use crate::providers::http;

const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
const KEY_URL: &str = "https://openrouter.ai/api/v1/key";

pub use crate::providers::http::EndpointResponse;

pub struct OpenRouterClient {
    client: reqwest::blocking::Client,
    credits_url: String,
    key_url: String,
}

impl OpenRouterClient {
    pub fn new() -> Result<Self, OpenRouterError> {
        Self::with_endpoints(CREDITS_URL, KEY_URL, Duration::from_secs(15))
    }

    fn with_endpoints(
        credits_url: &str,
        key_url: &str,
        timeout: Duration,
    ) -> Result<Self, OpenRouterError> {
        let client = http::build_client(timeout).map_err(|_| OpenRouterError::ConnectionFailed)?;
        Ok(Self {
            client,
            credits_url: credits_url.to_owned(),
            key_url: key_url.to_owned(),
        })
    }

    pub fn fetch_credits(&self, api_key: &str) -> Result<EndpointResponse, OpenRouterError> {
        http::get_json(
            &self.client,
            &self.credits_url,
            api_key,
            "openrouter credits",
        )
        .map_err(OpenRouterError::from)
    }

    pub fn fetch_key(&self, api_key: &str) -> Result<EndpointResponse, OpenRouterError> {
        http::get_json(&self.client, &self.key_url, api_key, "openrouter key")
            .map_err(OpenRouterError::from)
    }
}

#[cfg(test)]
impl OpenRouterClient {
    pub fn for_test(credits_url: &str, key_url: &str, timeout: Duration) -> Self {
        Self::with_endpoints(credits_url, key_url, timeout).unwrap()
    }
}
