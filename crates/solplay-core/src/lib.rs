//! Framework-independent Solplay domain types and matching rules.

use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use thiserror::Error;
use ulid::Ulid;
use url::Url;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid Solana address `{value}`: {reason}")]
    InvalidAddress { value: String, reason: String },
    #[error("invalid URL `{value}`: {reason}")]
    InvalidUrl { value: String, reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct SourceAddress(Pubkey);

impl SourceAddress {
    pub fn as_pubkey(&self) -> Pubkey {
        self.0
    }
}

impl fmt::Display for SourceAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SourceAddress {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Pubkey::from_str(value)
            .map(Self)
            .map_err(|error| CoreError::InvalidAddress {
                value: value.to_owned(),
                reason: error.to_string(),
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "account.changed")]
    AccountChanged,
    #[serde(rename = "transaction.confirmed")]
    TransactionConfirmed,
    #[serde(rename = "transaction.failed")]
    TransactionFailed,
    #[serde(rename = "program.invoked")]
    ProgramInvoked,
    #[serde(rename = "wallet.deposit")]
    WalletDeposit,
    #[serde(rename = "wallet.withdraw")]
    WalletWithdraw,
    #[serde(rename = "token.received")]
    TokenReceived,
    #[serde(rename = "token.sent")]
    TokenSent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mint: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "rust_decimal::serde::str_option"
    )]
    pub amount: Option<Decimal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolplayEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub created_at: DateTime<Utc>,
    pub slot: u64,
    pub signature: String,
    pub source: EventSource,
    pub data: EventData,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventSource {
    pub address: String,
}

impl SolplayEvent {
    pub fn new(
        event_type: EventType,
        slot: u64,
        signature: impl Into<String>,
        source: &SourceAddress,
        data: EventData,
    ) -> Self {
        Self {
            id: format!("evt_{}", Ulid::new()),
            event_type,
            created_at: Utc::now(),
            slot,
            signature: signature.into(),
            source: EventSource {
                address: source.to_string(),
            },
            data,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub mint: Option<String>,
    pub min_amount: Option<Decimal>,
    pub max_amount: Option<Decimal>,
}

impl Filters {
    pub fn matches(&self, event: &SolplayEvent) -> bool {
        if let Some(mint) = &self.mint
            && event.data.mint.as_deref() != Some(mint.as_str())
        {
            return false;
        }

        if let Some(minimum) = self.min_amount
            && event.data.amount.is_none_or(|amount| amount < minimum)
        {
            return false;
        }

        if let Some(maximum) = self.max_amount
            && event.data.amount.is_none_or(|amount| amount > maximum)
        {
            return false;
        }

        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subscription {
    pub name: String,
    pub source: SourceAddress,
    pub events: Vec<EventType>,
    pub filters: Filters,
    pub destination_names: Vec<String>,
}

impl Subscription {
    pub fn matches(&self, event: &SolplayEvent) -> bool {
        event.source.address == self.source.to_string()
            && self.events.contains(&event.event_type)
            && self.filters.matches(event)
    }
}

#[derive(Clone, Debug)]
pub enum DestinationConfig {
    Webhook {
        name: String,
        url: Url,
        secret: Option<String>,
    },
    Discord {
        name: String,
        url: Url,
    },
    Telegram {
        name: String,
        bot_token: String,
        chat_id: String,
    },
}

impl DestinationConfig {
    pub fn name(&self) -> &str {
        match self {
            Self::Webhook { name, .. }
            | Self::Discord { name, .. }
            | Self::Telegram { name, .. } => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceAddress {
        "11111111111111111111111111111111"
            .parse()
            .expect("system program is a valid public key")
    }

    #[test]
    fn filters_reject_amount_outside_range() {
        let event = SolplayEvent::new(
            EventType::TokenReceived,
            1,
            "signature",
            &source(),
            EventData {
                mint: Some("mint".to_owned()),
                amount: Some(Decimal::new(25, 0)),
            },
        );
        let filters = Filters {
            mint: Some("mint".to_owned()),
            min_amount: Some(Decimal::new(10, 0)),
            max_amount: Some(Decimal::new(20, 0)),
        };

        assert!(!filters.matches(&event));
    }

    #[test]
    fn event_serializes_documented_type_name() {
        let event = SolplayEvent::new(
            EventType::TokenReceived,
            1,
            "signature",
            &source(),
            EventData {
                mint: None,
                amount: None,
            },
        );

        assert_eq!(
            serde_json::to_value(event).expect("event serializes")["type"],
            "token.received"
        );
    }
}
