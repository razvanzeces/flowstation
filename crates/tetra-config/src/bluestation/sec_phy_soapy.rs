use serde::Deserialize;
use std::collections::HashMap;
use toml::Value;

/// SoapySDR configuration
#[derive(Debug, Clone)]
pub struct CfgSoapySdr {
    /// Uplink frequency in Hz
    pub ul_freq: f64,
    /// Downlink frequency in Hz
    pub dl_freq: f64,
    /// Optional SDR RX center frequency in Hz. Multi-carrier BS uses this as the RF center.
    pub rx_center_freq: Option<f64>,
    /// Optional SDR TX center frequency in Hz. Multi-carrier BS uses this as the RF center.
    pub tx_center_freq: Option<f64>,
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
}

impl CfgSoapySdr {
    /// Apply configured PPM correction to an arbitrary frequency.
    pub fn correct_frequency(&self, freq: f64) -> (f64, f64) {
        let err = (freq / 1_000_000.0) * self.ppm_err;
        (freq + err, err)
    }

    /// Get corrected UL frequency with PPM error applied
    pub fn ul_freq_corrected(&self) -> (f64, f64) {
        self.correct_frequency(self.ul_freq)
    }

    /// Get corrected DL frequency with PPM error applied
    pub fn dl_freq_corrected(&self) -> (f64, f64) {
        self.correct_frequency(self.dl_freq)
    }

    /// Get corrected SDR RX center frequency with PPM error applied.
    pub fn rx_center_freq_corrected(&self) -> Option<(f64, f64)> {
        self.rx_center_freq.map(|freq| self.correct_frequency(freq))
    }

    /// Get corrected SDR TX center frequency with PPM error applied.
    pub fn tx_center_freq_corrected(&self) -> Option<(f64, f64)> {
        self.tx_center_freq.map(|freq| self.correct_frequency(freq))
    }

    /// Effective corrected RX center frequency. Falls back to the legacy single-carrier RX frequency.
    pub fn effective_rx_center_freq_corrected(&self) -> (f64, f64) {
        self.rx_center_freq_corrected().unwrap_or_else(|| self.ul_freq_corrected())
    }

    /// Effective corrected TX center frequency. Falls back to the legacy single-carrier TX frequency.
    pub fn effective_tx_center_freq_corrected(&self) -> (f64, f64) {
        self.tx_center_freq_corrected().unwrap_or_else(|| self.dl_freq_corrected())
    }
}

#[derive(Deserialize)]
pub struct SoapySdrDto {
    pub rx_freq: f64,
    pub tx_freq: f64,
    pub rx_center_freq: Option<f64>,
    pub tx_center_freq: Option<f64>,
    pub ppm_err: Option<f64>,

    pub device: Option<String>,

    pub rx_antenna: Option<String>,
    pub tx_antenna: Option<String>,

    pub sample_rate: Option<f64>,
    pub rx_channel: Option<usize>,
    pub tx_channel: Option<usize>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}
