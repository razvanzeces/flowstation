use crate::net_control::{ControlCommand, ControlEndpoint, ControlResponse};
use crate::net_telemetry::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{Sap, TdmaTime, unimplemented_log};
use tetra_saps::control::brew::BrewSubscriberAction;
use tetra_saps::{SapMsg, SapMsgInner};

use super::components::pc_bs::{ControlRoute, LcmcRoute, PcBs};
use super::subentities::cc_bs::CcBsSubentity;
use super::subentities::sds_bs::{SdsBsSubentity, SdsPendingAction};
use super::subentities::ss_bs::SsBsSubentity;

pub struct CmceBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    dashboard_control: Option<ControlEndpoint>,

    pc: PcBs,
    cc: CcBsSubentity,
    sds: SdsBsSubentity,
    ss: SsBsSubentity,
}

impl CmceBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let mut sds = SdsBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry {
            sds.set_telemetry(sink.clone());
        }

        let mut cc = CcBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry {
            cc.set_telemetry(sink.clone());
        }

        let mut ss = SsBsSubentity::new(config.clone());
        if let Some(ref sink) = telemetry {
            ss.set_telemetry(sink.clone());
        }

        Self {
            config: config.clone(),
            telemetry,
            control,
            dashboard_control: None,
            pc: PcBs::new(),
            sds,
            cc,
            ss,
        }
    }

    pub fn set_dashboard_control(&mut self, endpoint: ControlEndpoint) {
        self.dashboard_control = Some(endpoint);
    }

    pub fn set_wx_cmd_sender(&mut self, tx: crossbeam_channel::Sender<ControlCommand>) {
        self.sds.set_wx_cmd_sender(tx);
    }

    /// Execute a single control command. Shared by both the main `control` link (where a
    /// `responder` is supplied so request/response commands can reply) and the dashboard
    /// control link (where `responder` is `None`). Unknown commands are logged, never panic —
    /// a control-plane peer must not be able to crash the base station.
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
            ControlCommand::SendRawSdsType4 { handle, .. } => {
                // FH-BUG-052: TPG2200 / DAPNET / GeoAlarm Call-Outs arrive as an already-built
                // Type-4 SDU (first byte is its own protocol id, e.g. 0xC3). Deliver it verbatim,
                // WITHOUT re-wrapping in the SDS-TL simple-text header the SendSds path adds.
                let success = sds.rx_raw_sds_type4_from_control(queue, cmd);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::SendSdsResponse { handle, success });
                }
            }
            ControlCommand::KickMs { issi } => {
                tracing::info!("CMCE: KickMs issi={} requested", issi);
                let success = cc.kick_ms(queue, issi);
                if let Some(cep) = responder {
                    cep.respond(ControlResponse::KickMsResponse { issi, success });
                }
            }
            ControlCommand::Dgna {
                issi,
                gssi,
                mnemonic,
                attachment_mode,
                attach,
            } => {
                // The dashboard control channel terminates at CMCE, but DGNA is a Mobility
                // Management procedure — group attach/detach state and the D-ATTACH/DETACH GROUP
                // IDENTITY send path both live in MM. Forward the request there.
                tracing::info!(
                    "CMCE: forwarding DGNA {} of GSSI {} on ISSI {} to MM (mnemonic={:?})",
                    if attach { "assign" } else { "deassign" },
                    gssi,
                    issi,
                    mnemonic
                );
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Mm,
                    msg: SapMsgInner::MmDgnaRequest {
                        issi,
                        gssi,
                        mnemonic,
                        attachment_mode,
                        attach,
                    },
                });
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
            ControlCommand::AddLiveSds {
                text,
                protocol_id,
                source_issi,
                repeat_count,
            } => {
                let mut state = sds.shared_config().state_write();
                let id = state.next_live_sds_id;
                state.next_live_sds_id = state.next_live_sds_id.wrapping_add(1).max(1);
                state.live_sds_queue.push_back(tetra_config::bluestation::LiveSdsMessage {
                    id,
                    text: text.clone(),
                    protocol_id,
                    source_issi,
                    repeat_count,
                    sent_count: 0,
                });
                tracing::info!("CMCE: AddLiveSds id={} repeat={} text={:?}", id, repeat_count, text);
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
            _ => {
                tracing::warn!("CMCE: ignoring unsupported control command {:?}", cmd);
            }
        }
    }

    pub fn rx_lcmc_mle_unitdata_ind(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_lcmc_mle_unitdata_ind");

        let Some(route) = self.pc.route_lcmc_unitdata_ind(&mut message) else {
            return;
        };

        match route {
            LcmcRoute::CcRd => {
                self.cc.route_rd_deliver(_queue, message);
            }
            LcmcRoute::SdsStatus => {
                self.sds.route_status_deliver(_queue, message);
            }
            LcmcRoute::SdsRf => {
                self.sds.route_rf_deliver(_queue, message);
            }
            LcmcRoute::SsRe => {
                self.ss.route_re_deliver(_queue, message);
            }
            LcmcRoute::Unsupported(pdu_type) => {
                unimplemented_log!("{:?}", pdu_type);
            }
        };
    }
}

impl TetraEntityTrait for CmceBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Cmce
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config.clone();
        self.cc.set_config(config.clone());
        self.ss.set_config(config);
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        // Propagate tick to subentities
        self.cc.tick_start(queue, ts);
        // Republish the in-call ISSI→timeslot map so SDS can FACCH-steal to in-call radios
        // (FH-BUG-034). Rebuilt from the live call tables every tick.
        self.cc.publish_active_call_ts();
        self.sds.tick_start(queue, ts);
        self.sds.tick_periodic_wx();

        // Process incoming control commands, if the main control link is enabled (request/response).
        if let Some(cep) = &self.control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, Some(cep));
            }
        }
        // Process commands from the dashboard control link (fire-and-forget, no responder).
        if let Some(cep) = &self.dashboard_control {
            while let Some(cmd) = cep.try_recv() {
                CmceBs::do_control_command(&mut self.sds, &mut self.cc, queue, cmd, None);
            }
        }

        // Drain SDS-triggered actions that require access to CcBsSubentity.
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
        tracing::trace!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::LcmcSap => match message.msg {
                SapMsgInner::LcmcMleUnitdataInd(_) => {
                    self.rx_lcmc_mle_unitdata_ind(queue, message);
                }
                _ => {
                    panic!("Unexpected message on LcmcSap: {:?}", message.msg);
                }
            },
            Sap::Control => match self.pc.route_control(&message) {
                ControlRoute::CcRa => {
                    self.cc.rx_call_control(queue, message);
                }
                ControlRoute::CcSubscriberUpdate => {
                    let source = message.src;
                    let SapMsgInner::MmSubscriberUpdate(update) = message.msg else {
                        unreachable!();
                    };
                    if source == TetraEntity::Brew {
                        if !crate::net_brew::is_brew_external_subscriber_allowed(&self.config, update.issi) {
                            tracing::trace!(
                                "CMCE: ignoring Brew subscriber update issi={} action={:?}",
                                update.issi,
                                update.action
                            );
                            return;
                        }
                        if update.action == BrewSubscriberAction::Register && update.groups.is_empty() {
                            tracing::trace!("CMCE: ignoring Brew presence-only register issi={}", update.issi);
                            return;
                        }
                    }
                    self.cc.handle_subscriber_update(queue, update);
                }
                ControlRoute::SdsRc => {
                    self.sds.rx_sds_from_brew(queue, message);
                }
                ControlRoute::SsDgnaAssign => {
                    let SapMsgInner::CmceSsDgnaAssign {
                        issi,
                        gssi,
                        mnemonic,
                        attachment_mode,
                        attach,
                    } = message.msg
                    else {
                        unreachable!();
                    };
                    // MM owns the group registry/affiliation; it has already committed the change and
                    // asks CMCE only to put the SS-DGNA ASSIGN/DEASSIGN on the air as a D-FACILITY.
                    self.ss.send_d_facility_dgna(queue, issi, gssi, mnemonic, attachment_mode, attach);
                }
                ControlRoute::Unsupported => {
                    panic!("Unexpected control message: {:?}", message.msg);
                }
            },
            _ => {
                panic!("Unexpected SAP: {:?}", message.sap);
            }
        }
    }
}
