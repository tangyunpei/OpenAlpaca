use anyhow::{Context, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use openalpaca_storage::discovery;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use sysinfo::System;

/// Start the Daemon if not running.
pub fn start_daemon() -> Result<()> {
    if is_daemon_running() {
        println!("⚠️  Daemon is already running.");
        return Ok(());
    }

    println!("🚀 Starting OpenAlpaca Daemon...");

    // Scan for binary or use cargo run
    // Since we are in dev environment, use cargo run
    let log_file = fs::File::create("daemon.log").unwrap(); // Simple log redirection

    Command::new("cargo")
        .args(["run", "-p", "openalpacad"])
        .stdout(Stdio::from(log_file.try_clone().unwrap()))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("Failed to spawn daemon process")?;

    println!("✅ Daemon spawned in background. Check 'daemon.log' for output.");
    Ok(())
}

/// Stop the Daemon using PID from discovery.json.
pub fn stop_daemon() -> Result<()> {
    if let Some(d) = discovery::read_discovery()? {
        println!("🛑 Stopping Daemon (PID: {})...", d.pid);
        let pid = Pid::from_raw(d.pid as i32);

        match signal::kill(pid, Signal::SIGTERM) {
            Ok(_) => println!("✅ Signal sent."),
            Err(e) => println!("⚠️  Failed to send signal: {}", e),
        }

        // Wait a bit?
    } else {
        println!("⚠️  No active daemon found (discovery.json missing).");
    }
    Ok(())
}

/// Check if daemon process is running.
pub fn is_daemon_running() -> bool {
    if let Ok(Some(d)) = discovery::read_discovery() {
        // Verify PID existence
        let s = System::new_all();
        if let Some(_) = s.processes().get(&sysinfo::Pid::from(d.pid as usize)) {
            return true;
        }
    }
    false
}

/// Start GUI (Tauri)
pub fn start_gui() -> Result<()> {
    // Check if GUI is running?
    // Hard to check exact process in dev mode (npm/node/cargo-tauri).
    // Let's just spawn it.

    println!("🖥️  Starting OpenAlpaca GUI...");

    // Assuming CWD is project root
    let gui_path = PathBuf::from("apps/openalpaca-gui");

    let log_file = fs::File::create("gui.log").unwrap();

    Command::new("npm")
        .args(["run", "tauri", "dev"])
        .current_dir(gui_path)
        .stdout(Stdio::from(log_file.try_clone().unwrap()))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("Failed to spawn GUI process")?;

    println!("✅ GUI started. Check 'gui.log'.");
    Ok(())
}

/// Stop GUI (Naive approach: lookup by name or port?)
/// Dev environment: killing the `npm` process doesn't always kill children.
/// We might need to find "openalpaca-gui" process.
pub fn stop_gui() -> Result<()> {
    println!("🛑 Stopping GUI...");
    let s = System::new_all();
    let mut killed = false;

    for (pid, process) in s.processes() {
        let name = process.name().to_string_lossy();
        if name.contains("openalpaca-gui") || name.contains("OpenAlpaca") {
            // In dev mode, the process name might be different on Mac.
            // Usually "OpenAlpaca" key.
            println!("Found potential GUI process: {} ({})", name, pid);
            #[cfg(unix)]
            {
                let _ = signal::kill(Pid::from_raw(pid.as_u32() as i32), Signal::SIGTERM);
                killed = true;
            }
        }
    }

    if killed {
        println!("✅ GUI stopped.");
    } else {
        println!("⚠️  No GUI process found.");
    }
    Ok(())
}
