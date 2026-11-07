use super::*;

use tetra_saps::control::call_control::HandedOffCall;

use super::super::state::HandoffHold;

/// How long a duplex call between two radios of this station waits for the network to take it
/// over after one leg lost its uplink (extension `call-handoff`). Project value, not an ETSI
/// timer: the radio registers on the other station and then restores the call within T306 (4 s
/// to 6 s, TS 100 392-2 clause 14.6), and brew-server gives a moved call 8 s
/// (`call_restore::RESTORE_WINDOW`). (8 s × 72 timeslots/s.)
pub(in crate::cmce::subentities::cc_bs) const HANDOFF_HOLD_TS: i32 = 8 * 18 * 4;

/// Which side of an individual call a party is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Calling,
    Called,
}

impl CcBsSubentity {
    /// This cell hands local individual calls to the network and joins network calls of two of
    /// its radios: with the extension `call-handoff` between separate stations, and always
    /// between the cells of a multi-cell station, whose site switch routes these requests itself.
    pub(in crate::cmce::subentities::cc_bs) fn hands_off_calls(&self) -> bool {
        if brew::is_site_linked(&self.config) {
            return true;
        }
        if !brew::is_active(&self.config) {
            return false;
        }
        let offered = self.config.state_read().brew_server_extensions.clone();
        crate::net_brew::worker::hands_off_calls(&self.config.config(), &offered)
    }

    fn side_of(call: &IndividualCall, issi: u32) -> Option<Side> {
        if call.calling_addr.ssi == issi {
            Some(Side::Calling)
        } else if call.called_addr.ssi == issi {
            Some(Side::Called)
        } else {
            None
        }
    }

    fn slot_of(call: &IndividualCall, side: Side) -> (u16, u8, u8) {
        match side {
            Side::Calling => (call.calling_carrier_num, call.calling_ts, call.calling_usage),
            Side::Called => (call.called_carrier_num, call.called_ts, call.called_usage),
        }
    }

    /// A duplex call between two radios of this station lost `issi`'s uplink. With `call-handoff`
    /// the radio may restore it on another station: hold the call instead of releasing it.
    /// Returns whether the call is held.
    pub(in crate::cmce::subentities::cc_bs) fn hold_for_handoff(&mut self, call_id: u16, issi: u32) -> bool {
        if !self.hands_off_calls() {
            return false;
        }
        let now = self.dltime;
        let Some(call) = self.individual_calls.get_mut(&call_id) else {
            return false;
        };
        if call.calling_over_brew || call.called_over_brew || call.ambience || !call.is_active() {
            return false;
        }
        if call.handoff_hold.is_none() {
            tracing::info!(
                "CMCE: ISSI {} lost its uplink in local call_id={}; holding the call for a handoff to another station",
                issi,
                call_id
            );
            call.handoff_hold = Some(HandoffHold { issi, since: now });
        }
        true
    }

    /// The held party is heard again on its own leg here: the hold ends, the call goes on.
    pub(in crate::cmce::subentities::cc_bs) fn end_handoff_hold_on_activity(&mut self, carrier_num: u16, ts: u8) {
        for (call_id, call) in self.individual_calls.iter_mut() {
            let Some(hold) = call.handoff_hold else { continue };
            let Some(side) = Self::side_of(call, hold.issi) else { continue };
            let (c, t, _) = Self::slot_of(call, side);
            if (c, t) == (carrier_num, ts) {
                tracing::info!("CMCE: ISSI {} is back on call_id={}, hold ended", hold.issi, call_id);
                call.handoff_hold = None;
            }
        }
    }

    /// Per tick: release held calls the network did not take over in time.
    pub(in crate::cmce::subentities::cc_bs) fn expire_handoff_holds(&mut self, queue: &mut MessageQueue) {
        let now = self.dltime;
        let expired: Vec<u16> = self
            .individual_calls
            .iter()
            .filter(|(_, c)| c.handoff_hold.is_some_and(|h| h.since.age(now) >= HANDOFF_HOLD_TS))
            .map(|(id, _)| *id)
            .collect();
        for call_id in expired {
            tracing::info!("CMCE: no handoff for held call_id={}, releasing it", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }
    }

    /// The network asks for the local call between `issi`, which restores it on another station,
    /// and `peer`, which is still here (`CallControl::IndividualCallHandoffRequest`). `peer`'s leg
    /// stays on its traffic channel and is bridged to the network under a new session; `issi`'s
    /// leg ends here without signalling (the radio is on the other station, which answers its
    /// U-CALL RESTORE, TS 100 392-2 clause 14.5.1.2.4). Answered with the call or `None`.
    pub(in crate::cmce::subentities::cc_bs) fn on_individual_call_handoff_request(&mut self, queue: &mut MessageQueue, issi: u32, peer: u32) {
        let answer = |queue: &mut MessageQueue, call: Option<HandedOffCall>| {
            Self::push_control(queue, TetraEntity::Brew, CallControl::IndividualCallHandoffAnswer { issi, peer, call });
        };
        let found = self.individual_calls.iter().find_map(|(&id, c)| {
            let local = !c.calling_over_brew && !c.called_over_brew && !c.ambience && c.is_active();
            let parties = (c.calling_addr.ssi == issi && c.called_addr.ssi == peer) || (c.calling_addr.ssi == peer && c.called_addr.ssi == issi);
            (local && parties).then_some(id)
        });
        let Some(call_id) = found.filter(|_| self.hands_off_calls() && issi != peer) else {
            tracing::info!("CMCE: no local call of ISSI {} with peer {} to hand to the network", issi, peer);
            answer(queue, None);
            return;
        };

        // The radio that left had the floor: its permission ended at the break (clause
        // 14.5.1.2.4), as if it had sent U-TX CEASED. A queued request of the other party is
        // granted by that.
        if self.individual_calls.get(&call_id).is_some_and(|c| c.is_simplex() && c.is_floor_held_by(issi)) {
            self.fsm_on_u_tx_ceased(
                queue,
                TetraAddress::new(issi, SsiType::Issi),
                UTxCeased {
                    call_identifier: call_id,
                    facility: None,
                    dm_ms_address: None,
                    proprietary: None,
                },
            );
        }

        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            answer(queue, None);
            return;
        };
        let (Some(gone), Some(stays)) = (Self::side_of(&call, issi), Self::side_of(&call, peer)) else {
            answer(queue, None);
            return;
        };
        let (gone_carrier, gone_ts, _) = Self::slot_of(&call, gone);
        let (carrier_num, ts, usage) = Self::slot_of(&call, stays);

        // The leg that left: its own traffic channel (duplex) ends here.
        if (gone_carrier, gone_ts) != (carrier_num, ts) {
            if let Ok(circuit) = self.circuits.close_circuit_slot(Direction::Both, gone_carrier, gone_ts) {
                Self::signal_umac_circuit_close(queue, circuit, self.dltime);
            }
            self.notify_call_ended(
                queue,
                CallTimeslot {
                    call_id,
                    carrier_num: gone_carrier,
                    ts: gone_ts,
                },
                true,
                BrewNotification::Never,
            );
            self.release_timeslot_slot(CarrierSlot {
                carrier_num: gone_carrier,
                ts: gone_ts,
            });
        }

        // The leg that stays: the same channel, its downlink now from the network (UMAC replaces
        // an open circuit on the same slot).
        if let Some(circuit) = self.circuits.circuit_slot(carrier_num, ts).cloned() {
            Self::signal_umac_circuit_open(queue, &circuit, self.dltime, None, None, CircuitDlMediaSource::SwMI);
        }

        let brew_uuid = uuid::Uuid::new_v4();
        let Some(c) = self.individual_calls.get_mut(&call_id) else {
            answer(queue, None);
            return;
        };
        match gone {
            Side::Calling => {
                c.calling_over_brew = true;
                c.calling_carrier_num = carrier_num;
                c.calling_ts = ts;
                c.calling_usage = usage;
            }
            Side::Called => {
                c.called_over_brew = true;
                c.called_carrier_num = carrier_num;
                c.called_ts = ts;
                c.called_usage = usage;
            }
        }
        c.network_entity = TetraEntity::Brew;
        c.brew_uuid = Some(brew_uuid);
        c.handoff_hold = None;
        c.queued_tx_demand = None;
        let handed = HandedOffCall {
            brew_uuid,
            calling_issi: c.calling_addr.ssi,
            simplex_duplex: c.simplex_duplex,
            priority: c.priority,
            floor_issi: c.floor_holder,
        };
        tracing::info!(
            "CMCE: handing local call_id={} to the network: ISSI {} restores it on another station, ISSI {} stays on ts={} (uuid={}, duplex={})",
            call_id,
            issi,
            peer,
            ts,
            brew_uuid,
            handed.simplex_duplex
        );
        // The answer goes first, so a site switch knows the call before its media is ready. Voice
        // of the leg that stays then runs between its channel and the network.
        answer(queue, Some(handed));
        Self::push_control(
            queue,
            TetraEntity::Brew,
            CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
            },
        );
    }

    /// `sender` restores, here, the network call it had with `peer` on another station, and
    /// `peer` is a radio of this station in that call (extension `call-handoff`). Join the two
    /// legs locally: the arriving radio gets a channel of this call (D-CALL RESTORE, clause
    /// 14.5.1.2.4) and the network call ends ("SwMI requested disconnection"; the server let the
    /// other station's leg go already). Returns whether the restore was handled.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cmce::subentities::cc_bs) fn join_network_call_locally(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        restored_call_id: u16,
        peer: u32,
        request_to_transmit: bool,
    ) -> bool {
        if peer == 0 || peer == sender.ssi || !self.hands_off_calls() {
            return false;
        }
        let found = self.individual_calls.iter().find_map(|(&id, c)| {
            if !c.is_active() || c.ambience || c.network_entity != TetraEntity::Brew {
                return None;
            }
            let joins = (c.calling_over_brew && !c.called_over_brew && c.calling_addr.ssi == sender.ssi && c.called_addr.ssi == peer)
                || (c.called_over_brew && !c.calling_over_brew && c.called_addr.ssi == sender.ssi && c.calling_addr.ssi == peer);
            joins.then_some(id)
        });
        let Some(call_id) = found else {
            return false;
        };
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            return false;
        };
        let (Some(arrives), Some(stays)) = (Self::side_of(&call, sender.ssi), Self::side_of(&call, peer)) else {
            return false;
        };
        let (carrier_num, ts, usage) = Self::slot_of(&call, stays);
        let Some(stay_circuit) = self.circuits.circuit_slot(carrier_num, ts).cloned() else {
            return false;
        };

        // Duplex: one traffic channel per radio, cross-routed. Simplex: both on the one channel.
        let arrive_slot = if call.simplex_duplex {
            let allocated = {
                let mut state = self.config.state_write();
                self.circuits
                    .allocate_circuit_for_call_with_allocator(
                        call_id,
                        Direction::Both,
                        CommunicationType::P2p,
                        true,
                        &mut state.timeslot_alloc,
                        TimeslotOwner::Cmce,
                    )
                    .cloned()
            };
            match allocated {
                Ok(c) => {
                    Self::signal_umac_circuit_open(queue, &c, self.dltime, Some(carrier_num), Some(ts), CircuitDlMediaSource::LocalLoopback);
                    Self::signal_umac_circuit_open(
                        queue,
                        &stay_circuit,
                        self.dltime,
                        Some(c.carrier_num),
                        Some(c.ts),
                        CircuitDlMediaSource::LocalLoopback,
                    );
                    (c.carrier_num, c.ts, c.usage)
                }
                Err(e) => {
                    // No channel for the arriving radio: the network path still works.
                    tracing::info!(
                        "CMCE: no channel to join ISSI {} locally into call_id={} ({:?}); asking the network instead",
                        sender.ssi,
                        call_id,
                        e
                    );
                    return false;
                }
            }
        } else {
            Self::signal_umac_circuit_open(queue, &stay_circuit, self.dltime, None, None, CircuitDlMediaSource::LocalLoopback);
            (carrier_num, ts, usage)
        };

        let brew_uuid = call.brew_uuid;
        if let Some(c) = self.individual_calls.get_mut(&call_id) {
            match arrives {
                Side::Calling => {
                    c.calling_over_brew = false;
                    (c.calling_carrier_num, c.calling_ts, c.calling_usage) = arrive_slot;
                    c.calling_handle = handle;
                    c.calling_link_id = link_id;
                    c.calling_endpoint_id = endpoint_id;
                }
                Side::Called => {
                    c.called_over_brew = false;
                    (c.called_carrier_num, c.called_ts, c.called_usage) = arrive_slot;
                    c.called_handle = Some(handle);
                    c.called_link_id = Some(link_id);
                    c.called_endpoint_id = Some(endpoint_id);
                }
            }
            c.brew_uuid = None;
            c.network_call = None;
            c.connect_request_sent = false;
            // The arriving radio is not on this channel yet: D-CALL RESTORE takes it there.
            c.restoring_cross_cell = true;
        }
        if let Some(uuid) = brew_uuid {
            tracing::info!(
                "CMCE: ISSI {} joins its call with ISSI {} here: network call uuid={} becomes local call_id={} (arriving ts={})",
                sender.ssi,
                peer,
                uuid,
                call_id,
                arrive_slot.1
            );
            self.notify_network_circuit_release(queue, TetraEntity::Brew, uuid, DisconnectCause::SwmiRequestedDisconnection);
        }

        let floor_was = call.floor_holder;
        let Some(grant) = self.restore_individual_call(
            queue,
            sender,
            handle,
            link_id,
            endpoint_id,
            restored_call_id,
            call_id,
            request_to_transmit,
        ) else {
            return true;
        };
        let Some(call) = self.individual_calls.get(&call_id).cloned() else {
            return true;
        };
        if call.is_simplex() {
            match grant {
                // The arriving radio talks on this station's channel now.
                TransmissionGrant::Granted => self.notify_floor_granted(
                    queue,
                    GroupFloorGrant {
                        call_id,
                        source_issi: sender.ssi,
                        dest_gssi: peer,
                        carrier_num: arrive_slot.0,
                        ts: arrive_slot.1,
                        is_group: false,
                    },
                    true,
                    BrewNotification::Never,
                ),
                // It held the floor through the network and gave it up: the other party hears it
                // ended.
                TransmissionGrant::NotGranted if floor_was == Some(sender.ssi) => {
                    let party = if stays == Side::Calling { call.calling_addr } else { call.called_addr };
                    self.send_individual_d_tx_ceased_to(queue, &call, call_id, party);
                }
                _ => {}
            }
        }
        true
    }
}
