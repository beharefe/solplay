# Solplay

Solplay is a small, self-hosted helper for seeing Solana activity as it happens. Watch a wallet, account, or program address, filter the events you care about, and send the result to a webhook, Discord, or Telegram.

It is useful for debugging a treasury, vault, or program in real time—and for running small automations locally or on your own server. You provide the RPC endpoints and keep control of the process, configuration, and event history.

## What it watches

- Transaction confirmations and failures
- SOL deposits and withdrawals
- Token received and sent events
- Writable-account changes and program invocations

Wallet subscriptions also discover Classic Token Program and Token Extensions Program accounts, so incoming token transfers can be observed through their token accounts.

## What it does

- Loads a portable TOML configuration with environment variables for secrets
- Persists event and delivery history locally in SQLite
- Reconnects address subscriptions and refreshes discovered token accounts
- Retries transient delivery failures
- Delivers normalized JSON to generic webhooks, formatted Discord webhooks, or Telegram chats

## Setup

Install a release binary, or use Rust 1.95 or newer. After the first release is
published, install the latest binary globally on supported macOS and Linux
systems with:

```bash
curl -fsSL https://raw.githubusercontent.com/beharefe/solplay/main/scripts/install.sh | sh
```

The installer verifies the release checksum and installs `solplay` into
`~/.local/bin`. If you use Rust instead, create a configuration file with:

```bash
cargo run -p solplay -- init
```

Set the environment variables referenced by `solplay.toml`, then validate it:

```bash
SOLPLAY_RPC_HTTP=https://api.devnet.solana.com \
SOLPLAY_RPC_WS=wss://api.devnet.solana.com \
SOLPLAY_WEBHOOK_URL=https://example.com/solana \
SOLPLAY_WEBHOOK_SECRET=change-me \
SOLPLAY_WATCH_ADDRESS=your-devnet-wallet-address \
cargo run -p solplay -- validate
```

The generated example watches incoming devnet USDC. Replace
`SOLPLAY_WATCH_ADDRESS` with your wallet's public address. It uses Circle's
devnet USDC mint, `4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU`; the
similarly common `EPjFW...` mint is for mainnet USDC.

You can also save these values in a `.env` file. Solplay first searches the
current directory and its parents, then checks the directory that contains the
executable. For a local debug build, both `./.env` and `target/debug/.env` work.
Variables exported in your shell take precedence over values in either file.

## Add and configure a wallet

After you have configured one destination, you can save a wallet without
editing TOML. The first command replaces the starter wallet created by `init`.

```bash
solplay set wallet HtNmnQDqoNNZfSPAZkv6WQPHQqrftD84iZ2ytgF8Wqiz test-wallet
solplay wallet
```

`solplay wallet` lists saved aliases and addresses, then lets you select one.
Enter a token mint and optional minimum or maximum amount to filter its incoming
token transfers. Use `solplay wallet list` to view saved wallets without making
changes.

## Interactive setup

Use the setup wizard to add a destination and route wallet subscriptions without
editing TOML. It supports generic webhooks, Discord webhooks, and Telegram.

```bash
solplay setup
```

For Telegram, choose **Add a destination**, select **Telegram**, then enter a
destination name, bot token, and chat ID. Before you look up the chat ID, open
the bot and press **Start** or send it a message; Telegram bots can't start a
private conversation themselves. The bot token is entered without echo and
saved in `.env`; `solplay.toml` contains only an environment-variable reference.
When you add a wallet subscription, select one or more destinations to receive
its events.

## Examples

Use the [examples](./examples/README.md) for complete Telegram, Discord, and
generic-webhook configurations. Each one includes the required environment
variables and integration-specific setup steps.

## Architecture

`solplay-core` owns framework-independent event, filter, subscription, and destination types. `solplay-engine` coordinates persistence and delivery. RPC, storage, and destination adapters depend on the core domain, while `solplay-cli` remains a thin user interface over those pieces.

## Testing

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p solplay-rpc -- --ignored
cargo test --workspace
```

## License

MIT. See [LICENSE](LICENSE).

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) and follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Usage

```bash
solplay init
solplay validate --config ./solplay.toml
solplay run --config ./solplay.toml
solplay logs --database ./.solplay/solplay.db
solplay monitor --database ./.solplay/solplay.db
```
