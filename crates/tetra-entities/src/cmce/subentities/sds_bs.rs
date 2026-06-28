use tetra_config::bluestation::SharedConfig;
use tetra_core::Layer2Service;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, tetra_entities::TetraEntity, unimplemented_log};
use tetra_pdus::cmce::enums::pre_coded_status::PreCodedStatus;
use tetra_pdus::cmce::enums::short_report_type::ShortReportType;
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::CmceSdsData;
use tetra_saps::lcmc::LcmcMleUnitdataReq;
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::pdus::d_sds_data::DSdsData;
use tetra_pdus::cmce::pdus::d_status::DStatus;
use tetra_pdus::cmce::pdus::u_sds_data::USdsData;
use tetra_pdus::cmce::pdus::u_status::UStatus;

use super::home_mode_display::HomeModeDisplaySender;
use crate::MessageQueue;
use crate::net_brew;
use crate::net_control::ControlCommand;
use crate::net_telemetry::{TelemetryEvent, TelemetrySink};

/// Clause 13 Short Data Service CMCE sub-entity
/// Actions that sds_bs cannot execute itself (need access to CcBsSubentity or system),
/// queued during U-STATUS processing and drained by CmceBs::tick_start.
#[derive(Debug, Clone)]
pub enum SdsPendingAction {
    KickAll,
}

/// An individual D-SDS-DATA whose delivery is deferred because the destination MS is currently
/// engaged in a call (camped on a traffic timeslot, not the MCCH). It is delivered on the MCCH —
/// the normal, reliable idle-MS path — as soon as the destination leaves the call.
///
/// We do NOT attempt in-band delivery on the traffic channel. That was tried exhaustively against
/// the field radios (FACCH stealing with MAC fragmentation across half-slots, single-block STCH,
/// and a full-slot SCH/F in the hangtime gap). The BS transmits all of them per ETSI, but the
/// field terminals never received any of them — they only accept an SDS on the MCCH. So the SDS is
/// held until the call releases and then delivered on the MCCH, which is acknowledged end-to-end
/// (verified on-air). (FH-BUG-034.)
#[derive(Debug, Clone)]
pub struct PendingSds {
    pub source_issi: u32,
    pub dest_ssi: u32,
    pub user_defined_data: SdsUserData,
    pub queued_at: std::time::Instant,
}

/// Single bounded deadline an SDS may sit deferred — destination in a call, or an EE MS asleep
/// outside its monitoring window — before we GIVE UP and report failure to the sender instead of
/// delivering it. Kept deliberately short, below the field terminals' own SDS delivery-report
/// timeout, so the outcome is never "failed then delivered" minutes later (FH-BUG-036): within the
/// deadline we deliver as soon as the destination is reachable; past it we fail cleanly. A normal
/// short call or EE window resolves well within this; a long (back-to-back) call makes the SDS fail
/// rather than arrive long after the sender's radio already declared it undelivered.
const SDS_DEFER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// SDS-TL delivery-report "delivery status" octet signalling a negative outcome (could not be
/// delivered), sent to the originator when we give up on a deferred SDS. NOTE: confirm on-air that
/// the field terminals (Motorola MXP600/MTP6750) render this as "not delivered" — it is
/// codeplug-dependent. If a radio ignores it, it still falls back to its own delivery-report
/// timeout (also "failed"), and we never deliver the message late, so the two cannot contradict.
const SDS_TL_STATUS_UNDELIVERABLE: u8 = 0x02;

/// One active status-based emergency session (keyed by source ISSI in `emergency_sessions`).
/// The radio re-sends the emergency status periodically while active and goes silent on exit, so
/// `last_seen` drives the clear-timeout sweep. Call-raised emergencies are NOT tracked here — the
/// dashboard derives those from the call lifecycle.
struct EmergencySession {
    dest_ssi: u32,
    last_seen: std::time::Instant,
}

pub struct SdsBsSubentity {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    home_mode_display_sender: HomeModeDisplaySender,
    sds_broadcast_sender: HomeModeDisplaySender,
    live_sds_sender: HomeModeDisplaySender,
    pub pending_actions: Vec<SdsPendingAction>,
    /// Individual SDS deferred until their destination is reachable (out of a call AND awake on its
    /// energy-economy monitoring window). See PendingSds / flush_pending_sds.
    pending_sds: Vec<PendingSds>,
    /// Most recent downlink TdmaTime, set each tick. Used to evaluate the EE monitoring-window gate.
    last_dltime: TdmaTime,
    /// Control-command sender used to re-inject WX/METAR replies into the stack from the
    /// background fetch thread. Cloned from the CMCE command dispatcher at startup. When
    /// None (no control links), the WX responder still works for nothing — replies need
    /// this channel — so it is wired in main.rs alongside the dashboard sender.
    wx_cmd_tx: Option<crossbeam_channel::Sender<ControlCommand>>,
    /// Monotonic timestamp of the last periodic WX auto-send, to rate-limit the broadcast.
    last_periodic_wx: Option<std::time::Instant>,
    /// Active status-based emergency sessions, keyed by source ISSI. Populated when a radio sends
    /// an emergency status (U-STATUS pre-coded status Emergency); refreshed on re-sends; removed on
    /// a non-Emergency status, clear-timeout (tick), or operator clear. Non-empty means at least one
    /// radio is in emergency. See [`EmergencySession`].
    emergency_sessions: std::collections::HashMap<u32, EmergencySession>,
}

impl SdsBsSubentity {
    pub fn new(config: SharedConfig) -> Self {
        SdsBsSubentity {
            config,
            telemetry: None,
            home_mode_display_sender: HomeModeDisplaySender::new(),
            sds_broadcast_sender: HomeModeDisplaySender::new(),
            live_sds_sender: HomeModeDisplaySender::new(),
            pending_actions: Vec::new(),
            pending_sds: Vec::new(),
            last_dltime: TdmaTime::default(),
            wx_cmd_tx: None,
            last_periodic_wx: None,
            emergency_sessions: std::collections::HashMap::new(),
        }
    }

    pub fn set_telemetry(&mut self, sink: TelemetrySink) {
        self.telemetry = Some(sink);
    }

    /// Provide the control-command sender used to deliver WX/METAR replies.
    pub fn set_wx_cmd_sender(&mut self, tx: crossbeam_channel::Sender<ControlCommand>) {
        self.wx_cmd_tx = Some(tx);
    }

    pub fn shared_config(&self) -> &SharedConfig {
        &self.config
    }

    fn emit(&self, event: TelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.send(event);
        }
    }

    // ── Emergency state (status-based) ──────────────────────────────────────────
    //
    // A radio signals emergency with a U-STATUS (pre-coded status Emergency, status 0), re-sent
    // periodically while active; it sends nothing on exit. We raise a session on the first
    // Emergency status from an ISSI and clear it when: (a) the ISSI sends a non-Emergency status,
    // (b) no Emergency status arrives for `clear_timeout_secs` (the radio went silent — swept in
    // tick), or (c) an operator clears it from the dashboard. Telemetry (EmergencyAlarm /
    // EmergencyCancel) fires only on the enter/clear transitions, never on re-sends.

    /// Raise (or refresh) a status-based emergency for `source_issi`. Emits `EmergencyAlarm` only
    /// on the idle→emergency transition so periodic re-sends don't re-fire the alarm / Telegram.
    fn emergency_enter(&mut self, source_issi: u32, dest_ssi: u32) {
        let now = std::time::Instant::now();
        match self.emergency_sessions.get_mut(&source_issi) {
            Some(s) => {
                // REFRESH — radio is still in emergency; keep the session alive.
                s.last_seen = now;
                s.dest_ssi = dest_ssi;
            }
            None => {
                self.emergency_sessions
                    .insert(source_issi, EmergencySession { dest_ssi, last_seen: now });
                tracing::warn!("EMERGENCY: ISSI {} entered emergency (status to ISSI {})", source_issi, dest_ssi);
                self.emit(TelemetryEvent::EmergencyAlarm { source_issi, dest_ssi });
            }
        }
    }

    /// Clear a status-based emergency for `source_issi` if present, emitting `EmergencyCancel`.
    fn emergency_clear(&mut self, source_issi: u32, reason: &str) {
        if self.emergency_sessions.remove(&source_issi).is_some() {
            tracing::info!("EMERGENCY: ISSI {} cleared ({})", source_issi, reason);
            self.emit(TelemetryEvent::EmergencyCancel { source_issi });
        }
    }

    /// Operator/manual clear dispatched from the dashboard (`issi == 0` clears every session).
    pub fn clear_emergency_command(&mut self, issi: u32) {
        if issi == 0 {
            let all: Vec<u32> = self.emergency_sessions.keys().copied().collect();
            for i in all {
                self.emergency_clear(i, "operator clear (all)");
            }
        } else {
            self.emergency_clear(issi, "operator clear");
        }
    }

    /// Sweep emergency sessions whose last emergency status is older than `clear_timeout_secs`.
    /// The radio sends nothing on exit, so silence past the timeout is treated as "cleared".
    fn expire_emergency_sessions(&mut self) {
        if self.emergency_sessions.is_empty() {
            return;
        }
        let timeout = std::time::Duration::from_secs(self.config.config().emergency.clear_timeout_secs);
        let expired: Vec<u32> = self
            .emergency_sessions
            .iter()
            .filter(|(_, s)| s.last_seen.elapsed() > timeout)
            .map(|(issi, _)| *issi)
            .collect();
        for issi in expired {
            self.emergency_clear(issi, "timeout");
        }
    }

    /// Record one SDS in the dashboard's SDS Log (best-effort, fire-and-forget). `direction`
    /// is "rx" (uplink from a local MS), "net" (from the network for local delivery), or "tx"
    /// (injected by the dashboard operator). The body is decoded best-effort; non-text
    /// payloads (status/reports/binary) log with empty text and the raw protocol-id byte.
    fn log_sds(&self, direction: &str, source_issi: u32, dest_issi: u32, is_group: bool, data: &SdsUserData) {
        let protocol_id = data.to_arr().first().copied().unwrap_or(0);
        self.emit(TelemetryEvent::SdsLog {
            direction: direction.to_string(),
            source_issi,
            dest_issi,
            is_group,
            protocol_id,
            text: Self::extract_sds_text(data),
        });
    }

    /// True if `dest_ssi` (an individual ISSI) is currently on one of our traffic timeslots —
    /// either directly (active talker / individual-call party) or as an affiliated member of an
    /// active group call. Such an MS follows the FACCH on its traffic slot, not the MCCH.
    fn issi_on_local_traffic(&self, dest_ssi: u32) -> bool {
        let state = self.config.state_read();
        state.active_call_ts.contains_key(&dest_ssi)
            || state
                .subscribers
                .attached_groups_of(dest_ssi)
                .into_iter()
                .any(|gssi| state.active_call_ts.contains_key(&gssi))
    }

    /// True if `dest_ssi` is an energy-economy MS that is NOT currently awake on its downlink
    /// monitoring window (so an unsolicited SDS sent now would be missed — defer it to the window).
    /// Returns false for StayAlive / unknown MSs (absent from the published map) and whenever the
    /// window is open, i.e. those are delivered immediately. (ETSI EN 300 392-2 §16.7.)
    fn ee_window_blocks(&self, dest_ssi: u32) -> bool {
        let state = self.config.state_read();
        match state.ee_monitoring_windows.get(&dest_ssi) {
            Some(&(frame, mframe, cycle_len)) => !self.last_dltime.in_ee_monitoring_window(frame, mframe, cycle_len),
            None => false, // not in energy economy — always reachable
        }
    }

    /// Deliver deferred SDS whose destination is now reachable, or fail them. An SDS is deferred
    /// while its destination is in a call (delivered on the MCCH once it returns) OR is an
    /// energy-economy MS asleep outside its monitoring window (delivered when the window opens).
    /// Called every tick. A single short deadline (`SDS_DEFER_DEADLINE`) keeps the outcome
    /// consistent with what the sending radio sees: within the deadline we deliver as soon as the
    /// destination is reachable; past it we GIVE UP and report failure to the originator rather than
    /// delivering minutes late — which would surface as "failed then delivered" once the sender's
    /// own delivery-report timer had already expired (FH-BUG-036).
    fn flush_pending_sds(&mut self, queue: &mut MessageQueue) {
        if self.pending_sds.is_empty() {
            return;
        }
        for p in std::mem::take(&mut self.pending_sds) {
            let reachable = !self.issi_on_local_traffic(p.dest_ssi) && !self.ee_window_blocks(p.dest_ssi);
            if reachable {
                // Out of any call and awake on its window (if in EE) — deliver on the MCCH.
                tracing::info!("SDS: destination {} reachable — delivering deferred SDS on the MCCH", p.dest_ssi);
                self.deliver_d_sds_data_now(queue, p.source_issi, p.dest_ssi, SsiType::Issi, p.user_defined_data, false);
            } else if p.queued_at.elapsed() > SDS_DEFER_DEADLINE {
                // Could not reach the destination within the deadline — fail cleanly and tell the
                // sender, instead of delivering late after its radio has already given up.
                tracing::warn!(
                    "SDS: {} -> {} undeliverable within {}s (destination stayed in a call / asleep) — failing",
                    p.source_issi,
                    p.dest_ssi,
                    SDS_DEFER_DEADLINE.as_secs()
                );
                self.report_sds_failure(queue, &p);
            } else {
                self.pending_sds.push(p); // still unreachable — keep waiting until the deadline
            }
        }
    }

    /// Send an SDS-TL delivery report with a failure status back to the originator of a deferred SDS
    /// we are giving up on, so its terminal shows "not delivered" promptly and definitively — and is
    /// never contradicted by a late delivery, since the message is dropped here. Only emitted when
    /// the original was an SDS-TL message carrying a message reference (status-only / non-TL SDS have
    /// nothing to report against, and an SDS-TL report itself has no reference, so this never loops).
    fn report_sds_failure(&mut self, queue: &mut MessageQueue, p: &PendingSds) {
        let Some(mr) = Self::sds_tl_message_reference(&p.user_defined_data) else {
            return;
        };
        // SDS-TL SHORT REPORT: [PID 0x82, type 0x10 (report), delivery status, message reference],
        // addressed FROM the unreachable destination TO the original sender. Sent immediately on the
        // MCCH (not deferred) — if the sender is itself busy it falls back to its own timeout.
        let report = SdsUserData::Type4(32, vec![0x82, 0x10, SDS_TL_STATUS_UNDELIVERABLE, mr]);
        tracing::info!(
            "SDS: reporting delivery failure to {} (MR={}) for undeliverable SDS to {}",
            p.source_issi,
            mr,
            p.dest_ssi
        );
        self.deliver_d_sds_data_now(queue, p.dest_ssi, p.source_issi, SsiType::Issi, report, false);
    }

    /// Called every tick from CmceBs::tick_start. Fires Home Mode Display broadcast when due.
    pub fn tick_start(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        self.last_dltime = dltime; // record current time for the EE monitoring-window gate
        // Auto-clear emergency sessions whose radio stopped re-sending the emergency status
        // (the radio is silent on exit, so silence past clear_timeout_secs == cleared).
        self.expire_emergency_sessions();
        // Flush SDS that were deferred while their destination was in a call or asleep (EE).
        self.flush_pending_sds(queue);
        // Feed the health monitor's Congestion domain: undelivered/deferred SDS backlog.
        crate::health::registry().set_sds_queue_depth(self.pending_sds.len());
        if let Some(hmd_tx) = self.home_mode_display_sender.tick_start(&self.config, dltime) {
            self.send_d_sds_data(queue, hmd_tx.source_issi, hmd_tx.dest_gssi, SsiType::Gssi, hmd_tx.payload);
        }
        if let Some(tx) = self.sds_broadcast_sender.tick_start_broadcast(&self.config, dltime) {
            self.send_d_sds_data(queue, tx.source_issi, tx.dest_gssi, SsiType::Gssi, tx.payload);
        }
        if let Some(tx) = self.live_sds_sender.tick_live_sds(&self.config, dltime) {
            self.send_d_sds_data(queue, tx.source_issi, tx.dest_gssi, SsiType::Gssi, tx.payload);
        }
    }

    /// Handle incoming U-SDS-DATA from a local MS (via RF uplink)
    pub fn route_rf_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("SDS route_rf_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let calling_party = prim.received_tetra_address;

        let pdu = match USdsData::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-SDS-DATA: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        if !Self::feature_check_u_sds_data(&pdu) {
            tracing::warn!("Unsupported features in U-SDS-DATA, dropping");
            return;
        }

        // Extract destination SSI (guaranteed present after feature check)
        let Some(dest_ssi_raw) = pdu.called_party_ssi else {
            tracing::warn!("SDS: U-SDS-DATA missing called_party_ssi after feature check, dropping");
            return;
        };
        let dest_ssi = dest_ssi_raw as u32;
        let source_ssi = calling_party.ssi;

        tracing::info!(
            "SDS: U-SDS-DATA from ISSI {} to ISSI {}, type={}",
            source_ssi,
            dest_ssi,
            pdu.user_defined_data.type_identifier()
        );

        // Record every inbound SDS-DATA in the dashboard SDS Log, regardless of how it is
        // routed afterwards (local ISSI, local group, Brew forward, WX request). is_group is
        // the BS's view of the destination; the read borrow is O(1) and dropped immediately.
        let rx_is_group = self.config.state_read().subscribers.has_group_members(dest_ssi);
        self.log_sds("rx", source_ssi, dest_ssi, rx_is_group, &pdu.user_defined_data);

        // Built-in WX/METAR service: if this SDS is addressed to the configured service
        // ISSI and the responder is enabled, treat the text as a weather command, fetch
        // asynchronously, and reply to the sender. Consumed locally (not routed onward).
        let wx = self.config.effective_wx_service();
        if wx.enabled && dest_ssi == wx.service_issi {
            // An SDS-TL SHORT REPORT / STATUS (PID 0x82/0x89, message-type byte 0x10) is a
            // delivery confirmation for a reply we already sent — never a fresh request.
            // Feeding it back into the responder produced an infinite SDS storm: each reply
            // requests a delivery report, the terminal returns one, and its message-reference
            // byte decoded as a single-character "command" that triggered yet another reply.
            // tetraflow-sds-bot guards against this in handle_downlink_sds / parse_text_payload
            // by rejecting data[1] == 0x10; mirror that here and absorb the report.
            if Self::is_sds_tl_report(&pdu.user_defined_data) {
                tracing::debug!("SDS: absorbing SDS-TL delivery report to WX service from ISSI {}", source_ssi);
                return;
            }
            // Delivery confirmation, identical to tetraflow-sds-bot's queue_u_status: before
            // answering, send an SDS-TL SHORT REPORT back to the requester so the terminal
            // marks its outgoing message as delivered. The report echoes the request's
            // message-reference byte and carries [0x82, 0x10, 0x00, MR], from the service
            // ISSI to the requester.
            if let Some(mr) = Self::sds_tl_message_reference(&pdu.user_defined_data) {
                let report = SdsUserData::Type4(32, vec![0x82u8, 0x10u8, 0x00u8, mr]);
                self.send_d_sds_data(queue, wx.service_issi, source_ssi, SsiType::Issi, report);
            }
            self.handle_wx_request(source_ssi, &pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
            return;
        }

        // ACKs/replies addressed to the dashboard ISSI (9999) are consumed locally.
        if dest_ssi == 9999 {
            tracing::debug!("SDS: absorbing message to dashboard ISSI 9999 from {}", source_ssi);
            return;
        }

        // Route: local delivery (ISSI or GSSI), Brew forward, or drop
        let is_local_issi = self.config.state_read().subscribers.is_registered(dest_ssi);
        let is_local_group = !is_local_issi && self.config.state_read().subscribers.has_group_members(dest_ssi);

        if is_local_issi {
            tracing::info!("SDS: local delivery: {} -> {}", source_ssi, dest_ssi);
            self.send_d_sds_data(queue, source_ssi, dest_ssi, SsiType::Issi, pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
        } else if is_local_group {
            tracing::info!("SDS: group delivery: {} -> GSSI {}", source_ssi, dest_ssi);
            self.send_d_sds_data(queue, source_ssi, dest_ssi, SsiType::Gssi, pdu.user_defined_data);
            self.emit(TelemetryEvent::SdsActivity {
                source_issi: source_ssi,
                dest_issi: dest_ssi,
            });
        } else if let Some(brew_entity) = net_brew::route_entity_for_local_issi(&self.config, source_ssi)
            && net_brew::feature_sds_enabled_for_entity(&self.config, brew_entity)
        {
            tracing::info!("SDS: forwarding to {:?}: {} -> {}", brew_entity, source_ssi, dest_ssi);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: brew_entity,
                msg: SapMsgInner::CmceSdsData(CmceSdsData {
                    source_issi: source_ssi,
                    dest_issi: dest_ssi,
                    user_defined_data: pdu.user_defined_data,
                }),
            });
        } else {
            tracing::warn!("SDS: dest SSI {} not local and not Brew-routable, dropping", dest_ssi);
        }
    }

    /// Handle incoming SDS data from Brew entity (network-originated SDS)
    pub fn rx_sds_from_brew(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        let SapMsgInner::CmceSdsData(sds) = message.msg else {
            tracing::error!("SDS: rx_sds_from_brew expected CmceSdsData, got unexpected message type");
            return;
        };

        tracing::info!(
            "SDS: received from Brew: {} -> {}, type={}, {} bits",
            sds.source_issi,
            sds.dest_issi,
            sds.user_defined_data.type_identifier(),
            sds.user_defined_data.length_bits()
        );

        // Mirror the RF ingress routing (route above): try individual delivery FIRST so an ISSI
        // that numerically collides with a GSSI still delivers individually, then fall back to
        // group delivery when the dest is a GSSI with locally-affiliated members (FH-FEAT-032 R2 —
        // previously a group-addressed Brew SDS was dropped because is_registered() only matches
        // individual ISSIs). Two short read borrows, each O(1), identical to sds_bs.rs:302-303.
        let is_local_issi = self.config.state_read().subscribers.is_registered(sds.dest_issi);
        let is_local_group = !is_local_issi && self.config.state_read().subscribers.has_group_members(sds.dest_issi);

        // Log the network-originated SDS in the dashboard SDS Log before it is delivered.
        self.log_sds("net", sds.source_issi, sds.dest_issi, is_local_group, &sds.user_defined_data);

        if is_local_issi {
            // Send D-SDS-DATA downlink to the local MS on the MCCH.
            tracing::info!("SDS: local delivery from Brew: {} -> {}", sds.source_issi, sds.dest_issi);
            self.send_d_sds_data(queue, sds.source_issi, sds.dest_issi, SsiType::Issi, sds.user_defined_data);
        } else if is_local_group {
            tracing::info!("SDS: group delivery from Brew: {} -> GSSI {}", sds.source_issi, sds.dest_issi);
            self.send_d_sds_data(queue, sds.source_issi, sds.dest_issi, SsiType::Gssi, sds.user_defined_data);
        } else {
            tracing::warn!(
                "SDS: dest SSI {} from Brew is neither a local ISSI nor a group with members, dropping",
                sds.dest_issi
            );
        }
    }

    /// Handle incoming SDS data from Control entity (network-originated SDS)
    pub fn rx_sds_from_control(&mut self, queue: &mut MessageQueue, message: ControlCommand) -> bool {
        let ControlCommand::SendSds {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            len_bits,
            payload,
        } = message
        else {
            tracing::error!("SDS: rx_sds_from_control expected SendSds command, got unexpected command type");
            return false;
        };

        tracing::info!(
            "SDS: received from Control {}: {} -> {}, type={}, {} bits",
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group.then(|| "GSSI").unwrap_or("ISSI"),
            len_bits
        );

        // SDS-TL Simple Text Message — format verificat din tetraflow-sds-bot:
        //   Byte 0: 0x82  — Protocol Identifier (SDS-TL text messaging)
        //   Byte 1: 0x04  — Message Type (Simple Text, cu TL-ACK request)
        //   Byte 2: MR    — Message Reference (1..255, incrementat)
        //   Byte 3: 0x01  — Encoding (ISO-8859-1 / ASCII)
        //   Bytes 4+: text payload
        static SDS_MR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
        let mr = {
            let v = SDS_MR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                SDS_MR.store(1, std::sync::atomic::Ordering::Relaxed);
                1
            } else {
                v
            }
        };
        let wrapped_payload: Vec<u8> = {
            let mut v = vec![0x82u8, 0x04u8, mr, 0x01u8];
            v.extend_from_slice(&payload);
            v
        };
        let wrapped_len_bits = (wrapped_payload.len() * 8) as u16;
        let sds_data = SdsUserData::Type4(wrapped_len_bits, wrapped_payload);

        // Log the dashboard-originated SDS before sending it.
        self.log_sds("tx", source_ssi, dest_ssi, dest_is_group, &sds_data);

        // Route the dashboard-composed SDS the same way route_rf_deliver routes a radio-originated
        // one: local subscribers/groups are served over our own RF, anything else goes up the Brew
        // link when the SDS feature is enabled. Without this, a dashboard SDS addressed to an ISSI
        // that lives behind Brew (e.g. on a bridged network) was delivered over RF "anyway", never
        // acknowledged, and lost once the LLC retransmissions exhausted.
        //
        // The diversion is scoped to the dashboard operator ISSI (9999) on purpose: the WX/METAR
        // responder re-injects its replies through this very path (see queue_wx_reply) addressed to
        // an on-air requester that may legitimately be absent from the static registry, and those
        // must keep going out over RF — so they are intentionally excluded here.
        const DASHBOARD_ISSI: u32 = 9999;
        let is_local_issi = !dest_is_group && self.config.state_read().subscribers.is_registered(dest_ssi);
        let is_local_group = dest_is_group && self.config.state_read().subscribers.has_group_members(dest_ssi);

        if source_ssi == DASHBOARD_ISSI && !is_local_issi && !is_local_group {
            if let Some(brew_entity) = net_brew::route_entity_for_local_issi(&self.config, source_ssi)
                .filter(|entity| net_brew::feature_sds_enabled_for_entity(&self.config, *entity))
            {
                tracing::info!("SDS: forwarding dashboard SDS to {:?}: {} -> {}", brew_entity, source_ssi, dest_ssi);
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: brew_entity,
                    msg: SapMsgInner::CmceSdsData(CmceSdsData {
                        source_issi: source_ssi,
                        dest_issi: dest_ssi,
                        user_defined_data: sds_data,
                    }),
                });
                return true;
            }
        }

        // Deliver over RF. As before, we do NOT gate on the SDS subscriber registry: a terminal
        // that just sent us an uplink request (e.g. the WX/METAR requester) is reachable on our
        // air interface even when it is not in the static local-subscriber table.
        if !dest_is_group && !is_local_issi {
            tracing::debug!(
                "SDS: dest ISSI {} from Control not in local registry; delivering over RF anyway",
                dest_ssi
            );
        }

        self.send_d_sds_data(
            queue,
            source_ssi,
            dest_ssi,
            if dest_is_group { SsiType::Gssi } else { SsiType::Issi },
            sds_data,
        );

        true
    }

    /// Handle an already-built SDS Type-4 SDU from the Control entity (FH-BUG-052).
    ///
    /// Unlike [`Self::rx_sds_from_control`], `payload` here is a COMPLETE Type-4 SDU whose first byte
    /// is its own protocol identifier (e.g. 0xC3 for a TPG2200 Call-Out). It MUST be delivered
    /// verbatim — we do NOT prepend the `[0x82, 0x04, mr, 0x01]` SDS-TL simple-text header that the
    /// SendSds path adds, because that would corrupt the SDU. Routing is otherwise identical.
    pub fn rx_raw_sds_type4_from_control(&mut self, queue: &mut MessageQueue, message: ControlCommand) -> bool {
        let ControlCommand::SendRawSdsType4 {
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group,
            len_bits,
            payload,
        } = message
        else {
            tracing::error!("SDS: rx_raw_sds_type4_from_control expected SendRawSdsType4 command, got unexpected command type");
            return false;
        };

        tracing::info!(
            "SDS: received RAW Type-4 from Control {}: {} -> {}, type={}, {} bits",
            handle,
            source_ssi,
            dest_ssi,
            dest_is_group.then(|| "GSSI").unwrap_or("ISSI"),
            len_bits
        );

        // Deliver the payload exactly as supplied — it is already a full Type-4 SDU. No SDS-TL wrap.
        let sds_data = SdsUserData::Type4(len_bits, payload);

        self.log_sds("tx", source_ssi, dest_ssi, dest_is_group, &sds_data);

        // Same routing as rx_sds_from_control: dashboard-originated remote SSIs go over Brew; local
        // subscribers/groups and on-air requesters are served over our own RF.
        const DASHBOARD_ISSI: u32 = 9999;
        let is_local_issi = !dest_is_group && self.config.state_read().subscribers.is_registered(dest_ssi);
        let is_local_group = dest_is_group && self.config.state_read().subscribers.has_group_members(dest_ssi);

        if source_ssi == DASHBOARD_ISSI && !is_local_issi && !is_local_group {
            if let Some(brew_entity) = net_brew::route_entity_for_local_issi(&self.config, source_ssi)
                .filter(|entity| net_brew::feature_sds_enabled_for_entity(&self.config, *entity))
            {
                tracing::info!(
                    "SDS: forwarding raw Type-4 dashboard SDS to {:?}: {} -> {}",
                    brew_entity,
                    source_ssi,
                    dest_ssi
                );
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: brew_entity,
                    msg: SapMsgInner::CmceSdsData(CmceSdsData {
                        source_issi: source_ssi,
                        dest_issi: dest_ssi,
                        user_defined_data: sds_data,
                    }),
                });
                return true;
            }
        }

        if !dest_is_group && !is_local_issi {
            tracing::debug!(
                "SDS: raw Type-4 dest ISSI {} from Control not in local registry; delivering over RF anyway",
                dest_ssi
            );
        }

        self.send_d_sds_data(
            queue,
            source_ssi,
            dest_ssi,
            if dest_is_group { SsiType::Gssi } else { SsiType::Issi },
            sds_data,
        );

        true
    }

    /// Handle incoming U-STATUS from a local MS (via RF uplink)
    pub fn route_status_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("SDS route_status_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let calling_party = prim.received_tetra_address;

        let pdu = match UStatus::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing U-STATUS: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        if !Self::feature_check_u_status(&pdu) {
            tracing::warn!("Unsupported features in U-STATUS, dropping");
            return;
        }

        // Extract destination SSI (guaranteed present after feature check)
        let Some(dest_ssi_raw) = pdu.called_party_ssi else {
            tracing::warn!("SDS: U-STATUS missing called_party_ssi after feature check, dropping");
            return;
        };
        let dest_ssi = dest_ssi_raw as u32;

        let source_ssi = calling_party.ssi;

        tracing::info!(
            "SDS: U-STATUS from ISSI {} to ISSI {}, status={}",
            source_ssi,
            dest_ssi,
            pdu.pre_coded_status
        );

        // Emergency-state tracking. The radio re-sends pre-coded status Emergency while in
        // emergency and is silent on exit: ENTER/REFRESH on Emergency, and a non-Emergency status
        // from the same ISSI CLEARS its session (the "first normal status = user cancelled" signal).
        // Skip the 9999 command channel (restart/kick_all/info statuses) so an unrelated command
        // status from a radio currently in emergency does not double as an emergency cancellation.
        // Local-only — evaluated before any Brew forward and gated by the [emergency] config below.
        if dest_ssi != 9999 {
            match pdu.pre_coded_status {
                PreCodedStatus::Emergency => self.emergency_enter(source_ssi, dest_ssi),
                _ => self.emergency_clear(source_ssi, "non-emergency status"),
            }
        }

        // SDS command control: U-STATUS to ISSI 9999 from an authorized ISSI triggers
        // a system action (restart, shutdown, kick_all) if the status code matches.
        if dest_ssi == 9999 {
            self.handle_sds_command_status(queue, source_ssi, &pdu.pre_coded_status);
            return;
        }

        // Emergency status is LOCAL-only by design — never forwarded to Brew unless the operator
        // opts in via [emergency] forward_to_brew. Non-emergency statuses keep their normal routing.
        let is_emergency = matches!(pdu.pre_coded_status, PreCodedStatus::Emergency);
        let brew_entity = net_brew::route_entity_for_local_issi(&self.config, source_ssi);
        let brew_ok = brew_entity.is_some_and(|entity| net_brew::feature_sds_enabled_for_entity(&self.config, entity))
            && (!is_emergency || self.config.config().emergency.forward_to_brew);

        // Route: local delivery, Brew forward, or drop
        if self.config.state_read().subscribers.is_registered(dest_ssi) {
            tracing::info!("SDS-STATUS: local delivery: {} -> {}", source_ssi, dest_ssi);
            self.send_d_status(queue, source_ssi, dest_ssi, pdu.pre_coded_status);
        } else if brew_ok {
            // Brew forwarding only: when the pre-coded status carries an SDS-TL short report
            // (ETSI 29.4.2.3), convert it to a full SDS-TL REPORT PDU (Type4) so the
            // remote end recognizes it as a delivery confirmation. ETSI 29.3.3.4.4
            // explicitly allows SwMI to "modify a short report to a standard report."
            // Non-SDS-TL pre-coded statuses are forwarded as-is (Type1).
            // Local delivery (D-STATUS) is not affected, it stays as pre-coded status above.
            let user_defined_data = if let PreCodedStatus::SdsTl(report) = &pdu.pre_coded_status {
                let delivery_status = match report.short_report_type() {
                    ShortReportType::MessageReceived => 0x00,
                    ShortReportType::MessageConsumed => 0x00,
                    ShortReportType::DestMemFull => 0x02,
                    ShortReportType::ProtOrEncodingNotSupported => 0x01,
                };
                // PID 0x82 = SDS-TL text messaging. Hardcoded because the SDS-SHORT REPORT
                // PDU does not carry a Protocol Identifier (ETSI 29.4.3.11). In practice
                // all observed SDS-TL traffic uses PID 0x82.
                let sds_tl_report = vec![0x82, 0x10, delivery_status, report.message_reference()];
                tracing::info!(
                    "SDS-STATUS: converting SDS-TL short report to Type4 for Brew: MR={} status=0x{:02x}",
                    report.message_reference(),
                    delivery_status
                );
                SdsUserData::Type4(32, sds_tl_report)
            } else {
                SdsUserData::Type1(pdu.pre_coded_status.into_raw())
            };

            let brew_entity = brew_entity.expect("checked by brew_ok");
            tracing::info!("SDS-STATUS: forwarding to {:?}: {} -> {}", brew_entity, source_ssi, dest_ssi);
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: brew_entity,
                msg: SapMsgInner::CmceSdsData(CmceSdsData {
                    source_issi: source_ssi,
                    dest_issi: dest_ssi,
                    user_defined_data,
                }),
            });
        } else {
            tracing::warn!(
                "SDS-STATUS: dest ISSI {} not locally registered and not Brew-routable, dropping",
                dest_ssi
            );
        }
    }

    /// Build and send a D-STATUS PDU to a local MS.
    ///
    /// Like `send_d_sds_data`, this honours ETSI EN 300 392-2 §23.5 — an MS engaged in a
    /// call follows the FACCH on its assigned traffic timeslot and is NOT listening to the
    /// MCCH. So if the destination is currently on a traffic channel, the D-STATUS is
    /// delivered via half-slot stealing on that timeslot (Unacknowledged basic-link, because
    /// the LLC acknowledged path drops stealing messages — see `llc_bs_ms::rx_tla_tldata_req_bl`).
    /// Otherwise it goes on the MCCH as before. Without this, an in-call MS never receives
    /// status messages and the U-STATUS feedback chain (e.g. SDS-TL delivery short reports)
    /// silently breaks during a QSO.
    fn send_d_status(&self, queue: &mut MessageQueue, source_issi: u32, dest_issi: u32, pre_coded_status: PreCodedStatus) {
        let pdu = DStatus {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(source_issi as u64),
            calling_party_extension: None,
            pre_coded_status,
            external_subscriber_number: None,
            dm_ms_address: None,
        };

        tracing::debug!("-> D-STATUS {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(64);
        if let Err(e) = pdu.to_bitbuf(&mut sdu) {
            tracing::error!("Failed to serialize D-STATUS: {:?}", e);
            return;
        }
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_issi, SsiType::Issi);

        // Same FACCH-stealing routing as send_d_sds_data: an in-call MS is on its traffic
        // TS, not the MCCH. Reach it via stealing on that TS; the unacknowledged basic-link
        // path forwards stealing_permission + chan_alloc straight to UMAC.
        let traffic = {
            let state = self.config.state_read();
            state.active_call_ts.get(&dest_issi).copied().or_else(|| {
                // The dest ISSI may also be a member of an active group call — reach it on
                // the group's traffic timeslot.
                state
                    .subscribers
                    .attached_groups_of(dest_issi)
                    .into_iter()
                    .find_map(|gssi| state.active_call_ts.get(&gssi).copied())
            })
        };

        let (stealing_permission, chan_alloc, layer2service) = match traffic {
            Some((carrier_num, ts, usage)) if (1..=4).contains(&ts) => {
                let mut timeslots = [false; 4];
                timeslots[(ts - 1) as usize] = true;
                tracing::debug!(
                    "SDS-STATUS: dest {} is on traffic ts {} — delivering D-STATUS via FACCH stealing",
                    dest_issi,
                    ts
                );
                (
                    true,
                    Some(CmceChanAllocReq {
                        usage: Some(usage),
                        carrier: Some(carrier_num),
                        timeslots,
                        alloc_type: ChanAllocType::Replace,
                        ul_dl_assigned: UlDlAssignment::Dl,
                    }),
                    Layer2Service::Unacknowledged,
                )
            }
            _ => (false, None, Layer2Service::Todo),
        };

        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission,
                stealing_repeats_flag: false,
                chan_alloc,
                main_address: dest_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    // ── Built-in WX/METAR service ──────────────────────────────────────────
    //
    // Extract the text from an incoming SDS, parse the weather command, fetch the METAR on
    // a background thread (network I/O must not block the stack loop), then re-inject the
    // reply as a ControlCommand::SendSds — the same path the dashboard uses, so it lands
    // back in rx_sds_from_control on the stack thread.

    /// True when the SDS user data is an SDS-TL SHORT REPORT / STATUS PDU — i.e. a
    /// delivery confirmation rather than a text request. Recognised as PID 0x82/0x89 with
    /// message-type byte 0x10. Mirrors the `data[1] == 0x10` check in tetraflow-sds-bot's
    /// `parse_text_payload` / `handle_downlink_sds`, the proven discriminator that keeps
    /// reports out of the responder.
    fn is_sds_tl_report(data: &SdsUserData) -> bool {
        let bytes = data.to_arr();
        bytes.len() >= 4 && matches!(bytes.first(), Some(0x82) | Some(0x89)) && bytes[1] == 0x10
    }

    /// Message-reference byte (data[2]) of an SDS-TL text request — PID 0x82/0x89 that is
    /// not itself a report. Echoed back in the delivery confirmation, mirroring the
    /// `message_reference` the bot pulls in `parse_text_payload`. `None` when there is no
    /// usable SDS-TL header.
    fn sds_tl_message_reference(data: &SdsUserData) -> Option<u8> {
        let bytes = data.to_arr();
        if bytes.len() >= 4 && matches!(bytes.first(), Some(0x82) | Some(0x89)) && bytes[1] != 0x10 {
            Some(bytes[2])
        } else {
            None
        }
    }

    /// Pull the human-readable text out of an SDS user-data field. Handles the SDS-TL
    /// "simple text" wrapper (PID 0x82/0x80/0x8A, msg-type byte, message-ref, encoding,
    /// then text) and a bare text-coding-scheme prefix (0x01..=0x03).
    ///
    /// Any OTHER protocol identifier is treated as non-text and yields an empty string:
    /// LIP/APRS position beacons (PID 0x0A), status/precoded messages, and unknown PIDs are
    /// binary, and interpreting their bytes as ASCII produced garbage in the dashboard SDS Log
    /// (FH-BUG-045 — e.g. a LIP payload `[10, 48, ..]` decoded to the stray text "0"). Returning
    /// empty lets the dashboard fall back to the protocol-id label (e.g. "[LIP position]")
    /// instead of showing mojibake. Returns an ASCII string (best-effort).
    fn extract_sds_text(data: &SdsUserData) -> String {
        let bytes = data.to_arr();
        // SDS-TL text messaging PIDs: 0x82 (text), 0x80/0x8A (text w/ variants). When the
        // first byte is one of these and there is a 4-byte header, skip it. A bare text-coding-
        // scheme byte (0x01..=0x03) is followed directly by text. Everything else is binary.
        let payload: &[u8] = match bytes.first() {
            Some(0x82) | Some(0x80) | Some(0x8A) if bytes.len() > 4 => &bytes[4..],
            Some(0x01..=0x03) if bytes.len() > 1 => &bytes[1..],
            _ => return String::new(),
        };
        payload
            .iter()
            .filter(|&&b| b == b'\t' || (0x20..=0x7E).contains(&b))
            .map(|&b| b as char)
            .collect::<String>()
            .trim()
            .to_string()
    }

    /// Handle a weather request SDS addressed to the service ISSI. Spawns a worker that
    /// fetches the METAR and sends the reply back to `requester_issi`.
    fn handle_wx_request(&self, requester_issi: u32, data: &SdsUserData) {
        use crate::net_dashboard::wx_service::{self, WxRequest};

        let text = Self::extract_sds_text(data);
        tracing::info!("WX: request from ISSI {}: {:?}", requester_issi, text);

        let Some(tx) = self.wx_cmd_tx.clone() else {
            tracing::warn!("WX: no control sender wired, cannot reply to {}", requester_issi);
            return;
        };
        let service_issi = self.config.effective_wx_service().service_issi;

        // Only two commands exist: METAR (aviationweather) and WX (wttr.in). Anything else is
        // not a command and gets no reply. Both do blocking network I/O, so each runs on a
        // worker thread and re-injects its reply via the control channel.
        let Some(request) = wx_service::parse_wx_request(&text) else {
            tracing::debug!(
                "WX: ignoring non-command SDS from ISSI {} (only METAR/WX): {:?}",
                requester_issi,
                text
            );
            return;
        };

        std::thread::Builder::new()
            .name("wx-fetch".into())
            .spawn(move || {
                let reply = match request {
                    WxRequest::Metar(icao) => match wx_service::fetch_metar_decoded(&icao) {
                        Ok(decoded) if !decoded.is_empty() => decoded,
                        Ok(_) => format!("{icao}: no data"),
                        Err(e) => {
                            tracing::warn!("WX: METAR fetch {} failed: {}", icao, e);
                            format!("{icao}: unavailable")
                        }
                    },
                    WxRequest::Wx(loc) => match wx_service::fetch_wx(&loc) {
                        Ok(decoded) if !decoded.is_empty() => decoded,
                        Ok(_) => format!("{loc}: no data"),
                        Err(e) => {
                            tracing::warn!("WX: wttr fetch {} failed: {}", loc, e);
                            format!("{loc}: unavailable")
                        }
                    },
                };
                Self::queue_wx_reply(&tx, service_issi, requester_issi, &reply);
            })
            .ok();
    }

    /// Build a SendSds control command carrying `text` and push it onto the control queue.
    /// `payload` here is the bare text; rx_sds_from_control wraps it in the SDS-TL header.
    fn queue_wx_reply(tx: &crossbeam_channel::Sender<ControlCommand>, source_issi: u32, dest_issi: u32, text: &str) {
        // TETRA SDS-TL simple text is length-limited; trim to a safe size.
        let mut payload: Vec<u8> = text.bytes().take(220).collect();
        if payload.is_empty() {
            payload = b"(no data)".to_vec();
        }
        let len_bits = (payload.len() * 8) as u16;
        let cmd = ControlCommand::SendSds {
            handle: 0,
            source_ssi: source_issi,
            dest_ssi: dest_issi,
            dest_is_group: false,
            len_bits,
            payload,
        };
        if tx.send(cmd).is_err() {
            tracing::warn!("WX: failed to enqueue reply to ISSI {}", dest_issi);
        }
    }

    /// Called every tick. When periodic WX is enabled and the interval has elapsed, fetch
    /// the configured station's METAR and send it to the configured destination.
    pub fn tick_periodic_wx(&mut self) {
        let wx = self.config.effective_wx_service();
        if !wx.periodic_enabled || wx.periodic_issi == 0 || wx.periodic_icao.trim().is_empty() {
            return;
        }
        let interval = std::time::Duration::from_secs(wx.effective_interval_secs());
        let due = match self.last_periodic_wx {
            None => true,
            Some(t) => t.elapsed() >= interval,
        };
        if !due {
            return;
        }
        self.last_periodic_wx = Some(std::time::Instant::now());

        let Some(tx) = self.wx_cmd_tx.clone() else {
            return;
        };
        let icao = wx.periodic_icao.clone();
        let dest = wx.periodic_issi;
        let is_group = wx.periodic_is_group;
        let source_issi = wx.service_issi;

        std::thread::Builder::new()
            .name("wx-periodic".into())
            .spawn(move || {
                use crate::net_dashboard::wx_service;
                let reply = match wx_service::fetch_metar_decoded(&icao) {
                    Ok(d) if !d.is_empty() => d,
                    _ => return, // skip this cycle on failure; try again next interval
                };
                let payload: Vec<u8> = reply.bytes().take(220).collect();
                let len_bits = (payload.len() * 8) as u16;
                let cmd = ControlCommand::SendSds {
                    handle: 0,
                    source_ssi: source_issi,
                    dest_ssi: dest,
                    dest_is_group: is_group,
                    len_bits,
                    payload,
                };
                let _ = tx.send(cmd);
            })
            .ok();
    }

    /// Build and send a D-SDS-DATA PDU to a local MS.
    ///
    /// For an INDIVIDUAL destination that is currently unreachable on the MCCH — engaged in a call,
    /// or an energy-economy MS asleep outside its monitoring window — the SDS is DEFERRED and
    /// delivered when the destination is reachable again (see PendingSds / flush_pending_sds). The
    /// field radios do not accept an SDS in-band on the traffic channel, and an EE MS only listens
    /// on its monitoring window. All other cases (reachable ISSI, group/GSSI) are sent immediately.
    fn send_d_sds_data(
        &mut self,
        queue: &mut MessageQueue,
        source_issi: u32,
        dest_ssi: u32,
        dest_ssi_type: SsiType,
        user_defined_data: SdsUserData,
    ) {
        if dest_ssi_type == SsiType::Issi && (self.issi_on_local_traffic(dest_ssi) || self.ee_window_blocks(dest_ssi)) {
            tracing::info!(
                "SDS: dest {} not reachable on MCCH now (in call or EE-asleep) — deferring until reachable",
                dest_ssi
            );
            self.pending_sds.push(PendingSds {
                source_issi,
                dest_ssi,
                user_defined_data,
                queued_at: std::time::Instant::now(),
            });
            return;
        }

        self.deliver_d_sds_data_now(queue, source_issi, dest_ssi, dest_ssi_type, user_defined_data, false);
    }

    /// Build and send a D-SDS-DATA immediately (no reachability gating). Used for the direct path
    /// and for flushing deferred SDS once the destination is reachable.
    fn deliver_d_sds_data_now(
        &mut self,
        queue: &mut MessageQueue,
        source_issi: u32,
        dest_ssi: u32,
        dest_ssi_type: SsiType,
        user_defined_data: SdsUserData,
        force_mcch: bool,
    ) {
        let pdu = DSdsData {
            calling_party_type_identifier: PartyTypeIdentifier::Ssi,
            calling_party_address_ssi: Some(source_issi as u64),
            calling_party_extension: None,
            user_defined_data,
            external_subscriber_number: None,
            dm_ms_address: None,
        };

        tracing::debug!("-> D-SDS-DATA {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(128);
        if let Err(e) = pdu.to_bitbuf(&mut sdu) {
            tracing::error!("Failed to serialize D-SDS-DATA: {:?}", e);
            return;
        }
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_ssi, dest_ssi_type);

        // ETSI EN 300 392-2 §23.5: an MS engaged in a call follows the associated control
        // channel (FACCH) on its assigned traffic timeslot and is NOT listening to the MCCH.
        // So if the destination is currently on a traffic channel, deliver the SDS by stealing
        // a half-slot on that timeslot; otherwise send on the MCCH as before. Without this, SDS
        // sent while a call is up are never received. The map is rebuilt from live call state
        // every tick, so it cannot point at a stale/closed circuit.
        let traffic = if force_mcch {
            // Caller knows the destination is camped on the MCCH right now (e.g. it just sent
            // us an uplink U-STATUS via random access on the MCCH), so skip the traffic
            // inference entirely. Without this, an MS that is merely *affiliated* to a group
            // which happens to have an active call is mistaken for "following that call on the
            // traffic channel" and the SDS is FACCH-stolen onto a timeslot the idle MS is not
            // listening to (or deferred). That is FH-BUG-038: a U-STATUS remote-control reply
            // never reaches the requesting (idle, scanning-off) radio while any voice traffic
            // is up, even though its own talkgroup is idle.
            None
        } else {
            let state = self.config.state_read();
            state.active_call_ts.get(&dest_ssi).copied().or_else(|| {
                // Individual SDS to an MS that is a member of an active group call: reach it on
                // that group's traffic timeslot.
                if dest_ssi_type == SsiType::Issi {
                    state
                        .subscribers
                        .attached_groups_of(dest_ssi)
                        .into_iter()
                        .find_map(|gssi| state.active_call_ts.get(&gssi).copied())
                } else {
                    None
                }
            })
        };

        let (stealing_permission, chan_alloc) = match traffic {
            Some((carrier_num, ts, usage)) if (1..=4).contains(&ts) => {
                let mut timeslots = [false; 4];
                timeslots[(ts - 1) as usize] = true;
                tracing::debug!("SDS: dest {} is on traffic ts {} — delivering via FACCH stealing", dest_ssi, ts);
                (
                    true,
                    Some(CmceChanAllocReq {
                        usage: Some(usage),
                        carrier: Some(carrier_num),
                        timeslots,
                        alloc_type: ChanAllocType::Replace,
                        ul_dl_assigned: UlDlAssignment::Dl,
                    }),
                )
            }
            // Idle destination (or no active call): MCCH, exactly as before.
            _ => (false, None),
        };

        // Choose the LLC basic-link service. When stealing a half-slot to reach an MS that is
        // engaged in a call, we MUST use the unacknowledged basic link: the LLC acknowledged
        // path (rx_tla_tldata_req_bl) explicitly drops any message with stealing_permission set
        // ("BL-DATA requested for STCH message — not supported, dropping"), so an Acknowledged
        // SDS to an in-call MS would never be transmitted. The unacknowledged path forwards the
        // stealing permission and chan_alloc straight down to the MAC. On the MCCH (idle dest)
        // we keep the previous behaviour: acknowledged for individual SDS, unacknowledged for
        // group/other addressing.
        let layer2service = if stealing_permission {
            Layer2Service::Unacknowledged
        } else {
            match dest_ssi_type {
                SsiType::Issi => Layer2Service::Acknowledged,
                _ => Layer2Service::Unacknowledged,
            }
        };

        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission,
                stealing_repeats_flag: false,
                chan_alloc,
                main_address: dest_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn feature_check_u_sds_data(pdu: &USdsData) -> bool {
        let mut supported = true;
        if pdu.called_party_ssi.is_none() {
            if pdu.called_party_short_number_address.is_some() {
                unimplemented_log!("SDS: short number addressing not supported");
            } else {
                tracing::warn!("SDS: no destination address in U-SDS-DATA");
            }
            supported = false;
        }
        if pdu.called_party_extension.is_some() {
            unimplemented_log!("SDS: TSI extension addressing not supported");
        }
        if pdu.external_subscriber_number.is_some() {
            unimplemented_log!("SDS: external_subscriber_number not supported");
        }
        if pdu.dm_ms_address.is_some() {
            unimplemented_log!("SDS: dm_ms_address not supported");
        }
        supported
    }

    fn feature_check_u_status(pdu: &UStatus) -> bool {
        let mut supported = true;
        if pdu.called_party_ssi.is_none() {
            if pdu.called_party_short_number_address.is_some() {
                unimplemented_log!("SDS-STATUS: short number addressing not supported");
            } else {
                tracing::warn!("SDS-STATUS: no destination address in U-STATUS");
            }
            supported = false;
        }
        if pdu.called_party_extension.is_some() {
            unimplemented_log!("SDS-STATUS: TSI extension addressing not supported");
        }
        if pdu.external_subscriber_number.is_some() {
            unimplemented_log!("SDS-STATUS: external_subscriber_number not supported");
        }
        if pdu.dm_ms_address.is_some() {
            unimplemented_log!("SDS-STATUS: dm_ms_address not supported");
        }
        supported
    }

    /// Execute a system action triggered by an SDS U-STATUS command to ISSI 9999.
    /// Send a short text reply as an SDS-TL simple-text message from `source_issi` to `dest_issi`.
    /// Used by the U-STATUS info responder (FH-FEAT-014). Mirrors the SDS-TL framing used elsewhere:
    /// [PID 0x82, message type 0x04, message reference, encoding 0x01 (ISO-8859-1), text…].
    fn send_text_sds(&mut self, queue: &mut MessageQueue, source_issi: u32, dest_issi: u32, text: &str) {
        static SDS_MR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
        let mr = {
            let v = SDS_MR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if v == 0 {
                SDS_MR.store(1, std::sync::atomic::Ordering::Relaxed);
                1
            } else {
                v
            }
        };
        let mut payload = vec![0x82u8, 0x04u8, mr, 0x01u8];
        // Keep printable ASCII only (the encoding byte declares ISO-8859-1/ASCII).
        payload.extend(text.bytes().filter(|&b| b == b'\t' || (0x20..=0x7E).contains(&b)));
        let len_bits = (payload.len() * 8) as u16;
        // Deliver the reply on the MCCH unconditionally: this is a response to a U-STATUS the
        // destination radio just sent us via random access on the MCCH, so it is provably
        // camped there right now (FH-BUG-038). Routing through send_d_sds_data instead would
        // defer/steal the reply whenever the requester is affiliated to some group that has an
        // active call — even though the requester's own talkgroup is idle and it is listening
        // on the MCCH — so the radio never sees its reply and retransmits the U-STATUS until
        // timeout.
        self.deliver_d_sds_data_now(
            queue,
            source_issi,
            dest_issi,
            SsiType::Issi,
            SdsUserData::Type4(len_bits, payload),
            true,
        );
    }

    fn handle_sds_command_status(&mut self, queue: &mut MessageQueue, source_ssi: u32, status: &PreCodedStatus) {
        let status_code = status.into_raw() as u16;

        let cfg = self.config.config();
        let Some(ref ctrl) = cfg.cell.sds_command_control else {
            tracing::debug!(
                "SDS-CMD: U-STATUS to 9999 from {} (status={}) but sds_command_control not configured, ignoring",
                source_ssi,
                status_code
            );
            return;
        };

        if !ctrl.authorized_issis.contains(&source_ssi) {
            tracing::warn!(
                "SDS-CMD: U-STATUS to 9999 from ISSI {} (status={}) — ISSI not in authorized_issis, ignoring",
                source_ssi,
                status_code
            );
            return;
        }

        let Some(entry) = ctrl.commands.iter().find(|e| e.status_code == status_code) else {
            tracing::debug!(
                "SDS-CMD: U-STATUS to 9999 from ISSI {} status={} — no matching command, ignoring",
                source_ssi,
                status_code
            );
            return;
        };

        tracing::info!(
            "SDS-CMD: ISSI {} triggered action='{}' via status={}",
            source_ssi,
            entry.action,
            status_code
        );

        match entry.action.as_str() {
            "restart" => {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Restart,
                    std::time::Duration::from_millis(500),
                );
            }
            "shutdown" => {
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Stop,
                    std::time::Duration::from_millis(500),
                );
            }
            "kick_all" => {
                self.pending_actions.push(SdsPendingAction::KickAll);
            }
            // ── FH-FEAT-014: query the host and reply to the requester as an SDS ──
            "ip" => {
                let ip = crate::sys_telemetry::primary_ip().unwrap_or_else(|| "n/a".to_string());
                self.send_text_sds(queue, 9999, source_ssi, &format!("Host IP: {ip}"));
            }
            "temp" => {
                let temp = crate::sys_telemetry::cpu_temp_c()
                    .map(|c| format!("{c:.1} C"))
                    .unwrap_or_else(|| "n/a".to_string());
                self.send_text_sds(queue, 9999, source_ssi, &format!("Host temp: {temp}"));
            }
            "info" => {
                let ip = crate::sys_telemetry::primary_ip().unwrap_or_else(|| "n/a".to_string());
                let temp = crate::sys_telemetry::cpu_temp_c()
                    .map(|c| format!("{c:.1}C"))
                    .unwrap_or_else(|| "n/a".to_string());
                self.send_text_sds(
                    queue,
                    9999,
                    source_ssi,
                    &format!("FlowStation v{} | IP {} | {}", tetra_core::STACK_VERSION, ip, temp),
                );
            }
            other => {
                tracing::warn!("SDS-CMD: unknown action '{}' for status={}, ignoring", other, status_code);
            }
        }
    }
}
