use serde::Deserialize;

use crate::bluestation::SecretField;

/// APRS weather-station SDS service (aprs.fi), an extension of the built-in WX/METAR service.
///
/// Two capabilities, both answered from the WX service ISSI (`[wx_service].service_issi`):
///   1. On-demand: a radio sends an SDS "APRSWX <CALL>" (e.g. "APRSWX DN9CHO-13") or just "APRSWX"
///      (uses `default_call`) to the service ISSI and gets the station's latest weather
///      report back. Requires `[wx_service].enabled = true` and a non-empty `apikey`.
///   2. Periodic: the BS auto-sends the weather of `periodic_call` (or `default_call`) to
///      `periodic_dest` every `periodic_interval_secs`. Independent of the METAR periodic.
///
/// Data comes from the aprs.fi API (https://aprs.fi/page/api); a free aprs.fi account
/// provides the API key. Lives in its own `[aprs_wx]` section rather than in `[wx_service]`:
/// the dashboard regenerates `[wx_service]` on save and would drop extra keys there.
#[derive(Debug, Clone)]
pub struct CfgAprsWx {
    /// aprs.fi API key. Empty = APRS weather disabled (requests get a short notice).
    pub apikey: SecretField,
    /// Callsign used for a bare "APRSWX" request and for the periodic send (e.g. "DN9CHO-13").
    pub default_call: String,
    /// Optional: report age (seconds) above which the reply is marked "OLD". 0 = never.
    pub max_age_secs: u64,
    /// On/off for periodic auto-broadcast of an APRS station's weather.
    pub periodic_enabled: bool,
    /// Callsign for the periodic send. Empty = use `default_call`.
    pub periodic_call: String,
    /// Destination of the periodic send (an ISSI, or a GSSI when `periodic_is_group`).
    pub periodic_dest: u32,
    /// Whether `periodic_dest` is a group (GSSI) rather than an individual (ISSI).
    pub periodic_is_group: bool,
    /// Period between automatic sends, in seconds. Clamped to `MIN_PERIODIC_SECS`.
    pub periodic_interval_secs: u64,
}

impl Default for CfgAprsWx {
    fn default() -> Self {
        CfgAprsWx {
            apikey: SecretField::from(String::new()),
            default_call: String::new(),
            max_age_secs: 3600,
            periodic_enabled: false,
            periodic_call: String::new(),
            periodic_dest: 0,
            periodic_is_group: false,
            periodic_interval_secs: 1800,
        }
    }
}

impl CfgAprsWx {
    /// Minimum periodic interval — aprs.fi asks clients not to poll aggressively.
    pub const MIN_PERIODIC_SECS: u64 = 300;

    pub fn effective_interval_secs(&self) -> u64 {
        self.periodic_interval_secs.max(Self::MIN_PERIODIC_SECS)
    }

    pub fn has_apikey(&self) -> bool {
        !self.apikey.val.trim().is_empty()
    }

    /// Callsign for the periodic send: `periodic_call`, falling back to `default_call`.
    pub fn effective_periodic_call(&self) -> String {
        let c = self.periodic_call.trim();
        if c.is_empty() {
            self.default_call.trim().to_string()
        } else {
            c.to_string()
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CfgAprsWxDto {
    #[serde(default)]
    pub apikey: String,
    #[serde(default)]
    pub default_call: String,
    #[serde(default = "default_max_age")]
    pub max_age_secs: u64,
    #[serde(default)]
    pub periodic_enabled: bool,
    #[serde(default)]
    pub periodic_call: String,
    #[serde(default)]
    pub periodic_dest: u32,
    #[serde(default)]
    pub periodic_is_group: bool,
    #[serde(default = "default_interval")]
    pub periodic_interval_secs: u64,
}

fn default_max_age() -> u64 {
    3600
}
fn default_interval() -> u64 {
    1800
}

pub fn apply_aprs_wx_patch(dto: CfgAprsWxDto) -> CfgAprsWx {
    CfgAprsWx {
        apikey: SecretField::from(dto.apikey.trim().to_string()),
        default_call: dto.default_call.trim().to_uppercase(),
        max_age_secs: dto.max_age_secs,
        periodic_enabled: dto.periodic_enabled,
        periodic_call: dto.periodic_call.trim().to_uppercase(),
        periodic_dest: dto.periodic_dest,
        periodic_is_group: dto.periodic_is_group,
        periodic_interval_secs: dto.periodic_interval_secs,
    }
}
