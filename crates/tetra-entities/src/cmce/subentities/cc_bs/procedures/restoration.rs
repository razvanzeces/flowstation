use super::*;

/// How long a U-CALL RESTORE may wait for a Brew group call this cell only holds as pending
/// inbound (or has not been handed yet) to come up locally. Project value, not an ETSI timer:
/// the MS waits for D-CALL RESTORE with T307, min 2 s and max 8 s, the value chosen by the MS
/// (TS 100 392-2 clause 14.6, group calls); 5 s lies within that range. It stays below MLE's
/// `RESTORE_ANSWER_TIMEOUT` (10 s) so CMCE always answers first. (5 s × 72 timeslots/s.)
pub(in crate::cmce::subentities::cc_bs) const DEFERRED_RESTORE_TIMEOUT_TS: i32 = 5 * 18 * 4;

/// A U-CALL RESTORE for a group call that runs on the Brew network but not (yet) on this cell.
#[derive(Debug, Clone)]
pub(in crate::cmce::subentities::cc_bs) struct DeferredRestore {
    pub sender: TetraAddress,
    pub handle: u32,
    pub link_id: u32,
    pub endpoint_id: u32,
    /// The call identifier the MS restores with (the old cell's)
    pub call_id: u16,
    pub gssi: u32,
    pub request_to_transmit: bool,
    pub since: TdmaTime,
}

/// How long a U-CALL RESTORE of an individual call this station does not hold waits for the
/// network's answer (separate station on a Brew network). Project value, not an ETSI timer: the
/// MS waits for D-CALL RESTORE with T306, min 4 s and max 6 s, the value chosen by the MS
/// (TS 100 392-2 clause 14.6); 2 s leaves room for the answer to reach it. (2 s × 72 timeslots/s.)
pub(in crate::cmce::subentities::cc_bs) const NETWORK_RESTORE_TIMEOUT_TS: i32 = 2 * 18 * 4;

/// A U-CALL RESTORE of an individual call that ran on the station the MS left, waiting for the
/// network to move the call here (`CallControl::IndividualCallRestoreRequest`).
#[derive(Debug, Clone)]
pub(in crate::cmce::subentities::cc_bs) struct DeferredIndividualRestore {
    pub sender: TetraAddress,
    pub handle: u32,
    pub link_id: u32,
    pub endpoint_id: u32,
    /// The call identifier the MS restores with (the old station's)
    pub call_id: u16,
    /// The other party's SSI from U-CALL RESTORE (0: dummy address)
    pub peer: u32,
    pub request_to_transmit: bool,
    pub since: TdmaTime,
}

/// How long the target station waits for a Brew group call to come up for an MS arriving by
/// announced type 1 cell reselection. Project value: brew-server gives the target 2 s for its
/// whole answer (`[handover] timeout_ms`), the forward registration included. (1 s × 72 timeslots/s.)
pub(in crate::cmce::subentities::cc_bs) const HANDOVER_CALL_TIMEOUT_TS: i32 = 18 * 4;

/// An MS of a group call that runs on the Brew network but not yet on this cell, announced by
/// the serving station for type 1 cell reselection.
#[derive(Debug, Clone)]
pub(in crate::cmce::subentities::cc_bs) struct HandoverReservation {
    pub issi: u32,
    pub gssi: u32,
    pub since: TdmaTime,
}

impl CcBsSubentity {
    /// A cell linked to other cells (site switch, or any Brew network) restores a group call by
    /// its GSSI when the MS brings another cell's call identifier: TS 100 392-2 clause 14.7.1.3
    /// lets D-CALL RESTORE give the call a new identifier. Only for groups the network shares
    /// with this cell; a local-only group call with the same GSSI is a different call.
    fn restores_group_by_gssi(&self, gssi: u32) -> bool {
        brew::is_site_linked(&self.config) || (brew::is_active(&self.config) && brew::is_network_group_inbound_allowed(&self.config, gssi))
    }

    pub(in crate::cmce::subentities::cc_bs) fn fsm_on_u_call_restore(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        pdu: UCallRestore,
    ) {
        let restored_call_id = pdu.call_identifier;
        let mut call_id = restored_call_id;
        // "Request to transmit/send data": 0 asks to transmit, 1 asks that another MS may
        // transmit (TS 100 392-2 clause 14.8.36, table 14.74).
        let request_to_transmit = !pdu.request_to_transmit_send_data;
        // The other party of a group call is its GSSI. The basic service information (note 3 to
        // clause 14.7.2.2) tells an individual call apart; without it, assume a group call as the
        // multi-cell lookup always did.
        let group_restore = pdu
            .basic_service_information
            .as_ref()
            .is_none_or(|b| b.communication_type != CommunicationType::P2p);
        let gssi = pdu.other_party_ssi.filter(|_| group_restore).map(|ssi| ssi as u32);
        self.request_registration_if_unknown(queue, sender.ssi);

        // A group call is the MS's only if it is to the group the MS names: call identifiers are
        // per cell, so another cell's identifier can equal a local call to a different group.
        let is_own_group_call = |call: &ActiveCall| gssi.is_none_or(|g| call.dest_gssi == g);
        let own_group_call = self.active_calls.get(&call_id).is_some_and(is_own_group_call);
        let own_individual_call = self
            .individual_calls
            .get(&call_id)
            .is_some_and(|c| sender.ssi == c.calling_addr.ssi || sender.ssi == c.called_addr.ssi);

        // The MS reselected from another cell and restores with that cell's call identifier.
        // Find the same group call here by its GSSI; D-CALL RESTORE gives our id as the new one.
        if !own_individual_call
            && !own_group_call
            && let Some(gssi) = gssi
            && self.restores_group_by_gssi(gssi)
        {
            if let Some((&id, _)) = self.active_calls.iter().find(|(_, c)| c.dest_gssi == gssi) {
                tracing::info!(
                    "CMCE: U-CALL RESTORE from ISSI {} for call_id={} (other cell) → local call_id={} gssi={}",
                    sender.ssi,
                    call_id,
                    id,
                    gssi
                );
                call_id = id;
            } else if !brew::is_site_linked(&self.config) {
                // Separate station on a Brew network: the call runs elsewhere and is at most
                // held here as pending inbound (no local listener yet). Wait for it.
                self.defer_call_restore(
                    queue,
                    DeferredRestore {
                        sender,
                        handle,
                        link_id,
                        endpoint_id,
                        call_id,
                        gssi,
                        request_to_transmit,
                        since: self.dltime,
                    },
                );
                return;
            }
        }

        // Multi-cell individual (private/duplex) call: an MS that reselected mid-call restores with
        // the OLD cell's call identifier, which does not exist here. Match the same private call by
        // the (this MS, peer) ISSI pair; the leg was pre-provisioned on this cell by the site
        // switch's individual-call handover (see net_site prepare_handover). D-CALL RESTORE then
        // carries our local id as the new call identifier (clause 14.5.1.2.4).
        if !self.individual_calls.contains_key(&call_id)
            && !self.active_calls.contains_key(&call_id)
            && brew::is_site_linked(&self.config)
            && let Some(peer) = pdu.other_party_ssi
            && let Some((&id, _)) = self.individual_calls.iter().find(|(_, c)| {
                c.is_active()
                    && ((c.calling_addr.ssi == sender.ssi && c.called_addr.ssi as u64 == peer)
                        || (c.called_addr.ssi == sender.ssi && c.calling_addr.ssi as u64 == peer))
            })
        {
            tracing::info!(
                "CMCE: U-CALL RESTORE from ISSI {} for call_id={} (sibling cell, individual) → local call_id={} peer={}",
                sender.ssi,
                call_id,
                id,
                peer
            );
            call_id = id;
        }

        // An individual call (basic service information point-to-point, mandatory in U-CALL
        // RESTORE, table 14.22 note 3) the MS had on the cell it left is not here. Ask the network
        // to move it here: the site switch when it runs on a sibling cell of this station, else
        // the Brew network (`call-restore`, separate stations); the answer completes or rejects
        // the restore. Call identifiers are per cell, so the other cell's identifier may be in use
        // here by a call the MS is not a party to. A leg pre-provisioned by the site switch's
        // announced handover (above) is the MS's own call already.
        // A party whose side of that call runs through the network is not on this station's leg
        // (the identifier is the other station's and only collides with ours).
        let party_here = self.individual_calls.get(&call_id).is_some_and(|c| {
            (sender.ssi == c.calling_addr.ssi && !c.calling_over_brew) || (sender.ssi == c.called_addr.ssi && !c.called_over_brew)
        });
        if !group_restore
            && !party_here
            && self.join_network_call_locally(
                queue,
                sender,
                handle,
                link_id,
                endpoint_id,
                restored_call_id,
                pdu.other_party_ssi.unwrap_or(0) as u32,
                request_to_transmit,
            )
        {
            return;
        }
        if !group_restore && !party_here && (brew::is_site_linked(&self.config) || brew::is_active(&self.config)) {
            let peer = pdu.other_party_ssi.unwrap_or(0) as u32;
            tracing::info!(
                "CMCE: U-CALL RESTORE from ISSI {} for call_id={} peer={} — not held here, asking the network",
                sender.ssi,
                call_id,
                peer
            );
            let restore = DeferredIndividualRestore {
                sender,
                handle,
                link_id,
                endpoint_id,
                call_id,
                peer,
                request_to_transmit,
                since: self.dltime,
            };
            // A repeated U-CALL RESTORE replaces the parked one (fresh handle, same deadline).
            if let Some(old) = self.deferred_individual_restores.iter_mut().find(|r| r.sender.ssi == sender.ssi) {
                let since = old.since;
                *old = DeferredIndividualRestore { since, ..restore };
            } else {
                self.deferred_individual_restores.push(restore);
                Self::push_control(
                    queue,
                    TetraEntity::Brew,
                    CallControl::IndividualCallRestoreRequest { issi: sender.ssi, peer },
                );
            }
            return;
        }

        if self.individual_calls.contains_key(&call_id) {
            self.restore_individual_call(queue, sender, handle, link_id, endpoint_id, restored_call_id, call_id, request_to_transmit);
            return;
        }

        if self.active_calls.get(&call_id).is_some_and(is_own_group_call) {
            self.restore_group_call(
                queue,
                sender,
                handle,
                link_id,
                endpoint_id,
                restored_call_id,
                call_id,
                request_to_transmit,
            );
            return;
        }

        self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
    }

    /// Restore an MS into the local individual call `call_id` (D-CALL RESTORE, TS 100 392-2 clause
    /// 14.5.1.2.4). `restored_call_id` is the identifier the MS sent (another cell's, possibly).
    /// Returns the transmission grant given, or `None` when the restore was rejected.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cmce::subentities::cc_bs) fn restore_individual_call(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        restored_call_id: u16,
        call_id: u16,
        request_to_transmit: bool,
    ) -> Option<TransmissionGrant> {
        if let Some(call) = self.individual_calls.get_mut(&call_id) {
            if !call.is_active() || (sender.ssi != call.calling_addr.ssi && sender.ssi != call.called_addr.ssi) {
                self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
                return None;
            }

            if call.begin_restore().is_err() {
                self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
                return None;
            }
            call.active_timer_started = Some(self.dltime);
            // A radio held for a handoff to another station came back here.
            if call.handoff_hold.is_some_and(|h| h.issi == sender.ssi) {
                call.handoff_hold = None;
            }
            // Clause 14.5.1.2.4. Duplex: always granted ("transmission not granted" and "granted
            // to another user" are only applicable for simplex calls; a duplex MS always requests
            // to transmit). Simplex: grant only when the floor is free or the sender already holds
            // it (same decision as the U-TX DEMAND path; granting while the peer is talking gives
            // two transmitters). With the peer talking the MS is "granted to another user", which
            // switches its U-plane on to receive, whether it asked to transmit or not. An MS that
            // does not ask to transmit gave up a floor it held at the break ("the CC shall
            // assume that the permission is now ended").
            let peer_holds_floor = call.floor_holder.is_some_and(|holder| holder != sender.ssi);
            let grant = if !call.is_simplex() {
                TransmissionGrant::Granted
            } else if peer_holds_floor {
                // "If the MS requested transmission in the U-CALL RESTORE PDU, the MS shall
                // assume that its transmission request is still queued in the SwMI": queue it as
                // a U-TX DEMAND, granted when the peer's transmission ends.
                if request_to_transmit {
                    let _ = call.queue_tx_demand(sender);
                }
                TransmissionGrant::GrantedToOtherUser
            } else if request_to_transmit {
                call.grant_floor(sender);
                TransmissionGrant::Granted
            } else {
                call.release_floor();
                TransmissionGrant::NotGranted
            };
            // A leg pre-provisioned for cross-cell handover has the MS arriving from another cell,
            // so it is not yet on this cell's traffic channel: the restore reply must carry a
            // channel allocation to move it there. An ordinary same-cell restore needs none (the
            // MS never left the channel). `restoring_cross_cell` is cleared by complete_restore, so
            // capture the channel first.
            let sender_chan = if sender.ssi == call.called_addr.ssi && sender.ssi != call.calling_addr.ssi {
                (call.called_carrier_num, call.called_ts, call.called_usage)
            } else {
                (call.calling_carrier_num, call.calling_ts, call.calling_usage)
            };
            let cross_cell_chan = call.restoring_cross_cell.then_some(sender_chan);
            call.complete_restore();
            // Clause 14.5.1.2.4: a layer 2 channel allocation shall accompany D-CALL RESTORE when
            // the U-plane is switched on; the MS still has the old cell's identifier, so this
            // cell's goes in "new call identifier".
            let chan_alloc = cross_cell_chan.map(|(carrier_num, ts, usage)| {
                let mut timeslots = [false; 4];
                timeslots[ts as usize - 1] = true;
                CmceChanAllocReq {
                    usage: Some(usage),
                    alloc_type: ChanAllocType::Replace,
                    carrier: Some(carrier_num),
                    timeslots,
                    ul_dl_assigned: UlDlAssignment::Both,
                    cell_change: false,
                }
            });
            self.send_d_call_restore(queue, sender, handle, link_id, endpoint_id, restored_call_id, call_id, grant, chan_alloc);
            return Some(grant);
        }
        self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
        None
    }

    /// The network's answer to `IndividualCallRestoreRequest`: restore `issi`'s parked U-CALL
    /// RESTORE into the call the network moved here, on a traffic channel of this cell, or reject
    /// it (D-RELEASE, which MLE turns into D-RESTORE-FAIL).
    pub(in crate::cmce::subentities::cc_bs) fn on_individual_call_restore_answer(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        peer: u32,
        call: Option<NetworkRestoredCall>,
    ) {
        let Some(pos) = self.deferred_individual_restores.iter().position(|r| r.sender.ssi == issi) else {
            // Answered too late: the MS was already rejected. The network moved the call here, so
            // end it there too.
            if let Some(call) = call {
                tracing::info!("CMCE: network answer for ISSI {}'s restore came too late, releasing uuid={}", issi, call.brew_uuid);
                self.notify_network_circuit_release(
                    queue,
                    TetraEntity::Brew,
                    call.brew_uuid,
                    DisconnectCause::CallRestorationOfTheOtherUserFailed,
                );
            }
            return;
        };
        let restore = self.deferred_individual_restores.remove(pos);
        let Some(call) = call else {
            tracing::info!("CMCE: the network has no call of ISSI {} with peer={} to restore", issi, peer);
            self.reject_call_restore(queue, restore.sender, restore.handle, restore.link_id, restore.endpoint_id, restore.call_id);
            return;
        };
        let Some(call_id) = self.provision_restoring_individual_leg(queue, call.brew_uuid, issi, peer, call.simplex_duplex, call.priority) else {
            self.reject_call_restore(queue, restore.sender, restore.handle, restore.link_id, restore.endpoint_id, restore.call_id);
            self.notify_network_circuit_release(queue, TetraEntity::Brew, call.brew_uuid, DisconnectCause::CongestionInInfrastructure);
            return;
        };
        let peer_talks = call.floor_issi.is_some_and(|f| f == peer) && !call.simplex_duplex;
        if peer_talks && let Some(c) = self.individual_calls.get_mut(&call_id) {
            c.floor_holder = Some(peer);
        }
        tracing::info!(
            "CMCE: network moved ISSI {}'s call here: uuid={} local call_id={} (restored call_id={}) peer={} duplex={}",
            issi,
            call.brew_uuid,
            call_id,
            restore.call_id,
            peer,
            call.simplex_duplex
        );
        let Some(grant) = self.restore_individual_call(
            queue,
            restore.sender,
            restore.handle,
            restore.link_id,
            restore.endpoint_id,
            restore.call_id,
            call_id,
            restore.request_to_transmit,
        ) else {
            return;
        };
        let Some(leg) = self.individual_calls.get(&call_id) else { return };
        let (carrier_num, ts) = (leg.calling_carrier_num, leg.calling_ts);
        // Voice of this call now runs between this traffic channel and the network.
        Self::push_control(
            queue,
            TetraEntity::Brew,
            CallControl::NetworkCircuitMediaReady {
                brew_uuid: call.brew_uuid,
                call_id,
                carrier_num,
                ts,
            },
        );
        let held_floor_on_the_network = call.floor_issi == Some(issi);
        match grant {
            // The restored talker: arm UL inactivity as every other individual grant does.
            TransmissionGrant::Granted if !call.simplex_duplex => {
                self.notify_floor_granted(
                    queue,
                    GroupFloorGrant {
                        call_id,
                        source_issi: issi,
                        dest_gssi: peer,
                        carrier_num,
                        ts,
                        is_group: false,
                    },
                    true,
                    BrewNotification::Never,
                );
                // The network had the floor free: the other party learns that this MS talks
                // now, as after a U-TX DEMAND grant.
                if !held_floor_on_the_network {
                    Self::push_control(
                        queue,
                        TetraEntity::Brew,
                        CallControl::NetworkCircuitSimplexGranted {
                            brew_uuid: call.brew_uuid,
                            grant: TransmissionGrant::Granted.into_raw() as u8,
                            permission: 0,
                        },
                    );
                }
            }
            // It held the floor at the break and did not ask to go on: its permission ended
            // (clause 14.5.1.2.4), so the floor is free for the other party.
            TransmissionGrant::NotGranted if held_floor_on_the_network => Self::push_control(
                queue,
                TetraEntity::Brew,
                CallControl::NetworkCircuitSimplexIdle {
                    brew_uuid: call.brew_uuid,
                    grant: TransmissionGrant::NotGranted.into_raw() as u8,
                    permission: 0,
                },
            ),
            TransmissionGrant::GrantedToOtherUser => {
                Self::push_control(queue, TetraEntity::Umac, CallControl::RemoteFloorGranted { call_id, carrier_num, ts });
            }
            _ => {}
        }
    }

    /// Per tick: reject individual restores the network did not answer in time.
    fn expire_individual_restores(&mut self, queue: &mut MessageQueue) {
        if self.deferred_individual_restores.is_empty() {
            return;
        }
        let parked = std::mem::take(&mut self.deferred_individual_restores);
        for restore in parked {
            if restore.since.age(self.dltime) >= NETWORK_RESTORE_TIMEOUT_TS {
                tracing::info!("CMCE: no network answer for ISSI {}'s individual call restore", restore.sender.ssi);
                self.reject_call_restore(queue, restore.sender, restore.handle, restore.link_id, restore.endpoint_id, restore.call_id);
            } else {
                self.deferred_individual_restores.push(restore);
            }
        }
    }

    /// Restore an MS into the local group call `call_id` (D-CALL RESTORE, floor if requested and
    /// free). `restored_call_id` is the identifier the MS sent (another cell's, possibly).
    #[allow(clippy::too_many_arguments)]
    fn restore_group_call(
        &mut self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        restored_call_id: u16,
        call_id: u16,
        request_to_transmit: bool,
    ) {
        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return;
        };
        if call.begin_restore().is_err() {
            self.reject_call_restore(queue, sender, handle, link_id, endpoint_id, call_id);
            return;
        }
        // Clause 14.5.2.2.4: "granted to another user" switches the MS's U-plane on to receive,
        // "not granted" leaves it off; a listener restored into a running over must hear it.
        let grant = if request_to_transmit && (!call.tx_active || call.source_issi == sender.ssi) {
            // A talker that was heard here through the network keeps its floor; forget the
            // network session (see `take_over_moved_talker`).
            call.take_over_moved_talker(sender.ssi);
            call.grant_floor(sender.ssi, Some(sender));
            TransmissionGrant::Granted
        } else if call.tx_active {
            TransmissionGrant::GrantedToOtherUser
        } else {
            TransmissionGrant::NotGranted
        };
        // Clause 14.5.2.2.4: a layer 2 channel allocation shall accompany D-CALL RESTORE when the
        // U-plane is switched on, and may when it stays off but a traffic channel is available.
        // The call has its traffic channel, so always send the MS there.
        let mut timeslots = [false; 4];
        timeslots[call.ts as usize - 1] = true;
        let chan_alloc = CmceChanAllocReq {
            usage: Some(call.usage),
            alloc_type: ChanAllocType::Replace,
            carrier: Some(call.carrier_num),
            timeslots,
            ul_dl_assigned: UlDlAssignment::Both,
            cell_change: false,
        };

        call.complete_restore();
        let floor = (grant == TransmissionGrant::Granted).then_some(GroupFloorGrant {
            call_id,
            source_issi: sender.ssi,
            dest_gssi: call.dest_gssi,
            carrier_num: call.carrier_num,
            ts: call.ts,
            is_group: true,
        });
        self.send_d_call_restore(
            queue,
            sender,
            handle,
            link_id,
            endpoint_id,
            restored_call_id,
            call_id,
            grant,
            Some(chan_alloc),
        );

        // A restore that hands over the floor must arm UMAC's UL-inactivity timer exactly like
        // every other grant path (see setup.rs / U-TX DEMAND): without the FloorGranted a
        // restored talker that then goes silent pins the traffic slot until the absolute call
        // time-out — forever for a duplex/Infinite call, so repeats exhaust the cell.
        if let Some(floor) = floor {
            self.send_d_tx_granted_facch(queue, call_id, floor.source_issi, floor.dest_gssi, floor.carrier_num, floor.ts);
            self.notify_floor_granted(queue, floor, true, BrewNotification::IfGroupRoutable(floor.dest_gssi));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send_d_call_restore(
        &self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        restored_call_id: u16,
        call_id: u16,
        grant: TransmissionGrant,
        chan_alloc: Option<CmceChanAllocReq>,
    ) {
        let new_call_id = (call_id != restored_call_id).then_some(call_id);
        let sdu = Self::build_d_call_restore(restored_call_id, new_call_id, grant, Some(CallStatus::Callcontinue));
        let mut msg = Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id);
        if let SapMsgInner::LcmcMleUnitdataReq(prim) = &mut msg.msg {
            prim.chan_alloc = chan_alloc;
        }
        queue.push_back(msg);
    }

    fn reject_call_restore(
        &self,
        queue: &mut MessageQueue,
        sender: TetraAddress,
        handle: u32,
        link_id: u32,
        endpoint_id: u32,
        call_id: u16,
    ) {
        tracing::info!("CMCE: rejecting U-CALL RESTORE for unknown or inactive call_id={}", call_id);
        let sdu = Self::build_d_release(call_id, DisconnectCause::CallRestorationOfTheOtherUserFailed);
        let msg = Self::build_sapmsg_direct(sdu, self.dltime, sender, handle, link_id, endpoint_id);
        queue.push_back(msg);
    }

    /// Park a U-CALL RESTORE until the Brew group call comes up on this cell.
    ///
    /// The MS was in the call on another cell, so it listens to the group: count it as a local
    /// listener, which promotes a call Brew holds as pending inbound (`GroupListenersAvailable`).
    /// The count is the cell's own bookkeeping, nothing is sent on air; it is dropped again when
    /// the restore fails or the call ends, unless MM affiliates the MS to the group meanwhile.
    /// An MS without a registration here was asked to register (`request_registration_if_unknown`);
    /// its registration makes the network hand this cell the running call.
    pub(in crate::cmce::subentities::cc_bs) fn defer_call_restore(&mut self, queue: &mut MessageQueue, restore: DeferredRestore) {
        let (issi, gssi) = (restore.sender.ssi, restore.gssi);
        tracing::info!(
            "CMCE: U-CALL RESTORE from ISSI {} for call_id={} gssi={} — no local call yet, waiting for the network call",
            issi,
            restore.call_id,
            gssi
        );
        // A repeated U-CALL RESTORE replaces the parked one (fresh handle, same deadline).
        if let Some(old) = self.deferred_restores.iter_mut().find(|r| r.sender.ssi == issi) {
            let since = old.since;
            *old = DeferredRestore { since, ..restore };
        } else {
            self.deferred_restores.push(restore);
        }

        self.add_restore_listener(queue, issi, gssi);
    }

    /// Count `issi` as a listener of `gssi` until MM affiliates it, the call ends or it leaves
    /// (see `defer_call_restore`). A cell that gains its first listener this way asks Brew for the
    /// call it holds as pending inbound.
    fn add_restore_listener(&mut self, queue: &mut MessageQueue, issi: u32, gssi: u32) {
        let affiliated = self.subscriber_groups.get(&issi).is_some_and(|groups| groups.contains(&gssi));
        if !affiliated && self.restore_listeners.insert((issi, gssi)) {
            let first_listener = !self.has_listener(gssi);
            self.inc_group_listener(gssi);
            if first_listener {
                Self::push_control(queue, TetraEntity::Brew, CallControl::GroupListenersAvailable { gssi });
            }
        }
    }

    /// Target station of an announced type 1 cell reselection (TS 100 392-2 clause 18.3.4.7.5):
    /// the serving station asks for the traffic channel of `gssi`'s call, so the MS can be sent
    /// straight there and continue the call without restoring it. The MS counts as a listener as
    /// for a restore, which brings a call Brew only holds as pending inbound up here. Answered
    /// with `HandoverGroupCallReady` when the call runs, or without a channel after
    /// `HANDOVER_CALL_TIMEOUT_TS` (the serving station then uses type 2).
    pub(in crate::cmce::subentities::cc_bs) fn prepare_handover_group_call(&mut self, queue: &mut MessageQueue, issi: u32, gssi: u32) {
        if !self.restores_group_by_gssi(gssi) || brew::is_site_linked(&self.config) {
            Self::push_control(queue, TetraEntity::Brew, CallControl::HandoverGroupCallReady { issi, gssi, call: None });
            return;
        }
        self.add_restore_listener(queue, issi, gssi);
        self.handover_reservations.retain(|r| r.issi != issi);
        if !self.answer_handover_reservation(queue, issi, gssi) {
            tracing::info!("CMCE: ISSI {} arrives for gssi={} by type 1 reselection — waiting for the network call", issi, gssi);
            self.handover_reservations.push(HandoverReservation {
                issi,
                gssi,
                since: self.dltime,
            });
        }
    }

    /// Answer a type 1 reservation if `gssi`'s call runs here. True if answered.
    fn answer_handover_reservation(&self, queue: &mut MessageQueue, issi: u32, gssi: u32) -> bool {
        let Some((&call_id, call)) = self.active_calls.iter().find(|(_, c)| c.dest_gssi == gssi) else {
            return false;
        };
        tracing::info!(
            "CMCE: channel for ISSI {}'s type 1 reselection: call_id={} gssi={} carrier={} ts={} usage={}",
            issi,
            call_id,
            gssi,
            call.carrier_num,
            call.ts,
            call.usage
        );
        Self::push_control(
            queue,
            TetraEntity::Brew,
            CallControl::HandoverGroupCallReady {
                issi,
                gssi,
                call: Some((call_id, call.carrier_num, call.ts, call.usage)),
            },
        );
        true
    }

    /// Per tick: answer type 1 reservations whose call is up, give up on the others in time.
    fn drive_handover_reservations(&mut self, queue: &mut MessageQueue) {
        if self.handover_reservations.is_empty() {
            return;
        }
        let parked = std::mem::take(&mut self.handover_reservations);
        for r in parked {
            if self.answer_handover_reservation(queue, r.issi, r.gssi) {
                continue;
            }
            if r.since.age(self.dltime) >= HANDOVER_CALL_TIMEOUT_TS {
                tracing::info!("CMCE: no network call for gssi={} came up for ISSI {}'s type 1 reselection", r.gssi, r.issi);
                Self::push_control(
                    queue,
                    TetraEntity::Brew,
                    CallControl::HandoverGroupCallReady {
                        issi: r.issi,
                        gssi: r.gssi,
                        call: None,
                    },
                );
                self.drop_restore_listener(queue, r.issi, r.gssi);
            } else {
                self.handover_reservations.push(r);
            }
        }
    }

    /// An MS restoring a call on a separate Brew station without being registered there
    /// reselected within its registered location area, so it did not register (TS 100 392-2
    /// clause 16). Ask it to: infrastructure initiated registration, D-LOCATION UPDATE COMMAND with
    /// group report request (clauses 16.4.3 and 16.9.2.8: "The SwMI may initiate registration at
    /// any time"). Its
    /// registration moves the ISSI to this station on Brew, so the network routes the next calls,
    /// SDS and individual calls here. A linked multi-cell station tracks its radios itself.
    fn request_registration_if_unknown(&self, queue: &mut MessageQueue, issi: u32) {
        if !brew::is_active(&self.config) || brew::is_site_linked(&self.config) || self.is_locally_registered_issi(issi) {
            return;
        }
        tracing::info!(
            "CMCE: restoring ISSI {} is not registered on this cell — requesting a location update",
            issi
        );
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Mm,
            msg: SapMsgInner::MmRequestLocationUpdate { issi },
        });
    }

    /// Per tick: answer parked restores whose group call is now up, reject the ones that waited
    /// too long (D-RELEASE, which MLE turns into D-RESTORE-FAIL).
    pub(in crate::cmce::subentities::cc_bs) fn drive_deferred_restores(&mut self, queue: &mut MessageQueue) {
        self.drive_handover_reservations(queue);
        self.expire_individual_restores(queue);
        self.expire_handoff_holds(queue);
        if self.deferred_restores.is_empty() {
            return;
        }
        let parked = std::mem::take(&mut self.deferred_restores);
        for restore in parked {
            let live_call = self
                .active_calls
                .iter()
                .find(|(_, c)| c.dest_gssi == restore.gssi)
                .map(|(&id, _)| id);
            if let Some(call_id) = live_call {
                tracing::info!(
                    "CMCE: network call for gssi={} is up — restoring ISSI {} into call_id={}",
                    restore.gssi,
                    restore.sender.ssi,
                    call_id
                );
                self.restore_group_call(
                    queue,
                    restore.sender,
                    restore.handle,
                    restore.link_id,
                    restore.endpoint_id,
                    restore.call_id,
                    call_id,
                    restore.request_to_transmit,
                );
            } else if restore.since.age(self.dltime) >= DEFERRED_RESTORE_TIMEOUT_TS {
                tracing::info!(
                    "CMCE: no network call for gssi={} came up for ISSI {}'s restore",
                    restore.gssi,
                    restore.sender.ssi
                );
                self.reject_call_restore(
                    queue,
                    restore.sender,
                    restore.handle,
                    restore.link_id,
                    restore.endpoint_id,
                    restore.call_id,
                );
                self.drop_restore_listener(queue, restore.sender.ssi, restore.gssi);
            } else {
                self.deferred_restores.push(restore);
            }
        }
    }

    /// Undo the listener count a restore added for (issi, gssi), if MM did not affiliate since.
    pub(in crate::cmce::subentities::cc_bs) fn drop_restore_listener(&mut self, queue: &mut MessageQueue, issi: u32, gssi: u32) {
        if self.restore_listeners.remove(&(issi, gssi)) {
            self.dec_group_listener(gssi);
            self.drop_group_calls_if_unlistened(queue, gssi);
        }
    }

    /// The group call ended: listeners that only came from a restore are gone with it.
    pub(in crate::cmce::subentities::cc_bs) fn drop_restore_listeners_for_group(&mut self, gssi: u32) {
        let pairs: Vec<(u32, u32)> = self.restore_listeners.iter().filter(|(_, g)| *g == gssi).copied().collect();
        for pair in pairs {
            self.restore_listeners.remove(&pair);
            self.dec_group_listener(gssi);
        }
    }
}
