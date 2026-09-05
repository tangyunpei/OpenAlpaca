use anyhow::{Context, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use openalpaca_storage::discovery;
use openalpaca_storage::store;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use sysinfo::System;

const DAEMON_BIN_ENV: &str = "OPENALPACA_DAEMON_BIN";
const GUI_APP_ENV: &str = "OPENALPACA_GUI_APP";
const DAEMON_CONFIG_ENV: &str = "OPENALPACA_CONFIG_DIR";

#[derive(Debug, Clone, PartialEq, Eq)]
enum DaemonLaunch {
    Binary(PathBuf),
    CargoRun { workspace_root: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GuiLaunch {
    AppBundle(PathBuf),
    DevTauri { workspace_root: PathBuf },
}

/// Start the Daemon if not running.
pub fn start_daemon() -> Result<()> {
    if is_daemon_running() {
        println!("⚠️  Daemon is already running.");
        return Ok(());
    }

    println!("🚀 Starting OpenAlpaca Daemon...");

    let runtime_dir = ensure_runtime_dirs()?;
    let config_dir = store::ensure_runtime_config_dir()?;
    let log_path = store::logs_dir()?.join("daemon.log");
    let log_file = fs::File::create(&log_path)
        .with_context(|| format!("Failed to create daemon log file: {}", log_path.display()))?;

    let current_exe = std::env::current_exe().context("Failed to resolve current executable")?;
    let launch = resolve_daemon_launch(&current_exe)?;
    let mut cmd = daemon_launch_command(&launch, &runtime_dir, &config_dir);
    cmd.stdout(Stdio::from(
        log_file
            .try_clone()
            .context("Failed to clone daemon log handle")?,
    ))
    .stderr(Stdio::from(log_file))
    .stdin(Stdio::null());

    cmd.spawn().context("Failed to spawn daemon process")?;

    match launch {
        DaemonLaunch::Binary(path) => {
            println!("✅ Daemon spawned from {}.", path.display());
        }
        DaemonLaunch::CargoRun { .. } => {
            println!("✅ Daemon spawned via development fallback (cargo run).");
        }
    }
    println!("📝 Daemon log: {}", log_path.display());
    Ok(())
}

/// Verify the given PID belongs to a live `openalpacad` process.
///
/// A stale discovery.json (left after a crash/reboot) can point at a PID the OS
/// has since recycled for an unrelated process; signalling or trusting it blindly
/// would kill/misreport that process. Check the process identity first.
fn pid_is_daemon(pid: u32) -> bool {
    let mut s = System::new();
    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    match s.process(sysinfo::Pid::from_u32(pid)) {
        Some(proc_) => {
            let name_matches = proc_.name().to_string_lossy().contains("openalpacad");
            let exe_matches = proc_
                .exe()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().contains("openalpacad"))
                .unwrap_or(false);
            name_matches || exe_matches
        }
        None => false,
    }
}

/// Stop the Daemon using PID from discovery.json.
pub fn stop_daemon() -> Result<()> {
    if let Some(d) = discovery::read_discovery()? {
        if !pid_is_daemon(d.pid) {
            println!(
                "⚠️  PID {} is not a running openalpacad (stale discovery.json?); not signalling.",
                d.pid
            );
            return Ok(());
        }
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
        // Verify the PID exists AND is actually openalpacad (not a recycled PID).
        return pid_is_daemon(d.pid);
    }
    false
}

/// Start GUI (Tauri)
pub fn start_gui() -> Result<()> {
    println!("🖥️  Starting OpenAlpaca GUI...");

    let current_exe = std::env::current_exe().context("Failed to resolve current executable")?;
    match resolve_gui_launch(&current_exe)? {
        GuiLaunch::AppBundle(app_path) => {
            Command::new("open")
                .arg(&app_path)
                .spawn()
                .with_context(|| format!("Failed to open GUI app: {}", app_path.display()))?;
            println!("✅ GUI started from {}.", app_path.display());
        }
        GuiLaunch::DevTauri { workspace_root } => {
            ensure_runtime_dirs()?;
            let log_path = store::logs_dir()?.join("gui.log");
            let log_file = fs::File::create(&log_path).with_context(|| {
                format!("Failed to create GUI log file: {}", log_path.display())
            })?;
            let gui_path = workspace_root.join("apps/openalpaca-gui");

            Command::new("bun")
                .args(["run", "tauri", "dev"])
                .current_dir(&gui_path)
                .stdout(Stdio::from(
                    log_file
                        .try_clone()
                        .context("Failed to clone GUI log handle")?,
                ))
                .stderr(Stdio::from(log_file))
                .spawn()
                .with_context(|| {
                    format!("Failed to spawn GUI process in {}", gui_path.display())
                })?;

            println!("✅ GUI started via development fallback.");
            println!("📝 GUI log: {}", log_path.display());
        }
    }
    Ok(())
}

/// Stop GUI (Naive approach: lookup by name or port?)
/// Dev environment: killing the `npm` process doesn't always kill children.
/// We might need to find "openalpaca-gui" process.
pub fn stop_gui() -> Result<()> {
    println!("🛑 Stopping GUI...");
    let mut s = System::new();
    s.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
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

fn daemon_launch_command(launch: &DaemonLaunch, runtime_dir: &Path, config_dir: &Path) -> Command {
    let mut cmd = match launch {
        DaemonLaunch::Binary(path) => Command::new(path),
        DaemonLaunch::CargoRun { workspace_root } => {
            let mut cmd = Command::new("cargo");
            cmd.args(["run", "--manifest-path"]);
            cmd.arg(workspace_root.join("Cargo.toml"));
            cmd.args(["-p", "openalpacad"]);
            cmd
        }
    };

    cmd.current_dir(runtime_dir);
    cmd.env(DAEMON_CONFIG_ENV, config_dir);
    cmd
}

fn ensure_runtime_dirs() -> Result<PathBuf> {
    let home_root = store::ensure_store(&store::StoreScope::Home)
        .context("Failed to create the OpenAlpaca home store")?;
    store::ensure_runtime_config_dir().context("Failed to create the runtime config directory")?;
    Ok(home_root)
}

fn resolve_daemon_launch(current_exe: &Path) -> Result<DaemonLaunch> {
    let env_override = env_path(DAEMON_BIN_ENV);
    let path_candidate = find_in_path(daemon_binary_name());
    resolve_daemon_launch_from_inputs(current_exe, env_override, path_candidate)
}

fn resolve_daemon_launch_from_inputs(
    current_exe: &Path,
    env_override: Option<PathBuf>,
    path_candidate: Option<PathBuf>,
) -> Result<DaemonLaunch> {
    if let Some(path) = env_override {
        ensure_executable_path(&path, DAEMON_BIN_ENV)?;
        return Ok(DaemonLaunch::Binary(path));
    }

    let exe_dir = current_exe
        .parent()
        .context("Current executable has no parent directory")?;

    let colocated = exe_dir.join(daemon_binary_name());
    if is_executable_file(&colocated) {
        return Ok(DaemonLaunch::Binary(colocated));
    }

    let libexec = exe_dir
        .parent()
        .map(|parent| parent.join("libexec").join(daemon_binary_name()));
    if let Some(path) = libexec
        && is_executable_file(&path)
    {
        return Ok(DaemonLaunch::Binary(path));
    }

    if let Some(path) = path_candidate
        && is_executable_file(&path)
    {
        return Ok(DaemonLaunch::Binary(path));
    }

    if let Some(workspace_root) = find_workspace_root(current_exe, "apps/openalpacad/Cargo.toml") {
        return Ok(DaemonLaunch::CargoRun { workspace_root });
    }

    anyhow::bail!(
        "Unable to locate daemon binary. Checked {}, colocated binary, ../libexec, and PATH.",
        DAEMON_BIN_ENV
    );
}

fn resolve_gui_launch(current_exe: &Path) -> Result<GuiLaunch> {
    resolve_gui_launch_from_inputs(
        current_exe,
        env_path(GUI_APP_ENV),
        home_dir(),
        Path::new("/Applications/openalpaca-gui.app"),
    )
}

fn resolve_gui_launch_from_inputs(
    current_exe: &Path,
    env_override: Option<PathBuf>,
    home: Option<PathBuf>,
    system_app_path: &Path,
) -> Result<GuiLaunch> {
    if let Some(path) = env_override {
        ensure_existing_path(&path, GUI_APP_ENV)?;
        return Ok(GuiLaunch::AppBundle(path));
    }

    if let Some(home_dir) = home {
        let user_app = home_dir.join("Applications").join("openalpaca-gui.app");
        if user_app.exists() {
            return Ok(GuiLaunch::AppBundle(user_app));
        }
    }

    if system_app_path.exists() {
        return Ok(GuiLaunch::AppBundle(system_app_path.to_path_buf()));
    }

    if let Some(workspace_root) =
        find_workspace_root(current_exe, "apps/openalpaca-gui/package.json")
    {
        return Ok(GuiLaunch::DevTauri { workspace_root });
    }

    anyhow::bail!(
        "Unable to locate GUI app bundle. Checked {}, ~/Applications, and /Applications.",
        GUI_APP_ENV
    );
}

fn daemon_binary_name() -> &'static str {
    if cfg!(windows) {
        "openalpacad.exe"
    } else {
        "openalpacad"
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn find_workspace_root(current_exe: &Path, required_relative_path: &str) -> Option<PathBuf> {
    let start = current_exe.parent()?;
    for candidate in start.ancestors() {
        if candidate.join("Cargo.toml").exists() && candidate.join(required_relative_path).exists()
        {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(binary_name))
        .find(|candidate| is_executable_file(candidate))
}

fn ensure_executable_path(path: &Path, env_key: &str) -> Result<()> {
    if is_executable_file(path) {
        return Ok(());
    }
    anyhow::bail!(
        "{env_key} points to an invalid executable: {}",
        path.display()
    );
}

fn ensure_existing_path(path: &Path, env_key: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    anyhow::bail!("{env_key} points to a missing path: {}", path.display());
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir must be created");
        }
        fs::write(path, b"#!/bin/sh\n").expect("file should be written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).expect("permissions");
        }
    }

    #[test]
    fn daemon_resolution_prefers_env_then_colocated_then_libexec_then_path() {
        let root = tempfile::TempDir::new().unwrap();
        let current_exe = root.path().join("bin/openalpaca");
        let env_daemon = root.path().join("env/openalpacad");
        let colocated = root.path().join("bin/openalpacad");
        let libexec = root.path().join("libexec/openalpacad");
        let path_daemon = root.path().join("path/openalpacad");

        touch_executable(&current_exe);
        touch_executable(&env_daemon);
        touch_executable(&colocated);
        touch_executable(&libexec);
        touch_executable(&path_daemon);

        let launch = resolve_daemon_launch_from_inputs(
            &current_exe,
            Some(env_daemon.clone()),
            Some(path_daemon.clone()),
        )
        .expect("launch should resolve");
        assert_eq!(launch, DaemonLaunch::Binary(env_daemon));

        let launch =
            resolve_daemon_launch_from_inputs(&current_exe, None, Some(path_daemon.clone()))
                .expect("launch should resolve");
        assert_eq!(launch, DaemonLaunch::Binary(colocated.clone()));

        fs::remove_file(&colocated).expect("remove colocated");
        let launch =
            resolve_daemon_launch_from_inputs(&current_exe, None, Some(path_daemon.clone()))
                .expect("launch should resolve");
        assert_eq!(launch, DaemonLaunch::Binary(libexec.clone()));

        fs::remove_file(&libexec).expect("remove libexec");
        let launch =
            resolve_daemon_launch_from_inputs(&current_exe, None, Some(path_daemon.clone()))
                .expect("launch should resolve");
        assert_eq!(launch, DaemonLaunch::Binary(path_daemon));
    }

    #[test]
    fn daemon_resolution_uses_dev_fallback_when_workspace_exists() {
        let root = tempfile::TempDir::new().unwrap();
        let current_exe = root.path().join("target/debug/openalpaca");
        touch_executable(&current_exe);
        fs::write(root.path().join("Cargo.toml"), "[workspace]\n")
            .expect("write workspace manifest");
        fs::create_dir_all(root.path().join("apps/openalpacad")).expect("apps dir");
        fs::write(
            root.path().join("apps/openalpacad/Cargo.toml"),
            "[package]\nname=\"openalpacad\"\n",
        )
        .expect("write daemon manifest");

        let launch =
            resolve_daemon_launch_from_inputs(&current_exe, None, None).expect("dev fallback");
        assert_eq!(
            launch,
            DaemonLaunch::CargoRun {
                workspace_root: root.path().to_path_buf()
            }
        );
    }

    #[test]
    fn gui_resolution_prefers_env_then_user_then_system() {
        let root = tempfile::TempDir::new().unwrap();
        let current_exe = root.path().join("bin/openalpaca");
        touch_executable(&current_exe);

        let env_app = root.path().join("custom/openalpaca-gui.app");
        let home_dir = root.path().join("home");
        let user_app = home_dir.join("Applications/openalpaca-gui.app");
        let system_app = root.path().join("system/openalpaca-gui.app");
        fs::create_dir_all(&env_app).expect("env app");
        fs::create_dir_all(&user_app).expect("user app");
        fs::create_dir_all(&system_app).expect("system app");

        let launch = resolve_gui_launch_from_inputs(
            &current_exe,
            Some(env_app.clone()),
            Some(home_dir.clone()),
            &system_app,
        )
        .expect("env app");
        assert_eq!(launch, GuiLaunch::AppBundle(env_app));

        let launch =
            resolve_gui_launch_from_inputs(&current_exe, None, Some(home_dir.clone()), &system_app)
                .expect("user app");
        assert_eq!(launch, GuiLaunch::AppBundle(user_app.clone()));

        fs::remove_dir_all(&user_app).expect("remove user app");
        let launch =
            resolve_gui_launch_from_inputs(&current_exe, None, Some(home_dir), &system_app)
                .expect("system app");
        assert_eq!(launch, GuiLaunch::AppBundle(system_app));
    }

    #[test]
    fn gui_resolution_uses_dev_fallback_when_workspace_exists() {
        let root = tempfile::TempDir::new().unwrap();
        let current_exe = root.path().join("target/debug/openalpaca");
        touch_executable(&current_exe);
        fs::write(root.path().join("Cargo.toml"), "[workspace]\n")
            .expect("write workspace manifest");
        fs::create_dir_all(root.path().join("apps/openalpaca-gui")).expect("gui dir");
        fs::write(
            root.path().join("apps/openalpaca-gui/package.json"),
            "{ \"name\": \"openalpaca-gui\" }\n",
        )
        .expect("write gui package");

        let launch = resolve_gui_launch_from_inputs(
            &current_exe,
            None,
            Some(root.path().join("home")),
            &root.path().join("system/openalpaca-gui.app"),
        )
        .expect("dev fallback");
        assert_eq!(
            launch,
            GuiLaunch::DevTauri {
                workspace_root: root.path().to_path_buf()
            }
        );
    }
}
