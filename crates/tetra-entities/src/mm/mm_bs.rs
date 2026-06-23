use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use crate::mm::components::recovery_cache::{RecoveryCache, TerminalRecord};
use crate::net_control::{ControlCommand, ControlEndpoint};
use crate::net_telemetry::channel::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait, net_brew};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, TdmaTime, TetraAddress, unimplemented_log};
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
use tetra_pdus::mm::pdus::d_attach_detach_group_identity::DAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity_acknowledgement::DAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_location_update_command::DLocationUpdateCommand;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity_acknowledgement::UAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::mm::pdus::u_mm_status::UMmStatus;
use tetra_pdus::mm::pdus::u_tei_provide::UTeiProvide;

pub struct MmBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    client_mgr: MmClientMgr,

    // ── Restart recovery ──────────────────────────────────────────────────────
    /// On-disk cache of known terminals. `Some` only after `init_recovery` runs (i.e. when the
    /// `[recovery]` section is enabled); `None` means recovery is off and all the hooks below are
    /// no-ops.
    recovery: Option<RecoveryCache>,
    /// ISSIs loaded from the cache still awaiting re-registration, replayed round-robin.
    recovery_pending: VecDeque<u32>,
    /// Per-ISSI count of D-LOCATION-UPDATE-COMMANDs sent during the startup sweep.
    recovery_attempts: HashMap<u32, u32>,
    /// Monotonic frame index of the last replay batch, so we emit at most `replay_per_frame`
    /// COMMANDs per TDMA frame rather than on every tick.
    recovery_last_frame: Option<i32>,
    /// Reactive recovery: per-ISSI timestamp of the last D-LOCATION-UPDATE-COMMAND keyed in
    /// response to an *unknown* radio transmitting on the uplink. Rate-limits re-keying the same
    /// ghost while it re-registers (see `maybe_reactive_recovery`). Independent of `recovery`
    /// above — populated even when the proactive cache is disabled.
    reactive_recovery_cooldown: HashMap<u32, std::time::Instant>,
}

/// Safety cap on `reactive_recovery_cooldown` so a churn of distinct unknown ISSIs can't grow it
/// without bound; lapsed entries are pruned once this many are held.
const REACTIVE_RECOVERY_COOLDOWN_CAP: usize = 4096;

impl MmBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let client_mgr = MmClientMgr::new(telemetry.clone());
        Self {
            config,
            telemetry,
            control,
            client_mgr,
            recovery: None,
            recovery_pending: VecDeque::new(),
            recovery_attempts: HashMap::new(),
            recovery_last_frame: None,
            reactive_recovery_cooldown: HashMap::new(),
        }
    }

    /// Initialise restart recovery from the resolved cache path. Called once at startup from the
    /// binary, only when `[recovery] enabled = true`. Loads the persisted terminals, restores them
    /// into the client registry as "known but Detached" (so the coverage-return re-affiliation can
    /// fire when they answer), and seeds the replay queue. Emits no SAP messages — terminals are
    /// re-affiliated to CMCE/Brew only when they actually re-register. Honours the current ISSI
    /// whitelist and the optional `[recovery] issi_allowlist`.
    pub fn init_recovery(&mut self, cache_path: PathBuf) {
        let rec_cfg = self.config.config().recovery.clone();
        let debounce = std::time::Duration::from_secs(rec_cfg.debounce_secs);
        let cache = RecoveryCache::new(cache_path, debounce);
        let records = cache.load();

        let mut restored = 0usize;
        let mut skipped = 0usize;
        for rec in records.into_iter().take(rec_cfg.max_cached_issis as usize) {
            // Honour both the access-control whitelist and the optional recovery allowlist.
            let whitelisted = self.config.config().security.is_issi_allowed(rec.issi);
            let in_allowlist = rec_cfg.issi_allowlist.is_empty() || rec_cfg.issi_allowlist.contains(&rec.issi);
            if !whitelisted || !in_allowlist {
                skipped += 1;
                continue;
            }
            let esm = EnergySavingMode::try_from(rec.energy_saving_mode as u64).unwrap_or(EnergySavingMode::StayAlive);
            self.client_mgr.restore_client(rec.issi, &rec.groups, esm);
            self.recovery_pending.push_back(rec.issi);
            self.recovery_attempts.insert(rec.issi, 0);
            restored += 1;
        }

        tracing::info!(
            "MM: restart recovery initialised — {} terminal(s) restored from cache ({} skipped by whitelist/allowlist); replaying D-LOCATION-UPDATE-COMMAND",
            restored,
            skipped
        );
        self.recovery = Some(cache);
    }

    /// Mark the recovery cache dirty (a flush is debounced from tick_start). No-op when recovery
    /// is disabled.
    fn recovery_mark_dirty(&mut self) {
        if let Some(cache) = &mut self.recovery {
            cache.mark_dirty();
        }
    }

    /// Stop replaying to an ISSI that has (re-)registered. Called from the location-update path.
    fn recovery_confirm(&mut self, issi: u32) {
        // Clear any reactive-recovery cooldown first: the radio answered, so a future re-drop of
        // this ISSI should be re-keyable immediately. Done before the proactive early-return so it
        // runs even when the boot-time cache replay is disabled (reactive is independent).
        self.reactive_recovery_cooldown.remove(&issi);
        if self.recovery.is_none() {
            return;
        }
        let was_pending = self.recovery_attempts.remove(&issi).is_some();
        self.recovery_pending.retain(|&i| i != issi);
        if was_pending {
            tracing::info!("MM: restart recovery — ISSI {} re-registered, stopping replay", issi);
        }
    }

    /// Per-tick startup replay: emit up to `replay_per_frame` D-LOCATION-UPDATE-COMMANDs per TDMA
    /// frame to terminals still awaiting re-registration, round-robin, giving up on a terminal
    /// after `max_replay_attempts` (e.g. one powered off mid-outage). Goes inert when the queue
    /// drains.
    fn drive_recovery_replay(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        if self.recovery.is_none() || self.recovery_pending.is_empty() {
            return;
        }
        // Monotonic frame index — emit at most one batch per frame.
        let frame = ts.to_int() / 4;
        if self.recovery_last_frame == Some(frame) {
            return;
        }
        self.recovery_last_frame = Some(frame);

        let rec_cfg = self.config.config().recovery.clone();
        // Emit at most `replay_per_frame` COMMANDs per frame, but never re-key the same radio
        // twice in one frame: bound the pops to the number of distinct pending entries, so a
        // queue shorter than replay_per_frame doesn't double-send (which would also burn that
        // ISSI's attempt budget faster than configured).
        let budget = (rec_cfg.replay_per_frame as usize).min(self.recovery_pending.len());
        let mut processed = 0usize;
        while processed < budget {
            let Some(issi) = self.recovery_pending.pop_front() else {
                break;
            };
            processed += 1;
            // Already confirmed (no longer tracked) — drop it from the queue.
            let Some(&attempts) = self.recovery_attempts.get(&issi) else {
                continue;
            };
            if attempts >= rec_cfg.max_replay_attempts {
                tracing::info!(
                    "MM: restart recovery — giving up on ISSI {} after {} unanswered COMMANDs",
                    issi,
                    attempts
                );
                self.recovery_attempts.remove(&issi);
                continue;
            }
            // handle = 0: addressed by ISSI on the MCCH (see send_d_location_update_command).
            Self::send_d_location_update_command(queue, issi, 0);
            self.recovery_attempts.insert(issi, attempts + 1);
            self.recovery_pending.push_back(issi); // round-robin until it answers or we give up
        }
    }

    /// Debounced flush of the recovery cache. Takes the cache out of `self` so the snapshot
    /// closure can borrow `self.client_mgr` without a borrow conflict, then restores it.
    fn recovery_maybe_flush(&mut self) {
        let Some(mut cache) = self.recovery.take() else {
            return;
        };
        cache.maybe_flush(|| {
            self.client_mgr
                .snapshot_for_recovery()
                .into_iter()
                .map(|(issi, groups, esm)| TerminalRecord {
                    issi,
                    groups,
                    energy_saving_mode: esm.into_raw() as u8,
                })
                .collect()
        });
        self.recovery = Some(cache);
    }

    /// Reactive restart recovery. Called on every uplink RSSI sample (`MsRssiUpdate`), i.e. for
    /// every random-access / PTT / SDS burst MM sees. If the transmitting ISSI is *unknown* to the
    /// client registry — the tell-tale of a radio still RF-camped on the cell but whose MM record
    /// was lost to a restart — key it a single D-LOCATION-UPDATE-COMMAND (ETSI EN 300 392-2
    /// §16.4.4) to force an immediate re-registration with a group report. The existing
    /// location-update path then re-affiliates it to CMCE/Brew and reports it to the dashboard, so
    /// the next PTT succeeds — no manual DMO/TMO toggle, no wait for the periodic T351.
    ///
    /// This is the catch-all companion to the proactive boot sweep (`drive_recovery_replay`): it
    /// needs no persisted cache, fires only in response to a demonstrably present + active radio,
    /// and so covers the cases the cache misses (radio absent from the cache, or the sweep already
    /// gave up on it). Rate-limited per ISSI; gated by the access whitelist and the optional
    /// `[recovery] issi_allowlist`; on by default (`[recovery] reactive_enabled`).
    fn maybe_reactive_recovery(&mut self, queue: &mut MessageQueue, issi: u32) {
        // Fast path, the overwhelming common case: a radio MM already knows needs no recovery.
        // Checked before touching config so healthy traffic stays cheap.
        if self.client_mgr.client_is_known(issi) {
            return;
        }
        // Let the proactive boot sweep own any ISSI it is already replaying, to avoid double-keying.
        if self.recovery_attempts.contains_key(&issi) {
            return;
        }

        let cfg = self.config.config();
        let rec = &cfg.recovery;
        if !rec.reactive_enabled {
            return;
        }
        // Never key a radio that isn't permitted on this network — same scoping as init_recovery:
        // the access-control whitelist plus the optional recovery allowlist.
        let permitted = cfg.security.is_issi_allowed(issi) && (rec.issi_allowlist.is_empty() || rec.issi_allowlist.contains(&issi));
        if !permitted {
            return;
        }
        let cooldown = std::time::Duration::from_secs(rec.reactive_cooldown_secs);

        let now = std::time::Instant::now();
        if let Some(&last) = self.reactive_recovery_cooldown.get(&issi) {
            if now.duration_since(last) < cooldown {
                // Already commanded recently — give it time to answer rather than spamming COMMANDs
                // across the burst of RSSI samples a single PTT produces.
                return;
            }
        }
        // Prune lapsed entries before growing the map past its safety cap.
        if self.reactive_recovery_cooldown.len() >= REACTIVE_RECOVERY_COOLDOWN_CAP {
            self.reactive_recovery_cooldown.retain(|_, t| now.duration_since(*t) < cooldown);
        }
        self.reactive_recovery_cooldown.insert(issi, now);

        tracing::info!(
            "MM: reactive recovery — unknown ISSI {} active on uplink, sending D-LOCATION-UPDATE-COMMAND to force re-registration",
            issi
        );
        // handle = 0: addressed by ISSI on the MCCH (see send_d_location_update_command).
        Self::send_d_location_update_command(queue, issi, 0);
    }

    /// Force CMCE to release any individual P2P calls involving the given ISSI,
    /// without touching Brew affiliations. Used on soft re-attach (e.g. MTP3550
    /// 2s RF dropout) to prevent "PTT denied" caused by stale call state in CMCE.
    ///
    /// Implementation: sends Deregister to CMCE only (not Brew), then re-sends
    /// Register + Affiliate so subscriber_groups and group_listener counts are
    /// restored. Brew is not informed because the MS is still considered registered.
    fn emit_individual_call_release_for_issi(&mut self, queue: &mut MessageQueue, issi: u32) {
        let groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|c| c.groups.iter().copied().collect())
            .unwrap_or_default();

        // CMCE Deregister: releases individual_calls + drops group_listener counts
        let dereg = MmSubscriberUpdate {
            issi,
            groups: Vec::new(),
            action: BrewSubscriberAction::Deregister,
        };
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(dereg),
        });

        // CMCE Register: re-introduces the ISSI as known
        let reg = MmSubscriberUpdate {
            issi,
            groups: Vec::new(),
            action: BrewSubscriberAction::Register,
        };
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(reg),
        });

        // CMCE Affiliate: restores group_listener counts so group calls still route
        if !groups.is_empty() {
            let aff = MmSubscriberUpdate {
                issi,
                groups,
                action: BrewSubscriberAction::Affiliate,
            };
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mm,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::MmSubscriberUpdate(aff),
            });
        }

        tracing::info!("MM: forced individual call release for ISSI {} (soft re-attach)", issi);
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
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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
        self.recovery_mark_dirty();
    }

    fn rx_u_location_update_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_location_update_demand");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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

        // The terminal answered with a location update — stop the restart-recovery replay to it
        // regardless of how this update is handled below (migration reject, whitelist reject, or
        // normal registration). Hoisted above all early-returns so a migrating/rejected terminal
        // isn't replayed to forever. No-op when recovery is disabled / ISSI not pending.
        self.recovery_confirm(prim.received_address.ssi);

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
            self.recovery_mark_dirty();
            Self::send_d_location_update_reject(queue, issi, prim.handle, pdu.location_update_type, pdu.address_extension);
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
        let prior_esm = self
            .client_mgr
            .get_client_by_issi(prim.received_address.ssi)
            .map(|c| c.energy_saving_mode);
        let effective_esm_request = pdu.energy_saving_mode.or(prior_esm);

        let esi = effective_esm_request.map(|esm| Self::grant_energy_saving(prim.received_address.ssi, esm));

        // Try to register the client
        let issi = prim.received_address.ssi;
        let handle = prim.handle;

        // ISSI whitelist check — reject if whitelist is non-empty and ISSI not in it.
        // The dashboard can override the config whitelist at runtime (state override takes
        // precedence so edits apply without a restart); fall back to the config value when
        // no override is set. An empty list (in either place) means "open network".
        let issi_allowed = {
            let state = self.config.state_read();
            match &state.issi_whitelist_override {
                Some(list) => list.is_empty() || list.contains(&issi),
                None => self.config.config().security.is_issi_allowed(issi),
            }
        };
        if !issi_allowed {
            tracing::warn!("MM: ISSI {} not in whitelist, rejecting registration", issi);
            Self::send_d_location_update_reject(queue, issi, handle, pdu.location_update_type, pdu.address_extension);
            return;
        }

        // Restart recovery: this terminal answered (re-registered), so stop replaying
        // D-LOCATION-UPDATE-COMMANDs to it. The coverage-return re-affiliation block below then
        // restores its CMCE/Brew group state. No-op when recovery is disabled / ISSI not pending.
        self.recovery_confirm(issi);

        let was_pending = self.client_mgr.is_pending_command(issi);
        let is_new = !self.client_mgr.client_is_known(issi);
        // A client still known to client_mgr but absent from the subscriber registry was dropped by
        // a T351 confirmed-gone expiry (we keep the client to preserve its groups). On its return
        // we must re-add it to the registry + dashboard + Brew, or it stays invisible and SDS to it
        // is misrouted as non-local. Sampled before the coverage-return re-affiliation below, whose
        // affiliate() would otherwise recreate the registry entry and mask the drop.
        let was_dropped = !is_new && !self.config.state_read().subscribers.is_registered(issi);
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
            let needs_cleanup = if pdu.location_update_type == LocationUpdateType::RoamingLocationUpdating
                || pdu.location_update_type == LocationUpdateType::ServiceRestorationRoamingLocationUpdating
            {
                // Some terminals (e.g. Sepura) send RoamingLocationUpdating after every PTT
                // release, not just on power-cycle or RF loss. If we treat this as a full reboot
                // and do deregister→register, CMCE has a brief window where it doesn't know the
                // terminal — a PTT press in that window gets "no listeners" and the terminal
                // interprets it as a network error and fully disconnects.
                //
                // Heuristic: treat RoamingLocationUpdating as a soft re-attach (no cleanup) if
                // the terminal registered less than 120 seconds ago.
                let recently_registered = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|c| c.last_registration_time.elapsed().as_secs() < 120)
                    .unwrap_or(false);
                if recently_registered {
                    tracing::debug!(
                        "MM: ISSI {} RoamingLocationUpdating within 120s of last register — treating as soft re-attach (Sepura post-PTT)",
                        issi
                    );
                    // Even on soft re-attach, force CMCE to release any individual P2P calls
                    // involving this ISSI. Terminals (e.g. Motorola MTP3550) that drop RF for
                    // 2s and re-attach lose call state but BS keeps the call alive — next PTT
                    // is rejected ("PTT denied") because the terminal doesn't recognize the call_id
                    // in our D-TX-GRANTED. Releasing the individual call here forces a clean U-SETUP
                    // on the next PTT.
                    self.emit_individual_call_release_for_issi(queue, issi);
                    false
                } else {
                    true
                }
            } else {
                false
            };

            // needs_cleanup: Roaming = MS rebooted, need full CMCE/Brew reset (deregister+register).
            // was_pending: the MS is answering our T351 COMMAND — Brew still holds the subscriber
            // (teardown is deferred to the confirmed-gone second expiry, which this answer prevents),
            // so no Brew action is needed; CMCE is re-affiliated via the coverage-return path below.
            if needs_cleanup {
                let old_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|c| c.groups.iter().copied().collect())
                    .unwrap_or_default();
                if !old_groups.is_empty() {
                    self.emit_subscriber_update(queue, issi, old_groups.clone(), BrewSubscriberAction::Deaffiliate);
                }
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Register);
                // The Deregister above wipes this subscriber's group affiliations in CMCE/Brew, but
                // client_mgr still holds them. A roaming MS with persistent attachment
                // (attachment_lifetime=0) re-reports its groups with group_identity_attach_detach_mode=0
                // — which is a no-op in client_mgr (the group is already present), so
                // try_attach_detach_groups emits NO Affiliate and CMCE is left with zero listeners.
                // The next group PTT is then rejected "no listeners" and inbound group calls are dropped
                // (observed for a Motorola MTP on RoamingLocationUpdating). Re-affiliate the stored
                // groups now so CMCE/Brew stay in sync with client_mgr across the reset; the
                // group-identity processing below then only needs to add genuinely new groups.
                if !old_groups.is_empty() {
                    {
                        let mut state = self.config.state_write();
                        for &gssi in &old_groups {
                            state.subscribers.affiliate(issi, gssi);
                        }
                    }
                    self.emit_subscriber_update(queue, issi, old_groups, BrewSubscriberAction::Affiliate);
                }
            } else if was_pending {
                tracing::info!("MM: ISSI {} re-registered after T351 COMMAND (Brew already holds it)", issi);
            }
            // Always reset the registration timer on any re-registration
            self.client_mgr.reset_registration_timer(issi);
        }
        // Determine if we need to emit Register toward Brew. Only when Brew was actually torn down
        // (or never had this ISSI):
        //   A) Terminal is genuinely new (never seen before).
        //   B) Terminal is known but re-attaching via ItsiAttach — migrated from another network.
        //   C) Terminal was dropped from Brew at a T351 confirmed-gone (second) expiry and is now
        //      back (was_dropped); the re-add path above already restored it locally.
        // A plain T351 re-registration (was_pending) is deliberately NOT here: teardown no longer
        // happens at first expiry, so Brew still holds the subscriber — re-registering would be a
        // needless REGISTER every interval (the Brew flap this avoids).
        let is_itsi_attach = pdu.location_update_type == LocationUpdateType::ItsiAttach;
        let needs_brew_register = is_new || (!is_new && is_itsi_attach) || was_dropped;

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
        // Re-add a radio that had been dropped by a T351 confirmed-gone expiry. The client never
        // left client_mgr (PTT keeps working), but the subscriber registry and the dashboard were
        // torn down at the drop; restore both so SDS routing and the dashboard reflect reality.
        // register() resets attached_groups — the group-identity processing / coverage-return
        // re-affiliation below rebuilds them, so this must run before either.
        if was_dropped {
            self.config.state_write().subscribers.register(issi);
            if let Some(sink) = &self.telemetry {
                sink.send(crate::net_telemetry::TelemetryEvent::MsRegistration { issi });
            }
            tracing::info!(
                "MM: ISSI {} re-appeared after T351 drop — re-registered in dashboard + subscriber registry",
                issi
            );
        }
        if needs_brew_register {
            if !is_new && is_itsi_attach {
                tracing::info!(
                    "MM: ISSI {} re-attaching via ItsiAttach (returned from another network) — re-registering in Brew",
                    issi
                );
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

        // Coverage-return re-affiliation (fixes "PTT no longer works after leaving and
        // returning to coverage", workaround = DMO→TMO).
        //
        // Sequence that breaks PTT:
        //   1. MS affiliates to a GSSI → CMCE group_listeners[gssi] += 1. PTT works.
        //   2. MS leaves coverage; BS T351 expires and emits Deregister to CMCE, which
        //      does dec_group_listener() → the GSSI now has 0 listeners.
        //   3. MS returns. Because we hand out attachment_lifetime=0 (persistent), the MS
        //      believes it is still affiliated and sends a plain location update WITHOUT a
        //      group identity report.
        //   4. MM re-registers the MS but never re-affiliates the groups → CMCE still has
        //      0 listeners for the GSSI → the next PTT is rejected with "no listeners"
        //      ("please wait" on the radio). DMO→TMO forces an ItsiAttach with a full group
        //      report, which is why that clears it.
        //
        // Fix: when a *known* MS re-registers without supplying a group report, but we
        // still hold groups for it in client_mgr, re-emit Affiliate for those groups so
        // CMCE's group_listeners (and Brew) are resynced with what the MS believes.
        if !is_new && !_has_groups {
            let stored_groups: Vec<u32> = self
                .client_mgr
                .get_client_by_issi(issi)
                .map(|c| c.groups.iter().copied().collect())
                .unwrap_or_default();
            if !stored_groups.is_empty() {
                tracing::info!(
                    "MM: ISSI {} re-registered without group report but has {} stored group(s) {:?} — re-affiliating to resync CMCE/Brew (coverage-return fix)",
                    issi,
                    stored_groups.len(),
                    stored_groups
                );
                {
                    let mut state = self.config.state_write();
                    for &gssi in &stored_groups {
                        state.subscribers.affiliate(issi, gssi);
                    }
                }
                self.emit_subscriber_update(queue, issi, stored_groups.clone(), BrewSubscriberAction::Affiliate);
                // Refresh the dashboard's group list for this MS. It may have just been re-added
                // with an empty entry by the T351-drop recovery above, and coverage-return emits no
                // per-group telemetry otherwise, so the radio would show with no groups.
                if let Some(sink) = &self.telemetry {
                    sink.send(crate::net_telemetry::TelemetryEvent::MsGroupsSnapshot {
                        issi,
                        gssis: stored_groups,
                    });
                }
            }
        }

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
        let scch_info = pdu
            .class_of_ms
            .as_ref()
            .and_then(|c| if c.clch_needed || c.common_scch { Some(0x01u64) } else { None });

        let _ = self.client_mgr.set_client_class_of_ms(issi, pdu.class_of_ms);

        // Reset periodic registration timer on every successful registration.
        self.client_mgr.reset_registration_timer(issi);

        // Registration / affiliation / EE state changed — persist for restart recovery (debounced).
        self.recovery_mark_dirty();

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

        // Send D-LOCATION-UPDATE-COMMAND to prompt a full re-registration (TEI + group
        // identity report) ONLY for a genuinely new (unknown) radio that didn't ITSI-attach
        // and didn't already include a group report.
        //
        // This mirrors BlueStation's behaviour and is deliberately narrow:
        //  - A new radio doing RoamingLocationUpdating without groups gets exactly one
        //    COMMAND so it re-registers with its group list.
        //  - A radio we ALREADY know never gets a COMMAND here. This is critical for
        //    receive-only devices like the Motorola TPG2200 pager, which never report any
        //    talkgroups: keying COMMAND at them on every update made them answer with yet
        //    another group-less RoamingLocationUpdating, producing an endless COMMAND loop
        //    and a permanent "Unit Not Attached" that even a kick couldn't clear (regression
        //    fixed here).
        //  - Motorola handsets (MTM800/MXP600) that answer a COMMAND with another
        //    RoamingLocationUpdating are now known on that second update, so they get no
        //    further COMMAND and can't loop.
        let has_groups = _has_groups;
        if is_new && pdu.location_update_type != LocationUpdateType::ItsiAttach && !has_groups {
            tracing::info!("Sending D-LOCATION UPDATE COMMAND to returning MS {} to request group report", issi);
            Self::send_d_location_update_command(queue, issi, handle);
        }
    }

    /// Rebuild StackState.ee_monitoring_windows from the live client registry. See the field doc
    /// in tetra_config StackState and `MmClientMgr::ee_monitoring_windows`.
    fn publish_monitoring_windows(&self) {
        let map: std::collections::HashMap<u32, (u8, u8, u8)> = self
            .client_mgr
            .ee_monitoring_windows()
            .map(|(issi, frame, mframe, cycle_len)| (issi, (frame, mframe, cycle_len)))
            .collect();
        self.config.state_write().ee_monitoring_windows = map;
    }

    /// Decide which energy saving mode to grant an MS and compute its monitoring window.
    ///
    /// Per clause 16.7.1 NOTE 1 the BS may allocate a different mode than requested. We cap at
    /// Eg3 (~3 s max delay) to bound call-setup latency, and for any non-StayAlive grant derive
    /// the wake-up frame/multiframe from the ISSI so MSs are spread across monitoring slots.
    ///
    /// Used both by the initial location update (U-LOCATION-UPDATING-DEMAND) and by mid-session
    /// energy saving toggles (U-MM-STATUS / ChangeOfEnergySavingModeRequest) so the two paths
    /// behave identically.
    fn grant_energy_saving(issi: u32, requested: EnergySavingMode) -> EnergySavingInformation {
        let granted_esm = match requested {
            EnergySavingMode::StayAlive => EnergySavingMode::StayAlive,
            EnergySavingMode::Eg1 => EnergySavingMode::Eg1,
            EnergySavingMode::Eg2 => EnergySavingMode::Eg2,
            EnergySavingMode::Eg3 => EnergySavingMode::Eg3,
            // Cap Eg4-Eg7 to Eg3
            _ => EnergySavingMode::Eg3,
        };

        if granted_esm != requested {
            tracing::debug!("MS {} requested {:?}, capping to {:?}", issi, requested, granted_esm);
        }

        let (frame_number, multiframe_number) = match crate::mm::components::client_state::ee_cycle_frames(granted_esm) {
            None => (None, None), // StayAlive — no monitoring window
            Some(cycle) => {
                // Frame-based start point (ETSI EN 300 392-2 Table 23.9 / clause 23.7.6): the MS
                // wakes for one TDMA frame every `cycle` frames. Spread MSs across the cycle by
                // ISSI so they don't all wake in the same frame. The start point's absolute frame
                // index (m-1)*18+(f-1) must be ≡ phase (mod cycle); anchoring it in multiframe 1
                // yields a valid Frame Number (1..=18) and Multiframe Number (1..=60 — never the
                // StayAlive-reserved 0 the old `(issi/18)%cycle` formula produced).
                let phase = (issi % cycle as u32) as u8; // 0..cycle-1, ≤ 5 for capped Eg1..Eg3
                let frame_num = (phase % 18) + 1; // 1..=18
                let mframe_num = (phase / 18) + 1; // 1..=60 (== 1 for the supported cycles ≤ 6)
                tracing::info!(
                    "MS {} granted {:?}: frame-based cycle={} frames, start frame={} multiframe={}",
                    issi,
                    granted_esm,
                    cycle,
                    frame_num,
                    mframe_num
                );
                (Some(frame_num), Some(mframe_num))
            }
        };

        EnergySavingInformation {
            energy_saving_mode: granted_esm,
            frame_number,
            multiframe_number,
        }
    }

    fn rx_u_mm_status(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_u_mm_status");
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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

                tracing::info!("MS {} requested mid-session energy saving mode change to {:?}", issi, esm);

                // Grant the mode the same way the initial location update does, so toggling
                // energy economy on/off at the radio takes effect mid-session — both for actual
                // paging (monitoring window) and for the dashboard, which mirrors the granted
                // mode via the MsEnergySaving telemetry emitted by set_client_energy_saving_mode.
                // Without this the handler used to force StayAlive, so a re-activation never
                // reached the dashboard until the terminal fully re-registered (power-cycle).
                let esi = Self::grant_energy_saving(issi, esm);
                // If the client was concurrently removed (T351 second-expiry race), the
                // setters return ClientNotFound — log it so the silent no-op is at least
                // visible in the operator log rather than vanishing.
                if let Err(e) = self.client_mgr.set_client_energy_saving_mode(issi, esi.energy_saving_mode) {
                    tracing::debug!("MM: mid-session ESM update on ISSI {} skipped: {:?}", issi, e);
                }
                if let Err(e) = self
                    .client_mgr
                    .set_client_monitoring_window(issi, esi.frame_number, esi.multiframe_number)
                {
                    tracing::debug!("MM: mid-session monitoring window update on ISSI {} skipped: {:?}", issi, e);
                }
                self.recovery_mark_dirty();

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
                self.recovery_mark_dirty();
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
                // Status types we don't handle (e.g. NetworkOrUserSpecific*, reserved
                // values). This is a valid-but-unsupported PDU, not a code bug, so log it
                // as unimplemented rather than asserting — assert_warn made it look like
                // an internal fault in the operator's logs. handled stays false, so we
                // still reply with "function not supported" below.
                unimplemented_log!("Unhandled UMmStatus type {:?}", pdu.status_uplink);
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
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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
            tracing::info!(
                "UAttachDetachGroupIdentity from {} has no uplink groups — sending ACK with current groups",
                issi
            );
            let current_groups: Vec<u32> = self
                .client_mgr
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

        // ETSI EN 300 392-2 §16.9.2.2: the ACK PDU travels in a single TM-SDU
        // and there is no MM-level segmentation. Empirically MXP600 and MTP3550
        // start losing the ACK around 12-15 GroupIdentityDownlink entries — the
        // PDU exceeds what fits in a FACCH/SACCH burst, the MS times out, and on
        // subsequent retries it eventually de-registers ("Unit not attached").
        //
        // We have to cap the request *before* affiliating on the BS side. A previous
        // version of this code affiliated everything and then truncated only the ACK
        // response — that desynced the MS and the BS: the BS thought N groups were
        // active, but the MS only saw confirmations for the first 12. Inbound calls
        // on the un-confirmed groups would deliver to the BS but never notify the MS
        // (FH-BUG-022 reopened, FH-BUG-025). Now the BS only affiliates what it can
        // confirm; the MS will keep re-requesting the remaining groups in subsequent
        // attach cycles per ETSI clause 16.4.3.
        const MAX_GROUPS_PER_ATTACH: usize = 12;
        // feature_check_u_attach_detach_group_identity above guarantees this is Some,
        // but use let-else instead of .unwrap() so a future refactor that loosens that
        // check doesn't crash the MM worker on a malformed PDU.
        let Some(giu) = pdu.group_identity_uplink else {
            tracing::warn!("rx_u_attach_detach_group_identity: group_identity_uplink missing after feature_check; ignoring");
            return;
        };
        let (giu_clamped, dropped) = if giu.len() > MAX_GROUPS_PER_ATTACH {
            tracing::warn!(
                "ISSI {} requested attach/detach for {} groups; capped at {} per ETSI PDU size limit. MS will retry remaining in next cycle.",
                issi,
                giu.len(),
                MAX_GROUPS_PER_ATTACH
            );
            let (head, _tail) = giu.split_at(MAX_GROUPS_PER_ATTACH);
            (head.to_vec(), giu.len() - MAX_GROUPS_PER_ATTACH)
        } else {
            (giu, 0)
        };
        let _ = dropped; // silence unused warning if logging is compiled out

        // Try to attach to requested groups, and retrieve list of accepted GroupIdentityDownlink elements
        let accepted_gid = self.try_attach_detach_groups(queue, issi, &giu_clamped);

        // Group affiliations changed — persist for restart recovery (debounced).
        self.recovery_mark_dirty();

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
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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
            MmPduTypeUl::UAttachDetachGroupIdentityAcknowledgement => self.rx_u_attach_detach_group_identity_ack(queue, message),
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
            // Currently only address_type=0 (plain GSSI) is implemented. Anything else
            // (vgssi, address extension, missing gssi) is unsupported — log and skip.
            let Some(gssi) = giu.gssi else {
                unimplemented_log!("GroupIdentityUplink without gssi field");
                continue;
            };
            if giu.vgssi.is_some() || giu.address_extension.is_some() {
                unimplemented_log!("Only support GroupIdentityUplink with address_type 0");
                continue;
            }

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
                        // We have added the client to this group. Add an entry to the downlink response.
                        //
                        // group_identity_attachment_lifetime values (ETSI EN 300 392-2 §16.10.19):
                        //   0 = Attachment not needed → MS keeps the group attached indefinitely
                        //                                until an explicit detach. This is what we want
                        //                                for scan lists / persistent group attachments.
                        //   1 = Attachment required for the next ITSI attach → MS re-affiliates on next
                        //                                ITSI attach (rare event: reboot, cell reselect).
                        //   2 = Attachment not allowed for next ITSI attach → SwMI denies.
                        //   3 = Attachment required for next location update → MS re-affiliates at every
                        //                                LU (every few minutes), generating churn.
                        //
                        // We previously used 1 with a "good default" comment, but that interacted badly
                        // with Motorola MTP-series radios in scan-list mode: those radios send the scan
                        // list incrementally (2 GSSIs at a time, with one anchor + one new GSSI), and
                        // expect the BS-side affiliation to persist between batches. With lifetime=1 the
                        // MS internally drops the affiliation a few minutes later ("5-minute timer" per
                        // dk5ras), then PTT fails with "Unit not attached" until the user changes GSSI.
                        // Lifetime=0 makes the attachment persistent on the MS side — matching the BS
                        // side which already keeps affiliations across attach cycles — and resolves
                        // FH-BUG-022.
                        let gid = GroupIdentityDownlink {
                            group_identity_attachment: Some(GroupIdentityAttachment {
                                group_identity_attachment_lifetime: 0,
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
        let all_groups: Vec<u32> = self
            .client_mgr
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
        use tetra_pdus::mm::fields::group_identity_attachment::GroupIdentityAttachment;
        use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
        let gid: Vec<GroupIdentityDownlink> = groups
            .iter()
            .map(|&gssi| GroupIdentityDownlink {
                group_identity_attachment: Some(GroupIdentityAttachment {
                    // 0 = Attachment not needed = persistent on MS side. See the
                    // long comment in try_attach_detach_groups for why this
                    // (rather than 1 / "until next ITSI attach") is the correct
                    // choice for scan-list-heavy Motorola radios.
                    group_identity_attachment_lifetime: 0,
                    class_of_usage: 4,
                }),
                group_identity_detachment_uplink: None,
                gssi: Some(gssi),
                address_extension: None,
                vgssi: None,
            })
            .collect();
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

    /// Class of usage advertised in DGNA group attachments. 4 mirrors the value the normal
    /// affiliation ACK path (`send_d_attach_detach_ack`) already sends, so DGNA-assigned groups
    /// behave identically to ones the radio affiliated itself.
    const DGNA_CLASS_OF_USAGE: u8 = 4;

    /// DGNA (Dynamic Group Number Assignment) — BS-initiated group attach/detach for one terminal,
    /// driven from the dashboard. ETSI EN 300 392-2 §16 (SS-DGNA).
    ///
    /// Local-only: updates the BS-side affiliation (so local group calls and group SDS route to the
    /// terminal) and pushes an unsolicited D-ATTACH/DETACH GROUP IDENTITY to the radio so it adds or
    /// removes the group in its own list. Brew is intentionally not involved.
    ///
    /// Returns `true` if the command was accepted and a PDU was sent to the terminal.
    fn do_dgna(&mut self, queue: &mut MessageQueue, issi: u32, gssi: u32, attach: bool) -> bool {
        let verb = if attach { "assign" } else { "deassign" };

        // The terminal must be registered on the cell — we cannot regroup a radio that is not here.
        if !self.client_mgr.client_is_known(issi) {
            tracing::warn!(
                "DGNA: ISSI {} is not registered on this cell — ignoring {} of GSSI {}",
                issi,
                verb,
                gssi
            );
            return false;
        }

        // Apply to the MM client registry. client_group_attach also validates the GSSI is a legal
        // group address (range + is_group + may_attach); an invalid GSSI returns an error here.
        match self.client_mgr.client_group_attach(issi, gssi, attach) {
            Ok(_changed) => {
                // Mirror into the shared subscriber state used for local call/SDS routing and notify
                // CMCE. Done unconditionally on success so DGNA stays authoritative even if the BS
                // believed the affiliation already matched (no desync window — FH-BUG-022/025).
                if attach {
                    self.config.state_write().subscribers.affiliate(issi, gssi);
                    self.emit_subscriber_update(queue, issi, vec![gssi], BrewSubscriberAction::Affiliate);
                } else {
                    self.config.state_write().subscribers.deaffiliate(issi, gssi);
                    self.emit_subscriber_update(queue, issi, vec![gssi], BrewSubscriberAction::Deaffiliate);
                }
            }
            Err(e) => {
                tracing::warn!("DGNA: cannot {} GSSI {} on ISSI {}: {:?}", verb, gssi, issi, e);
                return false;
            }
        }

        // Push the unsolicited D-ATTACH/DETACH GROUP IDENTITY to the terminal.
        self.send_d_attach_detach_group_identity(queue, issi, gssi, attach);

        // Persist for restart recovery (debounced) and refresh the dashboard with the full group set.
        self.recovery_mark_dirty();
        let all_groups: Vec<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|c| c.groups.iter().copied().collect())
            .unwrap_or_default();
        if let Some(sink) = &self.telemetry {
            sink.send(crate::net_telemetry::TelemetryEvent::MsGroupsSnapshot { issi, gssis: all_groups });
        }

        tracing::info!(
            "DGNA: {} GSSI {} {} ISSI {}",
            if attach { "assigned" } else { "deassigned" },
            gssi,
            if attach { "to" } else { "from" },
            issi
        );
        true
    }

    /// Build and queue an unsolicited D-ATTACH/DETACH GROUP IDENTITY for a single GSSI, addressed to
    /// `issi`, requesting an acknowledgement. `attach == true` carries a (persistent) group identity
    /// attachment; `false` carries a detachment. Used by [`Self::do_dgna`].
    fn send_d_attach_detach_group_identity(&self, queue: &mut MessageQueue, issi: u32, gssi: u32, attach: bool) {
        let gid = GroupIdentityDownlink {
            group_identity_attachment: attach.then_some(GroupIdentityAttachment {
                // 0 = "attachment not needed" → persistent on the MS until an explicit detach.
                // Matches the affiliation-ACK path; see try_attach_detach_groups for why lifetime=0
                // (not 1) is correct for scan-list-heavy Motorola radios (FH-BUG-022).
                group_identity_attachment_lifetime: 0,
                class_of_usage: Self::DGNA_CLASS_OF_USAGE,
            }),
            // 2-bit group identity detachment field; 0 = unknown/default. The attach/detach type
            // identifier plus the GSSI are what make the MS drop the group.
            group_identity_detachment_uplink: (!attach).then_some(0u8),
            gssi: Some(gssi),
            address_extension: None,
            vgssi: None,
        };
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: true,
            // false = amend the existing list (do NOT detach everything else first). DGNA touches one
            // group at a time and must leave the radio's other affiliations intact.
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(vec![gid]),
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if pdu.to_bitbuf(&mut sdu).is_err() {
            tracing::error!("DGNA: failed serializing D-ATTACH/DETACH GROUP IDENTITY for ISSI {}", issi);
            return;
        }
        sdu.seek(0);
        tracing::debug!(
            "-> DAttachDetachGroupIdentity (DGNA {}) gssi={} issi={} sdu {}",
            if attach { "attach" } else { "detach" },
            gssi,
            issi,
            sdu.dump_bin()
        );

        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: 0, // unsolicited, BS-initiated — no inbound L2 handle to echo
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

    /// Handle U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT — the terminal's reply to a BS-initiated
    /// D-ATTACH/DETACH GROUP IDENTITY (DGNA). BS-side group state is committed optimistically when the
    /// DGNA is issued, so this is confirmation/telemetry only: log the outcome.
    fn rx_u_attach_detach_group_identity_ack(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let issi = prim.received_address.ssi;
        match UAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => tracing::info!("DGNA: ISSI {} acknowledged group identity change: {:?}", issi, pdu),
            Err(e) => tracing::warn!(
                "DGNA: failed parsing U-ATTACH/DETACH GROUP IDENTITY ACK from {}: {:?} {}",
                issi,
                e,
                prim.sdu.dump_bin()
            ),
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
            queue,
            issi,
            handle,
            location_update_type,
            address_extension,
            RejectCause::MigrationNotSupported,
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
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
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
        tracing::info!("MM: TEI received from ISSI {} → TEI={} ({:060b})", issi, pdu.tei_hex(), pdu.tei,);

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

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        // Drain control commands addressed to the MM entity. We collect into a Vec first so the
        // immutable borrow on `self.control` is released before the handlers run — DGNA needs
        // `&mut self` (client registry, subscriber state, telemetry).
        if self.control.is_some() {
            let mut cmds = Vec::new();
            if let Some(cep) = &self.control {
                while let Some(cmd) = cep.try_recv() {
                    cmds.push(cmd);
                }
            }
            for cmd in cmds {
                match cmd {
                    ControlCommand::Dgna { issi, gssi, attach } => {
                        self.do_dgna(queue, issi, gssi, attach);
                    }
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
            // Restart-recovery interlock: never expire/remove a client the recovery sweep is
            // still replaying to. The sweep owns its lifecycle until it confirms (re-register →
            // recovery_confirm) or gives up (attempt cap). Without this, a restored client at
            // the tail of the replay queue could reach T351 second-expiry and be removed —
            // wiping its stored groups, the exact failure recovery exists to prevent — when
            // periodic_registration_secs is clamped low. recovery_attempts is empty unless
            // recovery is active, so this is a cheap no-op otherwise. It also prevents a
            // double-COMMAND (T351 + recovery) to the same ISSI in one window.
            if self.recovery_attempts.contains_key(&issi) {
                continue;
            }
            tracing::info!(
                "MM: ISSI {} periodic registration expired ({}s) — sending D-LOCATION-UPDATE-COMMAND",
                issi,
                interval_secs
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
                // Second expiry — the terminal didn't answer the first COMMAND within the grace
                // period. We deliberately do NOT remove the client and do NOT send
                // D-LOCATION-UPDATE-REJECT(ExpiryOfTimer):
                //   - removing wipes the terminal's stored groups, so a Motorola that later
                //     re-registers WITHOUT a group report (it still believes it is affiliated —
                //     persistent attachment_lifetime=0) leaves the coverage-return re-affiliation
                //     nothing to restore, and the next group PTT is denied (FH-BUG-031);
                //   - REJECT(ExpiryOfTimer) drops Motorola radios to "no service".
                // Instead, re-attract once more (handle 0 — the L2 handle is inert) and reset the
                // registration clock. The client and its groups stay in the registry; a genuinely
                // gone radio just lingers harmlessly (re-attracted at most once per interval), and
                // when it returns the coverage-return re-affiliation restores its group state so
                // PTT works immediately. (Trade-off: the registry isn't pruned by T351 anymore —
                // acceptable: it's bounded by the fleet and cleared on restart.)
                tracing::info!(
                    "MM: ISSI {} still unresponsive — re-attracting and keeping groups (no REJECT, no removal)",
                    issi
                );
                Self::send_d_location_update_command(queue, issi, 0);
                // Confirmed gone: the terminal ignored the first COMMAND through the whole grace
                // period. NOW tear it down everywhere — Brew backhaul, the dashboard, and the local
                // subscriber registry — but keep it in client_mgr so its groups survive for
                // coverage-return re-affiliation if it ever comes back. The guard makes this fire
                // once, on the registered→gone transition: this branch re-runs every interval for a
                // radio that stays away. The client is NOT removed, so MsDeregistration is never
                // emitted; MsTimeoutDrop is the sole drop signal here.
                // Presence guard (FH-BUG-044 — present stations vanishing from the dashboard at
                // the T351 interval): only tear a radio down if it is genuinely gone. A radio we
                // have heard transmitting within the re-registration interval is demonstrably
                // present even if it never answered the unsolicited COMMAND — e.g. an
                // energy-economy radio that was asleep when the (un-gated) COMMAND was sent on the
                // MCCH. Dropping it here would make a live station disappear from the dashboard
                // every interval. Keep it (the reset_registration_timer below re-arms the clock);
                // only a radio with NO sign of life on the air for a whole interval is torn down.
                let interval = std::time::Duration::from_secs(interval_secs as u64);
                let heard_on_air = self.client_mgr.heard_on_air_within(issi, interval);
                let still_registered = self.config.state_read().subscribers.is_registered(issi);
                if still_registered && heard_on_air {
                    tracing::info!(
                        "MM: ISSI {} ignored the T351 COMMAND but was heard on air within {}s — keeping it (present, not dropping from dashboard)",
                        issi,
                        interval_secs
                    );
                }
                if still_registered && !heard_on_air {
                    // Tell Brew to stop routing calls/SDS to this terminal until it re-registers.
                    // Deferred to here (not first expiry) so a healthy radio that answers the
                    // COMMAND within grace never flaps Brew; only a genuinely-gone radio is torn
                    // down, exactly once.
                    let groups: Vec<u32> = self
                        .client_mgr
                        .get_client_by_issi(issi)
                        .map(|c| c.groups.iter().copied().collect())
                        .unwrap_or_default();
                    if !groups.is_empty() {
                        self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                    }
                    self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                    // Drop from the local subscriber registry + dashboard.
                    self.config.state_write().subscribers.deregister(issi);
                    if let Some(sink) = &self.telemetry {
                        sink.send(crate::net_telemetry::TelemetryEvent::MsTimeoutDrop { issi });
                    }
                }
                self.client_mgr.reset_registration_timer(issi);
                self.recovery_mark_dirty();
                continue;
            }
            // First expiry — send the COMMAND and arm the 60s grace, nothing else. Emit NO teardown
            // (Brew, dashboard, or local registry). The terminal almost always answers within grace
            // and re-registers (MTM800/MXP600/MTM5400 rely on BS initiative — see above), so any
            // teardown here is a flap: it stayed registered on the air (PTT kept working) but, when
            // it answered, the re-registration path took the `is_new == false` branch and never
            // re-added it — so it vanished from the dashboard forever, SDS to it was misrouted as
            // non-local, and Brew was needlessly deregistered+reregistered every interval. All
            // teardown is deferred to the confirmed-gone second expiry above, which fires only if
            // the grace elapses with no answer.
            //
            // Do NOT remove_client here either: keeping the client in registry preserves ESM and
            // group state so the terminal re-registers cleanly without losing EE mode.
            //
            // handle = 0: the L2 handle is inert (MLE addresses downlink MM PDUs by ISSI; see
            // mle_bs.rs rx_lmm_mle_unitdata_req + uplink ind hardcoded handle 0). The COMMAND
            // reaches the camped radio by ISSI regardless. The second-expiry branch above no
            // longer removes the client or sends REJECT, so the terminal's groups survive an
            // unanswered COMMAND and are restored by coverage-return re-affiliation on its return
            // (FH-BUG-031 fix).
            //
            // EE gating (FH-BUG-044 follow-up — present energy-economy radios vanishing from the
            // dashboard): a sleeping EE MS only listens to the downlink during its monitoring
            // window, so an un-gated COMMAND sent while it sleeps is missed; it then never answers
            // and is dropped at the second expiry. Send the COMMAND only when the MS is reachable
            // (StayAlive always; EE on its wake window), retrying on later ticks until the window
            // opens. A bounded fallback (interval + T351_EE_WINDOW_WAIT_SECS) sends it blind so a
            // stale/incorrect window can never suppress the probe forever. The grace clock only
            // starts once the COMMAND is actually sent, so a deferred wake-window send still gets
            // the full grace to answer.
            const T351_EE_WINDOW_WAIT_SECS: u64 = 6;
            if self
                .client_mgr
                .should_send_t351_command_now(issi, ts, interval_secs, T351_EE_WINDOW_WAIT_SECS)
            {
                Self::send_d_location_update_command(queue, issi, 0);
                self.client_mgr.set_pending_command(issi, 60);
            }
        }

        // Restart recovery: replay D-LOCATION-UPDATE-COMMANDs to cached terminals awaiting
        // re-registration (no-op once the sweep drains / when recovery is disabled), then flush
        // the cache if dirty + debounce elapsed.
        self.drive_recovery_replay(queue, ts);
        self.recovery_maybe_flush();

        // Republish the per-MS energy-economy monitoring windows into shared state every tick, from
        // the authoritative client registry, so the downlink scheduler (CMCE/SDS) can gate
        // unsolicited traffic to a sleeping MS's wake window without reading stale data. Rebuilt
        // wholesale (like CMCE's active_call_ts) — empty when no MS is in energy economy.
        self.publish_monitoring_windows();
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        tracing::debug!("rx_prim: {:?}", message);
        // tracing::debug!(ts=%message.dltime, "rx_prim: {:?}", message);

        match message.sap {
            Sap::LmmSap => match message.msg {
                SapMsgInner::LmmMleUnitdataInd(_) => {
                    self.rx_lmm_mle_unitdata_ind(queue, message);
                }
                _ => {
                    tracing::error!("BUG: unexpected message or state -- routing error");
                    return;
                }
            },
            Sap::Control => {
                match message.msg {
                    SapMsgInner::BrewReconnected => {
                        self.rx_brew_reconnected(queue);
                    }
                    SapMsgInner::MsRssiUpdate { issi, rssi_dbfs } => {
                        self.client_mgr.update_client_rssi(issi, rssi_dbfs);
                        // This RSSI sample proves `issi` is RF-present on the uplink right now. If
                        // MM has no record of it (registry lost to a restart while the radio stayed
                        // camped), command it to re-register immediately rather than waiting for its
                        // periodic T351 or a manual DMO/TMO toggle.
                        self.maybe_reactive_recovery(queue, issi);
                        // Emit RSSI telemetry for dashboard
                        if let Some(sink) = &self.telemetry {
                            sink.send(crate::net_telemetry::TelemetryEvent::MsRssi { issi, rssi_dbfs });
                        }
                        // Forward to Brew entity for optional export to Brew server.
                        // BrewEntity applies its own rate limiting and checks feature_rssi_export.
                        queue.push_back(SapMsg {
                            sap: Sap::Control,
                            src: TetraEntity::Mm,
                            dest: TetraEntity::Brew,
                            msg: SapMsgInner::MsRssiUpdate { issi, rssi_dbfs },
                        });
                    }
                    SapMsgInner::MmSubscriberUpdate(update) => {
                        // CMCE can ask MM to deregister an MS (e.g. kick from dashboard)
                        if update.action == BrewSubscriberAction::Deregister {
                            let issi = update.issi;
                            tracing::info!(
                                "MM: kicking ISSI {} — sending D-LOCATION-UPDATE-COMMAND to force re-registration",
                                issi
                            );
                            // D-LOCATION-UPDATE-COMMAND forces the terminal to immediately
                            // send a new U-LOCATION-UPDATING-DEMAND, effectively re-registering.
                            // This is cleaner than a reject: the terminal stays on the network
                            // but goes through a full re-registration cycle.
                            //
                            // handle = 0: the L2 handle is inert in this stack. MLE addresses
                            // downlink MM PDUs purely by ISSI on the connectionless MCCH
                            // (mle_bs.rs: rx_lmm_mle_unitdata_req discards prim.handle), and the
                            // uplink ind hardcodes handle 0 (mle_bs.rs:132), so last_handle is
                            // always 0 anyway. The COMMAND reaches the camped radio by its ISSI.
                            // (NB: this means whatever makes FH-BUG-028 vendor-specific is NOT the
                            // handle — that root cause is still open.)
                            Self::send_d_location_update_command(queue, issi, 0);
                            let groups: Vec<u32> = self
                                .client_mgr
                                .get_client_by_issi(issi)
                                .map(|c| c.groups.iter().copied().collect())
                                .unwrap_or_default();
                            if !groups.is_empty() {
                                self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                            }
                            self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                            self.client_mgr.remove_client(issi);
                            self.config.state_write().subscribers.deregister(issi);
                            self.recovery_mark_dirty();
                        }
                    }
                    SapMsgInner::MmDgnaRequest { issi, gssi, attach } => {
                        // Dashboard-originated DGNA, forwarded by CMCE (the dashboard control channel
                        // terminates there). The group machinery lives here in MM.
                        self.do_dgna(queue, issi, gssi, attach);
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
        let issis = self.client_mgr.all_known_issis();
        if issis.is_empty() {
            tracing::info!("mm_bs: BrewReconnected — no registered MS to re-register");
            return;
        }
        tracing::info!(
            "mm_bs: BrewReconnected — sending D-LOCATION-UPDATE-COMMAND to {} MS unit(s)",
            issis.len()
        );
        for issi in issis {
            // handle = 0: addressed by ISSI on the MCCH (the handle is inert — see
            // all_known_issis). This path was previously dead because it filtered on
            // last_handle != 0, which is never true, so no MS was ever re-registered after a
            // Brew reconnect — the cause of "PTT denied after the backhaul blips".
            tracing::debug!("mm_bs: re-registering ISSI {}", issi);
            Self::send_d_location_update_command(queue, issi, 0);
        }
    }
}

#[cfg(test)]
mod ee_tests {
    use super::*;
    use tetra_core::TdmaTime;

    #[test]
    fn grant_energy_saving_produces_spec_valid_start_point() {
        // ETSI Table 16.40: Frame Number ∈ 1..=18, Multiframe Number ∈ 1..=60 (MN=0 is reserved
        // ONLY for StayAlive). The old (issi/18)%cycle formula produced MN=0 for half the radios —
        // an invalid anchor a conformant radio rejects (FH-BUG-034). Every active grant must now be
        // valid AND its start point must itself be a wake frame in the matching gating window.
        for mode in [
            EnergySavingMode::Eg1,
            EnergySavingMode::Eg2,
            EnergySavingMode::Eg3,
            EnergySavingMode::Eg5,
            EnergySavingMode::Eg7,
        ] {
            for issi in [1u32, 2, 17, 18, 19, 2260596, 2269000, 9_999_999] {
                let esi = MmBs::grant_energy_saving(issi, mode);
                let frame = esi.frame_number.expect("active EE carries a frame number");
                let mframe = esi.multiframe_number.expect("active EE carries a multiframe number");
                assert!((1..=18).contains(&frame), "FN {frame} out of 1..=18 (issi {issi}, {mode:?})");
                assert!((1..=60).contains(&mframe), "MN {mframe} out of 1..=60 (issi {issi}, {mode:?})");
                let cycle = crate::mm::components::client_state::ee_cycle_frames(esi.energy_saving_mode).expect("active EE has a cycle");
                assert!(
                    TdmaTime {
                        h: 0,
                        m: mframe,
                        f: frame,
                        t: 1
                    }
                    .in_ee_monitoring_window(frame, mframe, cycle),
                    "start point ({frame},{mframe}) must be open for cycle {cycle} (issi {issi}, {mode:?})"
                );
            }
        }
    }

    #[test]
    fn grant_energy_saving_stay_alive_and_eg4_7_capping() {
        // StayAlive → no monitoring window.
        let esi = MmBs::grant_energy_saving(42, EnergySavingMode::StayAlive);
        assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
        assert!(esi.frame_number.is_none() && esi.multiframe_number.is_none());
        // Eg4..Eg7 capped to Eg3 to bound call-setup latency.
        for mode in [
            EnergySavingMode::Eg4,
            EnergySavingMode::Eg5,
            EnergySavingMode::Eg6,
            EnergySavingMode::Eg7,
        ] {
            assert_eq!(MmBs::grant_energy_saving(42, mode).energy_saving_mode, EnergySavingMode::Eg3);
        }
        // Eg1..Eg3 granted as requested.
        for mode in [EnergySavingMode::Eg1, EnergySavingMode::Eg2, EnergySavingMode::Eg3] {
            assert_eq!(MmBs::grant_energy_saving(42, mode).energy_saving_mode, mode);
        }
    }
}
