use std::collections::HashMap;

use tetra_config::bluestation::SharedConfig;
use tetra_core::freqs::FreqInfo;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Direction, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, Todo, unimplemented_log};
use tetra_pdus::mle::fields::bs_service_details::BsServiceDetails;
use tetra_pdus::mle::pdus::d_mle_sync::DMleSync;
use tetra_pdus::mle::pdus::d_mle_sysinfo::DMleSysinfo;
use tetra_pdus::umac::enums::mac_pdu_type::MacPduType;
use tetra_pdus::umac::enums::sysinfo_opt_field_flag::SysinfoOptFieldFlag;
use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
use tetra_pdus::umac::fields::sysinfo_default_def_for_access_code_a::SysinfoDefaultDefForAccessCodeA;
use tetra_pdus::umac::fields::sysinfo_ext_services::SysinfoExtendedServices;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_data::MacData;
use tetra_pdus::umac::pdus::mac_end_hu::MacEndHu;
use tetra_pdus::umac::pdus::mac_end_ul::MacEndUl;
use tetra_pdus::umac::pdus::mac_frag_ul::MacFragUl;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_sync::MacSync;
use tetra_pdus::umac::pdus::mac_sysinfo::MacSysinfo;
use tetra_pdus::umac::pdus::mac_u_blck::MacUBlck;
use tetra_pdus::umac::pdus::mac_u_signal::MacUSignal;
use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::tma::{TmaReport, TmaReportInd, TmaUnitdataInd};
use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
use tetra_saps::tmv::{TmvConfigureReq, TmvUnitdataReqSlots};
use tetra_saps::{SapMsg, SapMsgInner};

use crate::lmac::components::scrambler;
use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use crate::umac::subcomp::bs_frag::BsFragger;
use crate::umac::subcomp::bs_sched::{BsChannelScheduler, CarrierDownlinkMode, PrecomputedUmacPdus, TCH_S_CAP};
use crate::umac::subcomp::fillbits;
use crate::{MessagePrio, MessageQueue, TetraEntityTrait};

use super::subcomp::bs_defrag::BsDefrag;

pub struct UmacBs {
    self_component: TetraEntity,
    config: SharedConfig,
    dltime: TdmaTime,
    system_wide_services: bool,

    /// This MAC's endpoint ID, for addressing by the higher layers
    /// When using only a single base radio, we can set this to a fixed value
    endpoint_id: u32,

    /// Subcomponents
    defrag: BsDefrag,
    /// Pending STCH MAC-DATA spanning block1+block2 (length_ind=0b111110), keyed by timeslot.
    pending_stch: Option<PendingStch>,
    // event_label_store: EventLabelStore,
    /// Contains UL/DL scheduling logic
    /// Access to this field is used only by testing code
    pub channel_scheduler: BsChannelScheduler,
    secondary_channel_schedulers: Vec<BsChannelScheduler>,
    /// Timestamp of last received UL voice frame per carrier/timeslot.
    /// Used to detect UL inactivity when a radio disappears mid-transmission.
    last_ul_voice: HashMap<(u16, u8), TdmaTime>,
    /// Local floor owner per traffic carrier/timeslot, used to attribute MAC-U-SIGNAL
    /// uplink signalling that does not carry an address field.
    ul_signal_owner: HashMap<(u16, u8), u32>,
    /// Traffic-slot circuit closes are delayed until queued FACCH/STCH signalling
    /// has had a scheduler turn to leave the BS.
    pending_circuit_closes: HashMap<(u16, u8), PendingCircuitClose>,
    telemetry: Option<TelemetrySink>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PendingCircuitClose {
    dl: bool,
    ul: bool,
}

struct PendingStch {
    addr: TetraAddress,
    scrambling_code: u32,
    encrypted: bool,
    fill_bits: bool,
    sdu_part: BitBuffer,
}

impl UmacBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>) -> Self {
        let c = config.config();
        let scrambling_code = scrambler::tetra_scramb_get_init(c.net.mcc, c.net.mnc, c.cell.colour_code);
        let system_wide_services = Self::get_system_wide_services_state(&config);
        let precomps = Self::generate_precomps(&config);
        let mut secondary_channel_schedulers = Vec::new();
        if let Some(secondary_carrier) = c.cell.secondary_carrier {
            let mut sched = BsChannelScheduler::new(scrambling_code, precomps.clone());
            sched.set_carrier_num(secondary_carrier);
            sched.set_downlink_mode(CarrierDownlinkMode::SecondaryBcchNoMcch);
            secondary_channel_schedulers.push(sched);
        }
        Self {
            self_component: TetraEntity::Umac,
            config,
            dltime: TdmaTime::default(),
            system_wide_services,
            endpoint_id: 1,
            defrag: BsDefrag::new(),
            pending_stch: None,
            // event_label_store: EventLabelStore::new(),
            channel_scheduler: BsChannelScheduler::new(scrambling_code, precomps),
            secondary_channel_schedulers,
            last_ul_voice: HashMap::new(),
            ul_signal_owner: HashMap::new(),
            pending_circuit_closes: HashMap::new(),
            telemetry,
        }
    }

    fn main_carrier(&self) -> u16 {
        self.config.config().cell.main_carrier
    }

    fn configured_carriers(&self) -> Vec<u16> {
        let cfg = self.config.config();
        [Some(cfg.cell.main_carrier), cfg.cell.secondary_carrier]
            .into_iter()
            .flatten()
            .collect()
    }

    fn scheduler_for_mut(&mut self, carrier_num: u16) -> &mut BsChannelScheduler {
        if carrier_num == self.main_carrier() {
            return &mut self.channel_scheduler;
        }
        if let Some(index) = self
            .secondary_channel_schedulers
            .iter()
            .position(|sched| sched.carrier_num() == carrier_num)
        {
            return &mut self.secondary_channel_schedulers[index];
        }
        // Should be unreachable with valid config (carrier_num always originates from a
        // configured carrier). Degrade to the primary scheduler instead of crashing UMAC live.
        tracing::error!("UMAC: unknown carrier {}, no scheduler configured -- falling back to primary", carrier_num);
        &mut self.channel_scheduler
    }

    fn scheduler_for(&self, carrier_num: u16) -> &BsChannelScheduler {
        if carrier_num == self.main_carrier() {
            return &self.channel_scheduler;
        }
        match self
            .secondary_channel_schedulers
            .iter()
            .find(|sched| sched.carrier_num() == carrier_num)
        {
            Some(sched) => sched,
            None => {
                // Should be unreachable with valid config; degrade to primary rather than panic.
                tracing::error!("UMAC: unknown carrier {}, no scheduler configured -- falling back to primary", carrier_num);
                &self.channel_scheduler
            }
        }
    }

    /// Precomputes SYNC, SYSINFO messages (and subfield variants) for faster TX msg building
    /// Precomputed PDUs are passed to scheduler
    /// Needs to be re-invoked if any network parameter changes
    pub fn generate_precomps(config: &SharedConfig) -> PrecomputedUmacPdus {
        let c = config.config();

        // TODO FIXME make more/all parameters configurable
        let ext_services = SysinfoExtendedServices {
            auth_required: false,
            class1_supported: true,
            class2_supported: true,
            class3_supported: false,
            sck_n: Some(0),
            dck_retrieval_during_cell_select: None,
            dck_retrieval_during_cell_reselect: None,
            linked_gck_crypto_periods: None,
            short_gck_vn: None,
            sdstl_addressing_method: 2,
            gck_supported: false,
            section: 0,
            section_data: 0,
        };

        let def_access = SysinfoDefaultDefForAccessCodeA {
            imm: 8,
            wt: 5,
            nu: 5,
            fl_factor: false,
            ts_ptr: 0,
            min_pdu_prio: 0,
        };

        let sysinfo1 = MacSysinfo {
            main_carrier: c.cell.main_carrier,
            freq_band: c.cell.freq_band,
            freq_offset_index: FreqInfo::freq_offset_hz_to_id(c.cell.freq_offset_hz)
                .unwrap_or_else(|| panic!(
                    "Invalid [cell] freq_offset_hz = {} Hz. TETRA only allows 0, +6250, -6250, or +12500 Hz (ETSI freq offset IDs). Fix the config.",
                    c.cell.freq_offset_hz
                )),
            duplex_spacing: c.cell.duplex_spacing_id,
            reverse_operation: c.cell.reverse_operation,
            num_of_csch: 0, // Common secondary control channels
            ms_txpwr_max_cell: c.cell.ms_txpwr_max_cell,
            rxlev_access_min: 3, // -110 dBm (permissive, suitable for single-cell)
            access_parameter: 7, // -39 dBm (MS open-loop power control setpoint)
            radio_dl_timeout: 3, // 432 timeslots (~6s radio link timeout)
            cck_id: None,
            hyperframe_number: Some(0), // Updated dynamically in scheduler
            option_field: SysinfoOptFieldFlag::DefaultDefForAccCodeA,
            ts_common_frames: None,
            default_access_code: Some(def_access),
            ext_services: None,
        };

        let sysinfo2 = MacSysinfo {
            main_carrier: sysinfo1.main_carrier,
            freq_band: sysinfo1.freq_band,
            freq_offset_index: sysinfo1.freq_offset_index,
            duplex_spacing: sysinfo1.duplex_spacing,
            reverse_operation: sysinfo1.reverse_operation,
            num_of_csch: sysinfo1.num_of_csch,
            ms_txpwr_max_cell: sysinfo1.ms_txpwr_max_cell,
            rxlev_access_min: sysinfo1.rxlev_access_min,
            access_parameter: sysinfo1.access_parameter,
            radio_dl_timeout: sysinfo1.radio_dl_timeout,
            cck_id: None,
            hyperframe_number: Some(0), // Updated dynamically in scheduler
            option_field: SysinfoOptFieldFlag::ExtServicesBroadcast,
            ts_common_frames: None,
            default_access_code: None,
            ext_services: Some(ext_services),
        };

        let system_wide_services = Self::get_system_wide_services_state(config);
        let mle_sysinfo_pdu = DMleSysinfo {
            location_area: c.cell.location_area,
            subscriber_class: c.cell.subscriber_class,
            bs_service_details: BsServiceDetails {
                registration: c.cell.registration,
                deregistration: c.cell.deregistration,
                priority_cell: c.cell.priority_cell,
                no_minimum_mode: c.cell.no_minimum_mode,
                migration: c.cell.migration,
                system_wide_services,
                voice_service: c.cell.voice_service,
                circuit_mode_data_service: c.cell.circuit_mode_data_service,
                sndcp_service: c.cell.sndcp_service,
                aie_service: c.cell.aie_service,
                advanced_link: c.cell.advanced_link,
            },
        };

        // Confirm in the log what packet-data-relevant capabilities the cell advertises in SYSINFO.
        // A radio that wants advanced-link packet data (class_of_ms.original_advanced_link=1) needs
        // BOTH sndcp_service AND advanced_link advertised, or it won't start the SNDCP procedure.
        tracing::info!(
            "SYSINFO advertising: sndcp_service={} advanced_link={} voice_service={} circuit_mode_data={}",
            c.cell.sndcp_service,
            c.cell.advanced_link,
            c.cell.voice_service,
            c.cell.circuit_mode_data_service
        );

        let mac_sync_pdu = MacSync {
            system_code: c.cell.system_code,
            colour_code: c.cell.colour_code,
            time: TdmaTime::default(), // replaced dynamically in scheduler
            sharing_mode: c.cell.sharing_mode,
            ts_reserved_frames: c.cell.ts_reserved_frames,
            u_plane_dtx: c.cell.u_plane_dtx,
            frame_18_ext: c.cell.frame_18_ext,
        };

        let mle_sync_pdu = DMleSync {
            mcc: c.net.mcc,
            mnc: c.net.mnc,
            // Per ETSI EN 300 392-2 Table 18.17:
            // 0 = no broadcast, 1 = broadcast+enquiry, 2 = broadcast only, 3 = reserved
            // Hardcoded to 2 (broadcast only) to match BlueStation v0.5.9 behavior —
            // required for Motorola terminals (MXP600, MTM800E, MTM5400) to accept
            // and display network time/date received via D-NWRK-BROADCAST.
            // Driving this from config (c.cell.neighbor_cell_broadcast) caused a
            // regression where missing config field → unwrap_or(0) → terminals ignore broadcast.
            neighbor_cell_broadcast: 2,
            cell_load_ca: 0, // TODO implement dynamic setting. 0 = info unavailable
            late_entry_supported: c.cell.late_entry_supported,
        };

        PrecomputedUmacPdus {
            mac_sysinfo1: sysinfo1,
            mac_sysinfo2: sysinfo2,
            mle_sysinfo: mle_sysinfo_pdu,
            mac_sync: mac_sync_pdu,
            mle_sync: mle_sync_pdu,
        }
    }

    /// Retrieve currently set value of system-wide services. If SwMI is active, this governs connection state
    /// Otherwise, value from config is used.
    fn get_system_wide_services_state(config: &SharedConfig) -> bool {
        let cfg = config.config();
        if cfg.brew.is_some() {
            config.state_read().network_connected
        } else {
            cfg.cell.system_wide_services
        }
    }

    fn refresh_system_wide_services(&mut self) {
        let is_effective = Self::get_system_wide_services_state(&self.config);
        if is_effective != self.system_wide_services {
            self.system_wide_services = is_effective;
            self.channel_scheduler.set_system_wide_services_state(is_effective);
            for scheduler in &mut self.secondary_channel_schedulers {
                scheduler.set_system_wide_services_state(is_effective);
            }

            // Should already be signalled at SwMI interface level
            tracing::debug!("UmacBs: system_wide_services {}", if is_effective { "ENABLED" } else { "DISABLED" });
        }
    }

    fn validate_chan_alloc(&self, chan_alloc: &CmceChanAllocReq, carrier_num: u16) -> bool {
        if chan_alloc.alloc_type == ChanAllocType::ReplaceWithCarrierSignalling {
            tracing::warn!(
                "UmacBs: rejecting channel allocation with CSS semantics for ordinary traffic carrier={} ts_bitmap={:?}",
                carrier_num,
                chan_alloc.timeslots
            );
            return false;
        }

        if carrier_num == self.main_carrier() && chan_alloc.timeslots[0] {
            tracing::warn!(
                "UmacBs: rejecting ordinary traffic allocation on main carrier TS1 carrier={} ts_bitmap={:?}",
                carrier_num,
                chan_alloc.timeslots
            );
            return false;
        }

        true
    }

    fn cmce_to_mac_chanalloc(&self, chan_alloc: &CmceChanAllocReq, carrier_num: u16) -> Option<ChanAllocElement> {
        if !self.validate_chan_alloc(chan_alloc, carrier_num) {
            return None;
        }

        // We grant clch permission for Replace and Additional allocations on the uplink
        let clch_permission = (chan_alloc.alloc_type == ChanAllocType::Replace || chan_alloc.alloc_type == ChanAllocType::Additional)
            && (chan_alloc.ul_dl_assigned == UlDlAssignment::Ul || chan_alloc.ul_dl_assigned == UlDlAssignment::Both);
        Some(ChanAllocElement {
            alloc_type: chan_alloc.alloc_type,
            ts_assigned: chan_alloc.timeslots,
            ul_dl_assigned: chan_alloc.ul_dl_assigned,
            clch_permission,
            cell_change_flag: false,
            carrier_num,
            ext: None,
            mon_pattern: 0,
            frame18_mon_pattern: Some(0),
        })
    }

    /// Convenience function to send a TMA-REPORT.ind
    fn send_tma_report_ind(queue: &mut MessageQueue, handle: Todo, report: TmaReport) {
        let tma_report_ind = TmaReportInd {
            req_handle: handle,
            report,
        };
        let msg = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaReportInd(tma_report_ind),
        };
        queue.push_back(msg);
    }

    fn rx_tmv_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tmv_prim");
        match message.msg {
            SapMsgInner::TmvUnitdataInd(_) => {
                self.rx_tmv_unitdata_ind(queue, message);
            }
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        }
    }

    pub fn rx_tmv_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        tracing::trace!("rx_tmv_unitdata_ind: {:?}", prim.logical_channel);

        match prim.logical_channel {
            LogicalChannel::SchF => {
                // Full slot signalling — must be a full block. A mismatched block_num
                // would indicate a PHY/LMAC routing problem; drop and log instead of
                // asserting so a single odd block can't take down the cell.
                if prim.block_num != PhyBlockNum::Both {
                    tracing::warn!(
                        "rx_tmv_unitdata_ind: {:?} with unexpected block_num {:?}, dropping",
                        prim.logical_channel,
                        prim.block_num
                    );
                    return;
                }
                self.rx_tmv_sch(queue, message);
            }
            LogicalChannel::Stch | LogicalChannel::SchHu => {
                // Half slot signalling
                if !matches!(prim.block_num, PhyBlockNum::Block1 | PhyBlockNum::Block2) {
                    tracing::warn!(
                        "rx_tmv_unitdata_ind: {:?} with unexpected block_num {:?}, dropping",
                        prim.logical_channel,
                        prim.block_num
                    );
                    return;
                }
                self.rx_tmv_sch(queue, message);
            }
            // Any other logical channel reaching here is a routing error. Log and drop
            // rather than unreachable!()-panicking on wire-derived data.
            other => {
                tracing::warn!("rx_tmv_unitdata_ind: unhandled logical channel {:?}, dropping", other);
            }
        }
    }

    /// Receive signalling (SCH, or STCH / BNCH)
    pub fn rx_tmv_sch(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_tmv_sch");

        // Iterate until no more messages left in mac block
        loop {
            // let (lchan, block_num) = match &message.msg {
            //     SapMsgInner::TmvUnitdataInd(prim) => (prim.logical_channel, prim.block_num),
            //     _ => { tracing::warn!("unhandled match variant, ignoring"); }
            // };

            // Handle STCH MAC-DATA spanning block1+block2 (length_ind=0b111110)
            // if lchan == LogicalChannel::Stch {
            //     if block_num == PhyBlockNum::Block2 {
            //         if let Some(pending) = self.pending_stch.take() {
            //             self.rx_stch_second_half(queue, &mut message, pending);
            //             break;
            //         }
            //     } else if self.pending_stch.is_some() {
            //         tracing::warn!(
            //             "rx_tmv_sch: pending STCH second-half but got {:?} on ts {}",
            //             block_num,
            //             message.dltime.t
            //         );
            //         self.pending_stch = None;
            //     }
            // }

            // Extract info from inner block
            let SapMsgInner::TmvUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            };
            let Some(bits) = prim.pdu.peek_bits(3) else {
                tracing::warn!("insufficient bits: {}", prim.pdu.dump_bin());
                return;
            };
            let orig_start = prim.pdu.get_raw_start();
            let lchan = prim.logical_channel;

            // Clause 21.4.1; handling differs between SCH_HU and others
            match lchan {
                LogicalChannel::SchF | LogicalChannel::Stch => {
                    // First two bits are MAC PDU type
                    let Ok(pdu_type) = MacPduType::try_from(bits >> 1) else {
                        tracing::warn!("invalid pdu type: {}", bits >> 1);
                        return;
                    };

                    match pdu_type {
                        MacPduType::MacResourceMacData => {
                            self.rx_mac_data(queue, &mut message);
                        }
                        MacPduType::MacFragMacEnd => {
                            // Also need third bit; designates mac-frag versus mac-end
                            if bits & 1 == 0 {
                                self.rx_mac_frag_ul(queue, &mut message);
                            } else {
                                self.rx_mac_end_ul(queue, &mut message);
                            }
                        }
                        MacPduType::SuppMacUSignal => {
                            // STCH determines which subtype is relevant
                            if lchan == LogicalChannel::Stch {
                                self.rx_ul_mac_u_signal(queue, &mut message);
                            } else {
                                // Supplementary MAC PDU type
                                if bits & 1 == 0 {
                                    self.rx_ul_mac_u_blck(queue, &mut message);
                                } else {
                                    tracing::warn!("unexpected supplementary PDU type")
                                }
                            }
                        }
                        _ => {
                            tracing::warn!("unknown pdu type: {}", pdu_type);
                        }
                    }
                }
                LogicalChannel::SchHu => {
                    // Need only 1 bit for a single subtype distinction
                    let pdu_type = (bits >> 2) & 1;
                    match pdu_type {
                        0 => self.rx_mac_access(queue, &mut message),
                        1 => self.rx_mac_end_hu(queue, &mut message),
                        _ => {
                            tracing::warn!("unhandled match variant, ignoring");
                        }
                    }
                }

                _ => {
                    tracing::warn!("unknown logical channel: {:?}", lchan);
                }
            }

            // Check if end of message reached by re-borrowing inner
            // If start was not updated, we also consider it end of message
            // If 16 or more bits remain (len of null pdu), we continue parsing
            if let SapMsgInner::TmvUnitdataInd(prim) = &message.msg {
                if prim.pdu.get_raw_start() != orig_start && prim.pdu.get_len() >= 16 {
                    tracing::trace!("orig {} now {}", orig_start, prim.pdu.get_raw_start());
                    tracing::trace!(
                        "rx_tmv_unitdata_ind_sch: Remaining {} bits: {:?}",
                        prim.pdu.get_len_remaining(),
                        prim.pdu.dump_bin_full(true)
                    );
                } else {
                    tracing::trace!("rx_tmv_unitdata_ind_sch: End of message reached");
                    break;
                }
            }
        }
    }

    fn rx_mac_data(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_data");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU
        let carrier_num = prim.carrier_num;

        let pdu = match MacData::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacData: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Get addr, either from pdu addr field or by resolving the event label
        if pdu.event_label.is_some() {
            unimplemented_log!("event labels not implemented");
            return;
        }
        // A well-formed MAC PDU must carry either addr or event_label. We already
        // returned for event_label above; if addr is also missing the PDU is malformed
        // — drop it instead of panicking on .unwrap().
        let Some(addr) = pdu.addr else {
            tracing::warn!("UMAC: rx_mac_data: PDU has neither addr nor event_label; dropping");
            return;
        };

        let (mut pdu_len_bits, is_frag_start, second_half_stolen, is_null_pdu) = {
            if let Some(len_ind) = pdu.length_ind {
                // We have a length ind, either clear length or a fragmentation start
                match len_ind {
                    0b000000 => {
                        // Null PDU
                        (if pdu.event_label.is_some() { 23 } else { 37 }, false, false, true)
                    }
                    0b000010..0b111000 => (len_ind as usize * 8, false, false, false),
                    0b111110 => {
                        // Second half stolen. Should be in STCH
                        (prim.pdu.get_len(), false, true, false)
                    }
                    0b111111 => {
                        // Start of fragmentation
                        (prim.pdu.get_len(), true, false, false)
                    }
                    _ => {
                        tracing::warn!("UMAC: rx_mac_data: unexpected length_ind {:#08b}, dropping PDU", len_ind);
                        return;
                    }
                }
            } else {
                // We have a capacity request — per spec, MacData with cap_req must
                // carry frag_flag. If a malformed PDU arrives without it, fall back to
                // frag_flag=false rather than panic.
                let frag_flag = pdu.frag_flag.unwrap_or_else(|| {
                    tracing::warn!("rx_mac_data: cap_req PDU missing frag_flag; assuming false");
                    false
                });
                tracing::trace!("rx_mac_data: cap_req {}", if frag_flag { "with frag_start" } else { "" });
                (prim.pdu.get_len(), frag_flag, false, false)
            }
        };

        if second_half_stolen {
            tracing::debug!("rx_mac_data: STCH 2nd half stolen");
            self.signal_lmac_second_half_stolen(queue, carrier_num, self.dltime.add_timeslots(-2));
        }

        // Truncate len if past end (okay with standard)
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-DATA len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len() as usize;
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        tracing::trace!("rx_mac_data: {}", prim.pdu.dump_bin_full(true));
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, is_null_pdu)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_data: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        if is_null_pdu {
            // TODO not sure if there is scenarios in which we want to pass a null pdu to the LLC
            // tracing::warn!("rx_mac_data: Null PDU not passed to LLC");
            return;
        }

        // Decrypt if needed
        if pdu.encrypted {
            unimplemented_log!("rx_mac_data: Encryption mode > 0");
            return;
        }

        // Handle reservation if present
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago. 
        if let Some(res_req) = &pdu.reservation_req {
            let grant_result = self.scheduler_for_mut(carrier_num).ul_process_cap_req(msg_dltime.t, addr, res_req);
            if let Some((grant, usage_marker)) = grant_result {
                // Schedule grant — marker propagates into the MAC-RESOURCE ACK
                // so the MS can tag its reservation when continuing the burst.
                self.scheduler_for_mut(carrier_num)
                    .dl_enqueue_grant(msg_dltime.t, addr, grant, usage_marker);
            } else {
                tracing::warn!("rx_mac_data: No grant for reservation request {:?}", res_req);
            }
        };

        tracing::debug!("rx_mac_data: {}", prim.pdu.dump_bin_full(true));
        if is_frag_start {
            // Fragmentation start, add to defragmenter
            self.defrag.insert_first(&mut prim.pdu, msg_dltime, addr, None);
        } else {
            // Pass directly to LLC
            let sdu = {
                if prim.pdu.get_len_remaining() == 0 {
                    None // No more data in this block
                } else {
                    // TODO FIXME should not copy here but take ownership
                    // Copy inner part, without MAC header or fill bits
                    Some(BitBuffer::from_bitbuffer_pos(&prim.pdu))
                }
            };

            if sdu.is_some() {
                // We have an SDU for the LLC, deliver it.
                let m = SapMsg {
                    sap: Sap::TmaSap,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Llc,

                    msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                        carrier_num: prim.carrier_num,
                        pdu: sdu,
                        main_address: addr,
                        scrambling_code: prim.scrambling_code,
                        link_id: msg_dltime.t as u32,
                        endpoint_id: 0,        // TODO FIXME
                        new_endpoint_id: None, // TODO FIXME
                        css_endpoint_id: None, // TODO FIXME
                        air_interface_encryption: pdu.encrypted as Todo,
                        chan_change_response_req: false,
                        chan_change_handle: None,
                        chan_info: None,
                    }),
                };
                queue.push_back(m);
            } else {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_data: empty PDU not passed to LLC");
            }
        }

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_access(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_access");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU
        let carrier_num = prim.carrier_num;

        let pdu = match MacAccess::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                // A candidate UL burst that isn't a valid MAC-ACCESS — typically a false-positive
                // training-sequence detection or a noise-corrupted burst. Dropped; normal on RF.
                tracing::debug!("Failed parsing MacAccess: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Resolve event label (if supplied)
        let addr = if let Some(_label) = pdu.event_label {
            tracing::warn!("event labels not implemented");
            return;
        } else if let Some(addr) = pdu.addr {
            addr
        } else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        // Compute len and extract flags
        let mut pdu_len_bits;
        if let Some(length_ind) = pdu.length_ind {
            if length_ind == 0 {
                // Null PDU
                if pdu.event_label.is_some() {
                    // Short event label present
                    pdu_len_bits = 22; // 22 bits for event label
                } else {
                    // SSI
                    pdu_len_bits = 36;
                }
            } else {
                // Full length ind
                pdu_len_bits = length_ind as usize * 8;
            }
        } else {
            // No length ind, we have capacity request. Fill slot.
            pdu_len_bits = prim.pdu.get_len();
        }
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-ACCESS len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }

        // Strip fill bits. Maintain original end to allow for later parsing of a second mac block
        // tracing::trace!("rx_mac_access: {}", prim.pdu.dump_bin_full(true));
        let num_fill_bits = if pdu.fill_bits {
            fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, pdu.is_null_pdu())
        } else {
            0
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_access: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        if pdu.is_null_pdu() {
            // tracing::warn!("rx_mac_access: Null PDU not passed to LLC");
            return;
        }

        // Schedule random-access acknowledgement only on signalling slots. A MAC-ACCESS
        // received on an already active traffic timeslot is ordinary in-call signalling
        // (FACCH/STCH LLC ACKs, short control PDUs), not a fresh random-access attempt.
        // Marking those as RA causes the next stolen downlink MAC-RESOURCE to carry
        // random_access_flag=true, which some radios reject during call setup.
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago.
        if msg_dltime.t == 1 && !self.scheduler_for(carrier_num).allow_mcch() {
            if !self.scheduler_for(carrier_num).circuit_is_active(Direction::Dl, msg_dltime.t) {
                tracing::info!(
                    "rx_mac_access: ignoring common-control random access on carrier={} ts1 because this carrier has no MCCH",
                    carrier_num
                );
                return;
            }
            tracing::trace!(
                "rx_mac_access: treating carrier={} ts1 as assigned traffic, not common control",
                carrier_num
            );
        }
        if !self.scheduler_for(carrier_num).circuit_is_active(Direction::Dl, msg_dltime.t) {
            self.scheduler_for_mut(carrier_num).dl_enqueue_random_access_ack(msg_dltime.t, addr);
        } else {
            tracing::trace!(
                "rx_mac_access: suppressing random-access ack on active DL traffic ts {} for {}",
                msg_dltime.t,
                addr
            );
        }

        // Notify MM of RSSI for this MS so it can be stored per-subscriber.
        // Only sent when RSSI is a finite value (i.e. demodulator calculated it).
        if prim.rssi_dbfs.is_finite() {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Umac,
                dest: TetraEntity::Mm,
                msg: SapMsgInner::MsRssiUpdate {
                    issi: addr.ssi,
                    rssi_dbfs: prim.rssi_dbfs,
                },
            });
        }

        // Decrypt if needed
        if pdu.encrypted {
            unimplemented_log!("rx_mac_access: Encryption mode > 0");
            return;
        }

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            let grant_result = self.scheduler_for_mut(carrier_num).ul_process_cap_req(msg_dltime.t, addr, res_req);
            if let Some((grant, usage_marker)) = grant_result {
                // Schedule grant — marker propagates into the MAC-RESOURCE ACK
                // so the MS can tag its reservation when continuing the burst.
                self.scheduler_for_mut(carrier_num)
                    .dl_enqueue_grant(msg_dltime.t, addr, grant, usage_marker);
            } else {
                tracing::warn!("rx_mac_access: No grant for reservation request {:?}", res_req);
            }
        };

        // tracing::debug!("rx_mac_access: {}", prim.pdu.dump_bin_full(true));
        if pdu.is_frag_start() {
            // Fragmentation start, add to defragmenter
            self.defrag.insert_first(&mut prim.pdu, msg_dltime, addr, None);
        } else {
            // Pass directly to LLC
            if prim.pdu.get_len_remaining() == 0 {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_access: empty PDU not passed to LLC");
                return;
            };

            // Pass directly to LLC
            let sdu = {
                if prim.pdu.get_len_remaining() == 0 {
                    None // No more data in this block
                } else {
                    // TODO FIXME check if there is a reasonable way to avoid copying here by taking ownership
                    Some(BitBuffer::from_bitbuffer_pos(&prim.pdu))
                }
            };

            if sdu.is_some() {
                // We have an SDU for the LLC, deliver it.
                let m = SapMsg {
                    sap: Sap::TmaSap,
                    src: TetraEntity::Umac,
                    dest: TetraEntity::Llc,
                    msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                        carrier_num: prim.carrier_num,
                        pdu: sdu,
                        main_address: addr,
                        scrambling_code: prim.scrambling_code,
                        link_id: msg_dltime.t as u32,
                        endpoint_id: 0,        // TODO FIXME
                        new_endpoint_id: None, // TODO FIXME
                        css_endpoint_id: None, // TODO FIXME
                        air_interface_encryption: pdu.encrypted as Todo,
                        chan_change_response_req: false,
                        chan_change_handle: None,
                        chan_info: None,
                    }),
                };
                queue.push_back(m);
            } else {
                // Either this is a null pdu or we are at the end of the block
                // For now, we don't deliver this. However, important data may need to be signalled upwards
                tracing::warn!("rx_mac_data: empty PDU not passed to LLC");
            }
        }

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_frag_ul(&mut self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_frag_ul");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU
        let carrier_num = prim.carrier_num;

        // Parse header and optional ChanAlloc
        let pdu = match MacFragUl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacFragUl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Strip fill bits. This message is known to fill the slot.
        let mut pdu_len_bits = prim.pdu.get_len();
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::debug!("rx_mac_frag_ul: pdu_len_bits: {} fill_bits: {}", pdu_len_bits, num_fill_bits);

        // Get slot owner from schedule
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago. 
        let Some(slot_owner) = self.scheduler_for(carrier_num).ul_get_slot_owner(msg_dltime, prim.block_num) else {
            tracing::debug!(
                "rx_mac_frag_ul: MAC-FRAG-UL for unassigned block {:?} (start not seen — normal on RF loss)",
                prim.block_num
            );
            self.scheduler_for(carrier_num).dump_ul_schedule_full(true);
            return;
        };

        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner, msg_dltime) {
            unimplemented_log!("rx_mac_frag_ul: Encryption not supported");
            return;
        }

        // Insert into defragmenter
        self.defrag.insert_next(&mut prim.pdu, slot_owner, msg_dltime);
    }

    fn rx_mac_end_ul(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_end_ul");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU
        let carrier_num = prim.carrier_num;

        // Parse header and optional ChanAlloc
        let pdu = match MacEndUl::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacEndUl: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Will have either length_ind or reservation_req, never none or both
        let mut pdu_len_bits = if let Some(length_ind) = pdu.length_ind {
            length_ind as usize * 8
        } else {
            // No length ind, we have capacity request. Fill slot.
            prim.pdu.get_len()
        };
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-END-UL len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }

        // Strip fill bits if any
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);
        tracing::trace!(
            "rx_mac_end_ul: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        // Get slot owner from schedule, decrypt if needed
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago. 
        let Some(slot_owner) = self.scheduler_for(carrier_num).ul_get_slot_owner(msg_dltime, prim.block_num) else {
            // Common with scan-list terminals that transmit on UL without waiting for a grant
            tracing::debug!("rx_mac_end_ul: Received MAC-END-UL for unassigned block {:?}", prim.block_num);
            return;
        };
        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner, msg_dltime) {
            unimplemented!("rx_mac_end_ul: Encryption not supported");
        }

        // Insert last fragment and retrieve finalized block
        let defragbuf = self.defrag.insert_last(&mut prim.pdu, slot_owner, msg_dltime);
        let Some(defragbuf) = defragbuf else {
            tracing::debug!("rx_mac_end_ul: could not obtain defragged buf (start not seen — normal on RF loss)");
            return;
        };

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            let grant_result = self
                .scheduler_for_mut(carrier_num)
                .ul_process_cap_req(msg_dltime.t, defragbuf.addr, res_req);
            if let Some((grant, usage_marker)) = grant_result {
                // Schedule grant — marker propagates into the MAC-RESOURCE ACK
                // so the MS can tag its reservation when continuing the burst.
                self.scheduler_for_mut(carrier_num)
                    .dl_enqueue_grant(msg_dltime.t, defragbuf.addr, grant, usage_marker);
            } else {
                tracing::warn!("rx_mac_end_ul: No grant for reservation request {:?}", res_req);
            }
        };

        // Pass completed block to LLC
        tracing::debug!("rx_mac_end_ul: sdu: {:?}", defragbuf.buffer.dump_bin());

        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                carrier_num: prim.carrier_num,
                pdu: Some(defragbuf.buffer),
                main_address: defragbuf.addr,
                scrambling_code: prim.scrambling_code,
                link_id: msg_dltime.t as u32,
                endpoint_id: 0,              // TODO FIXME
                new_endpoint_id: None,       // TODO FIXME
                css_endpoint_id: None,       // TODO FIXME
                air_interface_encryption: 0, // TODO FIXME implement
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    fn rx_mac_end_hu(&mut self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_mac_end_hu");
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        assert!(prim.pdu.get_pos() == 0); // We should be at the start of the MAC PDU
        let carrier_num = prim.carrier_num;

        // Parse header and optional ChanAlloc
        let pdu = match MacEndHu::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacEndHu: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Will have either length_ind or reservation_req, never none or both
        let mut pdu_len_bits = if let Some(length_ind) = pdu.length_ind {
            if length_ind == 0 {
                // Table 21.44: length indication 0 is reserved, discard PDU
                tracing::debug!("rx_mac_end_hu: discarding PDU with reserved length indication 0");
                return;
            }
            let len = length_ind as usize * 8;
            if len > prim.pdu.get_len() { prim.pdu.get_len() } else { len }
        } else {
            // No length ind, we have capacity request. Fill slot.
            prim.pdu.get_len()
        };
        if pdu_len_bits > prim.pdu.get_len() {
            tracing::warn!("truncating MAC-END-HU len from {} to {}", pdu_len_bits, prim.pdu.get_len());
            pdu_len_bits = prim.pdu.get_len();
        }

        // Strip fill bits if any
        let num_fill_bits = {
            if pdu.fill_bits {
                fillbits::removal::get_num_fill_bits(&prim.pdu, pdu_len_bits, false)
            } else {
                0
            }
        };
        pdu_len_bits -= num_fill_bits;
        let orig_end = prim.pdu.get_raw_end();
        prim.pdu.set_raw_end(prim.pdu.get_raw_start() + pdu_len_bits);

        // set to trace
        tracing::trace!(
            "rx_mac_end_hu: pdu: {} sdu: {} fb: {}: {}",
            pdu_len_bits,
            prim.pdu.get_len_remaining(),
            num_fill_bits,
            prim.pdu.dump_bin_full(true)
        );

        // Get slot owner from schedule, decrypt if needed
        let msg_dltime = self.dltime.add_timeslots(-2); // Msg on uplink was sent two timeslots ago. 
        let Some(slot_owner) = self.scheduler_for(carrier_num).ul_get_slot_owner(msg_dltime, prim.block_num) else {
            tracing::debug!(
                "rx_mac_end_hu: MAC-END-HU for unassigned block {:?} (start not seen — normal on RF loss)",
                prim.block_num
            );
            self.scheduler_for(carrier_num).dump_ul_schedule_full(true);
            return;
        };
        if let Some(_aie_info) = self.defrag.get_aie_info(slot_owner, msg_dltime) {
            unimplemented!("rx_mac_end_hu: Encryption not supported");
        }

        // Insert last fragment and retrieve finalized block
        let defragbuf = self.defrag.insert_last(&mut prim.pdu, slot_owner, msg_dltime);
        let Some(defragbuf) = defragbuf else {
            tracing::debug!("rx_mac_end_hu: could not obtain defragged buf (start not seen — normal on RF loss)");
            return;
        };

        // Handle reservation if present
        if let Some(res_req) = &pdu.reservation_req {
            let grant_result = self
                .scheduler_for_mut(carrier_num)
                .ul_process_cap_req(msg_dltime.t, defragbuf.addr, res_req);
            if let Some((grant, usage_marker)) = grant_result {
                // Schedule grant — marker propagates into the MAC-RESOURCE ACK
                // so the MS can tag its reservation when continuing the burst.
                self.scheduler_for_mut(carrier_num)
                    .dl_enqueue_grant(msg_dltime.t, defragbuf.addr, grant, usage_marker);
            } else {
                tracing::warn!("rx_mac_end_hu: No grant for reservation request {:?}", res_req);
            }
        };

        // Pass completed block to LLC
        tracing::debug!("rx_mac_end_hu: sdu: {:?}", defragbuf.buffer.dump_bin());

        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                carrier_num: prim.carrier_num,
                pdu: Some(defragbuf.buffer),
                main_address: defragbuf.addr,
                scrambling_code: prim.scrambling_code,
                link_id: msg_dltime.t as u32,
                endpoint_id: 0,              // TODO FIXME
                new_endpoint_id: None,       // TODO FIXME
                css_endpoint_id: None,       // TODO FIXME
                air_interface_encryption: 0, // TODO FIXME implement
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);

        // Since this is not a null pdu, more MAC PDUs may follow
        // This allows parent function to continue parsing
        // tracing::trace!("rx_mac_end_hu: orig_end {} raw_start {} num_fill_bits {} curr_pos {}", orig_end, prim.pdu.get_raw_start(), num_fill_bits, prim.pdu.get_raw_pos());
        prim.pdu.set_raw_end(orig_end);
        prim.pdu.set_raw_pos(prim.pdu.get_raw_start() + pdu_len_bits + num_fill_bits);
        prim.pdu.set_raw_start(prim.pdu.get_raw_pos());
    }

    /// UL MAC-U-SIGNAL on STCH: extract TM-SDU and forward to LLC → MLE → CMCE.
    /// This carries signaling like U-TX CEASED / U-TX DEMAND on the traffic channel.
    fn rx_ul_mac_u_signal(&self, queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_ul_mac_u_signal");

        // Extract sdu and parse pdu
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let pdu = match MacUSignal::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacUSignal: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        if pdu.second_half_stolen {
            tracing::warn!("rx_ul_mac_u_signal: second_half_stolen not implemented");
            return;
        }

        // The remaining bits after the MAC-U-SIGNAL header are the TM-SDU (LLC PDU)
        if prim.pdu.get_len_remaining() == 0 {
            tracing::trace!("rx_ul_mac_u_signal: empty TM-SDU");
            return;
        }

        let sdu = BitBuffer::from_bitbuffer_pos(&prim.pdu);
        tracing::debug!("rx_ul_mac_u_signal: forwarding {} bit TM-SDU to LLC", sdu.get_len());

        let msg_dltime = self.dltime.add_timeslots(-2);
        let key = (prim.carrier_num, msg_dltime.t);
        let owner_issi = self
            .scheduler_for(prim.carrier_num)
            .ul_get_slot_owner(msg_dltime, prim.block_num)
            .or_else(|| self.ul_signal_owner.get(&key).copied())
            .unwrap_or(0);

        // Forward to LLC via TMA-SAP, same path as MAC-DATA.
        let m = SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Umac,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
                carrier_num: prim.carrier_num,
                pdu: Some(sdu),
                main_address: TetraAddress::issi(owner_issi),
                scrambling_code: prim.scrambling_code,
                link_id: msg_dltime.t as u32,
                endpoint_id: 0,
                new_endpoint_id: None,
                css_endpoint_id: None,
                air_interface_encryption: 0,
                chan_change_response_req: false,
                chan_change_handle: None,
                chan_info: None,
            }),
        };
        queue.push_back(m);
    }

    /// TMA-SAP MAC-U-BLCK
    fn rx_ul_mac_u_blck(&self, _queue: &mut MessageQueue, message: &mut SapMsg) {
        tracing::trace!("rx_ul_mac_u_blck");

        // Extract sdu and parse pdu
        let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        let _pdu = match MacUBlck::from_bitbuf(&mut prim.pdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing MacUBlck: {:?} {}", e, prim.pdu.dump_bin());
                return;
            }
        };

        // Handle reservation if present
        // TODO implement slightly different handling since enum is not the same.
        unimplemented!();
    }

    fn rx_ul_tma_unitdata_req(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_ul_tma_unitdata_req");

        // Extract sdu
        let SapMsgInner::TmaUnitdataReq(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let preferred_carrier = prim.carrier_num.unwrap_or_else(|| self.main_carrier());
        let mut sdu = prim.pdu;

        // ── FACCH/Stealing path ──────────────────────────────────────────
        // stealing_permission → STCH on traffic channel for time-critical signaling
        // (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
        // CRITICAL: DL STCH uses MAC-RESOURCE (124-bit half-slot), NOT MAC-U-SIGNAL (UL-only).
        if prim.stealing_permission {
            // Determine the target traffic timeslot for FACCH stealing.
            // If chan_alloc specifies a timeslot, use it; otherwise fall back to first active DL circuit.
            let traffic_ts = prim
                .chan_alloc
                .as_ref()
                .and_then(|ca| ca.timeslots.iter().enumerate().find(|&(_, &set)| set).map(|(i, _)| (i + 1) as u8))
                .or_else(|| (1..=4u8).find(|&t| self.scheduler_for(preferred_carrier).can_deliver_stealing(t)));

            if let Some(ts) = traffic_ts {
                // Guard: don't steal on a circuit that was just released (race between
                // D-RELEASE enqueue and circuit close). Drop silently — the MS will
                // receive the release via MCCH fallback or retransmit as needed.
                if !self.scheduler_for(preferred_carrier).circuit_is_active(Direction::Dl, ts) {
                    tracing::debug!(
                        "rx_ul_tma_unitdata_req: FACCH stealing on ts {} skipped (circuit already closed)",
                        ts
                    );
                    // Fall through to MCCH path so the PDU still gets delivered.
                } else {
                    // Build MAC-RESOURCE PDU for the STCH half-slot (124 type1 bits).
                    const STCH_CAP: usize = 124;

                    let has_pending_ra = self.channel_scheduler.take_pending_ra_ack(ts, prim.main_address.ssi);
                    // FACCH/STCH on an already allocated traffic slot is not a random-access
                    // response by default. Only propagate the flag when we are actually
                    // carrying a pending RA acknowledgement on this same timeslot.
                    let is_random_access_response = has_pending_ra;
                    let mac_pdu = MacResource {
                        fill_bits: false,
                        pos_of_grant: 0,
                        encryption_mode: 0,
                        random_access_flag: is_random_access_response,
                        length_ind: 0,
                        addr: Some(prim.main_address),
                        event_label: None,
                        // The CMCE chan_alloc on this path is only an internal hint for
                        // selecting the traffic timeslot. Do not encode it as a usage marker
                        // on FACCH/STCH MAC-RESOURCE.
                        usage_marker: None,
                        power_control_element: None,
                        slot_granting_element: None,
                        chan_alloc_element: None,
                    };

                    sdu.seek(0);
                    let sdu_len = sdu.get_len();
                    let hdr_len = mac_pdu.compute_header_len();

                    if hdr_len + sdu_len <= STCH_CAP {
                        // Fits in a single stolen half-slot — one STCH block (unchanged path,
                        // used by small control PDUs and short status/SDS messages).
                        let mut mac_pdu = mac_pdu;
                        let num_fill_bits = mac_pdu.update_len_and_fill_ind(sdu_len);

                        let mut stch_block = BitBuffer::new(STCH_CAP);
                        mac_pdu.to_bitbuf(&mut stch_block);
                        sdu.seek(0);
                        stch_block.copy_bits(&mut sdu, sdu_len);
                        fillbits::addition::write(&mut stch_block, Some(num_fill_bits));

                        tracing::info!(
                            "rx_ul_tma_unitdata_req: FACCH stealing on ts {} (MAC-RESOURCE + {} SDU bits → {} STCH bits)",
                            ts,
                            sdu_len,
                            stch_block.get_len()
                        );

                        self.channel_scheduler.dl_enqueue_stealing(ts, stch_block, prim.tx_reporter);
                    } else {
                        // Larger than one stolen half-slot: fragment across consecutive stolen
                        // half-slots (panic-safe — a fixed 124-bit buffer used to overflow here and
                        // crash the stack). Individual in-call SDS no longer reach this path: the
                        // field radios do not accept an SDS in-band on the traffic channel, so CMCE
                        // defers them to the MCCH (see sds_bs PendingSds). This path now only covers
                        // the rare oversized group/other stealing PDU. (FH-BUG-034.)
                        let mut fragger = BsFragger::new(mac_pdu, sdu, prim.tx_reporter);
                        let mut produced = 0usize;
                        loop {
                            let mut stch_block = BitBuffer::new(STCH_CAP);
                            let done = fragger.get_next_chunk(&mut stch_block);
                            self.channel_scheduler.dl_enqueue_stealing(ts, stch_block, None);
                            produced += 1;
                            if done || produced >= 32 {
                                break;
                            }
                        }
                    }

                    return;
                } // end circuit_is_active guard
            } else {
                tracing::warn!("rx_ul_tma_unitdata_req: stealing requested but no active DL circuit, falling back to MCCH");
                // Fall through to normal MCCH path below
            }
        }

        // ── Normal signaling path (MCCH / SCH/F) ────────────────────────
        let (usage_marker, mac_chan_alloc) = if let Some(chan_alloc) = prim.chan_alloc {
            let carrier_num = self.config.config().cell.main_carrier;
            let Some(mac_chan_alloc) = self.cmce_to_mac_chanalloc(&chan_alloc, carrier_num) else {
                if let Some(tx_reporter) = prim.tx_reporter {
                    tx_reporter.mark_discarded();
                }
                return;
            };
            (chan_alloc.usage, Some(mac_chan_alloc))
        } else {
            (None, None)
        };

        // Build MAC-RESOURCE optimistically (as if it would always fit in one slot)
        // random_access_flag: true for SSI-addressed responses to random access requests,
        // false for GSSI-addressed unsolicited group signaling. When link_id == 0,
        // there is no LLC link context, so treat the message as unsolicited too.
        let is_random_access_response = prim.main_address.ssi_type != SsiType::Gssi && prim.link_id != 0;
        let mut pdu = MacResource {
            fill_bits: false, // Updated later
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: is_random_access_response,
            length_ind: 0, // Updated later
            addr: Some(prim.main_address),
            event_label: None,
            usage_marker,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: mac_chan_alloc,
        };
        pdu.update_len_and_fill_ind(sdu.get_len());

        // Let the scheduler choose MCCH versus linked signalling timeslot from
        // the LLC link context. FACCH/traffic-slot signalling uses the explicit
        // stealing path above.
        self.channel_scheduler
            .dl_enqueue_tma_for_link(prim.link_id, pdu, sdu, prim.tx_reporter);
    }

    fn rx_ul_tma_unitdata_req_carrier(&mut self, _queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_ul_tma_unitdata_req");

        let SapMsgInner::TmaUnitdataReq(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let preferred_carrier = prim.carrier_num.unwrap_or_else(|| self.main_carrier());
        let mut sdu = prim.pdu;

        if prim.stealing_permission {
            let requested_ts = prim
                .chan_alloc
                .as_ref()
                .and_then(|ca| ca.timeslots.iter().enumerate().find(|&(_, &set)| set).map(|(i, _)| (i + 1) as u8));
            let requested_carrier = prim.chan_alloc.as_ref().and_then(|ca| ca.carrier).unwrap_or(preferred_carrier);
            let traffic_ts = match requested_ts {
                Some(ts) if self.scheduler_for(requested_carrier).can_deliver_stealing(ts) => Some(ts),
                Some(ts) => {
                    tracing::warn!(
                        "rx_ul_tma_unitdata_req: stealing requested for carrier={} ts {} without active DL circuit, falling back to MCCH",
                        requested_carrier,
                        ts
                    );
                    None
                }
                None => (1..=4u8).find(|&t| self.scheduler_for(requested_carrier).can_deliver_stealing(t)),
            };

            if let Some(ts) = traffic_ts {
                if self.scheduler_for(requested_carrier).circuit_is_active(Direction::Dl, ts) {
                    const STCH_CAP: usize = 124;
                    let has_pending_ra = self
                        .scheduler_for_mut(requested_carrier)
                        .take_pending_ra_ack(ts, prim.main_address.ssi);
                    let mac_pdu = MacResource {
                        fill_bits: false,
                        pos_of_grant: 0,
                        encryption_mode: 0,
                        random_access_flag: has_pending_ra,
                        length_ind: 0,
                        addr: Some(prim.main_address),
                        event_label: None,
                        usage_marker: None,
                        power_control_element: None,
                        slot_granting_element: None,
                        chan_alloc_element: None,
                    };

                    sdu.seek(0);
                    let sdu_len = sdu.get_len();
                    let hdr_len = mac_pdu.compute_header_len();

                    if hdr_len + sdu_len <= STCH_CAP {
                        let mut mac_pdu = mac_pdu;
                        let num_fill_bits = mac_pdu.update_len_and_fill_ind(sdu_len);
                        let mut stch_block = BitBuffer::new(STCH_CAP);
                        mac_pdu.to_bitbuf(&mut stch_block);
                        sdu.seek(0);
                        stch_block.copy_bits(&mut sdu, sdu_len);
                        fillbits::addition::write(&mut stch_block, Some(num_fill_bits));
                        self.scheduler_for_mut(requested_carrier)
                            .dl_enqueue_stealing(ts, stch_block, prim.tx_reporter);
                    } else {
                        let mut fragger = BsFragger::new(mac_pdu, sdu, prim.tx_reporter);
                        let mut produced = 0usize;
                        loop {
                            let mut stch_block = BitBuffer::new(STCH_CAP);
                            let done = fragger.get_next_chunk(&mut stch_block);
                            self.scheduler_for_mut(requested_carrier).dl_enqueue_stealing(ts, stch_block, None);
                            produced += 1;
                            if done || produced >= 32 {
                                break;
                            }
                        }
                    }
                    return;
                }
            }
        }

        let (usage_marker, mac_chan_alloc) = if let Some(chan_alloc) = prim.chan_alloc {
            let carrier_num = chan_alloc.carrier.unwrap_or(preferred_carrier);
            let Some(mac_chan_alloc) = self.cmce_to_mac_chanalloc(&chan_alloc, carrier_num) else {
                if let Some(tx_reporter) = prim.tx_reporter {
                    tx_reporter.mark_discarded();
                }
                return;
            };
            (chan_alloc.usage, Some(mac_chan_alloc))
        } else {
            (None, None)
        };

        let is_random_access_response = prim.main_address.ssi_type != SsiType::Gssi && prim.link_id != 0;
        let mut pdu = MacResource {
            fill_bits: false,
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: is_random_access_response,
            length_ind: 0,
            addr: Some(prim.main_address),
            event_label: None,
            usage_marker,
            power_control_element: None,
            slot_granting_element: None,
            chan_alloc_element: mac_chan_alloc,
        };
        pdu.update_len_and_fill_ind(sdu.get_len());

        // Normal control signalling must stay on the primary MCCH even when the
        // embedded chan_alloc points at a secondary traffic carrier. Secondary
        // BCCH carriers in `SecondaryBcchNoMcch` mode intentionally have no MCCH,
        // so enqueueing call-setup/control PDUs there makes them unschedulable
        // and drops D-CONNECT/D-SETUP for cross-carrier calls.
        self.channel_scheduler
            .dl_enqueue_tma_for_link(prim.link_id, pdu, sdu, prim.tx_reporter);
    }

    fn rx_tma_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tma_prim");
        match message.msg {
            SapMsgInner::TmaUnitdataReq(_) => {
                self.rx_ul_tma_unitdata_req_carrier(queue, message);
            }
            _ => {
                tracing::warn!("unhandled match variant, ignoring");
            }
        }
    }

    fn rx_tlmb_prim(&mut self, _queue: &mut MessageQueue, _message: SapMsg) {
        tracing::trace!("rx_tlmb_prim");
        tracing::error!("BUG: unexpected message or state -- routing error");
        return;
    }

    fn rx_tmd_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_tmd_prim");

        let src = message.src;
        match message.msg {
            // DL voice from Brew/upper layer → schedule for DL transmission
            SapMsgInner::TmdCircuitDataReq(prim) => {
                let carrier_num = prim.carrier_num;
                let ts = prim.ts;
                // Network/shared downlink media (Brew/TetraPack -> DL) must NOT arm the local
                // UL stuck-transmitter timer: the speaker is remote and no local radio is
                // transmitting. The timer only tracks locally-owned uplink, which is armed by
                // CallControl::FloorGranted and refreshed by TmdCircuitDataInd (real UL frames).
                if self.scheduler_for(carrier_num).circuit_is_active(Direction::Dl, ts) {
                    self.scheduler_for_mut(carrier_num).dl_schedule_tmd(ts, prim.data);
                } else {
                    tracing::warn!(
                        "rx_tmd_prim: dropping DL voice on inactive circuit carrier={} ts={} src={:?} dltime={}",
                        carrier_num,
                        ts,
                        src,
                        self.dltime
                    );
                }
            }
            // UL voice from LMAC → forward to Brew + cross-route (duplex) or loopback (simplex) to DL
            SapMsgInner::TmdCircuitDataInd(prim) => {
                let carrier_num = prim.carrier_num;
                let ts = prim.ts;
                let data = prim.data;

                // Track last UL voice frame time for inactivity detection
                if (1..=4).contains(&ts) {
                    self.last_ul_voice.insert((carrier_num, ts), self.dltime);
                }
                if let Some(sink) = &self.telemetry {
                    sink.send(TelemetryEvent::TsVoiceActivity {
                        carrier_num,
                        ts,
                        speaker_issi: self.ul_signal_owner.get(&(carrier_num, ts)).copied(),
                    });
                }

                // Forward UL voice to Brew (User plane) if loaded
                if self.config.config().brew.is_some() {
                    if self.scheduler_for(carrier_num).circuit_is_active(Direction::Ul, ts) {
                        let msg = SapMsg {
                            sap: Sap::TmdSap,
                            src: TetraEntity::Umac,
                            dest: TetraEntity::Brew,
                            msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                                carrier_num,
                                ts,
                                data: data.clone(),
                            }),
                        };
                        queue.push_back(msg);
                    } else {
                        tracing::trace!("rx_tmd_prim: no active UL circuit on ts={}, dropping UL voice to Brew", ts);
                    }
                }

                // Forward UL voice to Asterisk if the SIP bridge is enabled. The Asterisk entity
                // keeps its own ts -> dialog map and ignores unrelated circuits, so we fan out to
                // every enabled network entity and let each self-filter by active call.
                #[cfg(feature = "asterisk")]
                if self.config.config().asterisk.enabled {
                    if self.scheduler_for(carrier_num).circuit_is_active(Direction::Ul, ts) {
                        let msg = SapMsg {
                            sap: Sap::TmdSap,
                            src: TetraEntity::Umac,
                            dest: TetraEntity::Asterisk,
                            msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                                carrier_num,
                                ts,
                                data: data.clone(),
                            }),
                        };
                        queue.push_back(msg);
                    } else {
                        tracing::trace!("rx_tmd_prim: no active UL circuit on ts={}, dropping UL voice to Asterisk", ts);
                    }
                }

                // Forward UL voice to EchoLink if the bridge is enabled. The EchoLink entity keeps
                // its own ts -> QSO map and ignores unrelated circuits.
                if self.config.effective_echolink().enabled {
                    if self.scheduler_for(carrier_num).circuit_is_active(Direction::Ul, ts) {
                        let msg = SapMsg {
                            sap: Sap::TmdSap,
                            src: TetraEntity::Umac,
                            dest: TetraEntity::Echolink,
                            msg: SapMsgInner::TmdCircuitDataInd(tetra_saps::tmd::TmdCircuitDataInd {
                                carrier_num,
                                ts,
                                data: data.clone(),
                            }),
                        };
                        queue.push_back(msg);
                    } else {
                        tracing::trace!("rx_tmd_prim: no active UL circuit on ts={}, dropping UL voice to EchoLink", ts);
                    }
                }

                // Determine DL target timeslot:
                //   - Full-duplex P2P (local): UL on `ts` cross-routed to peer MS's DL on `peer_ts`.
                //   - Group / simplex (LocalLoopback, no peer_ts): same-ts loopback so all members hear.
                //   - Circuit call via Brew/TetraPack (SwMI, no peer_ts): suppress local loopback.
                //     DL audio comes from Brew via TmdCircuitDataReq; looping back UL here causes the
                //     calling MS to hear their own voice instead of the remote party.
                // Refresh the peer's UL inactivity timer so the remote MS isn't timed out while
                // only the other party is talking.
                let (dl_target_carrier, dl_target_ts) = self.scheduler_for(carrier_num).duplex_peer_route(ts).unwrap_or((carrier_num, ts));
                if self.scheduler_for(dl_target_carrier).dl_media_source(dl_target_ts) == Some(CircuitDlMediaSource::SwMI) {
                    tracing::trace!(
                        "rx_tmd_prim: circuit call carrier={} ts={}, suppressing local UL loopback (SwMI)",
                        carrier_num,
                        ts
                    );
                    return;
                }

                if self.scheduler_for(dl_target_carrier).circuit_is_active(Direction::Dl, dl_target_ts) {
                    if let Some(packed) = pack_ul_acelp_bits(&data) {
                        self.scheduler_for_mut(dl_target_carrier).dl_schedule_tmd(dl_target_ts, packed);
                    } else {
                        tracing::warn!(
                            "rx_tmd_prim: unsupported UL voice length {} on carrier={} ts={} (target carrier={} ts={}), skipping",
                            data.len(),
                            carrier_num,
                            ts,
                            dl_target_carrier,
                            dl_target_ts
                        );
                    }
                } else {
                    tracing::trace!(
                        "rx_tmd_prim: no active DL circuit on carrier={} ts={} (UL src carrier={} ts={}), skipping",
                        dl_target_carrier,
                        dl_target_ts,
                        carrier_num,
                        ts
                    );
                }
            }
            _ => {
                tracing::warn!("rx_tmd_prim: unexpected message type");
            }
        }
    }

    fn signal_lmac_second_half_stolen(&mut self, queue: &mut MessageQueue, carrier_num: u16, time: TdmaTime) {
        let m = SapMsg {
            sap: Sap::TmvSap,
            src: self.self_component,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvConfigureReq(TmvConfigureReq {
                carrier_num: Some(carrier_num),
                blk2_stolen: Some(true),
                time: Some(time),
                ..Default::default()
            }),
        };
        queue.push_prio(m, MessagePrio::Immediate);
    }

    // fn rx_stch_second_half(&mut self, queue: &mut MessageQueue, message: &mut SapMsg, pending: PendingStch) {
    //     let SapMsgInner::TmvUnitdataInd(prim) = &mut message.msg else {
    //         panic!()
    //     };

    //     // Sanity checks
    //     assert!(prim.logical_channel == LogicalChannel::Stch, "rx_stch_second_half: expected STCH logical channel, got {:?}", prim.logical_channel);
    //     assert!(prim.block_num == PhyBlockNum::Block2, "rx_stch_second_half: expected Block2, got {:?}", prim.block_num);
    //     assert!(self.pending_stch.is_some(), "rx_stch_second_half: no pending STCH, cannot process second half");

    //     let mut first = pending.sdu_part;
    //     first.seek(0);
    //     let first_len = first.get_len_remaining();
    //     prim.pdu.seek(0);
    //     let second_len = prim.pdu.get_len_remaining();

    //     self.rx_mac_access(queue, message);

    //     let mut combined = BitBuffer::new(first_len + second_len);
    //     combined.copy_bits(&mut first, first_len);
    //     combined.copy_bits(&mut prim.pdu, second_len);
    //     combined.seek(0);

    //     if pending.fill_bits {
    //         let total_len = combined.get_len();
    //         let num_fill_bits = fillbits::removal::get_num_fill_bits(&combined, total_len, false);
    //         if num_fill_bits > 0 {
    //             combined.set_raw_end(total_len - num_fill_bits);
    //         }
    //         combined.seek(0);
    //     }

    //     let m = SapMsg {
    //         sap: Sap::TmaSap,
    //         src: TetraEntity::Umac,
    //         dest: TetraEntity::Llc,
    //         dltime: message.dltime,
    //         msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
    //             pdu: Some(combined),
    //             main_address: pending.addr,
    //             scrambling_code: pending.scrambling_code,
    //             endpoint_id: 0,
    //             new_endpoint_id: None,
    //             css_endpoint_id: None,
    //             air_interface_encryption: pending.encrypted as Todo,
    //             chan_change_response_req: false,
    //             chan_change_handle: None,
    //             chan_info: None,
    //         }),
    //     };
    //     queue.push_back(m);
    // }

    fn rx_control_circuit_open(&mut self, _queue: &mut MessageQueue, prim: CallControl) {
        let CallControl::Open(circuit) = prim else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let ts = circuit.ts;
        let carrier_num = circuit.carrier_num;
        let dir = circuit.direction;

        // Direction::Both needs to be split into separate DL and UL operations
        // because the UMAC circuit manager tracks them independently.
        let dirs: Vec<Direction> = match dir {
            Direction::Both => vec![Direction::Dl, Direction::Ul],
            d @ (Direction::Dl | Direction::Ul) => vec![d],
            Direction::None => {
                tracing::warn!("rx_control_circuit_open: Direction::None, ignoring");
                return;
            }
        };

        for d in dirs {
            // See if pre-existing circuit somehow needs to be closed
            if self.scheduler_for(carrier_num).circuit_is_active(d, ts) {
                tracing::warn!(
                    "rx_control_circuit_open: Circuit already exists for {:?} carrier={} ts={}, closing first",
                    d,
                    carrier_num,
                    ts
                );
                self.scheduler_for_mut(carrier_num).close_circuit(d, ts);
            }

            let c = Circuit {
                direction: d,
                carrier_num,
                peer_carrier_num: circuit.peer_carrier_num,
                ts: circuit.ts,
                peer_ts: circuit.peer_ts,
                usage: circuit.usage,
                circuit_mode: circuit.circuit_mode,
                speech_service: circuit.speech_service,
                etee_encrypted: circuit.etee_encrypted,
                dl_media_source: circuit.dl_media_source,
            };
            self.scheduler_for_mut(carrier_num).create_circuit(d, c);

            // Do NOT arm the UL inactivity (stuck-transmitter) timer merely because a
            // UL-capable circuit was opened. A UL circuit is a shared resource; opening it
            // says nothing about whether local uplink voice is expected. For locally-owned
            // calls the timer is armed by CallControl::FloorGranted and refreshed by real UL
            // frames; for SwMI network/shared calls no local radio transmits, so it must
            // stay disarmed (None) to avoid a false UlInactivityTimeout.

            tracing::debug!(
                "  rx_control_circuit_open: Setup {:?} circuit for carrier={} ts {}",
                d,
                carrier_num,
                ts
            );
        }
    }

    fn rx_control_circuit_close(&mut self, _queue: &mut MessageQueue, prim: CallControl) {
        let (dir, carrier_num, ts) = match prim {
            CallControl::Close(dir, ts) => (dir, self.main_carrier(), ts),
            CallControl::CloseSlot {
                direction,
                carrier_num,
                ts,
            } => (direction, carrier_num, ts),
            _ => {
                tracing::error!("BUG: unexpected message or state -- routing error");
                return;
            }
        };

        if (2..=4).contains(&ts) {
            let pending = self.pending_circuit_closes.entry((carrier_num, ts)).or_default();
            match dir {
                Direction::Both => {
                    pending.dl = true;
                    pending.ul = true;
                }
                Direction::Dl => pending.dl = true,
                Direction::Ul => pending.ul = true,
                Direction::None => {
                    tracing::warn!("rx_control_circuit_close: Direction::None, ignoring");
                    return;
                }
            }
            tracing::info!(
                "  rx_control_circuit_close: Deferred {:?} circuit close for carrier={} ts {}",
                dir,
                carrier_num,
                ts
            );
            return;
        }

        self.close_circuit_now(dir, carrier_num, ts);
    }

    fn close_circuit_now(&mut self, dir: Direction, carrier_num: u16, ts: u8) {
        let dirs: Vec<Direction> = match dir {
            Direction::Both => vec![Direction::Dl, Direction::Ul],
            d @ (Direction::Dl | Direction::Ul) => vec![d],
            Direction::None => {
                tracing::warn!("close_circuit_now: Direction::None, ignoring");
                return;
            }
        };

        for d in dirs {
            match self.scheduler_for_mut(carrier_num).close_circuit(d, ts) {
                Some(_) => {
                    if d == Direction::Ul && (1..=4).contains(&ts) {
                        self.last_ul_voice.remove(&(carrier_num, ts));
                    }
                    tracing::info!(
                        "  rx_control_circuit_close: Closed {:?} circuit for carrier={} ts {}",
                        d,
                        carrier_num,
                        ts
                    );
                }
                None => {
                    tracing::warn!(
                        "  rx_control_circuit_close: No {:?} circuit to close for carrier={} ts {}",
                        d,
                        carrier_num,
                        ts
                    );
                }
            }
        }
    }

    fn process_pending_circuit_closes(&mut self) {
        for (carrier_num, ts) in self.pending_circuit_closes.keys().copied().collect::<Vec<_>>() {
            let pending = self.pending_circuit_closes[&(carrier_num, ts)];
            if !pending.dl && !pending.ul {
                continue;
            }

            if self.scheduler_for(carrier_num).has_pending_stealing(ts) {
                tracing::debug!(
                    "process_pending_circuit_closes: waiting for FACCH/STCH drain on carrier={} ts {}",
                    carrier_num,
                    ts
                );
                continue;
            }

            self.pending_circuit_closes.remove(&(carrier_num, ts));
            if pending.dl {
                self.close_circuit_now(Direction::Dl, carrier_num, ts);
            }
            if pending.ul {
                self.close_circuit_now(Direction::Ul, carrier_num, ts);
            }
        }
    }

    /// Check for UL inactivity on traffic timeslots. If no voice frames have arrived
    /// for UL_INACTIVITY_TIMESLOTS on a timeslot with an active UL circuit (and not in
    /// hangtime), send UlInactivityTimeout to CMCE.
    fn check_ul_inactivity(&mut self, queue: &mut MessageQueue) {
        // Read from config: ul_inactivity_secs * timeslots_per_second (72 = 18 frames * 4 slots)
        // Must be above T.213 (1s) to tolerate DTX and brief RF fading.
        let ul_inactivity_timeslots: i32 = self.config.config().cell.ul_inactivity_secs as i32 * 18 * 4;

        for carrier_num in self.configured_carriers() {
            for ts in 1..=4u8 {
                let key = (carrier_num, ts);
                if !self.scheduler_for(carrier_num).circuit_is_active(Direction::Ul, ts) {
                    continue;
                }
                if self.pending_circuit_closes.get(&key).is_some_and(|pending| pending.ul) {
                    continue;
                }
                if self.scheduler_for(carrier_num).is_hangtime(ts) {
                    continue;
                }

                let timed_out = self
                    .last_ul_voice
                    .get(&key)
                    .is_some_and(|t| t.age(self.dltime) > ul_inactivity_timeslots);

                if timed_out {
                    tracing::warn!(
                        "UL inactivity timeout on carrier={} ts={}, sending notification to CMCE",
                        carrier_num,
                        ts
                    );
                    self.last_ul_voice.remove(&key);

                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Umac,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { carrier_num, ts }),
                    });
                }
            }
        }
    }

    fn rx_control(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::trace!("rx_control");
        let SapMsgInner::CmceCallControl(prim) = message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };

        match prim {
            CallControl::Open(_) => {
                self.rx_control_circuit_open(queue, prim);
            }
            CallControl::Close(_, _) | CallControl::CloseSlot { .. } => {
                self.rx_control_circuit_close(queue, prim);
            }
            // Floor-control signals drive traffic↔signalling transitions during hangtime.
            CallControl::FloorReleased { carrier_num, ts, .. } => {
                self.scheduler_for_mut(carrier_num).set_hangtime(ts, true);
                self.last_ul_voice.remove(&(carrier_num, ts));
                self.ul_signal_owner.remove(&(carrier_num, ts));
            }
            CallControl::FloorGranted {
                carrier_num,
                ts,
                source_issi,
                ..
            } => {
                self.scheduler_for_mut(carrier_num).set_hangtime(ts, false);
                self.last_ul_voice.insert((carrier_num, ts), self.dltime);
                self.ul_signal_owner.insert((carrier_num, ts), source_issi);
            }
            CallControl::RemoteFloorGranted { carrier_num, ts, .. } => {
                self.scheduler_for_mut(carrier_num).set_hangtime(ts, false);
                self.last_ul_voice.remove(&(carrier_num, ts));
                self.ul_signal_owner.remove(&(carrier_num, ts));
            }
            CallControl::CallEnded { carrier_num, ts, .. } => {
                self.scheduler_for_mut(carrier_num).set_hangtime(ts, false);
                self.last_ul_voice.remove(&(carrier_num, ts));
                self.ul_signal_owner.remove(&(carrier_num, ts));
            }

            // UlInactivityTimeout is UMAC→CMCE only, UMAC won't receive it back
            CallControl::UlInactivityTimeout { .. } => {}

            // NetworkCall* and NetworkCircuit* are for CMCE ↔ Brew, not UMAC
            CallControl::NetworkCallStart { .. }
            | CallControl::NetworkCallReady { .. }
            | CallControl::NetworkCallEnd { .. }
            | CallControl::NetworkCircuitSetupRequest { .. }
            | CallControl::NetworkCircuitSetupAccept { .. }
            | CallControl::NetworkCircuitSetupReject { .. }
            | CallControl::NetworkCircuitAlert { .. }
            | CallControl::NetworkCircuitConnectRequest { .. }
            | CallControl::NetworkCircuitConnectConfirm { .. }
            | CallControl::NetworkCircuitSimplexGranted { .. }
            | CallControl::NetworkCircuitSimplexIdle { .. }
            | CallControl::NetworkCircuitMediaReady { .. }
            | CallControl::NetworkCircuitDtmf { .. }
            | CallControl::NetworkCircuitRelease { .. } => {
                tracing::trace!("rx_control: ignoring CMCE-Brew notification (not for UMAC)");
            }
        }
    }
}

impl TetraEntityTrait for UmacBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Umac
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        // tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::TmvSap => {
                self.rx_tmv_prim(queue, message);
            }
            Sap::TmaSap => {
                self.rx_tma_prim(queue, message);
            }
            Sap::TmdSap => {
                self.rx_tmd_prim(queue, message);
            }
            Sap::TlmbSap => {
                self.rx_tlmb_prim(queue, message);
            }
            Sap::TlmcSap => {
                unimplemented!();
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

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.dltime = ts;
        self.refresh_system_wide_services();

        if self.channel_scheduler.cur_dltime != ts && self.channel_scheduler.cur_dltime == (TdmaTime { t: 0, f: 0, m: 0, h: 0 }) {
            // Upon start of the system, we need to set the dl time for the channel scheduler
            self.channel_scheduler.set_dl_time(ts);
            for scheduler in &mut self.secondary_channel_schedulers {
                scheduler.set_dl_time(ts);
            }
        } else {
            // When running, we adopt the new time and check for desync
            self.channel_scheduler.tick_start(ts);
            for scheduler in &mut self.secondary_channel_schedulers {
                scheduler.tick_start(ts);
            }
        }

        // Check for UL inactivity (stuck transmitter detection)
        self.check_ul_inactivity(queue);

        // Feed the health monitor's Congestion domain: current downlink scheduling backlog.
        crate::health::registry().set_dl_queue_depth(self.channel_scheduler.dl_queue_depth());

        // Collect/construct traffic that should be sent down to the LMAC
        // This is basically the _previous_ timeslot
        let mut slots = Vec::with_capacity(1 + self.secondary_channel_schedulers.len());
        slots.push(self.channel_scheduler.finalize_ts_for_tick());
        for scheduler in &mut self.secondary_channel_schedulers {
            if let Some(slot) = scheduler.finalize_secondary_ts_for_tick() {
                slots.push(slot);
            }
        }
        let s = SapMsg {
            sap: Sap::TmvSap,
            src: self.self_component,
            dest: TetraEntity::Lmac,
            msg: SapMsgInner::TmvUnitdataReqSlots(TmvUnitdataReqSlots { slots }),
        };
        tracing::trace!("UmacBs tick: Pushing finalized timeslot to LMAC: {:?}", s);
        queue.push_back(s);

        self.process_pending_circuit_closes();
    }
}

/// Pack UL ACELP voice bits (274 bits, one-bit-per-byte) into packed byte array for DL transmission.
/// Handles both already-packed (35 bytes) and unpacked (274 bytes) formats.
fn pack_ul_acelp_bits(bits: &[u8]) -> Option<Vec<u8>> {
    const PACKED_TCH_S_BYTES: usize = (TCH_S_CAP + 7) / 8;

    // Already packed format — pass through
    if bits.len() == PACKED_TCH_S_BYTES {
        return Some(bits.to_vec());
    }
    // Insufficient data
    if bits.len() < TCH_S_CAP {
        return None;
    }

    // Pack 274 one-bit-per-byte into 35 bytes (last byte has 2 padding bits)
    let mut out = Vec::with_capacity(PACKED_TCH_S_BYTES);
    for chunk_idx in 0..PACKED_TCH_S_BYTES {
        let mut byte = 0u8;
        for bit in 0..8 {
            let bit_idx = chunk_idx * 8 + bit;
            if bit_idx < TCH_S_CAP {
                byte |= (bits[bit_idx] & 1) << (7 - bit);
            }
        }
        out.push(byte);
    }
    Some(out)
}
