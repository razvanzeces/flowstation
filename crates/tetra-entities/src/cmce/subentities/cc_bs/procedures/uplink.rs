use super::super::dtmf::{DtmfKind, decode_dtmf, pack_type3_bits_to_bytes};
use super::*;

#[derive(Clone, Copy)]
struct IndividualFloorParty {
    addr: TetraAddress,
    carrier_num: u16,
    ts: u8,
    usage: u8,
}

impl CcBsSubentity {
    fn individual_floor_parties(call: &IndividualCall, party: TetraAddress) -> Option<(IndividualFloorParty, IndividualFloorParty)> {
        if party.ssi == call.calling_addr.ssi {
            Some((
                IndividualFloorParty {
                    addr: call.calling_addr,
                    carrier_num: call.calling_carrier_num,
                    ts: call.calling_ts,
                    usage: call.calling_usage,
                },
                IndividualFloorParty {
                    addr: call.called_addr,
                    carrier_num: call.called_carrier_num,
                    ts: call.called_ts,
                    usage: call.called_usage,
                },
            ))
        } else if party.ssi == call.called_addr.ssi {
            Some((
                IndividualFloorParty {
                    addr: call.called_addr,
                    carrier_num: call.called_carrier_num,
                    ts: call.called_ts,
                    usage: call.called_usage,
                },
                IndividualFloorParty {
                    addr: call.calling_addr,
                    carrier_num: call.calling_carrier_num,
                    ts: call.calling_ts,
                    usage: call.calling_usage,
                },
            ))
        } else {
            None
        }
    }

    fn individual_floor_party_is_local(call: &IndividualCall, party: IndividualFloorParty) -> bool {
        !(call.calling_over_brew && party.addr.ssi == call.calling_addr.ssi
            || call.called_over_brew && party.addr.ssi == call.called_addr.ssi)
            && !(call.is_local_echo_call() && party.addr.ssi == LOCAL_ECHO_ISSI)
    }

    fn send_individual_d_tx_granted(
        &self,
        queue: &mut MessageQueue,
        call: &IndividualCall,
        call_id: u16,
        target: IndividualFloorParty,
        transmission_grant: TransmissionGrant,
        transmitting_party_issi: Option<u32>,
        ul_dl_assigned: UlDlAssignment,
    ) {
        if !Self::individual_floor_party_is_local(call, target) {
            tracing::trace!(
                "Skipping local D-TX GRANTED for remote individual party call_id={} ISSI {}",
                call_id,
                target.addr.ssi
            );
            return;
        }

        let d_tx_granted = DTxGranted {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: transmitting_party_issi.map(|_| 1),
            transmitting_party_address_ssi: transmitting_party_issi.map(|issi| issi as u64),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::info!(
            "FSM -> D-TX GRANTED (individual simplex, {}) call_id={} to ISSI {} ul_dl={}",
            transmission_grant,
            call_id,
            target.addr.ssi,
            ul_dl_assigned
        );
        let mut sdu = BitBuffer::new_autoexpand(50);
        d_tx_granted.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);

        let msg = Self::build_sapmsg_stealing_ul_dl(
            sdu,
            self.dltime,
            target.addr,
            target.carrier_num,
            target.ts,
            Some(target.usage),
            ul_dl_assigned,
        );
        queue.push_back(msg);
    }

    fn send_individual_d_tx_ceased(&self, queue: &mut MessageQueue, call: &IndividualCall, call_id: u16, target: IndividualFloorParty) {
        if !Self::individual_floor_party_is_local(call, target) {
            tracing::trace!(
                "Skipping local D-TX CEASED for remote individual party call_id={} ISSI {}",
                call_id,
                target.addr.ssi
            );
            return;
        }

        let d_tx_ceased = DTxCeased {
            call_identifier: call_id,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::info!(
            "FSM -> D-TX CEASED (individual simplex) call_id={} to ISSI {}",
            call_id,
            target.addr.ssi
        );
        let mut sdu = BitBuffer::new_autoexpand(30);
        d_tx_ceased.to_bitbuf(&mut sdu).expect("Failed to serialize DTxCeased");
        sdu.seek(0);

        let msg = Self::build_sapmsg_stealing_ul_dl(
            sdu,
            self.dltime,
            target.addr,
            target.carrier_num,
            target.ts,
            Some(target.usage),
            UlDlAssignment::Dl,
        );
        queue.push_back(msg);
    }

    fn notify_individual_floor_granted(
        &self,
        queue: &mut MessageQueue,
        call: &IndividualCall,
        call_id: u16,
        speaker: IndividualFloorParty,
        listener: IndividualFloorParty,
    ) {
        self.notify_floor_granted(
            queue,
            GroupFloorGrant {
                call_id,
                source_issi: speaker.addr.ssi,
                dest_gssi: listener.addr.ssi,
                carrier_num: speaker.carrier_num,
                ts: speaker.ts,
                is_group: false,
            },
            true,
            BrewNotification::Never,
        );

        if Self::individual_floor_party_is_local(call, speaker)
            && let Some(brew_uuid) = call.brew_uuid
            && (call.calling_over_brew || call.called_over_brew)
        {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: call.network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
                    brew_uuid,
                    grant: TransmissionGrant::Granted.into_raw() as u8,
                    permission: 0,
                }),
            });
        }
    }

    fn notify_individual_floor_released(
        &self,
        queue: &mut MessageQueue,
        call: &IndividualCall,
        call_id: u16,
        speaker: IndividualFloorParty,
    ) {
        self.notify_floor_released(
            queue,
            CallTimeslot {
                call_id,
                carrier_num: speaker.carrier_num,
                ts: speaker.ts,
            },
            true,
            BrewNotification::Never,
        );

        if Self::individual_floor_party_is_local(call, speaker)
            && let Some(brew_uuid) = call.brew_uuid
            && (call.calling_over_brew || call.called_over_brew)
        {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: call.network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
                    brew_uuid,
                    grant: TransmissionGrant::NotGranted.into_raw() as u8,
                    permission: 0,
                }),
            });
        }
    }

    fn brew_individual_floor_parties(call: &IndividualCall) -> Option<(IndividualFloorParty, IndividualFloorParty)> {
        if call.calling_over_brew && !call.called_over_brew {
            Self::individual_floor_parties(call, call.calling_addr)
        } else if call.called_over_brew && !call.calling_over_brew {
            Self::individual_floor_parties(call, call.called_addr)
        } else {
            None
        }
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_simplex_granted(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call_snapshot)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_GRANTED for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if !call_snapshot.is_active() || !call_snapshot.is_simplex() {
            tracing::trace!(
                "CMCE: ignoring Brew SIMPLEX_GRANTED uuid={} call_id={} active={} simplex={}",
                brew_uuid,
                call_id,
                call_snapshot.is_active(),
                call_snapshot.is_simplex()
            );
            return;
        }

        let remote_grant = TransmissionGrant::try_from((grant & 0x03) as u64).unwrap_or(TransmissionGrant::Granted);
        if remote_grant != TransmissionGrant::Granted {
            tracing::trace!(
                "CMCE: ignoring Brew SIMPLEX_GRANTED uuid={} call_id={} grant={:?}",
                brew_uuid,
                call_id,
                remote_grant
            );
            return;
        }

        let Some((remote_party, local_party)) = Self::brew_individual_floor_parties(&call_snapshot) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_GRANTED uuid={} call_id={} without one Brew party",
                brew_uuid,
                call_id
            );
            return;
        };

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.grant_floor(remote_party.addr);
        }

        tracing::info!(
            "CMCE: Brew SIMPLEX_GRANTED uuid={} call_id={} remote_issi={} local_issi={}",
            brew_uuid,
            call_id,
            remote_party.addr.ssi,
            local_party.addr.ssi
        );
        self.send_individual_d_tx_granted(
            queue,
            &call_snapshot,
            call_id,
            local_party,
            TransmissionGrant::GrantedToOtherUser,
            Some(remote_party.addr.ssi),
            UlDlAssignment::Dl,
        );
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted {
                call_id,
                carrier_num: local_party.carrier_num,
                ts: local_party.ts,
            }),
        });
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_simplex_idle(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call_snapshot)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_IDLE for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if !call_snapshot.is_active() || !call_snapshot.is_simplex() {
            tracing::trace!(
                "CMCE: ignoring Brew SIMPLEX_IDLE uuid={} call_id={} active={} simplex={}",
                brew_uuid,
                call_id,
                call_snapshot.is_active(),
                call_snapshot.is_simplex()
            );
            return;
        }

        let Some((remote_party, local_party)) = Self::brew_individual_floor_parties(&call_snapshot) else {
            tracing::debug!(
                "CMCE: Brew SIMPLEX_IDLE uuid={} call_id={} without one Brew party",
                brew_uuid,
                call_id
            );
            return;
        };

        if !call_snapshot.is_floor_held_by(remote_party.addr.ssi) {
            tracing::trace!(
                "CMCE: ignoring Brew SIMPLEX_IDLE uuid={} call_id={} floor_holder={:?}",
                brew_uuid,
                call_id,
                call_snapshot.floor_holder
            );
            return;
        }

        tracing::info!(
            "CMCE: Brew SIMPLEX_IDLE uuid={} call_id={} remote_issi={} grant={} permission={}",
            brew_uuid,
            call_id,
            remote_party.addr.ssi,
            grant,
            permission
        );

        let queued_request = self
            .individual_calls
            .get_mut(&call_id)
            .and_then(IndividualCall::take_queued_tx_demand);

        if let Some(requester) = queued_request {
            let Some((requester_party, former_speaker_party)) = Self::individual_floor_parties(&call_snapshot, requester) else {
                tracing::warn!(
                    "CMCE: Brew SIMPLEX_IDLE call_id={} had queued non-participant ISSI {}, dropping request",
                    call_id,
                    requester.ssi
                );
                if let Some(call) = self.individual_calls.get_mut(&call_id) {
                    call.release_floor();
                }
                self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, local_party);
                self.notify_floor_released(
                    queue,
                    CallTimeslot {
                        call_id,
                        carrier_num: local_party.carrier_num,
                        ts: local_party.ts,
                    },
                    true,
                    BrewNotification::Never,
                );
                return;
            };

            if let Some(call) = self.individual_calls.get_mut(&call_id) {
                call.grant_floor(requester);
            }

            self.send_individual_d_tx_granted(
                queue,
                &call_snapshot,
                call_id,
                requester_party,
                TransmissionGrant::Granted,
                Some(requester_party.addr.ssi),
                UlDlAssignment::Ul,
            );
            self.send_individual_d_tx_granted(
                queue,
                &call_snapshot,
                call_id,
                former_speaker_party,
                TransmissionGrant::GrantedToOtherUser,
                Some(requester_party.addr.ssi),
                UlDlAssignment::Dl,
            );
            self.notify_individual_floor_granted(queue, &call_snapshot, call_id, requester_party, former_speaker_party);
            return;
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.release_floor();
        }
        self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, local_party);
        self.notify_floor_released(
            queue,
            CallTimeslot {
                call_id,
                carrier_num: local_party.carrier_num,
                ts: local_party.ts,
            },
            true,
            BrewNotification::Never,
        );
    }

    /// Handle parsed U-SETUP and dispatch into group/individual FSM paths.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        // Check if we can satisfy this request
        if !Self::feature_check_u_setup(pdu) {
            tracing::info!(
                "CMCE: rejecting U-SETUP from ISSI {} due to unsupported critical feature(s)",
                calling_party.ssi
            );
            let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
                panic!()
            };
            let reject_call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(reject_call_id, DisconnectCause::IncompatibleTrafficCase);
            let msg = Self::build_sapmsg_direct(sdu, self.dltime, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        // Handle P2P (individual) call setup separately
        if pdu.basic_service_information.communication_type == CommunicationType::P2p {
            self.fsm_on_u_setup_p2p(queue, message, pdu, calling_party);
            return;
        }
        self.fsm_on_u_setup_group(queue, message, pdu, calling_party);
    }

    /// Handle parsed U-TX CEASED.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_tx_ceased(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        pdu: UTxCeased,
    ) {
        let call_id = pdu.call_identifier;

        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            if !call_snapshot.is_active() {
                tracing::debug!("U-TX CEASED for inactive individual call_id={}, ignoring", call_id);
                return;
            }

            if !call_snapshot.is_simplex() {
                tracing::debug!("U-TX CEASED for duplex individual call_id={}, ignoring", call_id);
                return;
            }

            let Some((sender_party, peer_party)) = Self::individual_floor_parties(&call_snapshot, sender) else {
                tracing::warn!(
                    "U-TX CEASED for individual call_id={} from non-participant ISSI {}, ignoring",
                    call_id,
                    sender.ssi
                );
                return;
            };

            if call_snapshot.floor_holder.is_some() && !call_snapshot.is_floor_held_by(sender.ssi) {
                if let Some(call) = self.individual_calls.get_mut(&call_id)
                    && call.cancel_queued_tx_demand(sender)
                {
                    tracing::info!(
                        "U-TX CEASED (individual simplex) call_id={} from queued ISSI {}, cancelled queued request",
                        call_id,
                        sender.ssi
                    );
                    return;
                }

                tracing::debug!(
                    "U-TX CEASED (individual simplex) call_id={} from ISSI {} without floor holder match {:?}, ignoring",
                    call_id,
                    sender.ssi,
                    call_snapshot.floor_holder
                );
                return;
            }

            let queued_request = self
                .individual_calls
                .get_mut(&call_id)
                .and_then(IndividualCall::take_queued_tx_demand);

            if let Some(requester) = queued_request {
                let Some((requester_party, former_speaker_party)) = Self::individual_floor_parties(&call_snapshot, requester) else {
                    tracing::warn!(
                        "U-TX CEASED individual call_id={} had queued non-participant ISSI {}, dropping request",
                        call_id,
                        requester.ssi
                    );
                    if let Some(call) = self.individual_calls.get_mut(&call_id) {
                        call.release_floor();
                    }
                    self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, sender_party);
                    self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, peer_party);
                    self.notify_individual_floor_released(queue, &call_snapshot, call_id, sender_party);
                    return;
                };

                if let Some(call) = self.individual_calls.get_mut(&call_id) {
                    call.grant_floor(requester);
                }

                self.send_individual_d_tx_granted(
                    queue,
                    &call_snapshot,
                    call_id,
                    requester_party,
                    TransmissionGrant::Granted,
                    Some(requester_party.addr.ssi),
                    UlDlAssignment::Ul,
                );
                self.send_individual_d_tx_granted(
                    queue,
                    &call_snapshot,
                    call_id,
                    former_speaker_party,
                    TransmissionGrant::GrantedToOtherUser,
                    Some(requester_party.addr.ssi),
                    UlDlAssignment::Dl,
                );
                self.notify_individual_floor_granted(queue, &call_snapshot, call_id, requester_party, former_speaker_party);
                return;
            }

            self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, sender_party);
            self.send_individual_d_tx_ceased(queue, &call_snapshot, call_id, peer_party);
            if let Some(call) = self.individual_calls.get_mut(&call_id) {
                call.release_floor();
            }
            self.notify_individual_floor_released(queue, &call_snapshot, call_id, sender_party);
            return;
        }

        if let Err(err) = self.fsm_group_on_tx_ceased(queue, call_id, sender) {
            match err {
                GroupTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-TX CEASED for unknown call_id={}", call_id);
                }
                GroupTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!(
                        "U-TX CEASED ignored for call_id={} due to invalid transition in state {:?}",
                        call_id,
                        state
                    );
                }
                GroupTransitionError::NotCurrentSpeaker {
                    sender_issi,
                    current_speaker_issi,
                    ..
                } => {
                    tracing::warn!(
                        "U-TX CEASED from non-current speaker ISSI {} on call_id={} (current speaker={}), ignoring",
                        sender_issi,
                        call_id,
                        current_speaker_issi
                    );
                }
                GroupTransitionError::MissingCachedSetup(_) => {
                    tracing::error!("U-TX CEASED call_id={} missing cached D-SETUP", call_id);
                }
            }
        }
    }

    /// Handle parsed U-TX DEMAND.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_tx_demand(
        &mut self,
        queue: &mut MessageQueue,
        requesting_party: TetraAddress,
        pdu: UTxDemand,
    ) {
        let call_id = pdu.call_identifier;

        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            if !call_snapshot.is_active() {
                tracing::debug!("U-TX DEMAND for inactive individual call_id={}, ignoring", call_id);
                return;
            }

            if !call_snapshot.is_simplex() {
                tracing::debug!("U-TX DEMAND for duplex individual call_id={}, ignoring", call_id);
                return;
            }

            let Some((requester_party, peer_party)) = Self::individual_floor_parties(&call_snapshot, requesting_party) else {
                tracing::warn!(
                    "U-TX DEMAND for individual call_id={} from non-participant ISSI {}, ignoring",
                    call_id,
                    requesting_party.ssi
                );
                return;
            };

            tracing::info!(
                "U-TX DEMAND (individual simplex) call_id={} from ISSI {} priority={} floor_holder={:?}",
                call_id,
                requesting_party.ssi,
                pdu.tx_demand_priority,
                call_snapshot.floor_holder
            );

            match call_snapshot.floor_holder {
                None => {
                    if let Some(call) = self.individual_calls.get_mut(&call_id) {
                        call.grant_floor(requesting_party);
                    }
                    self.send_individual_d_tx_granted(
                        queue,
                        &call_snapshot,
                        call_id,
                        requester_party,
                        TransmissionGrant::Granted,
                        Some(requesting_party.ssi),
                        UlDlAssignment::Ul,
                    );
                    self.send_individual_d_tx_granted(
                        queue,
                        &call_snapshot,
                        call_id,
                        peer_party,
                        TransmissionGrant::GrantedToOtherUser,
                        Some(requesting_party.ssi),
                        UlDlAssignment::Dl,
                    );
                    self.notify_individual_floor_granted(queue, &call_snapshot, call_id, requester_party, peer_party);
                }
                Some(holder) if holder == requesting_party.ssi => {
                    self.send_individual_d_tx_granted(
                        queue,
                        &call_snapshot,
                        call_id,
                        requester_party,
                        TransmissionGrant::Granted,
                        Some(requesting_party.ssi),
                        UlDlAssignment::Ul,
                    );
                }
                Some(holder) => {
                    let queue_result = self
                        .individual_calls
                        .get_mut(&call_id)
                        .map(|call| call.queue_tx_demand(requesting_party))
                        .unwrap_or(TxDemandQueueResult::QueueBusy);

                    let grant = match queue_result {
                        TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => TransmissionGrant::RequestQueued,
                        TxDemandQueueResult::QueueBusy => TransmissionGrant::NotGranted,
                        TxDemandQueueResult::FromCurrentSpeaker => TransmissionGrant::Granted,
                    };

                    self.send_individual_d_tx_granted(
                        queue,
                        &call_snapshot,
                        call_id,
                        requester_party,
                        grant,
                        Some(holder),
                        UlDlAssignment::Dl,
                    );
                }
            }
            return;
        }

        tracing::info!("U-TX DEMAND: ISSI {} requests floor on call_id={}", requesting_party.ssi, call_id);
        if let Err(err) = self.fsm_group_on_tx_demand(queue, call_id, requesting_party) {
            match err {
                GroupTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-TX DEMAND for unknown call_id={}", call_id);
                }
                GroupTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!(
                        "U-TX DEMAND ignored for call_id={} due to invalid transition in state {:?}",
                        call_id,
                        state
                    );
                }
                GroupTransitionError::MissingCachedSetup(_) => {
                    tracing::error!("U-TX DEMAND call_id={} missing cached D-SETUP", call_id);
                }
                GroupTransitionError::NotCurrentSpeaker { .. } => {
                    tracing::debug!("U-TX DEMAND hit unexpected NotCurrentSpeaker transition error call_id={}", call_id);
                }
            }
        }
    }

    /// Handle parsed U-INFO.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_info(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UInfo,
    ) {
        let call_id = pdu.call_identifier;
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            tracing::warn!("U-INFO for unknown/non-individual call_id={}, rejecting", call_id);
            let sdu = Self::build_d_release(call_id, DisconnectCause::InvalidCallIdentifier);
            queue.push_back(Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id));
            return;
        };

        if !call.is_active() {
            tracing::debug!(
                "U-INFO for non-active individual call_id={} state={:?}, ignoring",
                call_id,
                call.state
            );
            return;
        }

        if let Some(call_mut) = self.individual_calls.get_mut(&call_id) {
            call_mut.active_timer_started = Some(self.dltime);
        }

        if pdu.facility.is_some() || pdu.proprietary.is_some() {
            unimplemented_log!(
                "U-INFO facility/proprietary not supported call_id={} facility={} proprietary={}",
                call_id,
                pdu.facility.is_some(),
                pdu.proprietary.is_some()
            );
        }

        if let Some(modify) = pdu.modify {
            self.fsm_on_u_info_modify(queue, sender, &call, call_id, modify);
        }

        let Some(dtmf) = pdu.dtmf.as_ref() else {
            tracing::trace!(
                "U-INFO call_id={} has no DTMF element (modify={:?} facility={} proprietary={})",
                call_id,
                pdu.modify,
                pdu.facility.is_some(),
                pdu.proprietary.is_some()
            );
            return;
        };

        if !call.called_over_brew && !call.calling_over_brew {
            tracing::trace!("U-INFO call_id={} is local individual call; DTMF is not forwarded", call_id);
            return;
        }

        let Some(brew_uuid) = call.brew_uuid else {
            tracing::warn!("U-INFO call_id={} marked Brew-routed but missing brew_uuid", call_id);
            return;
        };

        let decoded = decode_dtmf(dtmf);
        if decoded.full_len_bits > decoded.parsed_bits {
            tracing::warn!(
                "U-INFO call_id={} DTMF payload is {} bits, parser retained only first {} bits",
                call_id,
                decoded.full_len_bits,
                decoded.parsed_bits
            );
        }
        if decoded.malformed {
            tracing::warn!(
                "U-INFO call_id={} has malformed DTMF payload (len={} bits, parsed={} bits, data={:?}, kind={:?})",
                call_id,
                decoded.full_len_bits,
                decoded.parsed_bits,
                dtmf.data,
                decoded.kind
            );
        }

        match decoded.kind {
            DtmfKind::ToneStart | DtmfKind::LegacyDigits => {}
            DtmfKind::ToneEnd => {
                tracing::trace!("U-INFO call_id={} DTMF tone end", call_id);
                return;
            }
            DtmfKind::NotSupported => {
                tracing::info!("U-INFO call_id={} DTMF not supported indication", call_id);
                return;
            }
            DtmfKind::NotSubscribed => {
                tracing::info!("U-INFO call_id={} DTMF not subscribed indication", call_id);
                return;
            }
            DtmfKind::Reserved(v) => {
                tracing::trace!("U-INFO call_id={} DTMF reserved type value {}", call_id, v);
                return;
            }
            DtmfKind::Invalid => {
                tracing::trace!("U-INFO call_id={} invalid/empty DTMF payload, ignoring", call_id);
                return;
            }
        }
        if decoded.digits.is_empty() {
            tracing::trace!("U-INFO call_id={} DTMF has no decoded digits, ignoring", call_id);
            return;
        }

        let (length_bits, data) = pack_type3_bits_to_bytes(dtmf);
        if length_bits == 0 || data.is_empty() {
            tracing::debug!("U-INFO call_id={} has empty DTMF payload, ignoring", call_id);
            return;
        }

        tracing::info!(
            "U-INFO (individual Brew) call_id={} uuid={} dtmf_kind={:?} digits='{}' dtmf_bits={} dtmf_bytes={}",
            call_id,
            brew_uuid,
            decoded.kind,
            decoded.digits,
            length_bits,
            data.len()
        );

        for ch in decoded.digits.chars() {
            let digit = ch as u8;

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: call.network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf {
                    brew_uuid,
                    length_bits: 8,
                    data: vec![digit],
                }),
            });
        }
    }

    fn fsm_on_u_info_modify(&mut self, queue: &mut MessageQueue, sender: TetraAddress, call: &IndividualCall, call_id: u16, modify: u64) {
        if call.called_over_brew || call.calling_over_brew {
            unimplemented_log!("U-INFO modify over Brew not supported call_id={} modify=0x{:03x}", call_id, modify);
            return;
        }

        if let Some(call_mut) = self.individual_calls.get_mut(&call_id)
            && call_mut.apply_modify().is_err()
        {
            tracing::debug!("U-INFO modify rejected by formal FSM call_id={}", call_id);
            return;
        }

        let (target_addr, target_carrier_num, target_ts, target_usage) = if sender.ssi == call.calling_addr.ssi {
            (call.called_addr, call.called_carrier_num, call.called_ts, call.called_usage)
        } else if sender.ssi == call.called_addr.ssi {
            (call.calling_addr, call.calling_carrier_num, call.calling_ts, call.calling_usage)
        } else {
            tracing::warn!(
                "U-INFO modify call_id={} from unexpected ISSI {} (calling {}, called {})",
                call_id,
                sender.ssi,
                call.calling_addr.ssi,
                call.called_addr.ssi
            );
            return;
        };

        tracing::info!(
            "CMCE: forwarding U-INFO modify call_id={} modify=0x{:03x} from ISSI {} to ISSI {}",
            call_id,
            modify,
            sender.ssi,
            target_addr.ssi
        );
        let sdu = Self::build_d_info(call_id, Some(modify), Some(CallStatus::Callcontinue), true);
        let msg = Self::build_sapmsg_stealing(sdu, self.dltime, target_addr, target_carrier_num, target_ts, Some(target_usage));
        queue.push_back(msg);
    }

    /// Handle parsed U-RELEASE.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_release(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: URelease,
    ) {
        let call_id = pdu.call_identifier;
        let disconnect_cause = pdu.disconnect_cause;

        tracing::info!("U-RELEASE: call_id={} cause={}", call_id, disconnect_cause);
        if self.individual_calls.contains_key(&call_id) {
            tracing::info!("U-RELEASE (individual) call_id={} cause={}", call_id, disconnect_cause);
            self.release_individual_call(queue, call_id, disconnect_cause);
        } else if self.active_calls.contains_key(&call_id) || self.cached_setups.contains_key(&call_id) {
            self.release_group_call(queue, call_id, disconnect_cause);
        } else {
            tracing::debug!(
                "U-RELEASE for unknown call_id={} (likely duplicate), completing idempotently",
                call_id
            );
            let sdu = Self::build_d_release(call_id, disconnect_cause);
            queue.push_back(Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id));
        }
    }

    /// Handle parsed U-DISCONNECT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_disconnect(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        ul_handle: u32,
        ul_link_id: u32,
        ul_endpoint_id: u32,
        pdu: UDisconnect,
    ) {
        let call_id = pdu.call_identifier;
        let disconnect_cause = pdu.disconnect_cause;

        if self.individual_calls.contains_key(&call_id) {
            tracing::info!("U-DISCONNECT (individual) call_id={} cause={}", call_id, disconnect_cause);
            if let Some(call) = self.individual_calls.get_mut(&call_id) {
                call.begin_disconnect();
            }
            self.release_individual_call_from_u_disconnect(queue, call_id, disconnect_cause, sender.ssi);
            return;
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            tracing::debug!(
                "U-DISCONNECT for unknown call_id={} (likely duplicate), completing idempotently",
                call_id
            );
            let sdu = Self::build_d_release(call_id, disconnect_cause);
            queue.push_back(Self::build_sapmsg_direct(
                sdu,
                self.dltime,
                sender,
                ul_handle,
                ul_link_id,
                ul_endpoint_id,
            ));
            return;
        };

        let is_call_owner = matches!(&call.origin, CallOrigin::Local { caller_addr } if caller_addr.ssi == sender.ssi);

        if is_call_owner {
            tracing::info!("U-DISCONNECT: call owner ISSI {} disconnecting call_id={}", sender.ssi, call_id);
            if let Some(call) = self.active_calls.get_mut(&call_id) {
                call.begin_disconnect();
            }
            self.release_group_call(queue, call_id, DisconnectCause::UserRequestedDisconnection);
            return;
        }

        tracing::info!(
            "U-DISCONNECT: non-call-owner ISSI {} rejected for call_id={} cause={}",
            sender.ssi,
            call_id,
            disconnect_cause
        );

        let d_release = DRelease {
            call_identifier: call_id,
            disconnect_cause: DisconnectCause::RequestedServiceNotAvailable,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?} (to ISSI {})", d_release, sender.ssi);

        let mut sdu = BitBuffer::new_autoexpand(32);
        d_release.to_bitbuf(&mut sdu).expect("Failed to serialize DRelease");
        sdu.seek(0);

        let sender_addr = TetraAddress::new(sender.ssi, SsiType::Issi);
        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: ul_handle,
                endpoint_id: ul_endpoint_id,
                link_id: ul_link_id,
                // D-RELEASE to the disconnecting MS: the legacy `main` code sent CC PDUs
                // unacknowledged (FH FIX 2).
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: sender_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }
}
