use std::collections::HashMap;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::stdio_channel::StdioChannel;

/// A running plugin child process with its JSON-RPC transport channel.
pub struct PluginProcess {
    pub child: Child,
    pub channel: StdioChannel,
    /// JSON-RPC notifications ($/event etc.) from the plugin. Delivery is
    /// best-effort: if this receiver is not drained, notifications beyond the
    /// channel capacity are dropped (with a warning) instead of blocking the
    /// transport's reader loop. No consumer exists yet; a notification handler
    /// can be attached here later.
    pub notification_rx: mpsc::Receiver<Value>,
}

impl PluginProcess {
    /// Spawn a plugin child process and set up the StdioChannel transport.
    ///
    /// Resolves the entry path relative to `plugin_dir`, configures stdin/stdout/stderr
    /// as piped, applies manifest-defined args and environment variables, and creates
    /// the multiplexed JSON-RPC channel over stdio.
    pub fn spawn(manifest: &PluginManifest, plugin_dir: &Path) -> Result<Self, PluginError> {
        let entry = manifest.entry_path(plugin_dir);

        let mut cmd = Command::new(&entry);
        cmd.args(&manifest.plugin.args)
            .current_dir(plugin_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Ensure the child is killed if this PluginProcess is dropped without
            // an explicit shutdown, instead of leaking an orphaned process.
            .kill_on_drop(true);

        for (key, value) in &manifest.plugin.env {
            cmd.env(key, value);
        }

        info!(
            plugin = %manifest.plugin.name,
            entry = %entry.display(),
            "spawning plugin process"
        );

        let mut child = cmd.spawn().map_err(|e| {
            PluginError::SpawnFailed(format!(
                "failed to spawn {}: {}",
                entry.display(),
                e
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            PluginError::SpawnFailed("failed to capture plugin stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            PluginError::SpawnFailed("failed to capture plugin stdout".to_string())
        })?;

        // Drain the child's stderr in the background. It is piped but nothing
        // else reads it, so a plugin logging more than ~64KB to stderr would
        // block on a full pipe and freeze — every subsequent RPC then times out.
        if let Some(stderr) = child.stderr.take() {
            let plugin_name = manifest.plugin.name.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!(plugin = %plugin_name, "plugin stderr: {line}");
                }
            });
        }

        let (channel, notification_rx) = StdioChannel::new(
            stdin,
            stdout,
            manifest.plugin.max_concurrent,
            Duration::from_secs(60),
        );

        debug!(
            plugin = %manifest.plugin.name,
            max_concurrent = manifest.plugin.max_concurrent,
            "plugin process spawned, StdioChannel created"
        );

        Ok(Self {
            child,
            channel,
            notification_rx,
        })
    }

    /// Send the `initialize` JSON-RPC request and wait for a `{ ready: true }` response.
    ///
    /// Times out after 10 seconds. If the plugin does not respond with `ready: true`,
    /// returns `PluginError::RpcError`.
    pub async fn initialize(
        &self,
        plugin_id: &str,
        version: &str,
        capabilities_granted: &[String],
        config: HashMap<String, Value>,
    ) -> Result<(), PluginError> {
        let params = json!({
            "plugin_id": plugin_id,
            "version": version,
            "capabilities_granted": capabilities_granted,
            "config": config,
            "daemon_version": env!("CARGO_PKG_VERSION"),
        });

        info!(plugin_id, "sending initialize RPC");

        let result = self
            .channel
            .call_with_timeout("initialize", params, Duration::from_secs(10))
            .await?;

        let ready = result
            .get("ready")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !ready {
            error!(plugin_id, ?result, "plugin did not respond with ready: true");
            return Err(PluginError::RpcError {
                code: -1,
                message: format!(
                    "plugin '{}' initialize did not return ready: true",
                    plugin_id
                ),
            });
        }

        info!(plugin_id, "plugin initialized successfully");
        Ok(())
    }

    /// Send a graceful `shutdown` JSON-RPC request with a 3-second timeout.
    ///
    /// If the plugin does not respond in time, callers should follow up with [`kill`].
    pub async fn shutdown(&self) -> Result<(), PluginError> {
        debug!("sending shutdown RPC");
        self.channel
            .call_with_timeout("shutdown", json!({}), Duration::from_secs(3))
            .await?;
        debug!("shutdown RPC acknowledged");
        Ok(())
    }

    /// Forcefully kill the child process.
    ///
    /// This is a last resort after [`shutdown`] fails or times out.
    pub fn kill(&mut self) {
        warn!("killing plugin process");
        // Child::start_kill is non-blocking and initiates process termination.
        if let Err(e) = self.child.start_kill() {
            error!(error = %e, "failed to kill plugin process");
        }
    }

    /// Non-blocking check whether the child process has exited.
    ///
    /// Returns `Some(ExitStatus)` if the process has exited, `None` if still running.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        match self.child.try_wait() {
            Ok(status) => status,
            Err(e) => {
                warn!(error = %e, "error checking plugin process status");
                None
            }
        }
    }
}
