use std::collections::HashMap;
use serde::Deserialize;
use toml::Value;

/// Dashboard HTTP server configuration
#[derive(Debug, Clone)]
pub struct CfgDashboard {
    /// Port to listen on (default: 8080)
    pub port: u16,
    /// Bind address (default: 0.0.0.0)
    pub bind: String,
}

impl Default for CfgDashboard {
    fn default() -> Self {
        Self {
            port: 8080,
            bind: "0.0.0.0".to_string(),
        }
    }
}

#[derive(Deserialize)]
pub struct CfgDashboardDto {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_bind")]
    pub bind: String,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

fn default_port() -> u16 { 8080 }
fn default_bind() -> String { "0.0.0.0".to_string() }

pub fn apply_dashboard_patch(src: CfgDashboardDto) -> Result<CfgDashboard, String> {
    if src.port == 0 {
        return Err("dashboard: port cannot be 0".to_string());
    }
    Ok(CfgDashboard {
        port: src.port,
        bind: src.bind,
    })
}
