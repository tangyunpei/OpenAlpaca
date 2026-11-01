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
                let drained = drain_stderr(stderr, |line| {
                    debug!(plugin = %plugin_name, "plugin stderr: {line}");
                })
                .await;
                if let Err(e) = drained {
                    debug!(plugin = %plugin_name, error = %e, "plugin stderr drain ending");
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
                data: None,
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

/// Read a plugin's stderr to its end, handing `on_line` each line without its
/// `\n` or `\r\n`.
///
/// The stream is the plugin's bytes, not text: a line that is not UTF-8 is
/// decoded lossily (U+FFFD for each invalid sequence) and the drain reads on.
/// Stopping there would drop the pipe's read end, and the plugin's next write
/// to stderr would then fail with `EPIPE` — or `SIGPIPE` would kill it. Ends
/// at EOF, or with the first read error.
async fn drain_stderr<R>(stderr: R, mut on_line: impl FnMut(&str)) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;

    let mut reader = tokio::io::BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Ok(());
        }
        if line.ends_with(b"\n") {
            line.pop();
            if line.ends_with(b"\r") {
                line.pop();
            }
        }
        on_line(&String::from_utf8_lossy(&line));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// A stderr line that is not UTF-8 must not end the drain. The plugin keeps
    /// writing, and once the drain stops, each of its writes fails: `EPIPE` on
    /// the real pipe (or `SIGPIPE`, which kills it), `BrokenPipe` here. The
    /// duplex buffer is far smaller than what follows the bad line, so those
    /// writes complete only while the drain is still reading.
    #[tokio::test]
    async fn the_stderr_drain_reads_past_a_line_that_is_not_utf8() {
        let (mut plugin, daemon) = tokio::io::duplex(64);
        let drain = tokio::spawn(async move {
            let mut lines = Vec::new();
            let end = drain_stderr(daemon, |line| lines.push(line.to_owned())).await;
            (end, lines)
        });

        plugin.write_all(b"caf\xe9 au lait\r\n").await.unwrap();
        for i in 0..100 {
            plugin
                .write_all(format!("line {i}\n").as_bytes())
                .await
                .expect("the drain stopped reading after the line that is not UTF-8");
        }
        plugin
            .write_all(b"the last line, unterminated")
            .await
            .unwrap();
        drop(plugin);

        let (end, lines) = tokio::time::timeout(Duration::from_secs(5), drain)
            .await
            .expect("the drain did not end at EOF")
            .expect("the drain task panicked");
        assert!(end.is_ok(), "EOF ends the drain cleanly: {end:?}");
        assert_eq!(lines.len(), 102);
        assert_eq!(lines[0], "caf\u{FFFD} au lait");
        assert_eq!(lines[1], "line 0");
        assert_eq!(lines[100], "line 99");
        assert_eq!(lines[101], "the last line, unterminated");
    }
}
