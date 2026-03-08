use std::time::Duration;

/// Build a pre-configured HTTP client with sensible defaults.
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("openalpaca/", env!("CARGO_PKG_VERSION")))
        .pool_max_idle_per_host(10)
        .build()
        .expect("failed to build HTTP client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_http_client() {
        let client = build_http_client();
        // Just verify it doesn't panic
        drop(client);
    }
}
