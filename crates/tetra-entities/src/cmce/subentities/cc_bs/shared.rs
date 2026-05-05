use super::*;

impl CcBsSubentity {
    pub fn new(config: SharedConfig) -> Self {
        CcBsSubentity {
            config,
            dltime: TdmaTime::default(),
            cached_setups: HashMap::new(),
            circuits: CircuitMgr::new(),
            active_calls: HashMap::new(),
            individual_calls: HashMap::new(),
            subscriber_groups: HashMap::new(),
            group_listeners: HashMap::new(),
        }
    }

    pub fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    pub(super) fn build_d_setup_prim(pdu: &DSetup, usage: u8, ts: u8, ul_dl: UlDlAssignment) -> (BitBuffer, CmceChanAllocReq) {
        tracing::debug!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(80);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DSetup");
        sdu.seek(0);

        let mut timeslots = [false; 4];
        timeslots[ts as usize - 1] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(usage),
            alloc_type: ChanAllocType::Replace,
            carrier: None,
            timeslots,
            ul_dl_assigned: ul_dl,
        };
        (sdu, chan_alloc)
    }

    /// Build a generic SAP message addressed to MLE via LCMC.
    /// `layer2service` controls acknowledged vs unacknowledged LLC.
    pub(super) fn build_sapmsg(
        sdu: BitBuffer,
        chan_alloc: Option<CmceChanAllocReq>,
        address: TetraAddress,
        layer2service: Layer2Service,
        reporter: Option<TxReporter>,
    ) -> SapMsg {
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc,
                main_address: address,
                tx_reporter: reporter,
            }),
        }
    }

    /// Build a SAP message with explicit LLC link context (handle/link_id/endpoint_id).
    /// Used for individually-addressed responses that must be routed back through
    /// the established LLC link of a specific MS.
    pub(super) fn build_sapmsg_direct(
        sdu: BitBuffer,
        address: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
    ) -> SapMsg {
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle,
                endpoint_id,
                link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: address,
                tx_reporter: None,
            }),
        }
    }

    /// Build a SAP message using FACCH stealing on a traffic channel timeslot.
    /// ETSI EN 300 392-2 §21: FACCH stealing allows signalling PDUs to be
    /// transmitted in place of voice on an active TCH.
    pub(super) fn build_sapmsg_stealing(sdu: BitBuffer, address: TetraAddress, ts: u8, usage: Option<u8>) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl(sdu, address, ts, usage, UlDlAssignment::Both)
    }

    /// Like `build_sapmsg_stealing` but with an explicit UL/DL assignment.
    /// Used for simplex PTT floor changes: the new speaker gets `Ul`, the listener gets `Dl`.
    pub(super) fn build_sapmsg_stealing_ul_dl(
        sdu: BitBuffer,
        address: TetraAddress,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
    ) -> SapMsg {
        let mut timeslots = [false; 4];
        timeslots[(ts - 1) as usize] = true;
        let chan_alloc = CmceChanAllocReq {
            usage,
            carrier: None,
            timeslots,
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned,
        };

        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: true,
                stealing_repeats_flag: false,
                chan_alloc: Some(chan_alloc),
                main_address: address,
                tx_reporter: None,
            }),
        }
    }

    pub(super) fn build_d_release(call_identifier: u16, disconnect_cause: DisconnectCause) -> BitBuffer {
        let pdu = DRelease {
            call_identifier,
            disconnect_cause,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DRelease");
        sdu.seek(0);
        sdu
    }

    pub(super) fn build_d_release_from_d_setup(d_setup_pdu: &DSetup, disconnect_cause: DisconnectCause) -> BitBuffer {
        Self::build_d_release(d_setup_pdu.call_identifier, disconnect_cause)
    }

    pub(super) fn has_listener(&self, gssi: u32) -> bool {
        self.group_listeners.get(&gssi).copied().unwrap_or(0) > 0
    }

    pub(super) fn inc_group_listener(&mut self, gssi: u32) {
        let entry = self.group_listeners.entry(gssi).or_insert(0);
        *entry += 1;
    }

    pub(super) fn dec_group_listener(&mut self, gssi: u32) {
        if let Some(entry) = self.group_listeners.get_mut(&gssi) {
            if *entry <= 1 {
                self.group_listeners.remove(&gssi);
            } else {
                *entry -= 1;
            }
        }
    }

    pub(super) fn find_individual_call_by_issi(&self, issi: u32) -> Option<(u16, IndividualCallState)> {
        self.individual_calls
            .iter()
            .find(|(_, call)| call.calling_addr.ssi == issi || call.called_addr.ssi == issi)
            .map(|(call_id, call)| (*call_id, call.state))
    }

    pub(super) fn find_brew_individual_call(&self, brew_uuid: uuid::Uuid) -> Option<(u16, IndividualCall)> {
        self.individual_calls
            .iter()
            .find(|(_, call)| call.brew_uuid == Some(brew_uuid))
            .map(|(call_id, call)| (*call_id, call.clone()))
    }

    pub(super) fn drop_group_calls_if_unlistened(&mut self, queue: &mut MessageQueue, gssi: u32) {
        if self.has_listener(gssi) {
            return;
        }

        let to_drop: Vec<(u16, CallOrigin)> = self
            .active_calls
            .iter()
            .filter(|(_, call)| call.dest_gssi == gssi)
            .map(|(call_id, call)| (*call_id, call.origin.clone()))
            .collect();

        for (call_id, origin) in to_drop {
            tracing::info!("CMCE: dropping call_id={} gssi={} (no listeners)", call_id, gssi);
            if let CallOrigin::Network { brew_uuid } = origin {
                if net_brew::is_brew_gssi_routable(&self.config, gssi) {
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
                    });
                };
            };
            self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        }
    }

    pub fn handle_subscriber_update(&mut self, queue: &mut MessageQueue, update: MmSubscriberUpdate) {
        let issi = update.issi;
        let groups = update.groups;

        match update.action {
            BrewSubscriberAction::Register => {
                let known = self.subscriber_groups.contains_key(&issi);
                self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                tracing::info!("CMCE: subscriber register issi={} known={}", issi, known);
            }
            BrewSubscriberAction::Deregister => {
                if let Some(existing) = self.subscriber_groups.remove(&issi) {
                    for gssi in existing {
                        self.dec_group_listener(gssi);
                        self.drop_group_calls_if_unlistened(queue, gssi);
                    }
                }
                tracing::info!("CMCE: subscriber deregister issi={}", issi);
            }
            BrewSubscriberAction::Affiliate => {
                let mut new_groups = Vec::new();
                {
                    let entry = self.subscriber_groups.entry(issi).or_insert_with(HashSet::new);
                    for gssi in groups {
                        if entry.insert(gssi) {
                            new_groups.push(gssi);
                        }
                    }
                }
                for gssi in &new_groups {
                    self.inc_group_listener(*gssi);
                }

                if new_groups.is_empty() {
                    tracing::debug!("CMCE: affiliate ignored (no new groups) issi={}", issi);
                } else {
                    tracing::info!("CMCE: subscriber affiliate issi={} groups={:?}", issi, new_groups);
                }
            }
            BrewSubscriberAction::Deaffiliate => {
                let mut removed_groups = Vec::new();
                let mut known_issi = false;
                if let Some(entry) = self.subscriber_groups.get_mut(&issi) {
                    known_issi = true;
                    for gssi in groups {
                        if entry.remove(&gssi) {
                            removed_groups.push(gssi);
                        }
                    }
                } else {
                    removed_groups = groups;
                }
                if known_issi {
                    for gssi in &removed_groups {
                        self.dec_group_listener(*gssi);
                    }
                }

                if removed_groups.is_empty() {
                    tracing::debug!("CMCE: deaffiliate ignored (no matching groups) issi={}", issi);
                } else {
                    tracing::info!("CMCE: subscriber deaffiliate issi={} groups={:?}", issi, removed_groups);
                    for gssi in &removed_groups {
                        self.drop_group_calls_if_unlistened(queue, *gssi);
                    }
                }
            }
        }
    }

    /// Send D-CALL-PROCEEDING (ETSI 14.7.1 step 1 of call setup).
    pub(super) fn send_d_call_proceeding(
        &mut self,
        queue: &mut MessageQueue,
        message: &SapMsg,
        pdu_request: &USetup,
        call_id: u16,
        setup_timeout: CallTimeoutSetupPhase,
        hook_method_selection: bool,
    ) {
        tracing::trace!("send_d_call_proceeding");

        let SapMsgInner::LcmcMleUnitdataInd(prim) = &message.msg else {
            panic!()
        };

        let pdu_response = DCallProceeding {
            call_identifier: call_id,
            call_time_out_set_up_phase: setup_timeout,
            hook_method_selection,
            simplex_duplex_selection: pdu_request.simplex_duplex_selection,
            basic_service_information: None,
            call_status: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(25);
        pdu_response.to_bitbuf(&mut sdu).expect("Failed to serialize DCallProceeding");
        sdu.seek(0);
        tracing::debug!("send_d_call_proceeding: -> {:?} sdu {}", pdu_response, sdu.dump_bin());

        let msg = SapMsg {
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
                main_address: prim.received_tetra_address,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Send D-ALERT to the calling MS for an individual call.
    /// ETSI EN 300 392-2 §14.7.3: BS sends D-ALERT after called MS responds with U-ALERT.
    pub(super) fn send_d_alert_individual(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        simplex_duplex: bool,
        calling_addr: TetraAddress,
        calling_handle: u32,
        calling_link_id: u32,
        calling_endpoint_id: u32,
        setup_timeout: CallTimeoutSetupPhase,
    ) {
        let d_alert = DAlert {
            call_identifier: call_id,
            call_time_out_set_up_phase: setup_timeout.into_raw() as u8,
            reserved: true, // per spec note: set to 1 for backwards compatibility
            simplex_duplex_selection: simplex_duplex,
            call_queued: false,
            basic_service_information: None,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };

        tracing::info!("-> {:?}", d_alert);
        let mut sdu = BitBuffer::new_autoexpand(32);
        d_alert.to_bitbuf(&mut sdu).expect("Failed to serialize DAlert");
        sdu.seek(0);

        let msg = SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: calling_handle,
                endpoint_id: calling_endpoint_id,
                link_id: calling_link_id,
                layer2service: Layer2Service::Unacknowledged,
                pdu_prio: 0,
                layer2_qos: 0,
                stealing_permission: false,
                stealing_repeats_flag: false,
                chan_alloc: None,
                main_address: calling_addr,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Decode External Subscriber Number IE (BCD-packed digits, ETSI 14.8.21).
    /// Type3FieldGeneric.data is a u64 (up to 64 bits packed).
    pub(super) fn decode_external_subscriber_number(field: &Type3FieldGeneric) -> String {
        if field.len == 0 {
            return String::new();
        }

        let nibble_count = (field.len / 4).min(16); // max 64 bits = 16 nibbles
        let mut digits = String::with_capacity(nibble_count);
        for i in 0..nibble_count {
            // Extract nibble from packed u64 (big-endian nibble order)
            let shift = 60 - (i * 4);
            let nibble = ((field.data >> shift) & 0x0f) as u8;
            match nibble {
                0..=9 => digits.push(char::from(b'0' + nibble)),
                0x0a => digits.push('*'),
                0x0b => digits.push('#'),
                0x0c..=0x0f => {}
                _ => {}
            }
        }
        digits
    }

    /// Encode External Subscriber Number IE (ETSI 14.8.21).
    /// Type3FieldGeneric.data is a u64 (up to 64 bits packed).
    pub(super) fn encode_external_subscriber_number(number: &str) -> Option<Type3FieldGeneric> {
        let trimmed = number.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut nibbles: Vec<u8> = Vec::with_capacity(16);
        let mut encoded_preview = String::with_capacity(16);

        for ch in trimmed.chars() {
            let nibble = match ch {
                '0'..='9' => ch as u8 - b'0',
                '*' => 0x0a,
                '#' => 0x0b,
                _ => {
                    tracing::debug!("CMCE: ignoring unsupported external number char '{}' in '{}'", ch, number);
                    continue;
                }
            };

            if nibbles.len() == 16 {
                tracing::debug!(
                    "CMCE: truncating external number '{}' to first 16 BCD digits ('{}')",
                    number,
                    encoded_preview
                );
                break;
            }

            nibbles.push(nibble);
            encoded_preview.push(ch);
        }

        if nibbles.is_empty() {
            tracing::debug!("CMCE: external number '{}' has no encodable digits", number);
            return None;
        }

        let len_bits = nibbles.len() * 4;
        let mut data: u64 = 0;
        for (idx, nibble) in nibbles.into_iter().enumerate() {
            let shift = 60 - (idx * 4);
            data |= (nibble as u64) << shift;
        }

        Some(Type3FieldGeneric {
            field_id: CmceType3ElemId::ExtSubscriberNum.into_raw(),
            len: len_bits,
            data,
        })
    }

    pub(super) fn build_network_circuit_call_from_u_setup(pdu: &USetup, source_issi: u32) -> NetworkCircuitCall {
        let number = pdu
            .external_subscriber_number
            .as_ref()
            .map(Self::decode_external_subscriber_number)
            .unwrap_or_default();

        NetworkCircuitCall {
            source_issi,
            destination: pdu.called_party_ssi.unwrap_or(0) as u32,
            number,
            priority: pdu.call_priority,
            service: pdu.basic_service_information.speech_service.unwrap_or(0),
            mode: pdu.basic_service_information.circuit_mode_type.into_raw() as u8,
            duplex: pdu.simplex_duplex_selection as u8,
            method: pdu.hook_method_selection as u8,
            communication: pdu.basic_service_information.communication_type.into_raw() as u8,
            grant: 0,
            permission: pdu.request_to_transmit_send_data as u8,
            timeout: CallTimeout::T5m.into_raw() as u8,
            ownership: 1,
            queued: 0,
        }
    }

    #[inline]
    pub(super) fn has_external_called_party(pdu: &USetup, network_call: &NetworkCircuitCall) -> bool {
        !network_call.number.is_empty()
            || pdu.external_subscriber_number.is_some()
            || pdu.called_party_short_number_address.is_some()
    }

    /// Send D-DISCONNECT to the other party of an individual call.
    pub(super) fn send_d_disconnect_individual(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        call_snapshot: &IndividualCall,
        sender: TetraAddress,
        disconnect_cause: DisconnectCause,
    ) {
        let target_addr = if sender.ssi == call_snapshot.calling_addr.ssi {
            Some(call_snapshot.called_addr)
        } else if sender.ssi == call_snapshot.called_addr.ssi {
            Some(call_snapshot.calling_addr)
        } else {
            tracing::warn!(
                "U-DISCONNECT/U-RELEASE (individual) call_id={} from unexpected ISSI {} (calling {}, called {})",
                call_id,
                sender.ssi,
                call_snapshot.calling_addr.ssi,
                call_snapshot.called_addr.ssi
            );
            None
        };

        let Some(target_addr) = target_addr else {
            return;
        };

        let target_ts = if target_addr.ssi == call_snapshot.calling_addr.ssi {
            call_snapshot.calling_ts
        } else {
            call_snapshot.called_ts
        };

        let d_disconnect = DDisconnect {
            call_identifier: call_id,
            disconnect_cause,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?} (to ISSI {})", d_disconnect, target_addr.ssi);

        let mut sdu = BitBuffer::new_autoexpand(32);
        d_disconnect.to_bitbuf(&mut sdu).expect("Failed to serialize DDisconnect");
        sdu.seek(0);

        let msg = if call_snapshot.state == IndividualCallState::Active {
            let usage = if target_addr.ssi == call_snapshot.calling_addr.ssi {
                Some(call_snapshot.calling_usage)
            } else {
                Some(call_snapshot.called_usage)
            };
            Self::build_sapmsg_stealing(sdu, target_addr, target_ts, usage)
        } else if target_addr.ssi == call_snapshot.calling_addr.ssi {
            Self::build_sapmsg_direct(
                sdu,
                target_addr,
                call_snapshot.calling_handle,
                call_snapshot.calling_link_id,
                call_snapshot.calling_endpoint_id,
            )
        } else if let (Some(handle), Some(link_id), Some(endpoint_id)) = (
            call_snapshot.called_handle,
            call_snapshot.called_link_id,
            call_snapshot.called_endpoint_id,
        ) {
            Self::build_sapmsg_direct(sdu, target_addr, handle, link_id, endpoint_id)
        } else {
            Self::build_sapmsg(sdu, None, target_addr, Layer2Service::Unacknowledged, None)
        };
        queue.push_back(msg);
    }

    /// Notify UMAC to open a traffic circuit (ETSI §21 circuit management).
    /// `peer_ts` is Some only for full-duplex calls where UL of one MS feeds DL of the other.
    pub(super) fn signal_umac_circuit_open(
        queue: &mut MessageQueue,
        call: &CmceCircuit,
        peer_ts: Option<u8>,
        dl_media_source: CircuitDlMediaSource,
    ) {
        let circuit = Circuit {
            direction: call.direction,
            ts: call.ts,
            peer_ts,
            usage: call.usage,
            circuit_mode: call.circuit_mode,
            speech_service: call.speech_service,
            etee_encrypted: call.etee_encrypted,
            dl_media_source,
        };
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::Open(circuit)),
        };
        queue.push_back(cmd);
    }

    pub(super) fn signal_umac_circuit_close(queue: &mut MessageQueue, circuit: CmceCircuit) {
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::Close(circuit.direction, circuit.ts)),
        };
        queue.push_back(cmd);
    }

    /// Validate U-SETUP PDU for supported features.
    /// ETSI EN 300 392-2 §14.7.1: BS must check service compatibility before accepting.
    pub(super) fn feature_check_u_setup(pdu: &USetup) -> bool {
        let mut supported = true;

        if !(pdu.area_selection == 0 || pdu.area_selection == 1) {
            unimplemented_log!("Area selection not supported: {}", pdu.area_selection);
            supported = false;
        };
        // Both simplex and duplex are supported for P2P calls.
        // Group/broadcast remain simplex only.
        if pdu.basic_service_information.communication_type != CommunicationType::P2p
            && pdu.simplex_duplex_selection
        {
            unimplemented_log!(
                "Duplex only supported for P2P calls (comm_type={})",
                pdu.basic_service_information.communication_type
            );
            supported = false;
        };
        if pdu.clir_control != 0 {
            unimplemented_log!("clir_control not supported: {}", pdu.clir_control);
        };
        if pdu.called_party_ssi.is_none()
            && pdu.called_party_short_number_address.is_none()
            && pdu.external_subscriber_number.is_none()
        {
            unimplemented_log!("U-SETUP called party not set (no SSI, short number or external number)");
        };
        if pdu.called_party_extension.is_some() && pdu.called_party_type_identifier != tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier::Tsi {
            unimplemented_log!(
                "U-SETUP called_party_extension present with unexpected called_party_type_identifier={}",
                pdu.called_party_type_identifier
            );
        };
        if let Some(v) = &pdu.facility {
            unimplemented_log!("facility not supported: {:?}", v);
        };
        if let Some(v) = &pdu.dm_ms_address {
            unimplemented_log!("dm_ms_address not supported: {:?}", v);
        };
        if let Some(v) = &pdu.proprietary {
            unimplemented_log!("proprietary not supported: {:?}", v);
        };

        supported
    }


    /// Map call_timeout_secs from config to the nearest ETSI CallTimeout enum value.
    /// ETSI EN 300 392-2 Table 14.50: BS sets D-SETUP call_time_out to indicate max call duration.
    pub(super) fn config_call_timeout(&self) -> CallTimeout {
        let secs = self.config.config().cell.call_timeout_secs;
        match secs {
            0..=37  => CallTimeout::T30s,
            38..=52 => CallTimeout::T45s,
            53..=90 => CallTimeout::T60s,
            91..=150 => CallTimeout::T2m,
            151..=210 => CallTimeout::T3m,
            211..=270 => CallTimeout::T4m,
            271..=330 => CallTimeout::T5m,
            331..=420 => CallTimeout::T6m,
            421..=540 => CallTimeout::T8m,
            _          => CallTimeout::T5m,
        }
    }

    /// Send D-TX GRANTED via FACCH stealing on the group traffic channel.
    pub(super) fn send_d_tx_granted_facch(&mut self, queue: &mut MessageQueue, call_id: u16, source_issi: u32, dest_gssi: u32, ts: u8) {
        let pdu = DTxGranted {
            call_identifier: call_id,
            transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
            transmission_request_permission: false,
            encryption_control: false,
            reserved: false,
            notification_indicator: None,
            transmitting_party_type_identifier: Some(1), // SSI
            transmitting_party_address_ssi: Some(source_issi as u64),
            transmitting_party_extension: None,
            external_subscriber_number: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::debug!("-> D-TX GRANTED (FACCH) {:?}", pdu);
        let mut sdu = BitBuffer::new_autoexpand(30);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxGranted");
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        let msg = Self::build_sapmsg_stealing(sdu, dest_addr, ts, None);
        queue.push_back(msg);
    }

    /// Send D-TX CEASED via FACCH stealing on the group traffic channel.
    pub(super) fn send_d_tx_ceased_facch(&mut self, queue: &mut MessageQueue, call_id: u16, dest_gssi: u32, ts: u8) {
        let pdu = DTxCeased {
            call_identifier: call_id,
            transmission_request_permission: false, // ETSI 14.8.43: 0 = allowed to request transmission
            notification_indicator: None,
            facility: None,
            dm_ms_address: None,
            proprietary: None,
        };

        tracing::debug!("-> D-TX CEASED (FACCH) {:?}", pdu);
        let mut sdu = BitBuffer::new_autoexpand(30);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DTxCeased");
        sdu.seek(0);

        let dest_addr = TetraAddress::new(dest_gssi, SsiType::Gssi);
        let msg = Self::build_sapmsg_stealing(sdu, dest_addr, ts, None);
        queue.push_back(msg);
    }

    /// Release a group call: send D-RELEASE, close circuit, clean up state.
    pub(super) fn release_group_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        let Some(cached) = self.cached_setups.get(&call_id) else {
            tracing::error!("No cached D-SETUP for call_id={}", call_id);
            return;
        };
        let dest_addr = cached.dest_addr;

        let sdu = Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause);
        let prim = Self::build_sapmsg(sdu, None, dest_addr, Layer2Service::Unacknowledged, None);
        queue.push_back(prim);

        if let Some(call) = self.active_calls.get(&call_id) {
            // Extract all needed fields before any mutable borrow (release_timeslot).
            let ts = call.ts;
            let dest_ssi = call.dest_gssi;
            let is_local = matches!(call.origin, CallOrigin::Local { .. });
            let network_brew_uuid = if let CallOrigin::Network { brew_uuid } = call.origin {
                Some(brew_uuid)
            } else {
                None
            };

            if let Ok(circuit) = self.circuits.close_circuit(Direction::Both, ts) {
                Self::signal_umac_circuit_close(queue, circuit);
            }

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }),
            });

            self.release_timeslot(ts);

            if net_brew::is_brew_gssi_routable(&self.config, dest_ssi) {
                if is_local {
                    // Local call: notify Brew with generic CallEnded.
                    let notify = SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }),
                    };
                    queue.push_back(notify);
                } else if let Some(brew_uuid) = network_brew_uuid {
                    // Network call: notify Brew with NetworkCallEnd so it stops sending
                    // NetworkCallStart for new speakers (which would cause an ExpiryOfTimer loop).
                    // Use origin uuid — call.brew_uuid may be None if cleared on hangtime entry.
                    tracing::info!(
                        "release_group_call: notifying Brew of expired network call_id={} brew_uuid={} cause={:?}",
                        call_id, brew_uuid, disconnect_cause
                    );
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Cmce,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
                    });
                }
            }
        }

        self.cached_setups.remove(&call_id);
        self.active_calls.remove(&call_id);
    }

    /// Release an individual call: send D-RELEASE to both parties, close circuits, clean up state.
    /// Handles both active (FACCH stealing) and setup-phase (MCCH) delivery.
    pub(super) fn release_individual_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        let Some(call) = self.individual_calls.remove(&call_id) else {
            tracing::warn!("No individual call for call_id={}", call_id);
            return;
        };

        let send_calling_leg = !call.calling_over_brew;
        let send_called_leg = !call.called_over_brew;

        const SETUP_RELEASE_REPEATS: usize = 3;

        if call.is_active() {
            // On active call: deliver via FACCH on traffic channel (send twice for reliability).
            for _ in 0..2 {
                let sdu_calling = if let Some(cached) = self.cached_setups.get(&call_id) {
                    Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
                } else {
                    Self::build_d_release(call_id, disconnect_cause)
                };
                let sdu_called = if let Some(cached) = self.cached_setups.get(&call_id) {
                    Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
                } else {
                    Self::build_d_release(call_id, disconnect_cause)
                };
                if send_calling_leg {
                    let prim_calling = Self::build_sapmsg_stealing(
                        sdu_calling,
                        call.calling_addr,
                        call.calling_ts,
                        Some(call.calling_usage),
                    );
                    queue.push_back(prim_calling);
                }
                if send_called_leg {
                    let prim_called = Self::build_sapmsg_stealing(
                        sdu_called,
                        call.called_addr,
                        call.called_ts,
                        Some(call.called_usage),
                    );
                    queue.push_back(prim_called);
                }
            }
        } else {
            // During setup: deliver on MCCH (force link_id=0, both parties monitor MCCH).
            for _ in 0..SETUP_RELEASE_REPEATS {
                let sdu_calling = if let Some(cached) = self.cached_setups.get(&call_id) {
                    Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
                } else {
                    Self::build_d_release(call_id, disconnect_cause)
                };
                let sdu_called = if let Some(cached) = self.cached_setups.get(&call_id) {
                    Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause)
                } else {
                    Self::build_d_release(call_id, disconnect_cause)
                };
                if send_calling_leg {
                    let prim_calling =
                        Self::build_sapmsg(sdu_calling, None, call.calling_addr, Layer2Service::Unacknowledged, None);
                    queue.push_back(prim_calling);
                }

                if send_called_leg {
                    let prim_called =
                        Self::build_sapmsg(sdu_called, None, call.called_addr, Layer2Service::Unacknowledged, None);
                    queue.push_back(prim_called);
                }
            }
        }

        // Close circuits for both legs (may be the same ts for simplex/Brew).
        let mut ts_list = vec![call.calling_ts];
        if call.called_ts != call.calling_ts {
            ts_list.push(call.called_ts);
        }
        for ts in ts_list {
            if let Ok(circuit) = self.circuits.close_circuit(Direction::Both, ts) {
                Self::signal_umac_circuit_close(queue, circuit);
            }

            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::CallEnded { call_id, ts }),
            });

            self.release_timeslot(ts);
        }
        self.cached_setups.remove(&call_id);

        if (call.called_over_brew || call.calling_over_brew) && disconnect_cause != DisconnectCause::SwmiRequestedDisconnection {
            if let Some(brew_uuid) = call.brew_uuid {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease {
                        brew_uuid,
                        cause: disconnect_cause.into_raw() as u8,
                    }),
                });
            }
        }
    }

    pub(super) fn release_timeslot(&mut self, ts: u8) {
        let mut state = self.config.state_write();
        if let Err(err) = state.timeslot_alloc.release(TimeslotOwner::Cmce, ts) {
            tracing::warn!("CcBsSubentity: failed to release timeslot ts={} err={:?}", ts, err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CcBsSubentity;

    #[test]
    fn external_subscriber_number_supports_16_digits() {
        let number = "1234567890123456";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 64);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), number);
    }

    #[test]
    fn external_subscriber_number_truncates_to_16_digits() {
        let number = "12345678901234567";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 64);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), "1234567890123456");
    }
}
