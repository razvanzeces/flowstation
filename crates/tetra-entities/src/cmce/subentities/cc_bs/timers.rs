use super::*;

/// Energy-Economy D-SETUP gate (clause 16.7): individual-call setup resends are held for the
/// called MS's monitoring window, but if the window has not opened within this many timeslots of
/// setup start we fall back to the historical blind resend. ~6 s (a few EE cycles) — chosen to be
/// comfortably under the shortest setup timeout (`T10s`/`Predefined`) so a wrong granted window
/// phase degrades to "no worse than before", never to a setup that times out unanswered.
/// (6 s / (170/12 ms per slot) ≈ 423 timeslots.)
pub(super) const EE_DSETUP_FALLBACK_TS: i32 = 423;

impl CcBsSubentity {
    pub fn tick_start(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        self.dltime = dltime;

        // ETSI T310 equivalent for active calls.
        self.check_call_timeout_expiry(queue);
        // ETSI T301/T302 equivalent while waiting for call completion.
        self.check_individual_setup_timeout(queue);
        // Check hangtime expiry for active local calls
        self.check_hangtime_expiry(queue);

        // Energy-economy group-call announce batching: re-emit the group D-SETUP across the
        // union of affiliated EE members' wake frames so members on a different sleep phase
        // still receive the call. No-op for all-StayAlive groups.
        self.drive_group_ee_announce(queue);

        if let Some(tasks) = self.circuits.tick_start(dltime) {
            for task in tasks {
                match task {
                    CircuitMgrCmd::SendDSetup(call_id, usage, carrier_num, ts) => {
                        // Peek at routing info first (immutable) so the EE gate — a `&self` method —
                        // can run before we take the mutable borrow on the cached D-SETUP below.
                        let (dest_ssi, resend) = match self.cached_setups.get(&call_id) {
                            Some(c) => (c.dest_addr.ssi, c.resend),
                            None => {
                                tracing::debug!(
                                    "CMCE: skipping D-SETUP resend for call_id={} (no cached D-SETUP; likely Brew-routed individual call)",
                                    call_id
                                );
                                continue;
                            }
                        };
                        if !resend {
                            continue;
                        }
                        // Energy-Economy gate (clause 16.7): hold an individual-call D-SETUP resend
                        // until the called MS's downlink monitoring window opens, so the page lands
                        // when the radio is actually listening. The bounded fallback inside
                        // `ee_dsetup_blocks` reverts to the blind resend after EE_DSETUP_FALLBACK_TS,
                        // so a wrong granted window phase is never worse than the historical behaviour.
                        if self.individual_calls.contains_key(&call_id) && self.ee_dsetup_blocks(call_id, dest_ssi) {
                            tracing::debug!(
                                "EE: holding D-SETUP resend for {} (call_id {}) until its monitoring window",
                                dest_ssi,
                                call_id
                            );
                            continue;
                        }

                        // Take the mutable borrow now that the EE gate (a `&self` method) has run.
                        let cached = self.cached_setups.get_mut(&call_id).expect("cached D-SETUP present (peeked above)");
                        if let Some(receipt) = cached.tx_receipt.as_ref()
                            && !receipt.is_in_final_state()
                        {
                            tracing::debug!(
                                "CMCE: throttling D-SETUP resend for call_id={} while previous resend is {:?}",
                                call_id,
                                receipt.get_state()
                            );
                            continue;
                        }

                        // Update transmission_grant based on current call state:
                        // During NoActiveSpeaker (nobody transmitting), use NotGranted;
                        // during Transmitting, use GrantedToOtherUser.
                        if let Some(active) = self.active_calls.get(&call_id) {
                            cached.pdu.transmission_grant = if active.is_tx_active() {
                                TransmissionGrant::GrantedToOtherUser
                            } else {
                                TransmissionGrant::NotGranted
                            };
                        }
                        let dest_addr = cached.dest_addr;
                        let (sdu, chan_alloc) = Self::build_d_setup_prim(&cached.pdu, usage, carrier_num, ts, UlDlAssignment::Both);
                        let reporter = TxReporter::new_unacked();
                        let receipt = reporter.clone();
                        cached.tx_receipt = Some(receipt);
                        let prim = Self::build_sapmsg(sdu, Some(chan_alloc), self.dltime, dest_addr, Some(reporter));
                        queue.push_back(prim);
                    }

                    CircuitMgrCmd::SendClose(call_id, circuit) => {
                        tracing::warn!("need to send CLOSE for call id {}", call_id);
                        let slot = CarrierSlot {
                            carrier_num: circuit.carrier_num,
                            ts: circuit.ts,
                        };
                        // Safety circuit expiry is not a setup timeout. Do not report it to
                        // handsets as ExpiryOfTimer, which many radios render as "No answer".
                        let disconnect_cause = DisconnectCause::SwmiRequestedDisconnection;

                        // Get our cached D-SETUP, build D-RELEASE and send
                        if let Some(cached) = self.cached_setups.get(&call_id) {
                            let sdu = Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause);
                            let prim = Self::build_sapmsg(sdu, None, self.dltime, cached.dest_addr, None);
                            queue.push_back(prim);

                            if let Some(ind_call) = self.individual_calls.get(&call_id) {
                                if !ind_call.calling_over_brew {
                                    let sdu_calling = Self::build_d_release_from_d_setup(&cached.pdu, disconnect_cause);
                                    let prim_calling = SapMsg {
                                        sap: Sap::LcmcSap,
                                        src: TetraEntity::Cmce,
                                        dest: TetraEntity::Mle,
                                        msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                                            sdu: sdu_calling,
                                            handle: ind_call.calling_handle,
                                            endpoint_id: ind_call.calling_endpoint_id,
                                            link_id: ind_call.calling_link_id,
                                            // Unacknowledged BL-UDATA: the legacy `main` SendClose
                                            // calling-leg D-RELEASE was sent unacknowledged (FH FIX 2).
                                            layer2service: Layer2Service::Unacknowledged,
                                            pdu_prio: 0,
                                            layer2_qos: 0,
                                            stealing_permission: false,
                                            stealing_repeats_flag: false,
                                            chan_alloc: None,
                                            main_address: ind_call.calling_addr,
                                            tx_reporter: None,
                                        }),
                                    };
                                    queue.push_back(prim_calling);
                                }
                            }
                        } else {
                            tracing::warn!("No cached D-SETUP for call id {} during timer-close", call_id);
                            if let Some(ind_call) = self.individual_calls.get(&call_id) {
                                if !ind_call.calling_over_brew {
                                    let sdu_calling = Self::build_d_release(call_id, disconnect_cause);
                                    let prim_calling = if ind_call.is_active() {
                                        Self::build_sapmsg_stealing(
                                            sdu_calling,
                                            self.dltime,
                                            ind_call.calling_addr,
                                            ind_call.calling_carrier_num,
                                            ind_call.calling_ts,
                                            Some(ind_call.calling_usage),
                                        )
                                    } else {
                                        Self::build_sapmsg_direct(
                                            sdu_calling,
                                            self.dltime,
                                            ind_call.calling_addr,
                                            ind_call.calling_handle,
                                            ind_call.calling_link_id,
                                            ind_call.calling_endpoint_id,
                                        )
                                    };
                                    queue.push_back(prim_calling);
                                } else if !ind_call.called_over_brew {
                                    let sdu_called = Self::build_d_release(call_id, disconnect_cause);
                                    let prim_called = if ind_call.is_active() {
                                        Self::build_sapmsg_stealing(
                                            sdu_called,
                                            self.dltime,
                                            ind_call.called_addr,
                                            ind_call.called_carrier_num,
                                            ind_call.called_ts,
                                            Some(ind_call.called_usage),
                                        )
                                    } else if let (Some(handle), Some(link_id), Some(endpoint_id)) =
                                        (ind_call.called_handle, ind_call.called_link_id, ind_call.called_endpoint_id)
                                    {
                                        Self::build_sapmsg_direct(
                                            sdu_called,
                                            self.dltime,
                                            ind_call.called_addr,
                                            handle,
                                            link_id,
                                            endpoint_id,
                                        )
                                    } else {
                                        Self::build_sapmsg(sdu_called, None, self.dltime, ind_call.called_addr, None)
                                    };
                                    queue.push_back(prim_called);
                                }
                            }
                        }

                        if let Some(ind_call) = self.individual_calls.get(&call_id) {
                            if (ind_call.called_over_brew || ind_call.calling_over_brew)
                                && let Some(brew_uuid) = ind_call.brew_uuid
                            {
                                let network_entity = ind_call.network_entity;
                                self.notify_network_circuit_release(queue, network_entity, brew_uuid, disconnect_cause);
                            }
                        }

                        // Clean up call state
                        if let Some(call) = self.active_calls.get_mut(&call_id) {
                            call.begin_release(disconnect_cause);
                        }
                        if let Some(call) = self.individual_calls.get_mut(&call_id) {
                            call.begin_release(disconnect_cause);
                        }
                        self.cached_setups.remove(&call_id);
                        let removed_group = self.active_calls.remove(&call_id).is_some();
                        let removed_individual = self.individual_calls.remove(&call_id).is_some();

                        // Signal UMAC to release the circuit
                        Self::signal_umac_circuit_close(queue, circuit, self.dltime);
                        self.release_timeslot_slot(slot);

                        // Dashboard telemetry: the CircuitMgr safety-expiry (`SendClose`) tears a
                        // call down directly here without going through release_group_call /
                        // release_individual_call, so emit the matching CallEnded for whichever
                        // table the call was removed from above.
                        if removed_group {
                            self.emit(crate::net_telemetry::TelemetryEvent::GroupCallEnded { call_id, gssi: 0 });
                        }
                        if removed_individual {
                            self.emit(crate::net_telemetry::TelemetryEvent::IndividualCallEnded { call_id });
                        }
                    }
                }
            }
        }
    }

    /// Publish the live "identity on a traffic channel → (timeslot, usage_marker)" map into
    /// shared state so the SDS path can FACCH-steal to an MS engaged in a call instead of
    /// sending on the MCCH it is no longer monitoring (ETSI EN 300 392-2 §23.5). Rebuilt from
    /// the live call tables every tick, so it can never reference a stale/closed circuit.
    pub fn publish_active_call_ts(&self) {
        use std::collections::HashMap;
        let mut map: HashMap<u32, (u16, u8, u8)> = HashMap::new();
        // Group calls: the group address and the current/last speaker ISSI are both on the
        // group's assigned traffic slot.
        for call in self.active_calls.values() {
            map.insert(call.dest_gssi, (call.carrier_num, call.ts, call.usage));
            map.insert(call.source_issi, (call.carrier_num, call.ts, call.usage));
        }
        // Individual calls: parties are on a traffic channel only once the call is connected.
        for call in self.individual_calls.values() {
            if call.is_active() {
                map.insert(
                    call.calling_addr.ssi,
                    (call.calling_carrier_num, call.calling_ts, call.calling_usage),
                );
                map.insert(call.called_addr.ssi, (call.called_carrier_num, call.called_ts, call.called_usage));
            }
        }
        self.config.state_write().active_call_ts = map;
    }

    /// Release active calls when their configured call timeout expires.
    pub(super) fn check_call_timeout_expiry(&mut self, queue: &mut MessageQueue) {
        let expired_group_calls: Vec<u16> = self
            .active_calls
            .iter()
            .filter_map(|(&call_id, call)| call.call_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_group_calls {
            tracing::info!("Call timeout expired for group call_id={}, releasing", call_id);
            self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        }

        let expired_individual_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| call.active_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_individual_calls {
            tracing::info!("Call timeout expired for individual call_id={}, releasing", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        }
    }

    /// Release individual setup attempts that exceed setup timeout.
    pub(super) fn check_individual_setup_timeout(&mut self, queue: &mut MessageQueue) {
        let expired_setup_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| call.setup_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_setup_calls {
            tracing::info!("Setup timeout expired for individual call_id={}, releasing", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }

        // EE DSetup retry: for P2P individual calls still in CallSetupPending state
        // (called MS has not yet sent U-ALERT), periodically retransmit DSetup on MCCH
        // so that a sleeping MS can receive it at its next monitoring window.
        // Retry interval ~2.5 s (180 timeslots; one slot = 170/12 ms, ~72 slots/s). Frequent enough
        // that a retry instant has a good chance of coinciding with the called MS's EE monitoring
        // window (the ee_dsetup_blocks gate below only lets a retry through when that window is open),
        // yet bounded so we never flood the MS before the 60 s setup timeout.
        // NOTE: TdmaTime::age()/diff() return TIMESLOTS (not frames) — locals are named accordingly.
        const DSETUP_RETRY_INTERVAL_TS: i32 = 180; // ~2.5 s
        let retry_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| {
                if call.state != IndividualCallState::CallSetupPending {
                    return None;
                }
                let started = call.setup_timer_started?;
                let age_ts = started.age(self.dltime);
                // First retry after ~0.25 s, then every ~2.5 s.
                if age_ts >= 18 && age_ts % DSETUP_RETRY_INTERVAL_TS == 0 {
                    Some(call_id)
                } else {
                    None
                }
            })
            .collect();

        for call_id in retry_calls {
            let Some(cached) = self.cached_setups.get(&call_id) else {
                continue;
            };
            let dest_addr = cached.dest_addr;
            // Same Energy-Economy monitoring-window gate as the circuit_mgr resend path: while the
            // called MS's window is closed, hold this retry (the bounded fallback inside
            // `ee_dsetup_blocks` resumes it if the granted window phase turns out wrong). This is
            // what actually aligns the retry to the MS's wake window instead of blind spamming.
            if self.ee_dsetup_blocks(call_id, dest_addr.ssi) {
                tracing::debug!(
                    "EE: holding D-SETUP setup-retry for {} (call_id {}) until its monitoring window",
                    dest_addr.ssi,
                    call_id
                );
                continue;
            }
            let mut sdu = BitBuffer::new_autoexpand(80);
            if cached.pdu.to_bitbuf(&mut sdu).is_err() {
                continue;
            }
            sdu.seek(0);
            let prim = Self::build_sapmsg(sdu, None, self.dltime, dest_addr, None);
            tracing::debug!(
                "EE DSetup retry for call_id={} to ISSI {} (setup pending, MS reachable)",
                call_id,
                dest_addr.ssi
            );
            queue.push_back(prim);
        }
    }

    /// Energy-Economy monitoring-window gate for an individual-call D-SETUP resend (clause 16.7).
    ///
    /// Returns `true` when the called MS (`dest_ssi`) is under Energy Economy and its downlink
    /// monitoring window is currently closed, so the resend should be held until the window opens.
    /// Returns `false` — i.e. send now — when the MS is not in EE (absent from the published map),
    /// when its window is open, or once setup has been pending longer than `EE_DSETUP_FALLBACK_TS`
    /// (the bounded fallback: a wrong granted window phase degrades to the historical blind resend
    /// rather than blocking setup until it times out unanswered).
    fn ee_dsetup_blocks(&self, call_id: u16, dest_ssi: u32) -> bool {
        let window_closed = {
            let state = self.config.state_read();
            match state.ee_monitoring_windows.get(&dest_ssi) {
                Some(&(frame, mframe, cycle_len)) => !self.dltime.in_ee_monitoring_window(frame, mframe, cycle_len),
                None => false, // not in energy economy — always reachable
            }
        };
        if !window_closed {
            return false;
        }
        // Bounded fallback: stop holding once setup has been pending too long.
        match self.individual_calls.get(&call_id).and_then(|c| c.setup_timer_started) {
            Some(started) => started.age(self.dltime) < EE_DSETUP_FALLBACK_TS,
            None => false, // no setup clock to bound the wait — don't gate
        }
    }

    /// Energy-economy group-call announce batching (ETSI EN 300 392-2 §23.5 / §23.7).
    ///
    /// A group D-SETUP sent once reaches only members awake at that instant. EE members sleep on
    /// different phases (EG1/EG2/EG3 wake every 2/3/6 frames), so a member asleep at announce time
    /// would miss the call. While a group call is young (within `EE_DSETUP_FALLBACK_TS` of
    /// creation), this re-emits the cached group D-SETUP on each frame where a not-yet-covered
    /// affiliated EE member wakes, marking members covered as their window opens, until every EE
    /// member has had a wake frame. It is a strict no-op for an all-StayAlive group (those members
    /// received the first send and need no window) and after coverage completes — the normal ~5 s
    /// late-entry cadence then takes over for steady-state late joiners.
    fn drive_group_ee_announce(&mut self, queue: &mut MessageQueue) {
        // EE wake windows are whole-frame, so only act on frame boundaries (ts == 1).
        if self.dltime.t != 1 {
            return;
        }
        let now = self.dltime;

        // Group calls still inside their bounded announce window.
        let candidates: Vec<u16> = self
            .active_calls
            .iter()
            .filter(|(_, c)| !c.ee_announce_done && c.created_at.age(now) < EE_DSETUP_FALLBACK_TS)
            .map(|(&id, _)| id)
            .collect();
        if candidates.is_empty() {
            return;
        }

        for call_id in candidates {
            let Some(call) = self.active_calls.get(&call_id) else {
                continue;
            };
            let gssi = call.dest_gssi;
            let carrier_num = call.carrier_num;
            let ts = call.ts;
            let usage = call.usage;
            // The current speaker is awake by definition (it is transmitting); exclude it from
            // coverage so its own EE window — closed on most frames — can't keep the call from
            // ever reaching ee_announce_done or trigger pointless re-emits. Reading source_issi
            // (rather than the original caller) also tracks a mid-call floor handover.
            let source_issi = call.source_issi;
            let already_covered = call.ee_announce_covered.clone();

            // Affiliated members of this GSSI (CMCE's authoritative reverse affiliation map),
            // excluding the active speaker.
            let members: Vec<u32> = self
                .subscriber_groups
                .iter()
                .filter(|(issi, gs)| **issi != source_issi && gs.contains(&gssi))
                .map(|(&issi, _)| issi)
                .collect();

            // Refresh coverage for this frame: a member is covered when it is StayAlive (no
            // window — it got the first send) or its EE window is open this frame.
            let mut newly_covered: Vec<u32> = Vec::new();
            let mut any_ee_woke = false;
            let mut all_covered = true;
            {
                let state = self.config.state_read();
                for m in &members {
                    if already_covered.contains(m) {
                        continue;
                    }
                    match state.ee_monitoring_windows.get(m) {
                        None => newly_covered.push(*m), // StayAlive — already reached
                        Some(&(frame, mframe, cycle_len)) => {
                            if now.in_ee_monitoring_window(frame, mframe, cycle_len) {
                                newly_covered.push(*m);
                                any_ee_woke = true;
                            } else {
                                all_covered = false; // still asleep this frame
                            }
                        }
                    }
                }
            }

            // Apply coverage + completion to the call.
            if let Some(call) = self.active_calls.get_mut(&call_id) {
                for m in &newly_covered {
                    call.ee_announce_covered.insert(*m);
                }
                if all_covered {
                    call.ee_announce_done = true;
                }
            }

            // Re-emit the cached group D-SETUP only when a sleeping EE member actually woke this
            // frame, so it lands while the radio is listening. (Re-sending a group D-SETUP is the
            // established late-entry mechanism, so already-joined members tolerate the duplicate.)
            if any_ee_woke && let Some(cached) = self.cached_setups.get_mut(&call_id) {
                // Same late-entry grant tweak as the steady-state resend path.
                cached.pdu.transmission_grant = TransmissionGrant::GrantedToOtherUser;
                cached.pdu.transmission_request_permission = false;
                let dest_addr = cached.dest_addr;
                let (sdu, chan_alloc) = Self::build_d_setup_prim(&cached.pdu, usage, carrier_num, ts, UlDlAssignment::Both);
                let prim = Self::build_sapmsg(sdu, Some(chan_alloc), self.dltime, dest_addr, None);
                queue.push_back(prim);
                tracing::debug!(
                    "EE: group {} announce re-sent (call_id {}) to cover newly-awake member(s)",
                    gssi,
                    call_id
                );
            }
        }
    }

    /// Check if any active calls in NoActiveSpeaker (hangtime) have expired and release them.
    pub(super) fn check_hangtime_expiry(&mut self, queue: &mut MessageQueue) {
        // Hangtime in TDMA timeslots, from config (cell.hangtime_secs, default 5s).
        // TETRA: 18 frames/multiframe, 4 timeslots/frame → 72 timeslots/second.
        let hangtime_secs = self.config.config().cell.hangtime_secs as i32;
        let hangtime_frames: i32 = hangtime_secs * 18 * 4;

        let expired: Vec<u16> = self
            .active_calls
            .iter()
            .filter_map(|(&call_id, call)| match call.state() {
                GroupCallState::NoActiveSpeaker { since } if since.age(self.dltime) > hangtime_frames => Some(call_id),
                _ => None,
            })
            .collect();

        for call_id in expired {
            tracing::info!("Hangtime expired for call_id={}, releasing", call_id);
            self.release_group_call(queue, call_id, DisconnectCause::SwmiRequestedDisconnection);
        }
    }

    /// Handle UL inactivity timeout from UMAC: a radio disappeared mid-transmission.
    /// Force the group floor to released and enter hangtime.
    pub(super) fn handle_ul_inactivity_timeout_slot(&mut self, queue: &mut MessageQueue, carrier_num: u16, ts: u8) {
        let call_id = self
            .active_calls
            .iter()
            .find(|(_, call)| call.carrier_num == carrier_num && call.ts == ts && call.is_tx_active())
            .map(|(call_id, _)| *call_id);

        let Some(call_id) = call_id else {
            let individual_floor = self.individual_calls.iter().find_map(|(&call_id, call)| {
                if !call.is_active() || !call.is_simplex() {
                    return None;
                }

                match call.floor_holder {
                    Some(issi) if issi == call.calling_addr.ssi && call.calling_carrier_num == carrier_num && call.calling_ts == ts => {
                        Some((call_id, call.calling_addr))
                    }
                    Some(issi) if issi == call.called_addr.ssi && call.called_carrier_num == carrier_num && call.called_ts == ts => {
                        Some((call_id, call.called_addr))
                    }
                    _ => None,
                }
            });

            if let Some((call_id, sender)) = individual_floor {
                tracing::warn!(
                    "UL inactivity timeout on carrier={} ts={}, forcing simplex individual TX ceased for call_id={}",
                    carrier_num,
                    ts,
                    call_id
                );
                self.fsm_on_u_tx_ceased(
                    queue,
                    sender,
                    UTxCeased {
                        call_identifier: call_id,
                        facility: None,
                        dm_ms_address: None,
                        proprietary: None,
                    },
                );
                return;
            }

            let duplex_individual = self.individual_calls.iter().find_map(|(&call_id, call)| {
                if !call.is_active() || call.is_simplex() {
                    return None;
                }

                let calling_match = call.calling_carrier_num == carrier_num && call.calling_ts == ts;
                let called_match = call.called_carrier_num == carrier_num && call.called_ts == ts;

                if !calling_match && !called_match {
                    return None;
                }

                let failed_addr = if calling_match && called_match {
                    if call.calling_over_brew && !call.called_over_brew {
                        call.called_addr
                    } else {
                        call.calling_addr
                    }
                } else if calling_match {
                    call.calling_addr
                } else {
                    call.called_addr
                };

                Some((call_id, failed_addr))
            });

            if let Some((call_id, failed_addr)) = duplex_individual {
                tracing::warn!(
                    "UL inactivity timeout on carrier={} ts={}, releasing duplex individual call_id={} after hard failure from ISSI {}",
                    carrier_num,
                    ts,
                    call_id,
                    failed_addr.ssi
                );
                self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
                return;
            }

            tracing::debug!(
                "UL inactivity timeout on carrier={} ts={} but no active transmitting call found",
                carrier_num,
                ts
            );
            return;
        };

        let Some(call) = self.active_calls.get_mut(&call_id) else {
            return;
        };

        tracing::warn!(
            "UL inactivity timeout on carrier={} ts={}, forcing TX ceased for call_id={}",
            carrier_num,
            ts,
            call_id
        );
        let dest_gssi = call.dest_gssi;
        let brew_notification = Self::brew_notification_for_group_call(call, call.source_issi);
        call.enter_hangtime(self.dltime);

        self.send_d_tx_ceased_facch(queue, call_id, dest_gssi, carrier_num, ts);

        self.notify_floor_released(
            queue,
            CallTimeslot { call_id, carrier_num, ts },
            true,
            brew_notification,
        );
    }

    pub(super) fn handle_ul_inactivity_timeout(&mut self, queue: &mut MessageQueue, ts: u8) {
        self.handle_ul_inactivity_timeout_slot(queue, self.config.config().cell.main_carrier, ts);
    }
}
