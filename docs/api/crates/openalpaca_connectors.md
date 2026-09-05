# `openalpaca_connectors`

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_connectors`
- Entry: `crates/openalpaca_connectors/src/lib.rs`

- OpenAlpaca Connectors Module
- Provides unified connector interface for chat platforms.
- Each platform is feature-gated:
- - `telegram`: Telegram Bot API via teloxide
- - `imessage`: iMessage (macOS only)
- - `wechat`: WeChat (future)
- All connectors implement the `Connector` trait for a uniform interface.

## Modules

- `common` (crates/openalpaca_connectors/src/common/mod.rs)
- `telegram` (crates/openalpaca_connectors/src/telegram/mod.rs) `#[cfg(feature = "telegram")]`
- `imessage` (crates/openalpaca_connectors/src/imessage/mod.rs) `#[cfg(all(feature = "imessage", target_os = "macos"))]`
- `discord` (crates/openalpaca_connectors/src/discord/mod.rs) `#[cfg(feature = "discord")]`
- `startup` (crates/openalpaca_connectors/src/startup.rs)

## Re-exports

- `pub use telegram::TelegramConnector;`
- `pub use imessage::IMessageConnector;`
- `pub use discord::DiscordConnector;`

## Related Links

- [API Index](../README.md)
