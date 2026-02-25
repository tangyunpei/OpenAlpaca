//! Circuit breaker for tool execution.
//!
//! Tracks consecutive transient failures per (agent_id, tool_name) pair.
//! When the failure threshold is reached, the circuit opens and subsequent
//! calls are rejected immediately until a configurable timeout elapses,
//! at which point a single probe call is allowed (half-open state).

use crate::bus::EventBus;
use crate::daemon_config::CircuitBreakerConfig;
use crate::events::SystemEvent;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ── State Machine ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum CircuitState {
    /// Normal operation — calls pass through.
    Closed,
    /// Blocking — calls are rejected immediately.
    Open { opened_at: Instant },
    /// Testing — one probe call is allowed to see if the tool recovered.
    HalfOpen,
}

#[derive(Debug)]
struct ToolState {
    consecutive_failures: usize,
    state: CircuitState,
    last_updated: Instant,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            state: CircuitState::Closed,
            last_updated: Instant::now(),
        }
    }
}

// ── Circuit Breaker ──────────────────────────────────────────────────

/// Per-tool circuit breaker keyed by `(agent_id, tool_name)`.
pub struct ToolCircuitBreaker {
    /// Maps (agent_id, tool_name) → failure tracking state.
    state: Mutex<HashMap<(String, String), ToolState>>,
    failure_threshold: usize,
    reset_timeout: Duration,
    enabled: bool,
    bus: EventBus,
    reset_timeout_secs: u64,
}

impl ToolCircuitBreaker {
    pub fn new(config: &CircuitBreakerConfig, bus: EventBus) -> Self {
        Self {
            state: Mutex::new(HashMap::new()),
            failure_threshold: config.failure_threshold,
            reset_timeout: Duration::from_secs(config.reset_timeout_secs),
            enabled: config.enabled,
            bus,
            reset_timeout_secs: config.reset_timeout_secs,
        }
    }

    /// Check if a tool call should be allowed.
    ///
    /// Returns `Ok(())` if the call should proceed, or `Err(reason)` if the
    /// circuit is open and the call should be blocked.
    pub fn check(&self, agent_id: &str, tool_name: &str) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }

        let mut map = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Circuit breaker mutex poisoned, recovering — a panic may have occurred during state update");
            poisoned.into_inner()
        });
        let key = (agent_id.to_string(), tool_name.to_string());

        let entry = match map.get_mut(&key) {
            Some(e) => e,
            None => return Ok(()), // No state yet — first call, allow it
        };

        match entry.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open { opened_at } => {
                // Check if enough time has elapsed to transition to HalfOpen
                if opened_at.elapsed() >= self.reset_timeout {
                    entry.state = CircuitState::HalfOpen;
                    tracing::info!(
                        agent_id = agent_id,
                        tool = tool_name,
                        "Circuit breaker half-open: allowing probe call"
                    );
                    Ok(())
                } else {
                    let remaining = self.reset_timeout.saturating_sub(opened_at.elapsed());
                    Err(format!(
                        "Circuit breaker open for tool '{}': {} consecutive failures. \
                         Retry after {}s.",
                        tool_name,
                        entry.consecutive_failures,
                        remaining.as_secs()
                    ))
                }
            }
            CircuitState::HalfOpen => {
                // In half-open state, a probe call is already in-flight.
                // Block additional calls until the probe resolves.
                Err(format!(
                    "Circuit breaker half-open for tool '{}': probe call in progress",
                    tool_name
                ))
            }
        }
    }

    /// Record a successful tool execution. Resets the circuit to Closed.
    pub fn record_success(&self, agent_id: &str, tool_name: &str) {
        if !self.enabled {
            return;
        }

        let mut map = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Circuit breaker mutex poisoned, recovering — a panic may have occurred during state update");
            poisoned.into_inner()
        });
        let key = (agent_id.to_string(), tool_name.to_string());

        if let Some(entry) = map.get_mut(&key) {
            if entry.state != CircuitState::Closed || entry.consecutive_failures > 0 {
                tracing::debug!(
                    agent_id = agent_id,
                    tool = tool_name,
                    prev_failures = entry.consecutive_failures,
                    "Circuit breaker reset to closed after success"
                );
            }
            entry.consecutive_failures = 0;
            entry.state = CircuitState::Closed;
            entry.last_updated = Instant::now();
        }
    }

    /// Record a transient tool failure. Opens the circuit if the threshold is reached.
    ///
    /// Returns `true` if the circuit transitioned to Open (i.e., it was just tripped).
    pub fn record_failure(&self, agent_id: &str, tool_name: &str) -> bool {
        if !self.enabled {
            return false;
        }

        let mut map = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Circuit breaker mutex poisoned, recovering — a panic may have occurred during state update");
            poisoned.into_inner()
        });
        let key = (agent_id.to_string(), tool_name.to_string());

        // Prune stale entries to prevent unbounded memory growth from dynamic agent IDs.
        // When the map exceeds 10_000 entries, remove entries idle for more than 1 hour.
        const MAX_ENTRIES: usize = 10_000;
        const STALE_THRESHOLD: Duration = Duration::from_secs(3600);
        if map.len() > MAX_ENTRIES {
            let now = Instant::now();
            map.retain(|_, v| now.duration_since(v.last_updated) < STALE_THRESHOLD);
            if map.len() > MAX_ENTRIES {
                tracing::warn!(
                    "Circuit breaker map still has {} entries after pruning stale entries",
                    map.len()
                );
            }
        }

        let entry = map.entry(key).or_default();
        entry.consecutive_failures += 1;
        entry.last_updated = Instant::now();

        match entry.state {
            CircuitState::Closed => {
                if entry.consecutive_failures >= self.failure_threshold {
                    entry.state = CircuitState::Open {
                        opened_at: Instant::now(),
                    };
                    tracing::warn!(
                        agent_id = agent_id,
                        tool = tool_name,
                        failures = entry.consecutive_failures,
                        reset_secs = self.reset_timeout_secs,
                        "Circuit breaker OPENED: tool disabled for {}s",
                        self.reset_timeout_secs
                    );
                    // Publish event
                    self.bus.publish(SystemEvent::CircuitBreakerTripped {
                        agent_id: agent_id.to_string(),
                        tool_name: tool_name.to_string(),
                        consecutive_failures: entry.consecutive_failures,
                        reset_after_secs: self.reset_timeout_secs,
                        timestamp: Utc::now(),
                    });
                    return true;
                }
            }
            CircuitState::HalfOpen => {
                // Probe call failed — re-open the circuit
                entry.state = CircuitState::Open {
                    opened_at: Instant::now(),
                };
                tracing::warn!(
                    agent_id = agent_id,
                    tool = tool_name,
                    failures = entry.consecutive_failures,
                    "Circuit breaker re-opened: probe call failed"
                );
            }
            CircuitState::Open { .. } => {
                // Already open — just increment counter
            }
        }

        false
    }
}

// ── Error Classification ─────────────────────────────────────────────

/// Classify whether a tool error is transient (should trip circuit breaker)
/// vs permanent/contextual (should not trip).
///
/// Transient errors indicate infrastructure problems (server down, timeout,
/// network issue) where retrying is unlikely to help until the root cause
/// is resolved. Permanent errors (bad arguments, auth failures, tool not
/// found) are contextual and should not count toward the circuit breaker.
pub fn is_transient_tool_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    // Timeouts
    lower.contains("timed out")
        || lower.contains("timeout")
        // HTTP server errors (5xx)
        || lower.contains("http 5")
        || lower.contains("http request failed")
        // Connection errors
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network error")
        // Command failures (but NOT "not found" which is permanent)
        || (lower.contains("command failed") && !lower.contains("not found"))
        || lower.contains("command timed out")
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
