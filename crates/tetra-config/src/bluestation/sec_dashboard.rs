use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

/// Dashboard HTTP server configuration
#[derive(Debug, Clone)]
pub struct CfgDashboard {
    /// Port to listen on (default: 8080)
    pub port: u16,
    /// Bind address (default: 0.0.0.0)
    pub bind: String,
    /// Optional explicit path to the FlowStation git source directory used for OTA updates.
    /// When unset, the dashboard auto-detects by:
    ///   1. Walking up from the running binary path until a `.git` directory is found
    ///   2. Trying well-known install paths (/opt/tetra-bluestation, /opt/flowstation, /opt/tetra)
    ///   3. Falling back to the current working directory if it is a git repo
    /// Set this explicitly when the binary is installed outside the repo (e.g. /opt/tetra/
    /// with the git clone elsewhere), or when auto-detection picks the wrong directory.
    pub source_dir: Option<String>,
    /// Optional HTTP Basic Auth credentials.
    /// When both username and password are set, all dashboard requests require authentication.
    /// When omitted, the dashboard is accessible without a password (default, home-network use).
    ///
    /// SECURITY NOTE: HTTP Basic Auth sends credentials as base64 (not encrypted) on the wire.
    /// This protects against casual/accidental access on a LAN but is NOT secure over the
    /// public internet without TLS. For internet-facing deployments, put a reverse proxy
    /// with HTTPS in front of the dashboard.
    pub username: Option<String>,
    pub password: Option<String>,
    /// When true AND auth (username+password) is set, anonymous visitors get a read-only public
    /// overview page instead of being bounced to /login. Admin controls and raw config stay behind
    /// login. Default false = unchanged behaviour (auth is all-or-nothing). Inert without auth.
    pub public_overview: bool,
}

impl Default for CfgDashboard {
    fn default() -> Self {
        Self {
            port: 8080,
            bind: "0.0.0.0".to_string(),
            source_dir: None,
            username: None,
            password: None,
            public_overview: false,
        }
    }
}

#[derive(Deserialize)]
pub struct CfgDashboardDto {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub source_dir: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    // Mandatory DTO field (not optional): the DTO flattens unknown keys into `extra`, so without an
    // explicit field the TOML `public_overview` would be silently ignored.
    #[serde(default)]
    pub public_overview: bool,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_port() -> u16 {
    8080
}
fn default_bind() -> String {
    "0.0.0.0".to_string()
}

pub fn apply_dashboard_patch(src: CfgDashboardDto) -> Result<CfgDashboard, String> {
    if src.port == 0 {
        return Err("dashboard: port cannot be 0".to_string());
    }
    // Validate source_dir if provided: must be an existing directory.
    if let Some(ref sd) = src.source_dir {
        if sd.trim().is_empty() {
            return Err("dashboard: source_dir cannot be empty (omit the field instead)".to_string());
        }
        let path = std::path::Path::new(sd);
        if !path.exists() {
            return Err(format!("dashboard: source_dir '{}' does not exist", sd));
        }
        if !path.is_dir() {
            return Err(format!("dashboard: source_dir '{}' is not a directory", sd));
        }
    }
    // Auth: either both username+password are set, or neither.
    match (&src.username, &src.password) {
        (Some(u), Some(p)) => {
            if u.trim().is_empty() {
                return Err("dashboard: username cannot be empty".to_string());
            }
            if p.is_empty() {
                return Err("dashboard: password cannot be empty".to_string());
            }
        }
        (None, None) => {}
        _ => return Err("dashboard: set both 'username' and 'password', or neither".to_string()),
    }
    Ok(CfgDashboard {
        port: src.port,
        bind: src.bind,
        source_dir: src.source_dir,
        username: src.username,
        password: src.password,
        // public_overview is inert unless auth is set (with no auth the dashboard is already open),
        // so we accept it silently rather than erroring — keeps config validation lenient.
        public_overview: src.public_overview,
    })
}
