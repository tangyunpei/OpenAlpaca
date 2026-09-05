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

    // D1: one root. `ensure_store` seeds README/.layout; `runtime_config_dir`
    // creates the config dir the daemon is started with.
    let app_dir = store::ensure_store(&store::StoreScope::Home)?;
    let config_dir = store::runtime_config_dir()?;

    tracing::info!("Spawning daemon: {}", path_to_use.display());
    tracing::info!("Daemon runtime dir: {}", app_dir.display());
    tracing::info!("Daemon config dir: {}", config_dir.display());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // On Unix, use setsid to detach from terminal
        let mut cmd = Command::new(&path_to_use);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .current_dir(&app_dir)
            .env("OPENALPACA_CONFIG_DIR", &config_dir);

        // Create new session (detach from parent)
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        cmd.spawn()?;
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        Command::new(&path_to_use)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .current_dir(&app_dir)
            .env("OPENALPACA_CONFIG_DIR", &config_dir)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()?;
    }

    Ok(())
}

// ============================================================================
// Tauri App Entry Point
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            get_connection_info,
            ensure_daemon_running
        ])
        .run(tauri::generate_context!())
        .expect("Error while running Tauri application");
}
