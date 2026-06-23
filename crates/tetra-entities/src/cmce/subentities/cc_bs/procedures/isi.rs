use super::*;

/// ANF-ISIIC/ANF-ISIGC-adjacent procedures for the local network bridge.
///
/// The implementation still speaks the internal Brew control protocol, not
/// ETSI PSS1/ROSE ISI PDUs. The split is intentional: individual-call and
/// group-call interworking belongs beside CC procedures, while PC remains the
/// CMCE route discriminator.
impl CcBsSubentity {
    /// Handle network-initiated circuit setup request (Brew/Asterisk -> local called MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_setup_request(
        &mut self,
        queue: &mut MessageQueue,
        network_entity: TetraEntity,
        brew_uuid: uuid::Uuid,
        call: NetworkCircuitCall,
    ) {
        let called_addr = TetraAddress::new(call.destination, SsiType::Issi);
        if call.destination == 0 {
            tracing::info!(
                "CMCE: rejecting {:?} setup request uuid={} src={} dst=0 number='{}' (missing called ISSI)",
                network_entity,
                brew_uuid,
                call.source_issi,
                call.number
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyNotReachable.into_raw() as u8,
                }),
            });
            return;
        }

        if !self.is_locally_registered_issi(called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst={} number='{}' (called ISSI not registered locally, known registry ISSIs={:?})",
                brew_uuid,
                call.source_issi,
                call.destination,
                call.number,
                self.known_local_issis()
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyNotReachable.into_raw() as u8,
                }),
            });
            return;
        }

        if let Some((active_call_id, state)) = self.find_individual_call_by_issi(called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting Brew setup request uuid={} src={} dst={} number='{}' (called ISSI busy in call_id={} state={:?})",
                brew_uuid,
                call.source_issi,
                call.destination,
                call.number,
                active_call_id,
                state
            );
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: network_entity,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                    brew_uuid,
                    cause: DisconnectCause::CalledPartyBusy.into_raw() as u8,
                }),
            });
            return;
        }

        let communication = CommunicationType::try_from(call.communication as u64).unwrap_or(CommunicationType::P2p);
        let simplex_duplex = call.duplex != 0;
        let hook_method_selection = call.method != 0;

        let circuit_called = {
            let mut state = self.config.state_write();
            match self.circuits.allocate_circuit_with_allocator(
                Direction::Both,
                communication,
                simplex_duplex,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting Brew setup request uuid={} src={} dst={} (allocation failed: {:?})",
                        brew_uuid,
                        call.source_issi,
                        call.destination,
                        e
                    );
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: network_entity,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject {
                            brew_uuid,
                            cause: DisconnectCause::CongestionInInfrastructure.into_raw() as u8,
                        }),
                    });
                    return;
                }
            }
        };

        let call_id = circuit_called.call_id;
        let ts = circuit_called.ts;
        let usage = circuit_called.usage;
        let call_timeout = CallTimeout::try_from(call.timeout as u64).unwrap_or(CallTimeout::T5m);
        let circuit_mode = CircuitModeType::try_from(call.mode as u64).unwrap_or(CircuitModeType::TchS);
        let external_subscriber_number = Self::encode_external_subscriber_number(&call.number);
        let calling_party_extension = call.number.trim().parse::<u32>().ok().filter(|value| *value <= 0x00ff_ffff);

        // Asterisk SIP callers often have no real TETRA ISSI; derive a display SSI from the
        // dialled external number so the local MS sees a sensible calling party. Brew keeps its
        // original behaviour (always uses source_issi, even when 0).
        let calling_party_address_ssi = if call.source_issi != 0 {
            Some(call.source_issi)
        } else if network_entity == TetraEntity::Asterisk {
            Self::external_number_as_ssi(&call.number)
        } else {
            Some(call.source_issi)
        };

        tracing::info!(
            "CMCE: accepting {:?} setup request uuid={} call_id={} src={} dst={} ts={} duplex={} number='{}'",
            network_entity,
            brew_uuid,
            call_id,
            call.source_issi,
            call.destination,
            ts,
            simplex_duplex,
            call.number
        );

        // Acknowledge setup to the originating network entity first so network call state
        // progresses while the local MS is alerted.
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: network_entity,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }),
        });

        let setup_transmission_grant = if simplex_duplex {
            TransmissionGrant::NotGranted
        } else {
            TransmissionGrant::GrantedToOtherUser
        };

        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: call_timeout,
            hook_method_selection,
            simplex_duplex_selection: simplex_duplex,
            basic_service_information: BasicServiceInformation {
                circuit_mode_type: circuit_mode,
                encryption_flag: false,
                communication_type: communication,
                slots_per_frame: None,
                speech_service: Some(call.service),
            },
            transmission_grant: setup_transmission_grant,
            transmission_request_permission: false,
            call_priority: call.priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi,
            calling_party_extension,
            external_subscriber_number,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };
        tracing::debug!("-> {:?}", d_setup);

        self.cached_setups.insert(
            call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr: called_addr,
                resend: false, // no late-entry resends for individual calls
                tx_receipt: None,
            },
        );

        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;
        let mut setup_sdu = BitBuffer::new_autoexpand(80);
        d_setup_ref.to_bitbuf(&mut setup_sdu).expect("Failed to serialize DSetup");
        setup_sdu.seek(0);
        let setup_msg = Self::build_sapmsg(setup_sdu, None, self.dltime, called_addr, None);
        queue.push_back(setup_msg);

        if let Err(err) = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: TetraAddress::new(call.source_issi, SsiType::Issi),
                called_addr,
                calling_handle: 0,
                calling_link_id: 0,
                calling_endpoint_id: 0,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_carrier_num: self.config.config().cell.main_carrier,
                calling_ts: ts,
                called_carrier_num: self.config.config().cell.main_carrier,
                called_ts: ts,
                calling_usage: usage,
                called_usage: usage,
                simplex_duplex,
                priority: call.priority,
                state: IndividualCallState::IncomingSetupPending,
                formal_state: CcFormalState::Idle.after(CcFormalEvent::SetupRequest),
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout,
                called_over_brew: false,
                calling_over_brew: true,
                network_entity,
                brew_uuid: Some(brew_uuid),
                network_call: Some(call),
                connect_request_sent: false,
                floor_holder: None,
                queued_tx_demand: None,
            },
        ) {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating inbound Brew setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!(
                        "CMCE: inbound Brew setup call_id={} creation rejected for state {:?}",
                        call_id,
                        state
                    );
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle network circuit connect request (Brew -> local called MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_connect_request(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        call_info: NetworkCircuitCall,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!("CMCE: Brew connect request for unknown uuid={}", brew_uuid);
            return;
        };

        if call.calling_over_brew {
            tracing::warn!(
                "CMCE: unexpected Brew CONNECT_REQUEST for Brew-originated call uuid={} call_id={}, treating as CONNECT_CONFIRM",
                brew_uuid,
                call_id
            );
            self.fsm_on_network_circuit_connect_confirm(queue, brew_uuid, call_info.grant, call_info.permission);
            return;
        }

        if call.is_active() {
            tracing::trace!("CMCE: Brew connect request for active call_id={}, ignoring", call_id);
            return;
        }

        tracing::info!(
            "CMCE: Brew connect request uuid={} call_id={} dst={} number='{}'",
            brew_uuid,
            call_id,
            call_info.destination,
            call_info.number
        );
        let is_simplex = call.is_simplex();

        if let Err(err) = self.fsm_individual_set_network_call(call_id, call_info.clone()) {
            match err {
                IndividualTransitionError::UnknownCall(_) => {
                    tracing::warn!("CMCE: Brew connect request state update unknown call_id={}", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!(
                        "CMCE: Brew connect request state update rejected call_id={} from state {:?}",
                        call_id,
                        state
                    );
                }
                IndividualTransitionError::DuplicateCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }

        let mut calling_timeslots = [false; 4];
        calling_timeslots[call.calling_ts as usize - 1] = true;
        let chan_alloc_calling = CmceChanAllocReq {
            usage: Some(call.calling_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: Some(call.calling_carrier_num),
            timeslots: calling_timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
        };

        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: Self::p2p_call_timeout(call.simplex_duplex),
            hook_method_selection: call.simplex_duplex,
            simplex_duplex_selection: call.simplex_duplex,
            transmission_grant: TransmissionGrant::Granted,
            transmission_request_permission: false,
            call_ownership: true,
            call_priority: None,
            basic_service_information: None,
            temporary_address: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: call.calling_ts,
            carrier_num: call.calling_carrier_num,
            call_id,
            usage: call.calling_usage,
            circuit_mode: CircuitModeType::TchS,
            comm_type: CommunicationType::P2p,
            simplex_duplex: call.simplex_duplex,
            speech_service: Some(0),
            etee_encrypted: false,
        };
        Self::signal_umac_circuit_open(queue, &circuit, self.dltime, None, None, CircuitDlMediaSource::SwMI);

        tracing::info!("-> {:?}", d_connect);
        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);

        let connect_msg_stealing = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: call.calling_handle,
                endpoint_id: call.calling_endpoint_id,
                link_id: call.calling_link_id,
                // Network individual-call D-CONNECT: the legacy `main` code sent CC PDUs
                // unacknowledged (FH FIX 2).
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                chan_alloc: Some(chan_alloc_calling.clone()),
                main_address: call.calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg_stealing);

        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);
        // Keep a linkless MCCH copy alongside the traffic-leg send. Reusing the setup LLC link
        // with chan_alloc here can make UMAC treat D-CONNECT as a random-access response and
        // leave the local handset in alerting/ringing state after answer.
        let connect_msg_fallback =
            Self::build_sapmsg(connect_sdu, Some(chan_alloc_calling), self.dltime, call.calling_addr, None);
        queue.push_back(connect_msg_fallback);

        let activated = match self.fsm_individual_transition_to_active(call_id) {
            Ok(()) => true,
            Err(err) => {
                match err {
                    IndividualTransitionError::UnknownCall(_) => {
                        tracing::warn!("CMCE: Brew connect request activation unknown call_id={}", call_id);
                    }
                    IndividualTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!(
                            "CMCE: Brew connect request activation rejected call_id={} from state {:?}",
                            call_id,
                            state
                        );
                    }
                    IndividualTransitionError::MissingBrewUuid(_)
                    | IndividualTransitionError::DuplicateCall(_)
                    | IndividualTransitionError::NotBrewOriginated(_)
                    | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
                }
                false
            }
        };

        if activated && is_simplex {
            if let Some(call_state) = self.individual_calls.get_mut(&call_id) {
                let holder = call_state.calling_addr;
                call_state.grant_floor(holder);
            }
            self.notify_floor_granted(
                queue,
                GroupFloorGrant {
                    call_id,
                    source_issi: call.calling_addr.ssi,
                    dest_gssi: call.called_addr.ssi,
                    carrier_num: call.calling_carrier_num,
                    ts: call.calling_ts,
                    is_group: false,
                },
                true,
                BrewNotification::Never,
            );
        }

        let remote_grant = if is_simplex {
            TransmissionGrant::GrantedToOtherUser
        } else {
            TransmissionGrant::Granted
        };

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: call.network_entity,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                brew_uuid,
                grant: remote_grant.into_raw() as u8,
                permission: 0,
            }),
        });

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: call.network_entity,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num: call.calling_carrier_num,
                ts: call.calling_ts,
            }),
        });
    }

    /// Handle network circuit connect confirm (Brew -> local calling MS).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_circuit_connect_confirm(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        grant: u8,
        permission: u8,
    ) {
        let Some((call_id, call)) = self.find_brew_individual_call(brew_uuid) else {
            tracing::debug!(
                "CMCE: Brew connect confirm for unknown uuid={} grant={} permission={}",
                brew_uuid,
                grant,
                permission
            );
            return;
        };

        if !call.calling_over_brew {
            tracing::trace!(
                "CMCE: ignoring unexpected Brew connect confirm for local-origin call uuid={} call_id={}",
                brew_uuid,
                call_id
            );
            return;
        }

        if call.is_active() {
            tracing::trace!("CMCE: Brew connect confirm for active call_id={}, ignoring", call_id);
            return;
        }

        let (Some(called_handle), Some(called_link_id), Some(called_endpoint_id)) =
            (call.called_handle, call.called_link_id, call.called_endpoint_id)
        else {
            tracing::warn!(
                "CMCE: Brew connect confirm uuid={} call_id={} before local U-CONNECT context is known",
                brew_uuid,
                call_id
            );
            return;
        };

        let is_simplex = call.is_simplex();
        let remote_grant = TransmissionGrant::try_from((grant & 0x03) as u64).unwrap_or(TransmissionGrant::Granted);
        let local_grant = if is_simplex {
            match remote_grant {
                TransmissionGrant::Granted => TransmissionGrant::GrantedToOtherUser,
                TransmissionGrant::GrantedToOtherUser => TransmissionGrant::Granted,
                TransmissionGrant::RequestQueued => TransmissionGrant::RequestQueued,
                TransmissionGrant::NotGranted => TransmissionGrant::NotGranted,
            }
        } else {
            remote_grant
        };

        tracing::info!(
            "CMCE: Brew connect confirm uuid={} call_id={} remote_grant={} local_grant={:?} permission={}",
            brew_uuid,
            call_id,
            grant,
            local_grant,
            permission
        );
        let ul_dl_assigned = UlDlAssignment::Both;

        let mut called_timeslots = [false; 4];
        called_timeslots[call.called_ts as usize - 1] = true;
        let chan_alloc_called = CmceChanAllocReq {
            usage: Some(call.called_usage),
            alloc_type: ChanAllocType::Replace,
            carrier: Some(call.called_carrier_num),
            timeslots: called_timeslots,
            ul_dl_assigned,
        };

        let d_connect_ack = DConnectAcknowledge {
            call_identifier: call_id,
            call_time_out: Self::p2p_call_timeout(call.simplex_duplex).into_raw() as u8,
            transmission_grant: local_grant.into_raw() as u8,
            transmission_request_permission: permission != 0,
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

        let (circuit_mode, comm_type, speech_service, etee_encrypted) = if let Some(cached) = self.cached_setups.get(&call_id) {
            (
                cached.pdu.basic_service_information.circuit_mode_type,
                cached.pdu.basic_service_information.communication_type,
                cached.pdu.basic_service_information.speech_service,
                cached.pdu.basic_service_information.encryption_flag,
            )
        } else {
            (CircuitModeType::TchS, CommunicationType::P2p, Some(0), false)
        };

        let circuit = CmceCircuit {
            ts_created: self.dltime,
            direction: Direction::Both,
            ts: call.called_ts,
            carrier_num: call.called_carrier_num,
            call_id,
            usage: call.called_usage,
            circuit_mode,
            comm_type,
            simplex_duplex: call.simplex_duplex,
            speech_service,
            etee_encrypted,
        };
        Self::signal_umac_circuit_open(queue, &circuit, self.dltime, None, None, CircuitDlMediaSource::SwMI);

        let ack_msg_stealing = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: ack_sdu,
                handle: called_handle,
                endpoint_id: called_endpoint_id,
                link_id: called_link_id,
                // Network individual-call D-CONNECT-ACK: the legacy `main` code sent CC PDUs
                // unacknowledged (FH FIX 2).
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: true,
                chan_alloc: Some(chan_alloc_called.clone()),
                main_address: call.called_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(ack_msg_stealing);

        let mut ack_sdu = BitBuffer::new_autoexpand(28);
        d_connect_ack
            .to_bitbuf(&mut ack_sdu)
            .expect("Failed to serialize DConnectAcknowledge");
        ack_sdu.seek(0);
        // Same as the D-CONNECT path above: preserve an MCCH fallback, but keep it linkless so
        // chan_alloc is not mistaken for a random-access response on the old setup LLC link.
        let ack_msg_fallback = Self::build_sapmsg(ack_sdu, Some(chan_alloc_called), self.dltime, call.called_addr, None);
        queue.push_back(ack_msg_fallback);

        let activated = match self.fsm_individual_transition_to_active(call_id) {
            Ok(()) => true,
            Err(err) => {
                match err {
                    IndividualTransitionError::UnknownCall(_) => {
                        tracing::warn!("CMCE: Brew connect confirm activation unknown call_id={}", call_id);
                    }
                    IndividualTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!(
                            "CMCE: Brew connect confirm activation rejected call_id={} from state {:?}",
                            call_id,
                            state
                        );
                    }
                    IndividualTransitionError::MissingBrewUuid(_)
                    | IndividualTransitionError::DuplicateCall(_)
                    | IndividualTransitionError::NotBrewOriginated(_)
                    | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
                }
                false
            }
        };

        if activated && is_simplex {
            match local_grant {
                TransmissionGrant::Granted => {
                    if let Some(call_state) = self.individual_calls.get_mut(&call_id) {
                        let holder = call_state.called_addr;
                        call_state.grant_floor(holder);
                    }
                    self.notify_floor_granted(
                        queue,
                        GroupFloorGrant {
                            call_id,
                            source_issi: call.called_addr.ssi,
                            dest_gssi: call.calling_addr.ssi,
                            carrier_num: call.called_carrier_num,
                            ts: call.called_ts,
                            is_group: false,
                        },
                        true,
                        BrewNotification::Never,
                    );
                }
                TransmissionGrant::GrantedToOtherUser => {
                    if let Some(call_state) = self.individual_calls.get_mut(&call_id) {
                        let holder = call_state.calling_addr;
                        call_state.grant_floor(holder);
                    }
                }
                TransmissionGrant::RequestQueued | TransmissionGrant::NotGranted => {
                    if let Some(call_state) = self.individual_calls.get_mut(&call_id) {
                        call_state.release_floor();
                    }
                }
            }
        }

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: call.network_entity,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num: call.called_carrier_num,
                ts: call.called_ts,
            }),
        });
    }

    /// Handle network-initiated group call start.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_network_call_start(
        &mut self,
        queue: &mut MessageQueue,
        brew_uuid: uuid::Uuid,
        source_issi: u32,
        dest_gssi: u32,
        priority: u8,
    ) {
        assert!(brew::is_brew_gssi_routable(&self.config, dest_gssi));

        if !self.has_listener(dest_gssi) {
            tracing::info!(
                "CMCE: ignoring network call start uuid={} gssi={} (no listeners)",
                brew_uuid,
                dest_gssi
            );
            self.drop_group_calls_if_unlistened(queue, dest_gssi);

            self.notify_network_call_end(queue, brew_uuid);
            return;
        }

        // Speaker change for an existing GSSI call
        if let Some((call_id, old_speaker)) = self
            .active_calls
            .iter()
            .find(|(_, c)| c.dest_gssi == dest_gssi)
            .map(|(id, c)| (*id, c.source_issi))
        {
            tracing::info!(
                "CMCE: network call speaker change gssi={} new_speaker={} (was {})",
                dest_gssi,
                source_issi,
                old_speaker
            );

            if let Err(err) = self.fsm_group_on_network_call_start(queue, call_id, brew_uuid, source_issi) {
                match err {
                    GroupTransitionError::UnknownCall(_) => {
                        tracing::warn!(
                            "CMCE: network speaker change gssi={} resolved unknown call_id={}",
                            dest_gssi,
                            call_id
                        );
                    }
                    GroupTransitionError::InvalidTransition { state, .. } => {
                        tracing::warn!("CMCE: network speaker change rejected call_id={} from state {:?}", call_id, state);
                    }
                    GroupTransitionError::NotCurrentSpeaker { .. } => {
                        tracing::debug!(
                            "CMCE: network speaker change produced unexpected NotCurrentSpeaker for call_id={}",
                            call_id
                        );
                    }
                    GroupTransitionError::MissingCachedSetup(_) => {
                        tracing::debug!(
                            "CMCE: network speaker change call_id={} without cached setup (not required for this transition)",
                            call_id
                        );
                    }
                }
            }
            return;
        }

        // New network call - allocate circuit
        let circuit = match {
            let mut state = self.config.state_write();
            self.circuits.allocate_circuit_with_allocator(
                Direction::Both,
                CommunicationType::P2Mp,
                false,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
            )
        } {
            Ok(c) => c.clone(),
            Err(err) => {
                tracing::warn!("CMCE: failed to allocate circuit for network call: {:?}", err);
                return;
            }
        };

        let call_id = circuit.call_id;
        let ts = circuit.ts;
        let usage = circuit.usage;

        tracing::info!(
            "CMCE: starting NEW network call brew_uuid={} gssi={} speaker={} ts={} call_id={}",
            brew_uuid,
            dest_gssi,
            source_issi,
            ts,
            call_id
        );

        Self::signal_umac_circuit_open(queue, &circuit, self.dltime, None, None, CircuitDlMediaSource::LocalLoopback);

        tracing::debug!(
            "CMCE: sending D-SETUP for NEW call call_id={} gssi={} (network-initiated)",
            call_id,
            dest_gssi
        );

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: CallTimeout::T5m,
            hook_method_selection: false,
            simplex_duplex_selection: false,
            basic_service_information: BasicServiceInformation {
                circuit_mode_type: CircuitModeType::TchS,
                encryption_flag: false,
                communication_type: CommunicationType::P2Mp,
                slots_per_frame: None,
                speech_service: Some(0),
            },
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: false,
            call_priority: 0,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(source_issi),
            calling_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        self.cached_setups.insert(
            call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr: dest_addr.clone(),
                resend: true,
                tx_receipt: None,
            },
        );
        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;

        let (setup_sdu, setup_chan_alloc) = Self::build_d_setup_prim(d_setup_ref, usage, circuit.carrier_num, ts, UlDlAssignment::Both);
        let setup_msg = Self::build_sapmsg(setup_sdu, Some(setup_chan_alloc), self.dltime, dest_addr.clone(), None);
        queue.push_back(setup_msg);

        let d_connect = DConnect {
            call_identifier: call_id,
            call_time_out: CallTimeout::T5m,
            hook_method_selection: false,
            simplex_duplex_selection: false,
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: false,
            call_ownership: false,
            call_priority: None,
            basic_service_information: None,
            temporary_address: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);

        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                // GROUP-addressed D-CONNECT (main_address is the GSSI) for a network-initiated
                // group call: acknowledged LLC has no single peer to ACK, so this must be
                // unacknowledged BL-UDATA — same class as the FIX 2 group D-SETUP/D-RELEASE bug.
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: dest_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg);

        self.active_calls.insert(
            call_id,
            ActiveCall::new_network(
                brew_uuid,
                dest_gssi,
                source_issi,
                circuit.carrier_num,
                ts,
                usage,
                self.dltime,
                self.config_call_timeout(),
                priority,
            ),
        );

        // Dashboard telemetry: a Brew/network-initiated group call just became active.
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallStarted {
            call_id,
            gssi: dest_gssi,
            caller_issi: source_issi,
            carrier_num: circuit.carrier_num,
            ts,
            priority,
        });

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num: circuit.carrier_num,
                ts,
                usage,
            }),
        });
    }
}
