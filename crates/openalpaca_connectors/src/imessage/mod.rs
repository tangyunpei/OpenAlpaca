//! iMessage Connector Module
//!
//! Provides iMessage integration on macOS by reading from chat.db
//! and sending replies via AppleScript (osascript).

mod connector;
mod reader;
mod sender;

pub use connector::IMessageConnector;
pub use sender::IMessageSender;
