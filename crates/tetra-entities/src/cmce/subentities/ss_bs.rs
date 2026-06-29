use crate::MessageQueue;
use crate::net_telemetry::{TelemetryEvent, TelemetrySink};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, SsiType, TetraAddress};
use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
use tetra_pdus::cmce::pdus::d_facility::{DFacility, DFacilitySsBody};
use tetra_pdus::cmce::pdus::u_facility::UFacility;
use tetra_pdus::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;
use tetra_pdus::cmce::ss_dgna::fields::group_assignment::GroupAssignment;
use tetra_pdus::cmce::ss_dgna::fields::group_deassignment::GroupDeassignment;
use tetra_pdus::cmce::ss_dgna::pdus::assign::Assign;
use tetra_pdus::cmce::ss_dgna::pdus::deassign::Deassign;
use tetra_pdus::cmce::ss_dgna::ss_dgna_pdu::SsDgnaPdu;
use tetra_saps::lcmc::LcmcMleUnitdataReq;
use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::{SapMsg, SapMsgInner};

/// Clause 12 Supplementary Services CMCE sub-entity.
///
/// Hosts the BS side of SS-DGNA (TS 100 392-12-22 V1.5.1): it emits the
/// SwMI-initiated ASSIGN/DEASSIGN as a CMCE D-FACILITY (the FE2 role,
/// cl.4.1-4.2) and consumes the affected MS's ASSIGN ACK / DEASSIGN ACK off the
/// uplink U-FACILITY. The group registry and affiliation state are owned by MM;
/// this sub-entity only puts the SS PDU on the air and logs the ACK.
pub struct SsBsSubentity {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
}

/// Class of usage advertised in SS-DGNA group assignments. 4 mirrors the value
/// the MM affiliation/ACK path uses (`mm_bs.rs` `DGNA_CLASS_OF_USAGE`), so an
/// SS-DGNA-assigned group behaves identically to one the radio affiliated
/// itself. EN 300 392-2 V2.4.1 cl.16.10.6.
const DGNA_CLASS_OF_USAGE: u8 = 4;

impl SsBsSubentity {
    pub fn new(config: SharedConfig) -> Self {
        SsBsSubentity { config, telemetry: None }
    }

    pub fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    pub fn set_telemetry(&mut self, sink: TelemetrySink) {
        self.telemetry = Some(sink);
    }

    fn emit_dgna_status(&self, issi: u32, gssi: u32, attach: bool, accepted: bool, source: &str, detail: String) {
        if let Some(sink) = &self.telemetry {
            sink.send(TelemetryEvent::DgnaStatus(crate::net_telemetry::events::DgnaStatusInfo {
                issi,
                gssi,
                attach,
                accepted,
                source: source.to_string(),
                detail,
            }));
        }
    }

    /// Emit an SS-DGNA D-FACILITY to a single ISSI: an ASSIGN when `attach`, a
    /// DEASSIGN otherwise. The SS PDU is wrapped in the EN 300 392-9 V1.7.1
    /// Table 4 SS-PDU container (Routeing = 00, one SS PDU) inside the CMCE
    /// D-FACILITY (EN 300 392-2 V2.4.1 cl.14.7.1.7).
    ///
    /// ASSIGN carries the requested Table-51 attachment mode so the operator can
    /// choose whether the group is attached permanently, reattached later, or just
    /// defined without attachment. When present, the mnemonic is carried as the
    /// Table-45 "Mnemonic group name" so the terminal can label the TG directly
    /// from the operator request.
    ///
    /// Reliability rests on the LLC ACK of the FACILITY transport, not a
    /// DGNA-layer retransmit (cl.6.6 mandates no protocol timer), so this is sent
    /// with `Layer2Service::Acknowledged` — the same choice the MM DGNA path made
    /// for its individually-addressed D-ATTACH.
    pub fn send_d_facility_dgna(
        &self,
        queue: &mut MessageQueue,
        issi: u32,
        gssi: u32,
        mnemonic: Option<String>,
        attachment_mode: u8,
        attach: bool,
    ) {
        let attachment_mode =
            GroupIdentityAttachmentMode::try_from(attachment_mode as u64).unwrap_or(GroupIdentityAttachmentMode::AttachedPermanently);
        let ss_pdu = if attach {
            SsDgnaPdu::Assign(Assign {
                groups: vec![GroupAssignment {
                    group_ssi: gssi,
                    group_extension: None,
                    attachment_mode,
                    class_of_usage: Some(DGNA_CLASS_OF_USAGE),
                    mnemonic: mnemonic.as_deref().map(Self::encode_mnemonic),
                    security_related_information: None,
                    additional_group_information: None,
                    vgssi: None,
                }],
                // Request the ASSIGN ACK so the air-interface outcome is observable in logs;
                // BS-side state is already committed by MM at issue time.
                ack_requested: true,
            })
        } else {
            SsDgnaPdu::Deassign(Deassign {
                groups: vec![GroupDeassignment {
                    group_ssi: gssi,
                    group_extension: None,
                }],
                ack_requested: true,
            })
        };

        let pdu = DFacility {
            facility: Some(DFacilitySsBody { ss_pdu }),
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if let Err(e) = pdu.to_bitbuf(&mut sdu) {
            tracing::error!("CMCE: failed serializing SS-DGNA D-FACILITY for ISSI {}: {:?}", issi, e);
            return;
        }
        sdu.seek(0);
        tracing::debug!(
            "-> D-FACILITY (SS-DGNA {}) gssi={} issi={} sdu {}",
            if attach { "assign" } else { "deassign" },
            gssi,
            issi,
            sdu.dump_bin()
        );
        let traffic = self.resolve_traffic_delivery(issi);
        let (layer2service, stealing_permission, chan_alloc, route_detail) = match traffic {
            Some((carrier_num, ts, usage)) if (1..=4).contains(&ts) => {
                let mut timeslots = [false; 4];
                timeslots[(ts - 1) as usize] = true;
                (
                    Layer2Service::Unacknowledged,
                    true,
                    Some(CmceChanAllocReq {
                        usage: Some(usage),
                        carrier: Some(carrier_num),
                        timeslots,
                        alloc_type: ChanAllocType::Replace,
                        ul_dl_assigned: UlDlAssignment::Dl,
                    }),
                    format!("via FACCH stealing on carrier={} ts={}", carrier_num, ts),
                )
            }
            _ => (Layer2Service::Acknowledged, false, None, "via MCCH".to_string()),
        };
        self.emit_dgna_status(
            issi,
            gssi,
            attach,
            true,
            "CMCE",
            format!(
                "Queued SS-DGNA {} D-FACILITY {}",
                if attach { "ASSIGN" } else { "DEASSIGN" },
                route_detail
            ),
        );

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                // Unsolicited, BS-initiated — no inbound L2 context to echo.
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                // Idle target: acknowledged BL-DATA over MCCH. In-call target: unacknowledged
                // FACCH/STCH, because the acknowledged LLC path drops stealing traffic.
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission,
                stealing_repeats_flag: false,
                chan_alloc,
                main_address: TetraAddress::new(issi, SsiType::Issi),
                tx_reporter: None,
            }),
        });
    }

    pub fn route_re_deliver(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("route_re_deliver");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let issi = prim.received_tetra_address.ssi;

        // Try to parse the U-FACILITY as an SS-DGNA carrier. An ASSIGN ACK /
        // DEASSIGN ACK is the affected MS confirming a regroup we already issued;
        // BS-side group state was committed optimistically by MM at issue time,
        // so this is confirmation/telemetry only (mirrors MM's
        // rx_u_attach_detach_group_identity_ack). We do NOT reply
        // function-not-supported in that case.
        match UFacility::from_bitbuf(&mut prim.sdu) {
            Ok(UFacility { facility: Some(body) }) => {
                match &body.ss_pdu {
                    SsDgnaPdu::AssignAck(ack) => {
                        for ie in &ack.acks {
                            tracing::info!(
                                "SS-DGNA: ISSI {} ASSIGN ACK gssi={} assignment={} attachment={}",
                                issi,
                                ie.group_ssi,
                                ie.result_of_assignment,
                                ie.result_of_attachment
                            );
                            let accepted = ie.result_of_assignment.to_string().eq_ignore_ascii_case("accepted")
                                && ie.result_of_attachment.to_string().eq_ignore_ascii_case("accepted");
                            self.emit_dgna_status(
                                issi,
                                ie.group_ssi,
                                true,
                                accepted,
                                "CMCE",
                                format!(
                                    "SS-DGNA ASSIGN ACK assignment={} attachment={}",
                                    ie.result_of_assignment, ie.result_of_attachment
                                ),
                            );
                        }
                    }
                    SsDgnaPdu::DeassignAck(ack) => {
                        for ie in &ack.acks {
                            tracing::info!(
                                "SS-DGNA: ISSI {} DEASSIGN ACK gssi={} deassignment={}",
                                issi,
                                ie.group_ssi,
                                ie.result_of_deassignment
                            );
                            let accepted = ie.result_of_deassignment.to_string().eq_ignore_ascii_case("accepted");
                            self.emit_dgna_status(
                                issi,
                                ie.group_ssi,
                                false,
                                accepted,
                                "CMCE",
                                format!("SS-DGNA DEASSIGN ACK deassignment={}", ie.result_of_deassignment),
                            );
                        }
                    }
                    // An MS-originated ASSIGN/DEASSIGN would be the dispatcher (FE3) role, which the
                    // BS does not act as in v1. Acknowledge nothing; the radio expects no reply to
                    // an SS PDU we don't implement, and a function-not-supported here would be wrong
                    // since the SS *is* recognised.
                    other => {
                        tracing::warn!("SS-DGNA: ignoring unsupported uplink SS-DGNA PDU from ISSI {}: {}", issi, other);
                    }
                }
                return;
            }
            // Empty / non-DGNA U-FACILITY: a genuine SS request the BS does not support.
            Ok(UFacility { facility: None }) => {
                tracing::debug!(
                    "CMCE: received empty U-FACILITY from ISSI {} — responding D-CMCE-FUNCTION-NOT-SUPPORTED",
                    issi
                );
            }
            Err(e) => {
                tracing::debug!(
                    "CMCE: U-FACILITY from ISSI {} not a recognised SS-DGNA PDU ({:?}) — responding D-CMCE-FUNCTION-NOT-SUPPORTED",
                    issi,
                    e
                );
            }
        }

        // ETSI EN 300 392-2 V2.4.1 cl.14.7.2.5:
        // The BS does not support this supplementary service. Respond with
        // D-CMCE-FUNCTION-NOT-SUPPORTED, function_not_supported_pointer = 0
        // (the PDU type itself is not supported, not a specific field).
        let response = CmceFunctionNotSupported {
            not_supported_pdu_type: CmcePduTypeUl::UFacility.into_raw() as u8,
            call_identifier_present: false,
            call_identifier: None,
            function_not_supported_pointer: 0,
            length_of_received_pdu_extract: None,
            received_pdu_extract: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        if let Err(e) = response.to_bitbuf(&mut sdu) {
            tracing::error!("Failed to serialize D-CMCE-FUNCTION-NOT-SUPPORTED: {:?}", e);
            return;
        }
        sdu.seek(0);

        queue.push_back(SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: prim.handle,
                endpoint_id: prim.endpoint_id,
                link_id: prim.link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: TetraAddress::new(issi, SsiType::Issi),
                tx_reporter: None,
            }),
        });
    }

    fn encode_mnemonic(name: &str) -> String {
        let mut out = String::with_capacity(name.len().min(15));
        for ch in name.chars().take(15) {
            let cp = ch as u32;
            out.push(if cp <= 0xFF { ch } else { '?' });
        }
        out
    }

    fn resolve_traffic_delivery(&self, issi: u32) -> Option<(u16, u8, u8)> {
        let state = self.config.state_read();
        state.active_call_ts.get(&issi).copied().or_else(|| {
            state
                .subscribers
                .attached_groups_of(issi)
                .into_iter()
                .find_map(|gssi| state.active_call_ts.get(&gssi).copied())
        })
    }
}
