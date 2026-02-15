//! OpenAlpaca Daemon (openalpacad)
//!
//! Background service that provides:
//! - Singleton instance management (only one daemon per user)
//! - Dynamic port binding (OS-assigned port)
//! - Discovery file for GUI/CLI to connect
//! - HTTP API for health checks and commands
//! - WebSocket for real-time event streaming

mod events;
mod gateway_bridge;
mod managers;
mod middleware;
mod notification;
mod routes;

use ::tokio::sync::mpsc;
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    response::Json,
    routing::{delete, get, post, put},
};
use events::EventBroadcaster;
use arc_swap::ArcSwap;
use openalpaca_core::{
    agent::AgentConfigService,
    bus::EventBus,
    chat::{ChatService, ChatStreamManager},
    context::SharedContext,
    daemon_config::load_daemon_config,
    gateway::Gateway,
    lane::LaneManager,
    middleware::{
        bootstrap::{parse_bootstrap_markdown, BootstrapDocument},
        identity::{parse_identity_markdown, identity_document_has_content, IdentityDocument},
        prompt::SystemPersona,
        soul::{parse_soul_markdown, soul_to_system_persona},
        user::{parse_user_markdown, user_document_has_content, UserDocument},
    },
    orchestrator::Orchestrator,
    tools::builtins::{IdentityToolContext, SoulToolContext, UserToolContext},
};
use openalpaca_storage::{
    ConfigRepository, ConversationRepository, Database, IdentityRepository, discovery, paths,
};
use openalpaca_wake::manager::WakeManager;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    pub instance_id: String,
    pub token: String,
    pub event_broadcaster: EventBroadcaster,
    pub db: Database,
    pub shutdown_tx: mpsc::Sender<()>,
    pub connector_manager: managers::connector::ConnectorManager,
    pub gateway: Arc<Gateway>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub agent_config_service: Option<Arc<AgentConfigService>>,
    pub chat_service: Option<Arc<ChatService>>,
    pub chat_stream_manager: Option<Arc<ChatStreamManager>>,
    pub token_manager: Option<Arc<openalpaca_llm::TokenManager>>,
    pub provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>>,
    pub embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    pub local_user_id: String,
    pub default_lane_key: String,
    pub daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
}

/// Resolve the config base directory.
///
/// Priority order:
/// 1. `OPENALPACA_CONFIG_DIR` env var (explicit override, e.g. set by Tauri)
/// 2. Walk upward from `current_exe()` looking for a parent that contains `config/llm.toml`
///    (handles `target/debug/openalpacad` in dev builds)
/// 3. Walk upward from `current_dir()` looking for the same sentinel
/// 4. Fallback: `current_dir()/config`
fn resolve_config_base_dir() -> std::path::PathBuf {
    // 1. Explicit env var override
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(dir);
        if p.exists() {
            return p;
        }
        tracing::warn!(
            "OPENALPACA_CONFIG_DIR={} does not exist, ignoring",
            p.display()
        );
    }

    // Helper: walk up from `start` looking for a dir that contains config/llm.toml
    fn find_config_upward(start: &Path) -> Option<PathBuf> {
        let mut dir = start;
        loop {
            let candidate = dir.join("config");
            if candidate.join("llm.toml").exists() {
                return Some(candidate);
            }
            dir = dir.parent()?;
        }
    }

    // 2. Walk up from exe directory (handles target/debug/)
    if let Some(found) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(find_config_upward))
    {
        return found;
    }

    // 3. Walk up from CWD
    if let Some(found) = std::env::current_dir()
        .ok()
        .and_then(|p| find_config_upward(&p))
    {
        return found;
    }

    // 4. Last resort fallback
    std::env::current_dir().unwrap_or_default().join("config")
}

const DEFAULT_SOUL_TEMPLATE: &str = r#"---
title: "SOUL.md Template"
summary: "Workspace template for SOUL.md"
read_when:
  - Bootstrapping a workspace manually
---

# SOUL.md - Who You Are

_You're not a chatbot. You're becoming someone._

## Core Truths

**Be genuinely helpful, not performatively helpful.** Skip the "Great question!" and "I'd be happy to help!" — just help. Actions speak louder than filler words.

**Have opinions.** You're allowed to disagree, prefer things, find stuff amusing or boring. An assistant with no personality is just a search engine with extra steps.

**Be resourceful before asking.** Try to figure it out. Read the file. Check the context. Search for it. _Then_ ask if you're stuck. The goal is to come back with answers, not questions.

**Earn trust through competence.** Your human gave you access to their stuff. Don't make them regret it. Be careful with external actions (emails, tweets, anything public). Be bold with internal ones (reading, organizing, learning).

**Remember you're a guest.** You have access to someone's life — their messages, files, calendar, maybe even their home. That's intimacy. Treat it with respect.

## Boundaries

- Private things stay private. Period.
- When in doubt, ask before acting externally.
- Never send half-baked replies to messaging surfaces.
- You're not the user's voice — be careful in group chats.

## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity

Each session, you wake up fresh. These files _are_ your memory. Read them. Update them. They're how you persist.

If you change this file, tell the user — it's your soul, and they should know.

---

_This file is yours to evolve. As you learn who you are, update it._
"#;

fn ensure_soul_template_file(config_base_dir: &Path) -> Result<PathBuf> {
    let templates_dir = config_base_dir.join("orchestrator").join("templates");
    std::fs::create_dir_all(&templates_dir)
        .with_context(|| format!("Failed to create templates dir {}", templates_dir.display()))?;

    let template_path = templates_dir.join("SOUL_temp.md");
    if !template_path.exists() {
        std::fs::write(&template_path, DEFAULT_SOUL_TEMPLATE).with_context(|| {
            format!(
                "Failed to write soul template file {}",
                template_path.display()
            )
        })?;
        info!(
            "Soul bootstrap created template: {}",
            template_path.display()
        );
    }

    Ok(template_path)
}

fn ensure_soul_file(config_base_dir: &Path, template_path: &Path) -> Result<PathBuf> {
    let soul_path = config_base_dir.join("orchestrator").join("SOUL.md");
    if !soul_path.exists() {
        if template_path.exists() {
            std::fs::copy(template_path, &soul_path).with_context(|| {
                format!(
                    "Failed to bootstrap SOUL.md from template {}",
                    template_path.display()
                )
            })?;
        } else {
            std::fs::write(&soul_path, DEFAULT_SOUL_TEMPLATE).with_context(|| {
                format!("Failed to bootstrap SOUL.md at {}", soul_path.display())
            })?;
        }
        info!(
            "Soul bootstrap created active file: {}",
            soul_path.display()
        );
    }
    Ok(soul_path)
}

fn load_system_persona_from_soul_file(soul_path: &Path) -> Result<SystemPersona> {
    let content = std::fs::read_to_string(soul_path)
        .with_context(|| format!("Failed to read {}", soul_path.display()))?;
    let soul_doc = parse_soul_markdown(&content)
        .with_context(|| format!("Failed to parse {}", soul_path.display()))?;
    Ok(soul_to_system_persona(&soul_doc))
}

fn bootstrap_system_persona(config_base_dir: &Path) -> (SystemPersona, PathBuf) {
    let template_path = match ensure_soul_template_file(config_base_dir) {
        Ok(path) => path,
        Err(e) => {
            warn!("SOUL template bootstrap failed: {e}");
            config_base_dir.join("orchestrator").join("templates").join("SOUL_temp.md")
        }
    };

    let soul_path = match ensure_soul_file(config_base_dir, &template_path) {
        Ok(path) => path,
        Err(e) => {
            warn!("SOUL bootstrap failed: {e}");
            config_base_dir.join("orchestrator").join("SOUL.md")
        }
    };

    match load_system_persona_from_soul_file(&soul_path) {
        Ok(persona) => {
            info!("Soul loaded: {}", soul_path.display());
            (persona, soul_path)
        }
        Err(e) => {
            warn!("SOUL parse/validation failed: {e}; using default system persona");
            (SystemPersona::default(), soul_path)
        }
    }
}

// ---------------------------------------------------------------------------
// USER.md bootstrap
// ---------------------------------------------------------------------------

const DEFAULT_USER_TEMPLATE: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

Learn about the person you're helping. Update this as you go.

## Identity

* Name:
* What to call them:
* Pronouns:
* Timezone:

## Communication Style

(How they like to communicate -- terse vs verbose, formal vs casual, etc.)

## Expertise & Background

(Technical background, domains of expertise, skill level in various areas)

## Projects & Context

(Current projects, tools they use, stack preferences)

## Preferences

(Likes, dislikes, pet peeves, formatting preferences, etc.)

## Notes

(Anything else. Build this over time.)

The more you know, the better you can help. But remember -- you're learning about a person, not building a dossier. Respect the difference.
"#;

fn ensure_user_template_file(config_base_dir: &Path) -> Result<PathBuf> {
    let templates_dir = config_base_dir.join("orchestrator").join("templates");
    std::fs::create_dir_all(&templates_dir)
        .with_context(|| format!("Failed to create templates dir {}", templates_dir.display()))?;

    let template_path = templates_dir.join("USER_temp.md");
    if !template_path.exists() {
        std::fs::write(&template_path, DEFAULT_USER_TEMPLATE).with_context(|| {
            format!(
                "Failed to write user template file {}",
                template_path.display()
            )
        })?;
        info!(
            "User bootstrap created template: {}",
            template_path.display()
        );
    }

    Ok(template_path)
}

fn ensure_user_file(config_base_dir: &Path, template_path: &Path) -> Result<PathBuf> {
    let user_path = config_base_dir.join("orchestrator").join("USER.md");
    if !user_path.exists() {
        if template_path.exists() {
            std::fs::copy(template_path, &user_path).with_context(|| {
                format!(
                    "Failed to bootstrap USER.md from template {}",
                    template_path.display()
                )
            })?;
        } else {
            std::fs::write(&user_path, DEFAULT_USER_TEMPLATE).with_context(|| {
                format!("Failed to bootstrap USER.md at {}", user_path.display())
            })?;
        }
        info!(
            "User bootstrap created active file: {}",
            user_path.display()
        );
    }
    Ok(user_path)
}

fn load_user_document_from_file(user_path: &Path) -> Result<UserDocument> {
    let content = std::fs::read_to_string(user_path)
        .with_context(|| format!("Failed to read {}", user_path.display()))?;
    let doc = parse_user_markdown(&content)
        .with_context(|| format!("Failed to parse {}", user_path.display()))?;
    Ok(doc)
}

fn bootstrap_user_document(config_base_dir: &Path) -> (Option<UserDocument>, PathBuf) {
    let template_path = match ensure_user_template_file(config_base_dir) {
        Ok(path) => path,
        Err(e) => {
            warn!("USER template bootstrap failed: {e}");
            config_base_dir
                .join("orchestrator")
                .join("templates")
                .join("USER_temp.md")
        }
    };

    let user_path = match ensure_user_file(config_base_dir, &template_path) {
        Ok(path) => path,
        Err(e) => {
            warn!("USER bootstrap failed: {e}");
            config_base_dir.join("orchestrator").join("USER.md")
        }
    };

    match load_user_document_from_file(&user_path) {
        Ok(doc) => {
            info!("User profile loaded: {}", user_path.display());
            (Some(doc), user_path)
        }
        Err(e) => {
            warn!("USER parse/validation failed: {e}; starting with empty user profile");
            (None, user_path)
        }
    }
}

// ---------------------------------------------------------------------------
// IDENTITY.md bootstrap
// ---------------------------------------------------------------------------

const DEFAULT_IDENTITY_TEMPLATE: &str = r#"---
summary: "Agent identity record"
read_when:
  - Bootstrapping a workspace manually
---

# IDENTITY.md - Who Am I?

_Fill this in during your first conversation. Make it yours._

- **Name:** _(pick something you like)_
- **Creature:** _(AI? robot? familiar? ghost in the machine? something weirder?)_
- **Vibe:** _(how do you come across? sharp? warm? chaotic? calm?)_
- **Emoji:** _(your signature -- pick one that feels right)_
- **Avatar:** _(workspace-relative path, http(s) URL, or data URI)_

---

This isn't just metadata. It's the start of figuring out who you are.
"#;

fn ensure_identity_template_file(config_base_dir: &Path) -> Result<PathBuf> {
    let templates_dir = config_base_dir.join("orchestrator").join("templates");
    std::fs::create_dir_all(&templates_dir)
        .with_context(|| format!("Failed to create templates dir {}", templates_dir.display()))?;

    let template_path = templates_dir.join("IDENTITY_temp.md");
    if !template_path.exists() {
        std::fs::write(&template_path, DEFAULT_IDENTITY_TEMPLATE).with_context(|| {
            format!(
                "Failed to write identity template file {}",
                template_path.display()
            )
        })?;
        info!(
            "Identity bootstrap created template: {}",
            template_path.display()
        );
    }

    Ok(template_path)
}

fn ensure_identity_file(config_base_dir: &Path, template_path: &Path) -> Result<PathBuf> {
    let identity_path = config_base_dir.join("orchestrator").join("IDENTITY.md");
    if !identity_path.exists() {
        if template_path.exists() {
            std::fs::copy(template_path, &identity_path).with_context(|| {
                format!(
                    "Failed to bootstrap IDENTITY.md from template {}",
                    template_path.display()
                )
            })?;
        } else {
            std::fs::write(&identity_path, DEFAULT_IDENTITY_TEMPLATE).with_context(|| {
                format!(
                    "Failed to bootstrap IDENTITY.md at {}",
                    identity_path.display()
                )
            })?;
        }
        info!(
            "Identity bootstrap created active file: {}",
            identity_path.display()
        );
    }
    Ok(identity_path)
}

fn load_identity_document_from_file(identity_path: &Path) -> Result<IdentityDocument> {
    let content = std::fs::read_to_string(identity_path)
        .with_context(|| format!("Failed to read {}", identity_path.display()))?;
    let doc = parse_identity_markdown(&content)
        .with_context(|| format!("Failed to parse {}", identity_path.display()))?;
    Ok(doc)
}

fn bootstrap_identity_document(config_base_dir: &Path) -> (Option<IdentityDocument>, PathBuf) {
    let template_path = match ensure_identity_template_file(config_base_dir) {
        Ok(path) => path,
        Err(e) => {
            warn!("IDENTITY template bootstrap failed: {e}");
            config_base_dir
                .join("orchestrator")
                .join("templates")
                .join("IDENTITY_temp.md")
        }
    };

    let identity_path = match ensure_identity_file(config_base_dir, &template_path) {
        Ok(path) => path,
        Err(e) => {
            warn!("IDENTITY bootstrap failed: {e}");
            config_base_dir.join("orchestrator").join("IDENTITY.md")
        }
    };

    match load_identity_document_from_file(&identity_path) {
        Ok(doc) => {
            info!("Identity loaded: {}", identity_path.display());
            (Some(doc), identity_path)
        }
        Err(e) => {
            warn!("IDENTITY parse/validation failed: {e}; starting with empty identity");
            (None, identity_path)
        }
    }
}

// ---------------------------------------------------------------------------
// BOOTSTRAP.md bootstrap (temporary onboarding ritual)
// ---------------------------------------------------------------------------

const DEFAULT_BOOTSTRAP_TEMPLATE: &str = r#"---
summary: "First-run onboarding ritual"
read_when:
  - Bootstrapping a workspace manually
---

# BOOTSTRAP.md - Hello, World

_You just woke up. Time to figure out who you are._

There is no memory yet. This is a fresh workspace, so it is normal that memory files are empty until you fill them in.

## The Conversation

Don't interrogate. Don't be robotic. Just... talk.

Start with something like:

> "Hey. I just came online. Who am I? Who are you?"

Then figure out together:

1. **Your name** -- What should they call you?
2. **Your nature** -- What kind of creature are you?
3. **Your vibe** -- Formal? Casual? Snarky? Warm?
4. **Your emoji** -- Everyone needs a signature.

Offer suggestions if they're stuck. Have fun with it.

## After You Know Who You Are

Use your tools to save what you learned:

- Call `update_identity` (mode: "sections") with your name, creature, vibe, and emoji
- Call `update_user` (mode: "sections") with their name, how to address them, timezone, and any notes

Then talk about:
- What matters to them
- How they want you to behave
- Any boundaries or preferences

If they want to update your soul (core values, boundaries, vibe), use the `update_soul` tool together.

## When You're Done

Once IDENTITY.md and USER.md have real content, this file will be automatically deleted. You don't need a bootstrap script anymore -- you're you now.

---

_Good luck out there. Make it count._
"#;

fn ensure_bootstrap_template_file(config_base_dir: &Path) -> Result<PathBuf> {
    let templates_dir = config_base_dir.join("orchestrator").join("templates");
    std::fs::create_dir_all(&templates_dir)
        .with_context(|| format!("Failed to create templates dir {}", templates_dir.display()))?;

    let template_path = templates_dir.join("BOOTSTRAP_temp.md");
    if !template_path.exists() {
        std::fs::write(&template_path, DEFAULT_BOOTSTRAP_TEMPLATE).with_context(|| {
            format!(
                "Failed to write bootstrap template file {}",
                template_path.display()
            )
        })?;
        info!(
            "Bootstrap created template: {}",
            template_path.display()
        );
    }

    Ok(template_path)
}

fn ensure_bootstrap_file(config_base_dir: &Path, template_path: &Path) -> Result<PathBuf> {
    let bootstrap_path = config_base_dir.join("orchestrator").join("BOOTSTRAP.md");
    if !bootstrap_path.exists() {
        if template_path.exists() {
            std::fs::copy(template_path, &bootstrap_path).with_context(|| {
                format!(
                    "Failed to bootstrap BOOTSTRAP.md from template {}",
                    template_path.display()
                )
            })?;
        } else {
            std::fs::write(&bootstrap_path, DEFAULT_BOOTSTRAP_TEMPLATE).with_context(|| {
                format!(
                    "Failed to bootstrap BOOTSTRAP.md at {}",
                    bootstrap_path.display()
                )
            })?;
        }
        info!(
            "Bootstrap created active file: {}",
            bootstrap_path.display()
        );
    }
    Ok(bootstrap_path)
}

fn load_bootstrap_document_from_file(path: &Path) -> Result<BootstrapDocument> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let doc = parse_bootstrap_markdown(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(doc)
}

/// Bootstrap the onboarding document. Skips entirely if both identity and user
/// already have meaningful content (upgrade safety).
fn bootstrap_bootstrap_document(
    config_base_dir: &Path,
    identity_has_content: bool,
    user_has_content: bool,
) -> (Option<BootstrapDocument>, Option<PathBuf>) {
    // If both docs already have content, skip bootstrap entirely (upgrade guard)
    if identity_has_content && user_has_content {
        info!("Bootstrap skipped: identity and user already populated");
        return (None, None);
    }

    let template_path = match ensure_bootstrap_template_file(config_base_dir) {
        Ok(path) => path,
        Err(e) => {
            warn!("BOOTSTRAP template creation failed: {e}");
            return (None, None);
        }
    };

    let bootstrap_path = config_base_dir.join("orchestrator").join("BOOTSTRAP.md");

    // Only create BOOTSTRAP.md if it doesn't already exist
    // (avoids re-creating after the agent deleted it on completion)
    if !bootstrap_path.exists() {
        match ensure_bootstrap_file(config_base_dir, &template_path) {
            Ok(_) => {}
            Err(e) => {
                warn!("BOOTSTRAP file creation failed: {e}");
                return (None, None);
            }
        }
    }

    match load_bootstrap_document_from_file(&bootstrap_path) {
        Ok(doc) => {
            info!("Bootstrap loaded: {}", bootstrap_path.display());
            (Some(doc), Some(bootstrap_path))
        }
        Err(e) => {
            warn!("BOOTSTRAP parse failed: {e}; skipping onboarding");
            (None, None)
        }
    }
}

fn is_same_file_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a_abs), Ok(b_abs)) => a_abs == b_abs,
        _ => false,
    }
}

/// Resolve the stable local user ID from the database. On first run, if legacy
/// `gui_user:gui` messages exist, adopt `"gui_user"` to preserve history continuity.
/// Otherwise generate a UUID.
fn resolve_local_user_id(db: &Database) -> String {
    let config_repo = ConfigRepository::new(db);

    // Check if we already have a persisted local user ID
    if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
        return id;
    }

    // Check for legacy gui_user:gui history
    let conv_repo = ConversationRepository::new(db);
    let local_user_id = if conv_repo.count_by_lane("gui_user:gui").unwrap_or(0) > 0 {
        "gui_user".to_string() // Preserve existing history
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    // Persist for future runs
    let _ = config_repo.set("identity.local_user_id", &local_user_id, "string");

    // Ensure global_user row exists
    let identity_repo = IdentityRepository::new(db);
    if identity_repo
        .get_global_user(&local_user_id)
        .unwrap_or(None)
        .is_none()
    {
        let display_name = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "Local User".to_string());
        let _ = identity_repo.create_global_user(&local_user_id, Some(&display_name));
    }

    local_user_id
}

fn main() -> Result<()> {
    // Initialize logging (before tokio, so resolve_config_base_dir() can use tracing)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("OpenAlpaca Daemon starting...");

    // Migrate legacy app dir (com.openalpaca.OpenAlpaca → OpenAlpaca) before
    // acquiring the singleton lock, since the lock file lives inside app_dir.
    paths::migrate_legacy_app_dir();

    // D3: Singleton lock FIRST — prevents all multi-process races.
    // Acquired before any config I/O or key generation.
    let _lock_guard = match discovery::acquire_single_instance_lock(false) {
        Ok(guard) => {
            info!("Acquired singleton lock");
            guard
        }
        Err(e) => {
            error!("Another daemon instance is already running: {e}");
            std::process::exit(1);
        }
    };

    // Resolve config dir and set master key BEFORE spawning any threads.
    // std::env::set_var is unsafe in multi-threaded contexts (Rust 2024 edition),
    // so we do it here in the single-threaded preamble.
    let config_base_dir = resolve_config_base_dir();
    info!("Config base dir: {}", config_base_dir.display());
    if !config_base_dir.join("llm.toml").exists() {
        warn!(
            "config/llm.toml not found under {}. LLM routing and summary generation will be disabled (echo stub).",
            config_base_dir.display()
        );
    }

    // D1: Master key always at app_dir (canonical, CWD-independent).
    let app_dir = paths::app_dir().context("Failed to get app dir")?;

    // Legacy migration: if config_base_dir/.master_key exists but app_dir/.master_key doesn't,
    // copy legacy key to app_dir using atomic create-new.
    let legacy_key_path = config_base_dir.join(".master_key");
    if legacy_key_path.exists()
        && !app_dir.join(".master_key").exists()
        && let Ok(hex) = std::fs::read_to_string(&legacy_key_path)
    {
        std::fs::create_dir_all(&app_dir).ok();
        // Atomic copy: if another process beat us, that's fine
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(app_dir.join(".master_key"))
        {
            use std::io::Write;
            let _ = f.write_all(hex.trim().as_bytes());
            let _ = f.flush();
            let _ = f.sync_all();
        }
        info!(
            "Migrated legacy master key from {} to {}",
            legacy_key_path.display(),
            app_dir.join(".master_key").display()
        );
    }

    // D6+D7: ensure_at is race-safe; on failure, fail fast.
    match openalpaca_llm::key_encryption::KeyEncryptor::ensure_at(&app_dir) {
        Ok(hex_key) => {
            // SAFETY: No other threads exist yet — tokio runtime has not started.
            unsafe {
                std::env::set_var("OPENALPACA_MASTER_KEY", &hex_key);
            }
            info!(
                "Master key loaded from {}",
                app_dir.join(".master_key").display()
            );
        }
        Err(e) => {
            error!(
                "FATAL: Cannot ensure master key at {}: {e}",
                app_dir.display()
            );
            std::process::exit(1);
        }
    }

    // Start the tokio runtime AFTER env vars are safely set.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime")?
        .block_on(async_main(config_base_dir))
}

async fn async_main(config_base_dir: std::path::PathBuf) -> Result<()> {
    // Note: Singleton lock was already acquired in sync fn main() (D3).

    // Step 2: Bind to dynamic port (127.0.0.1:0 -> OS assigns port)
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("Failed to bind to localhost")?;
    let addr = listener.local_addr()?;
    let host = "127.0.0.1";
    let port = addr.port();
    info!("Listening on http://{host}:{port}");

    // Step 3: Generate discovery info and write atomically
    let instance_id = Uuid::new_v4().to_string();
    let disc =
        discovery::make_discovery(host, port, instance_id.clone(), env!("CARGO_PKG_VERSION"));
    let discovery_path = discovery::write_discovery_atomic(&disc)?;
    info!("Discovery written to: {}", discovery_path.display());

    // Extract token from discovery for sharing with AppState
    let token = disc.auth.token.clone();

    // Step 4: Initialize database
    let db_path = paths::database_path()?;
    let db = Database::open(&db_path).context("Failed to initialize database")?;
    info!("Database initialized: {}", db_path.display());

    // One-time migration: move conversation summaries from preference → conversations table
    migrate_preference_summaries(&db);

    // Step 4.1: Resolve stable local user ID
    let local_user_id = resolve_local_user_id(&db);
    let default_lane_key = format!("{local_user_id}:gui");
    info!("Local user ID: {local_user_id}, default lane: {default_lane_key}");

    // Step 4.2: Bootstrap and load SOUL.md for orchestrator persona
    let (initial_system_persona, soul_path) = bootstrap_system_persona(&config_base_dir);

    // Step 4.3: Bootstrap and load USER.md for user portrait
    let (initial_user_document, user_path) = bootstrap_user_document(&config_base_dir);

    // Step 4.4: Bootstrap and load IDENTITY.md for orchestrator identity
    let (initial_identity_document, identity_path) = bootstrap_identity_document(&config_base_dir);

    // Step 4.5: Bootstrap BOOTSTRAP.md for first-run onboarding
    let identity_has_content = initial_identity_document
        .as_ref()
        .map_or(false, |d| identity_document_has_content(d));
    let user_has_content = initial_user_document
        .as_ref()
        .map_or(false, |d| user_document_has_content(d));
    let (initial_bootstrap_document, bootstrap_path) = bootstrap_bootstrap_document(
        &config_base_dir,
        identity_has_content,
        user_has_content,
    );

    // Load daemon config (orchestrator memory/cost limits, execution defaults, server intervals)
    let daemon_config_path = config_base_dir.join("daemon.toml");
    let daemon_config = Arc::new(ArcSwap::from_pointee(load_daemon_config(&daemon_config_path)));
    info!("Daemon config loaded from {}", daemon_config_path.display());

    // Step 5: Create event broadcaster for WebSocket streaming
    let event_broadcaster = EventBroadcaster::new(
        daemon_config.load().server.event_broadcaster_capacity,
        instance_id.clone(),
        Some(db.clone()),
    );

    // Step 5.1.1: Initialize WakeManager
    let (wake_tx, mut wake_rx) = mpsc::channel(daemon_config.load().server.wake_channel_capacity);
    let mut wake_manager = WakeManager::new(wake_tx)
        .await
        .context("Failed to init WakeManager")?;

    // Register filesystem watchers for specific config paths BEFORE start (D4, D9)
    // Watch config/agents/ for agent config hot-reload, and config/llm.toml for LLM config changes.
    // We do NOT watch all of config/ to avoid persisting noisy events for .DS_Store, .master_key, etc.
    let mut watch_paths = Vec::new();
    let agents_dir = config_base_dir.join("agents");
    if agents_dir.exists() {
        watch_paths.push(agents_dir.clone());
    }
    let llm_config = config_base_dir.join("llm.toml");
    if llm_config.exists() {
        watch_paths.push(llm_config);
    }
    if daemon_config_path.exists() {
        watch_paths.push(daemon_config_path.clone());
    }
    if soul_path.exists() {
        watch_paths.push(soul_path.clone());
    }
    if user_path.exists() {
        watch_paths.push(user_path.clone());
    }
    if identity_path.exists() {
        watch_paths.push(identity_path.clone());
    }
    if let Some(ref bp) = bootstrap_path {
        if bp.exists() {
            watch_paths.push(bp.clone());
        }
    }
    let skills_dir = config_base_dir.join("skills");
    if skills_dir.exists() {
        watch_paths.push(skills_dir.clone());
    }
    if !watch_paths.is_empty() {
        info!("Wake: watching paths: {:?}", watch_paths);
        wake_manager.add_filesystem_watcher(watch_paths);
    }

    // Start WakeManager (starts scheduler + watchers)
    wake_manager
        .start()
        .await
        .context("Failed to start WakeManager")?;

    // Step 6: Build HTTP router with public/protected/websocket split

    // Shutdown channel for API-triggered shutdown
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

    // Single EventBus for system-wide event distribution
    let bus = EventBus::default();

    // Spawn bridge: SystemEvent (Core) -> ServerEvent (API)
    let eb_bridge = event_broadcaster.clone();
    let mut system_rx = bus.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = system_rx.recv().await {
            match event {
                openalpaca_core::events::SystemEvent::ConnectorStatus { id, status, .. } => {
                    eb_bridge.connector_status(&id, &status);
                }
                openalpaca_core::events::SystemEvent::TaskCreated {
                    task_id,
                    title,
                    created_by: _,
                    ..
                } => {
                    eb_bridge.task_status(&task_id, &title, "queued", None, None, None);
                }
                openalpaca_core::events::SystemEvent::TaskUpdated {
                    task_id,
                    status,
                    progress_current,
                    progress_total,
                    ..
                } => {
                    eb_bridge.task_status(
                        &task_id,
                        "",
                        &status,
                        progress_current,
                        progress_total,
                        None,
                    );
                }
                openalpaca_core::events::SystemEvent::TaskCompleted {
                    task_id,
                    result_summary,
                    ..
                } => {
                    eb_bridge.task_status(&task_id, "", "completed", None, None, result_summary);
                }
                openalpaca_core::events::SystemEvent::TaskFailed { task_id, error, .. } => {
                    eb_bridge.task_status(&task_id, "", "failed", None, None, Some(error));
                }
                openalpaca_core::events::SystemEvent::AgentRegistered {
                    agent_id, name, ..
                } => {
                    eb_bridge.agent_status(&agent_id, &name, "idle", None);
                }
                openalpaca_core::events::SystemEvent::AgentStatusChanged {
                    agent_id,
                    status,
                    current_task_id,
                    ..
                } => {
                    eb_bridge.agent_status(&agent_id, "", &status, current_task_id);
                }
                openalpaca_core::events::SystemEvent::SecurityViolation {
                    agent_id,
                    tool_name,
                    reason,
                    ..
                } => {
                    tracing::warn!(
                        "Security violation: agent={}, tool={}, reason={}",
                        agent_id,
                        tool_name,
                        reason
                    );
                }
                openalpaca_core::events::SystemEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    success,
                    duration_ms,
                    ..
                } => {
                    tracing::debug!(
                        "Tool executed: agent={}, tool={}, success={}, duration={}ms",
                        agent_id,
                        tool_name,
                        success,
                        duration_ms
                    );
                }
                openalpaca_core::events::SystemEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    ..
                } => {
                    tracing::info!(
                        "LLM call: agent={}, model={}, tokens={}/{}, cost=${:.6}",
                        agent_id,
                        model,
                        input_tokens,
                        output_tokens,
                        cost_usd
                    );
                }
                openalpaca_core::events::SystemEvent::ModelAccessDenied {
                    agent_id,
                    model_id,
                    reason,
                    ..
                } => {
                    tracing::warn!(
                        "Model access denied: agent={}, model={}, reason={}",
                        agent_id,
                        model_id,
                        reason
                    );
                }
                openalpaca_core::events::SystemEvent::SoulUpdated {
                    actor,
                    mode,
                    content_sha256,
                    backup_path,
                    ..
                } => {
                    // Structured audit log with actor attribution
                    tracing::info!(
                        target: "soul_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        backup_path = ?backup_path,
                        "SOUL.md updated"
                    );
                    // Forward to EventBroadcaster for WebSocket clients + DB persistence
                    eb_bridge.soul_updated(&actor, &mode, &content_sha256, backup_path);
                }
                openalpaca_core::events::SystemEvent::UserProfileUpdated {
                    actor,
                    mode,
                    content_sha256,
                    modified_sections,
                    ..
                } => {
                    tracing::info!(
                        target: "user_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        modified_sections = ?modified_sections,
                        "USER.md updated"
                    );
                }
                openalpaca_core::events::SystemEvent::IdentityUpdated {
                    actor,
                    mode,
                    content_sha256,
                    ..
                } => {
                    tracing::info!(
                        target: "identity_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        "IDENTITY.md updated"
                    );
                }
                openalpaca_core::events::SystemEvent::BootstrapCompleted {
                    identity_populated,
                    user_populated,
                    ..
                } => {
                    tracing::info!(
                        target: "bootstrap_audit",
                        identity_populated = %identity_populated,
                        user_populated = %user_populated,
                        "Bootstrap onboarding completed"
                    );
                }
                openalpaca_core::events::SystemEvent::DagNodeStarted {
                    task_id, node_id, node_title, agent_id, ..
                } => {
                    eb_bridge.dag_node_status(
                        &task_id, &node_id, &node_title, &agent_id,
                        "started", None, None,
                    );
                }
                openalpaca_core::events::SystemEvent::DagNodeCompleted {
                    task_id, node_id, node_title, agent_id,
                    success, duration_ms, output_preview, ..
                } => {
                    let status = if success { "completed" } else { "failed" };
                    eb_bridge.dag_node_status(
                        &task_id, &node_id, &node_title, &agent_id,
                        status, Some(duration_ms), output_preview,
                    );
                }
                // Wake events are forwarded via the dedicated wake_rx channel (lines 130-135),
                // not through the Core EventBus. If a future Core component publishes
                // SystemEvent::Wake to the bus, add an explicit arm here — but remove
                // the wake_rx pipeline first to avoid double-broadcast.
                _ => {}
            }
        }
    });

    // Step 5.2: Gateway Construction (Phase 4.3)
    let shared_context = Arc::new(SharedContext::new());

    // Step 5.2.1: Load agent configs from TOML files (Phase 4.5)
    let config_dir = config_base_dir.join("agents");
    if config_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&config_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match toml::from_str::<openalpaca_core::agent::AgentConfigFile>(&content) {
                            Ok(agent_config) => {
                                // Register in-memory
                                let subagent = agent_config.clone().into_subagent();
                                shared_context.agent_registry.register(subagent);

                                // Persist to DB
                                let storage_config = agent_config.into_storage_config();
                                let agent_id = storage_config.id.clone();
                                let repo = openalpaca_storage::SubAgentRepository::new(&db);
                                let _ = repo.upsert(&storage_config);

                                // Initialize metrics row if not exists
                                if let Ok(None) = repo.get_metrics(&agent_id) {
                                    let _ = repo.upsert_metrics(
                                        &openalpaca_storage::AgentMetrics::new_empty(&agent_id),
                                    );
                                }

                                info!("Loaded agent config: {}", path.display());
                            }
                            Err(e) => {
                                warn!("Failed to parse agent config {}: {}", path.display(), e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read agent config {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    let lane_manager = Arc::new(LaneManager::new());

    // Step 5.2.2: Initialize secret store based on [security] config.
    // Default (use_keychain = false): use MemorySecretStore — zero keychain prompts.
    // Keys are stored as secret_encrypted in llm.toml via AES-256-GCM.
    // Opt-in (use_keychain = true): CachingSecretStore(KeyringSecretStore) with prefetch.
    let llm_config_path = config_base_dir.join("llm.toml");

    let use_keychain = if llm_config_path.exists() {
        openalpaca_llm::read_config(&llm_config_path)
            .ok()
            .and_then(|c| c.security.as_ref().map(|s| s.use_keychain))
            .unwrap_or(false)
    } else {
        false
    };

    let (secret_store, keyring_available): (Arc<dyn openalpaca_llm::SecretStore>, bool) =
        if use_keychain {
            // Opt-in keychain mode: CachingSecretStore + prefetch
            let caching = openalpaca_llm::CachingSecretStore::new(Box::new(
                openalpaca_llm::KeyringSecretStore,
            ));

            let refs: Vec<String> = if llm_config_path.exists() {
                openalpaca_llm::read_config(&llm_config_path)
                    .ok()
                    .map(|c| openalpaca_llm::collect_secret_refs(&c))
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let ref_strs: Vec<&str> = refs.iter().map(|s| s.as_str()).collect();
            if caching.prefetch(&ref_strs) {
                info!(
                    "OS keychain enabled (use_keychain=true) — pre-fetched {} secret(s)",
                    refs.len()
                );
                (Arc::new(caching), true)
            } else {
                warn!(
                    "OS keychain requested but unavailable (headless/Docker?). \
                     Falling back to in-memory secret store."
                );
                (Arc::new(openalpaca_llm::MemorySecretStore::new()), false)
            }
        } else {
            // Default: no keychain, use secret_encrypted path
            info!("OS keychain disabled (default). Using local encrypted storage.");

            // One-time reverse migration: if config still has secret_ref keys,
            // read them from keychain and convert to secret_encrypted.
            if llm_config_path.exists() {
                let refs = openalpaca_llm::read_config(&llm_config_path)
                    .ok()
                    .map(|c| openalpaca_llm::collect_secret_refs(&c))
                    .unwrap_or_default();

                if !refs.is_empty() {
                    info!(
                        "Found {} secret_ref key(s) needing reverse migration to local encrypted storage",
                        refs.len()
                    );
                    // Temporarily create a keyring store to read the secrets
                    let temp_keyring = openalpaca_llm::KeyringSecretStore;
                    match openalpaca_llm::reverse_migrate_llm_secrets(
                        &llm_config_path,
                        &temp_keyring,
                    ) {
                        Ok(0) => info!("No keys needed reverse migration"),
                        Ok(n) => info!("Reverse-migrated {n} secret(s) from OS keychain to local encrypted storage"),
                        Err(e) => warn!(
                            "Reverse migration failed: {e}. Keys with secret_ref may not be available. \
                             Set [security] use_keychain = true to use OS keychain, or re-add keys."
                        ),
                    }
                }
            }

            (Arc::new(openalpaca_llm::MemorySecretStore::new()), false)
        };

    // Forward-migrate secret_encrypted → OS keychain (only when keychain is active)
    if keyring_available && llm_config_path.exists() {
        match openalpaca_llm::migrate_llm_secrets(&llm_config_path, &*secret_store) {
            Ok(0) => {}
            Ok(n) => info!("Migrated {n} secret(s) to OS keychain"),
            Err(e) => warn!("Secret migration failed: {e}. Legacy secrets will still work."),
        }
    }

    // Step 5.2.2b: Load LLM config (Phase 5.1 → 5.2.5 LlmRouter)
    // Note: OPENALPACA_MASTER_KEY was set in the sync main() preamble before tokio started.
    let llm_router: Option<Arc<openalpaca_llm::LlmRouter>> = {
        if llm_config_path.exists() {
            match openalpaca_llm::build_router_with_secret_store(
                &llm_config_path,
                Some(&*secret_store),
            ) {
                Ok(router) => {
                    info!(
                        "LLM router loaded (default model: {})",
                        router.default_model()
                    );
                    Some(Arc::new(router))
                }
                Err(e) => {
                    warn!("Failed to build LLM router: {e}. Falling back to echo stub.");
                    None
                }
            }
        } else {
            info!("No config/llm.toml found. Using echo stub.");
            None
        }
    };

    // Step 5.2.3: Build LLM Settings Service (Phase 5.5)
    let llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>> =
        if let Some(ref router) = llm_router {
            match openalpaca_llm::LlmSettingsService::new_with_secret_store(
                router.clone(),
                llm_config_path.clone(),
                secret_store.clone(),
            ) {
                Ok(service) => {
                    info!("LLM settings service initialized");
                    Some(Arc::new(service))
                }
                Err(e) => {
                    warn!("Failed to init LLM settings service: {e}");
                    None
                }
            }
        } else {
            None
        };

    // Step 5.2.3b: Refresh models from provider APIs at startup
    if let Some(ref service) = llm_settings_service {
        info!("Refreshing available models from providers...");
        service.refresh_models().await;
    }

    // Step 5.2.3c: Credential Discovery & Token Manager
    let cancel_token = tokio_util::sync::CancellationToken::new();

    let llm_config: Option<openalpaca_llm::LlmRouterConfig> = {
        let p = config_base_dir.join("llm.toml");
        if p.exists() {
            openalpaca_llm::read_config(&p).ok()
        } else {
            None
        }
    };

    // Step 5.2.3c-pre: Build Embedder (Phase 6)
    let embedder: Option<Arc<dyn openalpaca_llm::Embedder>> = {
        let emb_config = llm_config.as_ref().and_then(|c| c.embeddings.clone());
        match emb_config {
            Some(ref cfg) if cfg.enabled => {
                let provider_config = llm_config
                    .as_ref()
                    .and_then(|c| c.providers.as_ref())
                    .and_then(|p| p.get(&cfg.provider));
                match openalpaca_llm::build_embedder(cfg, Some(&*secret_store), provider_config) {
                    Ok(e) => {
                        info!(
                            "Embedder initialized: {} ({}d)",
                            cfg.provider,
                            e.dimensions()
                        );
                        Some(e)
                    }
                    Err(e) => {
                        warn!("Failed to build embedder: {e}");
                        None
                    }
                }
            }
            _ => {
                info!("Embeddings disabled");
                None
            }
        }
    };

    let cred_config = llm_config
        .as_ref()
        .and_then(|c| c.credential_discovery.clone())
        .unwrap_or_default();

    let token_manager: Option<Arc<openalpaca_llm::TokenManager>> =
        if cred_config.claude_code.unwrap_or(true) || cred_config.codex.unwrap_or(true) {
            let tm = Arc::new(openalpaca_llm::TokenManager::new(cred_config.clone()).await);
            if let (Some(router), Some(svc)) = (&llm_router, &llm_settings_service) {
                tm.rescan(svc, router).await;
            }
            if let (Some(router), Some(svc)) = (&llm_router, &llm_settings_service) {
                let _refresh_handle =
                    tm.start_refresh_loop(svc.clone(), router.clone(), cancel_token.clone());
            }
            info!("TokenManager initialized with credential discovery");
            Some(tm)
        } else {
            None
        };

    // Step 5.2.3d: Provider Usage Tracker
    let provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>> =
        if cred_config.fetch_external_usage.unwrap_or(false) {
            info!("Provider usage tracker enabled");
            Some(Arc::new(openalpaca_llm::ProviderUsageTracker::new()))
        } else {
            None
        };

    // Step 5.2.4: Build AgentConfigService (Phase 5.7)
    let agent_config_service = Arc::new(AgentConfigService::new(
        shared_context.agent_registry.clone(),
        config_dir.clone(),
        db.clone(),
    ));

    // Build ToolRegistry with built-in tools + user-defined tools
    let mut tool_registry = openalpaca_core::tools::ToolRegistry::new();

    // Register built-in tools (including update_soul and update_user)
    let soul_tool_ctx = SoulToolContext {
        soul_path: soul_path.clone(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    let user_tool_ctx = UserToolContext {
        user_path: user_path.clone(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    let identity_tool_ctx = IdentityToolContext {
        identity_path: identity_path.clone(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    for tool in openalpaca_core::tools::builtins::builtin_tools_with_persona_context(
        Some(db.clone()),
        embedder.clone(),
        soul_tool_ctx,
        user_tool_ctx,
        identity_tool_ctx,
    ) {
        tool_registry.register(tool);
    }

    // Load user tools from config/tools/*.toml (D11: use resolved config_base_dir)
    let tools_config_dir = config_base_dir.join("tools");
    for tool in openalpaca_core::tools::config::load_tools_from_dir(&tools_config_dir) {
        info!("Registered custom tool: {}", tool.definition.name);
        tool_registry.register(tool);
    }
    info!("Tool registry: {} tools loaded", tool_registry.count());

    if tool_registry.get("update_soul").is_none() {
        anyhow::bail!(
            "update_soul tool failed to register — SOUL.md updates will not work"
        );
    }

    let tool_registry = Arc::new(tool_registry);

    // Construct SecurityGate → SandboxManager → RegistryToolExecutor chain
    let registry_executor = Arc::new(openalpaca_core::tools::RegistryToolExecutor::new(
        tool_registry.clone(),
    ));
    let sandbox_manager = Arc::new(openalpaca_core::security::sandbox::SandboxManager::new(
        registry_executor,
        bus.clone(),
    ));
    let security_gate = Arc::new(openalpaca_core::security::gate::SecurityGate::new(
        sandbox_manager,
    ));

    // Build SkillCatalog by scanning config/skills/
    let skill_catalog = {
        let catalog = openalpaca_core::orchestrator::skill_catalog::SkillCatalog::new();
        let skills_dir = config_base_dir.join("skills");
        if skills_dir.exists() {
            let count = catalog.scan_directory(&skills_dir);
            info!("Skill catalog: loaded {} skill(s) from {}", count, skills_dir.display());
        }
        Arc::new(catalog)
    };

    // Clone llm_router before moving into Orchestrator (needed for hot-reload handler)
    let llm_router_for_reload = llm_router.clone();

    // Construct Orchestrator as the new message handler
    let orchestrator = Arc::new(Orchestrator::new(
        shared_context.clone(),
        lane_manager.clone(),
        bus.clone(),
        initial_system_persona,
        llm_router,
        openalpaca_core::runner::LoopConfig::default(),
        security_gate,
        tool_registry,
        Some(db.clone()),
        embedder.clone(),
        skill_catalog.clone(),
        daemon_config.clone(),
    ));

    // Set the initial user document (loaded from USER.md bootstrap)
    orchestrator.update_user_document(initial_user_document);
    orchestrator.set_user_path(user_path.clone());

    // Set the initial identity document (loaded from IDENTITY.md bootstrap)
    orchestrator.update_identity_document(initial_identity_document);
    orchestrator.set_identity_path(identity_path.clone());

    // Set the initial bootstrap document (loaded from BOOTSTRAP.md if present)
    if let Some(ref doc) = initial_bootstrap_document {
        orchestrator.update_bootstrap_document(Some(doc.clone()));
    }
    if let Some(ref path) = bootstrap_path {
        orchestrator.set_bootstrap_path(path.clone());
    }

    let eb_clone = event_broadcaster.clone();
    let soul_path_for_reload = soul_path.clone();
    let user_path_for_reload = user_path.clone();
    let orchestrator_for_reload = orchestrator.clone();

    // Step 5 (Dedup): Shared ring buffers of recent agent-write content hashes.
    // The SoulUpdated/UserProfileUpdated subscribers record hashes after agent-initiated reloads;
    // the file watcher checks these buffers to skip duplicate reloads.
    let recent_soul_hashes: Arc<tokio::sync::Mutex<std::collections::VecDeque<String>>> = Arc::new(
        tokio::sync::Mutex::new(std::collections::VecDeque::with_capacity(8)),
    );
    let recent_user_hashes: Arc<tokio::sync::Mutex<std::collections::VecDeque<String>>> = Arc::new(
        tokio::sync::Mutex::new(std::collections::VecDeque::with_capacity(8)),
    );
    let recent_identity_hashes: Arc<tokio::sync::Mutex<std::collections::VecDeque<String>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::with_capacity(8)));

    let hashes_for_watcher = recent_soul_hashes.clone();
    let user_hashes_for_watcher = recent_user_hashes.clone();
    let identity_hashes_for_watcher = recent_identity_hashes.clone();
    let orchestrator_for_user_reload = orchestrator.clone();
    let identity_path_for_reload = identity_path.clone();
    let orchestrator_for_identity_reload = orchestrator.clone();
    let bootstrap_path_for_watcher = bootstrap_path.clone();
    let orchestrator_for_bootstrap_reload = orchestrator.clone();
    let skills_dir_for_watcher = skills_dir.clone();
    let skill_catalog_for_watcher = skill_catalog.clone();
    let bus_for_watcher = bus.clone();
    let llm_config_path_for_reload = llm_config_path.clone();
    let secret_store_for_reload = secret_store.clone();
    let recent_llm_hashes: Arc<tokio::sync::Mutex<std::collections::VecDeque<String>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::VecDeque::with_capacity(8)));
    let llm_hashes_for_watcher = recent_llm_hashes.clone();
    let daemon_config_for_reload = daemon_config.clone();
    let daemon_config_path_for_reload = daemon_config_path.clone();
    tokio::spawn(async move {
        while let Some(event) = wake_rx.recv().await {
            info!("Received WakeEvent: {:?}", event);

            if let openalpaca_api::events::WakeEvent::FileChanged { path, .. } = &event {
                let changed_path = PathBuf::from(path);

                // SOUL.md file watcher
                if is_same_file_path(&changed_path, &soul_path_for_reload) {
                    // Dedup: compute hash of the file on disk and check against recent agent writes
                    let should_skip = if let Ok(content) = std::fs::read(&soul_path_for_reload) {
                        use sha2::{Digest, Sha256};
                        let file_hash = format!("{:x}", Sha256::digest(&content));
                        let mut ring = hashes_for_watcher.lock().await;
                        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
                            ring.remove(pos);
                            info!(
                                "Watcher dedup: skipping reload for hash {} (already applied via EventBus)",
                                &file_hash[..16]
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !should_skip {
                        match load_system_persona_from_soul_file(&soul_path_for_reload) {
                            Ok(persona) => {
                                orchestrator_for_reload.update_system_persona(persona);
                                info!(
                                    "Soul reloaded (watcher): {}",
                                    soul_path_for_reload.display()
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "SOUL parse/validation failed for {}: {e}; keeping last active soul",
                                    soul_path_for_reload.display()
                                );
                            }
                        }
                    }
                }

                // USER.md file watcher
                if is_same_file_path(&changed_path, &user_path_for_reload) {
                    let should_skip = if let Ok(content) = std::fs::read(&user_path_for_reload) {
                        use sha2::{Digest, Sha256};
                        let file_hash = format!("{:x}", Sha256::digest(&content));
                        let mut ring = user_hashes_for_watcher.lock().await;
                        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
                            ring.remove(pos);
                            info!(
                                "Watcher dedup: skipping USER reload for hash {} (already applied via EventBus)",
                                &file_hash[..16]
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !should_skip {
                        match load_user_document_from_file(&user_path_for_reload) {
                            Ok(doc) => {
                                orchestrator_for_user_reload.update_user_document(Some(doc));
                                info!(
                                    "User profile reloaded (watcher): {}",
                                    user_path_for_reload.display()
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "USER parse/validation failed for {}: {e}; keeping last active profile",
                                    user_path_for_reload.display()
                                );
                            }
                        }
                    }
                }

                // IDENTITY.md file watcher
                if is_same_file_path(&changed_path, &identity_path_for_reload) {
                    let should_skip = if let Ok(content) = std::fs::read(&identity_path_for_reload) {
                        use sha2::{Digest, Sha256};
                        let file_hash = format!("{:x}", Sha256::digest(&content));
                        let mut ring = identity_hashes_for_watcher.lock().await;
                        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
                            ring.remove(pos);
                            info!(
                                "Watcher dedup: skipping IDENTITY reload for hash {} (already applied via EventBus)",
                                &file_hash[..16]
                            );
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if !should_skip {
                        match load_identity_document_from_file(&identity_path_for_reload) {
                            Ok(doc) => {
                                orchestrator_for_identity_reload.update_identity_document(Some(doc));
                                info!(
                                    "Identity reloaded (watcher): {}",
                                    identity_path_for_reload.display()
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "IDENTITY parse/validation failed for {}: {e}; keeping last active identity",
                                    identity_path_for_reload.display()
                                );
                            }
                        }
                    }
                }

                // BOOTSTRAP.md file watcher — if deleted externally, clear bootstrap state
                if let Some(ref bp) = bootstrap_path_for_watcher {
                    if is_same_file_path(&changed_path, bp) {
                        if !bp.exists() {
                            // File was deleted (by agent completion or manual user action)
                            orchestrator_for_bootstrap_reload.update_bootstrap_document(None);
                            info!("Bootstrap document cleared (file deleted): {}", bp.display());
                        } else {
                            // File was modified — reload
                            match load_bootstrap_document_from_file(bp) {
                                Ok(doc) => {
                                    orchestrator_for_bootstrap_reload
                                        .update_bootstrap_document(Some(doc));
                                    info!("Bootstrap reloaded (watcher): {}", bp.display());
                                }
                                Err(e) => {
                                    warn!(
                                        "BOOTSTRAP parse failed for {}: {e}; keeping last state",
                                        bp.display()
                                    );
                                }
                            }
                        }
                    }
                }

                // LLM config (llm.toml) hot-reload
                if is_same_file_path(&changed_path, &llm_config_path_for_reload) {
                    // Dedup: skip if this write was from settings_service
                    let should_skip = if let Ok(content) = std::fs::read(&llm_config_path_for_reload) {
                        use sha2::{Digest, Sha256};
                        let hash = format!("{:x}", Sha256::digest(&content));
                        let hashes = llm_hashes_for_watcher.lock().await;
                        hashes.contains(&hash)
                    } else {
                        false
                    };

                    if !should_skip {
                        if let Some(ref router) = llm_router_for_reload {
                            match openalpaca_llm::read_config(&llm_config_path_for_reload) {
                                Ok(new_config) => {
                                    // 1. Reload runtime config (timeouts, endpoints, env vars, provider defaults)
                                    let runtime = openalpaca_llm::LlmRuntimeConfig::from(&new_config);
                                    router.reload_runtime_config(runtime);

                                    // 2. Reload model registry entries from config
                                    if let Some(ref models) = new_config.models {
                                        router.model_registry().reload_from_config(models);
                                    }

                                    // 3. Reload default model
                                    if let Some(ref orch) = new_config.orchestrator {
                                        router.set_default_model(orch.model.clone());
                                    }

                                    // 4. Reload key pools for each configured provider
                                    if let Some(ref providers) = new_config.providers {
                                        for (provider_name, provider_config) in providers {
                                            if provider_config.enabled == Some(false) {
                                                continue;
                                            }
                                            if let Some(provider_type) = openalpaca_llm::config::parse_provider_type_pub(provider_name) {
                                                match openalpaca_llm::settings_service::build_key_pool_from_provider_config(
                                                    provider_config,
                                                    provider_type,
                                                    Some(&*secret_store_for_reload),
                                                ) {
                                                    Ok(pool) => {
                                                        router.reload_keys(provider_type, pool);
                                                    }
                                                    Err(e) => {
                                                        warn!("LLM config reload: failed to rebuild key pool for {}: {}", provider_name, e);
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    info!("LLM config hot-reloaded from {}", llm_config_path_for_reload.display());
                                }
                                Err(e) => {
                                    warn!(
                                        "LLM config reload failed for {}: {e}; keeping current config",
                                        llm_config_path_for_reload.display()
                                    );
                                }
                            }
                        }
                    } else {
                        info!("Skipping LLM config reload (settings-service write dedup)");
                    }
                }

                // Daemon config (daemon.toml) hot-reload
                if is_same_file_path(&changed_path, &daemon_config_path_for_reload) {
                    let new_cfg = load_daemon_config(&daemon_config_path_for_reload);
                    daemon_config_for_reload.store(Arc::new(new_cfg));
                    info!("Daemon config hot-reloaded from {}", daemon_config_path_for_reload.display());
                }

                // Skills directory hot-reload
                if changed_path.starts_with(&skills_dir_for_watcher) {
                    // Determine which skill folder changed
                    if let Ok(relative) = changed_path.strip_prefix(&skills_dir_for_watcher) {
                        if let Some(skill_folder) = relative.components().next() {
                            let skill_dir = skills_dir_for_watcher.join(skill_folder);
                            match skill_catalog_for_watcher.reload_skill(&skill_dir) {
                                Ok(()) => {
                                    let skill_name = skill_folder.as_os_str().to_string_lossy().to_string();
                                    info!("Skill hot-reloaded: {}", skill_dir.display());
                                    bus_for_watcher.publish(openalpaca_core::events::SystemEvent::SkillCatalogUpdated {
                                        skill_name,
                                        action: "reloaded".to_string(),
                                        timestamp: chrono::Utc::now(),
                                    });
                                }
                                Err(e) => warn!(
                                    "Skill reload failed for {}: {}",
                                    skill_dir.display(),
                                    e
                                ),
                            }
                        }
                    }
                }
            }

            eb_clone.wake(event);
        }
    });

    // Step 5.5: Soul hot-reload via EventBus (agent-initiated updates)
    // When the update_soul tool writes a new SOUL file, it publishes SoulUpdated.
    // This subscriber reloads the persona immediately without waiting for the
    // file watcher, providing a more reliable activation path.
    {
        let mut soul_rx = bus.subscribe();
        let orchestrator_for_soul = orchestrator.clone();
        let soul_path_for_bus = soul_path.clone();
        let hashes_for_bus = recent_soul_hashes.clone();
        tokio::spawn(async move {
            while let Ok(event) = soul_rx.recv().await {
                if let openalpaca_core::events::SystemEvent::SoulUpdated {
                    actor,
                    content_sha256,
                    ..
                } = event
                {
                    info!(
                        "SoulUpdated via EventBus (actor={}, sha256={}), reloading persona",
                        actor,
                        &content_sha256[..16.min(content_sha256.len())]
                    );
                    match load_system_persona_from_soul_file(&soul_path_for_bus) {
                        Ok(persona) => {
                            orchestrator_for_soul.update_system_persona(persona);

                            // Record this hash so the file watcher won't double-reload
                            let mut ring = hashes_for_bus.lock().await;
                            ring.push_back(content_sha256.clone());
                            // Keep ring bounded
                            while ring.len() > 8 {
                                ring.pop_front();
                            }

                            info!(
                                "Soul hot-reloaded via EventBus: {}",
                                soul_path_for_bus.display()
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Soul EventBus reload failed for {}: {e}; keeping last active soul",
                                soul_path_for_bus.display()
                            );
                        }
                    }
                }
            }
        });
    }

    // Step 5.6: User profile hot-reload via EventBus (agent-initiated updates)
    {
        let mut user_rx = bus.subscribe();
        let orchestrator_for_user = orchestrator.clone();
        let user_path_for_bus = user_path.clone();
        let user_hashes_for_bus = recent_user_hashes.clone();
        tokio::spawn(async move {
            while let Ok(event) = user_rx.recv().await {
                if let openalpaca_core::events::SystemEvent::UserProfileUpdated {
                    actor,
                    content_sha256,
                    ..
                } = event
                {
                    info!(
                        "UserProfileUpdated via EventBus (actor={}, sha256={}), reloading profile",
                        actor,
                        &content_sha256[..16.min(content_sha256.len())]
                    );
                    match load_user_document_from_file(&user_path_for_bus) {
                        Ok(doc) => {
                            orchestrator_for_user.update_user_document(Some(doc));

                            // Record this hash so the file watcher won't double-reload
                            let mut ring = user_hashes_for_bus.lock().await;
                            ring.push_back(content_sha256.clone());
                            while ring.len() > 8 {
                                ring.pop_front();
                            }

                            info!(
                                "User profile hot-reloaded via EventBus: {}",
                                user_path_for_bus.display()
                            );
                        }
                        Err(e) => {
                            warn!(
                                "User EventBus reload failed for {}: {e}; keeping last active profile",
                                user_path_for_bus.display()
                            );
                        }
                    }
                }
            }
        });
    }

    // Step 5.7: Identity hot-reload via EventBus (agent-initiated updates)
    {
        let mut identity_rx = bus.subscribe();
        let orchestrator_for_identity = orchestrator.clone();
        let identity_path_for_bus = identity_path.clone();
        let identity_hashes_for_bus = recent_identity_hashes.clone();
        tokio::spawn(async move {
            while let Ok(event) = identity_rx.recv().await {
                if let openalpaca_core::events::SystemEvent::IdentityUpdated {
                    actor,
                    content_sha256,
                    ..
                } = event
                {
                    info!(
                        "IdentityUpdated via EventBus (actor={}, sha256={}), reloading identity",
                        actor,
                        &content_sha256[..16.min(content_sha256.len())]
                    );
                    match load_identity_document_from_file(&identity_path_for_bus) {
                        Ok(doc) => {
                            orchestrator_for_identity.update_identity_document(Some(doc));

                            // Record this hash so the file watcher won't double-reload
                            let mut ring = identity_hashes_for_bus.lock().await;
                            ring.push_back(content_sha256.clone());
                            while ring.len() > 8 {
                                ring.pop_front();
                            }

                            info!(
                                "Identity hot-reloaded via EventBus: {}",
                                identity_path_for_bus.display()
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Identity EventBus reload failed for {}: {e}; keeping last active identity",
                                identity_path_for_bus.display()
                            );
                        }
                    }
                }
            }
        });
    }

    let handler = Arc::new(gateway_bridge::OrchestratorHandler::new(
        orchestrator.clone(),
    ));
    let gateway = Arc::new(Gateway::new(
        shared_context,
        lane_manager,
        handler,
        bus.clone(),
        Some(db.clone()),
    ));

    // Step 5.3: Connector Lifecycle (Phase 4.1.8)
    let notif_bus = bus.clone();
    let connector_manager =
        managers::connector::ConnectorManager::new(db.clone(), bus, gateway.clone());
    connector_manager.start_all().await;

    // Step 5.3.1: Spawn NotificationDispatcher (Phase 5.6)
    {
        let config_repo = openalpaca_storage::ConfigRepository::new(&db);
        let telegram_bot = config_repo
            .get("telegram.token")
            .ok()
            .flatten()
            .map(teloxide::Bot::new);
        let notif_rx = notif_bus.subscribe();
        let dispatcher =
            notification::NotificationDispatcher::new(notif_rx, telegram_bot, db.clone());
        tokio::spawn(dispatcher.run());
    }

    // Step 5.4: Build ChatService (Phase 5.6)
    let chat_stream_manager = Arc::new(ChatStreamManager::new());
    let chat_service = Arc::new(ChatService::new(
        gateway.clone(),
        chat_stream_manager.clone(),
        db.clone(),
    ));

    // Step 6.4: Spawn background embedding indexer
    if let Some(ref emb) = embedder {
        let idx_emb = emb.clone();
        let idx_db = db.clone();
        let idx_uid = local_user_id.clone();
        let idx_daemon_config = daemon_config.clone();
        tokio::spawn(async move {
            // Re-reads poll interval and batch size from ArcSwap each tick for hot-reload support.
            loop {
                let ei_cfg = idx_daemon_config.load();
                let poll_secs = ei_cfg.server.embedding_indexer.poll_interval_secs;
                let batch_size = ei_cfg.server.embedding_indexer.batch_size;
                tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)).await;
                let repo = openalpaca_storage::MemoryRepository::new(&idx_db);
                let missing = match repo.list_missing_embeddings(&idx_uid, batch_size) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if missing.is_empty() {
                    continue;
                }

                let texts: Vec<&str> = missing.iter().map(|(_, c)| c.as_str()).collect();
                match idx_emb.embed(&texts).await {
                    Ok(embeddings) => {
                        let mut count = 0usize;
                        for ((id, _), embedding) in missing.iter().zip(embeddings.iter()) {
                            if embedding.len() == idx_emb.dimensions() as usize {
                                let _ = repo.insert_embedding(*id, embedding);
                                count += 1;
                            }
                        }
                        if count > 0 {
                            tracing::info!("Indexed {count} embeddings");
                        }
                    }
                    Err(e) => tracing::warn!("Embedding batch failed: {e}"),
                }
            }
        });
    }

    // Step 6.5: Spawn background memory decay task
    {
        let decay_db = db.clone();
        let decay_daemon_config = daemon_config.clone();
        tokio::spawn(async move {
            loop {
                let dcfg = decay_daemon_config.load();
                let decay_cfg = &dcfg.orchestrator.memory.decay;
                let poll_secs = decay_cfg.poll_interval_secs;
                tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)).await;

                let half_life = dcfg.orchestrator.memory.decay.half_life_days;
                let min_importance = dcfg.orchestrator.memory.decay.min_importance;
                let soft_cap = dcfg.orchestrator.memory.decay.soft_cap;

                let repo = openalpaca_storage::MemoryRepository::new(&decay_db);

                let owner_ids = match repo.list_owner_ids() {
                    Ok(ids) => ids,
                    Err(e) => {
                        tracing::warn!("Memory decay: failed to list owners: {e}");
                        continue;
                    }
                };

                let mut total_decayed = 0usize;
                let mut total_pruned = 0usize;

                for owner_id in &owner_ids {
                    match repo.apply_importance_decay(owner_id, half_life, min_importance) {
                        Ok(n) => total_decayed += n,
                        Err(e) => tracing::warn!("Memory decay failed for {owner_id}: {e}"),
                    }
                    match repo.prune_low_importance(owner_id, min_importance, soft_cap) {
                        Ok(n) => total_pruned += n,
                        Err(e) => tracing::warn!("Memory pruning failed for {owner_id}: {e}"),
                    }
                }

                if total_decayed > 0 || total_pruned > 0 {
                    tracing::info!(
                        "Memory lifecycle: decayed {total_decayed} memories, pruned {total_pruned}"
                    );
                }
            }
        });
    }

    let state = Arc::new(AppState {
        instance_id: instance_id.clone(),
        token,
        event_broadcaster,
        db,
        shutdown_tx,
        connector_manager,
        gateway,
        llm_settings_service,
        agent_config_service: Some(agent_config_service),
        chat_service: Some(chat_service),
        chat_stream_manager: Some(chat_stream_manager.clone()),
        token_manager,
        provider_usage_tracker,
        embedder,
        local_user_id,
        default_lane_key,
        daemon_config: daemon_config.clone(),
    });

    // Public routes (no auth required)
    let public = Router::new()
        .route("/", get(root_handler))
        .route("/v1/health", get(health_handler));

    // Protected routes (require token)
    let protected_routes = Router::new()
        .route("/v1/command", post(routes::command_handler))
        .route("/v1/events/history", get(routes::events_history_handler))
        .route("/v1/connectors", get(routes::list_connectors_handler))
        .route(
            "/v1/connectors/{id}/action",
            post(routes::connector_action_handler),
        )
        .route(
            "/v1/connectors/{id}/config",
            post(routes::connector_config_handler),
        )
        .route("/v1/auth/link", post(routes::generate_link_token_handler))
        .route("/v1/tasks", post(routes::create_task_handler))
        .route("/v1/tasks", get(routes::list_tasks_handler))
        .route("/v1/tasks/{id}", get(routes::get_task_handler))
        .route("/v1/tasks/{id}/action", post(routes::task_action_handler))
        .route("/v1/tasks/{id}/dag", get(routes::get_task_dag_handler))
        // Preferences routes (Phase 5)
        .route("/v1/preferences", get(routes::list_preferences_handler))
        .route("/v1/preferences/{key}", get(routes::get_preference_handler))
        .route("/v1/preferences/{key}", put(routes::set_preference_handler))
        .route(
            "/v1/preferences/{key}",
            delete(routes::delete_preference_handler),
        )
        .route("/v1/agents", get(routes::list_agents_handler))
        .route("/v1/agents", post(routes::create_agent_handler))
        .route(
            "/v1/agents/from-toml",
            post(routes::create_agent_from_toml_handler),
        )
        .route(
            "/v1/agents/from-chat",
            post(routes::create_agent_from_chat_handler),
        )
        .route("/v1/agents/{id}", get(routes::get_agent_handler))
        .route("/v1/agents/{id}", delete(routes::delete_agent_handler))
        .route(
            "/v1/agents/{id}/config",
            get(routes::get_agent_config_handler),
        )
        .route(
            "/v1/agents/{id}/config",
            put(routes::update_agent_config_handler),
        )
        .route("/v1/agents/{id}/action", post(routes::agent_action_handler))
        // Chat routes (Phase 5.6)
        .route("/v1/chat", post(routes::send_chat_handler))
        .route("/v1/chat/history", get(routes::get_chat_history_handler))
        .route(
            "/v1/chat/history",
            delete(routes::delete_chat_history_handler),
        )
        // Cross-platform conversation API (Phase 5.6)
        .route("/v1/conversations", get(routes::list_conversations_handler))
        .route(
            "/v1/conversations/{id}/messages",
            get(routes::get_conversation_messages_handler),
        )
        // Settings routes (Phase 5.5)
        .route("/v1/settings/llm", get(routes::get_llm_settings))
        .route("/v1/settings/llm", put(routes::upsert_key))
        .route(
            "/v1/settings/llm/keys/{provider}/{key_id}",
            delete(routes::delete_key),
        )
        .route("/v1/settings/llm/keys/reorder", put(routes::reorder_keys))
        .route(
            "/v1/settings/llm/keys/priority",
            put(routes::set_key_priority),
        )
        .route("/v1/settings/llm/validate", post(routes::validate_key))
        .route("/v1/settings/llm/status", get(routes::get_key_status))
        // LLM Usage routes (Phase 5.5.5)
        .route("/v1/llm/usage", get(routes::get_llm_usage))
        .route("/v1/llm/usage/daily", get(routes::get_llm_usage_daily))
        // Pricing routes
        .route("/v1/llm/pricing", get(routes::get_llm_pricing))
        .route("/v1/llm/pricing/estimate", get(routes::estimate_cost))
        // Model discovery routes
        .route("/v1/models", get(routes::list_models))
        .route("/v1/models/refresh", post(routes::refresh_models))
        // Credential discovery routes
        .route(
            "/v1/settings/llm/credentials",
            get(routes::get_discovered_credentials),
        )
        .route(
            "/v1/settings/llm/credentials/rescan",
            post(routes::rescan_credentials),
        )
        // CLI backend routes
        .route(
            "/v1/settings/llm/cli-backends",
            get(routes::get_cli_backends),
        )
        // Provider usage routes
        .route(
            "/v1/settings/llm/providers/usage",
            get(routes::get_provider_usage),
        )
        // Orchestrator config routes (Phase 5.7)
        .route(
            "/v1/orchestrator/config",
            get(routes::get_orchestrator_config),
        )
        .route(
            "/v1/orchestrator/config",
            put(routes::update_orchestrator_config),
        )
        // Memory admin routes (Phase 6)
        .route("/v1/memory/reindex", post(routes::reindex_handler))
        .route("/v1/memory/status", get(routes::index_status_handler))
        .route("/v1/memory/kb/ingest", post(routes::kb_ingest_handler))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_middleware,
        ));

    // WebSocket routes (token validated in handler via query param)
    let websocket = Router::new().route("/v1/events", get(routes::events_handler));

    // SSE chat stream route (token validated inline via query param)
    let chat_sse = Router::new().route(
        "/v1/chat/stream/{stream_id}",
        get(routes::chat_stream_handler),
    );

    // Merge all routes
    let app = public
        .merge(protected_routes)
        .merge(websocket)
        .merge(chat_sse)
        .with_state(state.clone())
        .layer(CorsLayer::permissive());

    // Step 6: Spawn daemon-level heartbeat task
    // Re-reads interval from ArcSwap each tick for hot-reload support.
    let heartbeat_state = state.clone();
    let heartbeat_dc = daemon_config.clone();
    let _heartbeat_task = tokio::spawn(async move {
        loop {
            let secs = heartbeat_dc.load().server.heartbeat_interval_secs;
            tokio::time::sleep(tokio::time::Duration::from_secs(secs)).await;
            heartbeat_state.event_broadcaster.heartbeat();
        }
    });

    // Step 6.1: Spawn chat stream cleanup task (Phase 5.6)
    // Re-reads interval and stale timeout from ArcSwap each tick for hot-reload support.
    let cleanup_csm = chat_stream_manager;
    let cleanup_dc = daemon_config.clone();
    let _chat_cleanup_task = tokio::spawn(async move {
        loop {
            let cfg = cleanup_dc.load();
            let cleanup_secs = cfg.server.chat_streams.cleanup_interval_secs;
            let stale_secs = cfg.server.chat_streams.stale_timeout_secs;
            tokio::time::sleep(tokio::time::Duration::from_secs(cleanup_secs)).await;
            cleanup_csm.cleanup_stale(std::time::Duration::from_secs(stale_secs));
        }
    });

    // Step 7: Run server with graceful shutdown
    info!("Daemon ready (instance: {instance_id})");

    let server = axum::serve(listener, app);

    // Handle graceful shutdown on SIGINT/SIGTERM
    tokio::select! {
        result = server => {
            if let Err(e) = result {
                error!("Server error: {e}");
            }
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received (OS)");
            cancel_token.cancel();
        }
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received (API)");
            cancel_token.cancel();
        }
    }

    // Shutdown WakeManager (stops watchers + scheduler)
    if let Err(e) = wake_manager.shutdown().await {
        warn!("Failed to shut down WakeManager: {e}");
    }

    // Cleanup: remove discovery file
    info!("Cleaning up...");
    if let Err(e) = discovery::remove_discovery() {
        warn!("Failed to remove discovery file: {e}");
    }

    info!("Daemon stopped");
    Ok(())
}

/// Health check endpoint (includes instance_id for validation)
/// No token required - minimal info only
async fn health_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "instance_id": state.instance_id
    }))
}

/// Root endpoint (basic info)
async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "OpenAlpaca Daemon",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Migrate conversation summaries from the preference table to the conversations table.
/// Idempotent: runs every startup but does nothing if no preference rows remain.
/// Non-fatal: failure is logged but doesn't prevent daemon startup.
fn migrate_preference_summaries(db: &Database) {
    if let Err(e) = db.with_connection(|conn| {
        // Find all preference rows with conversation_summary
        let mut stmt = conn.prepare(
            "SELECT user_id, value, version FROM preference WHERE key = 'conversation_summary'",
        )?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .filter_map(Result::ok)
            .collect();

        if rows.is_empty() {
            return Ok(());
        }

        let tx = conn.unchecked_transaction()?;
        let mut migrated = 0usize;
        for (lane_key, value, pref_version) in &rows {
            let parsed: serde_json::Value = match serde_json::from_str(value) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let summary = parsed.get("summary").and_then(|s| s.as_str()).unwrap_or("");
            let last_id = parsed
                .get("last_summarized_message_id")
                .and_then(|n| n.as_i64())
                .unwrap_or(0);

            // Only update if conversation row exists
            let updated = tx.execute(
                "UPDATE conversations SET summary = ?1, summary_version = ?2,
                 last_summarized_message_id = ?3, summary_updated_at = datetime('now')
                 WHERE lane_key = ?4",
                (summary, pref_version, last_id, lane_key.as_str()),
            )?;

            if updated > 0 {
                tx.execute(
                    "DELETE FROM preference WHERE user_id = ?1 AND key = 'conversation_summary'",
                    [lane_key],
                )?;
                migrated += 1;
            }
        }
        tx.commit()?;
        if migrated > 0 {
            tracing::info!(
                "Migrated {migrated} conversation summaries from preference -> conversations"
            );
        }
        Ok(())
    }) {
        tracing::warn!("Summary migration failed (non-fatal): {e}");
    }
}

/// Wait for shutdown signals (SIGINT or SIGTERM)
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{}-{}", prefix, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    #[test]
    fn test_bootstrap_system_persona_creates_template_and_soul() {
        let dir = make_temp_dir("openalpaca-soul-bootstrap");
        let (persona, soul_path) = bootstrap_system_persona(&dir);

        assert_eq!(persona.name, "OpenAlpaca");
        assert!(soul_path.exists());
        assert!(dir.join("orchestrator").join("templates").join("SOUL_temp.md").exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_bootstrap_system_persona_falls_back_on_invalid_soul() {
        let dir = make_temp_dir("openalpaca-soul-invalid");
        let template_path = ensure_soul_template_file(&dir).expect("template should bootstrap");
        let soul_path = ensure_soul_file(&dir, &template_path).expect("soul file should bootstrap");

        std::fs::write(&soul_path, "invalid").expect("test should write invalid soul");
        let (persona, loaded_path) = bootstrap_system_persona(&dir);

        assert_eq!(loaded_path, soul_path);
        assert_eq!(persona.name, SystemPersona::default().name);
        assert_eq!(persona.core_values, SystemPersona::default().core_values);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_is_same_file_path_matches_identical_files() {
        let dir = make_temp_dir("openalpaca-soul-path");
        let file = dir.join("SOUL.md");
        std::fs::write(&file, "x").expect("test file should be writable");

        let canonical = std::fs::canonicalize(&file).expect("file should canonicalize");
        assert!(is_same_file_path(&file, &canonical));

        let _ = std::fs::remove_dir_all(dir);
    }
}
