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
            telemetry: None,
        }
    }

    pub fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    /// Wire the dashboard telemetry sink so call-lifecycle events (Group/Individual
    /// CallStarted/CallEnded) reach the dashboard. Mirrors `SdsBsSubentity::set_telemetry`.
    pub fn set_telemetry(&mut self, sink: crate::net_telemetry::TelemetrySink) {
        self.telemetry = Some(sink);
    }

    /// Fire-and-forget emit of a telemetry event. No-op when telemetry is disabled.
    pub(super) fn emit(&self, event: crate::net_telemetry::TelemetryEvent) {
        if let Some(sink) = &self.telemetry {
            sink.send(event);
        }
    }

    pub(super) fn is_locally_registered_issi(&self, issi: u32) -> bool {
        let cmce_known = self.subscriber_groups.contains_key(&issi);
        let registry_known = self.config.state_read().subscribers.is_registered(issi);

        if cmce_known != registry_known {
            tracing::warn!(
                "CMCE: subscriber registry mismatch issi={} cmce_known={} registry_known={}",
                issi,
                cmce_known,
                registry_known
            );
        }

        registry_known
    }

    pub(super) fn known_local_issis(&self) -> Vec<u32> {
        self.config.state_read().subscribers.all_registered_issis().collect()
    }

    #[inline]
    pub(super) fn p2p_call_timeout(simplex_duplex: bool) -> CallTimeout {
        if simplex_duplex { CallTimeout::Infinite } else { CallTimeout::T5m }
    }

    pub(super) fn build_d_setup_prim(
        pdu: &DSetup,
        usage: u8,
        carrier_num: u16,
        ts: u8,
        ul_dl: UlDlAssignment,
    ) -> (BitBuffer, CmceChanAllocReq) {
        tracing::debug!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(80);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DSetup");
        sdu.seek(0);

        // Construct ChanAlloc descriptor for the allocated timeslot
        let mut timeslots = [false; 4];
        timeslots[ts as usize - 1] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(usage),
            alloc_type: ChanAllocType::Replace,
            carrier: Some(carrier_num),
            timeslots,
            ul_dl_assigned: ul_dl,
        };
        (sdu, chan_alloc)
    }

    pub(super) fn build_sapmsg(
        sdu: BitBuffer,
        chan_alloc: Option<CmceChanAllocReq>,
        _dltime: TdmaTime,
        address: TetraAddress,
        reporter: Option<TxReporter>,
    ) -> SapMsg {
        // Construct prim
        SapMsg {
            sap: Sap::LcmcSap,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                sdu,
                handle: 0,
                endpoint_id: 0,
                link_id: 0,
                // Unacknowledged BL-UDATA. This builder carries the MCCH/group-addressed sends —
                // D-SETUP and D-RELEASE to a GSSI have no single peer to ACK, so acknowledged LLC
                // (the `Todo` default) is wrong and can stall/retry at LLC. The legacy `main` code
                // sent every CC PDU here unacknowledged (FH FIX 2).
                layer2service: Layer2Service::Unacknowledged,
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

    pub(super) fn build_sapmsg_direct(
        sdu: BitBuffer,
        _dltime: TdmaTime,
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
                // Unacknowledged BL-UDATA. This builder serves the direct/reject broadcast paths
                // (e.g. congestion D-RELEASE in `reject_setup_request`); the legacy `main` code
                // hardcoded these unacknowledged (FH FIX 2).
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

    pub(super) fn build_sapmsg_stealing(
        sdu: BitBuffer,
        dltime: TdmaTime,
        address: TetraAddress,
        carrier_num: u16,
        ts: u8,
        usage: Option<u8>,
    ) -> SapMsg {
        Self::build_sapmsg_stealing_ul_dl(sdu, dltime, address, carrier_num, ts, usage, UlDlAssignment::Both)
    }

    pub(super) fn build_sapmsg_stealing_ul_dl(
        sdu: BitBuffer,
        _dltime: TdmaTime,
        address: TetraAddress,
        carrier_num: u16,
        ts: u8,
        usage: Option<u8>,
        ul_dl_assigned: UlDlAssignment,
    ) -> SapMsg {
        // For FACCH stealing on traffic channel, must specify target timeslot.
        let mut timeslots = [false; 4];
        timeslots[(ts - 1) as usize] = true;
        let chan_alloc = CmceChanAllocReq {
            usage,
            carrier: Some(carrier_num),
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
                // Unacknowledged BL-UDATA over FACCH stealing. Group floor PDUs (D-TX-CEASED /
                // D-SETUP late-entry re-sends) carried here are GSSI-addressed, so acknowledged
                // LLC would have no single peer to ACK; the legacy `main` code sent these
                // unacknowledged (FH FIX 2).
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

    pub(super) fn build_d_disconnect(call_identifier: u16, disconnect_cause: DisconnectCause) -> BitBuffer {
        let pdu = DDisconnect {
            call_identifier,
            disconnect_cause,
            notification_indicator: None,
            facility: None,
            proprietary: None,
        };
        tracing::info!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DDisconnect");
        sdu.seek(0);
        sdu
    }

    pub(super) fn build_d_disconnect_from_d_setup(d_setup_pdu: &DSetup, disconnect_cause: DisconnectCause) -> BitBuffer {
        Self::build_d_disconnect(d_setup_pdu.call_identifier, disconnect_cause)
    }

    pub(super) fn build_d_call_restore(
        call_identifier: u16,
        transmission_grant: TransmissionGrant,
        call_status: Option<CallStatus>,
    ) -> BitBuffer {
        let pdu = DCallRestore {
            call_identifier,
            transmission_grant: transmission_grant.into_raw() as u8,
            transmission_request_permission: false,
            reset_call_time_out_timer_t310_: true,
            new_call_identifier: None,
            call_time_out: None,
            call_status: call_status.map(CallStatus::into_raw),
            modify: None,
            notification_indicator: None,
            facility: None,
            temporary_address: None,
            dm_ms_address: None,
            proprietary: None,
        };
        tracing::info!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(48);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DCallRestore");
        sdu.seek(0);
        sdu
    }

    pub(super) fn build_d_info(call_identifier: u16, modify: Option<u64>, call_status: Option<CallStatus>, reset_t310: bool) -> BitBuffer {
        let pdu = DInfo {
            call_identifier,
            reset_call_time_out_timer_t310_: reset_t310,
            poll_request: false,
            new_call_identifier: None,
            call_time_out: None,
            call_time_out_set_up_phase_t301_t302_: None,
            call_ownership: None,
            modify,
            call_status: call_status.map(CallStatus::into_raw),
            temporary_address: None,
            notification_indicator: None,
            poll_response_percentage: None,
            poll_response_number: None,
            dtmf: None,
            facility: None,
            poll_response_addresses: None,
            proprietary: None,
        };
        tracing::info!("-> {:?}", pdu);

        let mut sdu = BitBuffer::new_autoexpand(64);
        pdu.to_bitbuf(&mut sdu).expect("Failed to serialize DInfo");
        sdu.seek(0);
        sdu
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

    // ── Dashboard / API helpers ────────────────────────────────────────────────

    /// Returns all currently registered ISSI values.
    pub fn subscriber_issis(&self) -> Vec<u32> {
        self.subscriber_groups.keys().copied().collect()
    }

    /// Returns the list of GSSIs the given ISSI is affiliated to.
    pub fn subscriber_groups_for(&self, issi: u32) -> Vec<u32> {
        self.subscriber_groups
            .get(&issi)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Force-deregister an MS: release its active calls and clean up state.
    /// Returns true if the MS was known.
    pub fn kick_ms(&mut self, queue: &mut MessageQueue, issi: u32) -> bool {
        if !self.subscriber_groups.contains_key(&issi) {
            tracing::warn!("CMCE: kick_ms issi={} not found in subscriber_groups", issi);
            return false;
        }
        // Release all active individual calls involving this MS
        let individual_ids: Vec<u16> = self
            .individual_calls
            .iter()
            .filter(|(_, c)| c.calling_addr.ssi == issi || c.called_addr.ssi == issi)
            .map(|(&id, _)| id)
            .collect();
        for id in individual_ids {
            self.release_individual_call(queue, id, DisconnectCause::UserRequestedDisconnection);
        }
        // Clean up CMCE state
        if let Some(groups) = self.subscriber_groups.remove(&issi) {
            for g in &groups {
                self.dec_group_listener(*g);
            }
        }
        // Tell MM to deregister the MS — this also notifies Brew
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mm,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                issi,
                groups: Vec::new(),
                action: BrewSubscriberAction::Deregister,
            }),
        });
        tracing::info!("CMCE: kick_ms issi={} — deregistered", issi);
        true
    }

    pub(super) fn find_individual_call_by_issi(&self, issi: u32) -> Option<(u16, IndividualCallState)> {
        self.individual_calls
            .iter()
            .find(|(_, call)| call.calling_addr.ssi == issi || call.called_addr.ssi == issi)
            .map(|(call_id, call)| (*call_id, call.state))
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
                if brew::is_brew_gssi_routable(&self.config, gssi) {
                    self.notify_network_call_end(queue, brew_uuid);
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
            basic_service_information: None, // Only needed if different from requested
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
                // D-CALL-PROCEEDING during setup: the legacy `main` code sent this unacknowledged
                // (FH FIX 2). It is a setup-phase MCCH response where the addressed MS is not yet
                // in a confirmed LLC link context, so acknowledged BL-DATA can stall.
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

    pub(super) fn send_d_alert_individual(
        &mut self,
        queue: &mut MessageQueue,
        _dltime: TdmaTime,
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
                // D-ALERT to the calling MS during individual setup: the legacy `main` code sent
                // this unacknowledged (FH FIX 2). Setup-phase MCCH signalling, same rationale as
                // D-CALL-PROCEEDING above.
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

    pub(super) fn decode_external_subscriber_number(field: &Type3FieldGeneric) -> String {
        if field.len == 0 {
            return String::new();
        }

        // External number IE is commonly BCD-like packed digits.
        // Keep best-effort conversion and drop filler nibbles.
        let len_bits = field.len.min(128);
        let nibble_count = (len_bits / 4).min(24);
        let mut digits = String::with_capacity(nibble_count);
        for i in 0..nibble_count {
            let shift = len_bits - ((i + 1) * 4);
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

    pub(super) fn encode_external_subscriber_number(number: &str) -> Option<Type3FieldGeneric> {
        let trimmed = number.trim();
        if trimmed.is_empty() {
            return None;
        }

        let mut nibbles = Vec::with_capacity(24);
        let mut encoded_preview = String::with_capacity(24);

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

            if nibbles.len() == 24 {
                tracing::debug!(
                    "CMCE: truncating external number '{}' to first 24 BCD digits ('{}')",
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
        let mut data = 0u128;
        for nibble in nibbles {
            data = (data << 4) | nibble as u128;
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
            timeout: Self::p2p_call_timeout(pdu.simplex_duplex_selection).into_raw() as u8,
            ownership: 1,
            queued: 0,
        }
    }

    #[inline]
    pub(super) fn has_external_called_party(pdu: &USetup, network_call: &NetworkCircuitCall) -> bool {
        !network_call.number.is_empty() || pdu.external_subscriber_number.is_some() || pdu.called_party_short_number_address.is_some()
    }

    /// Derive a usable display SSI for an inbound network call's calling party from its
    /// external (SIP/PBX) number when no real ISSI is available. Accepts a purely numeric
    /// extension within the 24-bit SSI range; rejects everything else. Mirrors the upstream
    /// fork's `external_number_as_ssi`.
    pub(super) fn external_number_as_ssi(number: &str) -> Option<u32> {
        let digits = number.trim();
        if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        let value = digits.parse::<u32>().ok()?;
        (value != 0 && value <= 0x00ff_ffff).then_some(value)
    }

    /// Decide whether a dialed (non-ISSI) number should be routed to the Asterisk SIP/RTP
    /// bridge instead of Brew. Returns the SIP number to dial (prefix-stripped when configured),
    /// or `None` to leave the call for Brew. Mirrors the upstream fork's `asterisk_route_number`.
    #[cfg(feature = "asterisk")]
    pub(super) fn asterisk_route_number(&self, network_call: &NetworkCircuitCall) -> Option<String> {
        let cfg = &self.config.config().asterisk;
        let raw = if !network_call.number.trim().is_empty() {
            network_call.number.trim().to_string()
        } else if network_call.destination != 0 {
            network_call.destination.to_string()
        } else {
            return None;
        };

        cfg.route_outbound_raw(&raw)
    }

    pub(super) fn signal_umac_circuit_open(
        queue: &mut MessageQueue,
        call: &CmceCircuit,
        _dltime: TdmaTime,
        peer_carrier_num: Option<u16>,
        peer_ts: Option<u8>,
        dl_media_source: CircuitDlMediaSource,
    ) {
        let circuit = Circuit {
            direction: call.direction,
            ts: call.ts,
            carrier_num: call.carrier_num,
            peer_carrier_num,
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

    pub(super) fn signal_umac_circuit_close(queue: &mut MessageQueue, circuit: CmceCircuit, _dltime: TdmaTime) {
        let cmd = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::CloseSlot {
                direction: circuit.direction,
                carrier_num: circuit.carrier_num,
                ts: circuit.ts,
            }),
        };
        queue.push_back(cmd);
    }

    pub(super) fn feature_check_u_setup(pdu: &USetup) -> bool {
        let mut supported = true;

        if !(pdu.area_selection == 0 || pdu.area_selection == 1) {
            unimplemented_log!("Area selection not supported: {}", pdu.area_selection);
            supported = false;
        };
        // if pdu.hook_method_selection {
        //     // We do not implement explicit hook transitions yet; force hook_method_selection=false in responses.
        //     unimplemented_log!("Hook method selection requested, forcing hook_method_selection=false");
        // };
        // Duplex is supported only for P2P calls. P2P supports both simplex and duplex.
        if pdu.basic_service_information.communication_type != CommunicationType::P2p && pdu.simplex_duplex_selection {
            unimplemented_log!(
                "Duplex only supported for P2P calls (comm_type={})",
                pdu.basic_service_information.communication_type
            );
            supported = false;
        }
        // if pdu.basic_service_information != 0xFC {
        //     // TODO FIXME implement parsing
        //     tracing::error!("Basic service information not supported: {}", pdu.basic_service_information);
        //     return;
        // };
        // request_to_transmit_send_data can be false for speech group calls — the MS
        // implicitly requests to transmit by initiating the call. No action needed.
        if pdu.clir_control != 0 {
            unimplemented_log!("clir_control not supported: {}", pdu.clir_control);
        };
        if pdu.called_party_ssi.is_none() && pdu.called_party_short_number_address.is_none() && pdu.external_subscriber_number.is_none() {
            unimplemented_log!("U-SETUP called party not set (no SSI, short number or external number)");
        };
        if pdu.called_party_extension.is_some() && pdu.called_party_type_identifier != PartyTypeIdentifier::Tsi {
            unimplemented_log!(
                "U-SETUP called_party_extension present with unexpected called_party_type_identifier={}",
                pdu.called_party_type_identifier
            );
        };
        // Then, we warn about some other unhandled/unsupported fields
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

    /// Send D-TX GRANTED via FACCH stealing
    pub(super) fn send_d_tx_granted_facch(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        source_issi: u32,
        dest_gssi: u32,
        carrier_num: u16,
        ts: u8,
    ) {
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
        let msg = Self::build_sapmsg_stealing(sdu, self.dltime, dest_addr, carrier_num, ts, None);
        queue.push_back(msg);
    }

    /// Send D-TX CEASED via FACCH stealing
    pub(super) fn send_d_tx_ceased_facch(&mut self, queue: &mut MessageQueue, call_id: u16, dest_gssi: u32, carrier_num: u16, ts: u8) {
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
        let msg = Self::build_sapmsg_stealing(sdu, self.dltime, dest_addr, carrier_num, ts, None);
        queue.push_back(msg);
    }

    /// Release a group call: send D-RELEASE, close circuits, clean up state
    pub(super) fn release_group_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        if let Some(call) = self.active_calls.get_mut(&call_id) {
            call.begin_release(disconnect_cause);
        }

        let Some(cached) = self.cached_setups.get(&call_id) else {
            tracing::error!("No cached D-SETUP for call_id={}", call_id);
            return;
        };
        let dest_addr = cached.dest_addr;

        // Send D-RELEASE to group
        let sdu = Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause);
        let prim = Self::build_sapmsg(sdu, None, self.dltime, dest_addr, None);
        queue.push_back(prim);

        // Close the circuit in CircuitMgr and notify Brew
        if let Some(call) = self.active_calls.get(&call_id) {
            let ts = call.ts;
            let dest_ssi = call.dest_gssi;
            let is_local = matches!(call.origin, CallOrigin::Local { .. });

            let carrier_num = call.carrier_num;
            if let Ok(circuit) = self.circuits.close_circuit_slot(Direction::Both, carrier_num, ts) {
                Self::signal_umac_circuit_close(queue, circuit, self.dltime);
            }

            // Ensure UMAC clears any hangtime override for this slot even if the circuit close is delayed.
            self.notify_call_ended(
                queue,
                CallTimeslot { call_id, carrier_num, ts },
                true,
                if is_local {
                    BrewNotification::IfGroupRoutable(dest_ssi)
                } else {
                    BrewNotification::Never
                },
            );

            self.release_timeslot_slot(CarrierSlot { carrier_num, ts });
        }

        // Clean up
        self.cached_setups.remove(&call_id);
        let was_active = self.active_calls.remove(&call_id).is_some();

        // Dashboard telemetry: group call released (normal disconnect, timeout, hangtime or
        // pre-emption — all of which funnel through here). Only emit if a call was actually
        // removed, so a double-release can't produce a phantom Ended.
        if was_active {
            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallEnded { call_id, gssi: 0 });
        }
    }

    /// Release an individual call: send D-RELEASE to both parties, close circuits, clean up state
    pub(super) fn release_individual_call(&mut self, queue: &mut MessageQueue, call_id: u16, disconnect_cause: DisconnectCause) {
        self.release_individual_call_inner(queue, call_id, disconnect_cause, None);
    }

    pub(super) fn release_individual_call_from_u_disconnect(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        disconnecting_issi: u32,
    ) {
        self.release_individual_call_inner(queue, call_id, disconnect_cause, Some(disconnecting_issi));
    }

    fn release_individual_call_inner(
        &mut self,
        queue: &mut MessageQueue,
        call_id: u16,
        disconnect_cause: DisconnectCause,
        _disconnecting_issi: Option<u32>,
    ) {
        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            call.begin_release(disconnect_cause);
        }

        let Some(call) = self.individual_calls.remove(&call_id) else {
            tracing::warn!("No individual call for call_id={}", call_id);
            return;
        };

        let send_calling_leg = !call.calling_over_brew;
        let send_called_leg = !call.called_over_brew;

        const SETUP_RELEASE_REPEATS: usize = 3;

        if call.is_active() {
            // Deliver on traffic channel via FACCH stealing so the MS is still listening.
            // EN 300 392-2 14.5.1.3.1 allows the SwMI to inform the other MS
            // with either D-DISCONNECT or D-RELEASE. Use D-RELEASE for both legs
            // here so neither MS has to complete a U-RELEASE exchange while the
            // traffic circuits are being torn down.
            // Send twice to reduce "no response" due to occasional STCH loss.
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
                        self.dltime,
                        call.calling_addr,
                        call.calling_carrier_num,
                        call.calling_ts,
                        Some(call.calling_usage),
                    );
                    queue.push_back(prim_calling);
                }
                if send_called_leg {
                    let prim_called = Self::build_sapmsg_stealing(
                        sdu_called,
                        self.dltime,
                        call.called_addr,
                        call.called_carrier_num,
                        call.called_ts,
                        Some(call.called_usage),
                    );
                    queue.push_back(prim_called);
                }
            }
        } else {
            // Send D-RELEASE to calling and called MS via MCCH (no LLC link context).
            // During setup, both parties are monitoring MCCH, so force link_id=0.
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
                    let prim_calling = Self::build_sapmsg(sdu_calling, None, self.dltime, call.calling_addr, None);
                    queue.push_back(prim_calling);
                }

                if send_called_leg {
                    let prim_called = Self::build_sapmsg(sdu_called, None, self.dltime, call.called_addr, None);
                    queue.push_back(prim_called);
                }
            }
        }

        // Close the circuit(s)
        let mut slot_list = vec![CarrierSlot {
            carrier_num: call.calling_carrier_num,
            ts: call.calling_ts,
        }];
        let called_slot = CarrierSlot {
            carrier_num: call.called_carrier_num,
            ts: call.called_ts,
        };
        if !slot_list.contains(&called_slot) {
            slot_list.push(called_slot);
        }
        for slot in slot_list {
            if let Ok(circuit) = self.circuits.close_circuit_slot(Direction::Both, slot.carrier_num, slot.ts) {
                Self::signal_umac_circuit_close(queue, circuit, self.dltime);
            }

            self.notify_call_ended(
                queue,
                CallTimeslot {
                    call_id,
                    carrier_num: slot.carrier_num,
                    ts: slot.ts,
                },
                true,
                BrewNotification::Never,
            );

            self.release_timeslot_slot(slot);
        }
        self.cached_setups.remove(&call_id);

        if (call.called_over_brew || call.calling_over_brew) && disconnect_cause != DisconnectCause::SwmiRequestedDisconnection {
            if let Some(brew_uuid) = call.brew_uuid {
                self.notify_network_circuit_release(queue, call.network_entity, brew_uuid, disconnect_cause);
            }
        }

        // Dashboard telemetry: individual call released. Reaching here means the call was present
        // and removed at the top of this function (early-return otherwise), so this fires exactly
        // once per released individual call across every teardown path that funnels through here
        // (normal disconnect, setup/active timeout, pre-emption).
        self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallEnded { call_id });
    }

    pub(super) fn release_timeslot_slot(&mut self, slot: CarrierSlot) {
        let mut state = self.config.state_write();
        if let Err(err) = state.timeslot_alloc.release_slot(TimeslotOwner::Cmce, slot) {
            tracing::warn!(
                "CcBsSubentity: failed to release timeslot carrier={} ts={} err={:?}",
                slot.carrier_num,
                slot.ts,
                err
            );
        }
    }

    pub(super) fn release_timeslot(&mut self, ts: u8) {
        self.release_timeslot_slot(CarrierSlot {
            carrier_num: self.config.config().cell.main_carrier,
            ts,
        });
    }

    /// Map `cell.call_timeout_secs` from config to the nearest ETSI `CallTimeout` enum value.
    /// ETSI EN 300 392-2 Table 14.50: the BS sets D-SETUP/D-CONNECT call_time_out to indicate the
    /// maximum call duration. 0 means "no limit" (Infinite). Default config value is 120s (→ T2m).
    pub(super) fn config_call_timeout(&self) -> CallTimeout {
        let secs = self.config.config().cell.call_timeout_secs;
        match secs {
            0 => CallTimeout::Infinite, // 0 = no limit
            1..=37 => CallTimeout::T30s,
            38..=52 => CallTimeout::T45s,
            53..=90 => CallTimeout::T60s,
            91..=150 => CallTimeout::T2m,
            151..=210 => CallTimeout::T3m,
            211..=270 => CallTimeout::T4m,
            271..=390 => CallTimeout::T5m,
            391..=540 => CallTimeout::T6m,
            541..=720 => CallTimeout::T8m,
            721..=900 => CallTimeout::T10m,
            901..=1080 => CallTimeout::T12m,
            1081..=1350 => CallTimeout::T15m,
            1351..=1800 => CallTimeout::T20m,
            _ => CallTimeout::T30m,
        }
    }

    /// Number of currently free traffic timeslots (TS2..=TS4) on this cell.
    fn free_traffic_timeslots(&self) -> usize {
        self.config.state_read().timeslot_alloc.free_slot_count()
    }

    /// Pick the best active call to pre-empt for a higher-priority call, or `None` if none is
    /// eligible. Only calls of *strictly lower* priority than `incoming_priority` may be
    /// pre-empted (equal priority keeps the channel — first come, first served). Among eligible
    /// calls the victim is chosen by: lowest priority first; then a call that is not actively
    /// transmitting (a group call in hangtime / a P2P call still in set-up — least disruptive to
    /// release); then the lowest call_id, purely for deterministic behaviour. `exclude` holds
    /// call_ids already released this round so the loop always makes progress.
    fn select_preemption_victim(&self, incoming_priority: u8, exclude: &[u16]) -> Option<PreemptVictim> {
        let mut candidates: Vec<(u8, u16, PreemptVictim, usize)> = Vec::new();
        for (id, call) in self.active_calls.iter() {
            if call.priority < incoming_priority && !exclude.contains(id) {
                candidates.push((call.priority, *id, PreemptVictim::Group(*id), 1));
            }
        }
        for (id, call) in self.individual_calls.iter() {
            if call.priority < incoming_priority && !exclude.contains(id) {
                let slots = if call.calling_carrier_num == call.called_carrier_num && call.calling_ts == call.called_ts {
                    1
                } else {
                    2
                };
                candidates.push((call.priority, *id, PreemptVictim::Individual(*id), slots));
            }
        }
        candidates
            .into_iter()
            .min_by_key(|(priority, call_id, _, slots)| (*priority, usize::MAX - *slots, *call_id))
            .map(|(_, _, victim, _)| victim)
    }

    /// ETSI EN 300 392-2 clause 14.8 pre-emptive priority handling. When a call requested at a
    /// pre-emptive priority (>= 12, e.g. an emergency call) cannot be granted a traffic channel,
    /// the SwMI may release active calls of strictly lower priority to free up to `needed` slots.
    /// Each round releases the lowest-priority eligible call (see [`Self::select_preemption_victim`])
    /// with `DisconnectCause::PreEmptiveUseOfResource`. This is a no-op for non-pre-emptive
    /// priorities, and stops as soon as enough slots are free or no lower-priority call remains
    /// (in which case the caller's own allocation will fail and reject the call normally).
    pub(super) fn ensure_timeslots_for_priority(&mut self, queue: &mut MessageQueue, required_slots: usize, priority: u8) -> bool {
        if self.free_traffic_timeslots() >= required_slots {
            return true;
        }
        if !is_preemptive_priority(priority) {
            return false;
        }
        let mut attempted: Vec<u16> = Vec::new();
        while self.free_traffic_timeslots() < required_slots {
            let Some(victim) = self.select_preemption_victim(priority, &attempted) else {
                tracing::info!(
                    "CMCE: pre-emption for priority {} call cannot free enough channels ({} of {} slots free, no lower-priority call to release)",
                    priority,
                    self.free_traffic_timeslots(),
                    required_slots
                );
                return false;
            };
            attempted.push(victim.call_id());
            tracing::info!(
                "CMCE: pre-empting {:?} to free a traffic channel for an incoming priority {} call",
                victim,
                priority
            );
            match victim {
                PreemptVictim::Group(call_id) => self.release_group_call(queue, call_id, DisconnectCause::PreEmptiveUseOfResource),
                PreemptVictim::Individual(call_id) => {
                    self.release_individual_call(queue, call_id, DisconnectCause::PreEmptiveUseOfResource)
                }
            }
        }
        true
    }
}

// ── Call priority & pre-emption (ETSI EN 300 392-2 clause 14.8 "Call priority") ───────────────
//
// The call priority is a 4-bit field (0..=15) carried in U-SETUP / D-SETUP / D-CONNECT:
//   0        → priority not defined (treated as the lowest / normal priority)
//   1..=11   → ordinary priority levels (increasing)
//   12..=15  → the four *pre-emptive* priority levels
//   15       → highest priority; what a terminal's emergency button generates
//
// A call requested at a pre-emptive priority (>= 12) is entitled to pre-empt an active call of
// *strictly lower* priority when no traffic channel is free. An emergency call (priority 15) is
// the top pre-emptive level: it is surfaced distinctly on the dashboard and always granted the
// floor immediately on set-up.

/// Highest call priority (ETSI clause 14.8) — an emergency call.
pub(super) const CALL_PRIORITY_EMERGENCY: u8 = 15;
/// Lowest of the four pre-emptive priority levels (12..=15).
pub(super) const CALL_PRIORITY_PREEMPTIVE_MIN: u8 = 12;

/// True when a call at this priority may pre-empt a lower-priority call (pre-emptive priority).
#[inline]
pub(super) fn is_preemptive_priority(priority: u8) -> bool {
    priority >= CALL_PRIORITY_PREEMPTIVE_MIN
}

/// True when this priority denotes an emergency call (the highest priority level).
#[inline]
pub(super) fn is_emergency_priority(priority: u8) -> bool {
    priority >= CALL_PRIORITY_EMERGENCY
}

/// A call selected for pre-emption: either an active group call or an individual (P2P) call.
#[derive(Clone, Copy, Debug)]
enum PreemptVictim {
    Group(u16),
    Individual(u16),
}

impl PreemptVictim {
    #[inline]
    fn call_id(self) -> u16 {
        match self {
            PreemptVictim::Group(id) | PreemptVictim::Individual(id) => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CcBsSubentity;
    #[cfg(feature = "asterisk")]
    use tetra_config::bluestation::{SharedConfig, parsing};
    #[cfg(feature = "asterisk")]
    use tetra_saps::control::call_control::NetworkCircuitCall;

    #[cfg(feature = "asterisk")]
    fn asterisk_test_cc() -> CcBsSubentity {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[asterisk]
enabled = true
outbound_prefix = "91"
strip_outbound_prefix = true
codec = "PCMU"
service_numbers = ["600", "601"]
"#;
        let cfg = parsing::from_toml_str(toml).expect("asterisk test config must parse");
        CcBsSubentity::new(SharedConfig::from_parts(cfg, None))
    }

    #[cfg(feature = "asterisk")]
    fn asterisk_open_prefix_test_cc() -> CcBsSubentity {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[asterisk]
enabled = true
outbound_prefix = "91"
strip_outbound_prefix = true
codec = "PCMU"
"#;
        let cfg = parsing::from_toml_str(toml).expect("asterisk open-prefix test config must parse");
        CcBsSubentity::new(SharedConfig::from_parts(cfg, None))
    }

    #[cfg(feature = "asterisk")]
    fn network_call(destination: u32, number: &str) -> NetworkCircuitCall {
        NetworkCircuitCall {
            source_issi: 1000001,
            destination,
            number: number.to_string(),
            priority: 0,
            service: 0,
            mode: 0,
            duplex: 0,
            method: 0,
            communication: 0,
            grant: 0,
            permission: 0,
            timeout: 0,
            ownership: 0,
            queued: 0,
        }
    }

    #[cfg(feature = "asterisk")]
    #[test]
    fn asterisk_route_strips_prefix_for_configured_service_numbers() {
        let cc = asterisk_test_cc();

        assert_eq!(cc.asterisk_route_number(&network_call(91600, "")), Some("600".to_string()));
        assert_eq!(cc.asterisk_route_number(&network_call(91601, "")), Some("601".to_string()));
    }

    #[cfg(feature = "asterisk")]
    #[test]
    fn asterisk_route_uses_dialed_number_and_leaves_non_matches_for_brew() {
        let cc = asterisk_test_cc();

        assert_eq!(cc.asterisk_route_number(&network_call(0, "91600")), Some("600".to_string()));
        assert_eq!(cc.asterisk_route_number(&network_call(91602, "")), None);
        assert_eq!(cc.asterisk_route_number(&network_call(0, "91234")), None);
    }

    #[cfg(feature = "asterisk")]
    #[test]
    fn asterisk_route_accepts_prefixed_numbers_when_service_list_is_empty() {
        let cc = asterisk_open_prefix_test_cc();

        assert_eq!(cc.asterisk_route_number(&network_call(91601, "")), Some("601".to_string()));
        assert_eq!(cc.asterisk_route_number(&network_call(0, "91601")), Some("601".to_string()));
        assert_eq!(cc.asterisk_route_number(&network_call(601, "")), None);
    }

    #[test]
    fn external_subscriber_number_supports_24_digits() {
        let number = "123456789012345678901234";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 96);
        assert_ne!(field.data, 0);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), number);
    }

    #[test]
    fn external_subscriber_number_truncates_to_24_digits() {
        let number = "1234567890123456789012345";
        let field = CcBsSubentity::encode_external_subscriber_number(number).expect("field should be generated");
        assert_eq!(field.len, 96);
        assert_eq!(CcBsSubentity::decode_external_subscriber_number(&field), "123456789012345678901234");
    }
}
