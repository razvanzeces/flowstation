use super::*;

impl CcBsSubentity {
    pub fn tick_start(&mut self, queue: &mut MessageQueue, dltime: TdmaTime) {
        self.dltime = dltime;

        // ETSI T310 equivalent for active calls.
        self.check_call_timeout_expiry(queue);
        // ETSI T301/T302 equivalent while waiting for call completion.
        self.check_individual_setup_timeout(queue);
        // Check hangtime expiry for active local calls
        self.check_hangtime_expiry(queue);

        if let Some(tasks) = self.circuits.tick_start(dltime) {
            for task in tasks {
                match task {
                    CircuitMgrCmd::SendDSetup(call_id, usage, ts) => {
                        // Get our cached D-SETUP, build a prim and send it down the stack
                        let Some(cached) = self.cached_setups.get_mut(&call_id) else {
                            tracing::trace!(
                                "CMCE: skipping D-SETUP resend for call_id={} (no cached D-SETUP; likely Brew-routed individual call)",
                                call_id
                            );
                            continue;
                        };
                        if !cached.resend {
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
                        let (sdu, chan_alloc) = Self::build_d_setup_prim(&cached.pdu, usage, ts, UlDlAssignment::Both);
                        let prim = Self::build_sapmsg(sdu, Some(chan_alloc), dest_addr, Layer2Service::Unacknowledged, None);
                        queue.push_back(prim);
                    }

                    CircuitMgrCmd::SendClose(call_id, circuit) => {
                        // Circuit expiry safety net: circuit_mgr detected a stale open circuit
                        // that CMCE forgot to close (e.g. MS lost coverage without disconnecting).
                        // Force cleanup unconditionally — release D-RELEASE, close circuit, free TS.
                        tracing::warn!(
                            "CMCE: force-closing stale circuit call_id={} ts={} (circuit expiry)",
                            call_id, circuit.ts
                        );
                        let ts = circuit.ts;
                        // Get our cached D-SETUP, build D-RELEASE and send
                        if let Some(cached) = self.cached_setups.get(&call_id) {
                            let sdu = Self::build_d_release_from_d_setup(&cached.pdu, DisconnectCause::ExpiryOfTimer);
                            let prim = Self::build_sapmsg(sdu, None, cached.dest_addr, Layer2Service::Unacknowledged, None);
                            queue.push_back(prim);

                            if let Some(ind_call) = self.individual_calls.get(&call_id) {
                                if !ind_call.calling_over_brew {
                                    let sdu_calling = Self::build_d_release_from_d_setup(&cached.pdu, DisconnectCause::ExpiryOfTimer);
                                    let prim_calling = SapMsg {
                                        sap: Sap::LcmcSap,
                                        src: TetraEntity::Cmce,
                                        dest: TetraEntity::Mle,
                                        msg: SapMsgInner::LcmcMleUnitdataReq(LcmcMleUnitdataReq {
                                            sdu: sdu_calling,
                                            handle: ind_call.calling_handle,
                                            endpoint_id: ind_call.calling_endpoint_id,
                                            link_id: ind_call.calling_link_id,
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
                                    let sdu_calling = Self::build_d_release(call_id, DisconnectCause::ExpiryOfTimer);
                                    let prim_calling = if ind_call.is_active() {
                                        Self::build_sapmsg_stealing(
                                            sdu_calling,
                                            ind_call.calling_addr,
                                            ind_call.calling_ts,
                                            Some(ind_call.calling_usage),
                                        )
                                    } else {
                                        Self::build_sapmsg_direct(
                                            sdu_calling,
                                            ind_call.calling_addr,
                                            ind_call.calling_handle,
                                            ind_call.calling_link_id,
                                            ind_call.calling_endpoint_id,
                                        )
                                    };
                                    queue.push_back(prim_calling);
                                } else if !ind_call.called_over_brew {
                                    let sdu_called = Self::build_d_release(call_id, DisconnectCause::ExpiryOfTimer);
                                    let prim_called = if ind_call.is_active() {
                                        Self::build_sapmsg_stealing(
                                            sdu_called,
                                            ind_call.called_addr,
                                            ind_call.called_ts,
                                            Some(ind_call.called_usage),
                                        )
                                    } else if let (Some(handle), Some(link_id), Some(endpoint_id)) =
                                        (ind_call.called_handle, ind_call.called_link_id, ind_call.called_endpoint_id)
                                    {
                                        Self::build_sapmsg_direct(
                                            sdu_called,
                                            ind_call.called_addr,
                                            handle,
                                            link_id,
                                            endpoint_id,
                                        )
                                    } else {
                                        Self::build_sapmsg(sdu_called, None, ind_call.called_addr, Layer2Service::Unacknowledged, None)
                                    };
                                    queue.push_back(prim_called);
                                }
                            }
                        }

                        if let Some(ind_call) = self.individual_calls.get(&call_id) {
                            if (ind_call.called_over_brew || ind_call.calling_over_brew)
                                && let Some(brew_uuid) = ind_call.brew_uuid
                            {
                                queue.push_back(SapMsg {
                                    sap: Sap::Control,
                                    src: TetraEntity::Cmce,
                                    dest: TetraEntity::Brew,
                                    msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease {
                                        brew_uuid,
                                        cause: DisconnectCause::ExpiryOfTimer.into_raw() as u8,
                                    }),
                                });
                            }
                        }

                        // Capture peer_ts before removing individual_calls (duplex P2P has two TS).
                        let peer_ts = self.individual_calls.get(&call_id).and_then(|ind| {
                            if ind.called_ts != ind.calling_ts { Some(ind.called_ts) } else { None }
                        });

                        // Clean up call state
                        self.cached_setups.remove(&call_id);
                        self.active_calls.remove(&call_id);
                        self.individual_calls.remove(&call_id);

                        // Signal UMAC to release the circuit
                        Self::signal_umac_circuit_close(queue, circuit);
                        self.release_timeslot(ts);

                        // For duplex P2P the call has two timeslots. The peer circuit will get
                        // its own SendClose from circuit_mgr, but individual_calls is already
                        // gone by then so its timeslot allocator entry would leak. Release it now.
                        if let Some(p_ts) = peer_ts {
                            if p_ts != ts {
                                self.release_timeslot(p_ts);
                            }
                        }
                    }
                }
            }
        }
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
            self.release_group_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }

        let expired_individual_calls: Vec<u16> = self
            .individual_calls
            .iter()
            .filter_map(|(&call_id, call)| call.active_timeout_expired(self.dltime).then_some(call_id))
            .collect();

        for call_id in expired_individual_calls {
            tracing::info!("Call timeout expired for individual call_id={}, releasing", call_id);
            self.release_individual_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
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
    }

    /// Check if any active calls in NoActiveSpeaker (hangtime) have expired and release them.
    pub(super) fn check_hangtime_expiry(&mut self, queue: &mut MessageQueue) {
        // Hangtime in TDMA timeslots: hangtime_secs * frames_per_sec * timeslots_per_frame
        // TETRA: 18 frames/multiframe, 4 timeslots/frame → 72 timeslots/second
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
            self.release_group_call(queue, call_id, DisconnectCause::ExpiryOfTimer);
        }
    }

    /// Handle UL inactivity timeout: force TX ceased for the transmitting MS on the given timeslot.
    /// Called when UMAC detects no voice frames on a traffic channel (UL side) for the timeout period.
    /// Corresponds to BS-side T323 expiry (ETSI EN 300 392-2 §14.9.2).
    pub(super) fn handle_ul_inactivity_timeout(&mut self, queue: &mut MessageQueue, ts: u8) {
        let call_entry = self
            .active_calls
            .iter()
            .find(|(_, call)| call.ts == ts && call.tx_active)
            .map(|(id, _)| *id);

        let Some(call_id) = call_entry else {
            tracing::debug!("UL inactivity timeout on ts={} but no active transmitting call found", ts);
            return;
        };

        let call = self.active_calls.get_mut(&call_id).unwrap();
        tracing::warn!("UL inactivity timeout on ts={}, forcing TX ceased for call_id={}", ts, call_id);

        let dest_gssi = call.dest_gssi;
        call.tx_active = false;
        call.hangtime_start = Some(self.dltime);

        // Send D-TX CEASED via FACCH to all group members
        self.send_d_tx_ceased_facch(queue, call_id, dest_gssi, ts);

        // Notify UMAC to enter hangtime signalling mode
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Cmce,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
        });

        // Notify Brew to stop forwarding audio
        if net_brew::is_brew_gssi_routable(&self.config, dest_gssi) {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Cmce,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id, ts }),
            });
        }
    }
}
