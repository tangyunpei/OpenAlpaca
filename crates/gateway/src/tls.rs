use std::path::Path;

use axum_server::tls_rustls::RustlsConfig;
use openalpaca_core::CoreError;

/// Load a rustls TLS config from PEM-encoded cert and key files.
///
/// Returns an error if either file does not exist or cannot be parsed.
pub async fn load_rustls_config(
    cert_path: &str,
    key_path: &str,
) -> Result<RustlsConfig, CoreError> {
    if !Path::new(cert_path).exists() {
        return Err(CoreError::InvalidConfig(format!(
            "TLS cert file not found: {cert_path}"
        )));
    }
    if !Path::new(key_path).exists() {
        return Err(CoreError::InvalidConfig(format!(
            "TLS key file not found: {key_path}"
        )));
    }

    RustlsConfig::from_pem_file(cert_path, key_path)
        .await
        .map_err(|e| CoreError::Io(std::io::Error::other(format!("TLS config error: {e}"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_cert_file_returns_error() {
        let result = load_rustls_config("/nonexistent/cert.pem", "/nonexistent/key.pem").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cert file not found"), "got: {err}");
    }

    #[tokio::test]
    async fn missing_key_file_returns_error() {
        let cert = std::env::temp_dir().join("test-cert.pem");
        std::fs::write(&cert, "dummy").unwrap();

        let result = load_rustls_config(cert.to_str().unwrap(), "/nonexistent/key.pem").await;

        std::fs::remove_file(&cert).ok();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("key file not found"), "got: {err}");
    }
}
