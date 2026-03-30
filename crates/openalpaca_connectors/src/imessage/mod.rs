//! iMessage Connector Module
//!
//! Provides iMessage integration on macOS by reading from chat.db
//! and sending replies via AppleScript (osascript).

mod connector;
mod handler;
mod reader;
mod routing;
mod sender;

#[cfg(test)]
mod tests;

pub use connector::IMessageConnector;
pub use sender::IMessageSender;
