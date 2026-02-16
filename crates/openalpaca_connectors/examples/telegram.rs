//! Example: Run Telegram Connector standalone
//!
//! Usage:
//! 1. Get a bot token from @BotFather
//! 2. Run: TELOXIDE_TOKEN=your_token cargo run --example telegram --features telegram

use async_trait::async_trait;
use openalpaca_connectors::TelegramConnector;
use openalpaca_core::{
    bus::EventBus,
    context::SharedContext,
    gateway::{Gateway, HandleResult, MessageHandler},
    lane::LaneManager,
    security::policy::{Principal, Scope},
};
use openalpaca_storage::{Database, paths};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup logging
    tracing_subscriber::fmt::init();

    // 2. Get token from environment
    let token =
        std::env::var("TELOXIDE_TOKEN").expect("TELOXIDE_TOKEN environment variable not set");

    // 3. Connect to the real system database (shared with Daemon)
    let db_path = paths::database_path()?;
    println!("Using Database: {}", db_path.display());

    let db = Database::open(&db_path)?;
    let bus = EventBus::default();

    println!("Starting Telegram Connector Example...");
    println!("Bot Token: {}...", &token[..8]);

    // 4. Create Gateway with a stub handler
    struct EchoHandler;

    #[async_trait]
    impl MessageHandler for EchoHandler {
        async fn handle(
            &self,
            _request_id: Uuid,
            _source: String,
            content: String,
            _principal: Principal,
            _scope: Scope,
            _lane_key: String,
        ) -> Result<HandleResult, String> {
            Ok(HandleResult::text(format!("Echo: {content}")))
        }
    }

    let gateway = Arc::new(Gateway::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        Arc::new(EchoHandler),
        bus.clone(),
        None,
    ));

    // 5. Create and run connector
    let connector = TelegramConnector::new(token, Arc::new(db), Arc::new(bus), gateway);

    // Note: In a real app, you'd spawn this or run in background
    // For the example, we run it blocking.
    let shutdown_token = connector.run_with_signal().await;

    // Simulate running for a bit
    tokio::signal::ctrl_c().await.unwrap();
    shutdown_token.shutdown().unwrap().await;

    Ok(())
}
