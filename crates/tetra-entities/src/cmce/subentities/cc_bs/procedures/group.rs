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

    pub(in crate::cmce::subentities::cc_bs) fn fsm_send_d_tx_granted_individual(
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
        let emergency = is_emergency_priority(call.priority);

        // Who may talk on the group (Brew server rules, `group-access`): a radio that may not is
        // told "transmission not granted" and stays in the call (clause 14.5.2.2.1 b), table
        // 14.80). An emergency call is never limited.
        if !emergency && !self.config.state_read().group_may_talk(dest_ssi, requesting_party.ssi) {
            tracing::info!(
                "CMCE: U-TX DEMAND call_id={} from ISSI {} on GSSI {} not granted (may not talk on the group, group-access)",
                call_id,
                requesting_party.ssi,
                dest_ssi
            );
            self.fsm_send_d_tx_granted_individual(queue, call_id, requesting_party, carrier_num, ts, TransmissionGrant::NotGranted, Some(current_speaker));
            return Ok(());
        }
        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };
        // The network talker of this over has moved here unannounced and keys up again: it
        // keeps the floor (see `take_over_moved_talker`).
        let grant_now = matches!(state, GroupCallState::NoActiveSpeaker { .. }) || call.take_over_moved_talker(requesting_party.ssi);
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

        let ts = call.ts;
        let carrier_num = call.carrier_num;
        let usage = call.usage;
        let dest_gssi = call.dest_gssi;
        let prev_speaker = call.source_issi;
        let was_local_floor = call.local_floor;
        let was_tx = call.tx_active;

        // Hard preempt: a local MS still thinks it owns the floor. Soft grant alone leaves the
        // walkie transmitting while NetworkCallReady makes the console believe it is on air.
        let hard_preempt = was_tx && was_local_floor && prev_speaker != source_issi;
        if hard_preempt {
            let prev_addr = TetraAddress::new(prev_speaker, SsiType::Issi);
            tracing::info!(
                "CMCE: network preempt of local speaker ISSI {} on call_id={} → ISSI {}",
                prev_speaker,
                call_id,
                source_issi
            );
            self.fsm_send_d_tx_granted_individual(
                queue,
                call_id,
                prev_addr,
                carrier_num,
                ts,
                TransmissionGrant::NotGranted,
                Some(source_issi),
            );
            self.send_d_tx_ceased_facch(queue, call_id, dest_gssi, carrier_num, ts);
            self.notify_floor_released(
                queue,
                CallTimeslot {
                    call_id,
                    carrier_num,
                    ts,
                },
                true,
                BrewNotification::IfGroupRoutable(dest_gssi),
            );
        }

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return Err(GroupTransitionError::UnknownCall(call_id));
        };

        call.grant_floor(source_issi, None);
        call.touch_activity(self.dltime);
        call.brew_uuid = Some(brew_uuid);
        // Brew holds the floor: call is network-owned. Keeps leave-TG / owner U-DISCONNECT
        // on the Hold → LATE ENTRY path (Ethernet) instead of Local teardown (WiFi race).
        match &call.origin {
            CallOrigin::Local { .. } => {
                tracing::info!(
                    "CMCE FSM: transferring call_id={} ownership Local→Network (Brew speaker uuid={})",
                    call_id,
                    brew_uuid
                );
                call.origin = CallOrigin::Network { brew_uuid };
            }
            CallOrigin::Network { brew_uuid: old_uuid } if *old_uuid != brew_uuid => {
                tracing::warn!("CMCE FSM: network call start changed brew_uuid call_id={}", call_id);
                call.origin = CallOrigin::Network { brew_uuid };
            }
            CallOrigin::Network { .. } => {}
        }

        // Network speaker owns DL: flip media source in-place (never Open — that closes/reopens
        // and destroys the traffic path / LST downlink).
        Self::signal_umac_set_dl_media_source(queue, carrier_num, ts, CircuitDlMediaSource::SwMI);

        // Soft restart after NetworkCallEnd while walkie UL still hot must not Ready immediately.
        let ul_hot = self.ul_slot_hot.get(&(carrier_num, ts)).copied().unwrap_or(false);
        let defer_ready = hard_preempt || ul_hot;

        if defer_ready {
            let preempted = if hard_preempt {
                prev_speaker
            } else {
                self.ul_slot_preempted_issi
                    .get(&(carrier_num, ts))
                    .copied()
                    .unwrap_or(prev_speaker)
            };
            if !hard_preempt {
                // Soft path: hangtime may already be on from NetworkCallEnd; push cease again.
                tracing::info!(
                    "CMCE: defer Ready (UL still hot) call_id={} C{}TS{}",
                    call_id,
                    carrier_num,
                    ts
                );
                self.send_d_tx_ceased_facch(queue, call_id, dest_gssi, carrier_num, ts);
                // Ensure hangtime stays ON (NetworkCallEnd already FloorReleased; soft must not
                // RemoteFloorGranted). Re-assert FloorReleased to UMAC only if needed — safe.
                self.notify_floor_released(
                    queue,
                    CallTimeslot {
                        call_id,
                        carrier_num,
                        ts,
                    },
                    true,
                    BrewNotification::Never,
                );
            } else {
                self.ul_slot_preempted_issi.insert((carrier_num, ts), preempted);
            }
            self.ul_slot_hot.insert((carrier_num, ts), true);

            tracing::info!(
                "CMCE: preempt pending Ready call_id={} (holding hangtime, awaiting UL quiet)",
                call_id
            );
            let next_cease_at = self.dltime.add_timeslots(PREEMPT_CEASE_INTERVAL_TS);
            let min_ready_at = self.dltime.add_timeslots(PREEMPT_MIN_HOLD_TS);
            let ready_deadline = self.dltime.add_timeslots(PREEMPT_READY_DEADLINE_TS);
            let post_cease_until = self.dltime.add_timeslots(PREEMPT_POST_CEASE_TS);
            self.preempt_pending.insert(
                call_id,
                PreemptPendingReady {
                    brew_uuid,
                    call_id,
                    dest_gssi,
                    carrier_num,
                    ts,
                    usage,
                    network_speaker: source_issi,
                    preempted_issi: preempted,
                    next_cease_at,
                    min_ready_at,
                    ready_deadline,
                    post_cease_until,
                    ready_sent: false,
                    ul_active: true,
                },
            );
            return Ok(());
        }

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

        self.preempt_pending.remove(&call_id);

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
