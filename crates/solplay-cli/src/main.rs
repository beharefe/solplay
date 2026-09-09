use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use dialoguer::{Input, MultiSelect, Password, Select, console::Style, theme::ColorfulTheme};
use dotenvy::{dotenv, from_path};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
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
name = "my-devnet-usdc-wallet"
address = "${SOLPLAY_WATCH_ADDRESS}"
events = ["token.received"]

[subscriptions.filters]
mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"
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
    Set {
        #[command(subcommand)]
        setting: SetCommand,
    },
    Wallet {
        #[arg(long, global = true, default_value = "solplay.toml")]
        config: PathBuf,
        #[command(subcommand)]
        command: Option<WalletCommand>,
    },
    /// Configure destinations and subscriptions through prompts.
    Setup {
        #[arg(long, default_value = "solplay.toml")]
        config: PathBuf,
    },
}

#[derive(Subcommand)]
enum SetCommand {
    /// Save a wallet address as a subscription.
    Wallet {
        address: String,
        alias: String,
        #[arg(long, default_value = "solplay.toml")]
        config: PathBuf,
        /// Destination to receive events. Required when more than one exists.
        #[arg(long)]
        destination: Option<String>,
    },
}

#[derive(Subcommand)]
enum WalletCommand {
    /// Print saved wallet aliases and addresses without prompting.
    List,
}

#[derive(Deserialize, Serialize)]
struct FileConfig {
    rpc: RpcFileConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    destinations: Vec<DestinationFileConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subscriptions: Vec<SubscriptionFileConfig>,
}
#[derive(Deserialize, Serialize)]
struct RpcFileConfig {
    http: String,
    ws: String,
}
#[derive(Deserialize, Serialize)]
struct DestinationFileConfig {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bot_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct SubscriptionFileConfig {
    name: String,
    address: String,
    events: Vec<EventType>,
    #[serde(default, skip_serializing_if = "FilterFileConfig::is_empty")]
    filters: FilterFileConfig,
    route: RouteFileConfig,
}
#[derive(Default, Deserialize, Serialize)]
struct FilterFileConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    mint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_amount: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_amount: Option<Decimal>,
}
impl FilterFileConfig {
    fn is_empty(&self) -> bool {
        self.mint.is_none() && self.min_amount.is_none() && self.max_amount.is_none()
    }
}
#[derive(Deserialize, Serialize)]
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
    load_environment();
    let log_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(log_filter).init();
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
        Command::Set { setting } => match setting {
            SetCommand::Wallet {
                address,
                alias,
                config,
                destination,
            } => set_wallet(&config, &address, &alias, destination.as_deref()),
        },
        Command::Wallet { config, command } => match command {
            Some(WalletCommand::List) => list_wallets(&config),
            None => configure_wallet(&config),
        },
        Command::Setup { config } => setup(&config),
    }
}

fn load_environment() {
    // Load the conventional .env from the working directory (or one of its
    // parents) first. Values already exported by the caller keep precedence.
    dotenv().ok();

    // Debuggers can use the workspace as their working directory while placing
    // their .env beside target/debug/solplay. Load that file as a fallback.
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        from_path(directory.join(".env")).ok();
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

fn set_wallet(path: &Path, address: &str, alias: &str, destination: Option<&str>) -> Result<()> {
    let config = read_editable_config(path)?;
    let destination = match destination {
        Some(destination) => destination.to_owned(),
        None if config.destinations.len() == 1 => config.destinations[0].name.clone(),
        None if config.destinations.is_empty() => {
            bail!("no destinations are configured; add one before adding a wallet")
        }
        None => bail!("multiple destinations exist; choose one with `--destination <name>`"),
    };
    if !config
        .destinations
        .iter()
        .any(|configured| configured.name == destination)
    {
        bail!("unknown destination `{destination}`");
    }
    save_wallet(path, address, alias, vec![destination])
}

fn save_wallet(path: &Path, address: &str, alias: &str, destinations: Vec<String>) -> Result<()> {
    let mut config = read_editable_config(path)?;
    let wallet = SourceAddress::from_str(address)
        .with_context(|| format!("invalid wallet address `{address}`"))?;
    if alias.trim().is_empty() {
        bail!("wallet alias cannot be empty");
    }
    if destinations.is_empty() {
        bail!("choose at least one destination");
    }
    let starter_index = config
        .subscriptions
        .iter()
        .position(is_starter_subscription);
    if config
        .subscriptions
        .iter()
        .enumerate()
        .any(|(index, subscription)| subscription.name == alias && Some(index) != starter_index)
    {
        bail!("a wallet with alias `{alias}` already exists");
    }
    for destination in &destinations {
        if !config
            .destinations
            .iter()
            .any(|configured| configured.name == *destination)
        {
            bail!("unknown destination `{destination}`");
        }
    }

    let subscription = SubscriptionFileConfig {
        name: alias.to_owned(),
        address: wallet.to_string(),
        events: vec![EventType::TokenReceived],
        filters: FilterFileConfig::default(),
        route: RouteFileConfig {
            destinations: destinations.clone(),
        },
    };
    let replaced_starter = if let Some(index) = starter_index {
        config.subscriptions[index] = subscription;
        true
    } else {
        config.subscriptions.push(subscription);
        false
    };
    write_editable_config(path, &config)?;
    if replaced_starter {
        println!(
            "Replaced the starter wallet with `{alias}` ({wallet}) and routed token.received events to {}.",
            destinations.join(", ")
        );
    } else {
        println!(
            "Saved wallet `{alias}` ({wallet}) and routed token.received events to {}.",
            destinations.join(", ")
        );
    }
    println!("Run `solplay wallet` to choose it and set token filters.");
    Ok(())
}

fn setup(path: &Path) -> Result<()> {
    let theme = ColorfulTheme::default();
    let guide = Style::new().dim().apply_to(
        "Quick guide\n\
         • Telegram: create a bot with @BotFather (/newbot), then press Start or send it a message before using getUpdates to find your chat ID.\n\
         • Webhook: enter an HTTPS endpoint that accepts JSON POST requests.\n\
         • Discord: create a webhook in Server Settings → Integrations → Webhooks.\n\
         • Routes: use Space to select every destination that should receive a wallet's events.\n",
    );
    println!("{guide}");
    loop {
        let choice = Select::with_theme(&theme)
            .with_prompt("Solplay setup")
            .items(&[
                "Add a destination",
                "Add a wallet subscription",
                "Configure a wallet",
                "Finish",
            ])
            .default(0)
            .interact()
            .context("could not read setup selection")?;
        match choice {
            0 => add_destination_interactive(path, &theme)?,
            1 => add_wallet_interactive(path, &theme)?,
            2 => configure_wallet(path)?,
            _ => return Ok(()),
        }
    }
}

fn add_destination_interactive(path: &Path, theme: &ColorfulTheme) -> Result<()> {
    let mut config = read_editable_config(path)?;
    let name: String = Input::with_theme(theme)
        .with_prompt("Destination name")
        .interact_text()
        .context("could not read destination name")?;
    if name.trim().is_empty()
        || config
            .destinations
            .iter()
            .any(|destination| destination.name == name)
    {
        bail!("destination name must be unique and non-empty");
    }
    let kind = Select::with_theme(theme)
        .with_prompt("Destination type")
        .items(&["Webhook", "Discord", "Telegram"])
        .default(0)
        .interact()
        .context("could not read destination type")?;
    let destination = match kind {
        0 => {
            let url = secret_input(theme, "Webhook URL")?;
            let secret = Password::with_theme(theme)
                .with_prompt("Webhook signing secret (leave blank for none)")
                .allow_empty_password(true)
                .interact()
                .context("could not read webhook signing secret")?;
            let url_key = environment_key(&name, "WEBHOOK_URL");
            write_environment_value(path, &url_key, &url)?;
            let secret_key = environment_key(&name, "WEBHOOK_SECRET");
            if !secret.is_empty() {
                write_environment_value(path, &secret_key, &secret)?;
            }
            DestinationFileConfig {
                name,
                kind: "webhook".to_owned(),
                url: Some(environment_placeholder(&url_key)),
                secret: (!secret.is_empty()).then(|| environment_placeholder(&secret_key)),
                bot_token: None,
                chat_id: None,
            }
        }
        1 => {
            let url = secret_input(theme, "Discord webhook URL")?;
            let url_key = environment_key(&name, "DISCORD_URL");
            write_environment_value(path, &url_key, &url)?;
            DestinationFileConfig {
                name,
                kind: "discord".to_owned(),
                url: Some(environment_placeholder(&url_key)),
                secret: None,
                bot_token: None,
                chat_id: None,
            }
        }
        _ => {
            println!(
                "{}",
                Style::new().dim().apply_to(
                    "Telegram tip: create a bot with @BotFather, press Start or send it a message, then call https://api.telegram.org/bot<TOKEN>/getUpdates to find chat.id."
                )
            );
            let token = Password::with_theme(theme)
                .with_prompt("Telegram bot token")
                .interact()
                .context("could not read Telegram bot token")?;
            let chat_id: String = Input::with_theme(theme)
                .with_prompt("Telegram chat ID")
                .interact_text()
                .context("could not read Telegram chat ID")?;
            let token_key = environment_key(&name, "TELEGRAM_BOT_TOKEN");
            write_environment_value(path, &token_key, &token)?;
            DestinationFileConfig {
                name,
                kind: "telegram".to_owned(),
                url: None,
                secret: None,
                bot_token: Some(environment_placeholder(&token_key)),
                chat_id: Some(chat_id),
            }
        }
    };
    config.destinations.push(destination);
    write_editable_config(path, &config)?;
    println!(
        "Saved destination. Its credentials are in {}.",
        environment_path(path).display()
    );
    Ok(())
}

fn add_wallet_interactive(path: &Path, theme: &ColorfulTheme) -> Result<()> {
    let config = read_editable_config(path)?;
    if config.destinations.is_empty() {
        bail!("add a destination before adding a wallet subscription");
    }
    let address: String = Input::with_theme(theme)
        .with_prompt("Wallet address")
        .interact_text()
        .context("could not read wallet address")?;
    let alias: String = Input::with_theme(theme)
        .with_prompt("Wallet alias")
        .interact_text()
        .context("could not read wallet alias")?;
    let names = config
        .destinations
        .iter()
        .map(|destination| destination.name.as_str())
        .collect::<Vec<_>>();
    let selected = MultiSelect::with_theme(theme)
        .with_prompt("Route events to")
        .items(&names)
        .interact()
        .context("could not read destination routes")?;
    let destinations = selected
        .into_iter()
        .map(|index| names[index].to_owned())
        .collect();
    save_wallet(path, &address, &alias, destinations)
}

fn secret_input(theme: &ColorfulTheme, prompt: &str) -> Result<String> {
    Password::with_theme(theme)
        .with_prompt(prompt)
        .interact()
        .context("could not read secret value")
}

fn environment_key(name: &str, suffix: &str) -> String {
    let name = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("SOLPLAY_{name}_{suffix}")
}

fn environment_placeholder(key: &str) -> String {
    format!("${{{key}}}")
}

fn environment_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".env")
}

fn write_environment_value(config_path: &Path, key: &str, value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        bail!("environment values cannot contain newlines");
    }
    let path = environment_path(config_path);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut lines = existing
        .lines()
        .filter(|line| !line.trim_start().starts_with(&format!("{key}=")))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lines.push(format!("{key}={value}"));
    fs::write(&path, format!("{}\n", lines.join("\n")))
        .with_context(|| format!("could not write {}", path.display()))
}

fn is_starter_subscription(subscription: &SubscriptionFileConfig) -> bool {
    (subscription.name == "my-devnet-usdc-wallet"
        && subscription.address == "${SOLPLAY_WATCH_ADDRESS}")
        || (subscription.name == "treasury-usdc"
            && subscription.address == "11111111111111111111111111111111")
}

fn list_wallets(path: &Path) -> Result<()> {
    let config = read_editable_config(path)?;
    print_wallets(&config);
    Ok(())
}

fn configure_wallet(path: &Path) -> Result<()> {
    let mut config = read_editable_config(path)?;
    if config.subscriptions.is_empty() {
        bail!("no wallets saved; add one with `solplay set wallet <ADDRESS> <ALIAS>`");
    }
    print_wallets(&config);
    let choice = prompt("Choose a wallet number: ")?;
    let index = choice
        .parse::<usize>()
        .context("enter a wallet number")?
        .checked_sub(1)
        .context("wallet numbers start at 1")?;
    let subscription = config
        .subscriptions
        .get_mut(index)
        .context("wallet number is out of range")?;
    let alias = subscription.name.clone();

    println!("\nEditing `{alias}` ({})", subscription.address);
    println!("Press Enter to keep a value, or enter - to clear a filter.");
    update_mint_filter(&mut subscription.filters.mint)?;
    update_decimal_filter("Minimum token amount", &mut subscription.filters.min_amount)?;
    update_decimal_filter("Maximum token amount", &mut subscription.filters.max_amount)?;
    write_editable_config(path, &config)?;
    println!("Saved filters for `{alias}`.");
    Ok(())
}

fn print_wallets(config: &FileConfig) {
    if config.subscriptions.is_empty() {
        println!("No wallets saved.");
        return;
    }
    println!("Saved wallets:");
    for (index, subscription) in config.subscriptions.iter().enumerate() {
        println!(
            "  {}. {:<20} {}",
            index + 1,
            subscription.name,
            subscription.address
        );
    }
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush().context("could not write prompt")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("could not read input")?;
    Ok(input.trim().to_owned())
}

fn update_mint_filter(filter: &mut Option<String>) -> Result<()> {
    let current = filter.as_deref().unwrap_or("any token");
    let value = prompt(&format!("Token mint [{current}]: "))?;
    if value.is_empty() {
        return Ok(());
    }
    if value == "-" {
        *filter = None;
        return Ok(());
    }
    let mint = SourceAddress::from_str(&value).context("invalid token mint address")?;
    *filter = Some(mint.to_string());
    Ok(())
}

fn update_decimal_filter(label: &str, filter: &mut Option<Decimal>) -> Result<()> {
    let current = filter
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "any amount".to_owned());
    let value = prompt(&format!("{label} [{current}]: "))?;
    if value.is_empty() {
        return Ok(());
    }
    if value == "-" {
        *filter = None;
        return Ok(());
    }
    *filter = Some(Decimal::from_str(&value).context("amount must be a decimal number")?);
    Ok(())
}

fn read_editable_config(path: &Path) -> Result<FileConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    toml::from_str(&raw).context("could not parse TOML configuration")
}

fn write_editable_config(path: &Path, config: &FileConfig) -> Result<()> {
    let raw = toml::to_string_pretty(config).context("could not serialize TOML configuration")?;
    fs::write(path, raw).with_context(|| format!("could not write {}", path.display()))
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
                    let event_type = event.event_type;
                    let signature = event.signature.clone();
                    let source = event.source.address.clone();
                    let mint = event.data.mint.clone();
                    let amount = event.data.amount;
                    match engine.process(event, &config.subscriptions).await? {
                        solplay_engine::ProcessResult::Duplicate => tracing::debug!("duplicate event ignored"),
                        solplay_engine::ProcessResult::Processed { delivered, failed } => tracing::info!(
                            ?event_type,
                            %signature,
                            %source,
                            ?mint,
                            ?amount,
                            delivered,
                            failed,
                            "processed Solana event"
                        ),
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

    #[test]
    fn recognizes_the_generated_starter_wallet() {
        let starter = SubscriptionFileConfig {
            name: "my-devnet-usdc-wallet".to_owned(),
            address: "${SOLPLAY_WATCH_ADDRESS}".to_owned(),
            events: vec![EventType::TokenReceived],
            filters: FilterFileConfig::default(),
            route: RouteFileConfig {
                destinations: vec!["backend".to_owned()],
            },
        };
        assert!(is_starter_subscription(&starter));
    }
}
