//! Shared URL validation for SSRF protection.
//!
//! Used by both the `web_fetch` built-in tool and the HTTP backend in
//! the tool registry to prevent Server-Side Request Forgery (SSRF).

/// Check if an IPv6 address is in the Unique Local Address range (fc00::/7).
/// This includes addresses starting with fc or fd (fc00::/7 covers both).
fn is_ipv6_unique_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// Check if an IPv6 address is in the link-local range (fe80::/10).
fn is_ipv6_link_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// Check if an IPv6 address is an IPv4-mapped address (::ffff:x.x.x.x)
/// where the embedded IPv4 address is private/reserved.
fn is_ipv4_mapped_private(ip: &std::net::Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        ipv4.is_loopback()
            || ipv4.is_private()
            || ipv4.is_link_local()
            || ipv4.is_unspecified()
            || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0) == 64) // CGN
            || ipv4.octets() == [169, 254, 169, 254] // AWS metadata
    } else {
        false
    }
}

/// Validate a URL before making an HTTP request to prevent SSRF attacks.
///
/// Blocks:
/// - Non-HTTP(S) schemes (file://, ftp://, etc.)
/// - Cloud metadata endpoints (169.254.169.254, metadata.google.internal, etc.)
/// - Private/reserved IP ranges (127.x, 10.x, 172.16-31.x, 192.168.x, [::1])
/// - Localhost variations
pub fn validate_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;

    // Only allow http and https
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "URL scheme '{}' is not allowed; only http and https are permitted",
                scheme
            ));
        }
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Block cloud metadata endpoints
    let blocked_hosts = [
        "169.254.169.254",          // AWS/GCP/Azure metadata
        "metadata.google.internal", // GCP metadata
        "metadata.internal",        // Generic cloud metadata
    ];
    let host_lower = host.to_lowercase();
    for blocked in &blocked_hosts {
        if host_lower == *blocked {
            return Err(format!(
                "Access to '{}' is blocked (cloud metadata endpoint)",
                host
            ));
        }
    }

    // Block localhost variants
    let localhost_patterns = ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"];
    for pattern in &localhost_patterns {
        if host_lower == *pattern {
            return Err(format!("Access to '{}' is blocked (localhost)", host));
        }
    }

    // Block private IP ranges.
    // Use parsed.host() to get a url::Host enum which correctly handles
    // IPv6 bracket notation (e.g., [fc00::1]) that host_str() preserves.
    let ip_addr = match parsed.host() {
        Some(url::Host::Ipv4(ipv4)) => Some(std::net::IpAddr::V4(ipv4)),
        Some(url::Host::Ipv6(ipv6)) => Some(std::net::IpAddr::V6(ipv6)),
        _ => host.parse::<std::net::IpAddr>().ok(),
    };
    if let Some(ip) = ip_addr {
        let is_private = match ip {
            std::net::IpAddr::V4(ipv4) => {
                ipv4.is_loopback()                          // 127.0.0.0/8
                    || ipv4.is_private()                    // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
                    || ipv4.is_link_local()                 // 169.254.0.0/16
                    || ipv4.is_unspecified()                // 0.0.0.0
                    || (ipv4.octets()[0] == 100 && (ipv4.octets()[1] & 0xC0) == 64) // 100.64.0.0/10 (CGN)
            }
            std::net::IpAddr::V6(ipv6) => {
                ipv6.is_loopback()                          // ::1
                    || ipv6.is_unspecified()                 // ::
                    || is_ipv6_unique_local(&ipv6)           // fc00::/7 (ULA)
                    || is_ipv6_link_local(&ipv6)             // fe80::/10
                    || is_ipv4_mapped_private(&ipv6)         // ::ffff:x.x.x.x with private IPv4
            }
        };
        if is_private {
            return Err(format!("Access to private/reserved IP '{}' is blocked", ip));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_https_url() {
        assert!(validate_url("https://example.com/api").is_ok());
    }

    #[test]
    fn test_valid_http_url() {
        assert!(validate_url("http://example.com/api").is_ok());
    }

    #[test]
    fn test_blocks_file_scheme() {
        let result = validate_url("file:///etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn test_blocks_ftp_scheme() {
        let result = validate_url("ftp://example.com/file");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not allowed"));
    }

    #[test]
    fn test_blocks_localhost() {
        let result = validate_url("http://localhost/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn test_blocks_127_0_0_1() {
        let result = validate_url("http://127.0.0.1/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn test_blocks_aws_metadata() {
        let result = validate_url("http://169.254.169.254/latest/meta-data/");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn test_blocks_gcp_metadata() {
        let result = validate_url("http://metadata.google.internal/computeMetadata/v1/");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn test_blocks_private_10_x() {
        let result = validate_url("http://10.0.0.1/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_private_172_16() {
        let result = validate_url("http://172.16.0.1/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_private_192_168() {
        let result = validate_url("http://192.168.1.1/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_cgn_100_64() {
        let result = validate_url("http://100.64.0.1/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_ipv6_loopback() {
        let result = validate_url("http://[::1]/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("blocked"));
    }

    #[test]
    fn test_invalid_url() {
        let result = validate_url("not-a-url");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid URL"));
    }

    #[test]
    fn test_blocks_ipv6_unique_local_fc() {
        let result = validate_url("http://[fc00::1]/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_ipv6_unique_local_fd() {
        let result = validate_url("http://[fd12:3456:789a::1]/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_ipv6_link_local() {
        let result = validate_url("http://[fe80::1]/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_allows_public_ipv6() {
        assert!(validate_url("http://[2607:f8b0:4004:800::200e]/page").is_ok());
    }

    #[test]
    fn test_blocks_ipv4_mapped_loopback() {
        let result = validate_url("http://[::ffff:127.0.0.1]/admin");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }

    #[test]
    fn test_blocks_ipv4_mapped_private() {
        let result = validate_url("http://[::ffff:10.0.0.1]/internal");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("private"));
    }
}
