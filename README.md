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

Install Rust 1.95 or newer, then create a configuration file:

```bash
cargo run -p solplay -- init
```

Set the environment variables referenced by `solplay.toml`, then validate it:

```bash
SOLPLAY_RPC_HTTP=https://api.devnet.solana.com \
SOLPLAY_RPC_WS=wss://api.devnet.solana.com \
SOLPLAY_WEBHOOK_URL=https://example.com/solana \
SOLPLAY_WEBHOOK_SECRET=change-me \
cargo run -p solplay -- validate
```

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
