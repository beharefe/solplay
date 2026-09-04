use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use dotenvy::dotenv;
use rust_decimal::Decimal;
use serde::Deserialize;
use solplay_core::{
    DestinationConfig, EventData, EventType, Filters, SolplayEvent, SourceAddress, Subscription,
};
use solplay_delivery::{Destination, DiscordDestination, TelegramDestination, WebhookDestination};
use solplay_engine::Engine;
use solplay_rpc::{
    RpcEndpoints, TransactionEnricher, WatchTarget, normalize_balances, observe_addresses,
    refresh_wallet_token_accounts,
};
use solplay_storage::Database;
use url::Url;

const SAMPLE_CONFIG: &str = r#"[rpc]
http = "${SOLPLAY_RPC_HTTP}"
ws = "${SOLPLAY_RPC_WS}"

[[destinations]]
name = "backend"
type = "webhook"
url = "${SOLPLAY_WEBHOOK_URL}"
secret = "${SOLPLAY_WEBHOOK_SECRET}"

[[subscriptions]]
name = "treasury-usdc"
address = "11111111111111111111111111111111"
events = ["token.received"]

[subscriptions.filters]
mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
min_amount = "10"

[subscriptions.route]
destinations = ["backend"]
"#;

#[derive(Parser)]
#[command(name = "solplay", version, about = "Self-hosted Solana event engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(long, default_value = "solplay.toml")]
        config: PathBuf,
    },
    Validate {
        #[arg(long, default_value = "solplay.toml")]
        config: PathBuf,
    },
    Logs {
        #[arg(long, default_value = ".solplay/solplay.db")]
        database: PathBuf,
        #[arg(long)]
        event: Option<String>,
        #[arg(long)]
        failed: bool,
    },
    Run {
        #[arg(long, default_value = "solplay.toml")]
        config: PathBuf,
        #[arg(long, default_value = ".solplay/solplay.db")]
        database: PathBuf,
    },
    Monitor {
        #[arg(long, default_value = ".solplay/solplay.db")]
        database: PathBuf,
        #[arg(long)]
        once: bool,
    },
}

#[derive(Deserialize)]
struct FileConfig {
    rpc: RpcFileConfig,
    #[serde(default)]
    destinations: Vec<DestinationFileConfig>,
    #[serde(default)]
    subscriptions: Vec<SubscriptionFileConfig>,
}
#[derive(Deserialize)]
struct RpcFileConfig {
    http: String,
    ws: String,
}
#[derive(Deserialize)]
struct DestinationFileConfig {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    secret: Option<String>,
    bot_token: Option<String>,
    chat_id: Option<String>,
}
#[derive(Deserialize)]
struct SubscriptionFileConfig {
    name: String,
    address: String,
    events: Vec<EventType>,
    #[serde(default)]
    filters: FilterFileConfig,
    route: RouteFileConfig,
}
#[derive(Default, Deserialize)]
struct FilterFileConfig {
    mint: Option<String>,
    min_amount: Option<Decimal>,
    max_amount: Option<Decimal>,
}
#[derive(Deserialize)]
struct RouteFileConfig {
    destinations: Vec<String>,
}
struct LoadedConfig {
    endpoints: RpcEndpoints,
    destinations: Vec<DestinationConfig>,
    subscriptions: Vec<Subscription>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().command.unwrap_or(Command::Validate {
        config: PathBuf::from("solplay.toml"),
    }) {
        Command::Init { config } => initialize(&config),
        Command::Validate { config } => validate(&config),
        Command::Logs {
            database,
            event,
            failed,
        } => logs(&database, event.as_deref(), failed),
        Command::Run { config, database } => run(&config, &database).await,
        Command::Monitor { database, once } => monitor(&database, once).await,
    }
}

async fn monitor(path: &Path, once: bool) -> Result<()> {
    let database =
        Database::open(path).with_context(|| format!("could not open {}", path.display()))?;
    loop {
        let summary = database.monitor_summary()?;
        print!("\x1b[2J\x1b[H");
        println!(
            "SOLPLAY MONITOR\n\nEVENTS      {}\nDELIVERED   {}\nFAILED      {}\n",
            summary.events, summary.delivered, summary.failed
        );
        for event in database.events(None, false)?.into_iter().take(10) {
            println!(
                "{}  {:<24}  {}",
                event.created_at.format("%H:%M:%S"),
                event.event_type,
                if event.has_failed_delivery {
                    "failed"
                } else {
                    ""
                }
            );
        }
        if once {
            return Ok(());
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
        }
    }
}

fn initialize(path: &Path) -> Result<()> {
    if path.exists() {
        bail!(
            "refusing to overwrite existing configuration: {}",
            path.display()
        );
    }
    fs::write(path, SAMPLE_CONFIG)
        .with_context(|| format!("could not write {}", path.display()))?;
    println!("Created {}", path.display());
    Ok(())
}

fn validate(path: &Path) -> Result<()> {
    let config = load_config(path)?;
    println!(
        "Configuration is valid: {} subscription(s), {} destination(s), HTTP RPC {}, WebSocket RPC {}",
        config.subscriptions.len(),
        config.destinations.len(),
        config.endpoints.http,
        config.endpoints.websocket
    );
    Ok(())
}

fn logs(path: &Path, event_type: Option<&str>, failed_only: bool) -> Result<()> {
    let database =
        Database::open(path).with_context(|| format!("could not open {}", path.display()))?;
    for event in database.events(event_type, failed_only)? {
        let failure = if event.has_failed_delivery {
            " failed"
        } else {
            ""
        };
        println!(
            "{} {} {} {}{}",
            event.created_at.to_rfc3339(),
            event.event_type,
            event.event.source.address,
            event.id,
            failure
        );
    }
    Ok(())
}

async fn run(config_path: &Path, database_path: &Path) -> Result<()> {
    let config = load_config(config_path)?;
    if let Some(parent) = database_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let database = Database::open(database_path)
        .with_context(|| format!("could not open {}", database_path.display()))?;
    let mut destinations: Vec<Box<dyn Destination>> = Vec::new();
    for destination in config.destinations {
        match destination {
            DestinationConfig::Webhook { name, url, secret } => {
                destinations.push(Box::new(WebhookDestination::new(name, url, secret)?));
            }
            DestinationConfig::Discord { name, url } => {
                destinations.push(Box::new(DiscordDestination::new(name, url)?))
            }
            DestinationConfig::Telegram {
                name,
                bot_token,
                chat_id,
            } => destinations.push(Box::new(TelegramDestination::new(
                name, bot_token, chat_id,
            )?)),
        }
    }
    let engine = Engine::new(&database, destinations);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1_024);
    let enricher = TransactionEnricher::new(&config.endpoints);
    let sources = config
        .subscriptions
        .iter()
        .map(|subscription| subscription.source.clone())
        .collect::<Vec<_>>();
    let mut targets = sources
        .iter()
        .map(|source| WatchTarget {
            source: source.clone(),
            address: source.to_string(),
        })
        .collect::<Vec<_>>();
    for source in &sources {
        match enricher.token_account_targets(source).await {
            Ok(token_targets) => targets.extend(token_targets),
            Err(error) => {
                tracing::warn!(source = %source, error = %error, "could not discover token accounts; watching wallet address only")
            }
        }
    }
    targets.sort_by(|left, right| {
        left.address
            .cmp(&right.address)
            .then_with(|| left.source.to_string().cmp(&right.source.to_string()))
    });
    targets.dedup_by(|left, right| left.address == right.address && left.source == right.source);
    let known = targets
        .iter()
        .map(|target| (target.source.to_string(), target.address.clone()))
        .collect();
    observe_addresses(config.endpoints.clone(), targets, sender.clone()).await;
    refresh_wallet_token_accounts(config.endpoints, sources, sender, known);
    println!(
        "Solplay listening to {} subscription(s). Press Ctrl-C to stop.",
        config.subscriptions.len()
    );
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            Some(observed) = receiver.recv() => {
                let event_type = if observed.failed { EventType::TransactionFailed } else { EventType::TransactionConfirmed };
                let mut events = vec![SolplayEvent::new(event_type, observed.slot, observed.signature.clone(), &observed.source, EventData { mint: None, amount: None })];
                match enricher.fetch(&observed.signature).await {
                    Ok(Some(transaction)) => {
                        events.extend(normalize_balances(&transaction, &observed.source, &observed.signature, observed.slot));
                        tracing::debug!(signature = %observed.signature, "enriched Solana transaction");
                    }
                    Ok(None) => tracing::warn!(signature = %observed.signature, "transaction was not available at confirmed commitment"),
                    Err(error) => tracing::warn!(signature = %observed.signature, error = %error, "could not enrich Solana transaction"),
                }
                for event in events {
                    match engine.process(event, &config.subscriptions).await? {
                        solplay_engine::ProcessResult::Duplicate => tracing::debug!("duplicate event ignored"),
                        solplay_engine::ProcessResult::Processed { delivered, failed } => tracing::info!(delivered, failed, "processed Solana transaction"),
                    }
                }
            }
        }
    }
    Ok(())
}

fn load_config(path: &Path) -> Result<LoadedConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    let raw = interpolate_environment(&raw)?;
    let file: FileConfig = toml::from_str(&raw).context("could not parse TOML configuration")?;
    let endpoints = RpcEndpoints::new(
        Url::parse(&file.rpc.http).context("invalid rpc.http URL")?,
        Url::parse(&file.rpc.ws).context("invalid rpc.ws URL")?,
    )?;
    let mut names = HashSet::new();
    let destinations = file
        .destinations
        .into_iter()
        .map(|destination| {
            if !names.insert(destination.name.clone()) {
                bail!("duplicate destination name `{}`", destination.name);
            }
            match destination.kind.as_str() {
                "webhook" => Ok(DestinationConfig::Webhook {
                    name: destination.name,
                    url: parse_required_url(destination.url, "webhook")?,
                    secret: destination.secret,
                }),
                "discord" => Ok(DestinationConfig::Discord {
                    name: destination.name,
                    url: parse_required_url(destination.url, "discord")?,
                }),
                "telegram" => Ok(DestinationConfig::Telegram {
                    name: destination.name,
                    bot_token: destination
                        .bot_token
                        .context("telegram destination requires bot_token")?,
                    chat_id: destination
                        .chat_id
                        .context("telegram destination requires chat_id")?,
                }),
                other => bail!("unsupported destination type `{other}`"),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let destination_names = destinations
        .iter()
        .map(|destination| destination.name())
        .collect::<HashSet<_>>();
    let mut subscription_names = HashSet::new();
    let subscriptions = file
        .subscriptions
        .into_iter()
        .map(|subscription| {
            if !subscription_names.insert(subscription.name.clone()) {
                bail!("duplicate subscription name `{}`", subscription.name);
            }
            if subscription.events.is_empty() {
                bail!("subscription `{}` has no events", subscription.name);
            }
            for destination in &subscription.route.destinations {
                if !destination_names.contains(destination.as_str()) {
                    bail!(
                        "subscription `{}` references unknown destination `{destination}`",
                        subscription.name
                    );
                }
            }
            Ok(Subscription {
                name: subscription.name,
                source: SourceAddress::from_str(&subscription.address)?,
                events: subscription.events,
                filters: Filters {
                    mint: subscription.filters.mint,
                    min_amount: subscription.filters.min_amount,
                    max_amount: subscription.filters.max_amount,
                },
                destination_names: subscription.route.destinations,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(LoadedConfig {
        endpoints,
        destinations,
        subscriptions,
    })
}

fn parse_required_url(value: Option<String>, kind: &str) -> Result<Url> {
    Url::parse(&value.with_context(|| format!("{kind} destination requires url"))?)
        .context("invalid destination URL")
}

fn interpolate_environment(value: &str) -> Result<String> {
    let mut result = String::new();
    let mut remainder = value;
    while let Some(start) = remainder.find("${") {
        result.push_str(&remainder[..start]);
        let after_start = &remainder[start + 2..];
        let Some(end) = after_start.find('}') else {
            bail!("unclosed environment variable placeholder");
        };
        let name = &after_start[..end];
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            bail!("invalid environment variable placeholder `${{{name}}}`");
        }
        result.push_str(
            &std::env::var(name)
                .with_context(|| format!("missing required environment variable `{name}`"))?,
        );
        remainder = &after_start[end + 1..];
    }
    result.push_str(remainder);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_rejects_unclosed_placeholder() {
        assert!(interpolate_environment("${MISSING").is_err());
    }

    #[test]
    fn sample_config_has_expected_sections() {
        assert!(SAMPLE_CONFIG.contains("[[subscriptions]]"));
    }
}
