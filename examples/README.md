# Solplay examples

Each example watches one wallet and routes matching incoming token transfers to
one destination. Copy the example you want to `solplay.toml`, create a matching
`.env`, then run `solplay validate` before you start the listener.

- [Telegram](./telegram/README.md) sends messages to a private chat, group, or
  channel.
- [Discord](./discord/README.md) posts formatted events to a Discord webhook.
- [Generic webhook](./webhook/README.md) sends signed JSON to your server.

The interactive alternative is `solplay setup`, which writes destination secrets
to `.env` and keeps `solplay.toml` portable.
