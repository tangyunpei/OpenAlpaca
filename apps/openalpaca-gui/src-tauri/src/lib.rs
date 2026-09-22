//! OpenAlpaca GUI - Tauri Backend
//!
//! Provides Tauri commands for:
//! - Connecting to the daemon via discovery.json
//! - Ensuring the daemon is running (spawning if needed)

use openalpaca_storage::discovery::{self, ConnectionInfo};
use openalpaca_storage::store;
use std::process::Command;
use std::time::Duration;

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get connection info from discovery.json.
/// Returns error if daemon is not running or discovery is invalid.
#[tauri::command]
fn get_connection_info() -> Result<ConnectionInfo, String> {
    let Some(d) = discovery::read_discovery().map_err(|e| e.to_string())? else {
        return Err("Discovery not found: daemon may not be running".into());
    };

    discovery::ensure_not_expired(&d).map_err(|e| e.to_string())?;

    Ok(ConnectionInfo::from(&d))
}

/// Ensure the daemon is running, spawning it if necessary.
/// Returns connection info once daemon is ready.
#[tauri::command]
async fn ensure_daemon_running() -> Result<ConnectionInfo, String> {
    // Step 1: Check if a daemon is already running AND actually alive. A stale
    // discovery.json (left by a crashed/killed daemon or a reboot) would
    // otherwise be trusted forever, so the GUI would never respawn. Liveness is
    // the authoritative signal — a live daemon is used even if the file's own
    // 24h expiry has lapsed (long uptime).
    if let Ok(Some(d)) = discovery::read_discovery()
        && daemon_is_alive(&d)
    {
        return Ok(ConnectionInfo::from(&d));
    }

    // Step 2: Spawn the daemon process
    spawn_daemon().map_err(|e| format!("Failed to spawn daemon: {e}"))?;

    // Step 3: Wait for the new daemon to appear AND accept connections.
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;

        if let Ok(Some(d)) = discovery::read_discovery()
            && daemon_is_alive(&d)
        {
            return Ok(ConnectionInfo::from(&d));
        }
    }

    Err("Daemon did not become ready within timeout".into())
}

/// Liveness probe: can we open a TCP connection to the daemon's listen address?
/// A stale discovery.json (dead daemon) fails this, so callers respawn instead
/// of returning a dead endpoint.
fn daemon_is_alive(d: &discovery::Discovery) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(addrs) = (d.listen.host.as_str(), d.listen.port).to_socket_addrs() else {
        return false;
    };
    for addr in addrs {
        if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
    }
    false
}

/// Spawn the daemon as a detached background process.
fn spawn_daemon() -> anyhow::Result<()> {
    // Find the daemon executable
    // In development: should be in the same target directory
    // In production: should be bundled with the app

    let daemon_name = if cfg!(windows) {
        "openalpacad.exe"
    } else {
        "openalpacad"
    };

    // Try to find daemon in common locations
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(std::path::Path::new("."));

    let daemon_path = exe_dir.join(daemon_name);

    let path_to_use = if daemon_path.exists() {
        daemon_path
    } else {
        #[cfg(debug_assertions)]
        {
            // Dev convenience: fall back to PATH
            std::path::PathBuf::from(daemon_name)
        }
        #[cfg(not(debug_assertions))]
        {
            anyhow::bail!(
                "Daemon binary not found at {}. The application may be incorrectly installed.",
                daemon_path.display()
            );
        }
    };

    // D1: one root. `ensure_store` seeds README/.layout; the config dir must
    // exist before the spawn — `resolve_config_base_dir` ignores an
    // `OPENALPACA_CONFIG_DIR` that does not.
    let app_dir = store::ensure_store(&store::StoreScope::Home)?;
    let config_dir = store::ensure_runtime_config_dir()?;

    tracing::info!("Spawning daemon: {}", path_to_use.display());
    tracing::info!("Daemon runtime dir: {}", app_dir.display());
    tracing::info!("Daemon config dir: {}", config_dir.display());

    #[cfg(any(unix, windows))]
    daemon_command(&path_to_use, &app_dir, &config_dir).spawn()?;
    Ok(())
}

/// Common launch settings; platform branches only configure detachment.
fn daemon_command(
    binary: &std::path::Path,
    runtime_dir: &std::path::Path,
    config_dir: &std::path::Path,
) -> Command {
    let mut cmd = Command::new(binary);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(runtime_dir)
        .env("OPENALPACA_CONFIG_DIR", config_dir);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and the callback allocates nothing.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::daemon_command;
    use std::{ffi::OsStr, path::Path};

    #[test]
    fn sidecar_command_uses_the_bundle_binary_and_runtime_config() {
        let binary = Path::new("bundle/openalpacad");
        let runtime = Path::new("runtime");
        let config = Path::new("runtime/config");
        let command = daemon_command(binary, runtime, config);
        assert_eq!(command.get_program(), binary.as_os_str());
        assert_eq!(command.get_current_dir(), Some(runtime));
        assert_eq!(command.get_args().count(), 0);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            vec![(
                OsStr::new("OPENALPACA_CONFIG_DIR"),
                Some(config.as_os_str())
            )]
        );
    }
}

// ============================================================================
// Tauri App Entry Point
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_connection_info,
            ensure_daemon_running
        ])
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}
