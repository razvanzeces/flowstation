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
use super::subentities::sds_bs::SdsBsSubentity;
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
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = std::process::Command::new("systemctl")
                        .args(["restart", "tetra"])
                        .status();
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
        let call_events = self.cc.tick_start_with_events(queue, ts);
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
                    // Forward UL audio to Brew so TetraPack receives the terminal's voice
                    queue.push_back(message);
                }
            }
            _ => { tracing::warn!("CMCE: unexpected SAP {:?}, ignoring", message.sap); }
        }
    }
}
