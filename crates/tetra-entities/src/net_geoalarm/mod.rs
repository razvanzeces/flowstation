//! GeoAlarm geofence worker.
//!
//! The worker is intentionally small and off the real-time TETRA path. TETRA LIP and MeshCom
//! sources feed decoded coordinates into this channel. When a device enters the configured
//! radius, the worker forwards through the existing SDS, TPG2200, Snom/SIP and Telegram paths.

use std::collections::{HashMap, VecDeque};
use std::thread;
use std::time::{Duration, Instant};

use tetra_config::bluestation::{CfgGeoalarm, GeoalarmEventStatus, GeoalarmRuntimeStatus, SharedConfig};

use crate::net_control::commands::ControlCommand;
use crate::net_snom::SnomNotifySink;
use crate::net_telegram::TelegramAlertSink;
use crate::tpg2200::{build_sds_text_payload, build_tpg2200_callout_payload, format_hex_bytes};

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

const STATUS_TICK: Duration = Duration::from_secs(1);
const MAX_EVENTS: usize = 250;

#[derive(Debug, Clone)]
enum GeoAlarmSource {
    Tetra { issi: u32 },
    Meshcom { src: String },
}

impl GeoAlarmSource {
    fn label(&self) -> String {
        match self {
            Self::Tetra { issi } => format!("TETRA {issi}"),
            Self::Meshcom { src } => format!("MeshCom {src}"),
        }
    }

    fn key(&self) -> String {
        match self {
            Self::Tetra { issi } => format!("tetra:{issi}"),
            Self::Meshcom { src } => format!("meshcom:{}", src.trim().to_ascii_uppercase()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Tetra { .. } => "tetra",
            Self::Meshcom { .. } => "meshcom",
        }
    }
}

#[derive(Debug, Clone)]
struct GeoAlarmUpdate {
    source: GeoAlarmSource,
    lat: f64,
    lon: f64,
}

#[derive(Clone)]
pub struct GeoAlarmSink {
    tx: crossbeam_channel::Sender<GeoAlarmUpdate>,
}

impl GeoAlarmSink {
    #[inline]
    pub fn send_tetra_lip(&self, source_issi: u32, text: &str) {
        if let Some((lat, lon)) = parse_lip_position_text(text) {
            self.send_tetra_position(source_issi, lat, lon);
        }
    }

    #[inline]
    pub fn send_tetra_position(&self, source_issi: u32, lat: f64, lon: f64) {
        let _ = self.tx.send(GeoAlarmUpdate {
            source: GeoAlarmSource::Tetra { issi: source_issi },
            lat,
            lon,
        });
    }

    #[inline]
    pub fn send_meshcom_position(&self, src: String, lat: f64, lon: f64) {
        let src = src.trim();
        if src.is_empty() {
            return;
        }
        let _ = self.tx.send(GeoAlarmUpdate {
            source: GeoAlarmSource::Meshcom { src: src.to_string() },
            lat,
            lon,
        });
    }
}

fn geoalarm_channel() -> (GeoAlarmSink, crossbeam_channel::Receiver<GeoAlarmUpdate>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    (GeoAlarmSink { tx }, rx)
}

pub fn spawn_geoalarm_worker(
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    snom_sink: Option<SnomNotifySink>,
) -> Option<GeoAlarmSink> {
    let (sink, rx) = geoalarm_channel();
    match thread::Builder::new()
        .name("geoalarm-worker".into())
        .spawn(move || GeoAlarmWorker::new(cfg, cmce_cmd_tx, telegram_sink, snom_sink, rx).run())
    {
        Ok(_handle) => Some(sink),
        Err(err) => {
            tracing::warn!("GeoAlarm: failed to spawn worker thread: {}", err);
            None
        }
    }
}

#[derive(Debug, Clone)]
struct DeviceState {
    inside: bool,
    last_alarm: Option<Instant>,
}

struct GeoAlarmWorker {
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    snom_sink: Option<SnomNotifySink>,
    rx: crossbeam_channel::Receiver<GeoAlarmUpdate>,
    devices: HashMap<String, DeviceState>,
    events: VecDeque<GeoalarmEventStatus>,
    seen_positions: u64,
    alarm_count: u64,
    last_position: Option<String>,
    last_alarm: Option<String>,
    last_error: Option<String>,
    last_enabled: Option<bool>,
    next_tpg2200_callout_id: u16,
    last_callout_id_base: Option<u16>,
}

impl GeoAlarmWorker {
    fn new(
        cfg: SharedConfig,
        cmce_cmd_tx: Option<CmdSender>,
        telegram_sink: Option<TelegramAlertSink>,
        snom_sink: Option<SnomNotifySink>,
        rx: crossbeam_channel::Receiver<GeoAlarmUpdate>,
    ) -> Self {
        let next_tpg2200_callout_id = cfg.effective_geoalarm().tpg2200_incident_base.min(255);
        Self {
            cfg,
            cmce_cmd_tx,
            telegram_sink,
            snom_sink,
            rx,
            devices: HashMap::new(),
            events: VecDeque::new(),
            seen_positions: 0,
            alarm_count: 0,
            last_position: None,
            last_alarm: None,
            last_error: None,
            last_enabled: None,
            next_tpg2200_callout_id,
            last_callout_id_base: None,
        }
    }

    fn run(&mut self) {
        loop {
            let geoalarm = self.cfg.effective_geoalarm();
            self.note_config_state(&geoalarm);
            match self.rx.recv_timeout(STATUS_TICK) {
                Ok(update) => self.handle_update(&geoalarm, update),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            self.publish_status(&geoalarm);
        }
        tracing::info!("GeoAlarm worker exiting");
    }

    fn note_config_state(&mut self, geoalarm: &CfgGeoalarm) {
        if self.last_enabled == Some(geoalarm.enabled) {
            return;
        }
        if geoalarm.enabled {
            tracing::info!(
                "GeoAlarm enabled (center={:.6},{:.6} radius={:.0}m tetra={} meshcom={} tpg2200={} sds={} sip={} telegram={})",
                geoalarm.flowstation_lat,
                geoalarm.flowstation_lon,
                geoalarm.radius_m,
                geoalarm.trigger_tetra,
                geoalarm.trigger_meshcom,
                geoalarm.forward_tpg2200,
                geoalarm.forward_sds,
                geoalarm.forward_sip,
                geoalarm.forward_telegram
            );
            if !(geoalarm.forward_tpg2200 || geoalarm.forward_sds || geoalarm.forward_sip || geoalarm.forward_telegram) {
                tracing::warn!("GeoAlarm: enabled but no forwarding target is enabled");
            }
        } else {
            tracing::info!("GeoAlarm disabled");
        }
        self.last_enabled = Some(geoalarm.enabled);
    }

    fn handle_update(&mut self, geoalarm: &CfgGeoalarm, update: GeoAlarmUpdate) {
        if !geoalarm.enabled {
            return;
        }
        if !coord_valid(update.lat, update.lon) {
            self.set_error(format!(
                "invalid coordinates from {}: {},{}",
                update.source.label(),
                update.lat,
                update.lon
            ));
            return;
        }
        if !source_enabled(geoalarm, &update.source) || !source_allowed(geoalarm, &update.source) {
            return;
        }

        self.seen_positions = self.seen_positions.saturating_add(1);
        let distance = haversine_m(geoalarm.flowstation_lat, geoalarm.flowstation_lon, update.lat, update.lon);
        let inside = distance <= geoalarm.radius_m;
        let label = update.source.label();
        let stamp = now_stamp();
        self.last_position = Some(format!("{} {:.0}m ({:.6},{:.6})", label, distance, update.lat, update.lon));

        let key = update.source.key();
        let cooldown = Duration::from_secs(geoalarm.cooldown_secs.max(1));
        let (was_inside, cooldown_elapsed) = {
            let state = self.devices.entry(key.clone()).or_insert(DeviceState {
                inside: false,
                last_alarm: None,
            });
            let cooldown_elapsed = state.last_alarm.map(|last| last.elapsed() >= cooldown).unwrap_or(true);
            let was_inside = state.inside;
            state.inside = inside;
            (was_inside, cooldown_elapsed)
        };
        let should_alarm = inside && (!was_inside || cooldown_elapsed);

        let mut paths = Vec::new();
        if should_alarm {
            paths = self.forward_alarm(geoalarm, &update.source, update.lat, update.lon, distance);
            if !paths.is_empty() {
                self.alarm_count = self.alarm_count.saturating_add(1);
                self.last_alarm = Some(format!(
                    "{} via {}",
                    self.last_position.clone().unwrap_or_default(),
                    paths.join(",")
                ));
                if let Some(state) = self.devices.get_mut(&key) {
                    state.last_alarm = Some(Instant::now());
                }
            }
        }

        self.events.push_front(GeoalarmEventStatus {
            ts: stamp,
            source: update.source.kind().to_string(),
            device: label,
            lat: update.lat,
            lon: update.lon,
            distance_m: distance,
            inside_radius: inside,
            alarmed: should_alarm && !paths.is_empty(),
            paths,
        });
        while self.events.len() > MAX_EVENTS {
            self.events.pop_back();
        }
    }

    fn forward_alarm(&mut self, geoalarm: &CfgGeoalarm, source: &GeoAlarmSource, lat: f64, lon: f64, distance_m: f64) -> Vec<String> {
        let mut paths = Vec::new();
        let label = source.label();
        let body = format!("{} {:.0}m ({:.6},{:.6})", label, distance_m.max(0.0), lat, lon);

        if geoalarm.forward_tpg2200 {
            match self.forward_tpg2200(geoalarm, &body) {
                Ok(()) => paths.push("tpg2200".to_string()),
                Err(err) => self.warn_forward("TPG2200", &label, err),
            }
        }
        if geoalarm.forward_sds {
            match self.forward_sds(geoalarm, &body) {
                Ok(()) => paths.push("sds".to_string()),
                Err(err) => self.warn_forward("SDS", &label, err),
            }
        }
        if geoalarm.forward_sip {
            match self.forward_sip(geoalarm, &label, &body, distance_m, lat, lon) {
                Ok(()) => paths.push("sip".to_string()),
                Err(err) => self.warn_forward("SIP", &label, err),
            }
        }
        if geoalarm.forward_telegram {
            match self.forward_telegram(geoalarm, &label, &body) {
                Ok(()) => paths.push("telegram".to_string()),
                Err(err) => self.warn_forward("Telegram", &label, err),
            }
        }

        if paths.is_empty() {
            tracing::info!("GeoAlarm: {} entered radius but no forwarding target succeeded", label);
        } else {
            tracing::info!(
                "GeoAlarm: {} entered radius ({:.0}m <= {:.0}m), paths={}",
                label,
                distance_m,
                geoalarm.radius_m,
                paths.join(",")
            );
            self.last_error = None;
        }
        paths
    }

    fn forward_tpg2200(&mut self, geoalarm: &CfgGeoalarm, body: &str) -> Result<(), String> {
        if geoalarm.tpg2200_dest_issi == 0 {
            return Err("tpg2200_dest_issi is 0".to_string());
        }
        let Some(tx) = self.cmce_cmd_tx.clone() else {
            return Err("CMCE control sender unavailable".to_string());
        };
        let callout_id_base = geoalarm.tpg2200_incident_base.min(255);
        if self.last_callout_id_base != Some(callout_id_base) {
            self.next_tpg2200_callout_id = callout_id_base;
            self.last_callout_id_base = Some(callout_id_base);
        }
        let callout_id = self.next_callout_id();
        let priority = geoalarm
            .tpg2200_issi_priorities
            .get(&geoalarm.tpg2200_dest_issi)
            .or_else(|| geoalarm.tpg2200_ric_priorities.get(&geoalarm.tpg2200_ric))
            .copied()
            .unwrap_or(geoalarm.tpg2200_priority)
            .min(15);
        let text = prefixed_text(&geoalarm.tpg2200_text_prefix, body);
        let (text, truncated) = truncate_chars(&text, geoalarm.tpg2200_max_text_chars);
        if truncated {
            tracing::warn!("GeoAlarm: TPG2200 text truncated to {} chars", geoalarm.tpg2200_max_text_chars);
        }
        let payload = build_tpg2200_callout_payload(geoalarm.tpg2200_ric, callout_id, priority, &text);
        tracing::debug!(
            "GeoAlarm: TPG2200 tpg_ric={:08X} callout_id={} priority={} dest={} payload=[{}]",
            geoalarm.tpg2200_ric,
            callout_id,
            priority,
            geoalarm.tpg2200_dest_issi,
            format_hex_bytes(&payload)
        );
        tx.send(ControlCommand::SendRawSdsType4 {
            handle: 0,
            source_ssi: geoalarm.tpg2200_source_issi,
            dest_ssi: geoalarm.tpg2200_dest_issi,
            dest_is_group: false,
            len_bits: (payload.len() * 8) as u16,
            payload,
        })
        .map_err(|e| format!("send to CMCE failed: {}", e))
    }

    fn forward_sds(&self, geoalarm: &CfgGeoalarm, body: &str) -> Result<(), String> {
        if geoalarm.sds_dest_issi == 0 {
            return Err("sds_dest_issi is 0".to_string());
        }
        let Some(tx) = &self.cmce_cmd_tx else {
            return Err("CMCE control sender unavailable".to_string());
        };
        let text = prefixed_text("GeoAlarm:", body);
        let (len_bits, payload) = build_sds_text_payload(&text);
        tx.send(ControlCommand::SendSds {
            handle: 0,
            source_ssi: geoalarm.sds_source_issi,
            dest_ssi: geoalarm.sds_dest_issi,
            dest_is_group: geoalarm.sds_dest_is_group,
            len_bits,
            payload,
        })
        .map_err(|e| format!("send to CMCE failed: {}", e))
    }

    fn forward_sip(&self, geoalarm: &CfgGeoalarm, source: &str, body: &str, distance_m: f64, lat: f64, lon: f64) -> Result<(), String> {
        let Some(sink) = &self.snom_sink else {
            return Err("Snom notify sink unavailable".to_string());
        };
        sink.send_geoalarm(
            geoalarm.sip_title_prefix.clone(),
            source.to_string(),
            body.to_string(),
            distance_m,
            lat,
            lon,
        );
        Ok(())
    }

    fn forward_telegram(&self, geoalarm: &CfgGeoalarm, source: &str, body: &str) -> Result<(), String> {
        let Some(sink) = &self.telegram_sink else {
            return Err("Telegram alert sink unavailable".to_string());
        };
        sink.send_geoalarm(geoalarm.telegram_prefix.clone(), source.to_string(), body.to_string());
        Ok(())
    }

    fn warn_forward(&mut self, path: &str, source: &str, err: String) {
        let msg = format!("{path} forwarding failed for {source}: {err}");
        tracing::warn!("GeoAlarm: {}", msg);
        self.set_error(msg);
    }

    fn set_error(&mut self, msg: String) {
        self.last_error = Some(msg);
    }

    fn next_callout_id(&mut self) -> u16 {
        let callout_id = self.next_tpg2200_callout_id.min(255);
        self.next_tpg2200_callout_id = if callout_id >= 255 { 0 } else { callout_id + 1 };
        callout_id
    }

    fn publish_status(&self, geoalarm: &CfgGeoalarm) {
        let mut state = self.cfg.state_write();
        state.geoalarm_status = GeoalarmRuntimeStatus {
            configured: true,
            enabled: geoalarm.enabled,
            center: format!("{:.6},{:.6}", geoalarm.flowstation_lat, geoalarm.flowstation_lon),
            radius_m: geoalarm.radius_m,
            trigger_tetra: geoalarm.trigger_tetra,
            trigger_meshcom: geoalarm.trigger_meshcom,
            forward_tpg2200: geoalarm.forward_tpg2200,
            forward_sds: geoalarm.forward_sds,
            forward_sip: geoalarm.forward_sip,
            forward_telegram: geoalarm.forward_telegram,
            seen_positions: self.seen_positions,
            alarm_count: self.alarm_count,
            last_position: self.last_position.clone(),
            last_alarm: self.last_alarm.clone(),
            last_error: self.last_error.clone(),
            events: self.events.iter().cloned().collect(),
        };
    }
}

fn source_enabled(geoalarm: &CfgGeoalarm, source: &GeoAlarmSource) -> bool {
    match source {
        GeoAlarmSource::Tetra { .. } => geoalarm.trigger_tetra,
        GeoAlarmSource::Meshcom { .. } => geoalarm.trigger_meshcom,
    }
}

fn source_allowed(geoalarm: &CfgGeoalarm, source: &GeoAlarmSource) -> bool {
    match source {
        GeoAlarmSource::Tetra { issi } => {
            !geoalarm.tetra_issi_blacklist.contains(issi)
                && (geoalarm.tetra_issi_whitelist.is_empty() || geoalarm.tetra_issi_whitelist.contains(issi))
        }
        GeoAlarmSource::Meshcom { src } => {
            let src = src.trim().to_ascii_uppercase();
            !geoalarm.meshcom_source_blacklist.contains(&src)
                && (geoalarm.meshcom_source_whitelist.is_empty() || geoalarm.meshcom_source_whitelist.contains(&src))
        }
    }
}

fn coord_valid(lat: f64, lon: f64) -> bool {
    lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}

pub fn parse_lip_position_text(text: &str) -> Option<(f64, f64)> {
    let rest = text.trim().strip_prefix("LIP position:")?.trim();
    let (lat, lon) = rest.split_once(',')?;
    let lat = lat.trim().parse::<f64>().ok()?;
    let lon = lon.trim().parse::<f64>().ok()?;
    if coord_valid(lat, lon) { Some((lat, lon)) } else { None }
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let phi1 = lat1.to_radians();
    let phi2 = lat2.to_radians();
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

fn prefixed_text(prefix: &str, text: &str) -> String {
    let prefix = prefix.trim();
    let text = text.trim();
    if prefix.is_empty() {
        text.to_string()
    } else if text.is_empty() {
        prefix.to_string()
    } else if prefix.ends_with(':') {
        format!("{prefix} {text}")
    } else {
        format!("{prefix} {text}")
    }
}

fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        (text.to_string(), false)
    } else {
        (text.chars().take(max).collect(), true)
    }
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
