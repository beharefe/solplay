//! Destination adapters for normalized Solplay events.

use std::time::Instant;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::StatusCode;
use sha2::Sha256;
use solplay_core::SolplayEvent;
use thiserror::Error;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum DeliveryError {
    #[error("event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("HTTP delivery failed")]
    Transport,
}

#[derive(Debug)]
pub struct DeliveryResult {
    pub status: DeliveryStatus,
    pub status_code: Option<u16>,
    pub duration_ms: u64,
    pub response: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum DeliveryStatus {
    Delivered,
    Failed,
}

#[async_trait]
pub trait Destination: Send + Sync {
    fn name(&self) -> &str;
    async fn deliver(&self, event: &SolplayEvent) -> Result<DeliveryResult, DeliveryError>;
}

pub struct WebhookDestination {
    name: String,
    url: Url,
    secret: Option<String>,
    client: reqwest::Client,
}

impl WebhookDestination {
    pub fn new(name: String, url: Url, secret: Option<String>) -> Result<Self, DeliveryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| DeliveryError::Transport)?;
        Ok(Self {
            name,
            url,
            secret,
            client,
        })
    }
}

#[async_trait]
impl Destination for WebhookDestination {
    fn name(&self) -> &str {
        &self.name
    }

    async fn deliver(&self, event: &SolplayEvent) -> Result<DeliveryResult, DeliveryError> {
        let body = serde_json::to_vec(event)?;
        let started_at = Instant::now();
        let mut request = self
            .client
            .post(self.url.clone())
            .header("content-type", "application/json")
            .body(body.clone());
        if let Some(secret) = &self.secret {
            let timestamp = chrono::Utc::now().timestamp();
            let mut signer = HmacSha256::new_from_slice(secret.as_bytes())
                .expect("HMAC accepts arbitrary key lengths");
            signer.update(format!("{timestamp}.").as_bytes());
            signer.update(&body);
            request = request.header(
                "Solplay-Signature",
                format!(
                    "t={timestamp},v1={}",
                    hex::encode(signer.finalize().into_bytes())
                ),
            );
        }
        let response = request.send().await.map_err(|_| DeliveryError::Transport)?;
        let status_code = response.status();
        let response_body = response
            .text()
            .await
            .ok()
            .filter(|body| !body.is_empty())
            .map(|body| body.chars().take(1_024).collect());
        Ok(DeliveryResult {
            status: if status_code.is_success() {
                DeliveryStatus::Delivered
            } else {
                DeliveryStatus::Failed
            },
            status_code: Some(status_code.as_u16()),
            duration_ms: started_at
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            response: response_body,
        })
    }
}

pub struct DiscordDestination {
    name: String,
    url: Url,
    client: reqwest::Client,
}
impl DiscordDestination {
    pub fn new(name: String, url: Url) -> Result<Self, DeliveryError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| DeliveryError::Transport)?;
        Ok(Self { name, url, client })
    }
}
#[async_trait]
impl Destination for DiscordDestination {
    fn name(&self) -> &str {
        &self.name
    }
    async fn deliver(&self, event: &SolplayEvent) -> Result<DeliveryResult, DeliveryError> {
        let payload = serde_json::json!({ "content": format!("**{}**\nSource: `{}`\nTransaction: `{}`", serde_json::to_string(&event.event_type)?.trim_matches('\"'), event.source.address, event.signature) });
        deliver_json(&self.client, self.url.clone(), payload).await
    }
}

pub struct TelegramDestination {
    name: String,
    url: Url,
    chat_id: String,
    client: reqwest::Client,
}
impl TelegramDestination {
    pub fn new(name: String, token: String, chat_id: String) -> Result<Self, DeliveryError> {
        let url = Url::parse(&format!("https://api.telegram.org/bot{token}/sendMessage"))
            .map_err(|_| DeliveryError::Transport)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| DeliveryError::Transport)?;
        Ok(Self {
            name,
            url,
            chat_id,
            client,
        })
    }
}
#[async_trait]
impl Destination for TelegramDestination {
    fn name(&self) -> &str {
        &self.name
    }
    async fn deliver(&self, event: &SolplayEvent) -> Result<DeliveryResult, DeliveryError> {
        let payload = serde_json::json!({ "chat_id": self.chat_id, "text": format!("{}\nSource: {}\nTransaction: {}", serde_json::to_string(&event.event_type)?.trim_matches('\"'), event.source.address, event.signature) });
        deliver_json(&self.client, self.url.clone(), payload).await
    }
}

async fn deliver_json(
    client: &reqwest::Client,
    url: Url,
    payload: serde_json::Value,
) -> Result<DeliveryResult, DeliveryError> {
    let started_at = Instant::now();
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|_| DeliveryError::Transport)?;
    let status_code = response.status();
    let response = response
        .text()
        .await
        .ok()
        .filter(|body| !body.is_empty())
        .map(|body| body.chars().take(1_024).collect());
    Ok(DeliveryResult {
        status: if status_code.is_success() {
            DeliveryStatus::Delivered
        } else {
            DeliveryStatus::Failed
        },
        status_code: Some(status_code.as_u16()),
        duration_ms: started_at
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        response,
    })
}

pub fn should_retry(status: Option<StatusCode>) -> bool {
    status.is_none_or(|status| {
        status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
    })
}
