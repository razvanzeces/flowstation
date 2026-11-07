use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::mle::components::broadcast::MleBroadcast;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, EndpointId, Layer2Service, LinkId, Sap, TdmaTime, TetraAddress, TxReporter, unimplemented_log};
use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use tetra_pdus::cmce::pdus::d_info::DInfo;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mle::enums::mle_pdu_type_ul::MlePduTypeUl;
use tetra_pdus::mle::pdus::{
    d_new_cell::DNewCell, d_prepare_fail::DPrepareFail, d_restore_ack::DRestoreAck, d_restore_fail::DRestoreFail,
    u_prepare::UPrepare, u_restore::URestore,
};
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_saps::control::call_control::{CallControl, CellReselection, HandoverChannel, HandoverOutcome};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::tla::{TlaTlDataReqBl, TlaTlUnitdataReqBl};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;

pub struct MleBs {
    config: SharedConfig,
    broadcast: MleBroadcast,
    /// MSs whose U-RESTORE (cell reselection) we handed to CMCE: CMCE's answer to the U-CALL
    /// RESTORE it carried goes back inside D-RESTORE-ACK / D-RESTORE-FAIL.
    pending_restores: HashMap<u32, Instant>,
    /// Serving cell: MSs whose D-NEW-CELL waits for the target cell's answer to the registration
    /// they forwarded in U-PREPARE (type 1 announced reselection).
    pending_prepares: HashMap<u32, PendingPrepare>,
    /// Target cell: MSs whose forwarded registration we handed to MM; MM's answer goes back to the
    /// serving cell instead of on air here.
    pending_forwards: HashMap<u32, Instant>,
    /// Serving cell: type 1 D-NEW-CELLs waiting on the traffic channel's STCH.
    pending_type1: HashMap<u32, PendingType1>,
}

/// A type 1 grant (D-NEW-CELL "follow MAC channel change" with a channel allocation for the
/// other cell) handed to layer 2. If it cannot go out on the MS's traffic channel, type 2 is
/// granted instead with the same registration accept.
struct PendingType1 {
    stch: (u16, u8),
    accept: BitBuffer,
    reporter: TxReporter,
    since: Instant,
}

struct PendingPrepare {
    link_id: LinkId,
    endpoint_id: EndpointId,
    since: Instant,
    /// "Location update type" of the forward registration the U-PREPARE carried, if any: a
    /// D-LOCATION UPDATE REJECT built here for it repeats the type.
    forwarded: Option<u8>,
    /// The target is a cell of another station, asked through Brew (not a sibling cell).
    cross_station: bool,
    /// Traffic channel (carrier, timeslot) the U-PREPARE came in on, if the MS sent it on an
    /// assigned channel rather than on the MCCH: the answer is stolen there, since an MS on an
    /// assigned channel receives its signalling on that channel (clause 23.8.4.2).
    stch: Option<(u16, u8)>,
}

/// How long the serving cell waits for the target cell's answer to a forwarded registration
/// before sending D-NEW-CELL without it (the MS then registers on arrival).
const FORWARD_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(3);

/// How long CMCE may take to answer the U-CALL RESTORE carried by a U-RESTORE.
const RESTORE_ANSWER_TIMEOUT: Duration = Duration::from_secs(10);
/// D-NEW-CELL "channel command valid": change channel immediately (TS 100 392-2 clause 18.5.6).
const CHANNEL_COMMAND_CHANGE_IMMEDIATELY: u8 = 1;
/// D-NEW-CELL "channel command valid": follow MAC channel change, i.e. the channel allocation in
/// the MAC header (type 1, table 18.43).
const CHANNEL_COMMAND_FOLLOW_MAC: u8 = 0;
/// On the STCH layer 2 sends the D-INFO with the new call identifier unacknowledged, so it goes
/// out several times before the type 1 D-NEW-CELL (clause 18.3.4.7.5 note 7).
const TYPE1_D_INFO_REPEATS: usize = 3;
/// How long a type 1 D-NEW-CELL may wait for its stolen slot before type 2 is granted instead.
const TYPE1_SEND_TIMEOUT: Duration = Duration::from_secs(1);
/// D-PREPARE-FAIL "fail cause" 0: temporary break in service (clause 18.5.7).
const FAIL_CAUSE_TEMPORARY_BREAK: u8 = 0;
/// D-PREPARE-FAIL "fail cause" 2: MS not allowed on cell (clause 18.5.7).
const FAIL_CAUSE_MS_NOT_ALLOWED: u8 = 2;
/// D-LOCATION UPDATE REJECT "reject cause" 7: forward registration failure (clause 16.10.42,
/// table 16.81).
const REJECT_CAUSE_FORWARD_REGISTRATION_FAILURE: u8 = 7;
/// D-RESTORE-FAIL "fail cause" 3: restoration cannot be done on cell, the value clause
/// 18.3.4.7 requires for a failed restore (clause 18.5.7).
const FAIL_CAUSE_RESTORATION_NOT_POSSIBLE: u8 = 3;

/// Multiframes at which D-NWRK-BROADCAST is sent within each hyperframe.
/// Two broadcasts per hyperframe (~30.6s interval) for faster time/date display on terminals.
/// BlueStation default was 1 per hyperframe (~61.2s) which is slow on cold attach.
/// We don't use the first multiframe to avoid congestion with other hyperframe-triggered events.
const MLE_BROADCAST_MULTIFRAMES: [u8; 2] = [20, 50];
/// Frame at which D-NWRK-BROADCAST is sent within the broadcast multiframe.
const MLE_BROADCAST_FRAME: u8 = 1;

impl MleBs {
    pub fn new(config: SharedConfig) -> Self {
        let broadcast = MleBroadcast::new(config.clone());
        Self {
            config,
            broadcast,
            pending_restores: HashMap::new(),
            pending_prepares: HashMap::new(),
            pending_forwards: HashMap::new(),
            pending_type1: HashMap::new(),
        }
    }

    /// An uplink MLE PDU (protocol discriminator already consumed from `sdu`).
    fn rx_mle_pdu(&mut self, queue: &mut MessageQueue, mut sdu: BitBuffer, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId) {
        let Some(bits) = sdu.peek_bits(3) else {
            tracing::warn!("MLE: uplink MLE PDU too short: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MlePduTypeUl::try_from(bits) else {
            tracing::warn!("MLE: invalid uplink MLE PDU type {} from {}", bits, addr.ssi);
            return;
        };
        match pdu_type {
            MlePduTypeUl::UPrepare => match UPrepare::from_bitbuf(&mut sdu) {
                Ok(pdu) => self.rx_u_prepare(queue, addr, link_id, endpoint_id, pdu),
                Err(e) => tracing::warn!("MLE: bad U-PREPARE from {}: {:?}", addr.ssi, e),
            },
            MlePduTypeUl::URestore => match URestore::from_bitbuf(&mut sdu) {
                Ok(pdu) => self.rx_u_restore(queue, addr, link_id, endpoint_id, pdu),
                Err(e) => tracing::warn!("MLE: bad U-RESTORE from {}: {:?}", addr.ssi, e),
            },
            MlePduTypeUl::UPrepareDa => unimplemented_log!("UPrepareDa"),
            MlePduTypeUl::UIrregularChannelAdvice => unimplemented_log!("UIrregularChannelAdvice"),
            MlePduTypeUl::UChannelClassAdvice => unimplemented_log!("UChannelClassAdvice"),
            MlePduTypeUl::UChannelRequest => unimplemented_log!("UChannelRequest"),
            MlePduTypeUl::ExtPdu => unimplemented_log!("ExtPdu"),
        }
    }

    /// Announced type 1/2 cell reselection (TS 100 392-2 clause 18.3.4.7.5): the MS asks to move
    /// to the neighbour `cell_identifier_ca`. A neighbour we do not advertise gets D-PREPARE-FAIL.
    /// Otherwise the target is prepared before D-NEW-CELL goes out: a sibling cell of a linked
    /// multi-cell station through the site switch, a cell of another station through Brew
    /// (`handover-v1`, also from a cell of a multi-cell station, whose switch passes the request
    /// on). Only a station with neither answers D-NEW-CELL at once.
    fn rx_u_prepare(&mut self, queue: &mut MessageQueue, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId, pdu: UPrepare) {
        let cfg = self.config.config();
        if pdu.cell_identifier_ca.is_none() {
            // Announced type 3 (clause 18.3.4.7.4): no preferred cell yet, so nothing to prepare.
            // D-PREPARE-FAIL "shall not be a valid response" here; answer D-NEW-CELL "change
            // channel immediately". The radio then restores its calls on the new cell.
            tracing::info!("MLE: U-PREPARE from {} without a cell (type 3) → D-NEW-CELL", addr.ssi);
            let new_cell = DNewCell {
                channel_command_valid: CHANNEL_COMMAND_CHANGE_IMMEDIATELY,
                sdu: pdu.sdu.as_ref().map(|demand| forward_registration_failure(location_update_type(demand))),
            };
            let stch = Self::stch_of_link(cfg.cell.main_carrier, link_id);
            self.send_l2(queue, addr, link_id, endpoint_id, MleProtocolDiscriminator::Mle, None, stch, None, |b| {
                new_cell.to_bitbuf(b)
            });
            self.report_reselection(queue, addr.ssi, CellReselection::AnnouncedType3);
            return;
        }
        let target = pdu
            .cell_identifier_ca
            .and_then(|ci| cfg.cell.neighbor_cells_ca.iter().find(|n| n.cell_identifier_ca as u64 == ci));
        let Some(target) = target else {
            tracing::info!(
                "MLE: U-PREPARE from {} for unknown neighbour {:?} → D-PREPARE-FAIL",
                addr.ssi,
                pdu.cell_identifier_ca
            );
            let fail = DPrepareFail {
                fail_cause: FAIL_CAUSE_TEMPORARY_BREAK,
                sdu: pdu.sdu.as_ref().map(|demand| forward_registration_failure(location_update_type(demand))),
            };
            let stch = Self::stch_of_link(cfg.cell.main_carrier, link_id);
            self.send_l2(queue, addr, link_id, endpoint_id, MleProtocolDiscriminator::Mle, None, stch, None, |b| fail.to_bitbuf(b));
            return;
        };

        let target_carrier = target.main_carrier_number;
        let stch = Self::stch_of_link(cfg.cell.main_carrier, link_id);
        let pending = |forwarded: Option<u8>, cross_station: bool| PendingPrepare {
            link_id,
            endpoint_id,
            since: Instant::now(),
            forwarded,
            cross_station,
            stch,
        };
        if crate::net_brew::is_site_linked(&self.config) && cfg.is_station_carrier(target_carrier) {
            tracing::info!(
                "MLE: U-PREPARE from {} → sibling cell ci={} carrier={}",
                addr.ssi,
                target.cell_identifier_ca,
                target_carrier
            );
            Self::push_site(queue, CallControl::SiteHandoverPrepare { issi: addr.ssi, target_carrier });
            if let Some(sdu) = pdu.sdu {
                // Forward registration: register on the target cell now; D-NEW-CELL carries its
                // MM's answer.
                tracing::info!("MLE: forwarding {}'s registration to the cell on carrier {}", addr.ssi, target_carrier);
                self.pending_prepares.insert(addr.ssi, pending(Some(location_update_type(&sdu)), false));
                Self::push_site(
                    queue,
                    CallControl::SiteForwardRegistration {
                        issi: addr.ssi,
                        target_carrier,
                        sdu,
                    },
                );
                return;
            }
        } else if crate::net_brew::is_active(&self.config) {
            // The other station may serve a different band plan only if the entry says so.
            let target_carrier_ext = target
                .main_carrier_number_extension
                .or_else(|| tetra_config::bluestation::carrier_band_plan(&cfg.cell));
            if let Some(target_carrier_ext) = target_carrier_ext {
                tracing::info!(
                    "MLE: U-PREPARE from {} → asking the station of neighbour ci={} carrier={}{}",
                    addr.ssi,
                    target.cell_identifier_ca,
                    target_carrier,
                    if pdu.sdu.is_some() { " (forward registration)" } else { "" }
                );
                self.pending_prepares
                    .insert(addr.ssi, pending(pdu.sdu.as_ref().map(location_update_type), true));
                Self::push_site(
                    queue,
                    CallControl::HandoverRequest {
                        issi: addr.ssi,
                        target_carrier,
                        target_carrier_ext,
                        target_mcc: target.mcc,
                        target_mnc: target.mnc,
                        target_location_area: target.location_area,
                        sdu: pdu.sdu,
                    },
                );
                return;
            }
        }
        tracing::info!(
            "MLE: U-PREPARE from {} → D-NEW-CELL (neighbour ci={} carrier={}, not prepared)",
            addr.ssi,
            target.cell_identifier_ca,
            target_carrier
        );
        let new_cell = DNewCell {
            channel_command_valid: CHANNEL_COMMAND_CHANGE_IMMEDIATELY,
            sdu: pdu.sdu.as_ref().map(|demand| forward_registration_failure(location_update_type(demand))),
        };
        self.send_l2(queue, addr, link_id, endpoint_id, MleProtocolDiscriminator::Mle, None, stch, None, |b| {
            new_cell.to_bitbuf(b)
        });
        self.report_reselection(queue, addr.ssi, CellReselection::AnnouncedType2);
    }

    /// The MS leaves by announced reselection of this type (D-NEW-CELL sent): Brew tells the
    /// server, for its cell change log.
    fn report_reselection(&self, queue: &mut MessageQueue, issi: u32, reselection: CellReselection) {
        if crate::net_brew::is_active(&self.config) {
            Self::push_site(queue, CallControl::ReselectionAnnounced { issi, reselection });
        }
    }

    /// The traffic channel an uplink PDU on `link_id` came from: timeslots 2 to 4 of the main
    /// carrier are assigned channels; timeslot 1 is the MCCH.
    fn stch_of_link(main_carrier: u16, link_id: LinkId) -> Option<(u16, u8)> {
        match link_id {
            2..=4 => Some((main_carrier, link_id as u8)),
            _ => None,
        }
    }

    fn push_site(queue: &mut MessageQueue, cc: CallControl) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mle,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(cc),
        });
    }

    /// Serving cell: the target answered (or could not be prepared). Send the withheld answer to
    /// the U-PREPARE, using only the combinations clause 18.3.4.7.5 / 18.3.4.7.6 allow:
    ///
    /// - accepted: D-NEW-CELL "change channel immediately" (type 2) with the target's
    ///   D-LOCATION UPDATE ACCEPT, if a registration was forwarded;
    /// - rejected by the target's MM: D-PREPARE-FAIL with its D-LOCATION UPDATE REJECT, fail cause
    ///   "MS not allowed on cell" unless the reject cause is "forward registration failure";
    /// - target station gone: D-PREPARE-FAIL "temporary break in service";
    /// - not prepared (unknown, no answer in time): type 2 anyway, the MS registers on arrival
    ///   (figure 18.12): D-NEW-CELL with a D-LOCATION UPDATE REJECT "forward registration
    ///   failure" if a registration was forwarded, since a successful D-NEW-CELL after a forward
    ///   registration request "shall contain a MM SDU".
    fn finish_prepare(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        outcome: HandoverOutcome,
        answer: Option<BitBuffer>,
        channel: Option<HandoverChannel>,
    ) {
        let Some(p) = self.pending_prepares.remove(&issi) else {
            return;
        };
        let addr = TetraAddress {
            ssi_type: tetra_core::SsiType::Issi,
            ssi: issi,
        };
        let is_reject = |a: &BitBuffer| {
            a.peek_bits_startoffset(0, 4)
                .is_some_and(|t| t == MmPduTypeDl::DLocationUpdateReject.into_raw())
        };
        // An accept without a forwarded registration, or no answer to one, is not an accept.
        let outcome = match (outcome, &answer, p.forwarded) {
            (HandoverOutcome::Accepted, Some(a), _) if is_reject(a) => HandoverOutcome::Rejected,
            (HandoverOutcome::Accepted, None, Some(_)) => HandoverOutcome::NotPrepared,
            (HandoverOutcome::Rejected, a, _) if !a.as_ref().is_some_and(is_reject) => HandoverOutcome::NotPrepared,
            (o, _, _) => o,
        };
        let local_reject = || p.forwarded.map(forward_registration_failure);
        match outcome {
            HandoverOutcome::Rejected | HandoverOutcome::TargetGone => {
                let (fail_cause, sdu) = if outcome == HandoverOutcome::Rejected {
                    let reject = answer.expect("checked above");
                    let cause = if reject_cause(&reject) == Some(REJECT_CAUSE_FORWARD_REGISTRATION_FAILURE) {
                        FAIL_CAUSE_TEMPORARY_BREAK
                    } else {
                        FAIL_CAUSE_MS_NOT_ALLOWED
                    };
                    (cause, Some(reject))
                } else {
                    (FAIL_CAUSE_TEMPORARY_BREAK, local_reject())
                };
                tracing::info!("MLE: reselection of {} refused ({:?}) → D-PREPARE-FAIL cause {}", issi, outcome, fail_cause);
                let fail = DPrepareFail { fail_cause, sdu };
                self.send_l2(queue, addr, p.link_id, p.endpoint_id, MleProtocolDiscriminator::Mle, None, p.stch, None, |b| {
                    fail.to_bitbuf(b)
                });
                if p.cross_station && p.forwarded.is_some() {
                    Self::push_site(queue, CallControl::HandoverFailed { issi });
                }
            }
            HandoverOutcome::Accepted | HandoverOutcome::NotPrepared => {
                let accepted = outcome == HandoverOutcome::Accepted && answer.is_some();
                if let (true, true, Some(channel), Some(accept)) = (accepted, p.cross_station, channel, answer.as_ref()) {
                    self.grant_type1(queue, addr, &p, channel, accept.clone());
                    return;
                }
                let sdu = if outcome == HandoverOutcome::Accepted { answer } else { local_reject() };
                tracing::info!(
                    "MLE: D-NEW-CELL to {}{}",
                    issi,
                    match (accepted, sdu.is_some()) {
                        (true, _) => " with the target cell's registration accept",
                        (false, true) => " with forward registration failure (registers on arrival)",
                        (false, false) => "",
                    }
                );
                let new_cell = DNewCell {
                    channel_command_valid: CHANNEL_COMMAND_CHANGE_IMMEDIATELY,
                    sdu,
                };
                self.send_l2(queue, addr, p.link_id, p.endpoint_id, MleProtocolDiscriminator::Mle, None, p.stch, None, |b| {
                    new_cell.to_bitbuf(b)
                });
                self.report_reselection(queue, issi, CellReselection::AnnouncedType2);
                if p.cross_station && accepted {
                    Self::push_site(queue, CallControl::HandoverDone { issi });
                }
            }
        }
    }

    /// Type 1 announced cell reselection (clause 18.3.4.7.5): the other station runs the MS's
    /// group call and gave its channel. If its call identifier differs, D-INFO carries the new
    /// one first; then D-NEW-CELL "follow MAC channel change" with the registration accept goes
    /// out with a channel allocation for the other cell ("cell change flag", clause 23.5.4.1),
    /// both on the STCH of the traffic channel the MS listens to here. The MS keeps its transmit
    /// permission and goes straight to the new channel, without U-RESTORE (figure 18.14).
    fn grant_type1(&mut self, queue: &mut MessageQueue, addr: TetraAddress, p: &PendingPrepare, channel: HandoverChannel, accept: BitBuffer) {
        let stch = (channel.serving_carrier, channel.serving_timeslot);
        tracing::info!(
            "MLE: type 1 reselection for {}: call {} → {} on carrier {} ts {} (STCH carrier {} ts {})",
            addr.ssi,
            channel.call_id,
            channel.new_call_id,
            channel.carrier,
            channel.timeslot,
            stch.0,
            stch.1
        );
        if channel.new_call_id != channel.call_id {
            let info = DInfo {
                call_identifier: channel.call_id,
                reset_call_time_out_timer_t310_: false,
                poll_request: false,
                new_call_identifier: Some(channel.new_call_id as u64),
                call_time_out: None,
                call_time_out_set_up_phase_t301_t302_: None,
                call_ownership: None,
                modify: None,
                call_status: None,
                temporary_address: None,
                notification_indicator: None,
                poll_response_percentage: None,
                poll_response_number: None,
                dtmf: None,
                facility: None,
                poll_response_addresses: None,
                proprietary: None,
            };
            for _ in 0..TYPE1_D_INFO_REPEATS {
                self.send_l2(queue, addr, p.link_id, p.endpoint_id, MleProtocolDiscriminator::Cmce, None, Some(stch), None, |b| {
                    info.to_bitbuf(b)
                });
            }
        }
        let mut timeslots = [false; 4];
        timeslots[(channel.timeslot.clamp(1, 4) - 1) as usize] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(channel.usage),
            carrier: Some(channel.carrier),
            timeslots,
            // Replace keeps the traffic authorization (clause 23.8.2.3.1)
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
            cell_change: true,
        };
        let new_cell = DNewCell {
            channel_command_valid: CHANNEL_COMMAND_FOLLOW_MAC,
            sdu: Some(accept.clone()),
        };
        let reporter = TxReporter::new_unacked();
        self.send_l2(
            queue,
            addr,
            p.link_id,
            p.endpoint_id,
            MleProtocolDiscriminator::Mle,
            Some(chan_alloc),
            Some(stch),
            Some(reporter.clone()),
            |b| new_cell.to_bitbuf(b),
        );
        self.pending_type1.insert(
            addr.ssi,
            PendingType1 {
                stch,
                accept,
                reporter,
                since: Instant::now(),
            },
        );
    }

    /// Type 1 grants that went out are done; those layer 2 could not send become type 2.
    fn drive_type1(&mut self, queue: &mut MessageQueue) {
        let issis: Vec<u32> = self.pending_type1.keys().copied().collect();
        for issi in issis {
            let p = &self.pending_type1[&issi];
            let sent = p.reporter.is_transmitted();
            if !sent && !p.reporter.is_discarded() && p.since.elapsed() < TYPE1_SEND_TIMEOUT {
                continue;
            }
            let p = self.pending_type1.remove(&issi).expect("present");
            if !sent {
                tracing::info!("MLE: type 1 D-NEW-CELL for {} could not be sent → type 2", issi);
                let addr = TetraAddress {
                    ssi_type: tetra_core::SsiType::Issi,
                    ssi: issi,
                };
                let new_cell = DNewCell {
                    channel_command_valid: CHANNEL_COMMAND_CHANGE_IMMEDIATELY,
                    sdu: Some(p.accept),
                };
                self.send_l2(queue, addr, 0, 0, MleProtocolDiscriminator::Mle, None, Some(p.stch), None, |b| {
                    new_cell.to_bitbuf(b)
                });
            }
            let reselection = if sent { CellReselection::AnnouncedType1 } else { CellReselection::AnnouncedType2 };
            self.report_reselection(queue, issi, reselection);
            Self::push_site(queue, CallControl::HandoverDone { issi });
        }
    }

    /// Target cell: an MS on a sibling cell forwarded its registration to us. Hand it to MM as if
    /// received over the air; MM's answer is captured in `rx_lmm_mle_unitdata_req`.
    fn rx_forward_registration(&mut self, queue: &mut MessageQueue, issi: u32, sdu: BitBuffer) {
        tracing::info!("MLE: forwarded registration from {} (announced reselection to this cell)", issi);
        self.pending_forwards.insert(issi, Instant::now());
        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Mm,
            msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
                sdu,
                handle: 0,
                received_address: TetraAddress {
                    ssi_type: tetra_core::SsiType::Issi,
                    ssi: issi,
                },
            }),
        });
    }

    fn rx_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistration { issi, sdu, .. }) => {
                self.rx_forward_registration(queue, issi, sdu);
            }
            SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistrationResult { issi, sdu }) => {
                // A sibling cell's MM answered (or the switch had no cell to ask).
                let outcome = if sdu.is_some() { HandoverOutcome::Accepted } else { HandoverOutcome::NotPrepared };
                self.finish_prepare(queue, issi, outcome, sdu, None);
            }
            SapMsgInner::CmceCallControl(CallControl::HandoverResult { issi, outcome, sdu, channel }) => {
                self.finish_prepare(queue, issi, outcome, sdu, channel);
            }
            _ => tracing::warn!("MLE: unexpected control message, ignoring"),
        }
    }

    /// The MS arrived after cell reselection and restores its call: hand the carried U-CALL
    /// RESTORE to CMCE; its answer is wrapped in D-RESTORE-ACK / D-RESTORE-FAIL (see
    /// `rx_lcmc_mle_unitdata_req`).
    fn rx_u_restore(&mut self, queue: &mut MessageQueue, addr: TetraAddress, link_id: LinkId, endpoint_id: EndpointId, pdu: URestore) {
        let Some(sdu) = pdu.sdu else {
            tracing::info!("MLE: U-RESTORE from {} without U-CALL RESTORE → D-RESTORE-FAIL", addr.ssi);
            let fail = DRestoreFail {
                fail_cause: FAIL_CAUSE_RESTORATION_NOT_POSSIBLE,
            };
            self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| fail.to_bitbuf(b));
            return;
        };
        tracing::info!("MLE: U-RESTORE from {} → CMCE", addr.ssi);
        self.pending_restores.insert(addr.ssi, Instant::now());
        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
                sdu,
                handle: 0,
                received_tetra_address: addr,
                endpoint_id,
                link_id,
                chan_change_resp_req: false,
                chan_change_handle: None,
            }),
        });
    }

    /// Send an MLE PDU (protocol discriminator MLE) to one MS, acknowledged.
    fn send_mle_pdu(
        &self,
        queue: &mut MessageQueue,
        addr: TetraAddress,
        link_id: LinkId,
        endpoint_id: EndpointId,
        write: impl FnOnce(&mut BitBuffer) -> Result<(), tetra_core::PduParseErr>,
    ) {
        self.send_mle_pdu_with_chan_alloc(queue, addr, link_id, endpoint_id, None, write);
    }

    /// As `send_mle_pdu`, with a layer 2 channel allocation sent along with the PDU.
    fn send_mle_pdu_with_chan_alloc(
        &self,
        queue: &mut MessageQueue,
        addr: TetraAddress,
        link_id: LinkId,
        endpoint_id: EndpointId,
        chan_alloc: Option<CmceChanAllocReq>,
        write: impl FnOnce(&mut BitBuffer) -> Result<(), tetra_core::PduParseErr>,
    ) {
        self.send_l2(queue, addr, link_id, endpoint_id, MleProtocolDiscriminator::Mle, chan_alloc, None, None, write);
    }

    /// Sends a PDU of protocol `disc` to an MS. With `stch` it is stolen on that traffic channel
    /// if the MS is on it (layer 2 falls back to the MCCH when the channel no longer carries
    /// traffic).
    #[allow(clippy::too_many_arguments)]
    fn send_l2(
        &self,
        queue: &mut MessageQueue,
        addr: TetraAddress,
        link_id: LinkId,
        endpoint_id: EndpointId,
        disc: MleProtocolDiscriminator,
        chan_alloc: Option<CmceChanAllocReq>,
        stch: Option<(u16, u8)>,
        tx_reporter: Option<TxReporter>,
        write: impl FnOnce(&mut BitBuffer) -> Result<(), tetra_core::PduParseErr>,
    ) {
        let mut body = BitBuffer::new_autoexpand(32);
        body.write_bits(disc.into_raw(), 3);
        if let Err(e) = write(&mut body) {
            tracing::error!("MLE: failed to encode PDU for {}: {:?}", addr.ssi, e);
            return;
        }
        let len = body.get_pos();
        body.seek(0);
        let mut pdu = BitBuffer::new(len);
        pdu.copy_bits(&mut body, len);
        pdu.seek(0);
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: addr,
                link_id,
                endpoint_id,
                tl_sdu: pdu,
                stealing_permission: stch.is_some(),
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0,
                graceful_degradation: None,
                chan_alloc,
                stealing_slot: stch,
                tx_reporter,
            }),
        });
    }

    fn rx_tla_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tla_prim");
        match message.msg {
            SapMsgInner::TlaTlDataIndBl(_) => {
                self.rx_tla_data_ind_bl(queue, message);
            }
            SapMsgInner::TlaTlUnitdataIndBl(_) => {
                // self.rx_tla_unitdata_ind_bl(queue, message);
                tracing::warn!("MLE: BS received unexpected TL-UNITDATA, ignoring");
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    fn rx_tla_data_ind_bl(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // Take ownership of bitbuf and read protocol discriminator
        let SapMsgInner::TlaTlDataIndBl(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let Some(mut sdu) = prim.tl_sdu.take() else {
            tracing::warn!("MLE: rx_tla_data_ind_bl received message with no tl_sdu, ignoring");
            return;
        };
        if sdu.get_pos() != 0 {
            tracing::warn!(
                "MLE: rx_tla_data_ind_bl sdu not at start position (pos={}), seeking to 0",
                sdu.get_pos()
            );
            sdu.seek(0);
        }
        let Some(bits) = sdu.read_bits(3) else {
            tracing::warn!("insufficient bits: {}", sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = MleProtocolDiscriminator::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, sdu.dump_bin());
            return;
        };

        // Dispatch to appropriate component (or to self if for MLE)
        match pdu_type {
            MleProtocolDiscriminator::Mm => {
                let m = LmmMleUnitdataInd {
                    sdu,
                    handle: 0,
                    received_address: prim.main_address,
                };
                let msg = SapMsg {
                    sap: Sap::LmmSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::LmmMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Cmce => {
                let m = LcmcMleUnitdataInd {
                    sdu,
                    handle: 0,
                    received_tetra_address: prim.main_address,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    chan_change_resp_req: false, // TODO FIXME
                    chan_change_handle: None,    // TODO FIXME
                };
                let msg = SapMsg {
                    sap: Sap::LcmcSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Cmce,
                    msg: SapMsgInner::LcmcMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Sndcp => {
                let m = LtpdMleUnitdataInd {
                    sdu,
                    endpoint_id: prim.endpoint_id,
                    link_id: prim.link_id,
                    received_tetra_address: prim.main_address,
                    chan_change_resp_req: false, // TODO FIXME
                    chan_change_handle: None,    // TODO FIXME
                };
                // SNDCP (packet data, MLE protocol discriminator 4) belongs to the SNDCP entity,
                // not CMCE. Route it over the TLPD SAP so the packet-data layer receives it.
                let msg = SapMsg {
                    sap: Sap::TlpdSap,
                    src: TetraEntity::Mle,
                    dest: TetraEntity::Sndcp,
                    msg: SapMsgInner::LtpdMleUnitdataInd(m),
                };
                queue.push_back(msg);
            }
            MleProtocolDiscriminator::Mle => {
                let (addr, link_id, endpoint_id) = (prim.main_address, prim.link_id, prim.endpoint_id);
                self.rx_mle_pdu(queue, sdu, addr, link_id, endpoint_id);
            }
            MleProtocolDiscriminator::TetraManagementEntity => {
                unimplemented_log!("MleProtocolDiscriminator::TetraManagementEntity");
            }
        }
    }

    fn rx_tlmc_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmc_prim");
        // TLMC SAP not implemented yet. Log instead of panicking so an unexpected
        // primitive doesn't kill the whole MLE worker.
        unimplemented_log!("rx_tlmc_prim called but TLMC SAP is not implemented");
    }

    fn rx_lmm_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lmm_mle_unitdata_req");
        let SapMsgInner::LmmMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // MM's answer to a registration forwarded from a sibling cell: the MS is still over there,
        // so it goes back to that cell (inside D-NEW-CELL) instead of on air here.
        self.pending_forwards.retain(|_, at| at.elapsed() < FORWARD_REGISTRATION_TIMEOUT);
        if self.pending_forwards.remove(&prim.address.ssi).is_some() {
            Self::push_site(
                queue,
                CallControl::SiteForwardRegistrationResult {
                    issi: prim.address.ssi,
                    sdu: Some(prim.sdu.clone()),
                },
            );
            return;
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Mm;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        if prim.layer2service == Layer2Service::Unacknowledged {
            tracing::warn!("MLE: rx_lmm_mle_unitdata_req with Unacknowledged layer2service not implemented, ignoring");
            return;
        }

        // let (addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(addr.ssi, prim.address.ssi);
        let sapmsg = SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: prim.address,
                link_id: 0,
                endpoint_id: 0,
                tl_sdu: pdu,
                stealing_permission: false,
                subscriber_class: 0, // TODO fixme
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0, // TODO FIXME; should we pass the same handle here?
                graceful_degradation: None,
                chan_alloc: None,
                stealing_slot: None,
                tx_reporter: prim.tx_reporter.take(),
            }),
        };
        queue.push_back(sapmsg);
    }

    fn rx_lmm_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lmm_prim");
        match &message.msg {
            SapMsgInner::LmmMleUnitdataReq(_prim) => {
                self.rx_lmm_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_tlpd_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlpd_prim");
        unimplemented_log!("rx_tlpd_prim called but TLPD SAP is not implemented");
        // match &message.msg {
        //     _ => {
        //         panic!();
        //     }
        // }
    }

    fn rx_lcmc_mle_unitdata_req(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_req");
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Answer to a U-CALL RESTORE that arrived inside a U-RESTORE: return it in the MLE
        // restore PDU instead (D-CALL RESTORE → D-RESTORE-ACK, D-RELEASE → D-RESTORE-FAIL).
        let ssi = prim.main_address.ssi;
        self.pending_restores.retain(|_, at| at.elapsed() < RESTORE_ANSWER_TIMEOUT);
        if self.pending_restores.contains_key(&ssi) {
            let cmce_type = prim.sdu.peek_bits_startoffset(0, 5).and_then(|t| CmcePduTypeDl::try_from(t).ok());
            let (addr, link_id, endpoint_id) = (prim.main_address, prim.link_id, prim.endpoint_id);
            match cmce_type {
                Some(CmcePduTypeDl::DCallRestore) => {
                    self.pending_restores.remove(&ssi);
                    tracing::info!("MLE: D-RESTORE-ACK to {} (call restored)", ssi);
                    let ack = DRestoreAck {
                        sdu: Some(prim.sdu.clone()),
                    };
                    // CMCE's channel allocation for the restored call goes with the D-CALL RESTORE
                    // it carries (TS 100 392-2 clause 14.5.2.2.4).
                    let chan_alloc = prim.chan_alloc.take();
                    self.send_mle_pdu_with_chan_alloc(queue, addr, link_id, endpoint_id, chan_alloc, |b| ack.to_bitbuf(b));
                    return;
                }
                Some(CmcePduTypeDl::DRelease) => {
                    self.pending_restores.remove(&ssi);
                    tracing::info!("MLE: D-RESTORE-FAIL to {} (call could not be restored)", ssi);
                    let fail = DRestoreFail {
                        fail_cause: FAIL_CAUSE_RESTORATION_NOT_POSSIBLE,
                    };
                    self.send_mle_pdu(queue, addr, link_id, endpoint_id, |b| fail.to_bitbuf(b));
                    return;
                }
                _ => {}
            }
        }

        let mle_prot_discriminator = MleProtocolDiscriminator::Cmce;
        let sdu_len = prim.sdu.get_len();
        let mut pdu = BitBuffer::new(3 + sdu_len);
        pdu.write_bits(mle_prot_discriminator.into_raw(), 3);
        pdu.copy_bits(&mut prim.sdu, sdu_len);
        pdu.seek(0);

        // let (_addr, link, endpoint) = self.router.use_handle(prim.handle, message.dltime);
        // assert_eq!(link, prim.link_id);
        // assert_eq!(endpoint, prim.endpoint_id);
        // Take Channel Allocation Request if any
        let chan_alloc = prim.chan_alloc.take();

        let sapmsg = if prim.layer2service == Layer2Service::Unacknowledged {
            // Unacknowledged service, send a TlUnitdataReqBl
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Llc,
                msg: SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class: 0, // TODO fixme
                    fcs_flag: false,
                    air_interface_encryption: None,
                    packet_data_flag: false,
                    n_tlsdu_repeats: 0,
                    data_class_info: None,
                    req_handle: 0,

                    chan_alloc,
                    stealing_slot: None,
                    tx_reporter: prim.tx_reporter.take(),
                }),
            }
        } else {
            // Acknowledged service, send a TlDataReqBl
            SapMsg {
                sap: Sap::TlaSap,
                src: TetraEntity::Mle,
                dest: TetraEntity::Llc,
                msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                    main_address: prim.main_address,
                    link_id: prim.link_id,
                    endpoint_id: prim.endpoint_id,
                    tl_sdu: pdu,
                    stealing_permission: prim.stealing_permission,
                    subscriber_class: 0, // TODO fixme
                    fcs_flag: false,
                    air_interface_encryption: None,
                    stealing_repeats_flag: None,
                    data_class_info: None,
                    req_handle: 0, // TODO FIXME
                    graceful_degradation: None,
                    chan_alloc,
                    stealing_slot: None,
                    tx_reporter: prim.tx_reporter.take(),
                }),
            }
        };

        queue.push_back(sapmsg);
    }

    fn rx_lcmc_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_lcmc_prim");
        match &message.msg {
            SapMsgInner::LcmcMleUnitdataReq(_) => {
                self.rx_lcmc_mle_unitdata_req(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }
}

impl TetraEntityTrait for MleBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mle
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        let late: Vec<u32> = self
            .pending_prepares
            .iter()
            .filter(|(_, p)| p.since.elapsed() >= FORWARD_REGISTRATION_TIMEOUT)
            .map(|(issi, _)| *issi)
            .collect();
        for issi in late {
            tracing::info!("MLE: target cell did not answer {}'s U-PREPARE in time", issi);
            self.finish_prepare(queue, issi, HandoverOutcome::NotPrepared, None, None);
        }
        self.drive_type1(queue);

        // Broadcast D-NWRK-BROADCAST twice per hyperframe (~30.6s interval) if timezone is configured.
        // Two evenly-spaced slots [20, 50] avoid congestion with other hyperframe-triggered events
        // and give terminals a faster time/date update after cold attach.
        if MLE_BROADCAST_MULTIFRAMES.contains(&ts.m) && ts.f == MLE_BROADCAST_FRAME && ts.t == 1 {
            tracing::debug!("MLE: hyperframe broadcast slot (hf={} m={} f={} t={})", ts.h, ts.m, ts.f, ts.t);
            self.broadcast.send_broadcast(queue);
        }
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TlaSap => {
                self.rx_tla_prim(queue, message);
            }
            Sap::TlmbSap => {
                tracing::warn!("MLE: BS received unexpected broadcast message on TlmbSap, ignoring");
            }
            Sap::TlmcSap => {
                self.rx_tlmc_prim(queue, message);
            }
            Sap::LmmSap => {
                self.rx_lmm_prim(queue, message);
            }
            Sap::TlpdSap => {
                self.rx_tlpd_prim(queue, message);
            }
            Sap::LcmcSap => {
                self.rx_lcmc_prim(queue, message);
            }
            Sap::Control => {
                self.rx_control(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }
}

/// "Location update type" of a U-LOCATION UPDATE DEMAND (after its 4 bit PDU type, table 16.12).
fn location_update_type(demand: &BitBuffer) -> u8 {
    demand.peek_bits_startoffset(4, 3).unwrap_or(0) as u8
}

/// "Reject cause" of a D-LOCATION UPDATE REJECT (after PDU type and location update type, table 16.13).
fn reject_cause(reject: &BitBuffer) -> Option<u8> {
    reject.peek_bits_startoffset(7, 5).map(|c| c as u8)
}

/// D-LOCATION UPDATE REJECT "forward registration failure" for a forward registration the target
/// did not accept (clause 18.3.4.7.6; reject cause clause 16.10.42).
fn forward_registration_failure(location_update_type: u8) -> BitBuffer {
    let reject = DLocationUpdateReject {
        location_update_type: LocationUpdateType::try_from(location_update_type as u64).unwrap_or(LocationUpdateType::RoamingLocationUpdating),
        reject_cause: REJECT_CAUSE_FORWARD_REGISTRATION_FAILURE,
        cipher_control: false,
        ciphering_parameters: None,
        address_extension: None,
        cell_type_control: None,
        proprietary: None,
    };
    let mut body = BitBuffer::new_autoexpand(16);
    reject.to_bitbuf(&mut body).expect("D-LOCATION UPDATE REJECT without optional elements");
    let len = body.get_pos();
    body.seek(0);
    let mut sdu = BitBuffer::new(len);
    sdu.copy_bits(&mut body, len);
    sdu.seek(0);
    sdu
}

#[cfg(test)]
mod tests {
    use tetra_core::SsiType;
    use tetra_saps::lcmc::LcmcMleUnitdataReq;
    use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
    use tetra_saps::tla::TlaTlDataIndBl;

    use super::*;

    const MS: u32 = 2_260_001;

    fn config(site_linked: bool) -> SharedConfig {
        config_with(site_linked, "")
    }

    /// A single-cell station connected to Brew: its neighbour belongs to another station.
    fn brew_config() -> SharedConfig {
        config_with(
            false,
            r#"
[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"
"#,
        )
    }

    fn config_with(site_linked: bool, extra: &str) -> SharedConfig {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1521
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1525
location_area = 7
"#;
        let mut cfg = tetra_config::bluestation::parsing::from_toml_str(&format!("{toml}{extra}")).unwrap();
        cfg.site_linked = site_linked;
        if site_linked {
            // The neighbour on 1525 is a sibling cell of the same station.
            cfg.station_carriers = vec![1521, 1525];
        }
        SharedConfig::from_parts(cfg, None)
    }

    fn addr() -> TetraAddress {
        TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: MS,
        }
    }

    /// An uplink MLE PDU from the MS, as LLC delivers it.
    fn uplink_mle(write: impl FnOnce(&mut BitBuffer)) -> SapMsg {
        let mut b = BitBuffer::new_autoexpand(64);
        b.write_bits(MleProtocolDiscriminator::Mle.into_raw(), 3);
        write(&mut b);
        let len = b.get_pos();
        b.seek(0);
        let mut sdu = BitBuffer::new(len);
        sdu.copy_bits(&mut b, len);
        sdu.seek(0);
        SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
                main_address: addr(),
                link_id: 0,
                endpoint_id: 0,
                new_endpoint_id: None,
                css_endpoint_id: None,
                tl_sdu: Some(sdu),
                scrambling_code: 0,
                fcs_flag: false,
                air_interface_encryption: 0,
                chan_change_resp_req: false,
                chan_change_handle: None,
                chan_info: None,
                req_handle: 0,
            }),
        }
    }

    fn drain(q: &mut MessageQueue) -> Vec<SapMsg> {
        std::iter::from_fn(|| q.pop_front()).collect()
    }

    /// The MLE PDUs sent down to LLC, as bit strings (discriminator stripped).
    fn sent_mle_pdus(msgs: &[SapMsg]) -> Vec<String> {
        msgs.iter()
            .filter_map(|m| match &m.msg {
                SapMsgInner::TlaTlDataReqBl(r) => Some(r.tl_sdu.to_bitstr()),
                _ => None,
            })
            .filter_map(|s| s.strip_prefix("101").map(str::to_string))
            .collect()
    }

    #[test]
    fn u_prepare_to_advertised_neighbour_gets_d_new_cell() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        let prepare = UPrepare {
            cell_identifier_ca: Some(1),
            sdu: None,
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec!["000010".to_string()], "D-NEW-CELL, change channel immediately");
        assert!(
            msgs.iter().any(|m| m.dest == TetraEntity::Brew
                && matches!(
                    m.msg,
                    SapMsgInner::CmceCallControl(CallControl::SiteHandoverPrepare {
                        issi: MS,
                        target_carrier: 1525
                    })
                )),
            "site switch is told"
        );
    }

    #[test]
    fn u_prepare_to_unknown_cell_gets_d_prepare_fail() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let prepare = UPrepare {
            cell_identifier_ca: Some(9),
            sdu: None,
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec!["001000".to_string()], "D-PREPARE-FAIL");
        assert!(!msgs.iter().any(|m| m.dest == TetraEntity::Brew));
    }

    #[test]
    fn type_3_u_prepare_gets_d_new_cell_not_d_prepare_fail() {
        // No cell identifier = announced type 3; D-PREPARE-FAIL is not a valid answer
        // (clause 18.3.4.7.4). Same on a multi-cell station and on a Brew-linked one.
        for cfg in [config(true), brew_config()] {
            let mut mle = MleBs::new(cfg);
            let mut q = MessageQueue::new();
            let prepare = UPrepare {
                cell_identifier_ca: None,
                sdu: None,
            };
            mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
            let msgs = drain(&mut q);
            assert_eq!(sent_mle_pdus(&msgs), vec!["000010".to_string()], "D-NEW-CELL, change channel immediately");
            assert!(
                !told_brew(&msgs, |cc| !matches!(cc, CallControl::ReselectionAnnounced { .. })),
                "nothing to prepare"
            );
        }
    }

    #[test]
    fn every_d_new_cell_is_reported_to_brew_with_its_type() {
        let reported = |msgs: &[SapMsg]| {
            msgs.iter()
                .filter_map(|m| match &m.msg {
                    SapMsgInner::CmceCallControl(CallControl::ReselectionAnnounced { issi: MS, reselection }) if m.dest == TetraEntity::Brew => {
                        Some(*reselection)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let type3 = UPrepare {
            cell_identifier_ca: None,
            sdu: None,
        };
        // Type 3: D-NEW-CELL at once.
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        mle.rx_prim(&mut q, uplink_mle(|b| type3.to_bitbuf(b).unwrap()));
        assert_eq!(reported(&drain(&mut q)), vec![CellReselection::AnnouncedType3]);
        // Type 2 once the other station answered (accepted or not prepared), not before.
        for outcome in [HandoverOutcome::Accepted, HandoverOutcome::NotPrepared] {
            let msgs = cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
            assert!(reported(&msgs).is_empty());
            mle.rx_prim(&mut q, result(outcome, if outcome == HandoverOutcome::Accepted { Some(MM_ACCEPT) } else { None }));
            assert_eq!(reported(&drain(&mut q)), vec![CellReselection::AnnouncedType2]);
        }
        // D-PREPARE-FAIL is no reselection.
        for outcome in [HandoverOutcome::Rejected, HandoverOutcome::TargetGone] {
            cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
            mle.rx_prim(&mut q, result(outcome, (outcome == HandoverOutcome::Rejected).then_some(MM_REJECT)));
            assert!(reported(&drain(&mut q)).is_empty());
        }
        // A station without Brew has nobody to tell.
        for cfg in [config(false), config(true)] {
            let mut mle = MleBs::new(cfg);
            mle.rx_prim(&mut q, uplink_mle(|b| type3.to_bitbuf(b).unwrap()));
            assert!(reported(&drain(&mut q)).is_empty());
        }
    }

    fn cmce_answer(sdu_bits: &str) -> SapMsg {
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: BitBuffer::from_bitstr(sdu_bits),
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service: Layer2Service::Acknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                main_address: addr(),
                chan_alloc: None,
                tx_reporter: None,
            }),
        }
    }

    #[test]
    fn u_restore_goes_to_cmce_and_its_answer_comes_back_in_the_mle_pdu() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let restore = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(BitBuffer::from_bitstr("0101100000000000100")),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| restore.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        let to_cmce = msgs.iter().find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataInd(ind) if m.dest == TetraEntity::Cmce => Some(ind.sdu.to_bitstr()),
            _ => None,
        });
        assert_eq!(to_cmce.as_deref(), Some("0101100000000000100"), "U-CALL RESTORE handed to CMCE");

        // CMCE answers with D-CALL RESTORE (type 14 = 01110): wrapped in D-RESTORE-ACK, O-bit 0
        // (TS 100 392-2 Annex E, table E.23).
        mle.rx_prim(&mut q, cmce_answer("01110000000000010"));
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec!["1000".to_string() + "01110000000000010"]);

        // A later, unrelated CMCE PDU for the same MS goes out normally (discriminator CMCE).
        mle.rx_prim(&mut q, cmce_answer("01110000000000010"));
        let msgs = drain(&mut q);
        assert!(sent_mle_pdus(&msgs).is_empty());
    }

    #[test]
    fn u_restore_in_the_same_la_reaches_cmce() {
        // Table E.29: no MCC/MNC/LA, O-bit 0, the U-CALL RESTORE right after it.
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        mle.rx_prim(&mut q, uplink_mle(|b| b.write_bits(0b1000_0101_1000, 12)));
        let to_cmce = drain(&mut q).into_iter().find_map(|m| match m.msg {
            SapMsgInner::LcmcMleUnitdataInd(ind) if m.dest == TetraEntity::Cmce => Some(ind.sdu.to_bitstr()),
            _ => None,
        });
        assert_eq!(to_cmce.as_deref(), Some("01011000"));
    }

    #[test]
    fn d_restore_ack_carries_the_channel_allocation_of_the_restore() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let restore = URestore {
            mcc: None,
            mnc: None,
            la: Some(2),
            sdu: Some(BitBuffer::from_bitstr("01011")),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| restore.to_bitbuf(b).unwrap()));
        drain(&mut q);
        let mut answer = cmce_answer("01110000000000010");
        if let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut answer.msg {
            prim.chan_alloc = Some(CmceChanAllocReq {
                usage: Some(5),
                alloc_type: ChanAllocType::Replace,
                carrier: Some(1530),
                timeslots: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                cell_change: false,
            });
        }
        mle.rx_prim(&mut q, answer);
        let chan = drain(&mut q).into_iter().find_map(|m| match m.msg {
            SapMsgInner::TlaTlDataReqBl(r) => r.chan_alloc,
            _ => None,
        });
        let chan = chan.expect("D-RESTORE-ACK goes out with the channel allocation (clause 14.5.2.2.4)");
        assert_eq!((chan.carrier, chan.timeslots, chan.usage), (Some(1530), [false, true, false, false], Some(5)));
    }

    #[test]
    fn failed_call_restore_becomes_d_restore_fail() {
        let mut mle = MleBs::new(config(false));
        let mut q = MessageQueue::new();
        let restore = URestore {
            mcc: None,
            mnc: None,
            la: None,
            sdu: Some(BitBuffer::from_bitstr("01011")),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| restore.to_bitbuf(b).unwrap()));
        drain(&mut q);
        // CMCE rejects with D-RELEASE (type 6 = 00110).
        mle.rx_prim(&mut q, cmce_answer("001100000000000"));
        // D-RESTORE-FAIL, fail cause "restoration cannot be done on cell" (clause 18.3.4.7).
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec!["101110".to_string()]);
    }

    // ── Forward registration (type 1 announced reselection) ─────────────────────────────────

    /// U-LOCATION UPDATE DEMAND-ish MM bits carried in U-PREPARE (content is opaque to MLE).
    const MM_DEMAND: &str = "0010000000000011";
    /// D-LOCATION UPDATE ACCEPT (type 5) / REJECT (type 7) answers from the target cell's MM.
    const MM_ACCEPT: &str = "0101000000001";
    const MM_REJECT: &str = "0111000000001";

    fn control(cc: CallControl) -> SapMsg {
        SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::CmceCallControl(cc),
        }
    }

    fn prepare_with_registration(mle: &mut MleBs, q: &mut MessageQueue) -> Vec<SapMsg> {
        let prepare = UPrepare {
            cell_identifier_ca: Some(1),
            sdu: Some(BitBuffer::from_bitstr(MM_DEMAND)),
        };
        mle.rx_prim(q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        drain(q)
    }

    #[test]
    fn forwarded_registration_holds_d_new_cell_until_the_target_answers() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        let msgs = prepare_with_registration(&mut mle, &mut q);
        assert!(sent_mle_pdus(&msgs).is_empty(), "D-NEW-CELL waits for the target cell");
        assert!(msgs.iter().any(|m| matches!(&m.msg,
            SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistration { issi: MS, target_carrier: 1525, sdu })
                if sdu.to_bitstr() == MM_DEMAND)));

        mle.rx_prim(
            &mut q,
            control(CallControl::SiteForwardRegistrationResult {
                issi: MS,
                sdu: Some(BitBuffer::from_bitstr(MM_ACCEPT)),
            }),
        );
        assert_eq!(
            sent_mle_pdus(&drain(&mut q)),
            vec![format!("000{}{}{}", "01", "0", MM_ACCEPT)],
            "D-NEW-CELL carrying the target cell's D-LOCATION UPDATE ACCEPT"
        );
    }

    #[test]
    fn rejected_forwarded_registration_becomes_d_prepare_fail() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        prepare_with_registration(&mut mle, &mut q);
        mle.rx_prim(
            &mut q,
            control(CallControl::SiteForwardRegistrationResult {
                issi: MS,
                sdu: Some(BitBuffer::from_bitstr(MM_REJECT)),
            }),
        );
        assert_eq!(
            sent_mle_pdus(&drain(&mut q)),
            vec![format!("001100{}", MM_REJECT)],
            "fail cause 2 'MS not allowed on cell' for a reject cause other than forward registration failure (18.3.4.7.6), O-bit 0 (table E.22)"
        );
    }

    #[test]
    fn unanswered_forwarded_registration_times_out_to_d_new_cell_with_forward_registration_failure() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        prepare_with_registration(&mut mle, &mut q);
        for p in mle.pending_prepares.values_mut() {
            p.since = Instant::now() - FORWARD_REGISTRATION_TIMEOUT;
        }
        mle.tick_start(&mut q, TdmaTime::default());
        assert_eq!(
            sent_mle_pdus(&drain(&mut q)),
            vec![format!("000010{}", FRF_REJECT)],
            "type 2 with failed forward registration (figure 18.12): D-NEW-CELL carries a reject"
        );
    }

    /// D-LOCATION UPDATE REJECT (0111), roaming location updating (000, as in MM_DEMAND),
    /// forward registration failure (00111), cipher control 0, no optional elements (O-bit 0).
    const FRF_REJECT: &str = "01110000011100";

    #[test]
    fn forward_registration_failure_reject_has_the_table_16_13_layout() {
        assert_eq!(forward_registration_failure(0).to_bitstr(), FRF_REJECT);
        assert_eq!(&forward_registration_failure(6).to_bitstr()[4..7], "110", "repeats the demand's type");
        assert_eq!(reject_cause(&BitBuffer::from_bitstr(FRF_REJECT)), Some(REJECT_CAUSE_FORWARD_REGISTRATION_FAILURE));
        assert_eq!(location_update_type(&BitBuffer::from_bitstr("0010110000000011")), 6);
    }

    #[test]
    fn unknown_neighbour_with_forward_registration_gets_a_reject_in_d_prepare_fail() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        let prepare = UPrepare {
            cell_identifier_ca: Some(9),
            sdu: Some(BitBuffer::from_bitstr(MM_DEMAND)),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec![format!("001000{}", FRF_REJECT)]);
    }

    // ── Neighbour on another station, prepared through Brew ─────────────────────────────────

    fn cross_station_prepare(mle: &mut MleBs, q: &mut MessageQueue, sdu: Option<&str>) -> Vec<SapMsg> {
        let prepare = UPrepare {
            cell_identifier_ca: Some(1),
            sdu: sdu.map(BitBuffer::from_bitstr),
        };
        mle.rx_prim(q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        drain(q)
    }

    fn result(outcome: HandoverOutcome, sdu: Option<&str>) -> SapMsg {
        control(CallControl::HandoverResult {
            issi: MS,
            outcome,
            sdu: sdu.map(BitBuffer::from_bitstr),
            channel: None,
        })
    }

    fn told_brew(msgs: &[SapMsg], f: impl Fn(&CallControl) -> bool) -> bool {
        msgs.iter()
            .any(|m| m.dest == TetraEntity::Brew && matches!(&m.msg, SapMsgInner::CmceCallControl(cc) if f(cc)))
    }

    #[test]
    fn u_prepare_to_another_station_asks_brew_and_holds_d_new_cell() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        let msgs = cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
        assert!(sent_mle_pdus(&msgs).is_empty(), "D-NEW-CELL waits for the other station");
        let own_plan = tetra_config::bluestation::carrier_band_plan(&mle.config.config().cell).unwrap();
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverRequest {
            issi: MS, target_carrier: 1525, target_carrier_ext, target_mcc: None, target_mnc: None,
            target_location_area: Some(7), sdu: Some(sdu) } if *target_carrier_ext == own_plan && sdu.to_bitstr() == MM_DEMAND)));
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::SiteHandoverPrepare { .. } | CallControl::SiteForwardRegistration { .. })));

        mle.rx_prim(&mut q, result(HandoverOutcome::Accepted, Some(MM_ACCEPT)));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec![format!("000010{}", MM_ACCEPT)]);
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverDone { issi: MS })));
    }

    #[test]
    fn u_prepare_from_a_multi_cell_station_to_another_station_asks_brew() {
        // Cell 1525 of a linked two-cell station; its neighbour 1521 is the sibling, 1600 is a
        // cell of another station.
        let mut cfg = tetra_config::bluestation::parsing::from_toml_str(&format!(
            "{}{}",
            r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1525
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1600
location_area = 7

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 2
cell_reselection_types_supported = 1
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1521
"#,
            r#"
[brew]
host = "brew.example"
port = 3000
tls = false
username = 1
password = "x"
"#
        ))
        .unwrap();
        cfg.site_linked = true;
        cfg.station_carriers = vec![1521, 1525];
        let mut mle = MleBs::new(SharedConfig::from_parts(cfg, None));
        let mut q = MessageQueue::new();

        let msgs = cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
        assert!(sent_mle_pdus(&msgs).is_empty(), "D-NEW-CELL waits for the other station");
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverRequest { issi: MS, target_carrier: 1600, sdu: Some(_), .. })));
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::SiteHandoverPrepare { .. } | CallControl::SiteForwardRegistration { .. })));
        mle.rx_prim(&mut q, result(HandoverOutcome::Accepted, Some(MM_ACCEPT)));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec![format!("000010{}", MM_ACCEPT)]);
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverDone { issi: MS })));

        // The sibling stays with the site switch.
        let prepare = UPrepare {
            cell_identifier_ca: Some(2),
            sdu: Some(BitBuffer::from_bitstr(MM_DEMAND)),
        };
        mle.rx_prim(&mut q, uplink_mle(|b| prepare.to_bitbuf(b).unwrap()));
        let msgs = drain(&mut q);
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::SiteForwardRegistration { target_carrier: 1521, .. })));
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverRequest { .. })));
    }

    #[test]
    fn u_prepare_without_registration_to_another_station_is_prepared_too() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        let msgs = cross_station_prepare(&mut mle, &mut q, None);
        assert!(sent_mle_pdus(&msgs).is_empty());
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverRequest { sdu: None, .. })));
        mle.rx_prim(&mut q, result(HandoverOutcome::Accepted, None));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec!["000010".to_string()]);
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverDone { .. })), "nothing moved: no registration");
    }

    #[test]
    fn rejected_by_the_other_station_maps_the_reject_cause_to_the_fail_cause() {
        // Reject cause "forward registration failure" → fail cause 0, any other → 2 (18.3.4.7.6).
        for (reject, fail_cause) in [(FRF_REJECT, "00"), (MM_REJECT, "10")] {
            let mut mle = MleBs::new(brew_config());
            let mut q = MessageQueue::new();
            cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
            mle.rx_prim(&mut q, result(HandoverOutcome::Rejected, Some(reject)));
            let msgs = drain(&mut q);
            assert_eq!(sent_mle_pdus(&msgs), vec![format!("001{fail_cause}0{reject}")]);
            assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverFailed { issi: MS })));
        }
    }

    #[test]
    fn other_station_gone_becomes_d_prepare_fail_temporary_break() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
        mle.rx_prim(&mut q, result(HandoverOutcome::TargetGone, None));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec![format!("001000{}", FRF_REJECT)]);
        assert!(told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverFailed { issi: MS })));
    }

    #[test]
    fn other_station_not_prepared_sends_the_ms_anyway_with_forward_registration_failure() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
        mle.rx_prim(&mut q, result(HandoverOutcome::NotPrepared, None));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec![format!("000010{}", FRF_REJECT)]);
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverDone { .. } | CallControl::HandoverFailed { .. })));

        // Without a forwarded registration a plain D-NEW-CELL, as before.
        cross_station_prepare(&mut mle, &mut q, None);
        mle.rx_prim(&mut q, result(HandoverOutcome::NotPrepared, None));
        assert_eq!(sent_mle_pdus(&drain(&mut q)), vec!["000010".to_string()]);
    }

    #[test]
    fn an_accept_without_the_target_mms_answer_is_not_an_accept() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        cross_station_prepare(&mut mle, &mut q, Some(MM_DEMAND));
        mle.rx_prim(&mut q, result(HandoverOutcome::Accepted, None));
        let msgs = drain(&mut q);
        assert_eq!(sent_mle_pdus(&msgs), vec![format!("000010{}", FRF_REJECT)]);
        assert!(!told_brew(&msgs, |cc| matches!(cc, CallControl::HandoverDone { .. })));
    }

    #[test]
    fn a_result_nobody_waits_for_sends_nothing() {
        let mut mle = MleBs::new(brew_config());
        let mut q = MessageQueue::new();
        mle.rx_prim(&mut q, result(HandoverOutcome::Accepted, Some(MM_ACCEPT)));
        assert!(drain(&mut q).is_empty());
    }

    #[test]
    fn target_cell_registers_the_ms_and_returns_mms_answer_instead_of_transmitting_it() {
        let mut mle = MleBs::new(config(true));
        let mut q = MessageQueue::new();
        mle.rx_prim(
            &mut q,
            control(CallControl::SiteForwardRegistration {
                issi: MS,
                target_carrier: 1521,
                sdu: BitBuffer::from_bitstr(MM_DEMAND),
            }),
        );
        let msgs = drain(&mut q);
        assert!(msgs.iter().any(|m| m.dest == TetraEntity::Mm
            && matches!(&m.msg, SapMsgInner::LmmMleUnitdataInd(ind) if ind.sdu.to_bitstr() == MM_DEMAND && ind.received_address.ssi == MS)));

        let mm_answer = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(tetra_saps::lmm::LmmMleUnitdataReq {
                sdu: BitBuffer::from_bitstr(MM_ACCEPT),
                handle: 0,
                address: addr(),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        mle.rx_prim(&mut q, mm_answer);
        let msgs = drain(&mut q);
        assert!(!msgs.iter().any(|m| m.dest == TetraEntity::Llc), "nothing on air here: the MS is on the other cell");
        assert!(msgs.iter().any(|m| matches!(&m.msg,
            SapMsgInner::CmceCallControl(CallControl::SiteForwardRegistrationResult { issi: MS, sdu: Some(s) }) if s.to_bitstr() == MM_ACCEPT)));
    }
}
