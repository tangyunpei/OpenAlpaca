//! Scheduled skills: wires skill frontmatter (`invoke.cron` /
//! `invoke.mode = "scheduled"`) to the wake-module cron scheduler.
//!
//! Registration: [`sync_all`] runs at daemon boot (after the skill catalog
//! loads) and again whenever `daemon.toml` hot-reloads; [`resync_skill`] runs
//! on per-skill hot-reload. Jobs use the stable id `skill:{skill_id}` so
//! re-syncs deregister before re-registering (the scheduler rejects
//! duplicate ids by design).
//!
//! Consumption: when `WakeEvent::Timer` fires for a `skill:{id}` job, the
//! daemon's wake loop calls [`spawn_timer_turn`], which injects
//! `/{slash-command}` through `Gateway::handle_event` — the same front door
//! as user messages — so the turn hits the deterministic slash-command tier
//! and completion flows through normal channels (persistence, events,
//! notifications).
//!
//! Identity: turns run as `Principal::User {{ local_user_id }}` on the
//! dedicated lane `"{local_user_id}:scheduled"` so the local user's memory
//! and preferences apply, results are readable in chat history, and the
//! NotificationDispatcher can push them cross-channel.
//!
//! Kill switch: `[orchestrator.routing] scheduled_skills_enabled` in
//! `daemon.toml` (default true) gates both registration and timer handling.

use openalpaca_api::events::EventSource;
use openalpaca_core::gateway::{Gateway, GatewayRequest};
use openalpaca_core::middleware::skill::SkillFrontmatter;
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::security::policy::{Principal, Scope};
use openalpaca_wake::{ScheduledTask, WakeManager};
use std::sync::Arc;
use tracing::{info, warn};

/// Prefix for wake-scheduler job ids owned by the scheduled-skills bridge.
pub const SKILL_JOB_PREFIX: &str = "skill:";

/// Tag attached to skill cron jobs (visible in `list_jobs`).
const SKILL_JOB_TAG: &str = "scheduled-skill";

/// Stable job id for a skill's cron job.
pub fn skill_job_id(skill_id: &str) -> String {
    format!("{SKILL_JOB_PREFIX}{skill_id}")
}

/// Extract the skill id from a `skill:{id}` job id, if it is one of ours.
pub fn parse_skill_job_id(job_id: &str) -> Option<&str> {
    job_id.strip_prefix(SKILL_JOB_PREFIX)
}

/// Register cron jobs for every catalog skill with a cron expression,
/// after removing all existing `skill:*` jobs (idempotent full re-sync).
///
/// When `enabled` is false only the removal half runs, leaving zero skill
/// jobs registered. Returns the number of jobs registered.
pub async fn sync_all(wake: &WakeManager, catalog: &SkillCatalog, enabled: bool) -> usize {
    // Deregister all skill jobs first so a re-sync never duplicates and
    // skills that lost their cron expression stop firing.
    for job in wake.list_jobs().await {
        if job.id.starts_with(SKILL_JOB_PREFIX)
            && let Err(e) = wake.remove_job(&job.id).await
        {
            warn!(job_id = %job.id, "Scheduled skills: failed to remove job during re-sync: {e}");
        }
    }

    if !enabled {
        info!("Scheduled skills disabled (orchestrator.routing.scheduled_skills_enabled=false)");
        return 0;
    }

    let mut count = 0;
    for (skill_id, entry) in catalog.entries_snapshot() {
        if register_skill(wake, &skill_id, &entry.frontmatter).await {
            count += 1;
        }
    }
    if count > 0 {
        info!("Scheduled skills: {count} cron job(s) registered");
    }
    count
}

/// Re-sync one skill's cron job after a hot-reload: deregister the old job,
/// then re-register from the current catalog entry (if the skill still
/// exists, still has a cron expression, and the kill switch is on).
pub async fn resync_skill(wake: &WakeManager, catalog: &SkillCatalog, skill_id: &str, enabled: bool) {
    let job_id = skill_job_id(skill_id);
    // Best-effort removal — the job may not exist (skill had no cron before).
    let _ = wake.remove_job(&job_id).await;

    if !enabled {
        return;
    }
    if let Some(entry) = catalog.get(skill_id) {
        register_skill(wake, skill_id, &entry.frontmatter).await;
    }
}

/// Register a single skill's cron job. Returns true when a job was added.
///
/// Invalid cron expressions are rejected by the scheduler at job-creation
/// time; they are logged and skipped, never fatal.
async fn register_skill(wake: &WakeManager, skill_id: &str, fm: &SkillFrontmatter) -> bool {
    let Some(cron) = fm.invoke.cron.as_deref() else {
        if fm.invoke.mode == "scheduled" {
            warn!(
                skill = %skill_id,
                "Skill has invoke.mode=\"scheduled\" but no invoke.cron expression — not scheduled"
            );
        }
        return false;
    };

    let task = ScheduledTask {
        id: skill_job_id(skill_id),
        cron: cron.to_string(),
        tag: SKILL_JOB_TAG.to_string(),
        job_uuid: None,
    };
    match wake.schedule_cron(task).await {
        Ok(_) => {
            info!(skill = %skill_id, cron = %cron, "Scheduled skill cron job registered");
            true
        }
        Err(e) => {
            warn!(
                skill = %skill_id,
                cron = %cron,
                "Skipping scheduled skill (invalid cron expression or scheduler error): {e}"
            );
            false
        }
    }
}

/// Handle a `WakeEvent::Timer` fire for a `skill:{id}` job: inject the
/// skill's slash command as a fresh turn through the gateway.
///
/// The turn is spawned so a long-running skill never stalls the wake loop
/// (which also serves config hot-reload).
pub fn spawn_timer_turn(
    gateway: Arc<Gateway>,
    catalog: Arc<SkillCatalog>,
    local_user_id: String,
    skill_id: String,
) {
    let Some(entry) = catalog.get(&skill_id) else {
        warn!(
            skill = %skill_id,
            "Scheduled skill timer fired but the skill is no longer in the catalog — ignoring"
        );
        return;
    };
    // Deterministic tier: /{slash command}; skills without an explicit slash
    // command resolve via the catalog's skill-ID fallback.
    let command = entry
        .frontmatter
        .effective_slash_command()
        .unwrap_or_else(|| skill_id.clone());
    let content = format!("/{command}");
    let lane_key = format!("{local_user_id}:scheduled");

    tokio::spawn(async move {
        info!(skill = %skill_id, lane = %lane_key, %content, "Scheduled skill fired — injecting turn");
        let response = gateway
            .handle_event(GatewayRequest {
                source: EventSource::Internal,
                content,
                attachments: Vec::new(),
                principal: Principal::User {
                    global_id: local_user_id,
                },
                scope: Scope::Global,
                workspace_path: None,
                stream_id: None,
                // Land on the dedicated scheduled lane instead of the
                // "{user}:internal" lane EventSource::Internal would derive.
                lane_override: Some(lane_key),
            })
            .await;
        if response.is_error {
            warn!(skill = %skill_id, "Scheduled skill turn failed: {}", response.content);
        } else {
            info!(skill = %skill_id, "Scheduled skill turn completed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::gateway::{HandleResult, MessageHandler};
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::middleware::skill::SkillScope;
    use std::io::Write;
    use std::path::Path;
    use std::sync::Mutex;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    fn create_skill_dir(parent: &Path, name: &str, skill_md: &str) {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("SKILL.md")).unwrap();
        f.write_all(skill_md.as_bytes()).unwrap();
    }

    const CRON_SKILL: &str = r#"---
name: "Daily Digest"
description: "Summarize the day"
invoke:
  mode: scheduled
  slash: "/digest"
  cron: "0 0 9 * * *"
---

## Instructions

Summarize.
"#;

    const BAD_CRON_SKILL: &str = r#"---
name: "Broken Schedule"
description: "Has an invalid cron expression"
invoke:
  mode: scheduled
  cron: "not a cron"
---

Body.
"#;

    const PLAIN_SKILL: &str = r#"---
name: "Plain"
description: "No schedule"
command: "plain"
---

Body.
"#;

    async fn wake_manager() -> WakeManager {
        let (tx, _rx) = mpsc::channel(16);
        let wm = WakeManager::new(tx).await.unwrap();
        wm.start().await.unwrap();
        wm
    }

    fn catalog_from(dir: &Path) -> SkillCatalog {
        let catalog = SkillCatalog::new();
        catalog.scan_directory(dir, SkillScope::Project);
        catalog
    }

    #[tokio::test]
    async fn test_sync_all_registers_cron_skills_and_skips_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        create_skill_dir(tmp.path(), "daily-digest", CRON_SKILL);
        create_skill_dir(tmp.path(), "broken-schedule", BAD_CRON_SKILL);
        create_skill_dir(tmp.path(), "plain", PLAIN_SKILL);

        let catalog = catalog_from(tmp.path());
        let wake = wake_manager().await;

        let count = sync_all(&wake, &catalog, true).await;
        assert_eq!(count, 1, "only the valid cron skill registers");

        let jobs = wake.list_jobs().await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "skill:daily-digest");
        assert_eq!(jobs[0].cron, "0 0 9 * * *");
        assert_eq!(jobs[0].tag, "scheduled-skill");
        wake.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_sync_all_disabled_removes_and_registers_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        create_skill_dir(tmp.path(), "daily-digest", CRON_SKILL);
        let catalog = catalog_from(tmp.path());
        let wake = wake_manager().await;

        assert_eq!(sync_all(&wake, &catalog, true).await, 1);
        // Kill switch off: existing skill jobs are removed, none registered.
        assert_eq!(sync_all(&wake, &catalog, false).await, 0);
        assert!(wake.list_jobs().await.is_empty());
        wake.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_sync_all_and_resync_do_not_duplicate() {
        let tmp = tempfile::tempdir().unwrap();
        create_skill_dir(tmp.path(), "daily-digest", CRON_SKILL);
        let catalog = catalog_from(tmp.path());
        let wake = wake_manager().await;

        assert_eq!(sync_all(&wake, &catalog, true).await, 1);
        // Full re-sync (daemon.toml reload) — still exactly one job.
        assert_eq!(sync_all(&wake, &catalog, true).await, 1);
        assert_eq!(wake.list_jobs().await.len(), 1);

        // Per-skill re-sync (skill hot-reload) — still exactly one job.
        resync_skill(&wake, &catalog, "daily-digest", true).await;
        assert_eq!(wake.list_jobs().await.len(), 1);

        // Skill removed from catalog — re-sync deregisters its job.
        catalog.remove("daily-digest");
        resync_skill(&wake, &catalog, "daily-digest", true).await;
        assert!(wake.list_jobs().await.is_empty());
        wake.shutdown().await.unwrap();
    }

    /// (source, content, principal, lane_key) captured per handled turn.
    type RecordedCall = (String, String, Principal, String);

    struct StubHandler {
        calls: Arc<Mutex<Vec<RecordedCall>>>,
    }

    #[async_trait]
    impl MessageHandler for StubHandler {
        async fn handle(
            &self,
            _request_id: Uuid,
            source: String,
            content: String,
            principal: Principal,
            _scope: Scope,
            lane_key: String,
            _workspace_path: Option<String>,
            _stream_id: Option<String>,
        ) -> Result<HandleResult, String> {
            self.calls
                .lock()
                .unwrap()
                .push((source, content, principal, lane_key));
            Ok(HandleResult::text("ack".to_string()))
        }
    }

    #[tokio::test]
    async fn test_timer_turn_reaches_gateway_as_slash_command() {
        let tmp = tempfile::tempdir().unwrap();
        create_skill_dir(tmp.path(), "daily-digest", CRON_SKILL);
        let catalog = Arc::new(catalog_from(tmp.path()));

        let calls = Arc::new(Mutex::new(Vec::new()));
        let gateway = Arc::new(Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler {
                calls: calls.clone(),
            }),
            EventBus::default(),
            None,
        ));

        spawn_timer_turn(
            gateway,
            catalog,
            "junpei".to_string(),
            "daily-digest".to_string(),
        );

        for _ in 0..200 {
            if !calls.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "handler should have been called once");
        let (source, content, principal, lane_key) = &calls[0];
        assert_eq!(source, "internal");
        assert_eq!(content, "/digest");
        assert_eq!(
            principal,
            &Principal::User {
                global_id: "junpei".to_string()
            }
        );
        assert_eq!(lane_key, "junpei:scheduled");
    }

    #[tokio::test]
    async fn test_timer_turn_for_unknown_skill_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = Arc::new(catalog_from(tmp.path()));

        let calls = Arc::new(Mutex::new(Vec::new()));
        let gateway = Arc::new(Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            Arc::new(StubHandler {
                calls: calls.clone(),
            }),
            EventBus::default(),
            None,
        ));

        spawn_timer_turn(gateway, catalog, "junpei".to_string(), "gone".to_string());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_job_id_roundtrip() {
        assert_eq!(skill_job_id("digest"), "skill:digest");
        assert_eq!(parse_skill_job_id("skill:digest"), Some("digest"));
        assert_eq!(parse_skill_job_id("other:digest"), None);
    }
}
