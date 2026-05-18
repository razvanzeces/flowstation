use std::time::{Duration, Instant};

use soapysdr::{Args, Device, Direction};
use tetra_config::bluestation::CfgSx1255Autocal;

use super::dsp_types::{ComplexSample, RealSample};

const IQ_COEFF_ABS_TOLERANCE: RealSample = 0.05;
const IQ_COEFF_REL_TOLERANCE: RealSample = 0.35;
const IQ_MAX_TONE_DELTA_DB: RealSample = 8.0;
const IQ_MIN_IMAGE_IMPROVEMENT_DB: RealSample = 6.0;
const IQ_MIN_MAIN_RETAINED_RATIO: RealSample = 0.35;
const IQ_FLOOR_DRIFT_DISABLE_DB: RealSample = 12.0;
const IQ_CLIP_LEVEL: RealSample = 0.98;
const IQ_MAX_CLIPPED_FRACTION: RealSample = 0.01;
const LOOPBACK_TX_PREFILL_BLOCKS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct AutocalFrequencies {
    pub rx_hz: Option<f64>,
    pub tx_hz: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct RxStartupCompensation {
    pub dc: ComplexSample,
    pub image_coeff: ComplexSample,
    pub apply_dc: bool,
    pub apply_iq: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToneOrientation {
    Normal,
    Inverted,
}

impl ToneOrientation {
    fn label(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Inverted => "inverted",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ToneMeasurement {
    orientation: ToneOrientation,
    main_is_positive_bin: bool,
    positive_bin: ComplexSample,
    negative_bin: ComplexSample,
    main_mag: RealSample,
    snr_db: RealSample,
    image_dbc: RealSample,
    image_coeff: ComplexSample,
    max_component_abs: RealSample,
    clipped_fraction: RealSample,
    good_blocks: usize,
    total_blocks: usize,
}

#[derive(Clone, Copy, Debug)]
struct ToneBins {
    positive_bin: ComplexSample,
    negative_bin: ComplexSample,
    max_component_abs: RealSample,
    clipped_fraction: RealSample,
    good_blocks: usize,
    total_blocks: usize,
}

impl Default for RxStartupCompensation {
    fn default() -> Self {
        Self {
            dc: ComplexSample { re: 0.0, im: 0.0 },
            image_coeff: ComplexSample { re: 0.0, im: 0.0 },
            apply_dc: false,
            apply_iq: false,
        }
    }
}

impl RxStartupCompensation {
    pub fn apply(&self, samples: &mut [ComplexSample]) {
        if !self.apply_dc && !self.apply_iq {
            return;
        }

        for sample in samples {
            let centered = if self.apply_dc { *sample - self.dc } else { *sample };
            *sample = if self.apply_iq {
                centered + self.image_coeff * centered.conj()
            } else {
                centered
            };
        }
    }

    fn enabled(&self) -> bool {
        self.apply_dc || self.apply_iq
    }
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
    rx_startup_compensation: RxStartupCompensation,
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
            rx_startup_compensation: RxStartupCompensation::default(),
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

        self.apply_rf_filter_profile(dev);

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

    pub fn startup_loopback_calibration(
        &mut self,
        dev: &Device,
        rx_ch: usize,
        tx_ch: usize,
        rx_sample_rate: f64,
        rx_args: &[(String, String)],
        tx_args: &[(String, String)],
    ) {
        if !self.cfg.enabled || !self.cfg.startup || !self.cfg.rf_loopback_startup_calibration {
            return;
        }
        if !self.ensure_sxceiver() {
            return;
        }

        match self.measure_loopback_calibration(dev, rx_ch, tx_ch, rx_sample_rate, rx_args, tx_args) {
            Ok(mut compensation) => {
                if compensation.enabled() {
                    compensation = self.install_driver_compensation(dev, rx_ch, compensation);
                    if compensation.enabled() {
                        tracing::info!(
                            "SX1255 autocal: startup RX software compensation active dc=({:+.6},{:+.6}) image_coeff=({:+.6},{:+.6})",
                            compensation.dc.re,
                            compensation.dc.im,
                            compensation.image_coeff.re,
                            compensation.image_coeff.im
                        );
                        self.rx_startup_compensation = compensation;
                    } else {
                        tracing::info!("SX1255 autocal: startup RX compensation installed in driver; software fallback disabled");
                    }
                } else {
                    tracing::info!("SX1255 autocal: startup loopback calibration completed without live RX correction");
                }
            }
            Err(err) => {
                tracing::warn!("SX1255 autocal: startup loopback calibration skipped: {}", err);
            }
        }
    }

    pub fn rx_startup_compensation(&self) -> RxStartupCompensation {
        self.rx_startup_compensation
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

    fn apply_rf_filter_profile(&self, dev: &Device) {
        if self.cfg.rf_filter_profile.is_empty() {
            return;
        }

        match dev.write_setting("RF_PROFILE", self.cfg.rf_filter_profile.as_str()) {
            Ok(()) => {
                let applied_profile = dev
                    .read_setting("RF_PROFILE")
                    .unwrap_or_else(|_| self.cfg.rf_filter_profile.clone());
                let tx_filter_bw = dev.read_setting("TX_FILTER_BW").unwrap_or_else(|_| "?".to_string());
                let tx_dac_bw = dev.read_setting("TX_DAC_BW").unwrap_or_else(|_| "?".to_string());
                let rx_adc_bw = dev.read_setting("RX_ADC_BW").unwrap_or_else(|_| "?".to_string());
                let rx_adc_trim = dev.read_setting("RX_ADC_TRIM").unwrap_or_else(|_| "?".to_string());
                let rx_pga_bw = dev.read_setting("RX_PGA_BW").unwrap_or_else(|_| "?".to_string());

                tracing::info!(
                    "SX1255 autocal: applied RF filter profile {} (tx_filter_bw={} tx_dac_bw={} rx_adc_bw={} rx_adc_trim={} rx_pga_bw={})",
                    applied_profile,
                    tx_filter_bw,
                    tx_dac_bw,
                    rx_adc_bw,
                    rx_adc_trim,
                    rx_pga_bw
                );
            }
            Err(err) => tracing::warn!(
                "SX1255 autocal: RF filter profile '{}' not applied; driver may not support RF_PROFILE: {}",
                self.cfg.rf_filter_profile,
                err
            ),
        }
    }

    fn install_driver_compensation(&self, dev: &Device, rx_ch: usize, mut compensation: RxStartupCompensation) -> RxStartupCompensation {
        if compensation.apply_dc {
            match dev.has_dc_offset(Direction::Rx, rx_ch) {
                Ok(true) => match dev.set_dc_offset(Direction::Rx, rx_ch, compensation.dc.re as f64, compensation.dc.im as f64) {
                    Ok(()) => {
                        tracing::info!(
                            "SX1255 autocal: installed RX DC correction in SoapySDR driver dc=({:+.6},{:+.6})",
                            compensation.dc.re,
                            compensation.dc.im
                        );
                        compensation.apply_dc = false;
                        compensation.dc = ComplexSample { re: 0.0, im: 0.0 };
                    }
                    Err(err) => tracing::warn!("SX1255 autocal: driver supports RX DC correction but set_dc_offset failed: {}", err),
                },
                Ok(false) => tracing::info!("SX1255 autocal: SoapySDR driver has no RX DC correction API; using software fallback"),
                Err(err) => tracing::warn!(
                    "SX1255 autocal: could not query RX DC correction support; using software fallback: {}",
                    err
                ),
            }
        }

        if compensation.apply_iq {
            match dev.has_iq_balance(Direction::Rx, rx_ch) {
                Ok(true) => match dev.set_iq_balance(
                    Direction::Rx,
                    rx_ch,
                    compensation.image_coeff.re as f64,
                    compensation.image_coeff.im as f64,
                ) {
                    Ok(()) => {
                        tracing::info!(
                            "SX1255 autocal: installed RX IQ image correction in SoapySDR driver coeff=({:+.6},{:+.6})",
                            compensation.image_coeff.re,
                            compensation.image_coeff.im
                        );
                        compensation.apply_iq = false;
                        compensation.image_coeff = ComplexSample { re: 0.0, im: 0.0 };
                    }
                    Err(err) => tracing::warn!(
                        "SX1255 autocal: driver supports RX IQ correction but set_iq_balance failed: {}",
                        err
                    ),
                },
                Ok(false) => tracing::info!("SX1255 autocal: SoapySDR driver has no RX IQ correction API; using software fallback"),
                Err(err) => tracing::warn!(
                    "SX1255 autocal: could not query RX IQ correction support; using software fallback: {}",
                    err
                ),
            }
        }

        compensation
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

    fn measure_loopback_calibration(
        &self,
        dev: &Device,
        rx_ch: usize,
        tx_ch: usize,
        rx_sample_rate: f64,
        rx_args: &[(String, String)],
        tx_args: &[(String, String)],
    ) -> Result<RxStartupCompensation, String> {
        if rx_sample_rate <= 0.0 {
            return Err("RX sample rate is not available".to_string());
        }

        let antennas = dev
            .antennas(Direction::Rx, rx_ch)
            .map_err(|err| format!("could not list RX antennas: {}", err))?;
        if !antennas.iter().any(|ant| ant == "LB") {
            return Err("RF loopback antenna LB is not available".to_string());
        }

        let original_rx_antenna = dev.antenna(Direction::Rx, rx_ch).unwrap_or_else(|_| "RX".to_string());
        let original_rx_frequency = dev.frequency(Direction::Rx, rx_ch).ok();
        let original_tx_frequency = dev.frequency(Direction::Tx, tx_ch).ok();
        let calibration_frequency = original_tx_frequency.or(original_rx_frequency);
        let block_len = stream_period_samples(rx_args).unwrap_or(900).max(64);
        let capture_blocks = self.cfg.rf_loopback_capture_blocks.max(1);
        let settle_blocks = self.cfg.rf_loopback_settle_blocks.max(1);
        let tone = quantized_tone_hz(self.cfg.rf_loopback_tone_hz, rx_sample_rate, block_len);
        let amplitude = self.cfg.rf_loopback_tone_amplitude as RealSample;

        tracing::info!(
            "SX1255 autocal: startup RF loopback calibration tone={:.1} Hz amplitude={:.3} block={} settle={} capture={} rf_center={}",
            tone,
            amplitude,
            block_len,
            settle_blocks,
            capture_blocks,
            calibration_frequency
                .map(|freq| format!("{:.0} Hz", freq))
                .unwrap_or_else(|| "unknown".to_string())
        );

        let result = (|| {
            if let Some(freq) = calibration_frequency {
                dev.set_frequency(Direction::Rx, rx_ch, freq, Args::new())
                    .map_err(|err| format!("could not tune RX to loopback calibration frequency: {}", err))?;
                dev.set_frequency(Direction::Tx, tx_ch, freq, Args::new())
                    .map_err(|err| format!("could not tune TX to loopback calibration frequency: {}", err))?;
            }

            dev.set_antenna(Direction::Rx, rx_ch, "LB")
                .map_err(|err| format!("could not select RX LB antenna: {}", err))?;

            let rx_args = args_from_pairs(rx_args);
            let tx_args = args_from_pairs(tx_args);
            let mut rx = dev
                .rx_stream_args::<ComplexSample, _>(&[rx_ch], rx_args)
                .map_err(|err| format!("could not setup RX calibration stream: {}", err))?;
            let mut tx = dev
                .tx_stream_args::<ComplexSample, _>(&[tx_ch], tx_args)
                .map_err(|err| format!("could not setup TX calibration stream: {}", err))?;

            rx.activate(None)
                .map_err(|err| format!("could not activate RX calibration stream: {}", err))?;
            tx.activate(None)
                .map_err(|err| format!("could not activate TX calibration stream: {}", err))?;

            let tone_pos_block = make_tone_block(block_len, tone, rx_sample_rate as RealSample, amplitude);
            let tone_neg_block = make_tone_block(block_len, -tone, rx_sample_rate as RealSample, amplitude);
            let zero_block = vec![ComplexSample { re: 0.0, im: 0.0 }; block_len];
            let floor_blocks = (capture_blocks.max(4) / 2).max(4);

            let capture_result = (|| {
                let floor_before_samples = capture_loopback_blocks(&mut rx, &mut tx, &zero_block, settle_blocks, floor_blocks, block_len)
                    .map_err(|err| format!("floor-before capture failed: {}", err))?;

                let tone_pos_samples = capture_loopback_blocks(&mut rx, &mut tx, &tone_pos_block, settle_blocks, capture_blocks, block_len)
                    .map_err(|err| format!("positive-tone capture failed: {}", err))?;

                let floor_after_samples = capture_loopback_blocks(&mut rx, &mut tx, &zero_block, settle_blocks, floor_blocks, block_len)
                    .map_err(|err| format!("floor-after capture failed: {}", err))?;

                let tone_neg_samples = capture_loopback_blocks(&mut rx, &mut tx, &tone_neg_block, settle_blocks, capture_blocks, block_len)
                    .map_err(|err| format!("negative-tone capture failed: {}", err))?;

                compute_loopback_compensation(
                    &tone_pos_samples,
                    &tone_neg_samples,
                    &floor_before_samples,
                    &floor_after_samples,
                    tone,
                    rx_sample_rate as RealSample,
                    block_len,
                    &self.cfg,
                )
            })();

            tx.write_all(&[&zero_block], None, false, 200_000).ok();
            tx.deactivate(None).ok();
            rx.deactivate(None).ok();

            capture_result
        })();

        if let Err(err) = dev.set_antenna(Direction::Rx, rx_ch, original_rx_antenna.as_str()) {
            tracing::warn!(
                "SX1255 autocal: failed to restore RX antenna '{}' after calibration: {}",
                original_rx_antenna,
                err
            );
        }
        if let Some(freq) = original_rx_frequency {
            if let Err(err) = dev.set_frequency(Direction::Rx, rx_ch, freq, Args::new()) {
                tracing::warn!("SX1255 autocal: failed to restore RX frequency after calibration: {}", err);
            }
        }
        if let Some(freq) = original_tx_frequency {
            if let Err(err) = dev.set_frequency(Direction::Tx, tx_ch, freq, Args::new()) {
                tracing::warn!("SX1255 autocal: failed to restore TX frequency after calibration: {}", err);
            }
        }

        result
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

fn args_from_pairs(pairs: &[(String, String)]) -> Args {
    let mut args = Args::new();
    for (key, value) in pairs {
        args.set(key.as_str(), value.as_str());
    }
    args
}

fn stream_period_samples(args: &[(String, String)]) -> Option<usize> {
    args.iter()
        .find(|(key, _)| key == "period")
        .and_then(|(_, value)| value.parse::<usize>().ok())
}

fn quantized_tone_hz(requested_hz: f64, sample_rate: f64, block_len: usize) -> f64 {
    let bin_hz = sample_rate / block_len as f64;
    let max_bin = block_len.saturating_div(2).saturating_sub(1).max(1) as f64;
    let bin = (requested_hz / bin_hz).round().clamp(1.0, max_bin);
    bin * bin_hz
}

fn make_tone_block(block_len: usize, tone_hz: f64, sample_rate: RealSample, amplitude: RealSample) -> Vec<ComplexSample> {
    let phase_step = std::f32::consts::TAU * tone_hz as RealSample / sample_rate;
    (0..block_len)
        .map(|idx| {
            let phase = phase_step * idx as RealSample;
            ComplexSample {
                re: amplitude * phase.cos(),
                im: amplitude * phase.sin(),
            }
        })
        .collect()
}

fn capture_loopback_blocks(
    rx: &mut soapysdr::RxStream<ComplexSample>,
    tx: &mut soapysdr::TxStream<ComplexSample>,
    tx_block: &[ComplexSample],
    settle_blocks: usize,
    capture_blocks: usize,
    block_len: usize,
) -> Result<Vec<ComplexSample>, String> {
    let mut rx_block = vec![ComplexSample { re: 0.0, im: 0.0 }; block_len];
    let mut captured = Vec::with_capacity(capture_blocks * block_len);

    for _ in 0..LOOPBACK_TX_PREFILL_BLOCKS {
        tx.write_all(&[tx_block], None, false, 200_000)
            .map_err(|err| format!("TX prefill failed: {}", err))?;
    }

    for block_idx in 0..(settle_blocks + capture_blocks) {
        tx.write_all(&[tx_block], None, false, 200_000)
            .map_err(|err| format!("TX write failed: {}", err))?;
        read_full(rx, &mut rx_block, 200_000)?;
        if block_idx >= settle_blocks {
            captured.extend_from_slice(&rx_block);
        }
    }

    Ok(captured)
}

fn read_full(rx: &mut soapysdr::RxStream<ComplexSample>, out: &mut [ComplexSample], timeout_us: i64) -> Result<(), String> {
    let mut offset = 0;
    while offset < out.len() {
        let len = rx
            .read(&mut [&mut out[offset..]], timeout_us)
            .map_err(|err| format!("RX read failed: {}", err))?;
        if len == 0 {
            return Err("RX read returned no samples".to_string());
        }
        offset += len;
    }
    Ok(())
}

fn compute_loopback_compensation(
    tone_pos_samples: &[ComplexSample],
    tone_neg_samples: &[ComplexSample],
    floor_before_samples: &[ComplexSample],
    floor_after_samples: &[ComplexSample],
    tone_hz: f64,
    sample_rate: RealSample,
    block_len: usize,
    cfg: &CfgSx1255Autocal,
) -> Result<RxStartupCompensation, String> {
    if tone_pos_samples.is_empty() || tone_neg_samples.is_empty() || floor_before_samples.is_empty() || floor_after_samples.is_empty() {
        return Err("empty calibration capture".to_string());
    }

    let floor_dc = mean_complex_pair(floor_before_samples, floor_after_samples);
    let floor_before_mag = floor_bin_magnitude(floor_before_samples, floor_dc, tone_hz, sample_rate);
    let floor_after_mag = floor_bin_magnitude(floor_after_samples, floor_dc, tone_hz, sample_rate);
    let floor_mag = floor_before_mag.max(floor_after_mag).max(1.0e-9);
    let floor_before_rms = centered_rms(floor_before_samples, floor_dc).max(1.0e-9);
    let floor_after_rms = centered_rms(floor_after_samples, floor_dc).max(1.0e-9);
    let floor_drift_db = ratio_db(floor_after_rms, floor_before_rms);

    let pos_measurement = measure_loopback_tone(tone_pos_samples, floor_dc, tone_hz, sample_rate, block_len, true, floor_mag);
    let neg_measurement = measure_loopback_tone(tone_neg_samples, floor_dc, tone_hz, sample_rate, block_len, false, floor_mag);

    let tone_mag = pos_measurement.main_mag.min(neg_measurement.main_mag);
    let snr_db = pos_measurement.snr_db.min(neg_measurement.snr_db);
    let image_dbc = pos_measurement.image_dbc.max(neg_measurement.image_dbc);
    let max_component_abs = pos_measurement.max_component_abs.max(neg_measurement.max_component_abs);
    let clipped_fraction = pos_measurement.clipped_fraction.max(neg_measurement.clipped_fraction);

    tracing::info!(
        "SX1255 autocal: loopback measured +tone={:.6}/{}({}/{}) -tone={:.6}/{}({}/{}) floor={:.6} snr={:.1} dB image={:.1} dBc dc=({:+.6},{:+.6}) floor_drift={:+.1} dB clip={:.4}/{:.3}",
        pos_measurement.main_mag,
        pos_measurement.orientation.label(),
        pos_measurement.good_blocks,
        pos_measurement.total_blocks,
        neg_measurement.main_mag,
        neg_measurement.orientation.label(),
        neg_measurement.good_blocks,
        neg_measurement.total_blocks,
        floor_mag,
        snr_db,
        image_dbc,
        floor_dc.re,
        floor_dc.im,
        floor_drift_db,
        clipped_fraction,
        max_component_abs
    );

    tracing::debug!(
        "SX1255 autocal: loopback detail +image={:.1} dBc +coeff=({:+.6},{:+.6}) -image={:.1} dBc -coeff=({:+.6},{:+.6}) floor_before={:.6} floor_after={:.6}",
        pos_measurement.image_dbc,
        pos_measurement.image_coeff.re,
        pos_measurement.image_coeff.im,
        neg_measurement.image_dbc,
        neg_measurement.image_coeff.re,
        neg_measurement.image_coeff.im,
        floor_before_mag,
        floor_after_mag
    );

    if tone_mag <= 1.0e-9 || !tone_mag.is_finite() || !snr_db.is_finite() || snr_db < cfg.rf_loopback_min_snr_db as RealSample {
        return Err(format!(
            "calibration tone SNR {:.1} dB below threshold {:.1} dB",
            snr_db, cfg.rf_loopback_min_snr_db
        ));
    }

    let dc_abs = complex_abs(floor_dc);
    let apply_dc = cfg.rf_loopback_apply_dc && dc_abs <= cfg.rf_loopback_max_dc as RealSample;
    if cfg.rf_loopback_apply_dc && !apply_dc {
        tracing::warn!(
            "SX1255 autocal: measured DC magnitude {:.6} exceeds limit {:.6}; DC correction disabled",
            dc_abs,
            cfg.rf_loopback_max_dc
        );
    }

    let mut image_coeff = ComplexSample { re: 0.0, im: 0.0 };
    let mut apply_iq = false;
    if cfg.rf_loopback_apply_iq {
        let coeff_weight = pos_measurement.main_mag + neg_measurement.main_mag;
        if coeff_weight > 1.0e-9 {
            image_coeff = (pos_measurement.image_coeff * pos_measurement.main_mag + neg_measurement.image_coeff * neg_measurement.main_mag)
                / coeff_weight;
        }

        let image_coeff_abs = complex_abs(image_coeff);
        let coeff_delta = complex_abs(pos_measurement.image_coeff - neg_measurement.image_coeff);
        let coeff_delta_limit = IQ_COEFF_ABS_TOLERANCE + IQ_COEFF_REL_TOLERANCE * image_coeff_abs;
        let tone_delta_db = ratio_db(pos_measurement.main_mag, neg_measurement.main_mag).abs();
        let (pos_corrected_main, pos_corrected_image, pos_corrected_image_dbc) = corrected_tone_metrics(&pos_measurement, image_coeff);
        let (neg_corrected_main, neg_corrected_image, neg_corrected_image_dbc) = corrected_tone_metrics(&neg_measurement, image_coeff);
        let corrected_image_dbc = pos_corrected_image_dbc.max(neg_corrected_image_dbc);
        let image_improvement_db = image_dbc - corrected_image_dbc;
        let main_retained_ratio =
            (pos_corrected_main / pos_measurement.main_mag.max(1.0e-9)).min(neg_corrected_main / neg_measurement.main_mag.max(1.0e-9));
        let same_orientation = pos_measurement.orientation == neg_measurement.orientation;
        let clipped = clipped_fraction > IQ_MAX_CLIPPED_FRACTION;
        let floor_unstable = floor_drift_db.abs() > IQ_FLOOR_DRIFT_DISABLE_DB;

        tracing::info!(
            "SX1255 autocal: IQ candidate coeff=({:+.6},{:+.6}) mag={:.6} delta={:.6}/{:.6} corrected_image={:.1} dBc improvement={:.1} dB main_retained={:.2}",
            image_coeff.re,
            image_coeff.im,
            image_coeff_abs,
            coeff_delta,
            coeff_delta_limit,
            corrected_image_dbc,
            image_improvement_db,
            main_retained_ratio
        );
        tracing::debug!(
            "SX1255 autocal: IQ candidate corrected detail +main={:.6} +image={:.6} -main={:.6} -image={:.6}",
            pos_corrected_main,
            pos_corrected_image,
            neg_corrected_main,
            neg_corrected_image
        );

        apply_iq = image_coeff_abs.is_finite()
            && image_coeff_abs <= cfg.rf_loopback_max_image_coeff as RealSample
            && coeff_delta <= coeff_delta_limit
            && tone_delta_db <= IQ_MAX_TONE_DELTA_DB
            && same_orientation
            && !clipped
            && !floor_unstable
            && image_improvement_db >= IQ_MIN_IMAGE_IMPROVEMENT_DB
            && main_retained_ratio >= IQ_MIN_MAIN_RETAINED_RATIO;

        if !apply_iq {
            tracing::warn!(
                "SX1255 autocal: IQ correction disabled (coeff_mag={:.6} limit={:.6}, coeff_delta={:.6} limit={:.6}, tone_delta={:.1} dB, orientation=+{} / -{}, floor_drift={:+.1} dB, clipped={} ({:.4}), improvement={:.1} dB, main_retained={:.2})",
                image_coeff_abs,
                cfg.rf_loopback_max_image_coeff,
                coeff_delta,
                coeff_delta_limit,
                tone_delta_db,
                pos_measurement.orientation.label(),
                neg_measurement.orientation.label(),
                floor_drift_db,
                clipped,
                clipped_fraction,
                image_improvement_db,
                main_retained_ratio
            );
        }
    }

    Ok(RxStartupCompensation {
        dc: if apply_dc { floor_dc } else { ComplexSample { re: 0.0, im: 0.0 } },
        image_coeff: if apply_iq {
            image_coeff
        } else {
            ComplexSample { re: 0.0, im: 0.0 }
        },
        apply_dc,
        apply_iq,
    })
}

fn mean_complex_pair(a: &[ComplexSample], b: &[ComplexSample]) -> ComplexSample {
    let mut sum = ComplexSample { re: 0.0, im: 0.0 };
    for sample in a {
        sum += *sample;
    }
    for sample in b {
        sum += *sample;
    }
    sum / (a.len() + b.len()) as RealSample
}

fn floor_bin_magnitude(samples: &[ComplexSample], dc: ComplexSample, tone_hz: f64, sample_rate: RealSample) -> RealSample {
    complex_abs(tone_bin_centered(samples, dc, tone_hz, sample_rate, false)).max(complex_abs(tone_bin_centered(
        samples,
        dc,
        tone_hz,
        sample_rate,
        true,
    )))
}

fn centered_rms(samples: &[ComplexSample], dc: ComplexSample) -> RealSample {
    let mut sum = 0.0;
    for sample in samples {
        let centered = *sample - dc;
        sum += centered.re * centered.re + centered.im * centered.im;
    }
    (sum / samples.len() as RealSample).sqrt()
}

fn max_component_abs(samples: &[ComplexSample]) -> RealSample {
    let mut max_abs: RealSample = 0.0;
    for sample in samples {
        max_abs = max_abs.max(sample.re.abs()).max(sample.im.abs());
    }
    max_abs
}

fn count_clipped_samples(samples: &[ComplexSample]) -> usize {
    samples
        .iter()
        .filter(|sample| sample.re.abs() >= IQ_CLIP_LEVEL || sample.im.abs() >= IQ_CLIP_LEVEL)
        .count()
}

fn clipped_fraction(samples: &[ComplexSample]) -> RealSample {
    if samples.is_empty() {
        0.0
    } else {
        count_clipped_samples(samples) as RealSample / samples.len() as RealSample
    }
}

fn ratio_db(a: RealSample, b: RealSample) -> RealSample {
    20.0 * (a.max(1.0e-9) / b.max(1.0e-9)).log10()
}

fn measure_loopback_tone(
    samples: &[ComplexSample],
    dc: ComplexSample,
    tone_hz: f64,
    sample_rate: RealSample,
    block_len: usize,
    transmitted_positive_tone: bool,
    floor_mag: RealSample,
) -> ToneMeasurement {
    let bins = tone_bins_centered_robust(samples, dc, tone_hz, sample_rate, block_len);
    let positive_bin = bins.positive_bin;
    let negative_bin = bins.negative_bin;
    let positive_mag = complex_abs(positive_bin);
    let negative_mag = complex_abs(negative_bin);

    let (main_is_positive_bin, orientation) = if transmitted_positive_tone {
        if positive_mag >= negative_mag {
            (true, ToneOrientation::Normal)
        } else {
            (false, ToneOrientation::Inverted)
        }
    } else if negative_mag >= positive_mag {
        (false, ToneOrientation::Normal)
    } else {
        (true, ToneOrientation::Inverted)
    };

    let (main, image, main_mag, image_mag) = if main_is_positive_bin {
        (positive_bin, negative_bin, positive_mag, negative_mag)
    } else {
        (negative_bin, positive_bin, negative_mag, positive_mag)
    };
    let snr_db = ratio_db(main_mag, floor_mag);
    let image_dbc = ratio_db(image_mag, main_mag);
    let image_coeff = if main_mag > 1.0e-9 && main_mag.is_finite() {
        -image / main.conj()
    } else {
        ComplexSample { re: 0.0, im: 0.0 }
    };

    ToneMeasurement {
        orientation,
        main_is_positive_bin,
        positive_bin,
        negative_bin,
        main_mag,
        snr_db,
        image_dbc,
        image_coeff,
        max_component_abs: bins.max_component_abs,
        clipped_fraction: bins.clipped_fraction,
        good_blocks: bins.good_blocks,
        total_blocks: bins.total_blocks,
    }
}

fn tone_bins_centered_robust(
    samples: &[ComplexSample],
    dc: ComplexSample,
    tone_hz: f64,
    sample_rate: RealSample,
    block_len: usize,
) -> ToneBins {
    if block_len == 0 || samples.len() < block_len {
        return ToneBins {
            positive_bin: tone_bin_centered(samples, dc, tone_hz, sample_rate, false),
            negative_bin: tone_bin_centered(samples, dc, tone_hz, sample_rate, true),
            max_component_abs: max_component_abs(samples),
            clipped_fraction: clipped_fraction(samples),
            good_blocks: 1,
            total_blocks: 1,
        };
    }

    let mut positive_sum = ComplexSample { re: 0.0, im: 0.0 };
    let mut negative_sum = ComplexSample { re: 0.0, im: 0.0 };
    let mut good_blocks = 0;
    let mut total_blocks = 0;
    let mut max_abs: RealSample = 0.0;
    let mut clipped_samples = 0usize;
    let mut total_samples = 0usize;

    for block in samples.chunks(block_len) {
        if block.len() != block_len {
            continue;
        }
        total_blocks += 1;
        total_samples += block.len();
        let block_max = max_component_abs(block);
        max_abs = max_abs.max(block_max);
        clipped_samples += count_clipped_samples(block);
        if block_max >= IQ_CLIP_LEVEL {
            continue;
        }
        positive_sum += tone_bin_centered(block, dc, tone_hz, sample_rate, false);
        negative_sum += tone_bin_centered(block, dc, tone_hz, sample_rate, true);
        good_blocks += 1;
    }

    let clipped_ratio = if total_samples == 0 {
        0.0
    } else {
        clipped_samples as RealSample / total_samples as RealSample
    };

    if good_blocks == 0 {
        return ToneBins {
            positive_bin: tone_bin_centered(samples, dc, tone_hz, sample_rate, false),
            negative_bin: tone_bin_centered(samples, dc, tone_hz, sample_rate, true),
            max_component_abs: max_abs,
            clipped_fraction: clipped_ratio,
            good_blocks,
            total_blocks,
        };
    }

    ToneBins {
        positive_bin: positive_sum / good_blocks as RealSample,
        negative_bin: negative_sum / good_blocks as RealSample,
        max_component_abs: max_abs,
        clipped_fraction: clipped_ratio,
        good_blocks,
        total_blocks,
    }
}

fn corrected_tone_metrics(measurement: &ToneMeasurement, image_coeff: ComplexSample) -> (RealSample, RealSample, RealSample) {
    let corrected_positive = measurement.positive_bin + image_coeff * measurement.negative_bin.conj();
    let corrected_negative = measurement.negative_bin + image_coeff * measurement.positive_bin.conj();
    let (main, image) = if measurement.main_is_positive_bin {
        (corrected_positive, corrected_negative)
    } else {
        (corrected_negative, corrected_positive)
    };
    let main_mag = complex_abs(main);
    let image_mag = complex_abs(image);
    (main_mag, image_mag, ratio_db(image_mag, main_mag))
}

fn tone_bin_centered(samples: &[ComplexSample], dc: ComplexSample, tone_hz: f64, sample_rate: RealSample, negative: bool) -> ComplexSample {
    let sign = if negative { 1.0 } else { -1.0 };
    let phase_step = sign * std::f32::consts::TAU * tone_hz as RealSample / sample_rate;
    let mut sum = ComplexSample { re: 0.0, im: 0.0 };
    for (idx, sample) in samples.iter().enumerate() {
        let phase = phase_step * idx as RealSample;
        let reference = ComplexSample {
            re: phase.cos(),
            im: phase.sin(),
        };
        sum += (*sample - dc) * reference;
    }
    sum / samples.len() as RealSample
}

fn complex_abs(value: ComplexSample) -> RealSample {
    (value.re * value.re + value.im * value.im).sqrt()
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

    #[test]
    fn loopback_iq_calibration_accepts_consistent_dual_tone_measurement() {
        let cfg = iq_test_cfg();
        let dc = ComplexSample { re: 0.02, im: -0.03 };
        let coeff = ComplexSample { re: 0.08, im: -0.04 };
        let floor = synthetic_floor(dc, 0.0);
        let pos = synthetic_tone_capture(true, false, dc, coeff);
        let neg = synthetic_tone_capture(false, false, dc, coeff);

        let compensation = compute_loopback_compensation(&pos, &neg, &floor, &floor, TEST_TONE_HZ, TEST_SAMPLE_RATE, TEST_BLOCK_LEN, &cfg)
            .expect("valid calibration");

        assert!(compensation.apply_dc);
        assert!(compensation.apply_iq);
        assert_complex_close(compensation.dc, dc, 1.0e-5);
        assert_complex_close(compensation.image_coeff, coeff, 1.0e-5);
    }

    #[test]
    fn loopback_iq_calibration_accepts_inverted_loopback_orientation() {
        let cfg = iq_test_cfg();
        let dc = ComplexSample { re: -0.01, im: 0.015 };
        let coeff = ComplexSample { re: 0.32, im: 0.11 };
        let floor = synthetic_floor(dc, 0.0);
        let pos = synthetic_tone_capture(true, true, dc, coeff);
        let neg = synthetic_tone_capture(false, true, dc, coeff);

        let compensation = compute_loopback_compensation(&pos, &neg, &floor, &floor, TEST_TONE_HZ, TEST_SAMPLE_RATE, TEST_BLOCK_LEN, &cfg)
            .expect("valid inverted calibration");

        assert!(compensation.apply_iq);
        assert_complex_close(compensation.image_coeff, coeff, 1.0e-5);
    }

    #[test]
    fn loopback_iq_calibration_rejects_unstable_floor() {
        let mut cfg = iq_test_cfg();
        cfg.rf_loopback_min_snr_db = 10.0;
        let dc = ComplexSample { re: 0.0, im: 0.0 };
        let coeff = ComplexSample { re: 0.08, im: 0.02 };
        let floor_before = synthetic_floor(dc, 0.0);
        let floor_after = synthetic_floor(dc, 0.03);
        let pos = synthetic_tone_capture(true, false, dc, coeff);
        let neg = synthetic_tone_capture(false, false, dc, coeff);

        let compensation = compute_loopback_compensation(
            &pos,
            &neg,
            &floor_before,
            &floor_after,
            TEST_TONE_HZ,
            TEST_SAMPLE_RATE,
            TEST_BLOCK_LEN,
            &cfg,
        )
        .expect("dc calibration can still complete");

        assert!(compensation.apply_dc);
        assert!(!compensation.apply_iq);
    }

    const TEST_SAMPLE_RATE: RealSample = 600_000.0;
    const TEST_TONE_HZ: f64 = 24_000.0;
    const TEST_BLOCK_LEN: usize = 900;
    const TEST_BLOCKS: usize = 6;

    fn iq_test_cfg() -> CfgSx1255Autocal {
        CfgSx1255Autocal {
            enabled: true,
            rf_loopback_apply_dc: true,
            rf_loopback_apply_iq: true,
            rf_loopback_min_snr_db: 20.0,
            rf_loopback_max_image_coeff: 0.95,
            rf_loopback_max_dc: 0.5,
            ..CfgSx1255Autocal::default()
        }
    }

    fn synthetic_floor(dc: ComplexSample, tone_leakage: RealSample) -> Vec<ComplexSample> {
        (0..TEST_BLOCK_LEN * TEST_BLOCKS)
            .map(|idx| {
                let phase = std::f32::consts::TAU * TEST_TONE_HZ as RealSample * idx as RealSample / TEST_SAMPLE_RATE;
                dc + ComplexSample {
                    re: tone_leakage * phase.cos(),
                    im: tone_leakage * phase.sin(),
                }
            })
            .collect()
    }

    fn synthetic_tone_capture(
        transmitted_positive_tone: bool,
        inverted_orientation: bool,
        dc: ComplexSample,
        correction_coeff: ComplexSample,
    ) -> Vec<ComplexSample> {
        let main = ComplexSample { re: 0.25, im: 0.04 };
        let image = -correction_coeff * main.conj();
        let (positive_bin, negative_bin) = match (transmitted_positive_tone, inverted_orientation) {
            (true, false) => (main, image),
            (false, false) => (image, main),
            (true, true) => (image, main),
            (false, true) => (main, image),
        };

        (0..TEST_BLOCK_LEN * TEST_BLOCKS)
            .map(|idx| {
                let phase = std::f32::consts::TAU * TEST_TONE_HZ as RealSample * idx as RealSample / TEST_SAMPLE_RATE;
                let positive_ref = ComplexSample {
                    re: phase.cos(),
                    im: phase.sin(),
                };
                let negative_ref = positive_ref.conj();
                dc + positive_bin * positive_ref + negative_bin * negative_ref
            })
            .collect()
    }

    fn assert_complex_close(actual: ComplexSample, expected: ComplexSample, tolerance: RealSample) {
        assert!(
            complex_abs(actual - expected) <= tolerance,
            "actual=({:+.6},{:+.6}) expected=({:+.6},{:+.6})",
            actual.re,
            actual.im,
            expected.re,
            expected.im
        );
    }
}
