use crate::net_control::{ControlCommand, ControlEndpoint, ControlResponse};
use crate::net_telemetry::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{Sap, TdmaTime, unimplemented_log};
use tetra_saps::{SapMsg, SapMsgInner};

use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
use tetra_core::{BitBuffer, Layer2Service, TetraAddress, SsiType};
use tetra_saps::lcmc::LcmcMleUnitdataReq;

use super::subentities::cc_bs::CcBsSubentity;
use super::subentities::sds_bs::{SdsBsSubentity, SdsPendingAction};
use super::subentities::ss_bs::SsBsSubentity;

pub struct CmceBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    dashboard_control: Option<ControlEndpoint>,
    cc: CcBsSubentity,
    sds: SdsBsSubentity,
    ss: SsBsSubentity,
}

impl CmceBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let mut cc = CcBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry { cc.set_telemetry(sink.clone()); }
        let mut sds = SdsBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry { sds.set_telemetry(sink.clone()); }
        Self {
            config: config.clone(),
            telemetry,
            control,
            dashboard_control: None,
            sds,
            cc,
            ss: SsBsSubentity::new(),
        }
    }

    pub fn set_dashboard_control(&mut self, endpoint: ControlEndpoint) {
        self.dashboard_control = Some(endpoint);
    }

    /// Wire the control-command sender used by the built-in WX/METAR service to deliver
    /// replies (it re-injects SendSds commands from its background fetch thread).
    pub fn set_wx_cmd_sender(
        &mut self,
        tx: crossbeam_channel::Sender<ControlCommand>,
    ) {
        self.sds.set_wx_cmd_sender(tx);
    }

    fn do_control_command(
        sds: &mut SdsBsSubentity,
        cc: &mut CcBsSubentity,
        queue: &mut MessageQueue,
        cmd: ControlCommand,
        responder: Option<&ControlEndpoint>,
    ) {
        match cmd {
            ControlCommand::SendSds { handle, .. } => {
                let success = sds.rx_sds_from_control(queue, cmd);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::SendSdsResponse { handle, success });
                }
            }
            ControlCommand::KickMs { issi } => {
                let groups: Vec<u32> = cc.subscriber_groups_for(issi);
                if !groups.is_empty() {
                    use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
                    use tetra_core::Sap;
                    queue.push_back(tetra_saps::SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Mm,
                        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                            issi,
                            groups: groups.clone(),
                            action: BrewSubscriberAction::Deaffiliate,
                        }),
                    });
                }
                tracing::info!("CMCE: KickMs issi={} requested", issi);
                let success = cc.kick_ms(queue, issi);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::KickMsResponse { issi, success });
                }
            }
            ControlCommand::RestartService => {
                tracing::info!("CMCE: RestartService requested");
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Restart,
                    std::time::Duration::from_millis(500),
                );
            }
            ControlCommand::ShutdownService => {
                tracing::info!("CMCE: ShutdownService requested");
                crate::service_control::schedule_service_action(
                    crate::service_control::ServiceAction::Stop,
                    std::time::Duration::from_millis(500),
                );
            }
            ControlCommand::AddLiveSds { text, protocol_id, source_issi, repeat_count } => {
                let mut state = sds.shared_config().state_write();
                let id = state.next_live_sds_id;
                state.next_live_sds_id = state.next_live_sds_id.wrapping_add(1).max(1);
                state.live_sds_queue.push_back(
                    tetra_config::bluestation::LiveSdsMessage {
                        id,
                        text: text.clone(),
                        protocol_id,
                        source_issi,
                        repeat_count,
                        sent_count: 0,
                    }
                );
                tracing::info!(
                    "CMCE: AddLiveSds id={} repeat={} text={:?}",
                    id, repeat_count, text
                );
            }
            ControlCommand::DeleteLiveSds { id } => {
                let mut state = sds.shared_config().state_write();
                let before = state.live_sds_queue.len();
                state.live_sds_queue.retain(|m| m.id != id);
                let removed = before - state.live_sds_queue.len();
                tracing::info!("CMCE: DeleteLiveSds id={} removed={}", id, removed);
            }
            ControlCommand::ClearLiveSds => {
                let mut state = sds.shared_config().state_write();
                let n = state.live_sds_queue.len();
                state.live_sds_queue.clear();
                tracing::info!("CMCE: ClearLiveSds removed={}", n);
            }
            ControlCommand::ClearEmergency { issi } => {
                tracing::info!("CMCE: ClearEmergency issi={} (operator)", issi);
                sds.clear_emergency_command(issi);
            }
            ControlCommand::Dgna { issi, gssi, attach } => {
                // The dashboard control channel terminates at CMCE, but DGNA is a Mobility
                // Management procedure — group attach/detach state and the D-ATTACH/DETACH GROUP
                // IDENTITY send path both live in MM. Forward the request there.
                use tetra_core::Sap;
                tracing::info!(
                    "CMCE: forwarding DGNA {} of GSSI {} on ISSI {} to MM",
                    if attach { "assign" } else { "deassign" },
                    gssi,
                    issi
                );
                queue.push_back(tetra_saps::SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::MmDgnaRequest { issi, gssi, attach },
                });
            }
            _ => {
                tracing::warn!("CMCE: ignoring unsupported control command {:?}", cmd);
            }
        }
    }

    pub fn rx_lcmc_mle_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_ind");
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error"); return;
        };
        let Some(bits) = prim.sdu.peek_bits(5) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };
        let Ok(pdu_type) = CmcePduTypeUl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };
        match pdu_type {
            CmcePduTypeUl::UAlert
            | CmcePduTypeUl::UConnect
            | CmcePduTypeUl::UDisconnect
            | CmcePduTypeUl::UInfo
            | CmcePduTypeUl::URelease
            | CmcePduTypeUl::USetup
            | CmcePduTypeUl::UTxCeased
            | CmcePduTypeUl::UTxDemand
            | CmcePduTypeUl::UCallRestore => { self.cc.route_xx_deliver(queue, message); }
            CmcePduTypeUl::UStatus => { self.sds.route_status_deliver(queue, message); }
            CmcePduTypeUl::USdsData => { self.sds.route_rf_deliver(queue, message); }
            CmcePduTypeUl::UFacility => {
                // ETSI EN 300 392-2 §14.7.2.5:
                // BS does not support supplementary services (SS). Respond with
                // D-CMCE-FUNCTION-NOT-SUPPORTED, function_not_supported_pointer=0
                // (the PDU type itself is not supported, not a specific field).
                tracing::debug!("CMCE: received UFacility from ISSI {} — responding D-CMCE-FUNCTION-NOT-SUPPORTED",
                    prim.received_tetra_address.ssi);
                let response = CmceFunctionNotSupported {
                    not_supported_pdu_type: CmcePduTypeUl::UFacility.into_raw() as u8,
                    call_identifier_present: false,
                    call_identifier: None,
                    function_not_supported_pointer: 0,
                    length_of_received_pdu_extract: None,
                    received_pdu_extract: None,
                };
                let mut sdu = BitBuffer::new_autoexpand(16);
                if response.to_bitbuf(&mut sdu).is_ok() {
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
                            main_address: TetraAddress { ssi: prim.received_tetra_address.ssi, ssi_type: SsiType::Issi },
                            tx_reporter: None,
                        }),
                    });
                }
            }
            CmcePduTypeUl::CmceFunctionNotSupported => { unimplemented_log!("{:?}", pdu_type); }
        };
    }
}

impl TetraEntityTrait for CmceBs {
    fn entity(&self) -> TetraEntity { TetraEntity::Cmce }

    fn set_config(&mut self, config: SharedConfig) { self.config = config; }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.sds.tick_start(queue, ts);
        self.sds.tick_periodic_wx();
        let call_events = self.cc.tick_start_with_events(queue, ts);
        // Refresh the "who is on a traffic channel" map so SDS can FACCH-steal to MSs in a call.
        self.cc.publish_active_call_ts();
        if let Some(sink) = &self.telemetry {
            for event in call_events { sink.send(event); }
        }
        if let Some(cep) = &self.control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, Some(cep));
            }
        }
        if let Some(cep) = &self.dashboard_control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, None);
            }
        }
        // Drain SDS-triggered actions that require access to CcBsSubentity
        let pending = std::mem::take(&mut self.sds.pending_actions);
        for action in pending {
            match action {
                SdsPendingAction::KickAll => {
                    let issis: Vec<u32> = self.cc.subscriber_issis();
                    tracing::info!("SDS-CMD: kick_all — deregistering {} subscribers", issis.len());
                    for issi in issis {
                        self.cc.kick_ms(queue, issi);
                    }
                }
            }
        }

    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        match message.sap {
            Sap::LcmcSap => match message.msg {
                SapMsgInner::LcmcMleUnitdataInd(_) => { self.rx_lcmc_mle_unitdata_ind(queue, message); }
                _ => { tracing::warn!("CMCE: unexpected message on LcmcSap: {:?}, ignoring", message.msg); }
            },
            Sap::Control => match message.msg {
                SapMsgInner::CmceCallControl(_) => { self.cc.rx_call_control(queue, message); }
                SapMsgInner::MmSubscriberUpdate(update) => { self.cc.handle_subscriber_update(queue, update); }
                SapMsgInner::CmceSdsData(_) => { self.sds.rx_sds_from_brew(queue, message); }
                _ => { tracing::warn!("CMCE: unexpected control message: {:?}, ignoring", message.msg); }
            },
            Sap::TmdSap => {
                // UL voice frame — feed to echo session if active, and forward to Brew for FDX calls
                if let SapMsgInner::TmdCircuitDataInd(ref prim) = message.msg {
                    self.cc.handle_echo_ul_frame(queue, prim.ts, prim.data.clone());
                    // Emit TS activity for dashboard visualizer
                    if let Some(ref sink) = self.telemetry {
                        let _ = sink.send(crate::net_telemetry::TelemetryEvent::TsVoiceActivity { ts: prim.ts });
                    }
                    // Forward UL audio to Brew so TetraPack receives the terminal's voice
                    queue.push_back(message);
                }
            }
            _ => { tracing::warn!("CMCE: unexpected SAP {:?}, ignoring", message.sap); }
        }
    }
}
