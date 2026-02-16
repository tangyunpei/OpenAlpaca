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
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            consecutive_failures: 0,
            state: CircuitState::Closed,
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

        let mut map = self.state.lock().unwrap_or_else(|e| e.into_inner());
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

        let mut map = self.state.lock().unwrap_or_else(|e| e.into_inner());
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
        }
    }

    /// Record a transient tool failure. Opens the circuit if the threshold is reached.
    ///
    /// Returns `true` if the circuit transitioned to Open (i.e., it was just tripped).
    pub fn record_failure(&self, agent_id: &str, tool_name: &str) -> bool {
        if !self.enabled {
            return false;
        }

        let mut map = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = (agent_id.to_string(), tool_name.to_string());

        let entry = map.entry(key).or_default();
        entry.consecutive_failures += 1;

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
mod tests {
    use super::*;
    use crate::bus::EventBus;

    fn make_config(threshold: usize, timeout_secs: u64) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            enabled: true,
            failure_threshold: threshold,
            reset_timeout_secs: timeout_secs,
        }
    }

    fn make_cb(threshold: usize) -> ToolCircuitBreaker {
        ToolCircuitBreaker::new(&make_config(threshold, 300), EventBus::default())
    }

    #[test]
    fn test_closed_allows_calls() {
        let cb = make_cb(5);
        assert!(cb.check("agent1", "web_search").is_ok());
        assert!(cb.check("agent1", "web_search").is_ok());
    }

    #[test]
    fn test_opens_after_threshold() {
        let cb = make_cb(3);

        // 3 consecutive failures should open the circuit
        cb.record_failure("agent1", "tool_x");
        cb.record_failure("agent1", "tool_x");
        assert!(cb.check("agent1", "tool_x").is_ok()); // still closed after 2
        let tripped = cb.record_failure("agent1", "tool_x");
        assert!(tripped); // 3rd failure trips it
        assert!(cb.check("agent1", "tool_x").is_err()); // now open
    }

    #[test]
    fn test_resets_on_success() {
        let cb = make_cb(3);

        cb.record_failure("agent1", "tool_x");
        cb.record_failure("agent1", "tool_x");
        // 2 failures, then success resets
        cb.record_success("agent1", "tool_x");
        assert!(cb.check("agent1", "tool_x").is_ok());

        // Need 3 more failures to trip again
        cb.record_failure("agent1", "tool_x");
        cb.record_failure("agent1", "tool_x");
        assert!(cb.check("agent1", "tool_x").is_ok()); // still closed
        cb.record_failure("agent1", "tool_x");
        assert!(cb.check("agent1", "tool_x").is_err()); // now open
    }

    #[test]
    fn test_half_open_after_timeout() {
        // Use a very short timeout for testing
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 1,
            reset_timeout_secs: 0, // instant reset for test
        };
        let cb = ToolCircuitBreaker::new(&config, EventBus::default());

        cb.record_failure("agent1", "tool_x");
        // Circuit is open, but timeout is 0 so it immediately transitions to half-open
        assert!(cb.check("agent1", "tool_x").is_ok()); // transitions to half-open, allows probe
    }

    #[test]
    fn test_half_open_success_closes() {
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 1,
            reset_timeout_secs: 0,
        };
        let cb = ToolCircuitBreaker::new(&config, EventBus::default());

        cb.record_failure("agent1", "tool_x"); // opens
        assert!(cb.check("agent1", "tool_x").is_ok()); // half-open, probe allowed
        cb.record_success("agent1", "tool_x"); // closes
        assert!(cb.check("agent1", "tool_x").is_ok()); // now closed again
    }

    #[test]
    fn test_half_open_failure_reopens() {
        let config = CircuitBreakerConfig {
            enabled: true,
            failure_threshold: 1,
            reset_timeout_secs: 0,
        };
        let cb = ToolCircuitBreaker::new(&config, EventBus::default());

        cb.record_failure("agent1", "tool_x"); // opens
        assert!(cb.check("agent1", "tool_x").is_ok()); // half-open, probe allowed
        cb.record_failure("agent1", "tool_x"); // probe failed, re-opens

        // Now it's open again with reset_timeout=0, so next check transitions to half-open
        assert!(cb.check("agent1", "tool_x").is_ok()); // half-open again
    }

    #[test]
    fn test_agent_isolation() {
        let cb = make_cb(2);

        // Agent A fails twice → open
        cb.record_failure("agent_a", "tool_x");
        cb.record_failure("agent_a", "tool_x");
        assert!(cb.check("agent_a", "tool_x").is_err());

        // Agent B should still be able to use the same tool
        assert!(cb.check("agent_b", "tool_x").is_ok());
    }

    #[test]
    fn test_tool_isolation() {
        let cb = make_cb(2);

        // tool_x fails twice → open
        cb.record_failure("agent1", "tool_x");
        cb.record_failure("agent1", "tool_x");
        assert!(cb.check("agent1", "tool_x").is_err());

        // tool_y should still work for the same agent
        assert!(cb.check("agent1", "tool_y").is_ok());
    }

    #[test]
    fn test_disabled_does_nothing() {
        let config = CircuitBreakerConfig {
            enabled: false,
            failure_threshold: 1,
            reset_timeout_secs: 300,
        };
        let cb = ToolCircuitBreaker::new(&config, EventBus::default());

        // Even after many failures, calls should still be allowed
        for _ in 0..10 {
            cb.record_failure("agent1", "tool_x");
        }
        assert!(cb.check("agent1", "tool_x").is_ok());
    }

    #[test]
    fn test_transient_error_classification() {
        // Transient errors (should trip)
        assert!(is_transient_tool_error("HTTP request failed: connection refused"));
        assert!(is_transient_tool_error("Tool 'api_call' timed out after 60s"));
        assert!(is_transient_tool_error("HTTP 503 — Service Unavailable"));
        assert!(is_transient_tool_error("HTTP 500 — Internal Server Error"));
        assert!(is_transient_tool_error("Connection refused"));
        assert!(is_transient_tool_error("connection reset by peer"));
        assert!(is_transient_tool_error("network error"));
        assert!(is_transient_tool_error("Command failed (exit 1): something broke"));
        assert!(is_transient_tool_error("Command timed out after 30s"));

        // Permanent errors (should NOT trip)
        assert!(!is_transient_tool_error("HTTP 404 — Not Found"));
        assert!(!is_transient_tool_error("HTTP 403 — Forbidden"));
        assert!(!is_transient_tool_error("Unknown tool: nonexistent"));
        assert!(!is_transient_tool_error("Access denied by policy"));
        assert!(!is_transient_tool_error("Command failed (exit 127): not found"));
        assert!(!is_transient_tool_error("Invalid argument: missing required field"));
    }

    #[test]
    fn test_tripped_event_emitted() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let config = make_config(2, 300);
        let cb = ToolCircuitBreaker::new(&config, bus);

        cb.record_failure("agent1", "web_search");
        let tripped = cb.record_failure("agent1", "web_search");
        assert!(tripped);

        let event = rx.try_recv().unwrap();
        match event {
            SystemEvent::CircuitBreakerTripped {
                agent_id,
                tool_name,
                consecutive_failures,
                reset_after_secs,
                ..
            } => {
                assert_eq!(agent_id, "agent1");
                assert_eq!(tool_name, "web_search");
                assert_eq!(consecutive_failures, 2);
                assert_eq!(reset_after_secs, 300);
            }
            other => panic!("Expected CircuitBreakerTripped, got: {:?}", other),
        }
    }
}
