# Changelog

## 0.1.0 - Unreleased

- Added the Rust workspace and crate boundaries.
- Added typed events, subscriptions, filtering, and destination configuration.
- Added TOML configuration initialization and validation with environment interpolation.
- Added SQLite event history and delivery-attempt persistence.
- Added signed generic-webhook delivery and the engine processing boundary.
- Added reconnecting Solana `logsSubscribe` observation for confirmed and failed transaction events.
- Added official Solana RPC client enrichment using parsed `getTransaction` responses.
- Added normalized wallet SOL and token received/sent events from parsed transaction balance changes.
- Added Discord webhook and Telegram Bot API destination adapters with redacted transport failures.
- Added Classic Token Program and Token Extensions Program account discovery for wallet subscriptions.
- Added three-attempt delivery retries for network failures and HTTP 408, 429, and 5xx responses.
- Added a SQLite-backed live `solplay monitor` dashboard.
- Added periodic token-account discovery, retrying transaction enrichment, and generic account/program event normalization.
- Aligned the Rust RPC packages to the Solana 4.2.2 family and added an opt-in local-validator RPC integration test.
