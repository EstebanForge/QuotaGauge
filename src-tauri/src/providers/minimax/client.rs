use std::time::Duration;

use super::MiniMaxError;
use crate::providers::http;

const REMAINS_URL: &str = "https://www.minimax.io/v1/token_plan/remains";

pub use crate::providers::http::EndpointResponse;

pub struct MiniMaxClient {
    client: reqwest::blocking::Client,
    url: String,
}

impl MiniMaxClient {
    pub fn new() -> Result<Self, MiniMaxError> {
        Self::with_endpoint(REMAINS_URL, Duration::from_secs(15))
    }

    fn with_endpoint(url: &str, timeout: Duration) -> Result<Self, MiniMaxError> {
        let client = http::build_client(timeout).map_err(|_| MiniMaxError::ConnectionFailed)?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }

    pub fn fetch(&self, api_key: &str) -> Result<EndpointResponse, MiniMaxError> {
        http::get_json(&self.client, &self.url, api_key, "minimax token_plan")
            .map_err(MiniMaxError::from)
    }
}

#[cfg(test)]
impl MiniMaxClient {
    pub fn for_test(url: &str, timeout: Duration) -> Self {
        Self::with_endpoint(url, timeout).unwrap()
    }
}
