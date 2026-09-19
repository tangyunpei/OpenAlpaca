//! First-boot content: the agent templates, skills and tool config the daemon
//! carries in its own binary (L9).
//!
//! `seed_default_configs` has always written `llm.toml`, `daemon.toml` and
//! `mcp.toml` on a first boot, and the release packager stages exactly those
//! three files — so an installed daemon started with an empty config directory
//! had **no agent templates and no skills at all**. `load_agent_templates`
//! returns `Ok(())` when `config/agents` is missing, the skill catalog loads
//! nothing, and the first workflow request dies in the lead dispatcher. The
//! only cure was to copy the repository's `config/` by hand, which no manual
//! describes.
//!
//! So the content ships inside the binary, the same way the three config files
//! do, and lands the same way: **only when the directory is absent**, never
//! over a file that exists. A directory the owner has curated — even down to
//! zero files — is theirs; the daemon fills a gap, it does not restore a
//! default.

use std::path::Path;
use tracing::{info, warn};

/// One embedded file: where it goes under the config dir, and what is in it.
struct SeedFile {
    /// Path relative to the config base dir, always `/`-separated.
    path: &'static str,
    contents: &'static str,
}

/// A directory the daemon can seed, and everything that belongs in it.
struct SeedDir {
    /// Directory name under the config base dir.
    name: &'static str,
    /// What it is, for the log line.
    label: &'static str,
    files: &'static [SeedFile],
}

macro_rules! seed {
    ($path:literal) => {
        SeedFile {
            path: $path,
            contents: include_str!(concat!("../../../../config/", $path)),
        }
    };
}

/// The nine shipped agent templates. Without at least one carrying the
/// `orchestration` capability nothing can act as Lead Agent.
const AGENTS: &[SeedFile] = &[
    seed!("agents/code_agent.md"),
    seed!("agents/explore_agent.md"),
    seed!("agents/general_agent.md"),
    seed!("agents/lead_agent.md"),
    seed!("agents/planning_agent.md"),
    seed!("agents/research_agent.md"),
    seed!("agents/review_agent.md"),
    seed!("agents/system_agent.md"),
    seed!("agents/writing_agent.md"),
];

/// The four shipped skills. `create-skill` carries helper scripts, which have
/// to land executable or the skill cannot run them.
const SKILLS: &[SeedFile] = &[
    seed!("skills/code-review/SKILL.md"),
    seed!("skills/commit-message/SKILL.md"),
    seed!("skills/create-skill/SKILL.md"),
    seed!("skills/create-skill/scripts/check_exists.sh"),
    seed!("skills/create-skill/scripts/list_skills.sh"),
    seed!("skills/create-skill/scripts/scaffold.sh"),
    seed!("skills/create-skill/scripts/write_skill_md.sh"),
    seed!("skills/explain-code/SKILL.md"),
];

/// The custom-tool example. Fully commented, like `mcp.toml`: it is the
/// documentation of the format as much as it is a file.
const TOOLS: &[SeedFile] = &[seed!("tools/example.toml")];

const SEED_DIRS: &[SeedDir] = &[
    SeedDir {
        name: "agents",
        label: "agent template",
        files: AGENTS,
    },
    SeedDir {
        name: "skills",
        label: "skill",
        files: SKILLS,
    },
    SeedDir {
        name: "tools",
        label: "tool config",
        files: TOOLS,
    },
];

/// Seed `config/agents`, `config/skills` and `config/tools` when they are
/// absent.
///
/// Called from [`super::seed_default_configs`], before anything reads them.
/// Each directory is decided on its own: an install that has agents but no
/// skills gets skills.
pub(super) fn seed_default_content(config_dir: &Path) {
    for dir in SEED_DIRS {
        let root = config_dir.join(dir.name);
        if root.exists() {
            continue;
        }

        let mut written = 0usize;
        for file in dir.files {
            let target = config_dir.join(file.path);
            // Belt and braces: the directory was absent a moment ago, so this
            // can only fire in a race — and even then the file on disk wins.
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                warn!("Failed to seed {}: {e}", target.display());
                continue;
            }
            match std::fs::write(&target, file.contents) {
                Ok(()) => {
                    make_executable_if_script(&target);
                    written += 1;
                }
                Err(e) => warn!("Failed to seed {}: {e}", target.display()),
            }
        }

        if written > 0 {
            info!(
                "Seeded {written} default {} file(s): {}",
                dir.label,
                root.display()
            );
        }
    }
}

/// A skill's helper scripts are executed, so they land with the mode git
/// records for them. `include_str!` carries bytes, never permissions.
#[cfg(unix)]
fn make_executable_if_script(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.extension().and_then(|e| e.to_str()) != Some("sh") {
        return;
    }
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)) {
        warn!("Seeded {} but could not make it executable: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn make_executable_if_script(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::AGENTS;
    use openalpaca_core::orchestrator::MAIN_LOOP_AGENT_ID;

    /// **T4.** The main loop's tool calls are attributed to `orchestrator`, so
    /// that a confirmation card can name who is asking instead of reading
    /// "unknown is blocked on this". That only works while the name belongs to
    /// nobody else: a shipped template with the same id would make the
    /// capability log ambiguous and would make the GUI call a real agent by
    /// the assistant's name. The templates are embedded here, so this is where
    /// the collision would be caught.
    #[test]
    fn no_shipped_agent_template_claims_the_main_loop_s_id() {
        let mut ids = Vec::new();
        for file in AGENTS {
            let id = file
                .contents
                .lines()
                .find_map(|line| line.trim().strip_prefix("id:"))
                .map(|value| value.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| panic!("{} has no `id:` in its frontmatter", file.path));
            ids.push(id);
        }

        assert_eq!(ids.len(), 9, "the nine shipped templates: {ids:?}");
        assert!(
            !ids.iter().any(|id| id == MAIN_LOOP_AGENT_ID),
            "`{MAIN_LOOP_AGENT_ID}` is the main loop's own id and must not \
             also be a template's: {ids:?}"
        );
        // And it is a name, not the absence of one.
        assert_ne!(MAIN_LOOP_AGENT_ID, "unknown");
        assert!(!MAIN_LOOP_AGENT_ID.is_empty());
    }
}
