//! Brew protocol entity bridging a remote network backend to UMAC/MLE with hangtime-based circuit reuse
//!
//! Transport-agnostic: the concrete transport (WebSocket, QUIC, TCP, …) is
//! injected at construction time via [`BrewEntity::new`].

use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::CmceSdsData;
use uuid::Uuid;

use crate::net_brew::components::jitter_buffer::{JitterFrame, VoiceJitterBuffer};
use crate::net_site::SharedDirectory;
use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use crate::network::transports::NetworkTransport;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::{CfgBrew, SharedConfig};
use tetra_core::{CellId, Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::call_control::{CallControl, CellReselection, NetworkCircuitCall},
    tmd::TmdCircuitDataReq,
};

use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;

use super::protocol::{
    BREW_HO_STATUS_NO_TARGET, BREW_HO_STATUS_OK, BREW_HO_STATUS_REJECTED, BrewHoChannel, BrewHoRequest, BrewHoResult, BrewHoSdu,
};
use tetra_saps::control::call_control::{HandoverChannel, HandoverOutcome};
use super::worker::{BREW_EVENT_CHANNEL_CAPACITY, BrewCommand, BrewEvent, BrewWorker};

/// Hangtime before releasing group call circuit to allow reuse without re-signaling.
const GROUP_CALL_HANGTIME_DEFAULT_SECS: u64 = 5;

/// Upper bound on worker events handled in a single TDMA tick.
///
/// Everything here runs on the one entity thread with a hard tick budget: a core that
/// streams faster than we consume must never be able to monopolise a tick (the watchdog
/// restarts the cell on a core stall). Leftover events stay queued for the next tick —
/// ~70 ticks/s means this is still ~4500 events/s, far above any legitimate load.
pub(crate) const MAX_EVENTS_PER_TICK: usize = 64;

/// Maximum concurrent inbound (Brew-originated) calls we will track.
///
/// Each one holds a CMCE circuit and a jitter buffer; without a cap a core sending
/// GROUP_TX with ever-fresh UUIDs would leak timeslots until the cell has none left.
const MAX_INBOUND_ACTIVE_CALLS: usize = 8;

/// Soft-recovery window after a real Brew reconnect: CMCE may request a selective
/// D-LOCATION-UPDATE-COMMAND for one ISSI if a Brew-routed setup fails.
const SOFT_RECOVERY_WINDOW_SECS: u64 = 120;

/// Cap on held inbound calls waiting for the first local listener (late-entry promote).
const MAX_PENDING_INBOUND: usize = 4;

/// Release an inbound call whose media/keepalive has been silent this long.
///
/// GROUP_IDLE is the normal way a call ends, but a buggy or hostile core can simply stop
/// talking — the entry (and the CMCE circuit behind it) would then live forever. Mirrors
/// the 30s stale `pending_sds` reaper in the worker.
const INBOUND_CALL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

// ─── Active call tracking ─────────────────────────────────────────

/// Tracks the state of a single active Brew group call (currently transmitting)
#[derive(Debug)]
struct ActiveCall {
    /// Brew session UUID
    uuid: Uuid,
    /// TETRA call identifier (14-bit) - None until NetworkCallReady received
    call_id: Option<u16>,
    /// Allocated timeslot (2-4) - None until NetworkCallReady received
    ts: Option<u8>,
    /// Allocated carrier - None until NetworkCallReady received
    carrier_num: Option<u16>,
    /// Usage number for the channel allocation - None until NetworkCallReady received
    usage: Option<u8>,
    /// Calling party ISSI (from Brew)
    source_issi: u32,
    /// Destination GSSI (from Brew)
    dest_gssi: u32,
    /// Call priority from last GROUP_TX (for promote after hold).
    priority: u8,
    /// Number of voice frames received
    frame_count: u64,
    /// Last downlink TDMA time at which we told CMCE this call still had network media.
    last_media_activity_signal: Option<TdmaTime>,
    /// Wall clock of the last GROUP_TX / voice frame seen for this call — drives the
    /// inactivity reaper so a call the core never idles cannot leak its CMCE circuit.
    last_network_activity: Instant,
}

/// Post-GROUP_IDLE jitter still playing out. NetworkCallEnd is deferred until empty so
/// D-TX CEASED / hangtime do not cut MS TCH decode before the tail is on the air.
#[derive(Debug)]
struct PendingTail {
    carrier_num: u16,
    ts: u8,
    dest_gssi: u32,
    call_id: u16,
    usage: u8,
    source_issi: u32,
    frame_count: u64,
    jitter: VoiceJitterBuffer,
    started: Instant,
    /// Last playout or late Brew frame while draining (quiet before CallEnd).
    last_media: Instant,
}

/// Inbound GROUP_TX held because CMCE had no local listeners yet (SS-LE promote path).
/// Keyed by GSSI in `pending_inbound` — at most one held session per group.
#[derive(Debug)]
struct PendingInbound {
    uuid: Uuid,
    source_issi: u32,
    priority: u8,
    last_network_activity: Instant,
}

/// Group call in hangtime with circuit still allocated.
#[derive(Debug)]
struct HangingCall {
    /// Brew session UUID
    uuid: Uuid,
    /// TETRA call identifier (14-bit)
    call_id: u16,
    /// Allocated timeslot (2-4)
    ts: u8,
    /// Allocated carrier
    carrier_num: u16,
    /// Usage number for the channel allocation
    usage: u8,
    /// Last calling party ISSI (needed for D-SETUP re-send during late entry)
    source_issi: u32,
    /// Destination GSSI
    dest_gssi: u32,
    /// Total voice frames received during the call
    frame_count: u64,
    /// When the call entered hangtime (wall clock)
    since: Instant,
}

/// Kind of UL call being forwarded to TetraPack
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UlForwardKind {
    /// PTT group call (floor-controlled)
    Group,
    /// Full-duplex individual circuit call
    Circuit,
}

/// Tracks a local UL call being forwarded to TetraPack
#[derive(Debug)]
struct UlForwardedCall {
    /// Brew session UUID for this forwarded call
    uuid: Uuid,
    /// TETRA call identifier
    call_id: u16,
    /// Source ISSI of the calling radio
    source_issi: u32,
    /// Destination GSSI (group calls) or called ISSI (circuit calls)
    dest_gssi: u32,
    /// Number of voice frames forwarded
    frame_count: u64,
    /// Call kind: group PTT or individual circuit
    kind: UlForwardKind,
    /// Carrier carrying this call
    carrier_num: u16,
}

/// Target side: a forwarded registration handed to MM, waiting for its answer and, for type 1,
/// for CMCE's channel of the MS's group call.
#[derive(Debug)]
struct HoForward {
    /// The server's transaction id
    transaction_id: u32,
    since: Instant,
    /// The group call the serving station would move the MS into directly (type 1)
    call_gssi: Option<u32>,
    /// MM's accept, kept while CMCE brings the call's channel up
    accept: Option<BrewHoSdu>,
}

/// Serving side: a handover request this station sent.
#[derive(Debug)]
struct HoRequest {
    issi: u32,
    since: Instant,
    /// The group call named in the request
    call: Option<Type1Call>,
}

/// Serving side: the one group call an MS listens to, offered for type 1 reselection, with the
/// traffic channel it runs on here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Type1Call {
    gssi: u32,
    call_id: u16,
    carrier: u16,
    ts: u8,
}

// ─── BrewEntity ───────────────────────────────────────────────────

pub struct BrewEntity {
    config: SharedConfig,

    /// Also contained in the SharedConfig, but kept for fast, convenient access
    brew_config: CfgBrew,

    dltime: TdmaTime,

    /// Receive events from the worker thread
    event_receiver: Receiver<BrewEvent>,
    /// Send commands to the worker thread
    command_sender: Sender<BrewCommand>,

    /// Active DL calls from Brew keyed by session UUID (currently transmitting)
    active_calls: HashMap<Uuid, ActiveCall>,
    /// Per-call jitter/playout buffer for downlink voice from Brew.
    dl_jitter: HashMap<Uuid, VoiceJitterBuffer>,
    /// Pending tails after GROUP_IDLE — media first, then NetworkCallEnd / hangtime.
    pending_tails: HashMap<Uuid, PendingTail>,

    /// Inbound GROUP_TX held until the first local MS affiliates (keyed by GSSI).
    pending_inbound: HashMap<u32, PendingInbound>,

    /// DL calls in hangtime keyed by dest_gssi — circuit stays open, waiting for
    /// new speaker or timeout. Only one hanging call per GSSI.
    hanging_calls: HashMap<u32, HangingCall>,

    /// UL calls being forwarded to TetraPack, keyed by timeslot
    ul_forwarded: HashMap<(u16, u8), UlForwardedCall>,

    /// Registered subscriber groups (ISSI -> set of GSSIs)
    subscriber_groups: HashMap<u32, HashSet<u32>>,

    /// Whether the worker is connected
    connected: bool,
    /// After a real transport disconnect, the next Connected opens the soft-recovery window
    /// (resync Brew subscribers; selective LU only on demand). Must NOT be set by lazy version
    /// detection — TetraPack omits `X-Brew-Version`, so the first mnemonic GROUP_TX would
    /// otherwise look like a reconnect.
    brew_needs_soft_recovery: bool,
    /// When set, SYSINFO `network_connected=false` is deferred until this Instant (hysteresis).
    /// Cleared on reconnect before it fires.
    pending_sysinfo_disconnect_at: Option<Instant>,
    /// One-shot log/telemetry when Brew v1 is inferred from message length (no handshake header).
    brew_version_announced: bool,
    /// Optional telemetry sink for emitting brew status events
    telemetry_sink: Option<TelemetrySink>,
    /// Multi-cell: where every cell's radios are registered. Radios on an additional cell are
    /// not in this (primary) stack's subscriber list, so network SDS checks this too.
    site_directory: Option<SharedDirectory>,

    /// Target side of an announced reselection from another station: forwarded registrations
    /// handed to MLE/MM, ISSI -> (server transaction id, when). MM's answer goes back as
    /// SERVICE_HO_RESULT.
    ho_forwards: HashMap<u32, HoForward>,
    /// Serving side: handover requests this station sent, by transaction id.
    ho_requests: HashMap<u32, HoRequest>,
    /// Last transaction id used for `ho_requests`.
    ho_last_transaction: u32,
    /// Serving side: radios that left for another station with its registration accept, to drop
    /// from MM once D-NEW-CELL had time to reach them (ISSI -> when to drop).
    ho_departing: HashMap<u32, Instant>,
    /// Radios dropped because another station registered them: MM's deregistration that follows
    /// is not sent to the server (the ISSI is no longer ours there). ISSI -> when dropped.
    moved_away: HashMap<u32, Instant>,
    /// Radios answered with D-NEW-CELL whose reselection was reported to the server: a later
    /// `SERVICE_MS_MOVED` for them is not reported again as unannounced. ISSI -> when answered.
    announced_away: HashMap<u32, Instant>,

    /// Rate limiting for RSSI export: tracks last sent time per ISSI.
    /// Only used when feature_rssi_export is enabled in config.
    rssi_last_sent: HashMap<u32, Instant>,

    /// Worker thread handle for graceful shutdown
    worker_handle: Option<thread::JoinHandle<()>>,
}

impl BrewEntity {
    /// Create a new BrewEntity with the given transport.
    ///
    /// The transport is moved into a worker thread. Any [`NetworkTransport`]
    /// implementation can be used (WebSocket, QUIC, TCP, …).
    pub fn new<T: NetworkTransport + 'static>(config: SharedConfig, transport: T) -> Self {
        // Create channels. The worker→entity direction is BOUNDED (see
        // BREW_EVENT_CHANNEL_CAPACITY): it is fed by the remote core, so it is the one an
        // attacker controls. The entity→worker direction is driven by our own tick, so it
        // is naturally rate limited and stays unbounded.
        let (event_sender, event_receiver) = bounded::<BrewEvent>(BREW_EVENT_CHANNEL_CAPACITY);
        let (command_sender, command_receiver) = unbounded::<BrewCommand>();

        // Spawn worker thread with the provided transport
        let brew_config = config.config().as_ref().brew.clone().unwrap(); // Never fails
        let worker_config = config.clone();
        let handle = thread::Builder::new()
            .name("brew-worker".to_string())
            .spawn(move || {
                let mut worker = BrewWorker::new(worker_config, event_sender, command_receiver, transport);
                worker.run();
            })
            .expect("failed to spawn BrewWorker thread");

        {
            let mut state = config.state_write();
            state.network_connected = false;
        }

        Self {
            config,
            brew_config,
            dltime: TdmaTime::default(),
            event_receiver,
            command_sender,
            active_calls: HashMap::new(),
            dl_jitter: HashMap::new(),
            pending_tails: HashMap::new(),
            pending_inbound: HashMap::new(),
            hanging_calls: HashMap::new(),
            ul_forwarded: HashMap::new(),
            subscriber_groups: HashMap::new(),
            connected: false,
            brew_needs_soft_recovery: false,
            pending_sysinfo_disconnect_at: None,
            brew_version_announced: false,
            telemetry_sink: None,
            site_directory: None,
            ho_forwards: HashMap::new(),
            ho_requests: HashMap::new(),
            ho_last_transaction: 0,
            ho_departing: HashMap::new(),
            moved_away: HashMap::new(),
            announced_away: HashMap::new(),
            rssi_last_sent: HashMap::new(),
            worker_handle: Some(handle),
        }
    }

    /// Set telemetry sink for emitting brew status events.
    pub fn set_telemetry_sink(&mut self, sink: TelemetrySink) {
        self.telemetry_sink = Some(sink);
    }

    /// Multi-cell: let network SDS reach radios registered on any cell of the site.
    pub fn set_site_directory(&mut self, directory: SharedDirectory) {
        self.site_directory = Some(directory);
    }

    /// Tell the server which cell `issi` registered on: in a multi-cell site the one the site
    /// directory has it on, otherwise the primary. The worker only sends it where offered.
    fn report_ms_cell(&self, issi: u32) {
        let cell = ms_cell(self.site_directory.as_ref(), issi);
        let _ = self.command_sender.send(BrewCommand::ReportMsCell { issi, cell });
    }

    /// True if `issi` is registered on this stack or, in a multi-cell site, on any of its cells.
    fn is_registered_on_site(&self, issi: u32) -> bool {
        self.config.state_read().subscribers.is_registered(issi)
            || self
                .site_directory
                .as_ref()
                .is_some_and(|d| d.read().expect("site directory").location(issi).is_some())
    }

    /// Process pending events from the worker thread, at most MAX_EVENTS_PER_TICK of them.
    fn process_events(&mut self, queue: &mut MessageQueue) {
        let mut processed = 0usize;
        while processed < MAX_EVENTS_PER_TICK {
            let Ok(event) = self.event_receiver.try_recv() else { break };
            processed += 1;
            match event {
                BrewEvent::Connected { server_version } => {
                    tracing::debug!("BrewEntity: connected to TetraPack server (Brew v{})", server_version);
                    self.connected = true;
                    self.brew_version_announced = false;
                    {
                        let mut state = self.config.state_write();
                        state.brew_link_up = true;
                    }
                    // Blip shorter than hysteresis: cancel deferred site-trunking announcement.
                    if self.pending_sysinfo_disconnect_at.take().is_some() {
                        tracing::info!(
                            "BrewEntity: backhaul restored before hysteresis — keeping system_wide_services"
                        );
                    }
                    self.resync_subscribers();
                    self.set_network_connected(true, server_version);
                    // Soft recovery only after a real prior disconnect (not first boot connect).
                    // Site-trunking style: resync the core; do NOT mass-kick MS on the air.
                    if self.brew_needs_soft_recovery {
                        self.brew_needs_soft_recovery = false;
                        let until = Instant::now() + Duration::from_secs(SOFT_RECOVERY_WINDOW_SECS);
                        self.config.state_write().brew_soft_recovery_until = Some(until);
                        tracing::info!(
                            "BrewEntity: backhaul reconnected — Brew subscriber resync only \
                             (no mass D-LOCATION-UPDATE-COMMAND; soft-recovery window {}s)",
                            SOFT_RECOVERY_WINDOW_SECS
                        );
                    }
                }
                BrewEvent::VersionDetected { version } => {
                    // Fires on mnemonic GROUP_TX when handshake had no X-Brew-Version (TetraPack).
                    // Telemetry only — never soft-recovery / mass LU here (that kicked MS mid first call).
                    if !self.brew_version_announced {
                        self.brew_version_announced = true;
                        tracing::info!(
                            "BrewEntity: server Brew version detected from message length: v{}",
                            version
                        );
                        self.emit_brew_version(version);
                    }
                }
                BrewEvent::Disconnected(reason) => {
                    tracing::warn!("BrewEntity: Brew backhaul disconnected: {} — releasing all active calls", reason);
                    self.connected = false;
                    self.brew_needs_soft_recovery = true;
                    self.brew_version_announced = false;
                    {
                        let mut state = self.config.state_write();
                        state.brew_link_up = false;
                    }
                    if let Some(ref sink) = self.telemetry_sink {
                        let _ = sink.send(TelemetryEvent::BrewConnected {
                            connected: false,
                            server_version: 0,
                        });
                    }
                    // ETSI EN 300 392-2 §14.9.4: BS must release all circuits immediately
                    // when backhaul connection is lost. MS will receive D-RELEASE.
                    self.release_all_calls(queue);
                    let hyst = self.brew_config.backhaul_hysteresis_secs;
                    if hyst == 0 {
                        self.pending_sysinfo_disconnect_at = None;
                        self.set_network_connected(false, 0);
                    } else {
                        let at = Instant::now() + Duration::from_secs(hyst);
                        self.pending_sysinfo_disconnect_at = Some(at);
                        tracing::info!(
                            "BrewEntity: deferring site-trunking SYSINFO for {}s (hysteresis)",
                            hyst
                        );
                    }
                }
                BrewEvent::GroupCallStart {
                    uuid,
                    source_issi,
                    dest_gssi,
                    priority,
                    service,
                } => {
                    tracing::info!("BrewEntity: GROUP_TX service={} (0=TETRA ACELP, expect 0)", service);
                    self.handle_group_call_start(queue, uuid, source_issi, dest_gssi, priority);
                }
                BrewEvent::GroupCallEnd { uuid, cause } => {
                    self.handle_group_call_end(queue, uuid, cause);
                }
                BrewEvent::VoiceFrame { uuid, length_bits, data } => {
                    self.handle_voice_frame(queue, uuid, length_bits, data);
                }
                BrewEvent::SdsTransfer {
                    uuid,
                    source,
                    destination,
                    data,
                    length_bits,
                } => {
                    self.handle_sds_transfer(queue, uuid, source, destination, data, length_bits);
                }
                BrewEvent::SdsReport { uuid, status } => {
                    tracing::debug!("BrewEntity: SDS report uuid={} status={}", uuid, status);
                }
                BrewEvent::SubscriberEvent { msg_type, issi, groups } => {
                    tracing::trace!("BrewEntity: subscriber event type={} issi={} groups={:?}", msg_type, issi, groups);
                    // External subscriber (e.g. SvxLink gateway) affiliated/deaffiliated on Brew server.
                    // Notify CMCE so it updates group_listeners — without this, has_listener()
                    // returns false for GSSIs where only external subscribers are present,
                    // causing BS to reject U-SETUP with "no listeners".
                    match msg_type {
                        crate::net_brew::protocol::BREW_SUBSCRIBER_AFFILIATE => {
                            if !groups.is_empty() {
                                self.queue_external_subscriber_update(
                                    queue,
                                    issi,
                                    groups.clone(),
                                    BrewSubscriberAction::Affiliate,
                                    "AFFILIATE",
                                );
                            }
                        }
                        crate::net_brew::protocol::BREW_SUBSCRIBER_DEAFFILIATE => {
                            if !groups.is_empty() {
                                self.queue_external_subscriber_update(
                                    queue,
                                    issi,
                                    groups.clone(),
                                    BrewSubscriberAction::Deaffiliate,
                                    "DEAFFILIATE",
                                );
                            }
                        }
                        crate::net_brew::protocol::BREW_SUBSCRIBER_DEREGISTER => {
                            self.queue_external_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister, "DEREGISTER");
                        }
                        _ => {}
                    }
                }
                BrewEvent::ServerError { error_type, data } => {
                    tracing::error!("BrewEntity: server error type={} data={} bytes", error_type, data.len());
                }

                // ── Circuit / individual call events ──────────────────────
                BrewEvent::CircuitSetupRequest { uuid, call } => {
                    // TetraPack initiates a call to a local MS (BS is the called side).
                    // Map Brew wire struct → SAP NetworkCircuitCall and forward to CMCE.
                    let network_call = Self::map_brew_to_network_circuit_call(&call);
                    tracing::info!(
                        "BrewEntity: CIRCUIT SETUP REQUEST uuid={} src={} dst={} number='{}' duplex={}",
                        uuid,
                        call.source,
                        call.destination,
                        call.number,
                        call.duplex
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
                            brew_uuid: uuid,
                            call: network_call,
                        }),
                    });
                }
                BrewEvent::CircuitSetupAccept { uuid } => {
                    tracing::info!("BrewEntity: CIRCUIT SETUP ACCEPT uuid={}", uuid);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid: uuid }),
                    });
                }
                BrewEvent::CircuitSetupReject { uuid, cause } => {
                    tracing::info!("BrewEntity: CIRCUIT SETUP REJECT uuid={} cause={}", uuid, cause);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid: uuid, cause }),
                    });
                }
                BrewEvent::CircuitCallAlert { uuid } => {
                    tracing::info!("BrewEntity: CIRCUIT CALL ALERT uuid={}", uuid);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid: uuid }),
                    });
                }
                BrewEvent::CircuitConnectRequest { uuid, call } => {
                    let network_call = Self::map_brew_to_network_circuit_call(&call);
                    tracing::info!(
                        "BrewEntity: CIRCUIT CONNECT REQUEST uuid={} src={} dst={} duplex={}",
                        uuid,
                        call.source,
                        call.destination,
                        call.duplex
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                            brew_uuid: uuid,
                            call: network_call,
                        }),
                    });
                }
                BrewEvent::CircuitConnectConfirm { uuid, grant, permission } => {
                    tracing::info!(
                        "BrewEntity: CIRCUIT CONNECT CONFIRM uuid={} grant={} permission={}",
                        uuid,
                        grant,
                        permission
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                            brew_uuid: uuid,
                            grant,
                            permission,
                        }),
                    });
                }
                BrewEvent::CircuitSimplexGranted { uuid, grant, permission } => {
                    tracing::info!(
                        "BrewEntity: CIRCUIT SIMPLEX GRANTED uuid={} grant={} permission={}",
                        uuid,
                        grant,
                        permission
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
                            brew_uuid: uuid,
                            grant,
                            permission,
                        }),
                    });
                }
                BrewEvent::CircuitSimplexIdle { uuid, grant, permission } => {
                    tracing::info!(
                        "BrewEntity: CIRCUIT SIMPLEX IDLE uuid={} grant={} permission={}",
                        uuid,
                        grant,
                        permission
                    );
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
                            brew_uuid: uuid,
                            grant,
                            permission,
                        }),
                    });
                }
                BrewEvent::CircuitCallRelease { uuid, cause } => {
                    tracing::info!("BrewEntity: CIRCUIT CALL RELEASE uuid={} cause={}", uuid, cause);
                    queue.push_back(SapMsg {
                        sap: tetra_core::Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause }),
                    });
                    // Clean up our own circuit state immediately. CMCE releases the call with
                    // DisconnectCause::SwmiRequestedDisconnection, and its loop-prevention guard
                    // (cc_bs/shared.rs: `disconnect_cause != SwmiRequestedDisconnection`) means it
                    // will NOT echo a NetworkCircuitRelease back to us. Without cleaning up here,
                    // active_calls / dl_jitter would leak this session and spam jitter warnings
                    // every 5s for a circuit that is already gone.
                    self.drop_network_circuit(uuid);
                }
                BrewEvent::HandoverRequest(req) => self.handle_handover_request(queue, req),
                BrewEvent::HandoverResult(res) => self.handle_handover_result(queue, res),
                BrewEvent::MsMoved { issi } => self.handle_ms_moved(queue, issi),
                BrewEvent::CallRestoreAnswer { issi, peer, call } => {
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreAnswer { issi, peer, call }),
                    });
                }
                BrewEvent::CallHandoffRequest { issi, peer } => {
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::IndividualCallHandoffRequest { issi, peer }),
                    });
                }
                BrewEvent::CircuitDtmf { uuid, length_bits, data } => {
                    tracing::debug!("BrewEntity: CIRCUIT DTMF uuid={} bits={}", uuid, length_bits);
                    // DTMF from network → CMCE (CMCE can forward to local MS via U-INFO if needed)
                    // For now we log it; full downstream DTMF is a future extension.
                    let _ = (uuid, length_bits, data);
                }
            }
        }

        if processed == MAX_EVENTS_PER_TICK {
            tracing::trace!("BrewEntity: hit the per-tick event cap, {} left for the next tick", processed);
        }
    }

    /// Release inbound calls that have gone silent (no GROUP_TX/voice within the window).
    ///
    /// Without this a core that starts calls and never sends GROUP_IDLE — buggy, or hostile
    /// with a fresh UUID per frame — leaks an `active_calls`/`dl_jitter` entry and the CMCE
    /// timeslot behind it for every one of them.
    fn reap_idle_calls(&mut self, queue: &mut MessageQueue) {
        let stale: Vec<(Uuid, bool)> = self
            .active_calls
            .iter()
            .filter(|(_, call)| call.last_network_activity.elapsed() >= INBOUND_CALL_IDLE_TIMEOUT)
            .map(|(uuid, call)| (*uuid, call.dest_gssi == 0))
            .collect();

        for (uuid, is_circuit) in stale {
            tracing::warn!(
                "BrewEntity: reaping inbound call uuid={} — no network media for {}s",
                uuid,
                INBOUND_CALL_IDLE_TIMEOUT.as_secs()
            );
            if is_circuit {
                // Individual/circuit call (registered by NetworkCircuitMediaReady, gssi 0):
                // release it the same way an inbound CALL_RELEASE would.
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Brew,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid: uuid, cause: 0 }),
                });
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendCallRelease { uuid, cause: 0 });
                }
                self.drop_network_circuit(uuid);
            } else {
                // Group call: take exactly the GROUP_IDLE path we never received, so the
                // circuit still gets its normal hangtime reuse window.
                self.handle_group_call_end(queue, uuid, 0);
            }
        }

        let stale_held: Vec<u32> = self
            .pending_inbound
            .iter()
            .filter(|(_, p)| p.last_network_activity.elapsed() >= INBOUND_CALL_IDLE_TIMEOUT)
            .map(|(&gssi, _)| gssi)
            .collect();
        for gssi in stale_held {
            if let Some(p) = self.pending_inbound.remove(&gssi) {
                tracing::warn!(
                    "BrewEntity: reaping pending-inbound gssi={} uuid={} — no network media for {}s",
                    gssi,
                    p.uuid,
                    INBOUND_CALL_IDLE_TIMEOUT.as_secs()
                );
                self.dl_jitter.remove(&p.uuid);
            }
        }
    }

    fn queue_external_subscriber_update(
        &self,
        queue: &mut MessageQueue,
        issi: u32,
        groups: Vec<u32>,
        action: BrewSubscriberAction,
        action_label: &str,
    ) {
        if !super::is_brew_external_subscriber_allowed(&self.config, issi) {
            tracing::trace!(
                "BrewEntity: external subscriber issi={} → {} ignored (local SSI range)",
                issi,
                action_label
            );
            return;
        }

        tracing::trace!(
            "BrewEntity: external subscriber issi={} → {} groups={:?}",
            issi,
            action_label,
            groups
        );
        queue.push_back(SapMsg {
            sap: tetra_core::Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
        });
    }

    /// Handle RSSI update from MM. Forwards to Brew server if feature_rssi_export is enabled,
    /// applying rate limiting (one update per MS every 5 seconds) to avoid flooding the server.
    fn handle_rssi_update(&mut self, issi: u32, rssi_dbfs: f32) {
        let brew_cfg = self.config.config();
        let Some(ref brew) = brew_cfg.brew else {
            return;
        };
        if !brew.feature_rssi_export {
            return;
        }
        if !self.connected {
            return;
        }

        const RSSI_EXPORT_INTERVAL: Duration = Duration::from_secs(5);

        let now = Instant::now();
        let should_send = match self.rssi_last_sent.get(&issi) {
            None => true,
            Some(last) => now.duration_since(*last) >= RSSI_EXPORT_INTERVAL,
        };

        if should_send {
            self.rssi_last_sent.insert(issi, now);
            let _ = self.command_sender.send(BrewCommand::SendRssiUpdate { issi, rssi_dbfs });
            tracing::debug!("Brew: queued RSSI export issi={} rssi={:.1}dBFS", issi, rssi_dbfs);
        }
    }

    fn handle_subscriber_update(&mut self, update: MmSubscriberUpdate) {
        let issi = update.issi;
        let groups = update.groups;
        let routable = super::is_brew_issi_routable(&self.config, issi);

        match update.action {
            BrewSubscriberAction::Register => {
                self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                if routable && self.connected {
                    tracing::info!("BrewEntity: subscriber register issi={} → REGISTER", issi);
                    let _ = self.command_sender.send(BrewCommand::RegisterSubscriber { issi });
                    self.report_ms_cell(issi);
                } else if !routable {
                    tracing::debug!("BrewEntity: subscriber register issi={} (filtered, not sent to Brew)", issi);
                } else {
                    // routable but disconnected — affiliations replayed on reconnect via resync
                    tracing::debug!("BrewEntity: subscriber register issi={} cached until Brew reconnect", issi);
                }
            }
            BrewSubscriberAction::Deregister => {
                let existing_groups: Vec<u32> = self
                    .subscriber_groups
                    .remove(&issi)
                    .map(|g| g.into_iter().collect())
                    .unwrap_or_default();
                if self.moved_away.remove(&issi).is_some() {
                    tracing::info!("BrewEntity: ISSI {} moved to another station, dropped here without DEREGISTER", issi);
                } else if routable && self.connected {
                    tracing::info!("BrewEntity: subscriber deregister issi={} → DEAFFILIATE + DEREGISTER", issi);
                    if !existing_groups.is_empty() {
                        let _ = self.command_sender.send(BrewCommand::DeaffiliateGroups {
                            issi,
                            groups: existing_groups,
                        });
                    }
                    let _ = self.command_sender.send(BrewCommand::DeregisterSubscriber { issi });
                } else if !routable {
                    tracing::debug!("BrewEntity: subscriber deregister issi={} (filtered, not sent to Brew)", issi);
                } else {
                    tracing::debug!("BrewEntity: subscriber deregister issi={} cached while Brew disconnected", issi);
                }
            }
            BrewSubscriberAction::Affiliate => {
                let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                let mut new_groups = Vec::new();
                for gssi in groups {
                    if entry.insert(gssi) {
                        new_groups.push(gssi);
                    }
                }
                if !new_groups.is_empty() && routable && self.connected {
                    tracing::info!("BrewEntity: affiliate issi={} → AFFILIATE groups={:?}", issi, new_groups);
                    let _ = self.command_sender.send(BrewCommand::AffiliateGroups { issi, groups: new_groups });
                } else if !new_groups.is_empty() && !routable {
                    tracing::debug!(
                        "BrewEntity: affiliate issi={} groups={:?} (filtered, not sent to Brew)",
                        issi,
                        new_groups
                    );
                } else if !new_groups.is_empty() {
                    tracing::debug!(
                        "BrewEntity: affiliate issi={} groups={:?} cached until Brew reconnect",
                        issi,
                        new_groups
                    );
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                let mut removed_groups = Vec::new();
                if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                    for gssi in groups {
                        if entry.remove(&gssi) {
                            removed_groups.push(gssi);
                        }
                    }
                }
                if self.moved_away.contains_key(&issi) {
                    // Dropped because it moved away: the other station holds its groups now.
                } else if !removed_groups.is_empty() && routable && self.connected {
                    tracing::info!("BrewEntity: deaffiliate issi={} → DEAFFILIATE groups={:?}", issi, removed_groups);
                    let _ = self.command_sender.send(BrewCommand::DeaffiliateGroups {
                        issi,
                        groups: removed_groups,
                    });
                } else if !removed_groups.is_empty() && !routable {
                    tracing::debug!(
                        "BrewEntity: deaffiliate issi={} groups={:?} (filtered, not sent to Brew)",
                        issi,
                        removed_groups
                    );
                } else if !removed_groups.is_empty() {
                    tracing::debug!(
                        "BrewEntity: deaffiliate issi={} groups={:?} cached while Brew disconnected",
                        issi,
                        removed_groups
                    );
                }
            }
        }
    }

    fn resync_subscribers(&self) {
        for issi in self.subscriber_groups.keys() {
            if !super::is_brew_issi_routable(&self.config, *issi) {
                tracing::debug!("BrewEntity: resync skipping issi={} (filtered)", issi);
                continue;
            }
            self.register_with_groups(*issi, "resync");
        }
    }

    /// REGISTER `issi` with the server and affiliate the groups it has here.
    fn register_with_groups(&self, issi: u32, why: &str) {
        let Some(groups) = self.subscriber_groups.get(&issi) else { return };
        let _ = self.command_sender.send(BrewCommand::RegisterSubscriber { issi });
        self.report_ms_cell(issi);
        if groups.is_empty() {
            tracing::info!("BrewEntity: {} issi={} — registered, no group affiliations", why, issi);
        } else {
            let gssi_list: Vec<u32> = groups.iter().copied().collect();
            tracing::info!(
                "BrewEntity: {} issi={} — registered, affiliating {} groups: {:?}",
                why,
                issi,
                gssi_list.len(),
                gssi_list
            );
            let _ = self.command_sender.send(BrewCommand::AffiliateGroups { issi, groups: gssi_list });
        }
    }

    fn set_network_connected(&mut self, connected: bool, server_version: u8) {
        // SYSINFO / site-trunking bit only — transport liveness is `self.connected` / brew_link_up.
        let changed = {
            let mut state = self.config.state_write();
            if state.network_connected != connected {
                state.network_connected = connected;
                tracing::info!(
                    "BrewEntity: SYSINFO system_wide_services {}",
                    if connected { "ENABLED" } else { "DISABLED (site trunking)" }
                );
                true
            } else {
                false
            }
        };
        if changed {
            if let Some(ref sink) = self.telemetry_sink {
                let _ = sink.send(TelemetryEvent::BrewConnected {
                    connected: self.connected,
                    server_version,
                });
            }
        }
    }

    /// Apply deferred site-trunking announcement after hysteresis expires.
    fn apply_pending_sysinfo_disconnect(&mut self) {
        let Some(at) = self.pending_sysinfo_disconnect_at else {
            return;
        };
        if self.connected {
            self.pending_sysinfo_disconnect_at = None;
            return;
        }
        if Instant::now() < at {
            return;
        }
        self.pending_sysinfo_disconnect_at = None;
        tracing::info!("BrewEntity: hysteresis elapsed — announcing site-trunking (system_wide_services=false)");
        self.set_network_connected(false, 0);
    }

    /// Emit a brew version upgrade event directly without changing connection state.
    fn emit_brew_version(&self, version: u8) {
        if let Some(ref sink) = self.telemetry_sink {
            let _ = sink.send(TelemetryEvent::BrewConnected {
                connected: true,
                server_version: version,
            });
        }
    }

    /// Finish a deferred GROUP_IDLE: NetworkCallEnd + hanging_calls.
    fn finalize_pending_tail(&mut self, queue: &mut MessageQueue, uuid: Uuid, tail: PendingTail, reason: &str) {
        let left = tail.jitter.len();
        if left > 0 {
            tracing::info!(
                "BrewEntity: pending tail {} uuid={} ({} frames discarded) — NetworkCallEnd",
                reason,
                uuid,
                left
            );
        } else {
            tracing::info!(
                "BrewEntity: pending tail {} uuid={} — NetworkCallEnd",
                reason,
                uuid
            );
        }

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
        });

        if let Some(stale) = self.hanging_calls.remove(&tail.dest_gssi) {
            if stale.uuid != uuid {
                self.dl_jitter.remove(&stale.uuid);
                self.pending_tails.remove(&stale.uuid);
            }
        }
        self.hanging_calls.insert(
            tail.dest_gssi,
            HangingCall {
                uuid,
                call_id: tail.call_id,
                carrier_num: tail.carrier_num,
                ts: tail.ts,
                usage: tail.usage,
                source_issi: tail.source_issi,
                dest_gssi: tail.dest_gssi,
                frame_count: tail.frame_count,
                since: Instant::now(),
            },
        );
    }

    fn abort_pending_tails_on_slot(&mut self, queue: &mut MessageQueue, carrier_num: u16, ts: u8, reason: &str) {
        let doomed: Vec<Uuid> = self
            .pending_tails
            .iter()
            .filter(|(_, t)| t.carrier_num == carrier_num && t.ts == ts)
            .map(|(uuid, _)| *uuid)
            .collect();
        for uuid in doomed {
            if let Some(t) = self.pending_tails.remove(&uuid) {
                self.finalize_pending_tail(queue, uuid, t, reason);
            }
        }
    }

    fn take_pending_tail_for_gssi(&mut self, dest_gssi: u32) -> Option<(Uuid, PendingTail)> {
        let uuid = self
            .pending_tails
            .iter()
            .find(|(_, t)| t.dest_gssi == dest_gssi)
            .map(|(uuid, _)| *uuid)?;
        self.pending_tails.remove(&uuid).map(|t| (uuid, t))
    }

    /// Handle new group call from Brew, reusing hanging call circuits if available.
    fn handle_group_call_start(&mut self, queue: &mut MessageQueue, uuid: Uuid, source_issi: u32, dest_gssi: u32, priority: u8) {
        // Check if this call is already active (speaker change or repeated GROUP_TX)
        if let Some(call) = self.active_calls.get_mut(&uuid) {
            // A repeated GROUP_TX is also a keepalive — it must hold off the idle reaper.
            call.last_network_activity = Instant::now();
            call.priority = priority;
            // Only notify CMCE if the speaker actually changed
            if call.source_issi != source_issi {
                tracing::info!(
                    "BrewEntity: GROUP_TX speaker change on uuid={} new_speaker={} (was {})",
                    uuid,
                    source_issi,
                    call.source_issi
                );
                call.source_issi = source_issi;

                // Flush stale audio from previous speaker immediately.
                // ETSI EN 300 392-2 §14.8.43: when transmission grant changes,
                // the previous speaker's audio must not be forwarded to the new speaker.
                if let Some(jitter) = self.dl_jitter.get_mut(&uuid) {
                    let dropped = jitter.flush();
                    if dropped > 0 {
                        tracing::debug!(
                            "BrewEntity: flushed {} stale frames from jitter on speaker change uuid={}",
                            dropped,
                            uuid
                        );
                    }
                }

                // Forward speaker change to CMCE
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Brew,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                        brew_uuid: uuid,
                        source_issi,
                        dest_gssi,
                        priority,
                    }),
                });
            } else {
                // Repeated GROUP_TX with same speaker - this is normal, just log at trace level
                tracing::trace!("BrewEntity: repeated GROUP_TX on uuid={} speaker={}", uuid, source_issi);
            }
            return;
        }

        // Already held awaiting listeners — refresh keepalive / replace session uuid.
        if let Some(pending) = self.pending_inbound.get_mut(&dest_gssi) {
            if pending.uuid != uuid {
                let old = pending.uuid;
                self.dl_jitter.remove(&old);
                pending.uuid = uuid;
                pending.source_issi = source_issi;
                pending.priority = priority;
                self.dl_jitter.entry(uuid).or_insert_with(|| {
                    VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize)
                });
                tracing::info!(
                    "BrewEntity: pending-inbound gssi={} uuid {} → {} (new GROUP_TX)",
                    dest_gssi,
                    old,
                    uuid
                );
            } else {
                pending.source_issi = source_issi;
                pending.priority = priority;
                pending.last_network_activity = Instant::now();
                tracing::trace!("BrewEntity: pending-inbound keepalive gssi={} uuid={}", dest_gssi, uuid);
            }
            return;
        }

        // Reclaim a circuit still Transmitting in CMCE while we play the previous tail.
        // Prefer finishing the old syllable on air: migrate leftover frames to the new
        // call's jitter (FIFO) instead of dropping them or letting them leak after hangtime.
        if let Some((old_uuid, mut tail)) = self.take_pending_tail_for_gssi(dest_gssi) {
            let left = tail.jitter.len();
            tracing::info!(
                "BrewEntity: reclaiming pending tail gssi={} old_uuid={} → uuid={} ({} frames carried)",
                dest_gssi,
                old_uuid,
                uuid,
                left
            );
            let call = ActiveCall {
                uuid,
                call_id: Some(tail.call_id),
                ts: Some(tail.ts),
                carrier_num: Some(tail.carrier_num),
                usage: Some(tail.usage),
                source_issi,
                dest_gssi,
                priority,
                frame_count: tail.frame_count,
                last_media_activity_signal: None,
                last_network_activity: Instant::now(),
            };
            self.active_calls.insert(uuid, call);
            let mut jitter = VoiceJitterBuffer::with_initial_latency(
                self.brew_config.jitter_initial_latency_frames as usize,
            );
            while let Some(frame) = tail.jitter.pop_drain() {
                jitter.push(frame.acelp_data);
            }
            jitter.force_started();
            self.dl_jitter.insert(uuid, jitter);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                    brew_uuid: uuid,
                    source_issi,
                    dest_gssi,
                    priority,
                }),
            });
            return;
        }

        // Admission control: every tracked call pins a CMCE circuit and a jitter buffer, so a
        // core that opens calls with ever-fresh UUIDs must not be able to exhaust them. Reuse
        // of a hanging circuit is checked first below — this only gates genuinely new calls.
        if !self.hanging_calls.contains_key(&dest_gssi) && self.active_calls.len() >= MAX_INBOUND_ACTIVE_CALLS {
            tracing::warn!(
                "BrewEntity: refusing GROUP_TX uuid={} gssi={} — {} inbound calls already active (cap {})",
                uuid,
                dest_gssi,
                self.active_calls.len(),
                MAX_INBOUND_ACTIVE_CALLS
            );
            return;
        }

        // Check if there's a hanging call we can reuse
        if let Some(hanging) = self.hanging_calls.remove(&dest_gssi) {
            tracing::info!(
                "BrewEntity: reusing hanging circuit for gssi={} uuid={} (hangtime {:.1}s)",
                dest_gssi,
                uuid,
                hanging.since.elapsed().as_secs_f32()
            );

            // Track the call. Carry over the resources the hanging circuit already
            // had (call_id/ts/usage) as a fallback: when CMCE reuses the circuit it
            // may not emit a fresh NetworkCallReady, and if these stayed None the call
            // could never be re-marked as hanging on its next end — leaking the circuit
            // and breaking reuse on subsequent PTTs. NetworkCallReady, if it does come,
            // will simply overwrite these with identical values.
            let call = ActiveCall {
                uuid,
                call_id: Some(hanging.call_id),
                ts: Some(hanging.ts),
                carrier_num: Some(hanging.carrier_num),
                usage: Some(hanging.usage),
                source_issi,
                dest_gssi,
                priority,
                frame_count: hanging.frame_count,
                last_media_activity_signal: None,
                last_network_activity: Instant::now(),
            };
            self.active_calls.insert(uuid, call);
            self.dl_jitter
                .entry(uuid)
                .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));

            // Forward to CMCE (will reuse circuit automatically)
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                    brew_uuid: uuid,
                    source_issi,
                    dest_gssi,
                    priority,
                }),
            });
            return;
        }

        // New call - track it and request CMCE to allocate and set up
        tracing::info!(
            "BrewEntity: requesting new network call uuid={} src={} gssi={}",
            uuid,
            source_issi,
            dest_gssi
        );

        // Track the call - resources will be set by NetworkCallReady
        let call = ActiveCall {
            uuid,
            call_id: None, // Set by NetworkCallReady
            ts: None,      // Set by NetworkCallReady
            carrier_num: None,
            usage: None, // Set by NetworkCallReady
            source_issi,
            dest_gssi,
            priority,
            frame_count: 0,
            last_media_activity_signal: None,
            last_network_activity: Instant::now(),
        };
        self.active_calls.insert(uuid, call);
        self.dl_jitter
            .entry(uuid)
            .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                brew_uuid: uuid,
                source_issi,
                dest_gssi,
                priority,
            }),
        });
    }

    /// Handle GROUP_IDLE: play remaining jitter first, then NetworkCallEnd / hangtime.
    fn handle_group_call_end(&mut self, queue: &mut MessageQueue, uuid: Uuid, _cause: u8) {
        // Clear held inbound if the core idle'd before any local listener arrived.
        let held_gssi = self
            .pending_inbound
            .iter()
            .find_map(|(gssi, p)| if p.uuid == uuid { Some(*gssi) } else { None });
        if let Some(gssi) = held_gssi {
            tracing::info!("BrewEntity: GROUP_IDLE for pending-inbound gssi={} uuid={}", gssi, uuid);
            self.pending_inbound.remove(&gssi);
            self.dl_jitter.remove(&uuid);
            return;
        }

        let Some(call) = self.active_calls.remove(&uuid) else {
            tracing::debug!("BrewEntity: GROUP_IDLE for unknown uuid={}", uuid);
            return;
        };

        // CALL_RELEASE reaches here too (the worker sends both end events); for an individual
        // call the circuit release that follows does the work, so only the label differs.
        let individual = self.ul_forwarded.values().any(|f| f.uuid == uuid && f.kind == UlForwardKind::Circuit);
        tracing::info!(
            "BrewEntity: {} ended uuid={} call_id={:?} gssi={} frames={}",
            if individual { "individual call" } else { "group call" },
            uuid,
            call.call_id,
            call.dest_gssi,
            call.frame_count
        );

        let jitter = self.dl_jitter.remove(&uuid);
        // Always defer NetworkCallEnd while we still own carrier/ts: even an empty Brew
        // jitter may have just queued TmdCircuitDataReq into UMAC. Ending immediately puts
        // the slot into hangtime signalling and those last TCH frames never reach the air
        // (or leak as a "ghost syllable" when the next GROUP_TX re-enables traffic).
        if let (Some(call_id), Some(carrier_num), Some(ts), Some(usage)) =
            (call.call_id, call.carrier_num, call.ts, call.usage)
        {
            let jitter = jitter.unwrap_or_default();
            let left = jitter.len();
            tracing::info!(
                "BrewEntity: GROUP_IDLE uuid={} pending tail {} frames (defer NetworkCallEnd)",
                uuid,
                left
            );
            self.pending_tails.insert(
                uuid,
                PendingTail {
                    carrier_num,
                    ts,
                    dest_gssi: call.dest_gssi,
                    call_id,
                    usage,
                    source_issi: call.source_issi,
                    frame_count: call.frame_count,
                    jitter,
                    started: Instant::now(),
                    last_media: Instant::now(),
                },
            );
            return;
        }
        if let Some(jitter) = jitter {
            if !jitter.is_empty() {
                tracing::debug!(
                    "BrewEntity: GROUP_IDLE uuid={} dropping {} buffered frames (no carrier/ts)",
                    uuid,
                    jitter.len()
                );
            }
        }
        self.pending_tails.remove(&uuid);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
        });

        // No circuit resources to hang — nothing to reuse on the next GROUP_TX.
    }

    /// Clean up expired hanging call tracking hints (CMCE already released circuits)
    fn expire_hanging_calls(&mut self, _queue: &mut MessageQueue) {
        let hangtime = Duration::from_secs(self.config.config().cell.hangtime_secs as u64);
        let expired: Vec<u32> = self
            .hanging_calls
            .iter()
            .filter(|(_, h)| h.since.elapsed() >= hangtime)
            .map(|(gssi, _)| *gssi)
            .collect();

        for gssi in expired {
            if let Some(hanging) = self.hanging_calls.remove(&gssi) {
                tracing::debug!("BrewEntity: hanging call expired gssi={} uuid={} (no reuse)", gssi, hanging.uuid);
                // No action needed - CMCE already released the circuit
            }
        }
    }

    /// Handle a voice frame from Brew — inject into the downlink
    fn handle_voice_frame(&mut self, queue: &mut MessageQueue, uuid: Uuid, _length_bits: u16, data: Vec<u8>) {
        let Some(call) = self.active_calls.get_mut(&uuid) else {
            // GROUP_IDLE may race ahead of the last STE frames — keep feeding the pending tail.
            if let Some(tail) = self.pending_tails.get_mut(&uuid) {
                if data.len() < 36 {
                    tracing::warn!(
                        "BrewEntity: late tail voice too short ({} bytes) uuid={}",
                        data.len(),
                        uuid
                    );
                    return;
                }
                tail.jitter.push(data[1..].to_vec());
                tail.last_media = Instant::now();
                tail.frame_count = tail.frame_count.saturating_add(1);
                tracing::trace!("BrewEntity: late voice into pending tail uuid={}", uuid);
                return;
            }
            // Held inbound (no listeners yet): buffer voice until promote allocates a circuit.
            if let Some((gssi, pending)) = self
                .pending_inbound
                .iter_mut()
                .find(|(_, p)| p.uuid == uuid)
                .map(|(g, p)| (*g, p))
            {
                pending.last_network_activity = Instant::now();
                if data.len() < 36 {
                    return;
                }
                self.dl_jitter
                    .entry(uuid)
                    .or_insert_with(|| {
                        VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize)
                    })
                    .push(data[1..].to_vec());
                tracing::trace!("BrewEntity: voice into pending-inbound gssi={} uuid={}", gssi, uuid);
                return;
            }
            tracing::trace!("BrewEntity: voice frame for unknown uuid={} ({} bytes)", uuid, data.len());
            return;
        };

        call.frame_count += 1;
        call.last_network_activity = Instant::now();
        let frame_count = call.frame_count;
        const NETWORK_CALL_ACTIVITY_REFRESH_TS: i32 = 72; // ~1 second
        let should_refresh_activity = call.dest_gssi != 0
            && call
                .last_media_activity_signal
                .is_none_or(|last| last.age(self.dltime) >= NETWORK_CALL_ACTIVITY_REFRESH_TS);
        if should_refresh_activity {
            call.last_media_activity_signal = Some(self.dltime);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallMediaActivity { brew_uuid: uuid }),
            });
        }
        // `ts` is None until NetworkCallReady opens the traffic channel (a brief window after
        // NetworkCallStart). We still buffer early frames below — see the playout note.
        let carrier_num = call.carrier_num;
        let ts = call.ts;
        // mutable borrow of `call` ends here; the jitter push below borrows other fields of `self`.

        // STE format: byte 0 = header (control bits), bytes 1-35 = 274 ACELP bits for TCH/S.
        // Validate before slicing so a short frame can never panic.
        if data.len() < 36 {
            tracing::warn!("BrewEntity: voice frame too short ({} bytes, expected 36 STE bytes)", data.len());
            return;
        }

        // Log the first frame per call (whether or not the channel is open yet).
        if frame_count == 1 {
            match ts {
                Some(ts) => tracing::info!(
                    "BrewEntity: voice frame #1 uuid={} len={} bytes carrier={} ts={}",
                    uuid,
                    data.len(),
                    carrier_num.unwrap_or_default(),
                    ts
                ),
                None => tracing::debug!("BrewEntity: early voice frame uuid={} buffered before resources allocated", uuid),
            }
        }

        // Buffer the ACELP payload even before the traffic channel is allocated (ts == None):
        // drain_jitter_playout only emits once call.ts is Some, so these early frames are HELD, not
        // dropped, eliminating the brief clip at the very start of a Brew-originated transmission
        // (FH-FEAT-032 R1 — the feature's MCCH channel allocation was already correct; this fixes
        // only the first-frame clip). The jitter buffer is pre-allocated and hard-capped, and this
        // runs on the Brew entity tick — off the RF/voice scheduler.
        let acelp_data = data[1..].to_vec(); // 35 bytes = 280 bits, of which 274 are ACELP
        self.dl_jitter
            .entry(uuid)
            .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize))
            .push(acelp_data);
    }

    fn drain_jitter_playout(&mut self, queue: &mut MessageQueue) {
        if self.dltime.f == 18 {
            return;
        }

        let mut to_send: Vec<(u16, u8, Uuid, usize, JitterFrame)> = Vec::new();

        for (uuid, call) in &self.active_calls {
            let (Some(carrier_num), Some(ts)) = (call.carrier_num, call.ts) else {
                continue;
            };
            if ts != self.dltime.t {
                continue;
            }
            let Some(jitter) = self.dl_jitter.get_mut(uuid) else {
                continue;
            };
            jitter.maybe_warn_unhealthy(*uuid);
            if let Some(frame) = jitter.pop_ready() {
                to_send.push((carrier_num, ts, *uuid, jitter.target_frames(), frame));
            }
        }

        // Pending tails: CMCE still Transmitting — play media, then quiet long enough for
        // UMAC to air the last TmdCircuitDataReq before NetworkCallEnd flips hangtime.
        // ~1 TDMA frame is 56.67 ms; a few frames of pipeline + one missed slot ⇒ ~400 ms.
        const TAIL_REAP_AFTER: Duration = Duration::from_millis(2500);
        const TAIL_QUIET_AFTER: Duration = Duration::from_millis(400);
        let finished: Vec<Uuid> = self
            .pending_tails
            .iter_mut()
            .filter_map(|(uuid, tail)| {
                if tail.started.elapsed() >= TAIL_REAP_AFTER {
                    return Some(*uuid);
                }
                let empty = tail.jitter.is_empty();
                if empty && tail.last_media.elapsed() >= TAIL_QUIET_AFTER {
                    return Some(*uuid);
                }
                if tail.ts != self.dltime.t {
                    return None;
                }
                match tail.jitter.pop_drain() {
                    Some(frame) => {
                        tail.last_media = Instant::now();
                        to_send.push((tail.carrier_num, tail.ts, *uuid, 0, frame));
                        None
                    }
                    None => {
                        if tail.last_media.elapsed() >= TAIL_QUIET_AFTER {
                            Some(*uuid)
                        } else {
                            None
                        }
                    }
                }
            })
            .collect();

        for (carrier_num, ts, uuid, target_frames, frame) in to_send {
            tracing::trace!(
                "BrewEntity: playout uuid={} carrier={} ts={} rx_seq={} age_ms={} target_frames={}",
                uuid,
                carrier_num,
                ts,
                frame.rx_seq,
                frame.rx_at.elapsed().as_millis(),
                target_frames
            );
            queue.push_back(SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Brew,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
                    carrier_num,
                    ts,
                    data: frame.acelp_data,
                }),
            });
        }
        for uuid in finished {
            if let Some(t) = self.pending_tails.remove(&uuid) {
                self.finalize_pending_tail(queue, uuid, t, "complete");
            }
        }
    }

    /// Release all active calls (on disconnect)
    fn release_all_calls(&mut self, queue: &mut MessageQueue) {
        let calls: Vec<(Uuid, ActiveCall)> = self.active_calls.drain().collect();
        for (uuid, _) in calls {
            self.dl_jitter.remove(&uuid);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
            });
        }

        let tails: Vec<(Uuid, PendingTail)> = self.pending_tails.drain().collect();
        for (uuid, _) in tails {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: uuid }),
            });
        }

        for (_, pending) in self.pending_inbound.drain() {
            self.dl_jitter.remove(&pending.uuid);
        }

        self.hanging_calls.clear();
        self.dl_jitter.clear();

        if !self.ul_forwarded.is_empty() {
            tracing::debug!(
                "BrewEntity: clearing {} stale ul_forwarded entries on release",
                self.ul_forwarded.len()
            );
            self.ul_forwarded.clear();
        }
    }

    /// Handle NetworkCallReady response from CMCE
    fn rx_network_call_ready(&mut self, queue: &mut MessageQueue, brew_uuid: Uuid, call_id: u16, carrier_num: u16, ts: u8, usage: u8) {
        tracing::info!(
            "BrewEntity: network call ready uuid={} call_id={} carrier={} ts={} usage={}",
            brew_uuid,
            call_id,
            carrier_num,
            ts,
            usage
        );

        // Update active call with CMCE-allocated resources
        if let Some(call) = self.active_calls.get_mut(&brew_uuid) {
            call.call_id = Some(call_id);
            call.carrier_num = Some(carrier_num);
            call.ts = Some(ts);
            call.usage = Some(usage);
        } else if let Some(tail) = self.pending_tails.get_mut(&brew_uuid) {
            // GROUP_IDLE raced ahead of Ready — keep playing the tail on the allocated slot.
            tail.call_id = call_id;
            tail.carrier_num = carrier_num;
            tail.ts = ts;
            tail.usage = usage;
            tracing::info!(
                "BrewEntity: NetworkCallReady applied to pending tail uuid={} call_id={} ts={}",
                brew_uuid,
                call_id,
                ts
            );
        } else {
            // Race: CMCE finished allocating a circuit (call_id/ts/usage) for a call that
            // Brew has already torn down — e.g. a GROUP_IDLE or disconnect arrived between
            // our NetworkCallStart and this NetworkCallReady. If we just drop this on the
            // floor, CMCE keeps the circuit allocated forever (orphaned ts/usage that no
            // GROUP_IDLE will ever release). Tell CMCE to release it so the timeslot is
            // freed for the next call.
            tracing::warn!(
                "BrewEntity: NetworkCallReady for unknown uuid={} (call already gone) — releasing orphaned circuit call_id={} ts={}",
                brew_uuid,
                call_id,
                ts
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
            });
        }
    }

    /// Drop an active circuit call state. Returns true if there was an active circuit.
    /// Flushes the jitter buffer immediately to prevent audio from being sent to a
    /// closed circuit (EN 300 392-2 §14.9: resources must be released immediately on disconnect).
    fn drop_network_circuit(&mut self, brew_uuid: Uuid) -> bool {
        // Flush and remove jitter buffer immediately — prevents DL voice frames
        // from being sent to UMAC after the circuit is already closed.
        let had_jitter = self.dl_jitter.remove(&brew_uuid).is_some();
        self.pending_tails.remove(&brew_uuid);
        let slots_to_remove: Vec<(u16, u8)> = self
            .ul_forwarded
            .iter()
            .filter_map(|(&(carrier_num, ts), fwd)| {
                if fwd.uuid == brew_uuid && fwd.kind == UlForwardKind::Circuit {
                    Some((carrier_num, ts))
                } else {
                    None
                }
            })
            .collect();
        let had_ts = !slots_to_remove.is_empty();
        for slot in slots_to_remove {
            self.ul_forwarded.remove(&slot);
        }
        // Remove from active_calls — circuit calls are registered there by NetworkCircuitMediaReady
        // so that DL audio from TetraPack reaches the MS. Clean up here to avoid stale entries.
        self.active_calls.remove(&brew_uuid);
        if had_jitter || had_ts {
            tracing::info!("BrewEntity: dropped circuit uuid={}", brew_uuid);
        } else {
            tracing::debug!("BrewEntity: drop_network_circuit for unknown uuid={}", brew_uuid);
        }
        had_jitter || had_ts
    }

    fn drop_network_call(&mut self, brew_uuid: Uuid) {
        if let Some(call) = self.active_calls.remove(&brew_uuid) {
            tracing::info!(
                "BrewEntity: dropping network call uuid={} gssi={} (CMCE request)",
                brew_uuid,
                call.dest_gssi
            );
            self.dl_jitter.remove(&brew_uuid);
            // Drop pending tail — CMCE already tore the call down.
            self.pending_tails.remove(&brew_uuid);
            self.hanging_calls.remove(&call.dest_gssi);
            return;
        }

        let hanging_gssi = self
            .hanging_calls
            .iter()
            .find_map(|(gssi, hanging)| if hanging.uuid == brew_uuid { Some(*gssi) } else { None });
        if let Some(gssi) = hanging_gssi {
            tracing::info!("BrewEntity: dropping hanging call uuid={} gssi={} (CMCE request)", brew_uuid, gssi);
            self.hanging_calls.remove(&gssi);
            return;
        }
        if self.pending_tails.remove(&brew_uuid).is_some() {
            tracing::info!("BrewEntity: dropping pending tail uuid={} (CMCE request)", brew_uuid);
            return;
        }
        let held_gssi = self
            .pending_inbound
            .iter()
            .find_map(|(gssi, p)| if p.uuid == brew_uuid { Some(*gssi) } else { None });
        if let Some(gssi) = held_gssi {
            tracing::info!("BrewEntity: dropping pending-inbound gssi={} uuid={}", gssi, brew_uuid);
            self.pending_inbound.remove(&gssi);
            self.dl_jitter.remove(&brew_uuid);
            return;
        }
        tracing::debug!("BrewEntity: drop requested for unknown uuid={}", brew_uuid);
    }

    /// A network talker came to this cell unannounced and CMCE gave it the floor of the over it
    /// was heard on. Its old session (sent by the station it left) ends here: no more downlink
    /// from it, and its late GROUP_IDLE finds nothing.
    fn drop_moved_talker_session(&mut self, carrier_num: u16, ts: u8, source_issi: u32, dest_gssi: u32) {
        let moved: Vec<Uuid> = self
            .active_calls
            .values()
            .filter(|c| {
                c.source_issi == source_issi
                    && c.dest_gssi == dest_gssi
                    && c.carrier_num == Some(carrier_num)
                    && c.ts == Some(ts)
            })
            .map(|c| c.uuid)
            .collect();
        for uuid in moved {
            tracing::info!(
                "BrewEntity: talker {} keyed up here, ending its network session uuid={} gssi={}",
                source_issi,
                uuid,
                dest_gssi
            );
            self.drop_network_call(uuid);
        }
    }

    /// CMCE had no listeners — hold the session until Affiliate promotes it (no circuit yet).
    fn hold_inbound_no_listeners(&mut self, brew_uuid: Uuid, dest_gssi: u32) {
        let Some(call) = self.active_calls.remove(&brew_uuid) else {
            // Already moved / raced — if uuid matches an existing pending, refresh.
            if let Some(p) = self.pending_inbound.get_mut(&dest_gssi) {
                if p.uuid == brew_uuid {
                    p.last_network_activity = Instant::now();
                }
            }
            tracing::debug!(
                "BrewEntity: NetworkCallHold for unknown uuid={} gssi={}",
                brew_uuid,
                dest_gssi
            );
            return;
        };
        // Cap: drop oldest held GSSI if full (cheap LRU by last_network_activity).
        if !self.pending_inbound.contains_key(&dest_gssi) && self.pending_inbound.len() >= MAX_PENDING_INBOUND {
            if let Some((&old_gssi, _)) = self
                .pending_inbound
                .iter()
                .min_by_key(|(_, p)| p.last_network_activity)
            {
                if let Some(old) = self.pending_inbound.remove(&old_gssi) {
                    self.dl_jitter.remove(&old.uuid);
                    tracing::warn!(
                        "BrewEntity: pending-inbound cap — dropped gssi={} uuid={}",
                        old_gssi,
                        old.uuid
                    );
                }
            }
        }
        // Replace any prior held session on this GSSI.
        if let Some(old) = self.pending_inbound.remove(&dest_gssi) {
            self.dl_jitter.remove(&old.uuid);
        }
        tracing::info!(
            "BrewEntity: holding inbound uuid={} gssi={} (awaiting listeners)",
            brew_uuid,
            dest_gssi
        );
        self.pending_inbound.insert(
            dest_gssi,
            PendingInbound {
                uuid: brew_uuid,
                source_issi: call.source_issi,
                priority: call.priority,
                last_network_activity: Instant::now(),
            },
        );
        // Keep jitter buffer keyed by uuid for voice that arrives while held.
        self.dl_jitter.entry(brew_uuid).or_insert_with(|| {
            VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize)
        });
    }

    /// First local listener appeared — re-issue NetworkCallStart so CMCE allocates + LE D-SETUP.
    fn promote_pending_inbound(&mut self, queue: &mut MessageQueue, gssi: u32) {
        let Some(pending) = self.pending_inbound.remove(&gssi) else {
            return;
        };
        tracing::info!(
            "BrewEntity: promoting pending-inbound gssi={} uuid={} speaker={}",
            gssi,
            pending.uuid,
            pending.source_issi
        );
        let call = ActiveCall {
            uuid: pending.uuid,
            call_id: None,
            ts: None,
            carrier_num: None,
            usage: None,
            source_issi: pending.source_issi,
            dest_gssi: gssi,
            priority: pending.priority,
            frame_count: 0,
            last_media_activity_signal: None,
            last_network_activity: Instant::now(),
        };
        self.active_calls.insert(pending.uuid, call);
        self.dl_jitter.entry(pending.uuid).or_insert_with(|| {
            VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize)
        });
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
                brew_uuid: pending.uuid,
                source_issi: pending.source_issi,
                dest_gssi: gssi,
                priority: pending.priority,
            }),
        });
    }
}

// ─── TetraEntityTrait implementation ──────────────────────────────

impl TetraEntityTrait for BrewEntity {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Brew
    }

    fn set_config(&mut self, config: SharedConfig) {
        if let Some(brew) = config.config().brew.clone() {
            self.brew_config = brew;
        }
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        // Process all pending events from the worker thread
        self.process_events(queue);
        self.apply_pending_sysinfo_disconnect();
        // Feed one buffered frame at each traffic playout opportunity.
        self.drain_jitter_playout(queue);
        // Expire hanging calls that have exceeded hangtime
        self.expire_hanging_calls(queue);
        // Release inbound calls the core stopped feeding without ever sending GROUP_IDLE
        self.reap_idle_calls(queue);
        self.drop_departed(queue);
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            // UL voice from UMAC — forward to TetraPack if this timeslot is being forwarded
            SapMsgInner::TmdCircuitDataInd(prim) => {
                self.handle_ul_voice(prim.carrier_num, prim.ts, prim.data);
            }
            // Floor-control and call lifecycle notifications from CMCE
            SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                call_id,
                source_issi,
                dest_gssi,
                carrier_num,
                ts,
            }) => {
                self.handle_local_call_start(queue, call_id, source_issi, dest_gssi, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, carrier_num, ts }) => {
                self.handle_local_call_tx_stopped(call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, carrier_num, ts }) => {
                self.handle_local_call_end(call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }) => {
                self.drop_network_call(brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallHold {
                brew_uuid,
                dest_gssi,
            }) => {
                self.hold_inbound_no_listeners(brew_uuid, dest_gssi);
            }
            SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { gssi }) => {
                self.promote_pending_inbound(queue, gssi);
            }
            SapMsgInner::CmceCallControl(CallControl::OngoingGroupCall { .. }) => {
                // LST Dispatch only — real Brew ignores.
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
                usage,
            }) => {
                self.rx_network_call_ready(queue, brew_uuid, call_id, carrier_num, ts, usage);
            }
            // UlInactivityTimeout is UMAC→CMCE only; Brew handles FloorReleased instead
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { .. }) => {}

            // ── Circuit / individual call outbound signals (CMCE → Brew → TetraPack) ──
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => {
                if !self.connected {
                    tracing::debug!("BrewEntity: not connected, dropping NetworkCircuitSetupRequest uuid={}", brew_uuid);
                    return;
                }
                let wire_call = Self::map_network_to_brew_circuit_call(&call);
                let _ = self.command_sender.send(BrewCommand::SendSetupRequest {
                    uuid: brew_uuid,
                    call: wire_call,
                });
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendSetupAccept { uuid: brew_uuid });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendSetupReject { uuid: brew_uuid, cause });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendCallAlert { uuid: brew_uuid });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid, call }) => {
                if !self.connected {
                    tracing::debug!(
                        "BrewEntity: not connected, dropping NetworkCircuitConnectRequest uuid={}",
                        brew_uuid
                    );
                    return;
                }
                let wire_call = Self::map_network_to_brew_circuit_call(&call);
                let _ = self.command_sender.send(BrewCommand::SendConnectRequest {
                    uuid: brew_uuid,
                    call: wire_call,
                });
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                grant,
                permission,
            }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendConnectConfirm {
                        uuid: brew_uuid,
                        grant,
                        permission,
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
                brew_uuid,
                grant,
                permission,
            }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendSimplexGranted {
                        uuid: brew_uuid,
                        grant,
                        permission,
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
                brew_uuid,
                grant,
                permission,
            }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendSimplexIdle {
                        uuid: brew_uuid,
                        grant,
                        permission,
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
            }) => {
                tracing::info!(
                    "BrewEntity: circuit media ready uuid={} call_id={} carrier={} ts={}",
                    brew_uuid,
                    call_id,
                    carrier_num,
                    ts
                );
                // A call restored on another cell of a multi-cell station moves to a new
                // timeslot: its old one no longer carries this call's uplink.
                self.ul_forwarded
                    .retain(|&slot, fwd| slot == (carrier_num, ts) || fwd.uuid != brew_uuid || fwd.kind != UlForwardKind::Circuit);
                // Register UL forwarding: voice on `ts` gets sent to TetraPack.
                self.ul_forwarded.insert(
                    (carrier_num, ts),
                    UlForwardedCall {
                        uuid: brew_uuid,
                        call_id,
                        source_issi: 0,
                        dest_gssi: 0,
                        kind: UlForwardKind::Circuit,
                        carrier_num,
                        frame_count: 0,
                    },
                );
                // Register in active_calls with ts already known so that DL voice frames received
                // from TetraPack (handle_voice_frame + drain_jitter_playout) are delivered to the MS.
                // Without this entry handle_voice_frame silently drops all incoming DL audio because
                // it looks up the uuid in active_calls and finds nothing.
                if let Some(call) = self.active_calls.get_mut(&brew_uuid) {
                    // Downlink voice follows the call to its new timeslot (restored on a sibling cell).
                    call.call_id = Some(call_id);
                    call.carrier_num = Some(carrier_num);
                    call.ts = Some(ts);
                }
                self.active_calls.entry(brew_uuid).or_insert_with(|| ActiveCall {
                    uuid: brew_uuid,
                    call_id: Some(call_id),
                    carrier_num: Some(carrier_num),
                    ts: Some(ts),
                    usage: None,
                    source_issi: 0,
                    dest_gssi: 0,
                    priority: 0,
                    frame_count: 0,
                    last_media_activity_signal: None,
                    last_network_activity: Instant::now(),
                });
                self.dl_jitter
                    .entry(brew_uuid)
                    .or_insert_with(|| VoiceJitterBuffer::with_initial_latency(self.brew_config.jitter_initial_latency_frames as usize));
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf {
                brew_uuid,
                length_bits,
                data,
            }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendDtmf {
                        uuid: brew_uuid,
                        length_bits,
                        data,
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }) => {
                // Always clean up any local media / jitter / UL-forward state for this circuit.
                self.drop_network_circuit(brew_uuid);
                // Forward the release to the Core whenever we are connected — NOT only when
                // the circuit had become "active". A call that was set up but never answered
                // was already announced to the Core via SendSetupRequest, yet it never reached
                // the active state (no connect/media), so `drop_network_circuit` reports
                // was_active = false. Gating the wire release on that meant FlowStation never
                // told the Core to tear the pending call down, leaking a hanging call on the
                // network. Sending unconditionally (like Setup/Reject/Alert do) fixes it; the
                // Core harmlessly ignores a release for a uuid it has already released.
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendCallRelease { uuid: brew_uuid, cause });
                }
            }
            SapMsgInner::MmSubscriberUpdate(update) => {
                self.handle_subscriber_update(update);
            }
            SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistrationResult { issi, sdu }) => {
                self.finish_forward_registration(queue, issi, sdu);
            }
            SapMsgInner::CmceCallControl(CallControl::HandoverGroupCallReady { issi, gssi, call }) => {
                self.finish_handover_channel(issi, gssi, call);
            }
            SapMsgInner::CmceCallControl(CallControl::HandoverRequest {
                issi,
                target_carrier,
                target_carrier_ext,
                target_mcc,
                target_mnc,
                target_location_area,
                sdu,
            }) => {
                let req = BrewHoRequest {
                    transaction_id: 0,
                    issi,
                    target_carrier,
                    target_carrier_ext,
                    target_mcc,
                    target_mnc,
                    target_location_area,
                    sdu: sdu.as_ref().map(BrewHoSdu::from_bitbuffer),
                    call_gssi: None,
                };
                self.send_handover_request(queue, req);
            }
            SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreRequest { issi, peer }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::RequestCallRestore { issi, peer });
                } else {
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Brew,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreAnswer { issi, peer, call: None }),
                    });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::IndividualCallHandoffAnswer { issi, peer, call }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::SendCallHandoff { issi, peer, call });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::RegisteredWithoutCall { issi }) => {
                if self.connected {
                    let _ = self.command_sender.send(BrewCommand::ReportRegisteredWithoutCall { issi });
                }
            }
            SapMsgInner::CmceCallControl(CallControl::HandoverDone { issi }) => {
                self.ho_departing.insert(issi, Instant::now() + HO_DEPARTURE_DELAY);
            }
            SapMsgInner::CmceCallControl(CallControl::ReselectionAnnounced { issi, reselection }) => {
                self.announced_away.retain(|_, at| at.elapsed() < ANNOUNCED_AWAY_LIFETIME);
                self.announced_away.insert(issi, Instant::now());
                self.report_reselection(issi, reselection);
            }
            SapMsgInner::CmceCallControl(CallControl::HandoverFailed { issi }) => {
                // The MS stays here (D-PREPARE-FAIL), but the target may have registered it
                // already: claim it back so calls and SDS keep reaching this station.
                if self.connected && super::is_brew_issi_routable(&self.config, issi) {
                    self.register_with_groups(issi, "reselection refused, re-register");
                }
            }
            SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => {
                self.handle_rssi_update(issi, rssi_dbfs);
            }
            SapMsgInner::CmceSdsData(sds) => {
                self.handle_sds_send(sds);
            }
            _ => {
                tracing::debug!("BrewEntity: unexpected rx_prim from {:?} on {:?}", message.src, message.sap);
            }
        }
    }
}

// ─── Announced reselection from another station (target side) ─────

/// How long MLE/MM may take to answer a forwarded registration (MLE drops it after 3 s too; the
/// server has answered the serving station with "timeout" long before).
const HO_FORWARD_ANSWER_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a request this station sent may wait for its result (MLE answers the MS after 3 s
/// anyway; T370 is 5 s, TS 100 392-2 clause 18.6.2).
const HO_REQUEST_LIFETIME: Duration = Duration::from_secs(5);

/// How long after D-NEW-CELL the radio's registration elsewhere still belongs to that announced
/// reselection (type 3 may scan for a while first). Matches brew-server's report window.
const ANNOUNCED_AWAY_LIFETIME: Duration = Duration::from_secs(10);
/// Time between D-NEW-CELL and dropping the departing MS here, so that nothing CMCE sends when
/// the MS's groups lose their last listener (D-RELEASE) reaches the MS before it has left.
const HO_DEPARTURE_DELAY: Duration = Duration::from_secs(2);

impl BrewEntity {
    /// A serving station relayed an announced cell reselection to this station
    /// (TS 100 392-2 clause 18.3.4.7, U-PREPARE clause 18.4.1.4.6).
    ///
    /// - With a registration (`SERVICE_HO_FORWARD_REG`, the MM PDU from U-PREPARE): hand it to
    ///   MLE as if forwarded by a sibling cell. MM registers the radio here (which moves the ISSI
    ///   to this station on Brew) and its answer goes back for D-NEW-CELL.
    /// - Without (`SERVICE_HO_PREPARE`): nothing to register; answer ok. The radio restores its
    ///   call with U-RESTORE on arrival, as after any reselection.
    fn handle_handover_request(&mut self, queue: &mut MessageQueue, req: BrewHoRequest) {
        let Some(sdu) = req.sdu else {
            tracing::info!(
                "BrewEntity: handover prepare for ISSI {} on carrier {} (transaction {}), nothing to register",
                req.issi,
                req.target_carrier,
                req.transaction_id
            );
            self.send_handover_result(req.transaction_id, req.issi, BREW_HO_STATUS_OK, None, None);
            return;
        };
        tracing::info!(
            "BrewEntity: forwarded registration for ISSI {} on carrier {} (transaction {}{}) → MM",
            req.issi,
            req.target_carrier,
            req.transaction_id,
            req.call_gssi.map(|g| format!(", call to {g}")).unwrap_or_default()
        );
        self.ho_forwards.retain(|_, f| f.since.elapsed() < HO_FORWARD_ANSWER_TIMEOUT);
        self.ho_forwards.insert(
            req.issi,
            HoForward {
                transaction_id: req.transaction_id,
                since: Instant::now(),
                call_gssi: req.call_gssi,
                accept: None,
            },
        );
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistration {
                issi: req.issi,
                target_carrier: req.target_carrier,
                sdu: sdu.to_bitbuffer(),
            }),
        });
    }

    /// MM answered a registration forwarded by another station: return it to the serving station.
    /// A D-LOCATION UPDATE REJECT is reported as rejected, so the serving station sends
    /// D-PREPARE-FAIL with it (clause 18.4.1.4.3). An accept for an MS in a group call the
    /// serving station named waits for CMCE's channel of that call (type 1), so both go back in
    /// one result.
    fn finish_forward_registration(&mut self, queue: &mut MessageQueue, issi: u32, answer: Option<tetra_core::BitBuffer>) {
        let Some(forward) = self.ho_forwards.get_mut(&issi) else {
            tracing::debug!("BrewEntity: forwarded registration answer for ISSI {} without a request", issi);
            return;
        };
        if forward.since.elapsed() >= HO_FORWARD_ANSWER_TIMEOUT || forward.accept.is_some() {
            tracing::info!("BrewEntity: MM answered ISSI {}'s forwarded registration too late, dropped", issi);
            self.ho_forwards.remove(&issi);
            return;
        }
        let rejected = answer
            .as_ref()
            .and_then(|a| a.peek_bits_startoffset(0, 4))
            .is_some_and(|t| t == MmPduTypeDl::DLocationUpdateReject.into_raw());
        let status = if rejected { BREW_HO_STATUS_REJECTED } else { BREW_HO_STATUS_OK };
        let transaction_id = forward.transaction_id;
        tracing::info!(
            "BrewEntity: forwarded registration for ISSI {} {} (transaction {})",
            issi,
            if rejected { "rejected" } else { "accepted" },
            transaction_id
        );
        let sdu = answer.as_ref().map(BrewHoSdu::from_bitbuffer);
        if let (false, Some(gssi), Some(accept)) = (rejected, forward.call_gssi, sdu.clone()) {
            forward.accept = Some(accept);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::HandoverPrepareGroupCall { issi, gssi }),
            });
            return;
        }
        self.ho_forwards.remove(&issi);
        self.send_handover_result(transaction_id, issi, status, sdu, None);
    }

    /// CMCE has the channel of the group call an accepted MS arrives into (or none): send the
    /// accept and the channel to the serving station.
    fn finish_handover_channel(&mut self, issi: u32, gssi: u32, call: Option<(u16, u16, u8, u8)>) {
        let Some(forward) = self.ho_forwards.remove(&issi) else { return };
        let Some(accept) = forward.accept.filter(|_| forward.call_gssi == Some(gssi)) else {
            return;
        };
        if forward.since.elapsed() >= HO_FORWARD_ANSWER_TIMEOUT {
            tracing::info!("BrewEntity: channel for ISSI {}'s reselection came too late, dropped", issi);
            return;
        }
        let channel = call.map(|(call_id, carrier, timeslot, usage)| BrewHoChannel {
            call_id,
            carrier,
            timeslot,
            usage,
        });
        tracing::info!(
            "BrewEntity: ISSI {} accepted{} (transaction {})",
            issi,
            channel
                .map(|c| format!(" with call {} on carrier {} ts {}", c.call_id, c.carrier, c.timeslot))
                .unwrap_or_default(),
            forward.transaction_id
        );
        self.send_handover_result(forward.transaction_id, issi, BREW_HO_STATUS_OK, Some(accept), channel);
    }

    /// Serving side: ask the station serving the neighbour cell to prepare the MS's arrival
    /// (`SERVICE_HO_PREPARE`, or `SERVICE_HO_FORWARD_REG` with its registration). Without a
    /// connection the answer is "not prepared" at once; a server without `handover-v1` gets
    /// nothing (the worker answers for it).
    fn send_handover_request(&mut self, queue: &mut MessageQueue, mut req: BrewHoRequest) {
        if !self.connected {
            Self::push_handover_result(queue, req.issi, HandoverOutcome::NotPrepared, None, None);
            return;
        }
        self.ho_requests.retain(|_, r| r.since.elapsed() < HO_REQUEST_LIFETIME && r.issi != req.issi);
        self.ho_last_transaction = self.ho_last_transaction.wrapping_add(1);
        req.transaction_id = self.ho_last_transaction;
        // Type 1 needs the target's channel in this cell's channel allocation, which names it
        // in this cell's band plan (no extended carrier numbering, clause 21.5.2).
        // A linked multi-cell station runs its group calls per cell (call identifiers and
        // channels belong to the cells, not to this entity), so it offers type 2 only.
        let same_plan = tetra_config::bluestation::carrier_band_plan(&self.config.config().cell) == Some(req.target_carrier_ext);
        let linked = super::is_site_linked(&self.config);
        let call = if req.sdu.is_some() && same_plan && !linked { self.type1_call(req.issi) } else { None };
        req.call_gssi = call.map(|c| c.gssi);
        self.ho_requests.insert(
            req.transaction_id,
            HoRequest {
                issi: req.issi,
                since: Instant::now(),
                call,
            },
        );
        tracing::info!(
            "BrewEntity: asking for carrier {} to prepare ISSI {} (transaction {}{})",
            req.target_carrier,
            req.issi,
            req.transaction_id,
            if req.sdu.is_some() { ", forward registration" } else { "" }
        );
        let _ = self.command_sender.send(BrewCommand::SendHandoverRequest(req));
    }

    /// Serving side: the answer to one of our requests (relayed from the target, or the server's
    /// own "no target" / "timeout"). MLE turns it into D-NEW-CELL or D-PREPARE-FAIL.
    fn handle_handover_result(&mut self, queue: &mut MessageQueue, res: BrewHoResult) {
        let Some(HoRequest { issi, since: at, call }) = self.ho_requests.remove(&res.transaction_id) else {
            tracing::debug!(
                "BrewEntity: handover result for unknown transaction {} (ISSI {}) ignored",
                res.transaction_id,
                res.issi
            );
            return;
        };
        if issi != res.issi || at.elapsed() >= HO_REQUEST_LIFETIME {
            tracing::info!("BrewEntity: stale or mismatched handover result for ISSI {} ignored", res.issi);
            return;
        }
        let outcome = match (res.status, res.reason.as_deref()) {
            (BREW_HO_STATUS_OK, _) => HandoverOutcome::Accepted,
            (BREW_HO_STATUS_REJECTED, _) => HandoverOutcome::Rejected,
            (BREW_HO_STATUS_NO_TARGET, Some("gone")) => HandoverOutcome::TargetGone,
            _ => HandoverOutcome::NotPrepared,
        };
        tracing::info!(
            "BrewEntity: handover result for ISSI {} (transaction {}): status {}{} → {:?}",
            issi,
            res.transaction_id,
            res.status,
            res.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default(),
            outcome
        );
        // A channel is only of use for the call we named, and only while that call still runs here.
        let channel = match (outcome, call, res.channel) {
            (HandoverOutcome::Accepted, Some(named), Some(c)) if self.type1_call(issi) == Some(named) => Some(HandoverChannel {
                call_id: named.call_id,
                new_call_id: c.call_id,
                carrier: c.carrier,
                timeslot: c.timeslot,
                usage: c.usage,
                serving_carrier: named.carrier,
                serving_timeslot: named.ts,
            }),
            _ => None,
        };
        Self::push_handover_result(queue, issi, outcome, res.sdu.as_ref().map(BrewHoSdu::to_bitbuffer), channel);
    }

    fn push_handover_result(
        queue: &mut MessageQueue,
        issi: u32,
        outcome: HandoverOutcome,
        sdu: Option<tetra_core::BitBuffer>,
        channel: Option<HandoverChannel>,
    ) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::CmceCallControl(CallControl::HandoverResult { issi, outcome, sdu, channel }),
        });
    }

    /// The group call `issi` listens to here, as (GSSI, call identifier), if exactly one call to
    /// its groups runs on this cell and `issi` is not its talker. That call can continue on the
    /// target's channel (type 1); a talker or an unclear call gets type 2 and restores it.
    fn type1_call(&self, issi: u32) -> Option<Type1Call> {
        let groups = self.subscriber_groups.get(&issi)?;
        let talking = self.ul_forwarded.values().any(|c| c.source_issi == issi);
        if talking {
            return None;
        }
        let mut calls: Vec<Type1Call> = self
            .active_calls
            .values()
            .filter_map(|c| {
                Some(Type1Call {
                    gssi: c.dest_gssi,
                    call_id: c.call_id?,
                    carrier: c.carrier_num?,
                    ts: c.ts?,
                })
            })
            .chain(self.hanging_calls.values().map(|c| Type1Call {
                gssi: c.dest_gssi,
                call_id: c.call_id,
                carrier: c.carrier_num,
                ts: c.ts,
            }))
            .chain(self.pending_tails.values().map(|t| Type1Call {
                gssi: t.dest_gssi,
                call_id: t.call_id,
                carrier: t.carrier_num,
                ts: t.ts,
            }))
            .chain(
                self.ul_forwarded
                    .iter()
                    .filter(|(_, c)| c.kind == UlForwardKind::Group)
                    .map(|(&(carrier, ts), c)| Type1Call {
                        gssi: c.dest_gssi,
                        call_id: c.call_id,
                        carrier,
                        ts,
                    }),
            )
            .filter(|c| groups.contains(&c.gssi))
            .collect();
        calls.sort_unstable();
        calls.dedup();
        match calls.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    }

    /// The server reports that `issi` registered on another station without a handover prepared
    /// here: the radio left (unannounced reselection). An over it is talking in here ends now, as
    /// when its uplink falls silent, instead of holding the group's floor until the uplink
    /// inactivity timer fires; the SwMI controls who transmits (TS 100 392-2 clause 14.5.2.2.1 a)).
    /// The station then stops serving it, without deregistering it on the server (the other
    /// station owns the ISSI). A radio this station is sending away itself is left to that path.
    fn handle_ms_moved(&mut self, queue: &mut MessageQueue, issi: u32) {
        let sending_away = self.ho_departing.contains_key(&issi) || self.ho_requests.values().any(|r| r.issi == issi);
        if sending_away || self.moved_away.contains_key(&issi) {
            tracing::debug!("BrewEntity: ISSI {} reported on another station, already leaving", issi);
            return;
        }
        let announced = self.announced_away.get(&issi).is_some_and(|at| at.elapsed() < ANNOUNCED_AWAY_LIFETIME);
        if !announced {
            // It left without U-PREPARE (or this station did not answer one). In a circuit mode
            // call that is unannounced reselection, otherwise undeclared (clause 18.3.4.7.0).
            let reselection = if self.in_circuit_call(issi) {
                CellReselection::Unannounced
            } else {
                CellReselection::Undeclared
            };
            self.report_reselection(issi, reselection);
        }
        let talking: Vec<(u16, u8)> = self
            .ul_forwarded
            .iter()
            .filter(|(_, c)| c.kind == UlForwardKind::Group && c.source_issi == issi)
            .map(|(slot, _)| *slot)
            .collect();
        for (carrier_num, ts) in talking {
            tracing::info!(
                "BrewEntity: talker {} left for another station, ending its over on carrier={} ts={}",
                issi,
                carrier_num,
                ts
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { carrier_num, ts }),
            });
        }
        self.ho_departing.insert(issi, Instant::now());
        self.drop_departed(queue);
    }

    /// Whether `issi` takes part in a circuit mode call here: a party of a connected individual
    /// call, the talker of a group call, or affiliated to a group with a call on a traffic
    /// channel here (hang time included), from CMCE's published call table. An individual call
    /// still being set up does not count.
    fn in_circuit_call(&self, issi: u32) -> bool {
        // On a linked multi-cell station the call table above is the primary cell's only; the
        // calls this entity carries for any cell count too (its talkers and individual calls,
        // and network group calls to the radio's groups).
        let groups = self.subscriber_groups.get(&issi);
        if self.ul_forwarded.values().any(|c| c.source_issi == issi || (c.kind != UlForwardKind::Group && c.dest_gssi == issi))
            || self.active_calls.values().any(|c| c.dest_gssi != 0 && groups.is_some_and(|g| g.contains(&c.dest_gssi)))
        {
            return true;
        }
        let state = self.config.state_read();
        state.active_call_ts.contains_key(&issi)
            || state
                .subscribers
                .attached_groups_of(issi)
                .into_iter()
                .any(|gssi| state.active_call_ts.contains_key(&gssi))
    }

    fn report_reselection(&self, issi: u32, reselection: CellReselection) {
        tracing::info!("BrewEntity: {} left by {:?} cell reselection", issi, reselection);
        let _ = self.command_sender.send(BrewCommand::ReportReselection { issi, reselection });
    }

    /// Serving side: radios sent to another station with its registration accept are dropped
    /// from MM here, without deregistering them on the server (the other station owns the ISSI
    /// now). CMCE sees the deregistration too and stops serving their groups if nobody else here
    /// listens.
    fn drop_departed(&mut self, queue: &mut MessageQueue) {
        let now = Instant::now();
        self.moved_away.retain(|_, at| now.duration_since(*at) < HO_REQUEST_LIFETIME);
        let due: Vec<u32> = self.ho_departing.iter().filter(|(_, at)| **at <= now).map(|(i, _)| *i).collect();
        for issi in due {
            self.ho_departing.remove(&issi);
            tracing::info!("BrewEntity: ISSI {} left for another station, dropping its registration here", issi);
            self.moved_away.insert(issi, now);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Brew,
                dest: TetraEntity::Mm,
                msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                    issi,
                    groups: Vec::new(),
                    action: BrewSubscriberAction::Deregister,
                }),
            });
        }
    }

    fn send_handover_result(&self, transaction_id: u32, issi: u32, status: u8, sdu: Option<BrewHoSdu>, channel: Option<BrewHoChannel>) {
        let _ = self.command_sender.send(BrewCommand::SendHandoverResult(BrewHoResult {
            transaction_id,
            issi,
            status,
            reason: None,
            sdu,
            channel,
        }));
    }
}

// ─── UL call forwarding to TetraPack ──────────────────────────────

impl BrewEntity {
    /// Map Brew wire BrewCircularCall to SAP NetworkCircuitCall (CMCE-facing).
    fn map_brew_to_network_circuit_call(call: &super::protocol::BrewCircularCall) -> NetworkCircuitCall {
        NetworkCircuitCall {
            source_issi: call.source,
            destination: call.destination,
            number: call.number.clone(),
            priority: call.priority,
            service: call.service,
            mode: call.mode,
            duplex: call.duplex,
            method: call.method,
            communication: call.communication,
            grant: call.grant,
            permission: call.permission,
            timeout: call.timeout,
            ownership: call.ownership,
            queued: call.queued,
        }
    }

    /// Map SAP NetworkCircuitCall to Brew wire BrewCircularCall (network-facing).
    fn map_network_to_brew_circuit_call(call: &NetworkCircuitCall) -> super::protocol::BrewCircularCall {
        super::protocol::BrewCircularCall {
            source: call.source_issi,
            destination: call.destination,
            number: call.number.clone(),
            priority: call.priority,
            service: call.service,
            mode: call.mode,
            duplex: call.duplex,
            method: call.method,
            communication: call.communication,
            grant: call.grant,
            permission: call.permission,
            timeout: call.timeout,
            ownership: call.ownership,
            queued: call.queued,
            mnemonic: None,
        }
    }

    /// Handle notification that a local UL group call has started.
    /// If the group is subscribed (in config.groups), start forwarding to TetraPack.
    fn handle_local_call_start(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        carrier_num: u16,
        ts: u8,
    ) {
        self.abort_pending_tails_on_slot(queue, carrier_num, ts, "local FloorGranted");
        self.drop_moved_talker_session(carrier_num, ts, source_issi, dest_gssi);

        if !self.connected {
            tracing::trace!("BrewEntity: not connected, ignoring local call start");
            return;
        }
        if !super::is_brew_issi_routable(&self.config, source_issi) {
            tracing::debug!(
                "BrewEntity: suppressing GROUP_TX for source_issi={} (filtered, not sent to Brew)",
                source_issi
            );
            return;
        }

        // If we're already forwarding on this timeslot, treat as a talker change/update
        if let Some(fwd) = self.ul_forwarded.get_mut(&(carrier_num, ts)) {
            if fwd.call_id != call_id || fwd.dest_gssi != dest_gssi {
                tracing::warn!(
                    "BrewEntity: updating forwarded call on carrier={} ts={} (was call_id={} gssi={}) -> (call_id={} gssi={})",
                    carrier_num,
                    ts,
                    fwd.call_id,
                    fwd.dest_gssi,
                    call_id,
                    dest_gssi
                );
            }

            fwd.call_id = call_id;
            fwd.source_issi = source_issi;
            fwd.dest_gssi = dest_gssi;
            fwd.frame_count = 0;

            // Send GROUP_TX update for the new talker
            let _ = self.command_sender.send(BrewCommand::SendGroupTx {
                uuid: fwd.uuid,
                source_issi,
                dest_gssi,
                priority: 0,
                service: 0, // TETRA encoded speech
            });
            return;
        }

        // Generate a UUID for this Brew session
        let uuid = Uuid::new_v4();
        tracing::info!(
            "BrewEntity: forwarding local call to TetraPack: call_id={} src={} gssi={} carrier={} ts={} uuid={}",
            call_id,
            source_issi,
            dest_gssi,
            carrier_num,
            ts,
            uuid
        );

        // Send GROUP_TX to TetraPack
        let _ = self.command_sender.send(BrewCommand::SendGroupTx {
            uuid,
            source_issi,
            dest_gssi,
            priority: 0,
            service: 0, // TETRA encoded speech
        });

        // Track this forwarded call
        self.ul_forwarded.insert(
            (carrier_num, ts),
            UlForwardedCall {
                uuid,
                call_id,
                source_issi,
                dest_gssi,
                frame_count: 0,
                kind: UlForwardKind::Group,
                carrier_num,
            },
        );
    }

    /// Handle notification that a local UL call has ended.
    fn handle_local_call_tx_stopped(&mut self, call_id: u16, carrier_num: u16, ts: u8) {
        if let Some(fwd) = self.ul_forwarded.remove(&(carrier_num, ts)) {
            if fwd.call_id != call_id {
                tracing::warn!(
                    "BrewEntity: call_id mismatch on carrier={} ts={}: expected {} got {}",
                    carrier_num,
                    ts,
                    fwd.call_id,
                    call_id
                );
            }
            match fwd.kind {
                UlForwardKind::Group => {
                    tracing::info!(
                        "BrewEntity: local call transmission stopped, sending GROUP_IDLE to TetraPack: uuid={} frames={}",
                        fwd.uuid,
                        fwd.frame_count
                    );
                    let _ = self.command_sender.send(BrewCommand::SendGroupIdle { uuid: fwd.uuid, cause: 0 });
                }
                UlForwardKind::Circuit => {
                    self.ul_forwarded.insert((carrier_num, ts), fwd);
                }
            }
        }
    }

    fn handle_local_call_end(&mut self, call_id: u16, carrier_num: u16, ts: u8) {
        // Check if ul_forwarded entry still exists (might have been removed by handle_local_call_tx_stopped)
        if let Some(fwd) = self.ul_forwarded.remove(&(carrier_num, ts)) {
            if fwd.call_id != call_id {
                tracing::warn!(
                    "BrewEntity: call_id mismatch on carrier={} ts={}: expected {} got {}",
                    carrier_num,
                    ts,
                    fwd.call_id,
                    call_id
                );
            }
            match fwd.kind {
                UlForwardKind::Group => {
                    tracing::debug!(
                        "BrewEntity: local call ended (already sent GROUP_IDLE during tx_stopped): uuid={} frames={}",
                        fwd.uuid,
                        fwd.frame_count
                    );
                }
                UlForwardKind::Circuit => {
                    self.drop_network_circuit(fwd.uuid);
                }
            }
        } else {
            tracing::debug!(
                "BrewEntity: local call ended on carrier={} ts={} (already cleaned up during tx_stopped)",
                carrier_num,
                ts
            );
        }
    }

    /// Handle UL voice data from UMAC. If the timeslot is being forwarded to TetraPack,
    /// convert to STE format and send.
    fn handle_ul_voice(&mut self, carrier_num: u16, ts: u8, acelp_bits: Vec<u8>) {
        let Some(fwd) = self.ul_forwarded.get_mut(&(carrier_num, ts)) else {
            return; // Not forwarded to TetraPack
        };

        fwd.frame_count += 1;
        // Receive-only calls (ambience listening) carry no downlink network media, so the
        // inbound-idle reaper would tear them down after 30 s. Uplink speech is activity too:
        // keep the call alive while the radio is actually transmitting.
        let fwd_uuid = fwd.uuid;
        if let Some(call) = self.active_calls.get_mut(&fwd_uuid) {
            call.last_network_activity = Instant::now();
        }

        // Convert ACELP bits to STE format.
        // Supported inputs:
        //   - 274 bytes (1-bit-per-byte) → pack to 35 bytes + header
        //   - 35 bytes (already packed) → prepend header
        //   - 36 bytes (already STE with header) → send as-is
        let ste_data = if acelp_bits.len() == 36 {
            acelp_bits
        } else if acelp_bits.len() == 35 {
            let mut ste = Vec::with_capacity(36);
            ste.push(0x00); // STE header byte: normal speech frame
            ste.extend_from_slice(&acelp_bits);
            ste
        } else {
            if acelp_bits.len() < 274 {
                tracing::warn!("BrewEntity: UL voice too short: {} bits", acelp_bits.len());
                return;
            }

            // Pack 274 bits into bytes, MSB first, prepend STE header
            let mut ste = Vec::with_capacity(36);
            ste.push(0x00); // STE header byte: normal speech frame

            // Pack 274 bits (1-per-byte) into 35 bytes (280 bits, last 6 bits padded)
            for chunk_idx in 0..35 {
                let mut byte = 0u8;
                for bit in 0..8 {
                    let bit_idx = chunk_idx * 8 + bit;
                    if bit_idx < 274 {
                        byte |= (acelp_bits[bit_idx] & 1) << (7 - bit);
                    }
                }
                ste.push(byte);
            }
            ste
        };

        let _ = self.command_sender.send(BrewCommand::SendVoiceFrame {
            uuid: fwd.uuid,
            length_bits: (ste_data.len() * 8) as u16,
            data: ste_data,
        });
    }
}

// ─── SDS handling ─────────────────────────────────────────────────

impl BrewEntity {
    /// Handle incoming SDS transfer from Brew (network → local MS)
    fn handle_sds_transfer(
        &mut self,
        queue: &mut MessageQueue,
        uuid: Uuid,
        source: u32,
        destination: u32,
        data: Vec<u8>,
        length_bits: u16,
    ) {
        tracing::info!(
            "BrewEntity: SDS transfer uuid={} src={} dst={} {} bytes",
            uuid,
            source,
            destination,
            data.len()
        );

        // Only forward and acknowledge if destination ISSI is locally registered
        if !self.is_registered_on_site(destination) {
            tracing::warn!(
                "BrewEntity: SDS dest ISSI {} not registered, dropping (no report sent) uuid={}",
                destination,
                uuid
            );
            return;
        }

        // Brew protocol always delivers SDS as variable-length (Type 4). This means the
        // downlink D-SDS-DATA will use SDTI=3, even if the original uplink was a 16-bit
        // pre-coded status (SDTI=0 / Type 1). This is a Brew protocol constraint.
        let user_defined_data = SdsUserData::Type4(length_bits, data);

        // Forward to CMCE SDS subentity for downlink delivery
        // Set dltime to next ts1 to ensure it gets sent on MCCH
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceSdsData(CmceSdsData {
                source_issi: source,
                dest_issi: destination,
                user_defined_data,
            }),
        });

        // Send SDS_REPORT (status=0) back to Brew to release session resources.
        // Without this, sessions are killed by timeout instead of being released cleanly.
        // TODO: should be sent after the radio ACKs on the air interface (LLC BL-ACK),
        // currently sent immediately after queuing for delivery.
        let _ = self.command_sender.send(BrewCommand::SendSdsReport { uuid, status: 0 });
        tracing::info!("BrewEntity: SDS_REPORT uuid={} status=0 -> Brew", uuid);
    }

    /// Handle outgoing SDS from CMCE → Brew (local MS → network)
    fn handle_sds_send(&self, sds: CmceSdsData) {
        if !self.connected {
            tracing::warn!(
                "BrewEntity: not connected, dropping outgoing SDS {} -> {}",
                sds.source_issi,
                sds.dest_issi
            );
            return;
        }

        let uuid = Uuid::new_v4();
        tracing::info!(
            "BrewEntity: sending SDS uuid={} src={} dst={} type={} {} bits",
            uuid,
            sds.source_issi,
            sds.dest_issi,
            sds.user_defined_data.type_identifier(),
            sds.user_defined_data.length_bits()
        );

        let _ = self.command_sender.send(BrewCommand::SendSds {
            uuid,
            source: sds.source_issi,
            destination: sds.dest_issi,
            data: sds.user_defined_data.to_arr(),
            length_bits: sds.user_defined_data.length_bits(),
        });
    }
}

impl Drop for BrewEntity {
    fn drop(&mut self) {
        tracing::debug!("BrewEntity: shutting down, sending graceful disconnect");
        let _ = self.command_sender.send(BrewCommand::Disconnect);

        // Give the worker thread time to send DEAFFILIATE + DEREGISTER and close
        if let Some(handle) = self.worker_handle.take() {
            let timeout = std::time::Duration::from_secs(3);
            let start = std::time::Instant::now();
            loop {
                if handle.is_finished() {
                    let _ = handle.join();
                    tracing::debug!("BrewEntity: worker thread joined cleanly");
                    break;
                }
                if start.elapsed() >= timeout {
                    tracing::warn!("BrewEntity: worker thread did not finish in time, abandoning");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// The cell of this station `issi` is on: where the site directory has it in a multi-cell
/// station, otherwise (or not known there) the primary.
fn ms_cell(directory: Option<&SharedDirectory>, issi: u32) -> CellId {
    directory
        .and_then(|d| d.read().expect("site directory").location(issi))
        .unwrap_or(CellId::PRIMARY)
}

#[cfg(test)]
mod ms_cell_tests {
    use super::*;
    use crate::net_site::SiteDirectory;
    use std::sync::{Arc, RwLock};
    use tetra_saps::control::brew::MmSubscriberUpdate;

    #[test]
    fn single_cell_stations_report_the_primary() {
        assert_eq!(ms_cell(None, 2320001), CellId::PRIMARY);
    }

    #[test]
    fn multi_cell_stations_report_where_the_directory_has_the_radio() {
        let directory: SharedDirectory = Arc::new(RwLock::new(SiteDirectory::default()));
        let register = MmSubscriberUpdate {
            issi: 2320001,
            groups: Vec::new(),
            action: BrewSubscriberAction::Register,
        };
        directory.write().unwrap().apply(CellId(3), &register);
        assert_eq!(ms_cell(Some(&directory), 2320001), CellId(3));
        assert_eq!(ms_cell(Some(&directory), 2320002), CellId::PRIMARY, "unknown radio");
        directory.write().unwrap().apply(CellId(1), &register);
        assert_eq!(ms_cell(Some(&directory), 2320001), CellId(1), "moved within the station");
    }
}
