use std::time::{Duration, Instant};

use soapysdr::{Args, Device, Direction};
use tetra_config::bluestation::CfgSx1255Autocal;

#[derive(Clone, Copy, Debug)]
pub struct AutocalFrequencies {
    pub rx_hz: Option<f64>,
    pub tx_hz: Option<f64>,
}

#[derive(Clone, Debug)]
enum TemperatureSensor {
    Device(String),
    Channel(Direction, usize, String),
}

impl TemperatureSensor {
    fn label(&self) -> String {
        match self {
            Self::Device(key) => format!("device:{}", key),
            Self::Channel(Direction::Rx, ch, key) => format!("rx{}:{}", ch, key),
            Self::Channel(Direction::Tx, ch, key) => format!("tx{}:{}", ch, key),
        }
    }
}

#[derive(Debug)]
pub struct Sx1255Autocal {
    cfg: CfgSx1255Autocal,
    is_sxceiver: bool,
    freqs: AutocalFrequencies,
    temperature_sensor: Option<TemperatureSensor>,
    baseline_temperature_c: Option<f64>,
    last_temperature_c: Option<f64>,
    last_run: Option<Instant>,
    last_rx_correction_hz: f64,
    last_tx_correction_hz: f64,
    missing_sensor_logged: bool,
    unsupported_logged: bool,
    retune_blocked_logged: bool,
    periodic_temperature_blocked_logged: bool,
}

impl Sx1255Autocal {
    pub fn new(cfg: CfgSx1255Autocal, is_sxceiver: bool, freqs: AutocalFrequencies) -> Self {
        Self {
            cfg,
            is_sxceiver,
            freqs,
            temperature_sensor: None,
            baseline_temperature_c: None,
            last_temperature_c: None,
            last_run: None,
            last_rx_correction_hz: 0.0,
            last_tx_correction_hz: 0.0,
            missing_sensor_logged: false,
            unsupported_logged: false,
            retune_blocked_logged: false,
            periodic_temperature_blocked_logged: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn startup(&mut self, dev: &Device, rx_ch: usize, tx_ch: usize) {
        if !self.cfg.enabled {
            return;
        }
        if !self.ensure_sxceiver() {
            return;
        }

        tracing::info!(
            "SX1255 autocal: enabled startup={} periodic={} interval={}s",
            self.cfg.startup,
            self.cfg.periodic,
            self.cfg.interval_secs
        );

        self.last_run = Some(Instant::now());
        if !self.cfg.startup {
            return;
        }

        if self.cfg.enable_dc_offset_mode {
            self.enable_dc_offset_mode(dev, Direction::Rx, rx_ch);
            self.enable_dc_offset_mode(dev, Direction::Tx, tx_ch);
        }

        if self.cfg.rf_loopback_startup_check {
            self.probe_rf_loopback(dev, rx_ch);
        }

        if let Some(temp_c) = self.read_temperature(dev, rx_ch, tx_ch) {
            self.observe_temperature(temp_c);
            self.apply_temperature_compensation(dev, rx_ch, tx_ch, temp_c, true, "startup");
        }
    }

    pub fn periodic(&mut self, dev: &Device, rx_ch: usize, tx_ch: usize) {
        if !self.cfg.enabled || !self.cfg.periodic {
            return;
        }
        if !self.ensure_sxceiver() {
            return;
        }

        let now = Instant::now();
        if let Some(last_run) = self.last_run {
            if now.duration_since(last_run) < Duration::from_secs(self.cfg.interval_secs) {
                return;
            }
        }
        self.last_run = Some(now);

        if !self.cfg.allow_periodic_temperature_read {
            if !self.periodic_temperature_blocked_logged {
                tracing::info!("SX1255 autocal: periodic temperature read skipped because allow_periodic_temperature_read=false");
                self.periodic_temperature_blocked_logged = true;
            }
            return;
        }

        if let Some(temp_c) = self.read_temperature(dev, rx_ch, tx_ch) {
            self.observe_temperature(temp_c);
            self.apply_temperature_compensation(dev, rx_ch, tx_ch, temp_c, self.cfg.allow_periodic_retune, "periodic");
        }
    }

    fn ensure_sxceiver(&mut self) -> bool {
        if self.is_sxceiver {
            true
        } else {
            if !self.unsupported_logged {
                tracing::warn!("SX1255 autocal: enabled but current SoapySDR device is not SXceiver; disabling autocal checks");
                self.unsupported_logged = true;
            }
            false
        }
    }

    fn enable_dc_offset_mode(&self, dev: &Device, direction: Direction, channel: usize) {
        match dev.has_dc_offset_mode(direction, channel) {
            Ok(true) => match dev.set_dc_offset_mode(direction, channel, true) {
                Ok(()) => tracing::info!("SX1255 autocal: enabled {:?} DC offset auto-correction", direction),
                Err(err) => tracing::warn!("SX1255 autocal: failed to enable {:?} DC offset mode: {}", direction, err),
            },
            Ok(false) => tracing::debug!(
                "SX1255 autocal: {:?} DC offset auto-correction is not supported by driver",
                direction
            ),
            Err(err) => tracing::debug!("SX1255 autocal: could not query {:?} DC offset mode support: {}", direction, err),
        }
    }

    fn probe_rf_loopback(&self, dev: &Device, rx_ch: usize) {
        match dev.antennas(Direction::Rx, rx_ch) {
            Ok(antennas) if antennas.iter().any(|ant| ant == "LB") => {
                let original = dev.antenna(Direction::Rx, rx_ch).unwrap_or_else(|_| "RX".to_string());
                match dev.set_antenna(Direction::Rx, rx_ch, "LB") {
                    Ok(()) => tracing::info!("SX1255 autocal: RF loopback antenna LB is available"),
                    Err(err) => {
                        tracing::warn!(
                            "SX1255 autocal: RF loopback antenna LB is listed but could not be selected: {}",
                            err
                        )
                    }
                }
                if let Err(err) = dev.set_antenna(Direction::Rx, rx_ch, original.as_str()) {
                    tracing::warn!(
                        "SX1255 autocal: failed to restore RX antenna '{}' after loopback probe: {}",
                        original,
                        err
                    );
                }
            }
            Ok(_) => tracing::debug!("SX1255 autocal: RF loopback antenna LB not listed by driver"),
            Err(err) => tracing::debug!("SX1255 autocal: could not list RX antennas for loopback probe: {}", err),
        }
    }

    fn read_temperature(&mut self, dev: &Device, rx_ch: usize, tx_ch: usize) -> Option<f64> {
        if !self.cfg.read_temperature {
            return None;
        }

        if self.temperature_sensor.is_none() {
            self.temperature_sensor = self.discover_temperature_sensor(dev, rx_ch, tx_ch);
        }

        let sensor = match self.temperature_sensor.clone() {
            Some(sensor) => sensor,
            None => {
                if !self.missing_sensor_logged {
                    tracing::warn!("SX1255 autocal: no SoapySDR temperature sensor found; temperature compensation remains inactive");
                    self.missing_sensor_logged = true;
                }
                return None;
            }
        };

        let raw = match &sensor {
            TemperatureSensor::Device(key) => dev.read_sensor(key),
            TemperatureSensor::Channel(direction, channel, key) => dev.read_channel_sensor(*direction, *channel, key),
        };

        match raw {
            Ok(value) => match parse_temperature_c(&value) {
                Some(temp_c) => {
                    tracing::debug!("SX1255 autocal: temperature {} = {:.2} C", sensor.label(), temp_c);
                    Some(temp_c)
                }
                None => {
                    tracing::warn!("SX1255 autocal: could not parse temperature '{}' from {}", value, sensor.label());
                    None
                }
            },
            Err(err) => {
                tracing::warn!("SX1255 autocal: failed to read temperature {}: {}", sensor.label(), err);
                None
            }
        }
    }

    fn discover_temperature_sensor(&self, dev: &Device, rx_ch: usize, tx_ch: usize) -> Option<TemperatureSensor> {
        if let Some(key) = &self.cfg.temperature_sensor {
            if self.sensor_exists(dev, key) {
                return Some(TemperatureSensor::Device(key.clone()));
            }
            if self.channel_sensor_exists(dev, Direction::Rx, rx_ch, key) {
                return Some(TemperatureSensor::Channel(Direction::Rx, rx_ch, key.clone()));
            }
            if self.channel_sensor_exists(dev, Direction::Tx, tx_ch, key) {
                return Some(TemperatureSensor::Channel(Direction::Tx, tx_ch, key.clone()));
            }

            tracing::warn!(
                "SX1255 autocal: configured temperature_sensor '{}' is not listed; trying it as a device sensor anyway",
                key
            );
            return Some(TemperatureSensor::Device(key.clone()));
        }

        let candidates = self
            .cfg
            .temperature_sensor_keys
            .iter()
            .map(|key| key.to_lowercase())
            .collect::<Vec<_>>();

        if let Ok(sensors) = dev.list_sensors() {
            if let Some(sensor) = find_matching_sensor(&sensors, &candidates) {
                return Some(TemperatureSensor::Device(sensor));
            }
        }
        if let Ok(sensors) = dev.list_channel_sensors(Direction::Rx, rx_ch) {
            if let Some(sensor) = find_matching_sensor(&sensors, &candidates) {
                return Some(TemperatureSensor::Channel(Direction::Rx, rx_ch, sensor));
            }
        }
        if let Ok(sensors) = dev.list_channel_sensors(Direction::Tx, tx_ch) {
            if let Some(sensor) = find_matching_sensor(&sensors, &candidates) {
                return Some(TemperatureSensor::Channel(Direction::Tx, tx_ch, sensor));
            }
        }

        None
    }

    fn sensor_exists(&self, dev: &Device, key: &str) -> bool {
        dev.list_sensors()
            .map(|sensors| sensors.iter().any(|sensor| sensor.eq_ignore_ascii_case(key)))
            .unwrap_or(false)
    }

    fn channel_sensor_exists(&self, dev: &Device, direction: Direction, channel: usize, key: &str) -> bool {
        dev.list_channel_sensors(direction, channel)
            .map(|sensors| sensors.iter().any(|sensor| sensor.eq_ignore_ascii_case(key)))
            .unwrap_or(false)
    }

    fn observe_temperature(&mut self, temp_c: f64) {
        if self.baseline_temperature_c.is_none() {
            let baseline = self.cfg.reference_temperature_c.unwrap_or(temp_c);
            self.baseline_temperature_c = Some(baseline);
            tracing::info!("SX1255 autocal: temperature baseline {:.2} C, current {:.2} C", baseline, temp_c);
        }

        if let Some(last_temp) = self.last_temperature_c {
            let delta = temp_c - last_temp;
            if delta.abs() >= self.cfg.min_temperature_delta_c {
                tracing::info!(
                    "SX1255 autocal: temperature changed by {:+.2} C since previous check ({:.2} -> {:.2} C)",
                    delta,
                    last_temp,
                    temp_c
                );
            }
        }
        self.last_temperature_c = Some(temp_c);
    }

    fn apply_temperature_compensation(&mut self, dev: &Device, rx_ch: usize, tx_ch: usize, temp_c: f64, retune_allowed: bool, phase: &str) {
        if self.cfg.temp_ppm_per_c == 0.0 {
            return;
        }

        let baseline = self.baseline_temperature_c.unwrap_or(temp_c);
        let ppm = (temp_c - baseline) * self.cfg.temp_ppm_per_c;
        let rx_target = self
            .freqs
            .rx_hz
            .map(|freq| (freq, clamp_frequency_correction(freq, ppm, self.cfg.max_frequency_correction_hz)));
        let tx_target = self
            .freqs
            .tx_hz
            .map(|freq| (freq, clamp_frequency_correction(freq, ppm, self.cfg.max_frequency_correction_hz)));

        let rx_needs_retune = rx_target
            .map(|(_, correction)| (correction - self.last_rx_correction_hz).abs() >= self.cfg.min_frequency_step_hz)
            .unwrap_or(false);
        let tx_needs_retune = tx_target
            .map(|(_, correction)| (correction - self.last_tx_correction_hz).abs() >= self.cfg.min_frequency_step_hz)
            .unwrap_or(false);

        if !rx_needs_retune && !tx_needs_retune {
            return;
        }

        if !retune_allowed {
            if !self.retune_blocked_logged {
                tracing::warn!(
                    "SX1255 autocal: temperature compensation needs retune but allow_periodic_retune=false; no active-stream retune applied"
                );
                self.retune_blocked_logged = true;
            }
            return;
        }

        if let Some((base_freq, correction)) = rx_target {
            if rx_needs_retune {
                let tuned = base_freq + correction;
                match dev.set_frequency(Direction::Rx, rx_ch, tuned, Args::new()) {
                    Ok(()) => {
                        self.last_rx_correction_hz = correction;
                        tracing::info!(
                            "SX1255 autocal: {} RX retune by {:+.1} Hz ({:.6} MHz)",
                            phase,
                            correction,
                            tuned / 1e6
                        );
                    }
                    Err(err) => tracing::warn!("SX1255 autocal: failed to retune RX for temperature compensation: {}", err),
                }
            }
        }

        if let Some((base_freq, correction)) = tx_target {
            if tx_needs_retune {
                let tuned = base_freq + correction;
                match dev.set_frequency(Direction::Tx, tx_ch, tuned, Args::new()) {
                    Ok(()) => {
                        self.last_tx_correction_hz = correction;
                        tracing::info!(
                            "SX1255 autocal: {} TX retune by {:+.1} Hz ({:.6} MHz)",
                            phase,
                            correction,
                            tuned / 1e6
                        );
                    }
                    Err(err) => tracing::warn!("SX1255 autocal: failed to retune TX for temperature compensation: {}", err),
                }
            }
        }
    }
}

fn find_matching_sensor(sensors: &[String], candidates: &[String]) -> Option<String> {
    sensors
        .iter()
        .find(|sensor| {
            let sensor_lc = sensor.to_lowercase();
            candidates
                .iter()
                .any(|candidate| sensor_lc == *candidate || sensor_lc.contains(candidate))
        })
        .cloned()
}

fn parse_temperature_c(raw: &str) -> Option<f64> {
    let mut token = String::new();
    let mut started = false;
    for ch in raw.chars() {
        let is_numeric = ch.is_ascii_digit() || matches!(ch, '+' | '-' | '.') || (started && matches!(ch, 'e' | 'E'));
        if is_numeric {
            token.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if token.is_empty() { None } else { token.parse::<f64>().ok() }
}

fn clamp_frequency_correction(freq_hz: f64, ppm: f64, max_abs_hz: f64) -> f64 {
    let correction = freq_hz * ppm / 1_000_000.0;
    correction.clamp(-max_abs_hz, max_abs_hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_temperature_values() {
        assert_eq!(parse_temperature_c("42.5"), Some(42.5));
        assert_eq!(parse_temperature_c("42.5 C"), Some(42.5));
        assert_eq!(parse_temperature_c("temp=-7.25C"), Some(-7.25));
        assert_eq!(parse_temperature_c("not available"), None);
    }

    #[test]
    fn matches_temperature_sensor_names() {
        let sensors = vec!["voltage".to_string(), "SX1255_Temp".to_string()];
        let candidates = vec!["temperature".to_string(), "temp".to_string()];
        assert_eq!(find_matching_sensor(&sensors, &candidates), Some("SX1255_Temp".to_string()));
    }

    #[test]
    fn clamps_temperature_frequency_correction() {
        assert_eq!(clamp_frequency_correction(438_000_000.0, 1.0, 5_000.0), 438.0);
        assert_eq!(clamp_frequency_correction(438_000_000.0, 100.0, 5_000.0), 5_000.0);
        assert_eq!(clamp_frequency_correction(438_000_000.0, -100.0, 5_000.0), -5_000.0);
    }
}
