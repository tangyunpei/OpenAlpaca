use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    /// Maximum input length in bytes.
    pub max_input_length: usize,
    /// Circuit breaker settings for repeated tool failures.
    pub circuit_breaker: CircuitBreakerConfig,
    /// When true, skip all interactive tool confirmations (dev/testing use).
    #[serde(default)]
    pub auto_approve_confirmations: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            max_input_length: 32 * 1024,
            circuit_breaker: CircuitBreakerConfig::default(),
            auto_approve_confirmations: false,
        }
    }
}

/// Circuit breaker configuration for tool execution.
///
/// When a tool (HTTP, Command, or BuiltIn) fails consecutively more than
/// `failure_threshold` times for a given (agent, tool) pair, the circuit
/// opens and subsequent calls are rejected immediately until `reset_timeout_secs`
/// elapses, at which point a single probe call is allowed (half-open state).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CircuitBreakerConfig {
    /// Enable/disable the tool circuit breaker.
    pub enabled: bool,
    /// Number of consecutive transient failures before the circuit opens.
    pub failure_threshold: usize,
    /// Seconds to keep the circuit open before allowing a probe call (half-open).
    pub reset_timeout_secs: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: 5,
            reset_timeout_secs: 300, // 5 minutes
        }
    }
}
