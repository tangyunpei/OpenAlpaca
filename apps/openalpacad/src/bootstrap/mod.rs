use anyhow::{Context, Result};
use openalpaca_core::middleware::{
    bootstrap::{BootstrapDocument, parse_bootstrap_markdown},
    identity::{IdentityDocument, parse_identity_markdown},
    prompt::SystemPersona,
    soul::{parse_soul_markdown, soul_to_system_persona},
    user::{UserDocument, parse_user_markdown},
};
use openalpaca_storage::{ConfigRepository, ConversationRepository, Database, IdentityRepository};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Config directory resolution
// ---------------------------------------------------------------------------

/// Resolve the config base directory.
///
/// Priority order:
/// 1. `OPENALPACA_CONFIG_DIR` env var (explicit override, e.g. set by Tauri)
/// 2. Walk upward from `current_exe()` looking for a parent that contains `config/llm.toml`
///    (handles `target/debug/openalpacad` in dev builds)
/// 3. Walk upward from `current_dir()` looking for the same sentinel
/// 4. Fallback: `current_dir()/config`
pub fn resolve_config_base_dir() -> PathBuf {
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
    warn!(
        "Config directory not found via OPENALPACA_CONFIG_DIR, exe path, or CWD walk; \
         falling back to $CWD/config which may not exist"
    );
    std::env::current_dir().unwrap_or_default().join("config")
}

// ---------------------------------------------------------------------------
// Config file seeding (first-run / fresh install)
// ---------------------------------------------------------------------------

const DEFAULT_LLM_TOML: &str =
    include_str!("../../../../scripts/release/templates/config/llm.toml");
const DEFAULT_DAEMON_TOML: &str =
    include_str!("../../../../scripts/release/templates/config/daemon.toml");

/// Seed default configuration files if they don't exist yet.
///
/// Called after `resolve_config_base_dir()` so that a fresh install (or GUI
/// launching the daemon before `install.sh` runs) gets working defaults
/// instead of an empty config directory.
pub fn seed_default_configs(config_dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(config_dir) {
        warn!("Cannot create config dir {}: {e}", config_dir.display());
        return;
    }

    let llm_path = config_dir.join("llm.toml");
    if !llm_path.exists() {
        match std::fs::write(&llm_path, DEFAULT_LLM_TOML) {
            Ok(()) => info!("Seeded default config: {}", llm_path.display()),
            Err(e) => warn!("Failed to seed {}: {e}", llm_path.display()),
        }
    }

    let daemon_path = config_dir.join("daemon.toml");
    if !daemon_path.exists() {
        match std::fs::write(&daemon_path, DEFAULT_DAEMON_TOML) {
            Ok(()) => info!("Seeded default config: {}", daemon_path.display()),
            Err(e) => warn!("Failed to seed {}: {e}", daemon_path.display()),
        }
    }
}

// ---------------------------------------------------------------------------
// SOUL.md bootstrap
// ---------------------------------------------------------------------------

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

pub fn load_system_persona_from_soul_file(soul_path: &Path) -> Result<SystemPersona> {
    let content = std::fs::read_to_string(soul_path)
        .with_context(|| format!("Failed to read {}", soul_path.display()))?;
    let soul_doc = parse_soul_markdown(&content)
        .with_context(|| format!("Failed to parse {}", soul_path.display()))?;
    Ok(soul_to_system_persona(&soul_doc))
}

pub fn bootstrap_system_persona(config_base_dir: &Path) -> (SystemPersona, PathBuf) {
    let template_path = match ensure_soul_template_file(config_base_dir) {
        Ok(path) => path,
        Err(e) => {
            warn!("SOUL template bootstrap failed: {e}");
            config_base_dir
                .join("orchestrator")
                .join("templates")
                .join("SOUL_temp.md")
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

pub fn load_user_document_from_file(user_path: &Path) -> Result<UserDocument> {
    let content = std::fs::read_to_string(user_path)
        .with_context(|| format!("Failed to read {}", user_path.display()))?;
    let doc = parse_user_markdown(&content)
        .with_context(|| format!("Failed to parse {}", user_path.display()))?;
    Ok(doc)
}

pub fn bootstrap_user_document(config_base_dir: &Path) -> (Option<UserDocument>, PathBuf) {
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

pub fn load_identity_document_from_file(identity_path: &Path) -> Result<IdentityDocument> {
    let content = std::fs::read_to_string(identity_path)
        .with_context(|| format!("Failed to read {}", identity_path.display()))?;
    let doc = parse_identity_markdown(&content)
        .with_context(|| format!("Failed to parse {}", identity_path.display()))?;
    Ok(doc)
}

pub fn bootstrap_identity_document(config_base_dir: &Path) -> (Option<IdentityDocument>, PathBuf) {
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

- Call `update_persona` (target: "identity", mode: "sections") with your name, creature, vibe, and emoji
- Call `update_persona` (target: "user", mode: "sections") to save what you learned about them. The sections object accepts:
  - `identity`: key-value pairs for Name, What to call them, Pronouns, Timezone, How to address
  - `communication_style`: how they like to communicate (terse vs verbose, formal vs casual, etc.)
  - `expertise`: technical background, domains, skill level
  - `projects`: current projects, tools, stack preferences
  - `preferences`: likes, dislikes, formatting preferences
  - `notes`: anything else worth remembering
  You can call `update_persona` (target: "user") multiple times as you learn more. Fill in every section you have information for.

Then keep talking and learn more about:
- What matters to them
- How they want you to behave
- Any boundaries or preferences
- Their technical background and current projects

As you learn more, call `update_persona` (target: "user") again to fill in additional sections.

If they want to update your soul (core values, boundaries, vibe), use the `update_persona` tool (target: "soul") together.

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
        info!("Bootstrap created template: {}", template_path.display());
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

pub fn load_bootstrap_document_from_file(path: &Path) -> Result<BootstrapDocument> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let doc = parse_bootstrap_markdown(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(doc)
}

/// Bootstrap the onboarding document. Skips entirely if both identity and user
/// already have meaningful content (upgrade safety).
pub fn bootstrap_bootstrap_document(
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

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

pub fn is_same_file_path(a: &Path, b: &Path) -> bool {
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
pub fn resolve_local_user_id(db: &Database) -> String {
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

/// Migrate conversation summaries from the preference table to the conversations table.
/// Idempotent: runs every startup but does nothing if no preference rows remain.
/// Non-fatal: failure is logged but doesn't prevent daemon startup.
pub fn migrate_preference_summaries(db: &Database) {
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

#[cfg(test)]
mod tests;
