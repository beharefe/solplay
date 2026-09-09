# Generic webhook

This example sends devnet USDC receipts as JSON to your server. Copy this
directory's `solplay.toml` to your project root and create `.env` beside it:

```dotenv
SOLPLAY_RPC_HTTP=https://api.devnet.solana.com
SOLPLAY_RPC_WS=wss://api.devnet.solana.com
SOLPLAY_WATCH_ADDRESS=your-devnet-wallet-address
SOLPLAY_WEBHOOK_URL=https://your-server.example.com/solplay
SOLPLAY_WEBHOOK_SECRET=replace-with-a-random-secret
```

Your endpoint must accept JSON `POST` requests. When a secret is set, Solplay
adds a `Solplay-Signature` header with a timestamp and HMAC-SHA256 signature of
`<timestamp>.<raw-request-body>`. Reject missing, invalid, or expired
signatures before processing the event.
