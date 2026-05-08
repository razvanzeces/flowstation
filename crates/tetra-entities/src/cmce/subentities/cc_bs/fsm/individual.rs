use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cmce::subentities::cc_bs) enum IndividualEvent {
    CreateSetup,
    BindCalledContext,
    SetNetworkCall,
    MarkConnectRequestSent,
    Alert,
    Connect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::cmce::subentities::cc_bs) enum IndividualTransitionError {
    UnknownCall(u16),
    DuplicateCall(u16),
    InvalidTransition {
        call_id: u16,
        state: IndividualCallState,
        event: IndividualEvent,
    },
    MissingBrewUuid(u16),
    NotBrewOriginated(u16),
    ConnectRequestAlreadySent(u16),
}

impl CcBsSubentity {
    fn validate_individual_transition(
        call_id: u16,
        state: IndividualCallState,
        event: IndividualEvent,
    ) -> Result<(), IndividualTransitionError> {
        let allowed = matches!(
            (state, event),
            (IndividualCallState::CallSetupPending, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::BindCalledContext)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::BindCalledContext)
                | (IndividualCallState::CallSetupPending, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::SetNetworkCall)
                | (IndividualCallState::CallSetupPending, IndividualEvent::MarkConnectRequestSent)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::MarkConnectRequestSent)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::MarkConnectRequestSent)
                | (
                    IndividualCallState::IncomingSetupWaitNetworkAck,
                    IndividualEvent::MarkConnectRequestSent
                )
                | (IndividualCallState::CallSetupPending, IndividualEvent::Alert)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::Alert)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::Alert)
                | (IndividualCallState::CallSetupPending, IndividualEvent::Connect)
                | (IndividualCallState::IncomingSetupPending, IndividualEvent::Connect)
                | (IndividualCallState::IncomingAlerting, IndividualEvent::Connect)
                | (IndividualCallState::IncomingSetupWaitNetworkAck, IndividualEvent::Connect)
        );
        if allowed {
            Ok(())
        } else {
            Err(IndividualTransitionError::InvalidTransition { call_id, state, event })
        }
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_create_setup_call(
        &mut self,
        call_id: u16,
        call: IndividualCall,
    ) -> Result<(), IndividualTransitionError> {
        if self.individual_calls.contains_key(&call_id) {
            return Err(IndividualTransitionError::DuplicateCall(call_id));
        }

        if !matches!(
            call.state,
            IndividualCallState::CallSetupPending | IndividualCallState::IncomingSetupPending
        ) {
            return Err(IndividualTransitionError::InvalidTransition {
                call_id,
                state: call.state,
                event: IndividualEvent::CreateSetup,
            });
        }

        self.individual_calls.insert(call_id, call);
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_bind_called_context(
        &mut self,
        call_id: u16,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::BindCalledContext)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id)
            && call.called_handle.is_none()
        {
            call.called_handle = Some(handle);
            call.called_link_id = Some(link_id);
            call.called_endpoint_id = Some(endpoint_id);
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_set_network_call(
        &mut self,
        call_id: u16,
        network_call: NetworkCircuitCall,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::SetNetworkCall)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.network_call = Some(network_call);
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_mark_connect_request_sent(
        &mut self,
        call_id: u16,
        network_call: NetworkCircuitCall,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::MarkConnectRequestSent)?;

        if !call_snapshot.calling_over_brew {
            return Err(IndividualTransitionError::NotBrewOriginated(call_id));
        }
        if call_snapshot.connect_request_sent {
            return Err(IndividualTransitionError::ConnectRequestAlreadySent(call_id));
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.connect_request_sent = true;
            call.network_call = Some(network_call);
            call.state = IndividualCallState::IncomingSetupWaitNetworkAck;
        }
        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_on_alert(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        called_handle_ctx: Option<(u32, u32, u32)>, // handle, link_id, endpoint_id
        setup_timeout: CallTimeoutSetupPhase,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::Alert)?;

        if let Some((handle, link_id, endpoint_id)) = called_handle_ctx {
            self.fsm_individual_bind_called_context(call_id, handle, link_id, endpoint_id)?;
        }

        if call_snapshot.calling_over_brew {
            let Some(brew_uuid) = call_snapshot.brew_uuid else {
                return Err(IndividualTransitionError::MissingBrewUuid(call_id));
            };

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }),
            });
        } else if !call_snapshot.is_alerted() {
            self.send_d_alert_individual(
                queue,
                call_id,
                call_snapshot.simplex_duplex,
                call_snapshot.calling_addr,
                call_snapshot.calling_handle,
                call_snapshot.calling_link_id,
                call_snapshot.calling_endpoint_id,
                setup_timeout,
            );
        }

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.mark_alerted(self.dltime, setup_timeout);
        }

        Ok(())
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_individual_transition_to_active(
        &mut self,
        call_id: u16,
    ) -> Result<(), IndividualTransitionError> {
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() else {
            return Err(IndividualTransitionError::UnknownCall(call_id));
        };

        Self::validate_individual_transition(call_id, call_snapshot.state, IndividualEvent::Connect)?;

        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.activate(self.dltime);
        }
        Ok(())
    }

    /// Handle parsed U-ALERT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_alert(
        &mut self,
        queue: &mut MessageQueue,
        received_tetra_address: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UAlert,
    ) {
        let call_id = pdu.call_identifier;
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            tracing::warn!("U-ALERT for unknown call_id={}", call_id);
            return;
        };

        if call.called_addr.ssi != received_tetra_address.ssi {
            tracing::warn!(
                "U-ALERT call_id={} from unexpected ISSI {} (expected {})",
                call_id,
                received_tetra_address.ssi,
                call.called_addr.ssi
            );
        }

        if let Err(err) = self.fsm_individual_on_alert(queue, call_id, Some((handle, link_id, endpoint_id)), CallTimeoutSetupPhase::T60s) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-ALERT for unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!("U-ALERT call_id={} ignored due to invalid transition in state {:?}", call_id, state);
                }
                IndividualTransitionError::MissingBrewUuid(_) => {
                    tracing::warn!("CMCE: Brew-originated call_id={} missing brew_uuid on U-ALERT", call_id);
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle parsed U-CONNECT.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_connect(
        &mut self,
        queue: &mut MessageQueue,
        received_tetra_address: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UConnect,
    ) {
        let call_id = pdu.call_identifier;
        let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() as Option<IndividualCall> else {
            tracing::warn!("U-CONNECT for unknown call_id={}", call_id);
            return;
        };

        if call_snapshot.is_active() {
            tracing::debug!("U-CONNECT for active call_id={}, ignoring", call_id);
            return;
        }

        if call_snapshot.called_addr.ssi != received_tetra_address.ssi {
            tracing::warn!(
                "U-CONNECT call_id={} from unexpected ISSI {} (expected {})",
                call_id,
                received_tetra_address.ssi,
                call_snapshot.called_addr.ssi
            );
        }

        if call_snapshot.simplex_duplex && !pdu.simplex_duplex_selection {
            tracing::warn!("U-CONNECT call_id={} downgraded to simplex by called MS; not supported", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::RequestedServiceNotAvailable);
            return;
        }

        if let Err(err) = self.fsm_individual_bind_called_context(call_id, handle, link_id, endpoint_id) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-CONNECT context bind failed, unknown call_id={}", call_id);
                    return;
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::debug!("U-CONNECT context bind rejected for call_id={} in state {:?}", call_id, state);
                    return;
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        if call_snapshot.calling_over_brew {
            let Some(brew_uuid) = call_snapshot.brew_uuid else {
                tracing::warn!("CMCE: Brew-originated call_id={} missing brew_uuid on U-CONNECT", call_id);
                return;
            };

            let mut call_info = call_snapshot.network_call.clone().unwrap_or(NetworkCircuitCall {
                source_issi: call_snapshot.calling_addr.ssi,
                destination: call_snapshot.called_addr.ssi,
                number: call_snapshot.called_addr.ssi.to_string(),
                priority: 0,
                service: 0,
                mode: CircuitModeType::TchS.into_raw() as u8,
                duplex: call_snapshot.simplex_duplex as u8,
                method: pdu.hook_method_selection as u8,
                communication: CommunicationType::P2p.into_raw() as u8,
                grant: 0,
                permission: 0,
                timeout: CallTimeout::T5m.into_raw() as u8,
                ownership: 0,
                queued: 0,
            });
            call_info.duplex = pdu.simplex_duplex_selection as u8;
            call_info.method = pdu.hook_method_selection as u8;
            // Update these fields as the call is accepted
            call_info.grant = 0;
            call_info.permission = 0;

            if let Err(err) = self.fsm_individual_mark_connect_request_sent(call_id, call_info.clone()) {
                match err {
                    IndividualTransitionError::ConnectRequestAlreadySent(_) => {
                        tracing::trace!(
                            "CMCE: duplicate U-CONNECT for Brew-originated call_id={}, CONNECT_REQUEST already sent",
                            call_id
                        );
                        return;
                    }
                    IndividualTransitionError::UnknownCall(_) => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent failed unknown call_id={}", call_id);
                        return;
                    }
                    IndividualTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent rejected call_id={} from state {:?}", call_id, state);
                        return;
                    }
                    IndividualTransitionError::NotBrewOriginated(_)
                    | IndividualTransitionError::MissingBrewUuid(_)
                    | IndividualTransitionError::DuplicateCall(_) => {
                        tracing::warn!("CMCE: U-CONNECT Brew mark sent inconsistent state for call_id={}", call_id);
                        return;
                    }
                }
            }

            tracing::info!(
                "CMCE: forwarding U-CONNECT as Brew CONNECT_REQUEST uuid={} call_id={} dst={} number='{}' grant='{}'",
                brew_uuid,
                call_id,
                call_info.destination,
                call_info.number,
                call_info.grant,
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                    brew_uuid,
                    call: call_info.clone(),
                }),
            });
            return;
        }

        let calling_addr = call_snapshot.calling_addr;
        let called_addr = call_snapshot.called_addr;
        let calling_handle = call_snapshot.calling_handle;
        let calling_link_id = call_snapshot.calling_link_id;
        let calling_endpoint_id = call_snapshot.calling_endpoint_id;
        let calling_ts = call_snapshot.calling_ts;
        let called_ts = call_snapshot.called_ts;
        let calling_usage = call_snapshot.calling_usage;
        let called_usage = call_snapshot.called_usage;
        let simplex_duplex = call_snapshot.simplex_duplex;

        let Some(cached) = self.cached_setups.get(&call_id) else {
            tracing::error!("No cached D-SETUP for call_id={}", call_id);
            return;
        };

        let mut calling_timeslots = [false; 4];
        calling_timeslots[calling_ts as usize - 1] = true;
        let mut called_timeslots = [false; 4];
        called_timeslots[called_ts as usize - 1] = true;

        // For simplex P2P: both MS initially get Both so they can receive the D-CONNECT /
        // D-CONNECT-ACK PDUs on the traffic channel. The floor (Ul/Dl restriction) is
        // enforced later via D-TX-GRANTED when either MS presses PTT (U-TX-DEMAND).
        // For duplex P2P: Both on both TS (cross-routed audio).
        let (calling_ul_dl, called_ul_dl) = (UlDlAssignment::Both, UlDlAssignment::Both);

        let chan_alloc_calling = CmceChanAllocReq {
            usage: Some(calling_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: calling_timeslots,
            ul_dl_assigned: calling_ul_dl,
        };
        let chan_alloc_called = CmceChanAllocReq {
            usage: Some(called_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots: called_timeslots,
            ul_dl_assigned: called_ul_dl,
        };
        tracing::debug!(
            "P2P chan_alloc: calling ts={} usage={} slots={:?}, called ts={} usage={} slots={:?}",
            calling_ts,
            calling_usage,
            calling_timeslots,
            called_ts,
            called_usage,
            called_timeslots
        );

        // Open UMAC circuits FIRST so traffic channel is ready before MS arrives
        let circuit_calling = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: calling_ts,
            call_id,
            usage: calling_usage,
            circuit_mode: cached.pdu.basic_service_information.circuit_mode_type,
            comm_type: cached.pdu.basic_service_information.communication_type,
            simplex_duplex,
            speech_service: cached.pdu.basic_service_information.speech_service,
            etee_encrypted: cached.pdu.basic_service_information.encryption_flag,
        };
        let duplex_peer = if calling_ts != called_ts { Some(called_ts) } else { None };
        Self::signal_umac_circuit_open(queue, &circuit_calling, duplex_peer, CircuitDlMediaSource::LocalLoopback);

        if called_ts != calling_ts {
            let circuit_called = CmceCircuit {
                ts_created: self.dltime,
                direction: Direction::Both,
                ts: called_ts,
                call_id,
                usage: called_usage,
                circuit_mode: cached.pdu.basic_service_information.circuit_mode_type,
                comm_type: cached.pdu.basic_service_information.communication_type,
                simplex_duplex,
                speech_service: cached.pdu.basic_service_information.speech_service,
                etee_encrypted: cached.pdu.basic_service_information.encryption_flag,
            };
            Self::signal_umac_circuit_open(queue, &circuit_called, Some(calling_ts), CircuitDlMediaSource::LocalLoopback);
        }

        // D-CONNECT to calling MS:
        //   - Simplex: GrantedToOtherUser - the called MS answers first and speaks first.
        //     Caller must send U-TX-DEMAND to get the floor.
        //   - Duplex: Granted - both MS may speak simultaneously.
        // transmission_request_permission=false = 0 = ALLOWED to request transmission (ETSI 14.8.43).
        let calling_grant = if simplex_duplex {
            TransmissionGrant::Granted
        } else {
            TransmissionGrant::GrantedToOtherUser
        };
        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: self.config_call_timeout(),
            hook_method_selection: cached.pdu.hook_method_selection,
            simplex_duplex_selection: simplex_duplex,
            transmission_grant: calling_grant,
            transmission_request_permission: false,
            call_ownership: true,
            call_priority: None,
            basic_service_information: None,
            temporary_address: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_connect);
        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);

        // --- STEP 1: DConnect via FACCH stealing (terminal already on TCH) ---
        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                chan_alloc: Some(chan_alloc_calling.clone()),
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg);

        // --- STEP 2: DConnect via MCCH as fallback (terminal still on control channel) ---
        let mut connect_sdu2 = BitBuffer::new_autoexpand(30);
        d_connect
            .to_bitbuf(&mut connect_sdu2)
            .expect("Failed to serialize DConnect (fallback)");
        connect_sdu2.seek(0);
        let connect_msg2 = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu2,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: Some(chan_alloc_calling.clone()),
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg2);

        // D-CONNECT-ACKNOWLEDGE to called MS:
        //   - Simplex: Granted - the called MS answered, it speaks first.
        //   - Duplex: Granted - both MS may speak simultaneously.
        // transmission_request_permission=false = 0 = ALLOWED to request transmission (ETSI 14.8.43).
        let d_connect_ack = DConnectAcknowledge {
            call_identifier: call_id,
            call_time_out: self.config_call_timeout().into_raw() as u8,
            transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
            transmission_request_permission: false,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_connect_ack);
        let mut ack_sdu = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu)
            .expect("Failed to serialize DConnectAcknowledge");
        ack_sdu.seek(0);

        // --- STEP 1: DConnectAcknowledge via FACCH stealing (terminal already on TCH) ---
        let ack_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                chan_alloc: Some(chan_alloc_called.clone()),
                main_address: called_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(ack_msg);

        // --- STEP 2: DConnectAcknowledge via MCCH as fallback (terminal still on control channel) ---
        let mut ack_sdu2 = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu2)
            .expect("Failed to serialize DConnectAcknowledge (fallback)");
        ack_sdu2.seek(0);
        let ack_msg2 = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu2,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: Some(chan_alloc_called.clone()),
                main_address: called_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(ack_msg2);

        if let Err(err) = self.fsm_individual_transition_to_active(call_id) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("U-CONNECT activation failed, unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("U-CONNECT activation rejected for call_id={} from state {:?}", call_id, state);
                }
                IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }
}
