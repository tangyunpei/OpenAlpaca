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
