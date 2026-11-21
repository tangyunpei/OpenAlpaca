//! First-boot content: the tool config the daemon carries in its own binary
//! (L9), and the empty `agents/` and `skills/` the person's own templates and
//! skills go in.
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
//!
//! The agent templates and the skills are no longer seeded (subagent design
//! §2.5 S-1): they are a bundled layer the daemon takes from its binary at
//! every start (`openalpaca_core::bundled`) — the skills written into
//! `state/cache/bundled-skills/`, where they run — and `agents/` and
//! `skills/` are created empty, when absent, for the person's own and
//! overriding files. A seeded copy an earlier build wrote stays where it is;
//! an unedited one is set aside at load.

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

/// The custom-tool example. Fully commented, like `mcp.toml`: it is the
/// documentation of the format as much as it is a file.
const TOOLS: &[SeedFile] = &[seed!("tools/example.toml")];

const SEED_DIRS: &[SeedDir] = &[SeedDir {
    name: "tools",
    label: "tool config",
    files: TOOLS,
}];

/// The directories created empty when absent, and never filled: `agents/`
/// and `skills/` hold the person's own templates and skills and their
/// overrides of the bundled ones (S-1). Created so the watcher has the paths
/// from the first start (L-6), the template routes a place to write, and
/// `create-skill` the directory it writes into.
const EMPTY_DIRS: &[&str] = &["agents", "skills"];

/// Create `config/agents` and `config/skills` empty, and seed `config/tools`,
/// when they are absent.
///
/// Called from [`super::seed_default_configs`], before anything reads them.
/// Each directory is decided on its own: an install that has skills but no
/// tools gets tools.
pub(super) fn seed_default_content(config_dir: &Path) {
    for name in EMPTY_DIRS {
        let root = config_dir.join(name);
        if root.exists() {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&root) {
            warn!("Failed to create {}: {e}", root.display());
        }
    }

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
                Ok(()) => written += 1,
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
