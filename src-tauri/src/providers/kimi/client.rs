use std::time::Duration;

use super::KimiError;
use crate::providers::http;

const USAGES_URL: &str = "https://api.kimi.ai/coding/v1/usages";

pub use crate::providers::http::EndpointResponse;

pub struct KimiClient {
    client: reqwest::blocking::Client,
    url: String,
}

impl KimiClient {
    pub fn new() -> Result<Self, KimiError> {
        Self::with_endpoint(USAGES_URL, Duration::from_secs(15))
    }

    fn with_endpoint(url: &str, timeout: Duration) -> Result<Self, KimiError> {
        let client = http::build_client(timeout).map_err(|_| KimiError::ConnectionFailed)?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }

    pub fn fetch(&self, api_key: &str) -> Result<EndpointResponse, KimiError> {
        http::get_json(&self.client, &self.url, api_key, "kimi usages").map_err(KimiError::from)
    }
}

#[cfg(test)]
impl KimiClient {
    pub fn for_test(url: &str, timeout: Duration) -> Self {
        Self::with_endpoint(url, timeout).unwrap()
    }
}
