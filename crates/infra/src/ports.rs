use openalpaca_core::CoreError;

/// Check that a TCP port is available on localhost.
pub async fn ensure_port_available(port: u16) -> Result<(), CoreError> {
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|_| CoreError::PortInUse { port })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_free_port_is_available() {
        // Port 0 asks the OS to pick any available port — always succeeds
        assert!(ensure_port_available(0).await.is_ok());
    }

    #[tokio::test]
    async fn test_used_port_is_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let result = ensure_port_available(port).await;
        assert!(result.is_err());

        drop(listener);
    }
}
