use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

/// SX1255/SoapySX resilience calibration policy.
///
/// The defaults are conservative: the feature is disabled unless the operator
/// opts in, and temperature based frequency retuning is disabled unless a board
/// specific coefficient is configured.
#[derive(Debug, Clone)]
pub struct CfgSx1255Autocal {
    /// Enable the SX1255 autocalibration manager.
    pub enabled: bool,
    /// Run non-streaming calibration/probing before RX/TX streams are activated.
    pub startup: bool,
    /// Run periodic non-disruptive checks while the BS is running.
    pub periodic: bool,
    /// Periodic check interval.
    pub interval_secs: u64,
    /// Try to read temperature through SoapySDR sensors.
    pub read_temperature: bool,
    /// Allow periodic temperature reads while RX/TX streams are active.
    ///
    /// Keep false for zero intentional RX/TX interruption on SX1255 boards where
    /// temperature measurement shares the RX ADC path.
    pub allow_periodic_temperature_read: bool,
    /// Optional exact SoapySDR sensor key. If unset, common names are probed.
    pub temperature_sensor: Option<String>,
    /// Candidate sensor names probed when `temperature_sensor` is unset.
    pub temperature_sensor_keys: Vec<String>,
    /// Minimum temperature delta that is considered meaningful.
    pub min_temperature_delta_c: f64,
    /// Optional reference temperature for frequency compensation.
    /// If unset, the first successful reading becomes the baseline.
    pub reference_temperature_c: Option<f64>,
    /// Frequency drift coefficient in ppm/degC. 0.0 disables retuning.
    pub temp_ppm_per_c: f64,
    /// Minimum absolute retune step per RF chain.
    pub min_frequency_step_hz: f64,
    /// Clamp absolute temperature-derived frequency correction per RF chain.
    pub max_frequency_correction_hz: f64,
    /// Allow frequency retuning while streams are active.
    pub allow_periodic_retune: bool,
    /// Enable SoapySDR automatic DC offset correction when supported.
    pub enable_dc_offset_mode: bool,
    /// At startup, switch RX antenna to SoapySX RF loopback (`LB`) and back to
    /// verify that loopback control is available before streams start.
    pub rf_loopback_startup_check: bool,
}

impl Default for CfgSx1255Autocal {
    fn default() -> Self {
        Self {
            enabled: false,
            startup: true,
            periodic: true,
            interval_secs: 3600,
            read_temperature: true,
            allow_periodic_temperature_read: false,
            temperature_sensor: None,
            temperature_sensor_keys: vec![
                "temperature".to_string(),
                "temp".to_string(),
                "sx1255_temperature".to_string(),
                "sx1255_temp".to_string(),
            ],
            min_temperature_delta_c: 2.0,
            reference_temperature_c: None,
            temp_ppm_per_c: 0.0,
            min_frequency_step_hz: 25.0,
            max_frequency_correction_hz: 5000.0,
            allow_periodic_retune: false,
            enable_dc_offset_mode: true,
            rf_loopback_startup_check: true,
        }
    }
}

#[derive(Default, Deserialize)]
pub struct CfgSx1255AutocalDto {
    pub enabled: Option<bool>,
    pub startup: Option<bool>,
    pub periodic: Option<bool>,
    pub interval_secs: Option<u64>,
    pub read_temperature: Option<bool>,
    pub allow_periodic_temperature_read: Option<bool>,
    pub temperature_sensor: Option<String>,
    pub temperature_sensor_keys: Option<Vec<String>>,
    pub min_temperature_delta_c: Option<f64>,
    pub reference_temperature_c: Option<f64>,
    pub temp_ppm_per_c: Option<f64>,
    pub min_frequency_step_hz: Option<f64>,
    pub max_frequency_correction_hz: Option<f64>,
    pub allow_periodic_retune: Option<bool>,
    pub enable_dc_offset_mode: Option<bool>,
    pub rf_loopback_startup_check: Option<bool>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

pub fn apply_sx1255_autocal_patch(src: Option<CfgSx1255AutocalDto>) -> CfgSx1255Autocal {
    let mut cfg = CfgSx1255Autocal::default();
    if let Some(src) = src {
        if let Some(v) = src.enabled {
            cfg.enabled = v;
        }
        if let Some(v) = src.startup {
            cfg.startup = v;
        }
        if let Some(v) = src.periodic {
            cfg.periodic = v;
        }
        if let Some(v) = src.interval_secs {
            cfg.interval_secs = v.max(1);
        }
        if let Some(v) = src.read_temperature {
            cfg.read_temperature = v;
        }
        if let Some(v) = src.allow_periodic_temperature_read {
            cfg.allow_periodic_temperature_read = v;
        }
        if let Some(v) = src.temperature_sensor {
            cfg.temperature_sensor = Some(v);
        }
        if let Some(v) = src.temperature_sensor_keys {
            cfg.temperature_sensor_keys = v;
        }
        if let Some(v) = src.min_temperature_delta_c {
            cfg.min_temperature_delta_c = v.max(0.0);
        }
        if let Some(v) = src.reference_temperature_c {
            cfg.reference_temperature_c = Some(v);
        }
        if let Some(v) = src.temp_ppm_per_c {
            cfg.temp_ppm_per_c = v;
        }
        if let Some(v) = src.min_frequency_step_hz {
            cfg.min_frequency_step_hz = v.max(0.0);
        }
        if let Some(v) = src.max_frequency_correction_hz {
            cfg.max_frequency_correction_hz = v.max(0.0);
        }
        if let Some(v) = src.allow_periodic_retune {
            cfg.allow_periodic_retune = v;
        }
        if let Some(v) = src.enable_dc_offset_mode {
            cfg.enable_dc_offset_mode = v;
        }
        if let Some(v) = src.rf_loopback_startup_check {
            cfg.rf_loopback_startup_check = v;
        }
    }
    cfg
}

/// SoapySDR configuration
#[derive(Debug, Clone)]
pub struct CfgSoapySdr {
    /// Uplink frequency in Hz
    pub ul_freq: f64,
    /// Downlink frequency in Hz
    pub dl_freq: f64,
    /// PPM frequency error correction
    pub ppm_err: f64,
    /// Argument string to select a specific SDR device.
    /// If None, devices will be enumerated until the first supported device is found.
    pub device: Option<String>,
    /// RX antenna. Device specific default will be used if None.
    pub rx_ant: Option<String>,
    /// TX antenna. Device specific default will be used if None.
    pub tx_ant: Option<String>,
    /// RX gain values.
    /// Device specific defaults will be used for gains that are not set.
    pub rx_gains: HashMap<String, f64>,
    /// TX gain values.
    /// Device specific defaults will be used for gains that are not set.
    pub tx_gains: HashMap<String, f64>,
    /// RX and TX sample rate. Device specific default will be used if None.
    pub fs: Option<f64>,
    /// RX channel number
    pub rx_ch: Option<usize>,
    /// TX channel number
    pub tx_ch: Option<usize>,
    /// SX1255/SoapySX resilience calibration policy.
    pub sx1255_autocal: CfgSx1255Autocal,
}

impl CfgSoapySdr {
    /// Get corrected UL frequency with PPM error applied
    pub fn ul_freq_corrected(&self) -> (f64, f64) {
        let ppm = self.ppm_err;
        let err = (self.ul_freq / 1_000_000.0) * ppm;
        (self.ul_freq + err, err)
    }

    /// Get corrected DL frequency with PPM error applied
    pub fn dl_freq_corrected(&self) -> (f64, f64) {
        let ppm = self.ppm_err;
        let err = (self.dl_freq / 1_000_000.0) * ppm;
        (self.dl_freq + err, err)
    }
}

#[derive(Deserialize)]
pub struct SoapySdrDto {
    pub rx_freq: f64,
    pub tx_freq: f64,
    pub ppm_err: Option<f64>,

    pub device: Option<String>,

    pub rx_antenna: Option<String>,
    pub tx_antenna: Option<String>,

    pub sample_rate: Option<f64>,
    pub rx_channel: Option<usize>,
    pub tx_channel: Option<usize>,

    pub sx1255_autocal: Option<CfgSx1255AutocalDto>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
