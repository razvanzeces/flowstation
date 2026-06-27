use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cmce::subentities::cc_bs) enum GroupEvent {
    TxDemand,
    TxCeased,
    NetworkCallStart,
    NetworkCallEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::cmce::subentities::cc_bs) enum GroupTransitionError {
    UnknownCall(u16),
    InvalidTransition {
        call_id: u16,
        state: GroupCallState,
        formal_state: CcFormalState,
        event: GroupEvent,
    },
    NotCurrentSpeaker {
        call_id: u16,
        sender_issi: u32,
        current_speaker_issi: u32,
    },
    MissingCachedSetup(u16),
}

impl CcBsSubentity {
    fn validate_group_transition(
        call_id: u16,
        state: GroupCallState,
        formal_state: CcFormalState,
        event: GroupEvent,
    ) -> Result<(), GroupTransitionError> {
        let allowed = state.formal_state() == formal_state
            && matches!(
                (formal_state, state, event),
                (CcFormalState::Active, GroupCallState::Transmitting, GroupEvent::TxDemand)
                    | (CcFormalState::Active, GroupCallState::NoActiveSpeaker { .. }, GroupEvent::TxDemand)
                    | (CcFormalState::Active, GroupCallState::Transmitting, GroupEvent::TxCeased)
                    | (CcFormalState::Active, GroupCallState::Transmitting, GroupEvent::NetworkCallStart)
                    | (
                        CcFormalState::Active,
                        GroupCallState::NoActiveSpeaker { .. },
                        GroupEvent::NetworkCallStart
                    )
                    | (CcFormalState::Active, GroupCallState::Transmitting, GroupEvent::NetworkCallEnd)
                    | (
                        CcFormalState::Active,
                        GroupCallState::NoActiveSpeaker { .. },
                        GroupEvent::NetworkCallEnd
                    )
            );
        if allowed {
            Ok(())
        } else {
            Err(GroupTransitionError::InvalidTransition {
                call_id,
                state,
                formal_state,
                event,
            })
        }
    }

    fn fsm_send_d_tx_granted_individual(
        &self,
        queue: &mut MessageQueue,
        call_id: u16,
        target_addr: TetraAddress,
        carrier_num: u16,
        ts: u8,
        transmission_grant: TransmissionGrant,
        transmitting_party_issi: Option<u32>,
    ) {
        let d_tx_granted = DTxGranted {
            call_identifier: call_id,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: transmitting_party_issi.map(|_| 1), // SSI
            transmitting_party_address_ssi: transmitting_party_issi.map(|ssi| ssi as u64),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::info!(
            "FSM -> D-TX GRANTED (individual, {}) call_id={} to ISSI {}",
            transmission_grant,
            call_id,
            target_addr.ssi
        );
        let mut sdu = BitBuffer::new_autoexpand(50);
        d_tx_granted.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);

        let msg = Self::build_sapmsg_stealing(sdu, self.dltime, target_addr, carrier_num, ts, None);
        queue.push_back(msg);
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_tx_demand(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        requesting_party: TetraAddress,
    ) -> Result<(), GroupTransitionError> {
        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        let formal_state = call.formal_state;
        Self::validate_group_transition(call_id, state, formal_state, GroupEvent::TxDemand)?;

        let ts = call.ts;
        let carrier_num = call.carrier_num;
        let dest_ssi = call.dest_gssi;
        let current_speaker = call.source_issi;
        let grant_now = matches!(state, GroupCallState::NoActiveSpeaker { .. });
        let queue_result = if grant_now {
            call.grant_floor(requesting_party.ssi, Some(requesting_party));
            None
        } else {
            Some(call.queue_tx_demand(requesting_party))
        };

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        if let Some(queue_result) = queue_result {
            match queue_result {
                TxDemandQueueResult::FromCurrentSpeaker => {
                    tracing::trace!(
                        "FSM: U-TX DEMAND call_id={} from current speaker ISSI {}, ignoring duplicate",
                        call_id,
                        requesting_party.ssi
                    );
                }
                TxDemandQueueResult::Queued | TxDemandQueueResult::AlreadyQueuedBySameUser => {
                    // Non-pre-emptive: keep current speaker active, queue requester.
                    self.fsm_send_d_tx_granted_individual(
                        queue,
                        call_id,
                        requesting_party,
                        carrier_num,
                        ts,
                        TransmissionGrant::RequestQueued,
                        Some(current_speaker),
                    );
                }
                TxDemandQueueResult::QueueBusy => {
                    self.fsm_send_d_tx_granted_individual(
                        queue,
                        call_id,
                        requesting_party,
                        carrier_num,
                        ts,
                        TransmissionGrant::NotGranted,
                        Some(current_speaker),
                    );
                }
            }
            return Ok(());
        }

        // NoActiveSpeaker -> Transmitting transition with granted floor.
        self.fsm_send_d_tx_granted_individual(
            queue,
            call_id,
            requesting_party,
            carrier_num,
            ts,
            TransmissionGrant::Granted,
            Some(requesting_party.ssi),
        );
        self.send_d_tx_granted_facch(queue, call_id, requesting_party.ssi, dest_addr.ssi, carrier_num, ts);

        self.notify_floor_granted(
            queue,
            GroupFloorGrant {
                call_id,
                source_issi: requesting_party.ssi,
                dest_gssi: dest_addr.ssi,
                carrier_num,
                ts,
                is_group: true,
            },
            true,
            BrewNotification::IfGroupRoutable(dest_ssi),
        );

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_tx_ceased(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        sender: TetraAddress,
    ) -> Result<(), GroupTransitionError> {
        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        let formal_state = call.formal_state;
        Self::validate_group_transition(call_id, state, formal_state, GroupEvent::TxCeased)?;

        if !call.is_current_speaker(sender.ssi) {
            return Err(GroupTransitionError::NotCurrentSpeaker {
                call_id,
                sender_issi: sender.ssi,
                current_speaker_issi: call.source_issi,
            });
        }

        let ts = call.ts;
        let carrier_num = call.carrier_num;
        let dest_ssi = call.dest_gssi;
        let queued_request = call.take_queued_tx_demand();
        if let Some(requester) = queued_request {
            // Transmitting -> Transmitting, hand over floor directly to queued requester.
            call.grant_floor(requester.ssi, Some(requester));
        } else {
            // Transmitting -> NoActiveSpeaker.
            call.enter_hangtime(self.dltime);
        }

        let Some(cached) = self.cached_setups.get(&call_id) else {
            return Err(GroupTransitionError::MissingCachedSetup(call_id));
        };
        let dest_addr = cached.dest_addr;

        if let Some(requester) = queued_request {
            self.fsm_send_d_tx_granted_individual(
                queue,
                call_id,
                requester,
                carrier_num,
                ts,
                TransmissionGrant::Granted,
                Some(requester.ssi),
            );
            self.send_d_tx_granted_facch(queue, call_id, requester.ssi, dest_addr.ssi, carrier_num, ts);

            self.notify_floor_granted(
                queue,
                GroupFloorGrant {
                    call_id,
                    source_issi: requester.ssi,
                    dest_gssi: dest_addr.ssi,
                    carrier_num,
                    ts,
                    is_group: true,
                },
                true,
                BrewNotification::IfGroupRoutable(dest_ssi),
            );
            return Ok(());
        }

        let d_tx_ceased = DTxCeased {
            call_identifier: call_id,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };
        tracing::info!("FSM -> {:?}", d_tx_ceased);
        let mut sdu = BitBuffer::new_autoexpand(25);
        d_tx_ceased.to_bitbuf(&mut sdu).expect("Failed to serialize DTxCeased");
        sdu.seek(0);

        let msg = Self::build_sapmsg_stealing(sdu, self.dltime, dest_addr, carrier_num, ts, None);
        queue.push_back(msg);

        self.notify_floor_released(
            queue,
            CallTimeslot { call_id, carrier_num, ts },
            true,
            BrewNotification::IfGroupRoutable(dest_ssi),
        );

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_network_call_start(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
    ) -> Result<(), GroupTransitionError> {
        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        let formal_state = call.formal_state;
        Self::validate_group_transition(call_id, state, formal_state, GroupEvent::NetworkCallStart)?;

        call.grant_floor(source_issi, None);
        call.brew_uuid = Some(brew_uuid);
        if let CallOrigin::Network { brew_uuid: old_uuid } = call.origin
            && old_uuid != brew_uuid
        {
            tracing::warn!("CMCE FSM: network call start changed brew_uuid call_id={}", call_id);
            call.origin = CallOrigin::Network { brew_uuid };
        }

        let ts = call.ts;
        let carrier_num = call.carrier_num;
        let usage = call.usage;
        let dest_gssi = call.dest_gssi;

        self.send_d_tx_granted_facch(queue, call_id, source_issi, dest_gssi, carrier_num, ts);

        self.notify_remote_floor_granted(queue, CallTimeslot { call_id, carrier_num, ts });

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
                usage,
            }),
        });

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_group_on_network_call_end(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
    ) -> Result<(), GroupTransitionError> {
        let Some(call) = self.active_calls.get(&call_id).cloned() else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        let state = call.state();
        Self::validate_group_transition(call_id, state, call.formal_state, GroupEvent::NetworkCallEnd)?;

        if matches!(state, GroupCallState::Transmitting) {
            if let Some(active_call) = self.active_calls.get_mut(&call_id) {
                active_call.enter_hangtime(self.dltime);
                active_call.brew_uuid = None;
            }

            self.send_d_tx_ceased_facch(queue, call_id, call.dest_gssi, call.carrier_num, call.ts);
            self.notify_floor_released(
                queue,
                CallTimeslot {
                    call_id,
                    carrier_num: call.carrier_num,
                    ts: call.ts,
                },
                true,
                BrewNotification::Never,
            );
            return Ok(());
        }

        self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        Ok(())
    }
}
