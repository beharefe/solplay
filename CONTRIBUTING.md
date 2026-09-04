# Contributing to Solplay

Thanks for helping improve Solplay.

## Before opening a pull request

- Keep changes focused on one clear behavior.
- Add or update tests for behavior changes.
- Do not include RPC credentials, webhook URLs, bot tokens, keypairs, or `.env` files.
- Keep Solana RPC provider-neutral. Solplay must work with user-supplied endpoints.

## Local checks

Run these before opening a pull request:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

The optional local-validator integration test requires `solana-test-validator` on your `PATH`:

```bash
cargo test -p solplay-rpc -- --ignored --test-threads=1
```

## Pull requests

Explain the problem, the behavior change, and how you tested it. Update the README when setup, configuration, or user-visible behavior changes.

## Reporting issues

Include the Solplay version, operating system, RPC endpoint type (without credentials), relevant sanitized configuration, and logs with secrets removed.
