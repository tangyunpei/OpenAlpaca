//! Telegram Connector Module
//!
//! Provides Telegram Bot integration via teloxide.

mod connector;
mod delivery;
mod handler;

#[cfg(test)]
mod tests;

pub use connector::TelegramConnector;
