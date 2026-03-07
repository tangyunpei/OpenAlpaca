//! Telegram Connector Module
//!
//! Provides Telegram Bot integration via teloxide.

mod connector;
mod delivery;
mod handler;
mod rate_limiter;

#[cfg(test)]
mod tests;

pub use connector::TelegramConnector;
