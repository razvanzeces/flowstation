use super::super::dtmf::{DtmfKind, decode_dtmf, pack_type3_bits_to_bytes};
use super::*;

impl CcBsSubentity {
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
                    tracing::error!("BUG: unexpected message or state -- routing error"); return;
                };
            let reject_call_id = self.circuits.get_next_call_id();
            let sdu = Self::build_d_release(reject_call_id, DisconnectCause::IncompatibleTrafficCase);
            let msg = Self::build_sapmsg_direct(sdu, calling_party, prim.handle, prim.link_id, prim.endpoint_id);
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

        if let Some(call) = self.individual_calls.get(&call_id).cloned() {
            // For simplex PTT individual calls: MS released PTT.
            // Send D-TX-CEASED to the sender (confirms floor released),
            // then grant floor to the peer via D-TX-GRANTED so they can speak immediately.
            // Radios with GrantedToOtherUser in D-CONNECT need an explicit D-TX-GRANTED
            // to enable their PTT button — D-TX-CEASED alone is not sufficient.
            if !call.is_active() {
                tracing::debug!("U-TX CEASED for inactive individual call_id={}, ignoring", call_id);
                return;
            }
            let (sender_ts, sender_usage, peer_addr, peer_ts, peer_usage) = if sender.ssi == call.calling_addr.ssi {
                (call.calling_ts, call.calling_usage, call.called_addr, call.called_ts, call.called_usage)
            } else {
                (call.called_ts, call.called_usage, call.calling_addr, call.calling_ts, call.calling_usage)
            };
            tracing::info!("U-TX CEASED (individual) call_id={} from ISSI {} -> sending D-TX-CEASED to sender, D-TX-GRANTED to peer ISSI {}", call_id, sender.ssi, peer_addr.ssi);

            // 1) D-TX-CEASED to sender so it knows floor was released and resets PTT state.
            tracing::info!("-> D-TX CEASED (individual simplex, FACCH) call_id={} to sender ISSI {}", call_id, sender.ssi);
            let ceased_pdu = DTxCeased {
                call_identifier: call_id,
                transmission_request_permission: false,
                notification_indicator: None,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            let mut ceased_sdu = BitBuffer::new_autoexpand(30);
            ceased_pdu.to_bitbuf(&mut ceased_sdu).expect("Failed to serialize DTxCeased");
            ceased_sdu.seek(0);
            // Former speaker becomes listener: DL-only so they receive the peer's audio.
            let ceased_msg = Self::build_sapmsg_stealing_ul_dl(ceased_sdu, sender, sender_ts, Some(sender_usage), UlDlAssignment::Dl);
            queue.push_back(ceased_msg);

            // 2) D-TX-GRANTED(Granted) to peer so it immediately gets the floor and can press PTT.
            // Without this explicit grant, radios that received GrantedToOtherUser in D-CONNECT
            // will not enable PTT after a D-TX-CEASED — they require an explicit D-TX-GRANTED.
            // Use UL-only so the new speaker transmits but does not loop back its own audio.
            tracing::info!("-> D-TX GRANTED Granted (individual simplex, FACCH) call_id={} to peer ISSI {}", call_id, peer_addr.ssi);
            let granted_pdu = DTxGranted {
                call_identifier: call_id,
                transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
                transmission_request_permission: false,
                encryption_control: false,
                reserved: false,
                notification_indicator: None,
                transmitting_party_type_identifier: Some(1),
                transmitting_party_address_ssi: Some(peer_addr.ssi as u64),
                transmitting_party_extension: None,
                external_subscriber_number: None,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            let mut granted_sdu = BitBuffer::new_autoexpand(50);
            granted_pdu.to_bitbuf(&mut granted_sdu).expect("Failed to serialize DTxGranted");
            granted_sdu.seek(0);
            // New speaker gets UL-only assignment so they transmit.
            let granted_msg = Self::build_sapmsg_stealing_ul_dl(granted_sdu, peer_addr, peer_ts, Some(peer_usage), UlDlAssignment::Ul);
            queue.push_back(granted_msg);

            // 3) D-TX-GRANTED(GrantedToOtherUser) back to former sender so it knows the peer
            // now holds the floor and it is the listener. This mirrors what U-TX-DEMAND sends
            // and keeps both radios in sync on who has UL vs DL for the remainder of the call.
            tracing::info!("-> D-TX GRANTED GrantedToOtherUser (individual simplex, FACCH) call_id={} to former sender ISSI {} (now listener)", call_id, sender.ssi);
            let gtou_pdu = DTxGranted {
                call_identifier: call_id,
                transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
                transmission_request_permission: false,
                encryption_control: false,
                reserved: false,
                notification_indicator: None,
                transmitting_party_type_identifier: Some(1),
                transmitting_party_address_ssi: Some(peer_addr.ssi as u64),
                transmitting_party_extension: None,
                external_subscriber_number: None,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            let mut gtou_sdu = BitBuffer::new_autoexpand(50);
            gtou_pdu.to_bitbuf(&mut gtou_sdu).expect("Failed to serialize DTxGranted GrantedToOtherUser");
            gtou_sdu.seek(0);
            // Former sender is now listener: DL-only assignment (already set by ceased_msg above,
            // but the explicit D-TX-GRANTED ensures the radio re-enables its PTT request button).
            let gtou_msg = Self::build_sapmsg_stealing_ul_dl(gtou_sdu, sender, sender_ts, Some(sender_usage), UlDlAssignment::Dl);
            queue.push_back(gtou_msg);

            // 4) Notify UMAC that the floor has been granted to the peer.
            // This resets the UL inactivity timer on the timeslot so UMAC doesn't
            // prematurely detect inactivity and close the circuit before the new
            // speaker begins transmitting.
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                    call_id,
                    source_issi: peer_addr.ssi,
                    dest_gssi: sender.ssi,
                    ts: peer_ts,
                }),
            });

            // 5) Notify Brew that the floor has been granted to the peer.
            // Without this, Brew detects audio inactivity on the timeslot and sends
            // a CALL_RELEASE, tearing down the circuit before the handoff completes.
            if (call.called_over_brew || call.calling_over_brew)
                && let Some(brew_uuid) = call.brew_uuid
            {
                queue.push_back(SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Cmce,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
                        call_id,
                        source_issi: peer_addr.ssi,
                        dest_gssi: sender.ssi,
                        ts: peer_ts,
                    }),
                });
                let _ = brew_uuid; // suppress unused warning
            }

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

        if let Some(call) = self.individual_calls.get(&call_id).cloned() {
            // For simplex PTT individual calls: MS requests PTT floor.
            if !call.is_active() {
                tracing::debug!("U-TX DEMAND for inactive individual call_id={}, ignoring", call_id);
                return;
            }
            let (peer_addr, peer_ts, peer_usage) = if requesting_party.ssi == call.calling_addr.ssi {
                (call.called_addr, call.called_ts, call.called_usage)
            } else {
                (call.calling_addr, call.calling_ts, call.calling_usage)
            };
            tracing::info!("U-TX DEMAND (individual) call_id={} from ISSI {} -> granting floor, notifying peer ISSI {}", call_id, requesting_party.ssi, peer_addr.ssi);

            // D-TX-GRANTED to requester (Granted) — they may now transmit.
            // For simplex: give them UL-only so they transmit but don't receive their own TX.
            let dtg_req = DTxGranted {
                call_identifier: call_id,
                transmission_grant: TransmissionGrant::Granted.into_raw() as u8,
                transmission_request_permission: false,
                encryption_control: false,
                reserved: false,
                notification_indicator: None,
                transmitting_party_type_identifier: Some(1),
                transmitting_party_address_ssi: Some(requesting_party.ssi as u64),
                transmitting_party_extension: None,
                external_subscriber_number: None,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            tracing::info!("-> D-TX GRANTED Granted (individual simplex) call_id={} to ISSI {}", call_id, requesting_party.ssi);
            let mut dtg_req_sdu = BitBuffer::new_autoexpand(50);
            dtg_req.to_bitbuf(&mut dtg_req_sdu).expect("Failed to serialize DTxGranted");
            dtg_req_sdu.seek(0);
            let req_ts = if requesting_party.ssi == call.calling_addr.ssi { call.calling_ts } else { call.called_ts };
            let req_usage = if requesting_party.ssi == call.calling_addr.ssi { call.calling_usage } else { call.called_usage };
            // Requester now owns the floor: give UL-only assignment so they transmit.
            let dtg_req_msg = Self::build_sapmsg_stealing_ul_dl(dtg_req_sdu, requesting_party, req_ts, Some(req_usage), UlDlAssignment::Ul);
            queue.push_back(dtg_req_msg);

            // D-TX-GRANTED to peer (GrantedToOtherUser) — they must listen.
            // ETSI 14.8.43: permission=false means "allowed to request transmission".
            // Peer should still be allowed to U-TX-DEMAND once the current speaker releases
            // the floor; sending true (= not allowed) would lock peer out of PTT permanently.
            let dtg_peer = DTxGranted {
                call_identifier: call_id,
                transmission_grant: TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
                transmission_request_permission: false,
                encryption_control: false,
                reserved: false,
                notification_indicator: None,
                transmitting_party_type_identifier: Some(1),
                transmitting_party_address_ssi: Some(requesting_party.ssi as u64),
                transmitting_party_extension: None,
                external_subscriber_number: None,
                facility: None,
                dm_ms_address: None,
                proprietary: None,
            };
            tracing::info!("-> D-TX GRANTED GrantedToOtherUser (individual simplex) call_id={} to ISSI {}", call_id, peer_addr.ssi);
            let mut dtg_peer_sdu = BitBuffer::new_autoexpand(50);
            dtg_peer.to_bitbuf(&mut dtg_peer_sdu).expect("Failed to serialize DTxGranted peer");
            dtg_peer_sdu.seek(0);
            // Peer is now the listener: give DL-only assignment so they receive only.
            let dtg_peer_msg = Self::build_sapmsg_stealing_ul_dl(dtg_peer_sdu, peer_addr, peer_ts, Some(peer_usage), UlDlAssignment::Dl);
            queue.push_back(dtg_peer_msg);
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
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_info(&mut self, queue: &mut MessageQueue, pdu: UInfo) {
        let call_id = pdu.call_identifier;
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            tracing::trace!("U-INFO for unknown/non-individual call_id={}, ignoring", call_id);
            return;
        };

        if !call.called_over_brew && !call.calling_over_brew {
            tracing::trace!("U-INFO call_id={} is local individual call, no Brew forwarding", call_id);
            return;
        }

        let Some(brew_uuid) = call.brew_uuid else {
            tracing::warn!("U-INFO call_id={} marked Brew-routed but missing brew_uuid", call_id);
            return;
        };

        let Some(dtmf) = pdu.dtmf.as_ref() else {
            tracing::trace!(
                "U-INFO call_id={} has no DTMF element (modify={:?} facility={} proprietary={}), ignoring",
                call_id,
                pdu.modify,
                pdu.facility.is_some(),
                pdu.proprietary.is_some()
            );
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
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf {
                    brew_uuid,
                    length_bits: 8,
                    data: vec![digit],
                }),
            });
        }
    }

    /// Handle parsed U-RELEASE.
    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_release(&mut self, queue: &mut MessageQueue, sender: TetraAddress, pdu: URelease) {
        let call_id = pdu.call_identifier;
        let disconnect_cause = pdu.disconnect_cause;

        tracing::info!("U-RELEASE: call_id={} cause={}", call_id, disconnect_cause);
        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            tracing::info!("U-RELEASE (individual) call_id={} cause={}", call_id, disconnect_cause);
            let sender_is_called = sender.ssi == call_snapshot.called_addr.ssi;
            if !call_snapshot.called_over_brew && !call_snapshot.calling_over_brew && (call_snapshot.is_active() || sender_is_called) {
                self.send_d_disconnect_individual(queue, call_id, &call_snapshot, sender, disconnect_cause);
            }
            self.release_individual_call(queue, call_id, disconnect_cause);
        } else {
            self.release_group_call(queue, call_id, disconnect_cause);
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

        if let Some(call_snapshot) = self.individual_calls.get(&call_id).cloned() {
            tracing::info!("U-DISCONNECT (individual) call_id={} cause={}", call_id, disconnect_cause);
            let sender_is_called = sender.ssi == call_snapshot.called_addr.ssi;
            if !call_snapshot.called_over_brew && !call_snapshot.calling_over_brew && (call_snapshot.is_active() || sender_is_called) {
                self.send_d_disconnect_individual(queue, call_id, &call_snapshot, sender, disconnect_cause);
            }
            self.release_individual_call(queue, call_id, disconnect_cause);
            return;
        }

        let Some(call) = self.active_calls.get(&call_id) else {
            tracing::debug!("U-DISCONNECT for unknown call_id={} (likely duplicate)", call_id);
            return;
        };

        let is_call_owner = matches!(&call.origin, CallOrigin::Local { caller_addr } if caller_addr.ssi == sender.ssi);

        if is_call_owner {
            tracing::info!("U-DISCONNECT: call owner ISSI {} disconnecting call_id={}", sender.ssi, call_id);
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
