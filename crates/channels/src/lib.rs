pub mod allow_from;
pub mod echo;
pub mod error;
pub mod inbound;
pub mod meta;
pub mod plugin;
pub mod registry;
pub mod typing;

pub use allow_from::is_sender_allowed;
pub use echo::EchoChannel;
pub use error::ChannelError;
pub use inbound::{EchoHandler, InboundHandler, InboundMessage};
pub use meta::{ChannelAccountSnapshot, ChannelCapabilities, ChannelMeta};
pub use plugin::{
    ChannelGatewayContext, ChannelPlugin, ChannelStatusIssue, DeliveryMode, MediaOutboundContext,
    OutboundContext, OutboundResult, ReplyToMode, StatusIssueKind,
};
pub use registry::{ChannelKey, ChannelRegistry};
pub use typing::TypingHandle;
