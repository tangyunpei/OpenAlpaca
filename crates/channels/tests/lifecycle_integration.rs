use std::sync::Arc;

use openalpaca_channels::plugin::{ChannelGatewayContext, OutboundContext};
use openalpaca_channels::{ChannelRegistry, EchoChannel};
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId};

#[tokio::test]
async fn test_register_start_check_stop() {
    let mut registry = ChannelRegistry::new();
    let account_id = AccountId::new_unchecked("default");
    registry.register(account_id.clone(), Box::new(EchoChannel::new()));

    let channel_id = ChannelId::new_unchecked("echo");
    let plugin = registry.get(&channel_id, &account_id).unwrap();

    let config = Arc::new(OpenAlpacaConfig::default());
    let ctx = ChannelGatewayContext {
        account_id: account_id.clone(),
        config: config.clone(),
    };

    // Full lifecycle: start → check_ready → stop
    plugin.start_account(&ctx).await.unwrap();

    let ready = plugin.check_ready(&config, &account_id).await.unwrap();
    assert!(ready);

    plugin.stop_account(&ctx).await.unwrap();
}

#[tokio::test]
async fn test_send_text_via_registry() {
    let mut registry = ChannelRegistry::new();
    let account_id = AccountId::new_unchecked("default");
    registry.register(account_id.clone(), Box::new(EchoChannel::new()));

    let channel_id = ChannelId::new_unchecked("echo");
    let plugin = registry.get(&channel_id, &account_id).unwrap();

    let ctx = OutboundContext {
        channel_id: channel_id.clone(),
        account_id,
        target: "user123".into(),
        text: "hello world".into(),
        reply_to_id: None,
        thread_id: None,
    };

    let result = plugin.send_text(&ctx).await.unwrap();
    assert!(result.message_id.is_some());
    assert_eq!(result.message_id.unwrap(), "echo-user123");
}

#[tokio::test]
async fn test_multiple_accounts_independent() {
    let mut registry = ChannelRegistry::new();
    let a1 = AccountId::new_unchecked("a1");
    let a2 = AccountId::new_unchecked("a2");

    registry.register(a1.clone(), Box::new(EchoChannel::new()));
    registry.register(a2.clone(), Box::new(EchoChannel::new()));

    let channel_id = ChannelId::new_unchecked("echo");

    // Both accounts accessible independently
    let p1 = registry.get(&channel_id, &a1);
    let p2 = registry.get(&channel_id, &a2);
    assert!(p1.is_some());
    assert!(p2.is_some());

    // Both are echo channels
    assert_eq!(p1.unwrap().id().as_str(), "echo");
    assert_eq!(p2.unwrap().id().as_str(), "echo");

    assert_eq!(registry.len(), 2);
}
