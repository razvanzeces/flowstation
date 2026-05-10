use super::*;

impl CcBsSubentity {
    /// Handle U-SETUP for group calls (non-P2P communication types).
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_group(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        // Get destination GSSI (called party)
        let Some(dest_gssi) = pdu.called_party_ssi else {
            tracing::warn!("U-SETUP without called_party_ssi, ignoring");
            return;
        };
        let dest_gssi = dest_gssi as u32;
        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);

        if !self.has_listener(dest_gssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP from issi={} to gssi={} (no listeners)",
                calling_party.ssi,
                dest_gssi
            );
            return;
        }

        // Allocate circuit (DL+UL for group call)
        let circuit = match {
            let mut state = self.config.state_write();
            self.circuits.allocate_circuit_with_allocator_duplex(Direction::Both, pdu.basic_service_information.communication_type, pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
            )
        } {
            Ok(circuit) => circuit.clone(),
            Err(e) => {
                tracing::error!("Failed to allocate circuit for U-SETUP: {:?}", e);
                return;
            }
        };

        tracing::info!(
            "rx_u_setup: call from ISSI {} to GSSI {} -> ts={} call_id={} usage={}",
            calling_party.ssi,
            dest_gssi,
            circuit.ts,
            circuit.call_id,
            circuit.usage
        );

        // Emit telemetry event for dashboard
        self.emit(crate::net_telemetry::TelemetryEvent::GroupCallStarted {
            call_id: circuit.call_id,
            gssi: dest_gssi,
            caller_issi: calling_party.ssi,
        });

        // Signal UMAC to open DL+UL circuits.
        Self::signal_umac_circuit_open(queue, &circuit, None, CircuitDlMediaSource::LocalLoopback);

        // Build channel allocation timeslot mask for this call.
        let mut timeslots = [false; 4];
        timeslots[circuit.ts as usize - 1] = true;

        // Extract UL message routing info for individually-addressed responses.
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };
        let ul_handle = prim.handle;
        let ul_link_id = prim.link_id;
        let ul_endpoint_id = prim.endpoint_id;

        // 1) D-CALL-PROCEEDING to caller.
        self.send_d_call_proceeding(
            queue,
            message,
            pdu,
            circuit.call_id,
            CallTimeoutSetupPhase::T10s,
            pdu.hook_method_selection,
        );

        // 2) D-CONNECT to caller.
        let d_connect = DConnect {
            call_identifier: circuit.call_id,
            call_time_out: self.config_call_timeout(),
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
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

        tracing::info!("-> {:?}", d_connect);
        let mut connect_sdu = BitBuffer::new_autoexpand(30);
        d_connect.to_bitbuf(&mut connect_sdu).expect("Failed to serialize DConnect");
        connect_sdu.seek(0);

        let connect_msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu: connect_sdu,
                handle: ul_handle,
                endpoint_id: ul_endpoint_id,
                link_id: ul_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: Some(CmceChanAllocReq {
                    usage: Some(circuit.usage),
                    alloc_type: ChanAllocType::Replace,
                    carrier: None,
                    timeslots,
                    ul_dl_assigned: UlDlAssignment::Both,
                }),
                main_address: calling_party,
                tx_reporter: None,
            }),
        };
        queue.push_back(connect_msg);

        // 3) D-SETUP to group.
        let d_setup = DSetup {
            call_identifier: circuit.call_id,
            call_time_out: self.config_call_timeout(),
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            basic_service_information: pdu.basic_service_information.clone(),
            transmission_grant: TransmissionGrant::GrantedToOtherUser,
            transmission_request_permission: false,
            call_priority: pdu.call_priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(calling_party.ssi),
            calling_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        self.cached_setups.insert(
            circuit.call_id,
            CachedSetup {
                pdu: d_setup,
                dest_addr,
                resend: true,
                is_individual: false,
            },
        );
        let d_setup_ref = &self.cached_setups.get(&circuit.call_id).unwrap().pdu;

        let (setup_sdu, setup_chan_alloc) = Self::build_d_setup_prim(d_setup_ref, circuit.usage, circuit.ts, UlDlAssignment::Both);
        let setup_msg = Self::build_sapmsg(setup_sdu, Some(setup_chan_alloc), dest_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(setup_msg);

        // Track active group call.
        self.active_calls.insert(
            circuit.call_id,
            ActiveCall::new_local(
                calling_party,
                dest_gssi,
                calling_party.ssi,
                circuit.ts,
                circuit.usage,
                self.dltime,
                self.config_call_timeout(),
            ),
        );

        if net_brew::is_brew_gssi_routable(&self.config, dest_gssi) {
            let msg = SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id: circuit.call_id,
                    source_issi: calling_party.ssi,
                    dest_gssi,
                    ts: circuit.ts,
                }),
            };
            queue.push_back(msg);
        }
    }

    /// Handle U-SETUP for point-to-point (individual) duplex calls.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_p2p(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
    ) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let is_issi_address = pdu.called_party_type_identifier == tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Ssi || pdu.called_party_type_identifier == tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi;
        if !is_issi_address && !net_brew::is_active(&self.config) {
            tracing::warn!(
                "U-SETUP P2P with non-ISSI called_party_type_identifier={} (rejecting, Brew disabled)",
                pdu.called_party_type_identifier
            );
            return;
        }
        if is_issi_address
            && (pdu.called_party_short_number_address.is_some()
                || (pdu.called_party_extension.is_some() && pdu.called_party_type_identifier != tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi))
        {
            tracing::warn!("U-SETUP P2P with invalid called party fields (short number/extension mismatch), rejecting");
            return;
        }

        let called_ssi = pdu.called_party_ssi.map(|v| v as u32).unwrap_or(0);
        let has_external_number = pdu.external_subscriber_number.is_some() || pdu.called_party_short_number_address.is_some();
        if called_ssi == 0 && !has_external_number {
            tracing::warn!("U-SETUP P2P without called ISSI/number, ignoring");
            return;
        }

        let called_addr = TetraAddress::new(called_ssi, SsiType::Issi);

        // PBX/phone calls (no concrete local ISSI) always go through Brew.
        if called_ssi == 0 {
            self.fsm_on_u_setup_p2p_over_brew(queue, message, pdu, calling_party, called_addr);
            return;
        }

        if !self.subscriber_groups.contains_key(&called_addr.ssi) {
            self.fsm_on_u_setup_p2p_over_brew(queue, message, pdu, calling_party, called_addr);
            return;
        }

        if let Some((active_call_id, state)) = self.find_individual_call_by_issi(called_addr.ssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {} (called party busy in call_id={} state={:?})",
                calling_party.ssi,
                called_addr.ssi,
                active_call_id,
                state
            );
            let reject_call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(reject_call_id, DisconnectCause::CalledPartyBusy);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        // Allocate circuit(s). Duplex uses two traffic timeslots, one per MS, with cross-routing.
        let (circuit_calling, circuit_called) = {
            let mut state = self.config.state_write();
            let circuit_calling = match self.circuits.allocate_circuit_with_allocator_duplex(Direction::Both, pdu.basic_service_information.communication_type, pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {}, failed to allocate circuit for U-SETUP P2P, error: {:?}",
                        calling_party.ssi,
                        called_addr.ssi,
                        e
                    );
                    let call_id = self.circuits.get_next_call_id();
                    let sdu = Self::build_d_release(call_id, DisconnectCause::CongestionInInfrastructure);
                    let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
                    queue.push_back(msg);
                    return;
                }
            };

            let circuit_called = if pdu.simplex_duplex_selection {
                match self.circuits.allocate_circuit_for_call_with_allocator(
                    circuit_calling.call_id,
                    Direction::Both,
                    pdu.basic_service_information.communication_type,
                    pdu.simplex_duplex_selection,
                    &mut state.timeslot_alloc,
                    TimeslotOwner::Cmce,
                ) {
                    Ok(circuit) => Some(circuit.clone()),
                    Err(e) => {
                        let _ = self.circuits.close_circuit(Direction::Both, circuit_calling.ts);
                        let _ = state.timeslot_alloc.release(TimeslotOwner::Cmce, circuit_calling.ts);
                        tracing::info!(
                            "CMCE: rejecting U-SETUP P2P from ISSI {} to ISSI {}, failed to allocate second circuit for duplex P2P, error {:?}",
                            calling_party.ssi,
                            called_addr.ssi,
                            e
                        );
                        let call_id = self.circuits.get_next_call_id();
                        let sdu = Self::build_d_release(call_id, DisconnectCause::CongestionInInfrastructure);
                        let msg =
                            Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
                        queue.push_back(msg);
                        return;
                    }
                }
            } else {
                None
            };

            (circuit_calling, circuit_called)
        };

        let calling_ts = circuit_calling.ts;
        let calling_usage = circuit_calling.usage;
        let call_id = circuit_calling.call_id;
        let (called_ts, called_usage) = if let Some(called) = &circuit_called {
            (called.ts, called.usage)
        } else {
            (calling_ts, calling_usage)
        };

        tracing::info!(
            "rx_u_setup_p2p: call from ISSI {} to ISSI {} -> call_id={} ts(call)={} usage(call)={} ts(called)={} usage(called)={}",
            calling_party.ssi,
            called_addr.ssi,
            call_id,
            calling_ts,
            calling_usage,
            called_ts,
            called_usage
        );

        // Emit telemetry event for dashboard
        self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallStarted {
            call_id,
            calling_issi: calling_party.ssi,
            called_issi: called_addr.ssi,
            simplex: !pdu.hook_method_selection,
        });

        // Do not open traffic channel yet. Let called MS respond on MCCH.
        self.send_d_call_proceeding(queue, message, pdu, call_id, CallTimeoutSetupPhase::T60s, pdu.hook_method_selection);

        let d_setup = DSetup {
            call_identifier: call_id,
            call_time_out: self.config_call_timeout(),
            hook_method_selection: pdu.hook_method_selection,
            simplex_duplex_selection: pdu.simplex_duplex_selection,
            basic_service_information: pdu.basic_service_information.clone(),
            transmission_grant: TransmissionGrant::NotGranted,
            transmission_request_permission: false,
            call_priority: pdu.call_priority,
            notification_indicator: None,
            temporary_address: None,
            calling_party_address_ssi: Some(calling_party.ssi),
            calling_party_extension: None,
            external_subscriber_number: None,
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
                resend: true,
                is_individual: true,
            },
        );

        let d_setup_ref = &self.cached_setups.get(&call_id).unwrap().pdu;
        let mut setup_sdu = BitBuffer::new_autoexpand(80);
        d_setup_ref.to_bitbuf(&mut setup_sdu).expect("Failed to serialize DSetup");
        setup_sdu.seek(0);
        let setup_msg = Self::build_sapmsg(setup_sdu, None, called_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(setup_msg);

        if let Err(err) = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: calling_party,
                called_addr,
                calling_handle: prim.handle,
                calling_link_id: prim.link_id,
                calling_endpoint_id: prim.endpoint_id,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts,
                called_ts,
                calling_usage,
                called_usage,
                simplex_duplex: pdu.simplex_duplex_selection,
                state: IndividualCallState::CallSetupPending,
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout: self.config_call_timeout(),
                called_over_brew: false,
                calling_over_brew: false,
                brew_uuid: None,
                network_call: None,
                connect_request_sent: false,
                floor_holder: None,
            },
        ) {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating local P2P setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("CMCE: local P2P setup call_id={} creation rejected for state {:?}", call_id, state);
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }

    /// Handle U-SETUP for non-local ISSI, PBX and phone calls via Brew.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_setup_p2p_over_brew(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu: &USetup,
        calling_party: TetraAddress,
        called_addr: TetraAddress,
    ) {
        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };
        let mut network_call = Self::build_network_circuit_call_from_u_setup(pdu, calling_party.ssi);

        if !net_brew::is_active(&self.config) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P from ISSI {} (Brew disabled, called_ssi={})",
                calling_party.ssi,
                called_addr.ssi
            );
            let call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(call_id, DisconnectCause::RequestedServiceNotAvailable);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        if !self.config.state_read().network_connected {
            tracing::info!(
                "CMCE: rejecting U-SETUP over Brew src={} dst={} (backhaul disconnected)",
                calling_party.ssi,
                called_addr.ssi
            );
            let call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(call_id, DisconnectCause::RequestedServiceNotAvailable);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        if !net_brew::is_brew_issi_routable(&self.config, calling_party.ssi) {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P over Brew src={} dst={} (source ISSI not routable)",
                calling_party.ssi,
                called_addr.ssi
            );
            let call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(call_id, DisconnectCause::CalledPartyNotReachable);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        let has_external_called_party = Self::has_external_called_party(pdu, &network_call);
        let destination_routable = network_call.destination == 0 || net_brew::is_brew_issi_routable(&self.config, network_call.destination);

        if !has_external_called_party && !destination_routable {
            tracing::info!(
                "CMCE: rejecting U-SETUP P2P over Brew src={} dst={} (destination ISSI not routable)",
                calling_party.ssi,
                network_call.destination
            );
            let call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(call_id, DisconnectCause::CalledPartyNotReachable);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
            queue.push_back(msg);
            return;
        }

        if has_external_called_party && !destination_routable && network_call.destination != 0 {
            tracing::debug!(
                "CMCE: overriding non-routable destination SSI {} with 0 for external-number call src={} number='{}'",
                network_call.destination,
                calling_party.ssi,
                network_call.number
            );
            network_call.destination = 0;
        }

        // Allocate one bearer for the local MS.
        let circuit_calling = {
            let mut state = self.config.state_write();
            match self.circuits.allocate_circuit_with_allocator_duplex(Direction::Both, pdu.basic_service_information.communication_type, pdu.simplex_duplex_selection,
                &mut state.timeslot_alloc,
                TimeslotOwner::Cmce,
            ) {
                Ok(circuit) => circuit.clone(),
                Err(e) => {
                    tracing::info!(
                        "CMCE: rejecting U-SETUP over Brew src={} dst={} (allocation failed: {:?})",
                        calling_party.ssi,
                        called_addr.ssi,
                        e
                    );
                    let call_id = self.circuits.get_next_call_id();
                    let sdu = Self::build_d_release(call_id, DisconnectCause::CongestionInInfrastructure);
                    let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
                    queue.push_back(msg);
                    return;
                }
            }
        };

        let call_id = circuit_calling.call_id;
        let ts = circuit_calling.ts;
        let usage = circuit_calling.usage;
        let brew_uuid = uuid::Uuid::new_v4();

        tracing::info!(
            "CMCE: forwarding U-SETUP over Brew call_id={} src={} dst={} ts={} duplex={} number='{}' uuid={}",
            call_id,
            calling_party.ssi,
            network_call.destination,
            ts,
            network_call.duplex,
            network_call.number,
            brew_uuid
        );

        self.send_d_call_proceeding(queue, message, pdu, call_id, CallTimeoutSetupPhase::T60s, pdu.hook_method_selection);

        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Brew,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
                brew_uuid,
                call: network_call.clone(),
            }),
        });

        if let Err(err) = self.fsm_individual_create_setup_call(
            call_id,
            IndividualCall {
                calling_addr: calling_party,
                called_addr,
                calling_handle: prim.handle,
                calling_link_id: prim.link_id,
                calling_endpoint_id: prim.endpoint_id,
                called_handle: None,
                called_link_id: None,
                called_endpoint_id: None,
                calling_ts: ts,
                called_ts: ts,
                calling_usage: usage,
                called_usage: usage,
                simplex_duplex: pdu.simplex_duplex_selection,
                state: IndividualCallState::CallSetupPending,
                setup_timer_started: Some(self.dltime),
                setup_timeout: Some(CallTimeoutSetupPhase::T60s),
                active_timer_started: None,
                call_timeout: self.config_call_timeout(),
                called_over_brew: true,
                calling_over_brew: false,
                brew_uuid: Some(brew_uuid),
                network_call: Some(network_call),
                connect_request_sent: false,
                floor_holder: None,
            },
        ) {
            match err {
                IndividualTransitionError::DuplicateCall(_) => {
                    tracing::warn!("CMCE: duplicate call_id={} while creating Brew P2P setup", call_id);
                }
                IndividualTransitionError::InvalidTransition { state, .. } => {
                    tracing::warn!("CMCE: Brew P2P setup call_id={} creation rejected for state {:?}", call_id, state);
                }
                IndividualTransitionError::UnknownCall(_)
                | IndividualTransitionError::MissingBrewUuid(_)
                | IndividualTransitionError::NotBrewOriginated(_)
                | IndividualTransitionError::ConnectRequestAlreadySent(_) => {}
            }
        }
    }
}
