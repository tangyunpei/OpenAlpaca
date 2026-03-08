use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;

use crate::protocol::error::ErrorShape;

/// Auth mode for the gateway.
#[derive(Debug, Clone)]
pub enum AuthMode {
    /// No authentication required.
    None,
    /// Token-based authentication.
    Token(String),
}

/// How the client was authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    None,
    Token,
}

/// Rate limiting bucket per IP.
struct RateBucket {
    failures: u32,
    window_start: Instant,
}

/// Gateway authentication and rate limiting.
pub struct GatewayAuth {
    mode: AuthMode,
    rate_limits: Arc<RwLock<HashMap<IpAddr, RateBucket>>>,
    max_failures: u32,
    window_secs: u64,
}

impl GatewayAuth {
    pub fn new(mode: AuthMode) -> Self {
        Self {
            mode,
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
            max_failures: 10,
            window_secs: 60,
        }
    }

    pub fn mode(&self) -> &AuthMode {
        &self.mode
    }

    /// Verify credentials and check rate limits.
    pub async fn verify(
        &self,
        token: Option<&str>,
        remote_ip: IpAddr,
    ) -> Result<AuthMethod, ErrorShape> {
        // Check rate limit
        if self.is_rate_limited(remote_ip).await {
            return Err(ErrorShape::rate_limited(self.window_secs * 1000));
        }

        match &self.mode {
            AuthMode::None => Ok(AuthMethod::None),
            AuthMode::Token(expected) => match token {
                Some(t) if constant_time_token_eq(t, expected) => {
                    self.reset_failures(remote_ip).await;
                    Ok(AuthMethod::Token)
                }
                _ => {
                    self.record_failure(remote_ip).await;
                    Err(ErrorShape::auth_failed("invalid or missing token"))
                }
            },
        }
    }

    async fn is_rate_limited(&self, ip: IpAddr) -> bool {
        let limits = self.rate_limits.read().await;
        if let Some(bucket) = limits.get(&ip)
            && bucket.window_start.elapsed().as_secs() < self.window_secs
        {
            return bucket.failures >= self.max_failures;
        }
        false
    }

    async fn record_failure(&self, ip: IpAddr) {
        let mut limits = self.rate_limits.write().await;
        let bucket = limits.entry(ip).or_insert(RateBucket {
            failures: 0,
            window_start: Instant::now(),
        });

        if bucket.window_start.elapsed().as_secs() >= self.window_secs {
            bucket.failures = 0;
            bucket.window_start = Instant::now();
        }

        bucket.failures += 1;
    }

    async fn reset_failures(&self, ip: IpAddr) {
        let mut limits = self.rate_limits.write().await;
        limits.remove(&ip);
    }
}

/// Constant-time token comparison via SHA-256 hashing.
/// Hashing both sides ensures equal-length inputs to `ct_eq`,
/// preventing timing leaks from length differences.
fn constant_time_token_eq(a: &str, b: &str) -> bool {
    let hash_a = Sha256::digest(a.as_bytes());
    let hash_b = Sha256::digest(b.as_bytes());
    hash_a.ct_eq(&hash_b).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auth_none_mode() {
        let auth = GatewayAuth::new(AuthMode::None);
        let result = auth.verify(None, "127.0.0.1".parse().unwrap()).await;
        assert_eq!(result.unwrap(), AuthMethod::None);
    }

    #[tokio::test]
    async fn test_auth_token_valid() {
        let auth = GatewayAuth::new(AuthMode::Token("secret".into()));
        let result = auth
            .verify(Some("secret"), "127.0.0.1".parse().unwrap())
            .await;
        assert_eq!(result.unwrap(), AuthMethod::Token);
    }

    #[tokio::test]
    async fn test_auth_token_invalid() {
        let auth = GatewayAuth::new(AuthMode::Token("secret".into()));
        let result = auth
            .verify(Some("wrong"), "127.0.0.1".parse().unwrap())
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, "AUTH_FAILED");
    }

    #[tokio::test]
    async fn test_auth_token_missing() {
        let auth = GatewayAuth::new(AuthMode::Token("secret".into()));
        let result = auth.verify(None, "127.0.0.1".parse().unwrap()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rate_limiting() {
        let mut auth = GatewayAuth::new(AuthMode::Token("secret".into()));
        auth.max_failures = 3;
        let ip: IpAddr = "10.0.0.1".parse().unwrap();

        // Fail 3 times
        for _ in 0..3 {
            let _ = auth.verify(Some("wrong"), ip).await;
        }

        // 4th attempt should be rate limited
        let result = auth.verify(Some("wrong"), ip).await;
        assert_eq!(result.unwrap_err().code, "RATE_LIMITED");

        // Even correct token should be rate limited
        let result = auth.verify(Some("secret"), ip).await;
        assert_eq!(result.unwrap_err().code, "RATE_LIMITED");
    }
}
