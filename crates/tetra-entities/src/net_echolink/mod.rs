//! EchoLink UDP/GSM bridge for simplex P2MP voice calls.

#[cfg(feature = "echolink")]
mod audio;
#[cfg(not(feature = "echolink"))]
#[path = "audio_stub.rs"]
mod audio;

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use tetra_config::bluestation::{
    CfgEcholink, EcholinkDirectoryStationStatus, EcholinkRuntimeStatus, SharedConfig, normalize_echolink_target,
};
use tetra_core::{Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_pdus::cmce::enums::call_timeout::CallTimeout;
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::call_control::{CallControl, NetworkCircuitCall},
    control::enums::communication_type::CommunicationType,
    tmd::{TmdCircuitDataInd, TmdCircuitDataReq},
};
use uuid::Uuid;

use crate::net_telegram::TelegramAlertSink;
use crate::{MessageQueue, TetraEntityTrait};

use self::audio::{ECHOLINK_GSM_FRAME_BYTES, ECHOLINK_GSM_PACKET_BYTES, EcholinkAudioTranscoder};

const RTP_VERSION_ECHOLINK: u8 = 3;
const RTCP_RR: u8 = 201;
const RTCP_SDES: u8 = 202;
const RTCP_BYE: u8 = 203;
const RTCP_SDES_END: u8 = 0;
const RTCP_SDES_CNAME: u8 = 1;
const RTCP_SDES_NAME: u8 = 2;
const RTCP_SDES_EMAIL: u8 = 3;
const RTCP_SDES_PHONE: u8 = 4;
const ECHOLINK_RTP_GSM_PT: u8 = 0x03;
const ECHOLINK_RTP_HEADER: usize = 12;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const AUDIO_RX_ACTIVITY_TIMEOUT: Duration = Duration::from_millis(350);
const AUDIO_RX_SETUP_TIMEOUT: Duration = Duration::from_secs(1);
const DIRECTORY_TIMEOUT: Duration = Duration::from_secs(3);
const DIRECTORY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const DIRECTORY_DESCRIPTION_MAX_CHARS: usize = 27;

#[derive(Debug, Clone)]
pub enum EcholinkCommand {
    Connect { target: String },
    Disconnect,
}

#[derive(Debug)]
enum EcholinkDirectoryEvent {
    Online {
        generation: u64,
        callsign: String,
        stations: Vec<DirectoryStation>,
    },
    Error {
        generation: u64,
        message: String,
    },
}

pub type EcholinkCmdSender = crossbeam_channel::Sender<EcholinkCommand>;
pub type EcholinkCmdReceiver = crossbeam_channel::Receiver<EcholinkCommand>;

pub fn echolink_channel() -> (EcholinkCmdSender, EcholinkCmdReceiver) {
    crossbeam_channel::unbounded()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QsoState {
    Connecting,
    Connected,
    Released,
}

struct EcholinkDialog {
    uuid: Option<Uuid>,
    call: Option<NetworkCircuitCall>,
    target: String,
    remote_call: String,
    remote_ip: IpAddr,
    remote_audio: SocketAddr,
    remote_control: SocketAddr,
    state: QsoState,
    audio: EcholinkAudioTranscoder,
    media_ready: Option<(u16, u16, u8)>,
    remote_floor_active: bool,
    remote_floor_ready: bool,
    remote_floor_since: Option<Instant>,
    seq: u16,
    inbound: bool,
    started: Instant,
    last_sdes: Instant,
    last_audio_rx: Option<Instant>,
}

#[derive(Clone, Debug)]
struct DirectoryStation {
    callsign: String,
    id: u32,
    ip: IpAddr,
}

pub struct EcholinkEntity {
    config: SharedConfig,
    cmd_rx: EcholinkCmdReceiver,
    audio_socket: Option<UdpSocket>,
    control_socket: Option<UdpSocket>,
    audio_bind: Option<String>,
    control_bind: Option<String>,
    dialogs: Vec<EcholinkDialog>,
    dialog_by_slot: std::collections::HashMap<(u16, u8), usize>,
    last_enabled: Option<bool>,
    last_directory_status: String,
    directory_stations: Vec<DirectoryStation>,
    directory_stations_dirty: bool,
    directory_event_tx: crossbeam_channel::Sender<EcholinkDirectoryEvent>,
    directory_event_rx: crossbeam_channel::Receiver<EcholinkDirectoryEvent>,
    directory_stop_tx: Option<crossbeam_channel::Sender<()>>,
    directory_generation: u64,
    directory_config_key: Option<String>,
    last_rx: Option<String>,
    last_tx: Option<String>,
    last_error: Option<String>,
    last_session_event: Option<String>,
    telegram_sink: Option<TelegramAlertSink>,
}

impl EcholinkEntity {
    pub fn new(config: SharedConfig, cmd_rx: EcholinkCmdReceiver, telegram_sink: Option<TelegramAlertSink>) -> Self {
        let (directory_event_tx, directory_event_rx) = crossbeam_channel::unbounded();
        let mut entity = Self {
            config,
            cmd_rx,
            audio_socket: None,
            control_socket: None,
            audio_bind: None,
            control_bind: None,
            dialogs: Vec::new(),
            dialog_by_slot: std::collections::HashMap::new(),
            last_enabled: None,
            last_directory_status: "disabled".to_string(),
            directory_stations: Vec::new(),
            directory_stations_dirty: true,
            directory_event_tx,
            directory_event_rx,
            directory_stop_tx: None,
            directory_generation: 0,
            directory_config_key: None,
            last_rx: None,
            last_tx: None,
            last_error: None,
            last_session_event: None,
            telegram_sink,
        };
        entity.refresh_status();
        entity
    }

    fn effective(&self) -> CfgEcholink {
        self.config.effective_echolink()
    }

    fn refresh_status(&mut self) {
        let cfg = self.effective();
        let connected = self.dialogs.iter().find(|d| d.state != QsoState::Released).map(|d| {
            if d.remote_call.is_empty() {
                d.target.clone()
            } else {
                d.remote_call.clone()
            }
        });
        let qso_status = if self.dialogs.iter().any(|d| d.state == QsoState::Connected) {
            "connected"
        } else if self.dialogs.iter().any(|d| d.state == QsoState::Connecting) {
            "connecting"
        } else {
            "idle"
        };
        let directory_stations = self.directory_stations_dirty.then(|| {
            self.directory_stations
                .iter()
                .map(|s| EcholinkDirectoryStationStatus {
                    callsign: s.callsign.clone(),
                    id: s.id,
                    ip: s.ip.to_string(),
                })
                .collect::<Vec<_>>()
        });
        if directory_stations.is_some() {
            self.directory_stations_dirty = false;
        }
        let mut state = self.config.state_write();
        let directory_stations = match directory_stations {
            Some(stations) => stations,
            None => std::mem::take(&mut state.echolink_status.directory_stations),
        };
        state.echolink_status = EcholinkRuntimeStatus {
            configured: true,
            enabled: cfg.enabled,
            directory_status: self.last_directory_status.clone(),
            qso_status: qso_status.to_string(),
            bind: format!("{}:{}/{}", cfg.bind_addr, cfg.audio_port, cfg.control_port),
            callsign: cfg.callsign.clone(),
            connected_target: connected,
            routed_tetra_dest: route_label(&cfg),
            last_session_event: self.last_session_event.clone(),
            last_rx: self.last_rx.clone(),
            last_tx: self.last_tx.clone(),
            last_error: self.last_error.clone(),
            directory_stations,
        };
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::warn!("EchoLink: {}", msg);
        self.last_error = Some(msg);
    }

    fn ensure_ports(&mut self, cfg: &CfgEcholink) -> Result<(), String> {
        let audio_bind = format!("{}:{}", cfg.bind_addr, cfg.audio_port);
        if self.audio_bind.as_deref() != Some(audio_bind.as_str()) {
            self.audio_socket = None;
            self.audio_bind = None;
        }
        if self.audio_socket.is_none() {
            let socket = UdpSocket::bind(&audio_bind).map_err(|e| format!("audio UDP bind {} failed: {}", audio_bind, e))?;
            socket
                .set_nonblocking(true)
                .map_err(|e| format!("audio UDP nonblocking failed: {}", e))?;
            self.audio_socket = Some(socket);
            self.audio_bind = Some(audio_bind);
        }
        let control_bind = format!("{}:{}", cfg.bind_addr, cfg.control_port);
        if self.control_bind.as_deref() != Some(control_bind.as_str()) {
            self.control_socket = None;
            self.control_bind = None;
        }
        if self.control_socket.is_none() {
            let socket = UdpSocket::bind(&control_bind).map_err(|e| format!("control UDP bind {} failed: {}", control_bind, e))?;
            socket
                .set_nonblocking(true)
                .map_err(|e| format!("control UDP nonblocking failed: {}", e))?;
            self.control_socket = Some(socket);
            self.control_bind = Some(control_bind);
        }
        Ok(())
    }

    fn release_ports(&mut self) {
        self.audio_socket = None;
        self.control_socket = None;
        self.audio_bind = None;
        self.control_bind = None;
    }

    fn handle_dashboard_commands(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink) {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                EcholinkCommand::Connect { target } => {
                    let target = normalize_echolink_target(&target);
                    if target.is_empty() {
                        self.set_error("connect target is empty");
                        continue;
                    }
                    if !target_allowed(cfg, &target, None, &self.directory_stations) {
                        self.set_error(format!("target {target} is not allowed by EchoLink routing"));
                        continue;
                    }
                    match self.resolve_target(cfg, &target) {
                        Ok(ip) => {
                            let Some(audio) = EcholinkAudioTranscoder::new() else {
                                self.set_error("EchoLink GSM/TETRA codec allocation failed");
                                continue;
                            };
                            let dialog = EcholinkDialog {
                                uuid: None,
                                call: None,
                                target: target.clone(),
                                remote_call: target.clone(),
                                remote_ip: ip,
                                remote_audio: SocketAddr::new(ip, cfg.audio_port),
                                remote_control: SocketAddr::new(ip, cfg.control_port),
                                state: QsoState::Connecting,
                                audio,
                                media_ready: None,
                                remote_floor_active: false,
                                remote_floor_ready: false,
                                remote_floor_since: None,
                                seq: 1,
                                inbound: false,
                                started: Instant::now(),
                                last_sdes: Instant::now() - KEEPALIVE_INTERVAL,
                                last_audio_rx: None,
                            };
                            self.dialogs.push(dialog);
                            let idx = self.dialogs.len() - 1;
                            self.send_sdes_idx(idx, cfg);
                            self.last_tx = Some(format!("connect requested to {target} ({ip})"));
                            tracing::info!("EchoLink: connect requested to {} ({})", target, ip);
                        }
                        Err(err) => self.set_error(err),
                    }
                }
                EcholinkCommand::Disconnect => {
                    self.disconnect_all(queue, true);
                    self.last_tx = Some("disconnect requested".to_string());
                }
            }
        }
    }

    fn start_outbound_call(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink, brew_uuid: Uuid, call: NetworkCircuitCall) {
        if !cfg.outbound_enabled {
            self.reject_setup(queue, brew_uuid, 34);
            return;
        }
        let call = echolink_simplex_p2mp_call(call);
        let target = normalize_echolink_target(&call.number);
        if target.is_empty() {
            self.set_error(format!("empty EchoLink target for uuid={}", brew_uuid));
            self.reject_setup(queue, brew_uuid, 34);
            return;
        }
        if !target_allowed(cfg, &target, None, &self.directory_stations) {
            self.set_error(format!("target {target} is not allowed by EchoLink routing"));
            self.reject_setup(queue, brew_uuid, 34);
            return;
        }
        let remote_ip = match self.resolve_target(cfg, &target) {
            Ok(ip) => ip,
            Err(err) => {
                self.set_error(err);
                self.reject_setup(queue, brew_uuid, 34);
                return;
            }
        };
        let Some(audio) = EcholinkAudioTranscoder::new() else {
            self.set_error(format!("EchoLink codec allocation failed for uuid={}", brew_uuid));
            self.reject_setup(queue, brew_uuid, 34);
            return;
        };

        let dialog = EcholinkDialog {
            uuid: Some(brew_uuid),
            call: Some(call),
            target: target.clone(),
            remote_call: target.clone(),
            remote_ip,
            remote_audio: SocketAddr::new(remote_ip, cfg.audio_port),
            remote_control: SocketAddr::new(remote_ip, cfg.control_port),
            state: QsoState::Connecting,
            audio,
            media_ready: None,
            remote_floor_active: false,
            remote_floor_ready: false,
            remote_floor_since: None,
            seq: 1,
            inbound: false,
            started: Instant::now(),
            last_sdes: Instant::now() - KEEPALIVE_INTERVAL,
            last_audio_rx: None,
        };
        self.dialogs.push(dialog);
        let idx = self.dialogs.len() - 1;
        self.send_setup_accept(queue, brew_uuid);
        self.send_sdes_idx(idx, cfg);
        self.last_tx = Some(format!("SETUP {} to EchoLink {}", brew_uuid, target));
        tracing::info!("EchoLink: outbound setup uuid={} target={} ip={}", brew_uuid, target, remote_ip);
    }

    fn reject_setup(&self, queue: &mut MessageQueue, brew_uuid: Uuid, cause: u8) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }),
        });
    }

    fn send_setup_accept(&self, queue: &mut MessageQueue, brew_uuid: Uuid) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }),
        });
    }

    fn send_alert(&self, queue: &mut MessageQueue, brew_uuid: Uuid) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }),
        });
    }

    fn send_release_to_cmce(&self, queue: &mut MessageQueue, brew_uuid: Uuid, cause: u8) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }),
        });
    }

    fn send_group_end_to_cmce(&self, queue: &mut MessageQueue, brew_uuid: Uuid) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
        });
    }

    fn request_group_floor_idx(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink, idx: usize, reason: &str) -> bool {
        if idx >= self.dialogs.len() {
            return false;
        }
        if !cfg.inbound_enabled || cfg.default_tetra_dest_issi == 0 {
            self.set_error("EchoLink received audio but no inbound TETRA route is configured");
            return false;
        }
        if !cfg.default_tetra_dest_is_group {
            self.set_error("EchoLink received audio but simplex/P2MP requires default_tetra_dest_is_group=true");
            return false;
        }
        if self.dialogs[idx].call.is_some() {
            return false;
        }

        let now = Instant::now();
        let (uuid, remote_call, remote_audio, remote_control) = {
            let dialog = &mut self.dialogs[idx];
            if dialog.remote_floor_active {
                return true;
            }
            let uuid = dialog.uuid.unwrap_or_else(Uuid::new_v4);
            dialog.uuid = Some(uuid);
            dialog.remote_floor_active = true;
            dialog.remote_floor_ready = false;
            dialog.remote_floor_since = Some(now);
            (uuid, dialog.remote_call.clone(), dialog.remote_audio, dialog.remote_control)
        };

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Echolink,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                brew_uuid: uuid,
                source_issi: cfg.default_tetra_source_issi,
                dest_gssi: cfg.default_tetra_dest_issi,
                priority: 0,
            }),
        });
        tracing::info!(
            "EchoLink: {} remote={} audio={} control={} -> GSSI {} source={} uuid={}",
            reason,
            remote_call,
            remote_audio,
            remote_control,
            cfg.default_tetra_dest_issi,
            cfg.default_tetra_source_issi,
            uuid
        );
        true
    }

    fn release_remote_floor_idx(&mut self, queue: &mut MessageQueue, idx: usize, reason: &str) {
        if idx >= self.dialogs.len() {
            return;
        }
        let (uuid, remote_call, media_ready) = {
            let dialog = &mut self.dialogs[idx];
            if dialog.call.is_some() || !dialog.remote_floor_active {
                return;
            }
            dialog.remote_floor_active = false;
            dialog.remote_floor_ready = false;
            dialog.remote_floor_since = None;
            dialog.last_audio_rx = None;
            (dialog.uuid.take(), dialog.remote_call.clone(), dialog.media_ready.take())
        };

        if let Some((_, carrier_num, ts)) = media_ready {
            self.dialog_by_slot.remove(&(carrier_num, ts));
        }
        if let Some(uuid) = uuid {
            self.send_group_end_to_cmce(queue, uuid);
            if let Some((call_id, carrier_num, ts)) = media_ready {
                tracing::info!(
                    "EchoLink: {} remote={} -> releasing TETRA floor uuid={} call_id={} carrier={} ts={}",
                    reason,
                    remote_call,
                    uuid,
                    call_id,
                    carrier_num,
                    ts
                );
            } else {
                tracing::info!(
                    "EchoLink: {} remote={} -> cancelling pending TETRA floor uuid={}",
                    reason,
                    remote_call,
                    uuid
                );
            }
        }
    }

    fn maybe_connect_dialog(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink, idx: usize) {
        enum ConnectAction {
            ConfirmTetraOriginated(Uuid, NetworkCircuitCall),
            StartGroupLeg {
                uuid: Uuid,
                source_issi: u32,
                dest_gssi: u32,
                remote_call: String,
            },
            Release(String),
            None,
        }

        let action = {
            let Some(dialog) = self.dialogs.get_mut(idx) else {
                return;
            };
            if dialog.state == QsoState::Connected {
                return;
            }
            dialog.state = QsoState::Connected;

            if let (Some(uuid), Some(call)) = (dialog.uuid, dialog.call.clone()) {
                ConnectAction::ConfirmTetraOriginated(uuid, echolink_simplex_p2mp_call(call))
            } else if dialog.uuid.is_none() {
                if !cfg.inbound_enabled || cfg.default_tetra_dest_issi == 0 {
                    ConnectAction::Release("no inbound TETRA route configured for EchoLink dashboard connect".to_string())
                } else if !cfg.default_tetra_dest_is_group {
                    ConnectAction::Release("EchoLink simplex/P2MP requires default_tetra_dest_is_group=true".to_string())
                } else {
                    let uuid = Uuid::new_v4();
                    let remote_call = dialog.remote_call.clone();
                    dialog.uuid = Some(uuid);
                    ConnectAction::StartGroupLeg {
                        uuid,
                        source_issi: cfg.default_tetra_source_issi,
                        dest_gssi: cfg.default_tetra_dest_issi,
                        remote_call,
                    }
                }
            } else {
                ConnectAction::None
            }
        };

        let notify_connected = matches!(
            &action,
            ConnectAction::ConfirmTetraOriginated(_, _) | ConnectAction::StartGroupLeg { .. }
        );
        let connected_remote = self.dialogs.get(idx).map(|dialog| dialog.remote_call.clone()).unwrap_or_default();

        match action {
            ConnectAction::ConfirmTetraOriginated(uuid, call) => {
                self.send_alert(queue, uuid);
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Echolink,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid: uuid, call }),
                });
            }
            ConnectAction::StartGroupLeg {
                uuid,
                source_issi,
                dest_gssi,
                remote_call,
            } => {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Echolink,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                        brew_uuid: uuid,
                        source_issi,
                        dest_gssi,
                        priority: 0,
                    }),
                });
                tracing::info!("EchoLink: dashboard QSO {} -> GSSI {}", remote_call, dest_gssi);
            }
            ConnectAction::Release(reason) => {
                self.set_error(reason);
                self.release_dialog_idx(queue, idx, false, true);
            }
            ConnectAction::None => {}
        }
        if notify_connected && !connected_remote.is_empty() {
            self.last_session_event = Some(format!("connected {}", connected_remote));
            self.notify_session(cfg, &connected_remote, true);
        }
    }

    fn mark_media_ready(&mut self, brew_uuid: Uuid, call_id: u16, carrier_num: u16, ts: u8) {
        if let Some((idx, dialog)) = self.dialogs.iter_mut().enumerate().find(|(_, d)| d.uuid == Some(brew_uuid)) {
            dialog.media_ready = Some((call_id, carrier_num, ts));
            dialog.remote_floor_active = true;
            dialog.remote_floor_ready = true;
            dialog.remote_floor_since = Some(Instant::now());
            self.dialog_by_slot.insert((carrier_num, ts), idx);
            tracing::info!(
                "EchoLink: media ready remote={} uuid={} call_id={} carrier={} ts={}",
                dialog.remote_call,
                brew_uuid,
                call_id,
                carrier_num,
                ts
            );
        }
    }

    fn release_dialog_by_uuid(&mut self, queue: &mut MessageQueue, brew_uuid: Uuid, from_cmce: bool) {
        if let Some(idx) = self.dialogs.iter().position(|d| d.uuid == Some(brew_uuid)) {
            self.release_dialog_idx(queue, idx, from_cmce, true);
        }
    }

    fn release_dialog_by_call_id(&mut self, queue: &mut MessageQueue, call_id: u16, from_cmce: bool) {
        if let Some(idx) = self
            .dialogs
            .iter()
            .position(|d| d.media_ready.map(|(ready_call_id, _, _)| ready_call_id) == Some(call_id))
        {
            if from_cmce && self.dialogs.get(idx).is_some_and(|d| d.call.is_none()) {
                self.clear_group_media_idx(idx, "CMCE released TETRA group leg");
            } else {
                self.release_dialog_idx(queue, idx, from_cmce, true);
            }
        }
    }

    fn clear_group_media_idx(&mut self, idx: usize, reason: &str) {
        if idx >= self.dialogs.len() {
            return;
        }
        let (remote_call, media_ready) = {
            let dialog = &mut self.dialogs[idx];
            let media_ready = dialog.media_ready.take();
            dialog.uuid = None;
            dialog.remote_floor_active = false;
            dialog.remote_floor_ready = false;
            dialog.remote_floor_since = None;
            dialog.last_audio_rx = None;
            (dialog.remote_call.clone(), media_ready)
        };
        if let Some((call_id, carrier_num, ts)) = media_ready {
            self.dialog_by_slot.remove(&(carrier_num, ts));
            tracing::debug!(
                "EchoLink: {} remote={} call_id={} carrier={} ts={} (session kept)",
                reason,
                remote_call,
                call_id,
                carrier_num,
                ts
            );
        }
    }

    fn release_dialog_idx(&mut self, queue: &mut MessageQueue, idx: usize, from_cmce: bool, send_bye: bool) {
        if idx >= self.dialogs.len() {
            return;
        }
        let remote_call = self.dialogs[idx].remote_call.clone();
        let remote_control = self.dialogs[idx].remote_control;
        let was_connected = self.dialogs[idx].state == QsoState::Connected;
        if send_bye {
            self.send_bye_idx(idx);
        }
        if let Some((_, carrier_num, ts)) = self.dialogs[idx].media_ready {
            self.dialog_by_slot.remove(&(carrier_num, ts));
        }
        let uuid = self.dialogs[idx].uuid;
        let is_circuit_call = self.dialogs[idx].call.is_some();
        self.dialogs[idx].state = QsoState::Released;
        if !from_cmce {
            if let Some(uuid) = uuid {
                if is_circuit_call {
                    self.send_release_to_cmce(queue, uuid, 16);
                } else {
                    self.send_group_end_to_cmce(queue, uuid);
                }
            }
        }
        self.dialogs.remove(idx);
        self.rebuild_ts_index();
        if was_connected {
            let cfg = self.effective();
            self.notify_session(&cfg, &remote_call, false);
            self.last_session_event = Some(format!("disconnected {} from {}", remote_call, remote_control));
        }
        tracing::info!(
            "EchoLink: session closed remote={} control={} from_cmce={} bye_sent={}",
            remote_call,
            remote_control,
            from_cmce,
            send_bye
        );
    }

    fn disconnect_all(&mut self, queue: &mut MessageQueue, send_bye: bool) {
        while !self.dialogs.is_empty() {
            self.release_dialog_idx(queue, 0, false, send_bye);
        }
    }

    fn rebuild_ts_index(&mut self) {
        self.dialog_by_slot.clear();
        for (idx, dialog) in self.dialogs.iter().enumerate() {
            if let Some((_, carrier_num, ts)) = dialog.media_ready {
                self.dialog_by_slot.insert((carrier_num, ts), idx);
            }
        }
    }

    fn handle_ul_voice(&mut self, prim: TmdCircuitDataInd) {
        let Some(&idx) = self.dialog_by_slot.get(&(prim.carrier_num, prim.ts)) else {
            return;
        };
        let Some(socket) = self.audio_socket.as_ref().and_then(|s| s.try_clone().ok()) else {
            return;
        };
        let decoded = {
            let Some(dialog) = self.dialogs.get_mut(idx) else {
                return;
            };
            if dialog.state != QsoState::Connected {
                return;
            }
            let remote_audio = dialog.remote_audio;
            let seq = dialog.seq;
            dialog
                .audio
                .decode_tmd_to_gsm_packets(&prim.data)
                .map(|payloads| (remote_audio, seq, payloads))
        };
        let Some((remote_audio, start_seq, payloads)) = decoded else {
            self.set_error(format!(
                "dropping unsupported TETRA audio block ts={} len={}",
                prim.ts,
                prim.data.len()
            ));
            return;
        };
        let mut seq = start_seq;
        let mut sent_packets = 0usize;
        let mut last_error = None;
        for payload in payloads {
            let packet = build_audio_packet(seq, &payload);
            match socket.send_to(&packet, remote_audio) {
                Ok(_) => {
                    seq = seq.wrapping_add(1);
                    sent_packets += 1;
                }
                Err(err) => {
                    last_error = Some(format!("audio send to {} failed: {}", remote_audio, err));
                    break;
                }
            }
        }
        if let Some(dialog) = self.dialogs.get_mut(idx) {
            dialog.seq = seq;
        }
        if sent_packets > 0 {
            self.last_tx = Some(format!("audio {} packet(s) to {}", sent_packets, remote_audio));
        }
        if let Some(err) = last_error {
            self.set_error(err);
        }
    }

    fn poll_audio(&mut self, queue: &mut MessageQueue) {
        let Some(socket) = self.audio_socket.as_ref().and_then(|s| s.try_clone().ok()) else {
            return;
        };
        let mut buf = [0u8; 1500];
        for _ in 0..32 {
            match socket.recv_from(&mut buf) {
                Ok((len, addr)) => self.handle_audio_packet(queue, &buf[..len], addr),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    self.set_error(format!("audio receive failed: {}", err));
                    break;
                }
            }
        }
    }

    fn handle_audio_packet(&mut self, queue: &mut MessageQueue, packet: &[u8], addr: SocketAddr) {
        if packet.starts_with(b"oNDATA") {
            tracing::debug!("EchoLink: oNDATA probe from {}", addr);
            return;
        }
        if packet.len() < ECHOLINK_RTP_HEADER || packet[0] != 0xc0 {
            return;
        }
        let payload_type = packet[1] & 0x7f;
        if payload_type != ECHOLINK_RTP_GSM_PT {
            tracing::trace!("EchoLink: dropping unsupported RTP payload type {}", payload_type);
            return;
        }
        let Some(idx) = self.find_dialog_by_ip(addr.ip()) else {
            return;
        };
        let payload = &packet[ECHOLINK_RTP_HEADER..];
        if payload.len() < ECHOLINK_GSM_PACKET_BYTES || payload.len() % ECHOLINK_GSM_FRAME_BYTES != 0 {
            tracing::trace!("EchoLink: dropping malformed GSM payload len={}", payload.len());
            return;
        }

        enum AudioAction {
            Forward {
                carrier_num: u16,
                ts: u8,
                frames: Vec<Vec<u8>>,
                remote_call: String,
                first_packet: bool,
            },
            RequestFloor {
                remote_call: String,
            },
            DropPending {
                remote_call: String,
            },
        }

        let now = Instant::now();
        let action = {
            let Some(dialog) = self.dialogs.get_mut(idx) else {
                return;
            };
            if dialog.state != QsoState::Connected {
                return;
            }
            let remote_call = dialog.remote_call.clone();
            let first_packet = dialog.last_audio_rx.is_none();
            dialog.remote_audio = addr;
            dialog.last_audio_rx = Some(now);

            if dialog.call.is_none() && !dialog.remote_floor_active {
                AudioAction::RequestFloor { remote_call }
            } else if dialog.call.is_none() && (!dialog.remote_floor_ready || dialog.media_ready.is_none()) {
                AudioAction::DropPending { remote_call }
            } else if let Some((_, carrier_num, ts)) = dialog.media_ready {
                AudioAction::Forward {
                    carrier_num,
                    ts,
                    frames: dialog.audio.decode_gsm_payload_to_tmd(payload),
                    remote_call,
                    first_packet,
                }
            } else {
                AudioAction::DropPending { remote_call }
            }
        };

        let (carrier_num, ts, frames, remote_call, first_packet) = match action {
            AudioAction::Forward {
                carrier_num,
                ts,
                frames,
                remote_call,
                first_packet,
            } => (carrier_num, ts, frames, remote_call, first_packet),
            AudioAction::RequestFloor { remote_call } => {
                let cfg = self.effective();
                tracing::info!(
                    "EchoLink: RTP audio burst from {} at {} -> requesting TETRA floor",
                    remote_call,
                    addr
                );
                self.request_group_floor_idx(queue, &cfg, idx, "RTP audio burst");
                self.last_rx = Some(format!("audio pending floor from {}", addr));
                return;
            }
            AudioAction::DropPending { remote_call } => {
                tracing::debug!("EchoLink: dropping RTP audio from {} while TETRA floor is not ready", remote_call);
                self.last_rx = Some(format!("audio pending media from {}", addr));
                return;
            }
        };

        if first_packet {
            tracing::info!(
                "EchoLink: RTP audio started remote={} audio={} -> carrier={} ts={}",
                remote_call,
                addr,
                carrier_num,
                ts
            );
        } else {
            tracing::debug!(
                "EchoLink: RTP audio remote={} bytes={} -> carrier={} ts={}",
                remote_call,
                packet.len(),
                carrier_num,
                ts
            );
        }

        for frame in frames {
            queue.push_back(SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Echolink,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                    carrier_num,
                    ts,
                    data: frame,
                }),
            });
        }
        self.last_rx = Some(format!("audio {} bytes from {}", packet.len(), addr));
    }

    fn poll_control(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink) {
        let Some(socket) = self.control_socket.as_ref().and_then(|s| s.try_clone().ok()) else {
            return;
        };
        let mut buf = [0u8; 1500];
        for _ in 0..32 {
            match socket.recv_from(&mut buf) {
                Ok((len, addr)) => self.handle_control_packet(queue, cfg, &buf[..len], addr),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    self.set_error(format!("control receive failed: {}", err));
                    break;
                }
            }
        }
    }

    fn handle_control_packet(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink, packet: &[u8], addr: SocketAddr) {
        if is_rtcp_bye(packet) {
            if let Some(idx) = self.find_dialog_by_ip(addr.ip()) {
                let remote_call = self
                    .dialogs
                    .get(idx)
                    .map(|dialog| dialog.remote_call.clone())
                    .unwrap_or_else(|| "-".to_string());
                self.last_rx = Some(format!("BYE from {}", addr));
                tracing::info!("EchoLink: BYE from remote={} control={}", remote_call, addr);
                self.release_dialog_idx(queue, idx, false, false);
            } else {
                tracing::debug!("EchoLink: BYE from unknown control={} -> replying BYE", addr);
                self.send_bye_to(addr);
            }
            return;
        }

        if let Some(remote_name) = parse_rtcp_sdes_name(packet) {
            let remote_call = remote_name.split_whitespace().next().unwrap_or("").trim().to_ascii_uppercase();
            if remote_call.is_empty() {
                return;
            }
            self.last_rx = Some(format!("SDES {} from {}", remote_call, addr));
            if let Some(idx) = self.find_dialog_by_ip(addr.ip()) {
                if let Some(dialog) = self.dialogs.get_mut(idx) {
                    tracing::debug!(
                        "EchoLink: SDES refresh remote={} control={} previous_remote={}",
                        remote_call,
                        addr,
                        dialog.remote_call
                    );
                    dialog.remote_control = addr;
                    dialog.remote_audio = SocketAddr::new(addr.ip(), cfg.audio_port);
                    dialog.remote_call = remote_call;
                    dialog.last_sdes = Instant::now();
                }
                self.maybe_connect_dialog(queue, cfg, idx);
                return;
            }

            if !cfg.inbound_enabled || cfg.default_tetra_dest_issi == 0 {
                self.send_bye_to(addr);
                return;
            }
            if !cfg.default_tetra_dest_is_group {
                self.set_error("inbound EchoLink requires default_tetra_dest_is_group=true for simplex/P2MP");
                self.send_bye_to(addr);
                return;
            }
            if !target_allowed(cfg, &remote_call, Some(addr.ip()), &self.directory_stations) {
                self.set_error(format!("inbound target {remote_call} is not allowed by EchoLink routing"));
                tracing::info!(
                    "EchoLink: rejecting inbound SDES remote={} control={} (not allowed)",
                    remote_call,
                    addr
                );
                self.send_bye_to(addr);
                return;
            }
            tracing::info!("EchoLink: inbound SDES remote={} control={} accepted", remote_call, addr);
            self.start_inbound_call(queue, cfg, remote_call, addr);
        }
    }

    fn start_inbound_call(&mut self, queue: &mut MessageQueue, cfg: &CfgEcholink, remote_call: String, remote_control: SocketAddr) {
        let Some(audio) = EcholinkAudioTranscoder::new() else {
            self.set_error("EchoLink codec allocation failed for inbound QSO");
            self.send_bye_to(remote_control);
            return;
        };
        let dialog = EcholinkDialog {
            uuid: None,
            call: None,
            target: remote_call.clone(),
            remote_call: remote_call.clone(),
            remote_ip: remote_control.ip(),
            remote_audio: SocketAddr::new(remote_control.ip(), cfg.audio_port),
            remote_control,
            state: QsoState::Connected,
            audio,
            media_ready: None,
            remote_floor_active: false,
            remote_floor_ready: false,
            remote_floor_since: None,
            seq: 1,
            inbound: true,
            started: Instant::now(),
            last_sdes: Instant::now(),
            last_audio_rx: None,
        };
        self.dialogs.push(dialog);
        let idx = self.dialogs.len() - 1;
        self.send_sdes_idx(idx, cfg);
        tracing::info!(
            "EchoLink: inbound session opened remote={} control={} audio={} -> GSSI {}",
            remote_call,
            remote_control,
            SocketAddr::new(remote_control.ip(), cfg.audio_port),
            cfg.default_tetra_dest_issi
        );
        self.last_session_event = Some(format!("connected {} from {}", remote_call, remote_control));
        self.notify_session(cfg, &remote_call, true);
        self.request_group_floor_idx(queue, cfg, idx, "inbound simplex/P2MP QSO");
    }

    fn notify_session(&self, cfg: &CfgEcholink, remote_call: &str, connected: bool) {
        if !cfg.telegram_session_alerts {
            return;
        }
        let Some(sink) = &self.telegram_sink else {
            tracing::debug!(
                "EchoLink: Telegram session alert skipped for remote={} because Telegram is not configured",
                remote_call
            );
            return;
        };
        sink.send_echolink_session(
            cfg.telegram_session_prefix.clone(),
            remote_call.to_string(),
            connected,
            route_label(cfg).unwrap_or_else(|| "not routed".to_string()),
        );
    }

    fn send_sdes_idx(&mut self, idx: usize, cfg: &CfgEcholink) {
        let Some(dialog) = self.dialogs.get(idx) else {
            return;
        };
        let packet = build_sdes_packet(&cfg.callsign, &cfg.location);
        let addr = dialog.remote_control;
        if let Some(socket) = self.control_socket.as_ref().and_then(|s| s.try_clone().ok()) {
            match socket.send_to(&packet, addr) {
                Ok(_) => {
                    if let Some(dialog) = self.dialogs.get_mut(idx) {
                        dialog.last_sdes = Instant::now();
                    }
                    tracing::debug!("EchoLink: SDES sent to {}", addr);
                    self.last_tx = Some(format!("SDES to {}", addr));
                }
                Err(err) => self.set_error(format!("SDES send to {} failed: {}", addr, err)),
            }
        }
    }

    fn send_bye_idx(&mut self, idx: usize) {
        let Some(dialog) = self.dialogs.get(idx) else {
            return;
        };
        let addr = dialog.remote_control;
        self.send_bye_to(addr);
    }

    fn send_bye_to(&mut self, addr: SocketAddr) {
        let packet = build_bye_packet();
        if let Some(socket) = self.control_socket.as_ref().and_then(|s| s.try_clone().ok()) {
            match socket.send_to(&packet, addr) {
                Ok(_) => {
                    tracing::info!("EchoLink: BYE sent to {}", addr);
                    self.last_tx = Some(format!("BYE to {}", addr));
                }
                Err(err) => self.set_error(format!("BYE send to {} failed: {}", addr, err)),
            }
        }
    }

    fn maybe_keepalive(&mut self, cfg: &CfgEcholink) {
        let now = Instant::now();
        let mut idxs = Vec::new();
        for (idx, dialog) in self.dialogs.iter().enumerate() {
            if dialog.state != QsoState::Released && now.duration_since(dialog.last_sdes) >= KEEPALIVE_INTERVAL {
                idxs.push(idx);
            }
        }
        for idx in idxs {
            self.send_sdes_idx(idx, cfg);
        }
    }

    fn maybe_timeout(&mut self, queue: &mut MessageQueue) {
        let now = Instant::now();
        let mut idx = 0;
        while idx < self.dialogs.len() {
            if self.dialogs[idx].state == QsoState::Connecting && now.duration_since(self.dialogs[idx].started) >= CONNECT_TIMEOUT {
                let target = self.dialogs[idx].target.clone();
                self.set_error(format!("connect to {target} timed out"));
                self.release_dialog_idx(queue, idx, false, true);
            } else {
                idx += 1;
            }
        }
        self.maybe_release_stale_remote_floors(queue, now);
    }

    fn maybe_release_stale_remote_floors(&mut self, queue: &mut MessageQueue, now: Instant) {
        let mut stale = Vec::new();
        for (idx, dialog) in self.dialogs.iter().enumerate() {
            if dialog.state != QsoState::Connected || dialog.call.is_some() || !dialog.remote_floor_active {
                continue;
            }
            let timed_out = if !dialog.remote_floor_ready {
                dialog
                    .remote_floor_since
                    .map(|since| now.duration_since(since) >= AUDIO_RX_SETUP_TIMEOUT)
                    .unwrap_or(false)
            } else if dialog.media_ready.is_some() {
                dialog
                    .last_audio_rx
                    .map(|last| now.duration_since(last) >= AUDIO_RX_ACTIVITY_TIMEOUT)
                    .unwrap_or_else(|| {
                        dialog
                            .remote_floor_since
                            .map(|since| now.duration_since(since) >= AUDIO_RX_SETUP_TIMEOUT)
                            .unwrap_or(false)
                    })
            } else {
                dialog
                    .remote_floor_since
                    .map(|since| now.duration_since(since) >= AUDIO_RX_SETUP_TIMEOUT)
                    .unwrap_or(false)
            };
            if timed_out {
                stale.push(idx);
            }
        }

        for idx in stale.into_iter().rev() {
            self.release_remote_floor_idx(queue, idx, "RTP audio inactive");
        }
    }

    fn find_dialog_by_ip(&self, ip: IpAddr) -> Option<usize> {
        self.dialogs.iter().position(|d| d.state != QsoState::Released && d.remote_ip == ip)
    }

    fn resolve_target(&mut self, _cfg: &CfgEcholink, target: &str) -> Result<IpAddr, String> {
        if let Ok(ip) = target.parse::<IpAddr>() {
            return Ok(ip);
        }
        if self.directory_stations.is_empty() {
            return Err("EchoLink directory station list is not ready yet".to_string());
        }
        let target_upper = normalize_echolink_target(target);
        let station = if let Ok(node_id) = target_upper.parse::<u32>() {
            self.directory_stations.iter().find(|s| s.id == node_id)
        } else {
            self.directory_stations
                .iter()
                .find(|s| station_matches_target(&s.callsign, &target_upper))
        };
        station
            .map(|s| s.ip)
            .ok_or_else(|| format!("EchoLink target {target_upper} not found in directory"))
    }

    fn start_directory_worker(&mut self) {
        self.stop_directory_worker();

        let config = self.config.clone();
        let cfg = config.effective_echolink();
        let config_key = directory_config_key(&cfg);
        let event_tx = self.directory_event_tx.clone();
        let (stop_tx, stop_rx) = crossbeam_channel::bounded(1);
        self.directory_generation = self.directory_generation.wrapping_add(1);
        let generation = self.directory_generation;
        self.directory_stop_tx = Some(stop_tx);
        self.directory_config_key = Some(config_key);
        self.last_directory_status = "registering".to_string();
        tracing::info!(
            "EchoLink: directory registration worker starting for {} via {}:{}",
            cfg.callsign,
            cfg.directory_servers.join(","),
            cfg.directory_port
        );

        let spawn = thread::Builder::new().name("flow-echolink-directory".to_string()).spawn(move || {
            loop {
                let cfg = config.effective_echolink();
                if !cfg.enabled {
                    break;
                }

                let wait = match directory_make_online_request(&cfg) {
                    Ok(()) => {
                        match directory_get_calls_request(&cfg) {
                            Ok(stations) => {
                                let _ = event_tx.send(EcholinkDirectoryEvent::Online {
                                    generation,
                                    callsign: cfg.callsign.clone(),
                                    stations,
                                });
                            }
                            Err(err) => {
                                let _ = event_tx.send(EcholinkDirectoryEvent::Online {
                                    generation,
                                    callsign: cfg.callsign.clone(),
                                    stations: Vec::new(),
                                });
                                let _ = event_tx.send(EcholinkDirectoryEvent::Error {
                                    generation,
                                    message: format!("directory list after ONLINE failed: {}", err),
                                });
                            }
                        }
                        DIRECTORY_REFRESH_INTERVAL
                    }
                    Err(err) => {
                        let _ = event_tx.send(EcholinkDirectoryEvent::Error { generation, message: err });
                        Duration::from_secs(cfg.reconnect_interval_secs.max(1))
                    }
                };

                if !matches!(stop_rx.recv_timeout(wait), Err(crossbeam_channel::RecvTimeoutError::Timeout)) {
                    break;
                }
            }
        });

        if let Err(err) = spawn {
            self.directory_stop_tx = None;
            self.directory_config_key = None;
            self.set_error(format!("directory worker start failed: {}", err));
        }
    }

    fn stop_directory_worker(&mut self) {
        if let Some(stop_tx) = self.directory_stop_tx.take() {
            let _ = stop_tx.send(());
            self.directory_generation = self.directory_generation.wrapping_add(1);
        }
        self.directory_config_key = None;
        self.directory_stations.clear();
        self.directory_stations_dirty = true;
    }

    fn ensure_directory_worker(&mut self, cfg: &CfgEcholink) {
        let key = directory_config_key(cfg);
        if self.directory_stop_tx.is_none() || self.directory_config_key.as_deref() != Some(key.as_str()) {
            self.start_directory_worker();
        }
    }

    fn poll_directory_events(&mut self) {
        while let Ok(event) = self.directory_event_rx.try_recv() {
            match event {
                EcholinkDirectoryEvent::Online {
                    generation,
                    callsign,
                    stations,
                } if generation == self.directory_generation => {
                    let station_count = stations.len();
                    self.directory_stations = stations;
                    self.directory_stations_dirty = true;
                    self.last_directory_status = format!("online; {} stations", station_count);
                    self.last_error = None;
                    self.last_tx = Some(format!("directory ONLINE refreshed for {} ({} stations)", callsign, station_count));
                    tracing::info!("EchoLink: directory ONLINE refreshed for {} ({} stations)", callsign, station_count);
                }
                EcholinkDirectoryEvent::Error { generation, message } if generation == self.directory_generation => {
                    self.last_directory_status = "directory error".to_string();
                    self.set_error(message);
                }
                _ => {}
            }
        }
    }
}

impl TetraEntityTrait for EcholinkEntity {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Echolink
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let cfg = self.effective();
        match message.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => {
                self.start_outbound_call(queue, &cfg, brew_uuid, call);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
                ..
            }) => {
                self.mark_media_ready(brew_uuid, call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }) => {
                self.release_dialog_by_uuid(queue, brew_uuid, true);
            }
            SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, .. }) => {
                self.release_dialog_by_call_id(queue, call_id, true);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }) => {
                tracing::info!("EchoLink: inbound setup accepted by CMCE uuid={}", brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }) => {
                tracing::info!("EchoLink: setup rejected by CMCE uuid={} cause={}", brew_uuid, cause);
                self.release_dialog_by_uuid(queue, brew_uuid, true);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }) => {
                tracing::info!("EchoLink: TETRA side alert uuid={}", brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid, call }) => {
                if let Some(dialog) = self.dialogs.iter_mut().find(|d| d.uuid == Some(brew_uuid)) {
                    dialog.call = Some(echolink_simplex_p2mp_call(call));
                    dialog.state = QsoState::Connected;
                }
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Echolink,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                        brew_uuid,
                        grant: 0,
                        permission: 0,
                    }),
                });
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm { .. }) => {}
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
            }) => {
                self.mark_media_ready(brew_uuid, call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, .. }) => {
                self.release_dialog_by_uuid(queue, brew_uuid, true);
            }
            SapMsgInner::TmdCircuitDataInd(prim) => {
                self.handle_ul_voice(prim);
            }
            _ => {}
        }
        self.refresh_status();
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
        let cfg = self.effective();
        if !cfg.enabled {
            if self.last_enabled != Some(false) {
                tracing::info!("EchoLink integration disabled");
                self.last_enabled = Some(false);
            }
            self.disconnect_all(queue, false);
            self.release_ports();
            self.stop_directory_worker();
            self.last_directory_status = "disabled".to_string();
            self.refresh_status();
            return;
        }

        if self.last_enabled != Some(true) {
            tracing::info!(
                "EchoLink integration enabled (call={} inbound={} outbound={} ports={}/{})",
                cfg.callsign,
                cfg.inbound_enabled,
                cfg.outbound_enabled,
                cfg.audio_port,
                cfg.control_port
            );
            self.last_enabled = Some(true);
        }

        match self.ensure_ports(&cfg) {
            Ok(()) => {
                if self.last_directory_status == "disabled" {
                    self.last_directory_status = "ports ready".to_string();
                }
                self.ensure_directory_worker(&cfg);
                self.poll_directory_events();
                self.handle_dashboard_commands(queue, &cfg);
                self.poll_control(queue, &cfg);
                self.poll_audio(queue);
                self.maybe_keepalive(&cfg);
                self.maybe_timeout(queue);
            }
            Err(err) => {
                self.last_directory_status = "error".to_string();
                self.set_error(err);
                self.release_ports();
                self.stop_directory_worker();
            }
        }
        self.refresh_status();
    }
}

fn target_allowed(cfg: &CfgEcholink, target: &str, remote_ip: Option<IpAddr>, stations: &[DirectoryStation]) -> bool {
    if cfg.allowed_callsigns.is_empty() && cfg.allowed_node_ids.is_empty() {
        return true;
    }
    if cfg.allowed_callsigns.iter().any(|c| station_matches_target(c, target)) {
        return true;
    }
    if target
        .parse::<u32>()
        .ok()
        .map(|id| cfg.allowed_node_ids.contains(&id))
        .unwrap_or(false)
    {
        return true;
    }

    stations
        .iter()
        .find(|station| station_matches_target(&station.callsign, target) || remote_ip.is_some_and(|ip| station.ip == ip))
        .is_some_and(|station| {
            cfg.allowed_node_ids.contains(&station.id)
                || cfg
                    .allowed_callsigns
                    .iter()
                    .any(|allowed| station_matches_target(allowed, &station.callsign))
        })
}

fn station_matches_target(station: &str, target: &str) -> bool {
    if station.eq_ignore_ascii_case(target) {
        return true;
    }
    station.trim_matches('*').eq_ignore_ascii_case(target.trim_matches('*'))
}

fn echolink_simplex_p2mp_call(mut call: NetworkCircuitCall) -> NetworkCircuitCall {
    call.duplex = 0;
    call.communication = CommunicationType::P2Mp.into_raw() as u8;
    call.timeout = CallTimeout::T5m.into_raw() as u8;
    call
}

fn route_label(cfg: &CfgEcholink) -> Option<String> {
    if cfg.default_tetra_dest_issi == 0 {
        return None;
    }
    Some(format!(
        "{} {} from {}",
        if cfg.default_tetra_dest_is_group { "GSSI" } else { "ISSI" },
        cfg.default_tetra_dest_issi,
        cfg.default_tetra_source_issi
    ))
}

fn directory_description(status_text: &str) -> String {
    status_text
        .trim()
        .chars()
        .map(|ch| if ch == '\r' || ch == '\n' { ' ' } else { ch })
        .take(DIRECTORY_DESCRIPTION_MAX_CHARS)
        .collect()
}

fn directory_config_key(cfg: &CfgEcholink) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}:{}:{}",
        cfg.callsign,
        cfg.password.as_ref(),
        cfg.status_text,
        cfg.directory_servers.join("\u{1e}"),
        cfg.directory_port,
        cfg.bind_addr,
        cfg.audio_port,
        cfg.control_port
    )
}

fn directory_make_online_request(cfg: &CfgEcholink) -> Result<(), String> {
    let mut stream = directory_connect(cfg)?;
    let time = chrono::Local::now().format("%H:%M").to_string();
    let description = directory_description(&cfg.status_text);
    let mut cmd = Vec::new();
    cmd.push(b'l');
    cmd.extend_from_slice(cfg.callsign.as_bytes());
    cmd.extend_from_slice(&[0xac, 0xac]);
    cmd.extend_from_slice(cfg.password.as_ref().as_bytes());
    cmd.extend_from_slice(b"\rONLINE3.38(");
    cmd.extend_from_slice(time.as_bytes());
    cmd.extend_from_slice(b")\r");
    cmd.extend_from_slice(description.as_bytes());
    cmd.extend_from_slice(b"\r");
    stream
        .write_all(&cmd)
        .map_err(|e| format!("directory ONLINE write failed: {}", e))?;
    let mut buf = [0u8; 256];
    let len = stream.read(&mut buf).map_err(|e| format!("directory ONLINE read failed: {}", e))?;
    let reply = String::from_utf8_lossy(&buf[..len]).to_string();
    if reply.starts_with("OK") {
        Ok(())
    } else {
        Err(format!("directory ONLINE rejected: {}", reply.trim()))
    }
}

fn directory_get_calls_request(cfg: &CfgEcholink) -> Result<Vec<DirectoryStation>, String> {
    let mut stream = directory_connect(cfg)?;
    stream.write_all(b"s").map_err(|e| format!("directory list write failed: {}", e))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(len) => {
                buf.extend_from_slice(&chunk[..len]);
                if buf.windows(3).any(|w| w == b"+++") {
                    break;
                }
            }
            Err(err) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) && !buf.is_empty() => {
                break;
            }
            Err(err) => return Err(format!("directory list read failed: {}", err)),
        }
    }
    let text = String::from_utf8_lossy(&buf);
    parse_directory_list(&text)
}

fn directory_connect(cfg: &CfgEcholink) -> Result<TcpStream, String> {
    for server in &cfg.directory_servers {
        let server = server.trim();
        if server.is_empty() {
            continue;
        }
        let addrs = (server, cfg.directory_port)
            .to_socket_addrs()
            .map_err(|e| format!("directory DNS {}:{} failed: {}", server, cfg.directory_port, e))?;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, DIRECTORY_TIMEOUT) {
                Ok(stream) => {
                    let _ = stream.set_read_timeout(Some(DIRECTORY_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(DIRECTORY_TIMEOUT));
                    return Ok(stream);
                }
                Err(_) => continue,
            }
        }
    }
    Err("could not connect to any EchoLink directory server".to_string())
}

fn build_audio_packet(seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(ECHOLINK_RTP_HEADER + payload.len());
    packet.push(0xc0);
    packet.push(ECHOLINK_RTP_GSM_PT);
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn build_sdes_packet(callsign: &str, name: &str) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(RTP_VERSION_ECHOLINK << 6);
    packet.push(RTCP_RR);
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());

    let sdes_start = packet.len();
    packet.push((RTP_VERSION_ECHOLINK << 6) | 1);
    packet.push(RTCP_SDES);
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());

    add_sdes_item(&mut packet, RTCP_SDES_CNAME, "CALLSIGN");
    let display = format!("{:<15}{}", callsign, name);
    add_sdes_item(&mut packet, RTCP_SDES_NAME, &display);
    add_sdes_item(&mut packet, RTCP_SDES_EMAIL, "CALLSIGN");
    let time = chrono::Local::now().format("%H:%M").to_string();
    add_sdes_item(&mut packet, RTCP_SDES_PHONE, &time);
    packet.push(RTCP_SDES_END);
    packet.push(0);
    while (packet.len() - sdes_start) % 4 != 0 {
        packet.push(0);
    }
    let len_words = ((packet.len() - sdes_start) / 4).saturating_sub(1) as u16;
    packet[sdes_start + 2..sdes_start + 4].copy_from_slice(&len_words.to_be_bytes());
    packet
}

fn build_bye_packet() -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(RTP_VERSION_ECHOLINK << 6);
    packet.push(RTCP_RR);
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());

    let bye_start = packet.len();
    packet.push((RTP_VERSION_ECHOLINK << 6) | 1);
    packet.push(RTCP_BYE);
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&0u32.to_be_bytes());
    add_counted_text(&mut packet, "jan2002");
    while (packet.len() - bye_start) % 4 != 0 {
        packet.push(0);
    }
    let len_words = ((packet.len() - bye_start) / 4).saturating_sub(1) as u16;
    packet[bye_start + 2..bye_start + 4].copy_from_slice(&len_words.to_be_bytes());
    packet
}

fn add_sdes_item(packet: &mut Vec<u8>, item_type: u8, text: &str) {
    packet.push(item_type);
    add_counted_text(packet, text);
}

fn add_counted_text(packet: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let len = bytes.len().min(255);
    packet.push(len as u8);
    packet.extend_from_slice(&bytes[..len]);
}

fn is_rtcp_bye(packet: &[u8]) -> bool {
    rtcp_contains(packet, RTCP_BYE)
}

fn parse_rtcp_sdes_name(packet: &[u8]) -> Option<String> {
    let mut offset = 0usize;
    while offset + 4 <= packet.len() {
        let version = (packet[offset] >> 6) & 0x03;
        if version != RTP_VERSION_ECHOLINK && version != 1 {
            return None;
        }
        let pt = packet[offset + 1];
        let words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize + 1;
        let len = words * 4;
        if len == 0 || offset + len > packet.len() {
            return None;
        }
        if pt == RTCP_SDES && offset + 8 <= packet.len() {
            let mut item = offset + 8;
            let end = offset + len;
            while item + 2 <= end {
                let item_type = packet[item];
                if item_type == RTCP_SDES_END {
                    break;
                }
                let item_len = packet[item + 1] as usize;
                if item + 2 + item_len > end {
                    break;
                }
                if item_type == RTCP_SDES_NAME {
                    return Some(String::from_utf8_lossy(&packet[item + 2..item + 2 + item_len]).to_string());
                }
                item += 2 + item_len;
            }
        }
        offset += len;
    }
    None
}

fn rtcp_contains(packet: &[u8], packet_type: u8) -> bool {
    let mut offset = 0usize;
    while offset + 4 <= packet.len() {
        let version = (packet[offset] >> 6) & 0x03;
        if version != RTP_VERSION_ECHOLINK && version != 1 {
            return false;
        }
        let words = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize + 1;
        let len = words * 4;
        if len == 0 || offset + len > packet.len() {
            return false;
        }
        if packet[offset + 1] == packet_type {
            return true;
        }
        offset += len;
    }
    false
}

fn parse_directory_list(text: &str) -> Result<Vec<DirectoryStation>, String> {
    let mut lines = text.lines();
    let Some(start) = lines.next() else {
        return Err("empty directory response".to_string());
    };
    if start.trim() != "@@@" {
        return Err("directory response did not start with @@@".to_string());
    }
    let count = lines.next().and_then(|s| s.trim().parse::<usize>().ok()).unwrap_or(0);
    let mut stations = Vec::new();
    for _ in 0..count {
        let Some(callsign) = lines.next() else {
            break;
        };
        if callsign.trim() == "+++" {
            break;
        }
        let _data = lines.next().unwrap_or_default();
        let id = lines.next().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0);
        let ip = lines.next().and_then(|s| s.trim().parse::<IpAddr>().ok());
        if let Some(ip) = ip {
            let callsign = callsign.trim().to_ascii_uppercase();
            if !callsign.is_empty() && callsign != "." && callsign != " " {
                stations.push(DirectoryStation { callsign, id, ip });
            }
        }
    }
    Ok(stations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echolink_conference_targets_match_with_or_without_stars() {
        assert!(station_matches_target("*ECHOTEST*", "ECHOTEST"));
        assert!(station_matches_target("ECHOTEST", "*ECHOTEST*"));
        assert!(station_matches_target("*ECHOTEST*", "*ECHOTEST*"));
    }

    #[test]
    fn node_id_allowlist_accepts_inbound_directory_callsign() {
        let mut cfg = CfgEcholink::default();
        cfg.allowed_node_ids = vec![123456];
        let stations = vec![DirectoryStation {
            callsign: "DB0ABC-L".to_string(),
            id: 123456,
            ip: "192.0.2.10".parse().unwrap(),
        }];

        assert!(target_allowed(&cfg, "DB0ABC-L", None, &stations));
        assert!(target_allowed(&cfg, "DIFFERENT-L", Some("192.0.2.10".parse().unwrap()), &stations));
        assert!(!target_allowed(&cfg, "DB0XYZ-L", None, &stations));
    }
}
