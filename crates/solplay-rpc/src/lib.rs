//! Provider-neutral Solana RPC endpoint validation and address observation.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::json;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::{client_error::Error as ClientError, request::RpcRequest};
use solplay_core::{EventData, EventType, SolplayEvent, SourceAddress};
use thiserror::Error;
use tokio::{sync::mpsc, time::sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

#[derive(Clone, Debug)]
pub struct RpcEndpoints {
    pub http: Url,
    pub websocket: Url,
}

#[derive(Debug, Error)]
pub enum RpcEndpointError {
    #[error("HTTP RPC endpoint must use http or https, got `{0}`")]
    InvalidHttpScheme(Url),
    #[error("WebSocket RPC endpoint must use ws or wss, got `{0}`")]
    InvalidWebsocketScheme(Url),
}

impl RpcEndpoints {
    pub fn new(http: Url, websocket: Url) -> Result<Self, RpcEndpointError> {
        if !matches!(http.scheme(), "http" | "https") {
            return Err(RpcEndpointError::InvalidHttpScheme(http));
        }
        if !matches!(websocket.scheme(), "ws" | "wss") {
            return Err(RpcEndpointError::InvalidWebsocketScheme(websocket));
        }
        Ok(Self { http, websocket })
    }
}

#[derive(Clone, Debug)]
pub struct ObservedTransaction {
    pub source: SourceAddress,
    pub signature: String,
    pub slot: u64,
    pub failed: bool,
}

#[derive(Clone, Debug)]
pub struct WatchTarget {
    pub source: SourceAddress,
    pub address: String,
}

/// HTTP transaction enrichment kept behind the RPC boundary.
pub struct TransactionEnricher {
    client: RpcClient,
}

impl TransactionEnricher {
    pub fn new(endpoints: &RpcEndpoints) -> Self {
        Self {
            client: RpcClient::new(endpoints.http.to_string()),
        }
    }

    pub async fn fetch(&self, signature: &str) -> Result<Option<serde_json::Value>, ClientError> {
        let mut last_error = None;
        for delay in [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(5),
        ] {
            if !delay.is_zero() {
                sleep(delay).await;
            }
            match self.client.send(RpcRequest::GetTransaction, json!([signature, { "encoding": "jsonParsed", "commitment": "confirmed", "maxSupportedTransactionVersion": 0 }])).await {
                Ok(Some(transaction)) => return Ok(Some(transaction)),
                Ok(None) => {},
                Err(error) => last_error = Some(error),
            }
        }
        match last_error {
            Some(error) => Err(error),
            None => Ok(None),
        }
    }

    pub async fn token_account_targets(
        &self,
        source: &SourceAddress,
    ) -> Result<Vec<WatchTarget>, ClientError> {
        let mut targets = Vec::new();
        for program_id in [
            spl_token_interface::id().to_string(),
            spl_token_2022_interface::id().to_string(),
        ] {
            let response: serde_json::Value = self.client.send(
                RpcRequest::GetTokenAccountsByOwner,
                json!([source.to_string(), { "programId": program_id }, { "encoding": "jsonParsed" }]),
            ).await?;
            if let Some(accounts) = response
                .pointer("/value")
                .and_then(serde_json::Value::as_array)
            {
                targets.extend(
                    accounts
                        .iter()
                        .filter_map(|account| {
                            account.get("pubkey").and_then(serde_json::Value::as_str)
                        })
                        .map(|address| WatchTarget {
                            source: source.clone(),
                            address: address.to_owned(),
                        }),
                );
            }
        }
        Ok(targets)
    }
}

pub fn refresh_wallet_token_accounts(
    endpoints: RpcEndpoints,
    sources: Vec<SourceAddress>,
    sender: mpsc::Sender<ObservedTransaction>,
    mut known: HashSet<(String, String)>,
) {
    tokio::spawn(async move {
        loop {
            sleep(Duration::from_secs(300)).await;
            let enricher = TransactionEnricher::new(&endpoints);
            let mut new_targets = Vec::new();
            for source in &sources {
                if let Ok(targets) = enricher.token_account_targets(source).await {
                    for target in targets {
                        if known.insert((target.source.to_string(), target.address.clone())) {
                            new_targets.push(target);
                        }
                    }
                }
            }
            if !new_targets.is_empty() {
                observe_addresses(endpoints.clone(), new_targets, sender.clone()).await;
            }
        }
    });
}

pub fn normalize_balances(
    transaction: &serde_json::Value,
    source: &SourceAddress,
    signature: &str,
    slot: u64,
) -> Vec<SolplayEvent> {
    let source_address = source.to_string();
    let account_keys = transaction
        .pointer("/transaction/message/accountKeys")
        .and_then(serde_json::Value::as_array)
        .map(|keys| keys.iter().filter_map(account_key).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut events = Vec::new();
    if transaction
        .pointer("/meta/logMessages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|logs| {
            logs.iter()
                .filter_map(serde_json::Value::as_str)
                .any(|log| log.starts_with(&format!("Program {source_address} invoke")))
        })
    {
        events.push(SolplayEvent::new(
            EventType::ProgramInvoked,
            slot,
            signature,
            source,
            EventData {
                mint: None,
                amount: None,
            },
        ));
    }
    if let Some(index) = account_keys
        .iter()
        .position(|address| address == &source_address)
    {
        if transaction
            .pointer(&format!(
                "/transaction/message/accountKeys/{index}/writable"
            ))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            events.push(SolplayEvent::new(
                EventType::AccountChanged,
                slot,
                signature,
                source,
                EventData {
                    mint: None,
                    amount: None,
                },
            ));
        }
        let pre = transaction
            .pointer(&format!("/meta/preBalances/{index}"))
            .and_then(serde_json::Value::as_u64);
        let post = transaction
            .pointer(&format!("/meta/postBalances/{index}"))
            .and_then(serde_json::Value::as_u64);
        if let (Some(pre), Some(post)) = (pre, post) {
            let delta = i128::from(post) - i128::from(pre);
            if delta != 0 {
                let kind = if delta > 0 {
                    EventType::WalletDeposit
                } else {
                    EventType::WalletWithdraw
                };
                events.push(SolplayEvent::new(
                    kind,
                    slot,
                    signature,
                    source,
                    EventData {
                        mint: None,
                        amount: Some(Decimal::from_i128_with_scale(
                            delta.unsigned_abs() as i128,
                            9,
                        )),
                    },
                ));
            }
        }
    }
    let pre = token_balances(
        transaction.pointer("/meta/preTokenBalances"),
        &source_address,
        &account_keys,
    );
    let post = token_balances(
        transaction.pointer("/meta/postTokenBalances"),
        &source_address,
        &account_keys,
    );
    let mut keys: HashSet<_> = pre.keys().chain(post.keys()).cloned().collect();
    for key in keys.drain() {
        let before = pre.get(&key).copied().unwrap_or_default();
        let after = post.get(&key).copied().unwrap_or_default();
        let delta = i128::from(after.raw) - i128::from(before.raw);
        if delta == 0 {
            continue;
        }
        let kind = if delta > 0 {
            EventType::TokenReceived
        } else {
            EventType::TokenSent
        };
        events.push(SolplayEvent::new(
            kind,
            slot,
            signature,
            source,
            EventData {
                mint: Some(key.mint),
                amount: Some(Decimal::from_i128_with_scale(
                    delta.unsigned_abs() as i128,
                    u32::from(after.decimals.max(before.decimals)),
                )),
            },
        ));
    }
    events
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TokenKey {
    index: usize,
    mint: String,
}
#[derive(Clone, Copy, Default)]
struct TokenBalance {
    raw: u64,
    decimals: u8,
}
fn account_key(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_owned).or_else(|| {
        value
            .get("pubkey")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    })
}
fn token_balances(
    value: Option<&serde_json::Value>,
    source: &str,
    accounts: &[String],
) -> HashMap<TokenKey, TokenBalance> {
    value
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|balance| {
            let index: usize = balance.get("accountIndex")?.as_u64()?.try_into().ok()?;
            let owner_matches =
                balance.get("owner").and_then(serde_json::Value::as_str) == Some(source);
            if !owner_matches && accounts.get(index).is_none_or(|address| address != source) {
                return None;
            }
            let mint = balance.get("mint")?.as_str()?.to_owned();
            let raw = balance
                .pointer("/uiTokenAmount/amount")?
                .as_str()?
                .parse()
                .ok()?;
            let decimals = balance
                .pointer("/uiTokenAmount/decimals")?
                .as_u64()?
                .try_into()
                .ok()?;
            Some((TokenKey { index, mint }, TokenBalance { raw, decimals }))
        })
        .collect()
}

pub async fn observe_addresses(
    endpoints: RpcEndpoints,
    targets: Vec<WatchTarget>,
    sender: mpsc::Sender<ObservedTransaction>,
) {
    for target in targets {
        let endpoints = endpoints.clone();
        let sender = sender.clone();
        tokio::spawn(async move {
            loop {
                if let Err(error) = observe_address(&endpoints, target.clone(), &sender).await {
                    tracing::warn!(source = %target.source, address = %target.address, error = %error, "Solana WebSocket observer disconnected; reconnecting");
                }
                sleep(Duration::from_secs(1)).await;
            }
        });
    }
}

async fn observe_address(
    endpoints: &RpcEndpoints,
    target: WatchTarget,
    sender: &mpsc::Sender<ObservedTransaction>,
) -> Result<(), ObserverError> {
    let (stream, _) = connect_async(endpoints.websocket.as_str()).await?;
    let (mut writer, mut reader) = stream.split();
    writer.send(Message::Text(json!({ "jsonrpc": "2.0", "id": 1, "method": "logsSubscribe", "params": [{ "mentions": [target.address] }, { "commitment": "confirmed" }] }).to_string().into())).await?;
    while let Some(message) = reader.next().await {
        let Message::Text(text) = message? else {
            continue;
        };
        let value: serde_json::Value = serde_json::from_str(&text)?;
        let Some(result) = value.pointer("/params/result") else {
            continue;
        };
        let Some(signature) = result
            .pointer("/value/signature")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Some(slot) = result
            .pointer("/context/slot")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let failed = !result
            .pointer("/value/err")
            .is_none_or(serde_json::Value::is_null);
        if sender
            .send(ObservedTransaction {
                source: target.source.clone(),
                signature: signature.to_owned(),
                slot,
                failed,
            })
            .await
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
enum ObserverError {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("invalid WebSocket notification: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod validator_tests {
    use std::{
        net::TcpStream,
        process::{Child, Command},
        thread::sleep as blocking_sleep,
        time::{Duration, Instant},
    };

    use tempfile::TempDir;

    use super::RpcClient;

    fn start_validator() -> Option<(Child, TempDir)> {
        if Command::new("solana-test-validator")
            .arg("--version")
            .output()
            .is_err()
        {
            return None;
        }
        let ledger = TempDir::new().expect("temporary validator ledger");
        let child = Command::new("solana-test-validator")
            .args([
                "--ledger",
                ledger.path().to_str().expect("UTF-8 ledger path"),
                "--reset",
                "--rpc-port",
                "18999",
                "--quiet",
            ])
            .spawn()
            .expect("validator starts");
        Some((child, ledger))
    }

    #[test]
    #[ignore = "starts solana-test-validator; run with cargo test -p solplay-rpc -- --ignored"]
    fn local_validator_accepts_json_rpc_connections() {
        let Some((mut validator, _ledger)) = start_validator() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect("127.0.0.1:18999").is_err() {
            assert!(
                Instant::now() < deadline,
                "local validator did not accept RPC connections in time"
            );
            blocking_sleep(Duration::from_millis(250));
        }
        let runtime = tokio::runtime::Runtime::new().expect("Tokio runtime");
        let _slot = runtime.block_on(async {
            RpcClient::new("http://127.0.0.1:18999".to_owned())
                .get_slot()
                .await
                .expect("validator answers getSlot")
        });
        validator.kill().expect("validator stops");
        validator.wait().expect("validator process is reaped");
    }
}
