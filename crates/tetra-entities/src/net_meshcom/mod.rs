//! MeshCom external UDP integration.
//!
//! MeshCom nodes can expose an external-client UDP interface that exchanges JSON packets,
//! commonly on UDP/1799. FlowStation listens for received `msg`, `pos`, and `tele` packets
//! and keeps a small runtime directory/log for the dashboard. Outbound text messages are sent
//! by the dashboard API using the same documented JSON format.

use std::collections::{BTreeSet, VecDeque};
use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use tetra_config::bluestation::{
    CfgMeshcom, MeshcomMessageStatus, MeshcomNodeStatus, MeshcomRuntimeStatus, SharedConfig,
};

use crate::net_control::commands::ControlCommand;
use crate::net_snom::SnomNotifySink;
use crate::net_telegram::TelegramAlertSink;
use crate::tpg2200::build_sds_text_payload;

type CmdSender = crossbeam_channel::Sender<ControlCommand>;

const UDP_READ_TIMEOUT: Duration = Duration::from_secs(1);
const DISABLED_SLEEP: Duration = Duration::from_secs(1);
const ERROR_SLEEP: Duration = Duration::from_secs(5);

pub fn spawn_meshcom_worker(
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    snom_sink: Option<SnomNotifySink>,
) -> Option<thread::JoinHandle<()>> {
    match thread::Builder::new()
        .name("meshcom-worker".into())
        .spawn(move || MeshcomWorker::new(cfg, cmce_cmd_tx, telegram_sink, snom_sink).run())
    {
        Ok(handle) => Some(handle),
        Err(err) => {
            tracing::warn!("MeshCom: failed to spawn worker thread: {}", err);
            None
        }
    }
}

struct MeshcomWorker {
    cfg: SharedConfig,
    cmce_cmd_tx: Option<CmdSender>,
    telegram_sink: Option<TelegramAlertSink>,
    snom_sink: Option<SnomNotifySink>,
    socket: Option<UdpSocket>,
    bind_key: String,
    rx_packets: u64,
    nodes: Vec<MeshcomNodeStatus>,
    messages: VecDeque<MeshcomMessageStatus>,
    last_enabled: Option<bool>,
}

impl MeshcomWorker {
    fn new(
        cfg: SharedConfig,
        cmce_cmd_tx: Option<CmdSender>,
        telegram_sink: Option<TelegramAlertSink>,
        snom_sink: Option<SnomNotifySink>,
    ) -> Self {
        Self {
            cfg,
            cmce_cmd_tx,
            telegram_sink,
            snom_sink,
            socket: None,
            bind_key: String::new(),
            rx_packets: 0,
            nodes: Vec::new(),
            messages: VecDeque::new(),
            last_enabled: None,
        }
    }

    fn run(&mut self) {
        loop {
            let meshcom = self.cfg.effective_meshcom();
            if !meshcom.enabled {
                if self.last_enabled != Some(false) {
                    tracing::info!("MeshCom UDP integration disabled");
                    self.last_enabled = Some(false);
                }
                self.socket = None;
                self.bind_key.clear();
                self.publish_status(&meshcom, None, None);
                thread::sleep(DISABLED_SLEEP);
                continue;
            }

            if self.last_enabled != Some(true) {
                tracing::info!(
                    "MeshCom UDP integration enabled (bind={}:{} tx={}:{} forward_sds={} forward_sip={} forward_telegram={})",
                    meshcom.bind_addr,
                    meshcom.bind_port,
                    meshcom.tx_host,
                    meshcom.tx_port,
                    meshcom.forward_sds,
                    meshcom.forward_sip,
                    meshcom.forward_telegram
                );
                self.last_enabled = Some(true);
            }

            if let Err(err) = self.ensure_socket(&meshcom) {
                tracing::warn!("MeshCom: {}", err);
                self.publish_status(&meshcom, None, Some(err));
                thread::sleep(ERROR_SLEEP);
                continue;
            }

            let mut buf = [0u8; 8192];
            let Some(socket) = self.socket.as_ref() else {
                thread::sleep(ERROR_SLEEP);
                continue;
            };
            let recv = socket.recv_from(&mut buf);

            match recv {
                Ok((len, from)) => {
                    let text = String::from_utf8_lossy(&buf[..len]).trim().to_string();
                    match self.handle_packet(&meshcom, &text) {
                        Ok(()) => {
                            tracing::debug!("MeshCom: received {} bytes from {}", len, from);
                            self.publish_status(&meshcom, Some(now_stamp()), None);
                        }
                        Err(err) => {
                            tracing::warn!("MeshCom: dropping invalid UDP packet from {}: {}", from, err);
                            self.publish_status(&meshcom, None, Some(err));
                        }
                    }
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut => {}
                Err(err) => {
                    let msg = format!("UDP receive failed: {err}");
                    tracing::warn!("MeshCom: {}", msg);
                    self.publish_status(&meshcom, None, Some(msg));
                    self.socket = None;
                    self.bind_key.clear();
                    thread::sleep(ERROR_SLEEP);
                }
            }
        }
    }

    fn ensure_socket(&mut self, meshcom: &CfgMeshcom) -> Result<(), String> {
        let key = format!("{}:{}", meshcom.bind_addr.trim(), meshcom.bind_port);
        if self.socket.is_some() && self.bind_key == key {
            return Ok(());
        }

        self.socket = None;
        let socket = UdpSocket::bind(&key).map_err(|e| format!("UDP bind {key} failed: {e}"))?;
        socket
            .set_read_timeout(Some(UDP_READ_TIMEOUT))
            .map_err(|e| format!("UDP set_read_timeout failed: {e}"))?;
        if let Err(err) = socket.set_broadcast(meshcom.allow_broadcast) {
            tracing::warn!(
                "MeshCom: failed to set UDP broadcast={} on {}: {}",
                meshcom.allow_broadcast,
                key,
                err
            );
        }
        self.bind_key = key;
        self.socket = Some(socket);
        self.publish_status(meshcom, None, None);
        Ok(())
    }

    fn handle_packet(&mut self, meshcom: &CfgMeshcom, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Err("empty packet".to_string());
        }
        let value: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
        let msg_type = string_field(&value, "type").unwrap_or_else(|| "unknown".to_string());
        let src = string_field(&value, "src");
        let dst = string_field(&value, "dst");
        let src_type = string_field(&value, "src_type");
        let msg = string_field(&value, "msg").map(|s| truncate_chars(&s, 512));
        let msg_id = string_field(&value, "msg_id");
        let lat = signed_coord(f64_field(&value, "lat"), string_field(&value, "lat_dir").as_deref());
        let lon = signed_coord(f64_field(&value, "long"), string_field(&value, "long_dir").as_deref());
        let alt = f64_field(&value, "alt");
        let batt = f64_field(&value, "batt");
        let rssi = i64_field(&value, "rssi");
        let snr = i64_field(&value, "snr");
        let firmware = string_field(&value, "firmware");
        let fw_sub = string_field(&value, "fw_sub");
        let hw_id = string_field(&value, "hw_id");
        let ts = now_stamp();
        let paths = self.forward_message(
            meshcom,
            &msg_type,
            src.as_deref(),
            dst.as_deref(),
            msg.as_deref(),
            msg_id.as_deref(),
        );

        self.rx_packets = self.rx_packets.saturating_add(1);
        let event = MeshcomMessageStatus {
            ts: ts.clone(),
            direction: "rx".to_string(),
            msg_type: msg_type.clone(),
            src_type,
            src: src.clone(),
            dst,
            msg,
            msg_id,
            paths,
            lat,
            lon,
            alt,
            batt,
            rssi,
            snr,
        };

        if let Some(source) = src {
            self.upsert_node(
                meshcom,
                MeshcomNodeStatus {
                    src: source,
                    last_seen: ts,
                    last_type: msg_type,
                    lat,
                    lon,
                    alt,
                    batt,
                    rssi,
                    snr,
                    firmware,
                    fw_sub,
                    hw_id,
                },
            );
        }
        self.messages.push_front(event);
        while self.messages.len() > meshcom.max_messages {
            self.messages.pop_back();
        }
        Ok(())
    }

    fn upsert_node(&mut self, meshcom: &CfgMeshcom, update: MeshcomNodeStatus) {
        if let Some(node) = self.nodes.iter_mut().find(|node| node.src == update.src) {
            node.last_seen = update.last_seen;
            node.last_type = update.last_type;
            if update.lat.is_some() {
                node.lat = update.lat;
            }
            if update.lon.is_some() {
                node.lon = update.lon;
            }
            if update.alt.is_some() {
                node.alt = update.alt;
            }
            if update.batt.is_some() {
                node.batt = update.batt;
            }
            if update.rssi.is_some() {
                node.rssi = update.rssi;
            }
            if update.snr.is_some() {
                node.snr = update.snr;
            }
            if update.firmware.is_some() {
                node.firmware = update.firmware;
            }
            if update.fw_sub.is_some() {
                node.fw_sub = update.fw_sub;
            }
            if update.hw_id.is_some() {
                node.hw_id = update.hw_id;
            }
            return;
        }
        self.nodes.push(update);
        while self.nodes.len() > meshcom.max_nodes {
            self.nodes.remove(0);
        }
    }

    fn forward_message(
        &self,
        meshcom: &CfgMeshcom,
        msg_type: &str,
        src: Option<&str>,
        dst: Option<&str>,
        msg: Option<&str>,
        msg_id: Option<&str>,
    ) -> Vec<String> {
        let mut paths = Vec::new();
        if !msg_type.eq_ignore_ascii_case("msg") {
            return paths;
        }
        let Some(text) = msg.map(str::trim).filter(|s| !s.is_empty()) else {
            return paths;
        };
        let src = src.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("unknown");

        if meshcom.forward_sds && source_allowed(&meshcom.sds_allowed_sources, src) {
            match self.forward_sds(meshcom, src, text) {
                Ok(()) => paths.push("sds".to_string()),
                Err(err) => tracing::warn!("MeshCom: SDS forwarding failed src={}: {}", src, err),
            }
        }
        if meshcom.forward_sip && source_allowed(&meshcom.sip_allowed_sources, src) {
            match self.forward_sip(meshcom, src, dst, text, msg_id) {
                Ok(()) => paths.push("sip".to_string()),
                Err(err) => tracing::warn!("MeshCom: SIP notify forwarding failed src={}: {}", src, err),
            }
        }
        if meshcom.forward_telegram && source_allowed(&meshcom.telegram_allowed_sources, src) {
            match self.forward_telegram(meshcom, src, text) {
                Ok(()) => paths.push("telegram".to_string()),
                Err(err) => tracing::warn!("MeshCom: Telegram forwarding failed src={}: {}", src, err),
            }
        }

        if paths.is_empty()
            && (meshcom.forward_sds || meshcom.forward_sip || meshcom.forward_telegram)
        {
            tracing::info!(
                "MeshCom: received message src={} dst={} with no successful forwarding target",
                src,
                dst.unwrap_or("-")
            );
        } else if !paths.is_empty() {
            tracing::info!(
                "MeshCom: forwarded message src={} dst={} paths={}",
                src,
                dst.unwrap_or("-"),
                paths.join(",")
            );
        }
        paths
    }

    fn forward_sds(&self, meshcom: &CfgMeshcom, src: &str, text: &str) -> Result<(), String> {
        if meshcom.sds_dest_issi == 0 {
            return Err("sds_dest_issi is 0".to_string());
        }
        let Some(tx) = &self.cmce_cmd_tx else {
            return Err("CMCE control sender unavailable".to_string());
        };
        let body = format_plain_meshcom_message(src, text);
        let (len_bits, payload) = build_sds_text_payload(&body);
        tx.send(ControlCommand::SendSds {
            handle: 0,
            source_ssi: meshcom.sds_source_issi,
            dest_ssi: meshcom.sds_dest_issi,
            dest_is_group: meshcom.sds_dest_is_group,
            len_bits,
            payload,
        })
        .map_err(|e| format!("send to CMCE failed: {}", e))
    }

    fn forward_sip(
        &self,
        meshcom: &CfgMeshcom,
        src: &str,
        dst: Option<&str>,
        text: &str,
        msg_id: Option<&str>,
    ) -> Result<(), String> {
        let Some(sink) = &self.snom_sink else {
            return Err("Snom notify sink unavailable".to_string());
        };
        sink.send_meshcom(
            meshcom.sip_title_prefix.clone(),
            src.to_string(),
            dst.map(ToString::to_string),
            text.to_string(),
            msg_id.map(ToString::to_string),
        );
        Ok(())
    }

    fn forward_telegram(&self, meshcom: &CfgMeshcom, src: &str, text: &str) -> Result<(), String> {
        let Some(sink) = &self.telegram_sink else {
            return Err("Telegram alert sink unavailable".to_string());
        };
        sink.send_meshcom(
            meshcom.telegram_prefix.clone(),
            src.to_string(),
            text.to_string(),
        );
        Ok(())
    }

    fn publish_status(
        &self,
        meshcom: &CfgMeshcom,
        last_rx: Option<String>,
        last_error: Option<String>,
    ) {
        let mut state = self.cfg.state_write();
        let previous_tx_packets = state.meshcom_status.tx_packets;
        let previous_last_tx = state.meshcom_status.last_tx.clone();
        let previous_last_rx = state.meshcom_status.last_rx.clone();
        let mut messages: Vec<MeshcomMessageStatus> = self.messages.iter().cloned().collect();
        for msg in state
            .meshcom_status
            .messages
            .iter()
            .filter(|msg| msg.direction == "tx")
        {
            if messages.len() >= meshcom.max_messages {
                break;
            }
            messages.push(msg.clone());
        }
        state.meshcom_status = MeshcomRuntimeStatus {
            configured: true,
            enabled: meshcom.enabled,
            bind: format!("{}:{}", meshcom.bind_addr, meshcom.bind_port),
            tx: format!("{}:{}", meshcom.tx_host, meshcom.tx_port),
            rx_packets: self.rx_packets,
            tx_packets: previous_tx_packets,
            last_rx: last_rx.or(previous_last_rx),
            last_tx: previous_last_tx,
            last_error,
            forward_sds: meshcom.forward_sds,
            forward_sip: meshcom.forward_sip,
            forward_telegram: meshcom.forward_telegram,
            nodes: self.nodes.clone(),
            messages,
        };
    }
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn f64_field(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
    })
}

fn i64_field(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_u64().and_then(|n| i64::try_from(n).ok()))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
    })
}

fn signed_coord(value: Option<f64>, dir: Option<&str>) -> Option<f64> {
    let mut value = value?;
    if matches!(dir.map(|d| d.trim().to_ascii_uppercase()), Some(d) if d == "S" || d == "W") {
        value = -value.abs();
    }
    Some(value)
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

fn source_allowed(allowed: &BTreeSet<String>, src: &str) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let src = src.trim().to_ascii_uppercase();
    allowed.contains(&src)
}

fn format_plain_meshcom_message(src: &str, text: &str) -> String {
    format!("MeshCom: {} - {}", src.trim(), text.trim())
}
