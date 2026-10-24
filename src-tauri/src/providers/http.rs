use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde_json::Value;

const USER_AGENT: &str = concat!("QuotaGauge/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug)]
pub struct EndpointResponse {
    pub status: StatusCode,
    pub body: Value,
}

#[derive(Debug)]
pub enum HttpError {
    Transport,
    Body,
}

pub fn build_client(request_timeout: Duration) -> reqwest::Result<Client> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .user_agent(USER_AGENT)
        .build()
}

pub fn get_json(
    client: &Client,
    url: &str,
    api_key: &str,
    label: &str,
) -> Result<EndpointResponse, HttpError> {
    let started = std::time::Instant::now();
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .header("Accept", "application/json")
        .send()
        .map_err(|_| {
            crate::app_warn!("http", "{label} request failed (transport)");
            HttpError::Transport
        })?;
    parse_response(response, started, label)
}

pub fn parse_response(
    response: reqwest::blocking::Response,
    started: std::time::Instant,
    label: &str,
) -> Result<EndpointResponse, HttpError> {
    let status = response.status();
    crate::app_debug!(
        "http",
        "{label} HTTP {} ({}ms)",
        status.as_u16(),
        started.elapsed().as_millis()
    );
    let text = response.text().map_err(|_| HttpError::Body)?;
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(EndpointResponse { status, body })
}
