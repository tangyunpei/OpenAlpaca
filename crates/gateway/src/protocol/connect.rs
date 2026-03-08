use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectParams {
    pub min_protocol: u32,
    pub max_protocol: u32,
    pub client: ClientInfo,
    pub auth: Option<AuthParams>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthParams {
    pub token: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloOk {
    pub protocol: u32,
    pub server: ServerInfo,
    pub features: Features,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthResult>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Features {
    pub channels: Vec<String>,
    pub rpc_methods: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthResult {
    pub method: String,
    pub authenticated: bool,
}

/// Current wire protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Negotiate the protocol version between client and server.
/// Returns the highest mutually supported version, or None if incompatible.
pub fn negotiate_version(client_min: u32, client_max: u32) -> Option<u32> {
    if client_min > PROTOCOL_VERSION {
        // Client too new
        return None;
    }
    if client_max < PROTOCOL_VERSION {
        // Client too old — but we only have v1, so accept if range includes v1
        // In future with multiple versions, this would check min supported server version
    }
    // Use the minimum of server's version and client's max
    if PROTOCOL_VERSION > client_max {
        return None;
    }
    Some(PROTOCOL_VERSION.min(client_max))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_compatible() {
        assert_eq!(negotiate_version(1, 1), Some(1));
        assert_eq!(negotiate_version(1, 3), Some(1)); // server is v1, client supports up to v3
    }

    #[test]
    fn test_negotiate_client_too_new() {
        assert_eq!(negotiate_version(2, 3), None); // client requires v2+, server only has v1
    }

    #[test]
    fn test_negotiate_exact_match() {
        assert_eq!(negotiate_version(1, 1), Some(1));
    }

    #[test]
    fn test_negotiate_wide_range() {
        // Client supports v1-v10, server is v1
        assert_eq!(negotiate_version(1, 10), Some(1));
    }
}
