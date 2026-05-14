use crate::net_control::ControlEndpoint;
use crate::net_telemetry::channel::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait, net_brew};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, TdmaTime, TetraAddress, assert_warn, unimplemented_log};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::{SapMsg, SapMsgInner};

use crate::mm::components::client_state::{ClientMgrErr, MmClientMgr, MmClientState};
use crate::mm::components::not_supported::make_ul_mm_pdu_function_not_supported;
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_ul::MmPduTypeUl;
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::enums::status_downlink::StatusDownlink;
use tetra_pdus::mm::enums::status_uplink::StatusUplink;
use tetra_pdus::mm::fields::energy_saving_information::EnergySavingInformation;
use tetra_pdus::mm::fields::group_identity_attachment::GroupIdentityAttachment;
use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use tetra_pdus::mm::fields::group_identity_location_accept::GroupIdentityLocationAccept;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity_acknowledgement::DAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_location_update_command::DLocationUpdateCommand;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::mm::pdus::u_mm_status::UMmStatus;
use tetra_pdus::mm::pdus::u_tei_provide::UTeiProvide;

pub struct MmBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    client_mgr: MmClientMgr,
}

impl MmBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let client_mgr = MmClientMgr::new(telemetry.clone());
        Self {
            config,
            telemetry,
            control,
            client_mgr,
        }
    }

    fn emit_subscriber_update(&self, queue: &mut MessageQueue, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
        // If brew is active, forward subscriber updates to the Brew entity.
        // Register/Deregister must always be sent for brew-routable ISSIs,
        // even when there are no group affiliations yet. The Brew worker
        // decides whether to send REGISTER or REREGISTER based on its own state.
        // Affiliate/Deaffiliate only sent when there are brew-routable groups.
        if net_brew::is_active(&self.config) {
            let brew_groups = groups
                .iter()
                .filter(|gssi| net_brew::is_brew_gssi_routable(&self.config, **gssi))
                .copied()
                .collect::<Vec<u32>>();
            let should_send = match action {
                BrewSubscriberAction::Register | BrewSubscriberAction::Deregister => net_brew::is_brew_issi_routable(&self.config, issi),
                BrewSubscriberAction::Affiliate | BrewSubscriberAction::Deaffiliate => !brew_groups.is_empty(),
            };
            if should_send {
                let brew_update = MmSubscriberUpdate {
                    issi,
                    groups: brew_groups,
                    action,
                };
                let msg = SapMsg {
                    sap: Sap::Control,
                    src: TetraEntity::Mm,
                    dest: TetraEntity::Brew,
                    msg: SapMsgInner::MmSubscriberUpdate(brew_update),
                };
                queue.push_back(msg);
            }
        }

        // Always emit an update to the Cmce entity
        let mm_update = MmSubscriberUpdate { issi, groups, action };
        let msg = SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(mm_update),
        };
        queue.push_back(msg);
    }

    fn rx_u_itsi_detach(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_itsi_detach");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let pdu = match UItsiDetach::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UItsiDetach: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_itsi_detach(&pdu) {
            tracing::error!("Unsupported critical features in UItsiDetach");
            return;
        }

        let ssi = prim.received_address.ssi;
        let detached_client = self.client_mgr.remove_client(ssi);
        if let Some(client) = detached_client {
            self.config.state_write().subscribers.deregister(ssi);
            if !client.groups.is_empty() {
                let groups: Vec<u32> = client.groups.iter().copied().collect();
                self.emit_subscriber_update(_queue, ssi, groups, BrewSubscriberAction::Deaffiliate);
            }
            self.emit_subscriber_update(_queue, ssi, Vec::new(), BrewSubscriberAction::Deregister);
        } else {
            tracing::warn!("Received UItsiDetach for unknown client with SSI: {}", ssi);
            // return;
        };
    }

    fn rx_u_location_update_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_location_update_demand");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let pdu = match ULocationUpdateDemand::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing ULocationUpdateDemand: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        // Migration not supported: ETSI 16.4.1.1 case b) requires identity exchange via
        // D-LOCATION-UPDATE-PROCEEDING which we don't implement. Reject with cause
        // "Migration not supported" (12, Table 16.81) so the MS can act on it.
        if pdu.location_update_type == LocationUpdateType::MigratingLocationUpdating
            || pdu.location_update_type == LocationUpdateType::ServiceRestorationMigratingLocationUpdating
        {
            // Terminal wants to migrate to another network (e.g. SmartConnect).
            // We don't implement D-LOCATION-UPDATE-PROCEEDING identity exchange (ETSI §16.4.1.1 case b),
            // so we can't accept migration formally. But we MUST release the terminal from Brew
            // so the destination network can register it without identity conflict.
            // Send REJECT so terminal knows to try the other network, but first deregister from Brew.
            let issi = prim.received_address.ssi;
            tracing::info!("MM: ISSI {} migrating to another network — releasing from Brew", issi);
            let detached = self.client_mgr.remove_client(issi);
            if let Some(client) = detached {
                self.config.state_write().subscribers.deregister(issi);
                if !client.groups.is_empty() {
                    let groups: Vec<u32> = client.groups.iter().copied().collect();
                    self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                }
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
            }
            Self::send_d_location_update_reject(
                queue,
                issi,
                prim.handle,
                pdu.location_update_type,
                pdu.address_extension,
            );
            return;
        }

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_location_update_demand(&pdu) {
            tracing::error!("Unsupported critical features in ULocationUpdateDemand");
            return;
        }

        // Handle Energy Saving Mode request (clause 23.7.6).
        // We honour the mode requested by the MS (capped at Eg3 for safety).
        // LLC retransmissions ensure DL messages are delivered even when the MS is sleeping:
        // the BS retransmits on the next monitoring window automatically.
        // frame_number and multiframe_number are derived from ISSI to spread MSs evenly
        // across monitoring slots and avoid simultaneous wake-ups.
        // Per clause 16.7.1 NOTE 1: "The BS may allocate a different energy saving mode
        // than requested and the BS assumes that the allocated value will be used."
        // For DemandLocationUpdating (response to D-LOCATION-UPDATE-COMMAND), the terminal
        // often omits energy_saving_mode from the PDU. In that case, reuse the previously
        // stored ESM — client_mgr retains it because we no longer remove_client at T351 expiry.
        // Preserve energy saving mode across re-registrations.
        // If the terminal omits ESM from the PDU (common after T351 expiry),
        // reuse the previously granted mode so the terminal stays in EE mode.
        // We no longer filter out StayAlive — if that's what was granted before, keep it.
        let prior_esm = self.client_mgr.get_client_by_issi(prim.received_address.ssi)
            .map(|c| c.energy_saving_mode);
        let effective_esm_request = pdu.energy_saving_mode.or(prior_esm);

        let esi = if let Some(esm) = effective_esm_request {
            // Cap at Eg3 (~3s max delay) to avoid excessive call setup latency
            let granted_esm = match esm {
                EnergySavingMode::StayAlive => EnergySavingMode::StayAlive,
                EnergySavingMode::Eg1 => EnergySavingMode::Eg1,
                EnergySavingMode::Eg2 => EnergySavingMode::Eg2,
                EnergySavingMode::Eg3 => EnergySavingMode::Eg3,
                // Cap Eg4-Eg7 to Eg3
                _ => EnergySavingMode::Eg3,
            };

            if granted_esm != esm {
                tracing::debug!("MS {} requested {:?}, capping to {:?}", prim.received_address.ssi, esm, granted_esm);
            }

            let (frame_number, multiframe_number) = if granted_esm == EnergySavingMode::StayAlive {
                (None, None)
            } else {
                // Spread MSs evenly: frame 0-17, multiframe offset within Eg cycle
                let cycle_len = granted_esm as u8 + 1; // Eg1=2, Eg2=3, Eg3=4
                // TETRA frames are 1-indexed (1..18); use 1..=18 to avoid frame 0
                let frame_num = ((prim.received_address.ssi % 18) + 1) as u8;
                let mframe_num = ((prim.received_address.ssi / 18) % cycle_len as u32) as u8;
                tracing::info!(
                    "MS {} granted {:?}: monitoring frame={} multiframe={}",
                    prim.received_address.ssi, granted_esm, frame_num, mframe_num
                );
                (Some(frame_num), Some(mframe_num))
            };

            Some(EnergySavingInformation {
                energy_saving_mode: granted_esm,
                frame_number,
                multiframe_number,
            })
        } else {
            None
        };

        // Try to register the client
        let issi = prim.received_address.ssi;
        let handle = prim.handle;

        // ISSI whitelist check — reject if whitelist is non-empty and ISSI not in it
        if !self.config.config().security.is_issi_allowed(issi) {
            tracing::warn!("MM: ISSI {} not in whitelist, rejecting registration", issi);
            Self::send_d_location_update_reject(
                queue,
                issi,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
            );
            return;
        }

        let was_pending = self.client_mgr.is_pending_command(issi);
        let is_new = !self.client_mgr.client_is_known(issi);
        if !is_new {
            // MS is re-registering while already known. Three cases:
            //
            // A) RoamingLocationUpdating — MS re-registered from scratch (RF loss / reboot /
            //    power-cycle, no prior U-ITSI-DETACH). Clean up stale state so CMCE releases
            //    any ghost calls and group_listeners stays accurate.
            //
            // B) PeriodicLocationUpdating — healthy MS renewing its T351 timer. No cleanup.
            //
            // C) DemandLocationUpdating — MS responding to our D-LOCATION-UPDATE-COMMAND.
            //    This is the second message in the normal registration flow; the first message
            //    already registered+affiliated the MS. Do NOT clean up here.
            let needs_cleanup = pdu.location_update_type == LocationUpdateType::RoamingLocationUpdating
                || pdu.location_update_type == LocationUpdateType::ServiceRestorationRoamingLocationUpdating;

            // needs_cleanup: Roaming = MS rebooted, need CMCE reset
            // was_pending: T351 expired, we already sent Deregister to Brew — just re-register
            if needs_cleanup {
                let old_groups: Vec<u32> = self.client_mgr
                    .get_client_by_issi(issi)
                    .map(|c| c.groups.iter().copied().collect())
                    .unwrap_or_default();
                if !old_groups.is_empty() {
                    self.emit_subscriber_update(queue, issi, old_groups, BrewSubscriberAction::Deaffiliate);
                }
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
            } else if was_pending {
                // T351 re-registration: Brew already got Deregister — just re-register
                // CMCE gets a fresh affiliate when groups are processed below
                tracing::info!("MM: ISSI {} re-registered after T351 COMMAND", issi);
            }
            // Always reset the registration timer on any re-registration
            self.client_mgr.reset_registration_timer(issi);
        }
        // Determine if we need to emit Register toward Brew.
        // We do this when:
        //   A) Terminal is genuinely new (never seen before).
        //   B) Terminal is known but re-attaching via ItsiAttach — migrated from another network.
        //   C) Terminal is known but had pending_command_sent=true — T351 expired, we sent COMMAND
        //      and deregistered from Brew. Now terminal is back, re-register.
        let is_itsi_attach = pdu.location_update_type == LocationUpdateType::ItsiAttach;
        let needs_brew_register = is_new || (!is_new && is_itsi_attach) || (!is_new && was_pending);

        if is_new {
            match self.client_mgr.try_register_client(issi, true) {
                Ok(_) => {
                    self.config.state_write().subscribers.register(issi);
                }
                Err(e) => {
                    tracing::warn!("Failed registering roaming MS {}: {:?}", issi, e);
                    return;
                }
            }
        } else if let Err(e) = self.client_mgr.set_client_state(issi, MmClientState::Attached) {
            tracing::warn!("Failed updating roaming MS {}: {:?}", issi, e);
            return;
        }
        if needs_brew_register {
            if !is_new {
                tracing::info!("MM: ISSI {} re-attaching via ItsiAttach (returned from another network) — re-registering in Brew", issi);
            }
            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
        }

        // Always update the last known L2 handle so we can send downlink PDUs later
        // (e.g. D-LOCATION-UPDATE-COMMAND after Brew reconnection).
        self.client_mgr.set_client_handle(issi, handle);

        // Store energy saving mode and monitoring window in client state
        let esm = esi.as_ref().map(|e| e.energy_saving_mode).unwrap_or(EnergySavingMode::StayAlive);
        let _ = self.client_mgr.set_client_energy_saving_mode(issi, esm);
        let mf = esi.as_ref().and_then(|e| e.frame_number);
        let mmf = esi.as_ref().and_then(|e| e.multiframe_number);
        let _ = self.client_mgr.set_client_monitoring_window(issi, mf, mmf);


        // Process optional GroupIdentityLocationDemand field
        let _has_groups = pdu.group_identity_location_demand.is_some();
        let gila = if let Some(gild) = pdu.group_identity_location_demand {
            // ETSI Table 16.49 (clause 16.10.17): mode=1 means "detach all currently
            // attached group identities and attach group identities defined in the
            // group identity uplink element."
            if gild.group_identity_attach_detach_mode == 1 {
                let prior_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|client| client.groups.iter().copied().collect())
                    .unwrap_or_default();
                if let Err(e) = self.client_mgr.client_detach_all_groups(issi) {
                    tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
                } else if !prior_groups.is_empty() {
                    {
                        let mut state = self.config.state_write();
                        for &gssi in &prior_groups {
                            state.subscribers.deaffiliate(issi, gssi);
                        }
                    }
                    self.emit_subscriber_update(queue, issi, prior_groups, BrewSubscriberAction::Deaffiliate);
                }
            }

            // Try to attach to requested groups, then build GroupIdentityLocationAccept element
            let accepted_groups = if let Some(giu) = &gild.group_identity_uplink {
                Some(self.try_attach_detach_groups(queue, issi, &giu))
            } else {
                None
            };
            let gila = GroupIdentityLocationAccept {
                group_identity_accept_reject: 0, // Accept
                group_identity_downlink: accepted_groups,
            };

            Some(gila)
        } else {
            // No GroupIdentityLocationAccept element present
            None
        };

        // Store and log class_of_ms
        if let Some(ref class) = pdu.class_of_ms {
            tracing::info!("MS {} class_of_ms: {}", issi, class);
        }
        // Per ETSI EN 300 392-2 clause 16.9.4: if the MS signals clch_needed=true or
        // common_scch=true, the BS must populate scch_information_and_distribution_on_18th_frame
        // so the MS knows which timeslots carry SCCH on frame 18.
        // Without this, MS with scan list active stays in scan mode and blocks PTT.
        // Value 0x01: 1 SCCH on frame 18, assigned to TS1 (our MCCH/control channel).
        // Bits: b1-b2 = 01 (1 SCCH), b3-b6 = 0000 (TS2/3/4 not used as SCCH).
        let scch_info = pdu.class_of_ms.as_ref().and_then(|c| {
            if c.clch_needed || c.common_scch {
                Some(0x01u64)
            } else {
                None
            }
        });

        let _ = self.client_mgr.set_client_class_of_ms(issi, pdu.class_of_ms);

        // Reset periodic registration timer on every successful registration.
        self.client_mgr.reset_registration_timer(issi);

        // Use PeriodicLocationUpdating accept type when periodic registration is enabled.
        // This signals to the MS that it must re-register within the configured interval.
        let periodic_secs = self.config.config().cell.periodic_registration_secs;
        let accept_type = if periodic_secs > 0 {
            LocationUpdateType::PeriodicLocationUpdating
        } else {
            pdu.location_update_type
        };

        // Build D-LOCATION UPDATE ACCEPT pdu
        let pdu_response = DLocationUpdateAccept {
            location_update_accept_type: accept_type,
            ssi: Some(issi as u64),
            address_extension: None,
            subscriber_class: None,
            energy_saving_information: esi,
            scch_information_and_distribution_on_18th_frame: scch_info,
            new_registered_area: None,
            security_downlink: None,
            group_identity_location_accept: gila,
            default_group_attachment_lifetime: None,
            authentication_downlink: None,
            group_identity_security_related_information: None,
            cell_type_control: None,
            proprietary: None,
        };

        // Convert pdu to bits
        let pdu_len = 4 + 3 + 24 + 1 + 1 + 1; // Minimal lenght; may expand beyond this. 
        let mut sdu = BitBuffer::new_autoexpand(pdu_len);
        pdu_response.to_bitbuf(&mut sdu).unwrap(); // we want to know when this happens
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu_response, sdu.dump_bin());

        // Build and submit response prim
        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: prim.handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);

        // Send D-LOCATION-UPDATE-COMMAND only on genuine first attach (ItsiAttach or Demand),
        // NOT on RoamingLocationUpdating. Motorola terminals (MTM800, MXP600) respond to
        // D-LOCATION-UPDATE-COMMAND with another RoamingLocationUpdating instead of
        // DemandLocationUpdating, which triggers needs_cleanup=true → Deregister → is_new=true
        // again, creating an infinite registration loop.
        let is_roaming = pdu.location_update_type == LocationUpdateType::RoamingLocationUpdating
            || pdu.location_update_type == LocationUpdateType::ServiceRestorationRoamingLocationUpdating;
        if is_new && !is_roaming {
            tracing::info!("Sending D-LOCATION UPDATE COMMAND to MS {} to prompt TEI and group report", issi);
            Self::send_d_location_update_command(queue, issi, handle);
        }
    }

    fn rx_u_mm_status(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_mm_status");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let pdu = match UMmStatus::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UMmStatus: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let issi = prim.received_address.ssi;
        let handle = prim.handle;

        let mut handled = false;
        match pdu.status_uplink {
            StatusUplink::ChangeOfEnergySavingModeRequest => {
                // Parse energy saving mode from the sub-PDU payload
                let esm = if let Some(dep_info) = pdu.status_uplink_dependent_information {
                    // First 3 bits of the dependent information contain the energy saving mode
                    let dep_len = pdu.status_uplink_dependent_information_len.unwrap_or(0);
                    if dep_len >= 3 {
                        let mode_val = dep_info >> (dep_len - 3);
                        EnergySavingMode::try_from(mode_val).unwrap_or(EnergySavingMode::StayAlive)
                    } else {
                        EnergySavingMode::StayAlive
                    }
                } else {
                    EnergySavingMode::StayAlive
                };

                if esm != EnergySavingMode::StayAlive {
                    tracing::info!(
                        "MS {} requested energy saving mode change to {:?}, overriding to StayAlive",
                        issi,
                        esm
                    );
                } else {
                    tracing::info!("MS {} energy saving mode change request: StayAlive", issi);
                }

                // Store StayAlive (see clause 16.7.1 NOTE 1)
                let _ = self.client_mgr.set_client_energy_saving_mode(issi, EnergySavingMode::StayAlive);

                // Respond with StayAlive
                let esi = EnergySavingInformation {
                    energy_saving_mode: EnergySavingMode::StayAlive,
                    frame_number: None,
                    multiframe_number: None,
                };
                Self::send_d_mm_status_energy_saving(queue, issi, handle, esi);
                handled = true;
            }
            StatusUplink::ChangeOfEnergySavingModeResponse => {
                // MS confirming a BS-initiated change
                let esm = if let Some(dep_info) = pdu.status_uplink_dependent_information {
                    let dep_len = pdu.status_uplink_dependent_information_len.unwrap_or(0);
                    if dep_len >= 3 {
                        let mode_val = dep_info >> (dep_len - 3);
                        EnergySavingMode::try_from(mode_val).unwrap_or(EnergySavingMode::StayAlive)
                    } else {
                        EnergySavingMode::StayAlive
                    }
                } else {
                    EnergySavingMode::StayAlive
                };

                tracing::info!("MS {} energy saving mode change response: {:?}", issi, esm);
                let _ = self.client_mgr.set_client_energy_saving_mode(issi, esm);
                handled = true;
            }
            StatusUplink::DualWatchModeRequest
            | StatusUplink::TerminatingDualWatchModeRequest
            | StatusUplink::ChangeOfDualWatchModeResponse
            | StatusUplink::StartOfDirectModeOperation
            | StatusUplink::MsFrequencyBandsInformation
            | StatusUplink::RequestToStartDmGatewayOperation
            | StatusUplink::RequestToContinuedmGatewayOperation
            | StatusUplink::RequestToStopDmGatewayOperation
            | StatusUplink::RequestToAddDmMsAddresses
            | StatusUplink::RequestToRemoveDmMsAddresses
            | StatusUplink::RequestToReplaceDmMsAddresses
            | StatusUplink::AcceptanceToRemovalOfDmMsAddresses
            | StatusUplink::AcceptanceToChangeRegistrationLabel
            | StatusUplink::AcceptanceToStopDmGatewayOperation => {
                unimplemented_log!("{:?}", pdu.status_uplink)
            }
            _ => {
                assert_warn!(false, "Unrecognized UMmStatus type {:?}", pdu.status_uplink);
            }
        }

        if !handled {
            // A fairly untested, best-effort way of sending a PDU not supported error back
            // Note that an MS is not required to really do anything with this message.
            let (sapmsg, debug_str) = make_ul_mm_pdu_function_not_supported(
                handle,
                MmPduTypeUl::UMmStatus,
                Some((6, pdu.status_uplink.into())),
                prim.received_address,
            );
            tracing::debug!("-> {}", debug_str);
            queue.push_back(sapmsg);
        }
    }

    fn rx_u_attach_detach_group_identity(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_attach_detach_group_identity");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let issi = prim.received_address.ssi;

        let pdu = match UAttachDetachGroupIdentity::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UAttachDetachGroupIdentity: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_attach_detach_group_identity(&pdu) {
            // group_identity_uplink missing — terminal is sending a group report response
            // without requesting any group changes. Send ACK with current groups so
            // terminal knows it's affiliated and can use PTT.
            tracing::info!("UAttachDetachGroupIdentity from {} has no uplink groups — sending ACK with current groups", issi);
            let current_groups: Vec<u32> = self.client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            self.send_d_attach_detach_ack(queue, issi, prim.handle, &current_groups);
            return;
        }

        // If group_identity_attach_detach_mode == 1, we first detach all groups
        if pdu.group_identity_attach_detach_mode == true {
            if !self.client_mgr.client_is_known(issi) {
                // Client unknown (e.g. never registered via location update).
                // Re-register so group attachment can proceed.
                match self.client_mgr.try_register_client(issi, true) {
                    Ok(_) => {
                        self.config.state_write().subscribers.register(issi);
                        self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
                    }
                    Err(e) => {
                        // ETSI EN 300 392-2 §16.3.4: if MS cannot be registered,
                        // send D-ATTACH-DETACH-GROUP-IDENTITY-ACKNOWLEDGEMENT with reject.
                        tracing::warn!("Failed re-registering MS {} on group attach: {:?} — sending reject", issi, e);
                        self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
                        return;
                    }
                }
            } else {
                // Client is known — detach all existing groups first
                let prior_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|client| client.groups.iter().copied().collect())
                    .unwrap_or_default();
                match self.client_mgr.client_detach_all_groups(issi) {
                    Ok(_) => {
                        if !prior_groups.is_empty() {
                            {
                                let mut state = self.config.state_write();
                                for &gssi in &prior_groups {
                                    state.subscribers.deaffiliate(issi, gssi);
                                }
                            }
                            self.emit_subscriber_update(queue, issi, prior_groups, BrewSubscriberAction::Deaffiliate);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
                        return;
                    }
                }
            }
        }

        // Try to attach to requested groups, and retrieve list of accepted GroupIdentityDownlink elements
        // We can unwrap since we did compat check earlier
        let accepted_gid = self.try_attach_detach_groups(queue, issi, &pdu.group_identity_uplink.unwrap());

        // Build reply PDU
        let pdu_response = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 0, // Accept
            reserved: false,                 // TODO FIXME Guessed proper value of reserved field
            proprietary: None,
            group_identity_downlink: Some(accepted_gid),
            group_identity_security_related_information: None,
        };

        // Write to PDU
        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu_response.to_bitbuf(&mut sdu).unwrap(); // We want to know when this happens
        sdu.seek(0);
        tracing::debug!("-> {:?} sdu {}", pdu_response, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: prim.handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn rx_lmm_mle_unitdata_ind(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        // unimplemented_log!("rx_lmm_mle_unitdata_ind for MM component");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
                tracing::error!("BUG: unexpected message or state -- routing error"); return;
            };

        let Some(bits) = prim.sdu.peek_bits(4) else {
            tracing::warn!("insufficient bits: {}", prim.sdu.dump_bin());
            return;
        };

        let Ok(pdu_type) = MmPduTypeUl::try_from(bits) else {
            tracing::warn!("invalid pdu type: {} in {}", bits, prim.sdu.dump_bin());
            return;
        };

        match pdu_type {
            MmPduTypeUl::UAuthentication => unimplemented_log!("UAuthentication"),
            MmPduTypeUl::UItsiDetach => self.rx_u_itsi_detach(queue, message),
            MmPduTypeUl::ULocationUpdateDemand => self.rx_u_location_update_demand(queue, message),
            MmPduTypeUl::UMmStatus => self.rx_u_mm_status(queue, message),
            MmPduTypeUl::UCkChangeResult => unimplemented_log!("UCkChangeResult"),
            MmPduTypeUl::UOtar => unimplemented_log!("UOtar"),
            MmPduTypeUl::UInformationProvide => unimplemented_log!("UInformationProvide"),
            MmPduTypeUl::UAttachDetachGroupIdentity => self.rx_u_attach_detach_group_identity(queue, message),
            MmPduTypeUl::UAttachDetachGroupIdentityAcknowledgement => unimplemented_log!("UAttachDetachGroupIdentityAcknowledgement"),
            MmPduTypeUl::UTeiProvide => self.rx_u_tei_provide(queue, message),
            MmPduTypeUl::UDisableStatus => unimplemented_log!("UDisableStatus"),
            MmPduTypeUl::MmPduFunctionNotSupported => unimplemented_log!("MmPduFunctionNotSupported"),
        };
    }

    fn try_attach_detach_groups(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        giu_vec: &Vec<GroupIdentityUplink>,
    ) -> Vec<GroupIdentityDownlink> {
        let mut accepted_groups = Vec::new();
        let mut aff_groups = Vec::new();
        let mut deaff_groups = Vec::new();

        for giu in giu_vec.iter() {
            if giu.gssi.is_none() || giu.vgssi.is_some() || giu.address_extension.is_some() {
                unimplemented_log!("Only support GroupIdentityUplink with address_type 0");
                continue;
            }

            let gssi = giu.gssi.unwrap(); // can't fail
            let is_detach = giu.group_identity_detachment_uplink.is_some();

            if is_detach {
                match self.client_mgr.client_group_attach(issi, gssi, false) {
                    Ok(changed) => {
                        if changed {
                            self.config.state_write().subscribers.deaffiliate(issi, gssi);
                            deaff_groups.push(gssi);
                        }
                        let gid = GroupIdentityDownlink {
                            group_identity_attachment: None,
                            group_identity_detachment_uplink: giu.group_identity_detachment_uplink,
                            gssi: Some(gssi),
                            address_extension: None,
                            vgssi: None,
                        };
                        accepted_groups.push(gid);
                    }
                    Err(ClientMgrErr::ClientNotFound { .. }) => {
                        tracing::debug!("Group detach for ISSI {} gssi={} skipped: client no longer registered", issi, gssi);
                    }
                    Err(e) => {
                        tracing::warn!("Failed detaching MS {} from group {}: {:?}", issi, gssi, e);
                    }
                }
            } else {
                match self.client_mgr.client_group_attach(issi, gssi, true) {
                    Ok(changed) => {
                        if changed {
                            self.config.state_write().subscribers.affiliate(issi, gssi);
                            aff_groups.push(gssi);
                        }
                        // We have added the client to this group. Add an entry to the downlink response
                        let gid = GroupIdentityDownlink {
                            group_identity_attachment: Some(GroupIdentityAttachment {
                                group_identity_attachment_lifetime: 1, // re-attach after ITSI attach (ETSI default per clause 16.4.2)
                                class_of_usage: giu.class_of_usage.unwrap_or(0),
                            }),
                            group_identity_detachment_uplink: None,
                            gssi: Some(gssi),
                            address_extension: None,
                            vgssi: None,
                        };
                        accepted_groups.push(gid);
                    }
                    Err(ClientMgrErr::ClientNotFound { .. }) => {
                        // Terminal was removed (T351 second expiry) while PDU was in flight — ignore.
                        tracing::debug!("Group attach for ISSI {} gssi={} skipped: client no longer registered", issi, gssi);
                    }
                    Err(e) => {
                        tracing::warn!("Failed attaching MS {} to group {}: {:?}", issi, gssi, e);
                    }
                }
            }
        }

        if !aff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, aff_groups, BrewSubscriberAction::Affiliate);
        }
        if !deaff_groups.is_empty() {
            self.emit_subscriber_update(queue, issi, deaff_groups, BrewSubscriberAction::Deaffiliate);
        }

        // Emit a single snapshot of all current groups so the dashboard always has
        // the full list (not just incremental add/remove events).
        let _sink = self.client_mgr.telemetry_sink().cloned();
        let all_groups: Vec<u32> = self.client_mgr
            .get_client_by_issi(issi)
            .map(|c| c.groups.iter().copied().collect())
            .unwrap_or_default();
        if let Some(sink) = _sink {
            sink.send(crate::net_telemetry::TelemetryEvent::MsGroupsSnapshot { issi, gssis: all_groups });
        }

        accepted_groups
    }

    /// Sends a D-LOCATION UPDATE COMMAND to force the radio to re-register
    /// with full group identity report
    /// Send D-ATTACH-DETACH-GROUP-IDENTITY-ACKNOWLEDGEMENT with reject.
    /// ETSI EN 300 392-2 §16.3.4: used when MS is not registered.
    fn send_d_attach_detach_ack_reject(&self, queue: &mut MessageQueue, issi: u32, handle: u32) {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 1, // 1 = reject per ETSI §14.8.7
            reserved: false,
            proprietary: None,
            group_identity_downlink: None,
            group_identity_security_related_information: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> DAttachDetachGroupIdentityAcknowledgement (reject) to ISSI {}", issi);
        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn send_d_attach_detach_ack(&self, queue: &mut MessageQueue, issi: u32, handle: u32, groups: &[u32]) {
        use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
        use tetra_pdus::mm::fields::group_identity_attachment::GroupIdentityAttachment;
        let gid: Vec<GroupIdentityDownlink> = groups.iter().map(|&gssi| GroupIdentityDownlink {
            group_identity_attachment: Some(GroupIdentityAttachment {
                group_identity_attachment_lifetime: 1,
                class_of_usage: 4,
            }),
            group_identity_detachment_uplink: None,
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        }).collect();
        let ack = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 0,
            reserved: false,
            proprietary: None,
            group_identity_downlink: if gid.is_empty() { None } else { Some(gid) },
            group_identity_security_related_information: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(32);
        if ack.to_bitbuf(&mut sdu).is_ok() {
            sdu.seek(0);
            tracing::debug!("-> DAttachDetachGroupIdentityAcknowledgement (ack-only) sdu {}", sdu.dump_bin());
            queue.push_back(SapMsg {
                sap: Sap::LmmSap,
                src: TetraEntity::Mm,
                dest: TetraEntity::Mle,
                msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                    sdu,
                    handle,
                    address: TetraAddress::issi(issi),
                    layer2service: Layer2Service::Acknowledged,
                    stealing_permission: false,
                    stealing_repeats_flag: false,
                    encryption_flag: false,
                    is_null_pdu: false,
                    tx_reporter: None,
                }),
            });
        }
    }

    fn send_d_location_update_command(queue: &mut MessageQueue, issi: u32, handle: u32) {
        let pdu = DLocationUpdateCommand {
            group_identity_report: true,
            cipher_control: false,
            ciphering_parameters: None,
            address_extension: None,
            cell_type_control: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> DLocationUpdateCommand sdu {}", sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Sends a D-LOCATION UPDATE REJECT PDU (ETSI clause 16.9.2.9)
    fn send_d_location_update_reject(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
    ) {
        Self::send_d_location_update_reject_cause(
            queue, issi, handle, location_update_type,
            address_extension, RejectCause::MigrationNotSupported,
        )
    }

    fn send_d_location_update_reject_cause(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        reject_cause: RejectCause,
    ) {
        let pdu = DLocationUpdateReject {
            location_update_type,
            reject_cause: reject_cause as u8,
            cipher_control: false,
            ciphering_parameters: None,
            address_extension,
            cell_type_control: None,
            proprietary: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    /// Sends a D-MM-STATUS with ChangeOfEnergySavingModeResponse
    fn send_d_mm_status_energy_saving(queue: &mut MessageQueue, issi: u32, handle: u32, esi: EnergySavingInformation) {
        let pdu = DMmStatus {
            status_downlink: StatusDownlink::ChangeOfEnergySavingModeResponse,
            energy_saving_information: Some(esi),
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        pdu.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {} sdu {}", pdu, sdu.dump_bin());

        let msg = SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address: TetraAddress::issi(issi),
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        };
        queue.push_back(msg);
    }

    fn feature_check_u_itsi_detach(pdu: &UItsiDetach) -> bool {
        let supported = true;
        if pdu.address_extension.is_some() {
            unimplemented_log!("Unsupported address_extension present");
        };
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
        };
        supported
    }

    fn rx_u_tei_provide(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_tei_provide");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error"); return;
        };

        let pdu = match UTeiProvide::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::debug!("<- {:?}", pdu);
                pdu
            }
            Err(e) => {
                tracing::warn!("Failed parsing UTeiProvide: {:?} {}", e, prim.sdu.dump_bin());
                return;
            }
        };

        let issi = prim.received_address.ssi;
        tracing::info!(
            "MM: TEI received from ISSI {} → TEI={} ({:060b})",
            issi,
            pdu.tei_hex(),
            pdu.tei,
        );

        // Store TEI in client state for future use (e.g. whitelist checking)
        if let Err(e) = self.client_mgr.set_client_tei(issi, pdu.tei) {
            tracing::warn!("MM: failed to store TEI for ISSI {}: {:?}", issi, e);
        }
    }

    fn feature_check_u_location_update_demand(pdu: &ULocationUpdateDemand) -> bool {
        let mut supported = true;
        if pdu.location_update_type == LocationUpdateType::MigratingLocationUpdating
            || pdu.location_update_type == LocationUpdateType::DisabledMsUpdating
        {
            unimplemented_log!("Unsupported {}", pdu.location_update_type);
            supported = false;
        }
        if pdu.request_to_append_la == true {
            unimplemented_log!("Unsupported request_to_append_la == true");
            supported = false;
        }
        if pdu.cipher_control == true {
            unimplemented_log!("Unsupported cipher_control == true");
            supported = false;
        }
        if pdu.ciphering_parameters.is_some() {
            unimplemented_log!("Unsupported ciphering_parameters present");
            supported = false;
        }
        if pdu.la_information.is_some() {
            unimplemented_log!("Unsupported la_information present");
        }
        if pdu.ssi.is_some() {
            tracing::debug!("DemandLocationUpdating: ssi present (expected from radio, ignored)");
        }
        if pdu.address_extension.is_some() {
            tracing::debug!("DemandLocationUpdating: address_extension present (expected from radio, ignored)");
        }
        if pdu.group_report_response.is_some() {
            tracing::debug!("DemandLocationUpdating: group_report_response present (expected from radio, ignored)");
        }
        if pdu.authentication_uplink.is_some() {
            unimplemented_log!("Unsupported authentication_uplink present");
        }
        if pdu.extended_capabilities.is_some() {
            unimplemented_log!("Unsupported extended_capabilities present");
        }
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
        }

        supported
    }

    /// Check for unsupported features in U-ATTACH/DETACH GROUP IDENTITY
    /// Returns false if a critical feature is missing
    fn feature_check_u_attach_detach_group_identity(pdu: &UAttachDetachGroupIdentity) -> bool {
        let mut supported = true;
        if pdu.group_identity_report == true {
            unimplemented_log!("Unsupported group_identity_report == true");
        }
        if pdu.group_identity_uplink.is_none() {
            unimplemented_log!("Missing group_identity_uplink");
            supported = false;
        }
        if pdu.group_report_response.is_some() {
            tracing::debug!("UAttachDetachGroupIdentity: group_report_response present (expected from radio, ignored)");
        }
        if pdu.proprietary.is_some() {
            unimplemented_log!("Unsupported proprietary present");
        }

        supported
    }
}

impl TetraEntityTrait for MmBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mm
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
        if let Some(cep) = &self.control {
            while let Some(cmd) = cep.try_recv() {
                match cmd {
                    _ => {
                        tracing::warn!("MM: ignoring unsupported control command {:?}", cmd);
                    }
                }
            }
        }

        // Periodic registration expiry check (T351 equivalent, ETSI EN 300 392-2 §16.9).
        // Uses wall-clock time — no TDMA precision needed.
        let interval_secs = self.config.config().cell.periodic_registration_secs;
        let expired = self.client_mgr.collect_expired_registrations(interval_secs);
        for issi in expired {
            tracing::info!(
                "MM: ISSI {} periodic registration expired ({}s) — sending D-LOCATION-UPDATE-COMMAND",
                issi, interval_secs
            );
            // Send D-LOCATION-UPDATE-COMMAND to prompt re-registration.
            //
            // Analysis of real traffic (MTM800/MXP600/MTM5400) shows these terminals
            // have their own T351 timer either disabled or set much longer than the BS.
            // They rely entirely on BS initiative to re-register.
            //
            // - REJECT(ExpiryOfTimer): terminals enter waiting state, never re-attach. BAD.
            // - Silent removal: terminals never notice, never re-register. BAD.
            // - D-LOCATION-UPDATE-COMMAND: terminals respond with U-LOCATION-UPDATING-DEMAND
            //   (DemandLocationUpdating), BS re-registers them immediately. GOOD.
            //
            // The Roaming loop bug from before is NOT triggered here because:
            // 1. This command is sent once per expiry, not on every registration.
            // 2. The fix in rx_u_location_updating_demand already skips sending
            //    COMMAND after RoamingLocationUpdating.
            let already_sent = self.client_mgr.is_pending_command(issi);
            if already_sent {
                // Second expiry — terminal didn't respond to COMMAND within grace period.
                // Remove silently: terminal is powered off or out of coverage.
                tracing::info!(
                    "MM: ISSI {} did not respond to D-LOCATION-UPDATE-COMMAND — removing",
                    issi
                );
                let detached = self.client_mgr.remove_client(issi);
                if let Some(client) = detached {
                    self.config.state_write().subscribers.deregister(issi);
                    if !client.groups.is_empty() {
                        let groups: Vec<u32> = client.groups.iter().copied().collect();
                        self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                    }
                    self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                }
                continue;
            }
            // First expiry — send COMMAND and wait grace period (60s) for response.
            // Do NOT remove_client here: keeping the client in registry preserves ESM
            // and group state so the terminal re-registers cleanly without losing EE mode.
            // Only notify Brew so it stops routing calls to this terminal until it re-registers.
            Self::send_d_location_update_command(queue, issi, 0);
            self.client_mgr.set_pending_command(issi, 60);
            let groups: Vec<u32> = self.client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            if !groups.is_empty() {
                self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
            }
            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
            // Mark as detached in state but keep in client_mgr (preserves ESM + groups)
            self.config.state_write().subscribers.deregister(issi);
        }
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::LmmSap => {
                match message.msg {
                    SapMsgInner::LmmMleUnitdataInd(_) => {
                        self.rx_lmm_mle_unitdata_ind(queue, message);
                    }
                    _ => {
                        tracing::error!("BUG: unexpected message or state -- routing error"); return;
                    }
                }
            }
            Sap::Control => {
                match message.msg {
                    SapMsgInner::BrewReconnected => {
                        self.rx_brew_reconnected(queue);
                    }
                    SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => {
                        self.client_mgr.update_client_rssi(issi, rssi_dbfs);
                        // Emit RSSI telemetry for dashboard
                        if let Some(sink) = &self.telemetry {
                            sink.send(crate::net_telemetry::TelemetryEvent::MsRssi { issi, rssi_dbfs });
                        }
                    }
                    SapMsgInner::MmSubscriberUpdate(update) => {
                        // CMCE can ask MM to deregister an MS (e.g. kick from dashboard)
                        if update.action == BrewSubscriberAction::Deregister {
                            let issi = update.issi;
                            tracing::info!("MM: kicking ISSI {} — sending D-LOCATION-UPDATE-COMMAND to force re-registration", issi);
                            // D-LOCATION-UPDATE-COMMAND forces the terminal to immediately
                            // send a new U-LOCATION-UPDATING-DEMAND, effectively re-registering.
                            // This is cleaner than a reject: the terminal stays on the network
                            // but goes through a full re-registration cycle.
                            Self::send_d_location_update_command(queue, issi, 0);
                            let groups: Vec<u32> = self.client_mgr
                                .get_client_by_issi(issi)
                                .map(|c| c.groups.iter().copied().collect())
                                .unwrap_or_default();
                            if !groups.is_empty() {
                                self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                            }
                            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                            self.client_mgr.remove_client(issi);
                            self.config.state_write().subscribers.deregister(issi);
                        }
                    }
                    _ => {
                        tracing::warn!("mm_bs: unexpected Control message from {:?}", message.src);
                    }
                }
            }
            _ => {
                tracing::warn!("MM: unexpected SAP {:?}, ignoring", message.sap);
            }
        }
    }
}

impl MmBs {
    /// Called when Brew backhaul reconnects. Sends D-LOCATION-UPDATE-COMMAND to all
    /// locally registered MS to force them to re-affiliate. This fixes the PTT-denied
    /// symptom where MS units registered before a Brew disconnect never re-register.
    fn rx_brew_reconnected(&mut self, queue: &mut MessageQueue) {
        let clients: Vec<(u32, u32)> = self.client_mgr.all_clients_with_handle().collect();
        if clients.is_empty() {
            tracing::info!("mm_bs: BrewReconnected — no registered MS to re-register");
            return;
        }
        tracing::info!(
            "mm_bs: BrewReconnected — sending D-LOCATION-UPDATE-COMMAND to {} MS unit(s)",
            clients.len()
        );
        for (issi, handle) in clients {
            tracing::debug!("mm_bs: re-registering ISSI {} (handle={})", issi, handle);
            Self::send_d_location_update_command(queue, issi, handle);
        }
    }
}
