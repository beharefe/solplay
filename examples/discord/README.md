# Discord notifications

This example posts devnet USDC receipts to a Discord webhook. In your Discord
server, create a webhook in **Server Settings** → **Integrations** →
**Webhooks**, then copy this directory's `solplay.toml` to your project root.

Create `.env` beside the copied configuration:

```dotenv
SOLPLAY_RPC_HTTP=https://api.devnet.solana.com
SOLPLAY_RPC_WS=wss://api.devnet.solana.com
SOLPLAY_WATCH_ADDRESS=your-devnet-wallet-address
SOLPLAY_DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/...
```

Run `solplay validate`, then `solplay run`. Treat the Discord webhook URL as a
secret because anyone who has it can post to that channel.
