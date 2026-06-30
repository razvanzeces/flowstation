//! Brew worker thread: transport-agnostic message loop for Brew protocol exchange
//!
//! Generic over any [`NetworkTransport`] implementation (WebSocket, QUIC, TCP, etc.).
//! The transport handles connection lifecycle and heartbeat; this worker handles
//! Brew protocol parsing, command dispatch, and event generation.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use tetra_config::bluestation::CfgBrew;
use tetra_config::bluestation::SharedConfig;
use uuid::Uuid;

use crate::net_brew;
use crate::network::transports::NetworkTransport;

use super::protocol::*;

// ─── Events passed from worker to entity ─────────────────────────

/// Events the Brew worker sends to the BrewEntity
#[derive(Debug)]
pub enum BrewEvent {
    /// Successfully connected to TetraPack server
    Connected { server_version: u8 },
    /// Brew protocol version detected from incoming message length (mnemonic presence)
    VersionDetected { version: u8 },

    /// Disconnected (with reason)
    Disconnected(String),

    /// Group call started
    GroupCallStart {
        uuid: Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
        service: u16,
    },

    /// Group call ended
    GroupCallEnd { uuid: Uuid, cause: u8 },

    /// Voice frame received (ACELP traffic)
    VoiceFrame { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// Subscriber event received
    SubscriberEvent { msg_type: u8, issi: u32, groups: Vec<u32> },

    /// SDS transfer received (SHORT_TRANSFER + SDS_TRANSFER combined)
    SdsTransfer {
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    },

    /// SDS report received
    SdsReport { uuid: Uuid, status: u8 },

    // ── Circuit / individual call events ──────────────────────────
    /// TetraPack initiates a circuit call to a local MS (inbound SETUP_REQUEST)
    CircuitSetupRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// TetraPack accepted our outbound circuit call (SETUP_ACCEPT)
    CircuitSetupAccept { uuid: Uuid },

    /// TetraPack rejected our outbound circuit call (SETUP_REJECT)
    CircuitSetupReject { uuid: Uuid, cause: u8 },

    /// Remote side is alerting/ringing (CALL_ALERT)
    CircuitCallAlert { uuid: Uuid },

    /// Remote side accepted the call and sent CONNECT_REQUEST
    CircuitConnectRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// TetraPack confirmed connect (CONNECT_CONFIRM)
    CircuitConnectConfirm { uuid: Uuid, grant: u8, permission: u8 },

    /// Network granted simplex floor to the local side.
    CircuitSimplexGranted { uuid: Uuid, grant: u8, permission: u8 },

    /// Network released simplex floor / returned to idle.
    CircuitSimplexIdle { uuid: Uuid, grant: u8, permission: u8 },

    /// Circuit call released (CALL_RELEASE, inbound)
    CircuitCallRelease { uuid: Uuid, cause: u8 },

    /// DTMF digits received from network
    CircuitDtmf { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// Error from server
    ServerError { error_type: u8, data: Vec<u8> },
}

/// Commands the BrewEntity sends to the worker
#[derive(Debug)]
pub enum BrewCommand {
    /// Register a subscriber (ISSI)
    RegisterSubscriber { issi: u32 },

    /// Deregister a subscriber (ISSI)
    DeregisterSubscriber { issi: u32 },

    /// Affiliate subscriber to groups
    AffiliateGroups { issi: u32, groups: Vec<u32> },

    /// Deaffiliate subscriber from groups
    DeaffiliateGroups { issi: u32, groups: Vec<u32> },

    /// Send GROUP_TX to TetraPack (local radio started transmitting on subscribed group)
    SendGroupTx {
        uuid: Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
        service: u16,
    },

    /// Send a voice frame to TetraPack (ACELP data from UL)
    SendVoiceFrame { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// Send GROUP_IDLE to TetraPack (transmission ended)
    SendGroupIdle { uuid: Uuid, cause: u8 },

    /// Send SDS to TetraPack (SHORT_TRANSFER + SDS_TRANSFER)
    SendSds {
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    },

    /// Send SDS report to Brew (delivery acknowledgement)
    SendSdsReport { uuid: Uuid, status: u8 },

    // ── Circuit / individual call commands ────────────────────────
    /// CMCE → Brew: initiate a circuit call setup to TetraPack (outbound)
    SendSetupRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// CMCE → Brew: accept an inbound circuit call (BS is the called side)
    SendSetupAccept { uuid: Uuid },

    /// CMCE → Brew: reject an inbound circuit call
    SendSetupReject { uuid: Uuid, cause: u8 },

    /// CMCE → Brew: called MS is ringing
    SendCallAlert { uuid: Uuid },

    /// CMCE → Brew: called MS accepted the call
    SendConnectRequest {
        uuid: Uuid,
        call: super::protocol::BrewCircularCall,
    },

    /// CMCE → Brew: call confirmed and connected
    SendConnectConfirm { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: local simplex side granted the floor.
    SendSimplexGranted { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: local simplex side released the floor.
    SendSimplexIdle { uuid: Uuid, grant: u8, permission: u8 },

    /// CMCE → Brew: release/terminate a circuit call
    SendCallRelease { uuid: Uuid, cause: u8 },

    /// CMCE → Brew: forward DTMF digits from MS
    SendDtmf { uuid: Uuid, length_bits: u16, data: Vec<u8> },

    /// MM → Brew: send RSSI measurement for an MS to the Brew server (Service 0xf4)
    SendRssiUpdate { issi: u32, rssi_dbfs: f32 },

    /// Disconnect gracefully
    Disconnect,
}

// ─── Worker ───────────────────────────────────────────────────────

/// Pending SDS header data (from CALL_STATE_SHORT_TRANSFER), awaiting matching FRAME_TYPE_SDS_TRANSFER
#[derive(Debug)]
struct PendingSds {
    source: u32,
    destination: u32,
    received_at: Instant,
}

/// Brew protocol worker, generic over the network transport.
///
/// Runs in a separate thread. Communicates with [`super::entity::BrewEntity`] via
/// crossbeam channels ([`BrewEvent`] and [`BrewCommand`]).
pub struct BrewWorker<T: NetworkTransport> {
    log_label: String,
    config: SharedConfig,
    brew_config: CfgBrew,
    /// Network transport (WebSocket, QUIC, TCP, …)
    transport: T,
    /// Send events to the BrewEntity
    event_sender: Sender<BrewEvent>,
    /// Receive commands from the BrewEntity
    command_receiver: Receiver<BrewCommand>,
    /// Registered subscribers and their affiliated groups (tracked from commands)
    subscriber_groups: HashMap<u32, HashSet<u32>>,
    /// Pending SDS transfers keyed by UUID, awaiting matching SDS_TRANSFER frame
    pending_sds: HashMap<Uuid, PendingSds>,
}

impl<T: NetworkTransport> BrewWorker<T> {
    pub fn new(
        log_label: String,
        config: SharedConfig,
        brew_config: CfgBrew,
        event_sender: Sender<BrewEvent>,
        command_receiver: Receiver<BrewCommand>,
        transport: T,
    ) -> Self {
        Self {
            log_label,
            config,
            brew_config,
            transport,
            event_sender,
            command_receiver,
            subscriber_groups: HashMap::new(),
            pending_sds: HashMap::new(),
        }
    }

    fn log_label(&self) -> &str {
        &self.log_label
    }

    /// Main worker entry point — runs until disconnect or fatal error
    pub fn run(&mut self) {
        tracing::info!("[{}] BrewWorker: starting", self.log_label());

        loop {
            // Attempt connection via transport
            match self.transport.connect() {
                Ok(()) => {
                    tracing::info!("[{}] BrewWorker: transport connected", self.log_label());
                    let _ = self.event_sender.send(BrewEvent::Connected {
                        server_version: self.transport.server_brew_version(),
                    });
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] BrewWorker: connection error: {}, reconnecting in {:?}",
                        self.log_label(),
                        e,
                        self.brew_config.reconnect_delay
                    );
                    let _ = self.event_sender.send(BrewEvent::Disconnected(e.to_string()));
                    std::thread::sleep(self.brew_config.reconnect_delay);
                    continue;
                }
            }

            // Run the message loop until error or clean shutdown
            match self.message_loop() {
                Ok(()) => {
                    tracing::info!("[{}] BrewWorker: connection closed normally", self.log_label());
                    break;
                }
                Err(e) => {
                    tracing::error!(
                        "[{}] BrewWorker: connection error: {}, reconnecting in {:?}",
                        self.log_label(),
                        e,
                        self.brew_config.reconnect_delay
                    );
                    let _ = self.event_sender.send(BrewEvent::Disconnected(e));
                    std::thread::sleep(self.brew_config.reconnect_delay);
                }
            }
        }
    }

    /// Graceful teardown: DEAFFILIATE → DEREGISTER → transport disconnect
    fn graceful_teardown(&mut self) {
        for (issi, groups) in &self.subscriber_groups {
            if !groups.is_empty() {
                let mut group_list: Vec<u32> = groups.iter().copied().collect();
                group_list.sort_unstable();
                let deaff_msg = build_subscriber_deaffiliate_with_type(*issi, &group_list, self.brew_config.subscriber_type_deaffiliate);
                if let Err(e) = self.transport.send_reliable(&deaff_msg) {
                    tracing::error!("[{}] BrewWorker: failed to send deaffiliation: {}", self.log_label(), e);
                } else {
                    tracing::info!(
                        "[{}] BrewWorker: deaffiliated issi={} groups={:?}",
                        self.log_label(),
                        issi,
                        group_list
                    );
                }
            }

            let dereg_msg = build_subscriber_deregister_with_type(*issi, self.brew_config.subscriber_type_deregister);
            if let Err(e) = self.transport.send_reliable(&dereg_msg) {
                tracing::error!("[{}] BrewWorker: failed to send deregistration: {}", self.log_label(), e);
            } else {
                tracing::info!("[{}] BrewWorker: deregistered ISSI {}", self.log_label(), issi);
            }
        }
        self.transport.disconnect();
    }

    /// Main message processing loop (transport-agnostic)
    fn message_loop(&mut self) -> Result<(), String> {
        loop {
            let now = Instant::now();

            // Expire stale pending SDS entries (SHORT_TRANSFER without matching SDS_TRANSFER)
            let log_label = self.log_label().to_string();
            self.pending_sds.retain(|uuid, pending| {
                let age = now.duration_since(pending.received_at);
                if age > Duration::from_secs(30) {
                    tracing::warn!("[{}] BrewWorker: expiring stale pending SDS uuid={}", log_label, uuid);
                    false
                } else {
                    true
                }
            });

            // ── Receive incoming messages from transport ──
            let messages = self.transport.receive_reliable();
            for msg in messages {
                self.handle_incoming_binary(&msg.payload);
            }

            // Check if transport is still connected (may have been dropped during receive)
            if !self.transport.is_connected() {
                return Err("transport disconnected".to_string());
            }

            // ── Check for commands from the BrewEntity ──
            loop {
                let cmd = match self.command_receiver.try_recv() {
                    Ok(cmd) => cmd,
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        // Entity was dropped — do graceful teardown
                        tracing::info!(
                            "[{}] BrewWorker: command channel closed, performing graceful teardown",
                            self.log_label()
                        );
                        self.graceful_teardown();
                        return Ok(());
                    }
                };
                match cmd {
                    BrewCommand::RegisterSubscriber { issi } => {
                        let already_registered = self.subscriber_groups.contains_key(&issi);
                        self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                        let msg = if already_registered {
                            build_subscriber_reregister_with_type(issi, self.brew_config.subscriber_type_reregister)
                        } else {
                            build_subscriber_register_with_type(issi, &[], self.brew_config.subscriber_type_register)
                        };
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send registration: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent {} issi={}",
                                self.log_label(),
                                if already_registered { "REREGISTER" } else { "REGISTER" },
                                issi
                            );
                        }
                    }
                    BrewCommand::DeregisterSubscriber { issi } => {
                        self.subscriber_groups.remove(&issi);
                        let msg = build_subscriber_deregister_with_type(issi, self.brew_config.subscriber_type_deregister);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send deregistration: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent DEREGISTER issi={}", self.log_label(), issi);
                        }
                    }
                    BrewCommand::AffiliateGroups { issi, groups } => {
                        let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                        for gssi in &groups {
                            entry.insert(*gssi);
                        }
                        let msg = build_subscriber_affiliate_with_type(issi, &groups, self.brew_config.subscriber_type_affiliate);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send affiliation: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent AFFILIATE issi={} groups={:?}",
                                self.log_label(),
                                issi,
                                groups
                            );
                        }
                    }
                    BrewCommand::DeaffiliateGroups { issi, groups } => {
                        if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                            for gssi in &groups {
                                entry.remove(gssi);
                            }
                        }
                        let msg = build_subscriber_deaffiliate_with_type(issi, &groups, self.brew_config.subscriber_type_deaffiliate);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send deaffiliation: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent DEAFFILIATE issi={} groups={:?}",
                                self.log_label(),
                                issi,
                                groups
                            );
                        }
                    }
                    BrewCommand::SendGroupTx {
                        uuid,
                        source_issi,
                        dest_gssi,
                        priority,
                        service,
                    } => {
                        let msg = build_group_tx(&uuid, source_issi, dest_gssi, priority, service, None);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send GROUP_TX: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent GROUP_TX uuid={} src={} dst={}",
                                self.log_label(),
                                uuid,
                                source_issi,
                                dest_gssi
                            );
                        }
                    }
                    BrewCommand::SendVoiceFrame { uuid, length_bits, data } => {
                        let msg = build_voice_frame(&uuid, length_bits, &data);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send voice frame: {}", self.log_label(), e);
                        }
                    }
                    BrewCommand::SendGroupIdle { uuid, cause } => {
                        let msg = build_group_idle(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send GROUP_IDLE: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent GROUP_IDLE uuid={} cause={}", self.log_label(), uuid, cause);
                        }
                    }
                    BrewCommand::SendSds {
                        uuid,
                        source,
                        destination,
                        data,
                        length_bits,
                    } => {
                        if !self.brew_config.feature_sds_enabled {
                            tracing::warn!(
                                "[{}] BrewWorker: ignoring SendSds command because SDS over Brew is disabled in config",
                                self.log_label()
                            );
                            continue;
                        }

                        // Send SHORT_TRANSFER first (header with source/dest)
                        let short_msg = build_short_transfer(&uuid, source, destination);
                        if let Err(e) = self.transport.send_reliable(&short_msg) {
                            tracing::error!("[{}] BrewWorker: failed to send SHORT_TRANSFER: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent SHORT_TRANSFER uuid={} src={} dst={}",
                                self.log_label(),
                                uuid,
                                source,
                                destination
                            );
                            // Then send SDS_TRANSFER with the payload
                            let sds_msg = build_sds_frame(&uuid, length_bits, &data);
                            if let Err(e) = self.transport.send_reliable(&sds_msg) {
                                tracing::error!("[{}] BrewWorker: failed to send SDS_TRANSFER: {}", self.log_label(), e);
                            } else {
                                tracing::debug!(
                                    "[{}] BrewWorker: sent SDS_TRANSFER uuid={} {} bytes",
                                    self.log_label(),
                                    uuid,
                                    data.len()
                                );
                            }
                        }
                    }
                    BrewCommand::SendSdsReport { uuid, status } => {
                        if !self.brew_config.feature_sds_enabled {
                            tracing::warn!(
                                "[{}] BrewWorker: ignoring SendSdsReport command because SDS over Brew is disabled in config",
                                self.log_label()
                            );
                            continue;
                        }

                        let msg = build_sds_report(&uuid, status);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::warn!("[{}] BrewWorker: failed to send SDS_REPORT: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent SDS_REPORT uuid={} status={}", self.log_label(), uuid, status);
                        }
                    }
                    BrewCommand::SendSetupRequest { uuid, call } => {
                        let data = build_setup_request(&uuid, &call);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send SETUP_REQUEST: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent SETUP_REQUEST uuid={}", self.log_label(), uuid);
                        }
                    }
                    BrewCommand::SendSetupAccept { uuid } => {
                        let data = build_setup_accept(&uuid);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send SETUP_ACCEPT: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent SETUP_ACCEPT uuid={}", self.log_label(), uuid);
                        }
                    }
                    BrewCommand::SendSetupReject { uuid, cause } => {
                        let data = build_setup_reject(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send SETUP_REJECT: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent SETUP_REJECT uuid={} cause={}", self.log_label(), uuid, cause);
                        }
                    }
                    BrewCommand::SendCallAlert { uuid } => {
                        let data = build_call_alert(&uuid);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send CALL_ALERT: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent CALL_ALERT uuid={}", self.log_label(), uuid);
                        }
                    }
                    BrewCommand::SendConnectRequest { uuid, call } => {
                        let data = build_connect_request(&uuid, &call);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send CONNECT_REQUEST: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent CONNECT_REQUEST uuid={}", self.log_label(), uuid);
                        }
                    }
                    BrewCommand::SendConnectConfirm { uuid, grant, permission } => {
                        let data = build_connect_confirm(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send CONNECT_CONFIRM: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent CONNECT_CONFIRM uuid={} grant={} perm={}",
                                self.log_label(),
                                uuid,
                                grant,
                                permission
                            );
                        }
                    }
                    BrewCommand::SendSimplexGranted { uuid, grant, permission } => {
                        let data = build_simplex_granted(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send SIMPLEX_GRANTED: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent SIMPLEX_GRANTED uuid={} grant={} perm={}",
                                self.log_label(),
                                uuid,
                                grant,
                                permission
                            );
                        }
                    }
                    BrewCommand::SendSimplexIdle { uuid, grant, permission } => {
                        let data = build_simplex_idle(&uuid, grant, permission);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send SIMPLEX_IDLE: {}", self.log_label(), e);
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent SIMPLEX_IDLE uuid={} grant={} perm={}",
                                self.log_label(),
                                uuid,
                                grant,
                                permission
                            );
                        }
                    }
                    BrewCommand::SendCallRelease { uuid, cause } => {
                        let data = build_call_release(&uuid, cause);
                        if let Err(e) = self.transport.send_reliable(&data) {
                            tracing::error!("[{}] BrewWorker: failed to send CALL_RELEASE: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent CALL_RELEASE uuid={} cause={}", self.log_label(), uuid, cause);
                        }
                    }
                    BrewCommand::SendDtmf { uuid, length_bits, data } => {
                        let msg = build_dtmf_frame(&uuid, length_bits, &data);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!("[{}] BrewWorker: failed to send DTMF: {}", self.log_label(), e);
                        } else {
                            tracing::debug!("[{}] BrewWorker: sent DTMF uuid={} bits={}", self.log_label(), uuid, length_bits);
                        }
                    }
                    BrewCommand::SendRssiUpdate { issi, rssi_dbfs } => {
                        let msg = build_service_rssi(issi, rssi_dbfs);
                        if let Err(e) = self.transport.send_reliable(&msg) {
                            tracing::error!(
                                "[{}] BrewWorker: failed to send RSSI update for ISSI {}: {}",
                                self.log_label(),
                                issi,
                                e
                            );
                        } else {
                            tracing::debug!(
                                "[{}] BrewWorker: sent RSSI issi={} rssi={:.1}dBFS",
                                self.log_label(),
                                issi,
                                rssi_dbfs
                            );
                        }
                    }
                    BrewCommand::Disconnect => {
                        self.graceful_teardown();
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Parse an incoming binary Brew message and forward as event
    fn handle_incoming_binary(&mut self, data: &[u8]) {
        match parse_brew_message(data) {
            Ok(msg) => match msg {
                BrewMessage::CallControl(cc) => self.handle_call_control(cc),
                BrewMessage::Frame(frame) => self.handle_frame(frame),
                BrewMessage::Subscriber(sub) => {
                    tracing::debug!("[{}] BrewWorker: subscriber event type={}", self.log_label(), sub.msg_type);
                    // TODO FIXME we could check whether this call is indeed a brew ssi here
                    let _ = self.event_sender.send(BrewEvent::SubscriberEvent {
                        msg_type: sub.msg_type,
                        issi: sub.number,
                        groups: sub.groups,
                    });
                }
                BrewMessage::Error(err) => {
                    tracing::warn!(
                        "[{}] BrewWorker: server error type={}: {} bytes",
                        self.log_label(),
                        err.error_type,
                        err.data.len()
                    );
                    // TODO FIXME we could check whether this call is indeed a brew ssi here
                    let _ = self.event_sender.send(BrewEvent::ServerError {
                        error_type: err.error_type,
                        data: err.data,
                    });
                }
                BrewMessage::Service(svc) => {
                    tracing::debug!(
                        "[{}] BrewWorker: service type={}: {}",
                        self.log_label(),
                        svc.service_type,
                        svc.json_data
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    "[{}] BrewWorker: failed to parse message ({} bytes): {}",
                    self.log_label(),
                    data.len(),
                    e
                );
            }
        }
    }

    /// Handle a parsed call control message
    fn handle_call_control(&mut self, cc: BrewCallControlMessage) {
        match cc.call_state {
            CALL_STATE_GROUP_TX => {
                if let BrewCallPayload::GroupTransmission(gt) = cc.payload {
                    tracing::info!(
                        "[{}] BrewWorker: GROUP_TX uuid={} src={} dst={} prio={} service={} mnemonic={}",
                        self.log_label(),
                        cc.identifier,
                        gt.source,
                        gt.destination,
                        gt.priority,
                        gt.service,
                        gt.mnemonic.is_some()
                    );
                    // Detect server version from mnemonic presence (v1 includes 34-byte mnemonic)
                    if gt.mnemonic.is_some() {
                        let _ = self.event_sender.send(BrewEvent::VersionDetected { version: 1 });
                    }
                    // Inbound admission (FH-FEAT-032 R3): a Brew-originated group call must NOT be
                    // gated by the outbound-only `whitelisted_ssis`; only `local_ssi_ranges` reject it.
                    if !net_brew::is_brew_inbound_allowed(&self.config, gt.destination) {
                        tracing::warn!(
                            "[{}] BrewWorker: dropping GROUP_TX to inactive/local-only GSSI {}",
                            self.log_label(),
                            gt.destination
                        );
                        return;
                    };
                    let _ = self.event_sender.send(BrewEvent::GroupCallStart {
                        uuid: cc.identifier,
                        source_issi: gt.source,
                        dest_gssi: gt.destination,
                        priority: gt.priority,
                        service: gt.service,
                    });
                }
            }
            CALL_STATE_GROUP_IDLE => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!(
                    "[{}] BrewWorker: GROUP_IDLE uuid={} cause={}",
                    self.log_label(),
                    cc.identifier,
                    cause
                );
                // TODO FIXME we could check whether this call is indeed a brew call here
                let _ = self.event_sender.send(BrewEvent::GroupCallEnd {
                    uuid: cc.identifier,
                    cause,
                });
            }

            CALL_STATE_SETUP_REQUEST => {
                if let BrewCallPayload::CircularCall(call) = cc.payload {
                    tracing::info!(
                        "[{}] BrewWorker: SETUP_REQUEST uuid={} src={} dst={} number='{}' duplex={}",
                        self.log_label(),
                        cc.identifier,
                        call.source,
                        call.destination,
                        call.number,
                        call.duplex
                    );
                    let _ = self.event_sender.send(BrewEvent::CircuitSetupRequest { uuid: cc.identifier, call });
                }
            }
            CALL_STATE_SETUP_ACCEPT => {
                tracing::info!("[{}] BrewWorker: SETUP_ACCEPT uuid={}", self.log_label(), cc.identifier);
                let _ = self.event_sender.send(BrewEvent::CircuitSetupAccept { uuid: cc.identifier });
            }
            CALL_STATE_SETUP_REJECT => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!(
                    "[{}] BrewWorker: SETUP_REJECT uuid={} cause={}",
                    self.log_label(),
                    cc.identifier,
                    cause
                );
                let _ = self.event_sender.send(BrewEvent::CircuitSetupReject {
                    uuid: cc.identifier,
                    cause,
                });
            }
            CALL_STATE_CALL_ALERT => {
                tracing::info!("[{}] BrewWorker: CALL_ALERT uuid={}", self.log_label(), cc.identifier);
                let _ = self.event_sender.send(BrewEvent::CircuitCallAlert { uuid: cc.identifier });
            }
            CALL_STATE_CONNECT_REQUEST => {
                if let BrewCallPayload::CircularCall(call) = cc.payload {
                    tracing::info!(
                        "[{}] BrewWorker: CONNECT_REQUEST uuid={} src={} dst={} duplex={}",
                        self.log_label(),
                        cc.identifier,
                        call.source,
                        call.destination,
                        call.duplex
                    );
                    let _ = self
                        .event_sender
                        .send(BrewEvent::CircuitConnectRequest { uuid: cc.identifier, call });
                }
            }
            CALL_STATE_CONNECT_CONFIRM => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "[{}] BrewWorker: CONNECT_CONFIRM uuid={} grant={} perm={}",
                    self.log_label(),
                    cc.identifier,
                    grant,
                    permission
                );
                let _ = self.event_sender.send(BrewEvent::CircuitConnectConfirm {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_SIMPLEX_GRANTED => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "[{}] BrewWorker: SIMPLEX_GRANTED uuid={} grant={} perm={}",
                    self.log_label(),
                    cc.identifier,
                    grant,
                    permission
                );
                let _ = self.event_sender.send(BrewEvent::CircuitSimplexGranted {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_SIMPLEX_IDLE => {
                let (grant, permission) = if let BrewCallPayload::CircularGrant(g) = cc.payload {
                    (g.grant, g.permission)
                } else {
                    (0, 0)
                };
                tracing::info!(
                    "[{}] BrewWorker: SIMPLEX_IDLE uuid={} grant={} perm={}",
                    self.log_label(),
                    cc.identifier,
                    grant,
                    permission
                );
                let _ = self.event_sender.send(BrewEvent::CircuitSimplexIdle {
                    uuid: cc.identifier,
                    grant,
                    permission,
                });
            }
            CALL_STATE_CALL_RELEASE => {
                let cause = if let BrewCallPayload::Cause(c) = cc.payload { c } else { 0 };
                tracing::info!(
                    "[{}] BrewWorker: CALL_RELEASE uuid={} cause={}",
                    self.log_label(),
                    cc.identifier,
                    cause
                );
                // Send both events — entity will handle whichever is relevant
                let _ = self.event_sender.send(BrewEvent::GroupCallEnd {
                    uuid: cc.identifier,
                    cause,
                });
                let _ = self.event_sender.send(BrewEvent::CircuitCallRelease {
                    uuid: cc.identifier,
                    cause,
                });
            }
            CALL_STATE_SHORT_TRANSFER => {
                if let BrewCallPayload::ShortTransfer { source, destination } = cc.payload {
                    tracing::info!(
                        "[{}] BrewWorker: SHORT_TRANSFER uuid={} src={} dst={}",
                        self.log_label(),
                        cc.identifier,
                        source,
                        destination
                    );
                    // Stash for matching with upcoming SDS_TRANSFER frame
                    self.pending_sds.insert(
                        cc.identifier,
                        PendingSds {
                            source,
                            destination,
                            received_at: Instant::now(),
                        },
                    );
                }
            }
            state => {
                tracing::debug!(
                    "[{}] BrewWorker: unhandled call state {} uuid={}",
                    self.log_label(),
                    state,
                    cc.identifier
                );
            }
        }
    }

    /// Handle a parsed voice/data frame
    fn handle_frame(&mut self, frame: BrewFrameMessage) {
        match frame.frame_type {
            FRAME_TYPE_TRAFFIC_CHANNEL => {
                // Forward ACELP voice frame to entity
                // TODO FIXME we could check whether this call is indeed a brew call here
                let _ = self.event_sender.send(BrewEvent::VoiceFrame {
                    uuid: frame.identifier,
                    length_bits: frame.length_bits,
                    data: frame.data,
                });
            }
            FRAME_TYPE_SDS_TRANSFER => {
                if !self.brew_config.feature_sds_enabled {
                    tracing::warn!(
                        "[{}] BrewWorker: ignoring incoming SDS_TRANSFER because SDS over Brew is disabled in config",
                        self.log_label()
                    );
                    return;
                }

                if frame.length_bits > 2047 {
                    // TODO FIXME we could split into multiple SDS messages here
                    tracing::warn!(
                        "[{}] BrewWorker: ignoring SDS_TRANSFER with excessive length_bits={} ({} bytes)",
                        self.log_label(),
                        frame.length_bits,
                        frame.data.len()
                    );
                    return;
                }

                // Reject frames whose declared bit-length exceeds the actual payload:
                // length_bits and data are independent wire fields, and the downstream
                // D-SDS-DATA serializer indexes the payload by length_bits — an
                // over-claimed length would otherwise panic the base station (OOB).
                if (frame.length_bits as usize).div_ceil(8) > frame.data.len() {
                    tracing::warn!(
                        "[{}] BrewWorker: ignoring SDS_TRANSFER with length_bits={} exceeding payload of {} bytes",
                        self.log_label(),
                        frame.length_bits,
                        frame.data.len()
                    );
                    return;
                }

                // Match with pending SHORT_TRANSFER by UUID
                if let Some(pending) = self.pending_sds.remove(&frame.identifier) {
                    tracing::info!(
                        "[{}] BrewWorker: SDS_TRANSFER uuid={} src={} dst={} {} bytes",
                        self.log_label(),
                        frame.identifier,
                        pending.source,
                        pending.destination,
                        frame.data.len()
                    );
                    let _ = self.event_sender.send(BrewEvent::SdsTransfer {
                        uuid: frame.identifier,
                        source: pending.source,
                        destination: pending.destination,
                        data: frame.data,
                        length_bits: frame.length_bits,
                    });
                } else {
                    tracing::warn!(
                        "[{}] BrewWorker: SDS_TRANSFER uuid={} without matching SHORT_TRANSFER, {} bytes",
                        self.log_label(),
                        frame.identifier,
                        frame.data.len()
                    );
                }
            }
            FRAME_TYPE_SDS_REPORT => {
                let status = if frame.data.is_empty() { 0 } else { frame.data[0] };
                tracing::debug!(
                    "[{}] BrewWorker: SDS_REPORT uuid={} status={}",
                    self.log_label(),
                    frame.identifier,
                    status
                );
                let _ = self.event_sender.send(BrewEvent::SdsReport {
                    uuid: frame.identifier,
                    status,
                });
            }
            ft => {
                tracing::debug!(
                    "[{}] BrewWorker: unhandled frame type {} uuid={}",
                    self.log_label(),
                    ft,
                    frame.identifier
                );
            }
        }
    }
}
