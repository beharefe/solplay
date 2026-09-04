//! The reusable runtime that matches, persists, and delivers Solplay events.

use std::{collections::HashMap, time::Duration};

use solplay_core::{SolplayEvent, Subscription};
use solplay_delivery::{DeliveryStatus, Destination};
use solplay_storage::{Database, DeliveryAttempt, StorageError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Storage(#[from] StorageError),
}

pub struct Engine<'a> {
    database: &'a Database,
    destinations: HashMap<String, Box<dyn Destination>>,
}

impl<'a> Engine<'a> {
    pub fn new(
        database: &'a Database,
        destinations: impl IntoIterator<Item = Box<dyn Destination>>,
    ) -> Self {
        Self {
            database,
            destinations: destinations
                .into_iter()
                .map(|destination| (destination.name().to_owned(), destination))
                .collect(),
        }
    }

    pub async fn process(
        &self,
        event: SolplayEvent,
        subscriptions: &[Subscription],
    ) -> Result<ProcessResult, EngineError> {
        let origin_key = format!(
            "{}:{}:{:?}:{}:{:?}",
            event.signature,
            event.source.address,
            event.event_type,
            event.data.mint.as_deref().unwrap_or_default(),
            event.data.amount
        );
        if !self.database.insert_event(&event, &origin_key)? {
            return Ok(ProcessResult::Duplicate);
        }

        let mut delivered = 0;
        let mut failed = 0;
        for subscription in subscriptions
            .iter()
            .filter(|subscription| subscription.matches(&event))
        {
            for destination_name in &subscription.destination_names {
                let Some(destination) = self.destinations.get(destination_name) else {
                    continue;
                };
                for attempt in 1..=3 {
                    let retry = match destination.deliver(&event).await {
                        Ok(result) => {
                            let delivered_now = result.status == DeliveryStatus::Delivered;
                            let retry = !delivered_now && is_retryable(result.status_code);
                            self.database.record_delivery(
                                &event.id,
                                destination.name(),
                                &DeliveryAttempt {
                                    status: if delivered_now { "delivered" } else { "failed" },
                                    status_code: result.status_code,
                                    duration_ms: result.duration_ms,
                                    response: result.response,
                                },
                            )?;
                            if delivered_now {
                                delivered += 1;
                            } else if !retry {
                                failed += 1;
                            }
                            retry
                        }
                        Err(error) => {
                            self.database.record_delivery(
                                &event.id,
                                destination.name(),
                                &DeliveryAttempt {
                                    status: "failed",
                                    status_code: None,
                                    duration_ms: 0,
                                    response: Some(error.to_string()),
                                },
                            )?;
                            true
                        }
                    };
                    if !retry {
                        break;
                    }
                    if attempt == 3 {
                        failed += 1;
                        break;
                    }
                    tokio::time::sleep(if attempt == 1 {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_secs(5)
                    })
                    .await;
                }
            }
        }
        Ok(ProcessResult::Processed { delivered, failed })
    }
}

fn is_retryable(status_code: Option<u16>) -> bool {
    status_code.is_none_or(|status| status == 408 || status == 429 || (500..600).contains(&status))
}

#[derive(Debug, Eq, PartialEq)]
pub enum ProcessResult {
    Duplicate,
    Processed { delivered: u32, failed: u32 },
}
