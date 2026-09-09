# Telegram notifications

This example sends devnet USDC receipts to a Telegram chat. Copy
`solplay.toml` from this directory to your project root and create `.env` with
these values:

```dotenv
SOLPLAY_RPC_HTTP=https://api.devnet.solana.com
SOLPLAY_RPC_WS=wss://api.devnet.solana.com
SOLPLAY_WATCH_ADDRESS=your-devnet-wallet-address
SOLPLAY_TELEGRAM_BOT_TOKEN=your-bot-token
SOLPLAY_TELEGRAM_CHAT_ID=your-private-chat-id
```

Create the bot with `@BotFather`, then open it and press **Start** or send it a
message. Bots cannot initiate private conversations. Request
`https://api.telegram.org/bot<TOKEN>/getUpdates` and use `message.chat.id` from
the response as `SOLPLAY_TELEGRAM_CHAT_ID`.

Run these commands from the directory that contains the copied configuration:

```bash
solplay validate
solplay run
```
