//! Example: Run Telegram Connector standalone
//!
//! Usage:
//! 1. Get a bot token from @BotFather
//! 2. Run: TELOXIDE_TOKEN=your_token cargo run --example telegram --features telegram

use openalpaca_connectors::TelegramConnector;
use openalpaca_core::bus::EventBus;
use openalpaca_storage::{Database, paths};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup logging
    tracing_subscriber::fmt::init();

    // 2. Get token from environment
    let token =
        std::env::var("TELOXIDE_TOKEN").expect("TELOXIDE_TOKEN environment variable not set");

    // 3. Connect to the real system database (shared with Daemon)
    let db_path = paths::database_path()?;
    println!("📂 Using Database: {}", db_path.display());

    let db = Database::open(&db_path)?;
    let bus = EventBus::default();

    println!("🚀 Starting Telegram Connector Example...");
    println!("Bot Token: {}...", &token[..8]);

    // 4. Create and run connector
    let connector = TelegramConnector::new(token, Arc::new(db), Arc::new(bus));

    // Note: In a real app, you'd spawn this or run in background
    // For the example, we run it blocking.
    connector.run_blocking().await;

    Ok(())
}
