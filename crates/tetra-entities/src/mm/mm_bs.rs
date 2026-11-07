use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use crate::mm::components::recovery_cache::{RecoveryCache, TerminalRecord};
use crate::mm::components::visitors::{GroupForm, Visitors};
use crate::net_control::{ControlCommand, ControlEndpoint};
use crate::net_telemetry::channel::TelemetrySink;
use crate::{MessageQueue, TetraEntityTrait, net_brew};
use tetra_config::bluestation::{Mni, SharedConfig};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, Sap, TdmaTime, TetraAddress, unimplemented_log};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::call_control::CallControl;
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::{SapMsg, SapMsgInner};

use crate::mm::components::client_state::{ClientMgrErr, MmClientMgr, MmClientState};
use crate::mm::components::not_supported::make_ul_mm_pdu_function_not_supported;
use tetra_pdus::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;
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
use tetra_pdus::mm::pdus::d_location_update_proceeding::DLocationUpdateProceeding;
use tetra_pdus::mm::pdus::d_location_update_reject::DLocationUpdateReject;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity_acknowledgement::UAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_pdus::mm::pdus::u_mm_status::UMmStatus;
use tetra_pdus::mm::pdus::u_tei_provide::UTeiProvide;
use std::time::{Duration, Instant};
use tetra_config::bluestation::sec_security::AuthenticationMode;
use tetra_pdus::mm::enums::authentication_reject_reason::AuthenticationRejectReason;
use tetra_pdus::mm::enums::authentication_sub_type::AuthenticationSubType;
use tetra_pdus::mm::enums::otar_sub_type::{OtarRejectReason, OtarSubType, ProvisionResult};
use tetra_pdus::mm::pdus::authentication::{
    DAuthenticationDemand, DAuthenticationReject, DAuthenticationResponse, DAuthenticationResult, UAuthenticationDemand,
    UAuthenticationReject, UAuthenticationResponse, UAuthenticationResult, peek_sub_type,
};
use tetra_pdus::mm::pdus::otar_sck::{self, DOtarSckProvide, DOtarSckReject, OtarSessionKey, SckKeyAndIdentifier, UOtarSckDemand, UOtarSckResult};
use tetra_security::auth::{MsChallenge, answer_ms_challenge, derive_dck, random_nonce};
use tetra_security::keys::AuthKey;
use tetra_security::taa1::{ta41, ta51};

/// Group identity detachment downlink "Temporary 1 detachment" (TS 100 392-2 table 16.52).
const GROUP_DETACHMENT_TEMPORARY_1: u8 = 1;

/// A challenge in flight (EN 300 392-7 clause 4.1.2).
struct PendingAuth {
    challenge: MsChallenge,
    /// L2 handle the challenge went out on.
    handle: u32,
    /// The U-LOCATION UPDATE DEMAND that is waiting for the result, if the challenge was part
    /// of a registration.
    held: Option<SapMsg>,
    started: Instant,
}

struct PendingOtar {
    sckn: u8,
    sck_vn: u16,
    started: Instant,
}

/// T354: how long the infrastructure waits for the radio's authentication response.
const T354: Duration = Duration::from_secs(30);
/// How long a successful authentication stays valid for the held registration to go through.
const AUTH_PASS_GRACE: Duration = Duration::from_secs(5);
/// How long to wait for U-OTAR SCK RESULT after an individually addressed provide.
const T_OTAR_RESULT: Duration = Duration::from_secs(30);

pub struct MmBs {
    config: SharedConfig,
    telemetry: Option<TelemetrySink>,
    control: Option<ControlEndpoint>,
    client_mgr: MmClientMgr,
    registration_checks_rf_ready: bool,
    registration_command_last_frame: Option<i32>,
    registration_check_last_tick: Option<Instant>,
    registration_probe_receipts: HashMap<u32, tetra_core::tx_receipt::TxReporter>,

    // ── Air-interface security (EN 300 392-7) ──────────────────────────────────────────────
    /// Radios we have challenged and are waiting on (U-AUTHENTICATION RESPONSE / RESULT).
    auth_pending: HashMap<u32, PendingAuth>,
    /// Radios that just passed, so the replayed registration is not challenged again.
    auth_passed: HashMap<u32, Instant>,
    /// Derived cipher key per authenticated radio (for a future class 3).
    dck: HashMap<u32, [u8; 10]>,
    /// SCK deliveries awaiting the radio's U-OTAR SCK RESULT.
    otar_pending: HashMap<u32, PendingOtar>,

    // â”€â”€ Restart recovery â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
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
    /// above â€” populated even when the proactive cache is disabled.
    reactive_recovery_cooldown: HashMap<u32, std::time::Instant>,
    /// Per-ISSI timestamp of the last SwMI-initiated group report request (ETSI EN 300 392-2
    /// §16.8.3) after a U-ATTACH overflowed the single-PDU ACK cap. Prevents re-request loops
    /// while the MS is expected to multipacket (detach-all + amendments). Cleared on
    /// group-report-complete or after [`GROUP_REPORT_REQUEST_COOLDOWN`].
    group_report_requested_at: HashMap<u32, std::time::Instant>,
    /// Groups discarded by the ACK-size cap, kept until the MS finishes multipacket attach or
    /// we fall back to a SwMI D-ATTACH of the remainder (MXP600 often re-sends the full list
    /// in a PDU that exceeds the uplink and fails to parse).
    attach_overflow_remainder: HashMap<u32, Vec<GroupIdentityUplink>>,
    /// Radios of other networks registered here by migration, keyed by their (V)ASSI
    /// (Multi Network, clause 16.4.1.1 b)).
    visitors: Visitors,
}

/// How a migrating location update (clause 16.4.1.1 b) or c)) is handled.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Migration {
    /// Multi Network off, `[cell_info] migration` off, no MNI, or a network not accepted here.
    Refused,
    /// Case c): a radio of this network coming back.
    Home,
    /// Case b): a radio of another accepted network.
    Visitor(Mni),
}

/// Safety cap on `reactive_recovery_cooldown` so a churn of distinct unknown ISSIs can't grow it
/// without bound; lapsed entries are pruned once this many are held.
const REACTIVE_RECOVERY_COOLDOWN_CAP: usize = 4096;

/// Do not re-issue a SwMI group report request for the same ISSI within this window while a
/// multipacket attach cycle may still be in flight.
const GROUP_REPORT_REQUEST_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);

impl MmBs {
    pub fn new(config: SharedConfig, telemetry: Option<TelemetrySink>, control: Option<ControlEndpoint>) -> Self {
        let mut client_mgr = MmClientMgr::new(telemetry.clone());
        let cell_id = config.config().cell_id.0;
        {
            let cfg = config.config();
            let sec = &cfg.security;
            client_mgr.set_limits(sec.max_registered_clients, sec.registration_rate_limit_per_min);
            // State the effective posture in the operator log. There is no way to read it off the
            // TOML alone (an empty issi_whitelist means "open" under the legacy default), and the
            // air interface is unauthenticated regardless — so say both, once, unmissably.
            tracing::warn!(
                "MM: access control posture: {} | authentication: {}",
                sec.access_control_posture(),
                sec.authentication_posture()
            );
            tracing::warn!("MM: air interface encryption: {}", crate::aie::posture(&cfg));
            if sec.honour_unauthenticated_detach {
                tracing::warn!(
                    "MM: honouring unauthenticated U-ITSI-DETACH / migration teardown (only from an ISSI registered on the same link) — set [security] honour_unauthenticated_detach = false to refuse it entirely"
                );
            }
        }
        Self {
            config,
            telemetry,
            control,
            client_mgr,
            registration_checks_rf_ready: false,
            registration_command_last_frame: None,
            registration_check_last_tick: None,
            registration_probe_receipts: HashMap::new(),
            auth_pending: HashMap::new(),
            auth_passed: HashMap::new(),
            dck: HashMap::new(),
            otar_pending: HashMap::new(),
            recovery: None,
            recovery_pending: VecDeque::new(),
            recovery_attempts: HashMap::new(),
            recovery_last_frame: None,
            reactive_recovery_cooldown: HashMap::new(),
            group_report_requested_at: HashMap::new(),
            attach_overflow_remainder: HashMap::new(),
            visitors: Visitors::new(cell_id),
        }
    }

    /// Initialise restart recovery from the resolved cache path. Called once at startup from the
    /// binary, only when `[recovery] enabled = true`. Loads the persisted terminals, restores them
    /// into the client registry as "known but Detached" (so the coverage-return re-affiliation can
    /// fire when they answer), and seeds the replay queue. Emits no SAP messages â€” terminals are
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
            {
                let mut state = self.config.state_write();
                for group in rec.dgna_groups {
                    // The recovery cache is on-disk and may be stale or corrupt. Validate each
                    // persisted DGNA group before it re-enters the registry: an out-of-range GSSI
                    // would later panic the 24-bit SS-DGNA PDU serializer, and an out-of-range
                    // attachment mode would be silently coerced to a different mode. Skip bad entries.
                    if group.gssi == 0 || group.gssi > 0xFF_FFFF {
                        tracing::warn!(
                            "MM: skipping restored DGNA group with out-of-range GSSI {} for ISSI {}",
                            group.gssi,
                            rec.issi
                        );
                        continue;
                    }
                    if GroupIdentityAttachmentMode::try_from(group.attachment_mode as u64).is_err() {
                        tracing::warn!(
                            "MM: skipping restored DGNA group GSSI {} for ISSI {} with invalid attachment mode {}",
                            group.gssi,
                            rec.issi,
                            group.attachment_mode
                        );
                        continue;
                    }
                    state
                        .subscribers
                        .remember_dgna_group(rec.issi, group.gssi, group.mnemonic, group.attachment_mode);
                }
            }
            self.recovery_pending.push_back(rec.issi);
            self.recovery_attempts.insert(rec.issi, 0);
            restored += 1;
        }

        tracing::info!(
            "MM: restart recovery initialised â€” {} terminal(s) restored from cache ({} skipped by whitelist/allowlist); replaying D-LOCATION-UPDATE-COMMAND",
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
            tracing::info!("MM: restart recovery â€” ISSI {} re-registered, stopping replay", issi);
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
        // Monotonic frame index â€” emit at most one batch per frame.
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
            // Already confirmed (no longer tracked) â€” drop it from the queue.
            let Some(&attempts) = self.recovery_attempts.get(&issi) else {
                continue;
            };
            if attempts >= rec_cfg.max_replay_attempts {
                tracing::info!(
                    "MM: restart recovery â€” giving up on ISSI {} after {} unanswered COMMANDs",
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
            let state = self.config.state_read();
            self.client_mgr
                .snapshot_for_recovery()
                .into_iter()
                // A (V)ASSI is gone with the process: the radio has to migrate again.
                .filter(|(issi, _, _)| !self.visitors.is_visitor(*issi))
                .map(|(issi, groups, esm)| TerminalRecord {
                    issi,
                    groups,
                    dgna_groups: state.subscribers.dgna_groups_of(issi),
                    energy_saving_mode: esm.into_raw() as u8,
                })
                .collect()
        });
        self.recovery = Some(cache);
    }

    /// Reactive restart recovery. Called on every uplink RSSI sample (`MsRssiUpdate`), i.e. for
    /// every random-access / PTT / SDS burst MM sees. If the transmitting ISSI is *unknown* to the
    /// client registry â€” the tell-tale of a radio still RF-camped on the cell but whose MM record
    /// was lost to a restart â€” key it a single D-LOCATION-UPDATE-COMMAND (ETSI EN 300 392-2
    /// Â§16.4.4) to force an immediate re-registration with a group report. The existing
    /// location-update path then re-affiliates it to CMCE/Brew and reports it to the dashboard, so
    /// the next PTT succeeds â€” no manual DMO/TMO toggle, no wait for the periodic T351.
    ///
    /// This is the catch-all companion to the proactive boot sweep (`drive_recovery_replay`): it
    /// needs no persisted cache, fires only in response to a demonstrably present + active radio,
    /// and so covers the cases the cache misses (radio absent from the cache, or the sweep already
    /// gave up on it). Rate-limited per ISSI; gated by the access whitelist and the optional
    /// `[recovery] issi_allowlist`; on by default (`[recovery] reactive_enabled`).
    fn maybe_reactive_recovery(&mut self, queue: &mut MessageQueue, issi: u32) {
        // Known identity and current reachability are distinct. A radio returning after a
        // presence timeout may still know its affiliations, but must re-register before the
        // subscriber registry and Brew advertise it as reachable again.
        let known = self.client_mgr.client_is_known(issi);
        if known && self.config.state_read().subscribers.is_registered(issi) {
            return;
        }
        // A (V)ASSI is never one of this network's ISSIs: a radio on an unknown one has to
        // migrate again (clause 16.4.1.1 b)), not be commanded to re-register on it.
        if self.visitors.is_visitor(issi) || self.config.config().multi_network.is_vassi(issi) {
            return;
        }
        // Let the proactive boot sweep own any ISSI it is already replaying, to avoid double-keying.
        if self.recovery_attempts.contains_key(&issi) {
            return;
        }

        let cfg = self.config.config();
        let rec = &cfg.recovery;
        let coverage_return = known && cfg.cell.presence_check_secs != 0;
        if !coverage_return && !rec.reactive_enabled {
            return;
        }
        // Never key a radio that isn't permitted on this network â€” same scoping as init_recovery:
        // the access-control whitelist plus the optional recovery allowlist.
        let permitted = cfg.security.is_issi_allowed(issi)
            && (coverage_return || rec.issi_allowlist.is_empty() || rec.issi_allowlist.contains(&issi));
        if !permitted {
            return;
        }
        let cooldown = std::time::Duration::from_secs(rec.reactive_cooldown_secs);

        let now = std::time::Instant::now();
        if let Some(&last) = self.reactive_recovery_cooldown.get(&issi) {
            if now.duration_since(last) < cooldown {
                // Already commanded recently â€” give it time to answer rather than spamming COMMANDs
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
            "MM: ISSI {} active on uplink without a current registration; requesting a location update",
            issi
        );
        // handle = 0: addressed by ISSI on the MCCH (see send_d_location_update_command).
        if coverage_return {
            if let Some(receipt) = self.send_registration_check(queue, issi, true) {
                self.client_mgr.queue_registration_command(issi, now);
                self.registration_probe_receipts.insert(issi, receipt);
            }
        } else {
            Self::send_d_location_update_command(queue, issi, 0);
        }
    }

    /// Finish a "detach all currently attached group identities and attach group identities
    /// defined in the group identity uplink element" (TS 100 392-2 table 16.49): only the
    /// groups the MS had before and does not hold after the attach are deaffiliated. A group
    /// it re-attaches stays affiliated throughout, so a running call to it is not released
    /// (a group report re-attaches every group this way, clauses 16.4.3 and 16.8.3).
    fn deaffiliate_groups_left_after_detach_all(&mut self, queue: &mut MessageQueue, issi: u32, prior_groups: Vec<u32>) {
        let attached: HashSet<u32> = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|client| client.groups.iter().copied().collect())
            .unwrap_or_default();
        let left: Vec<u32> = prior_groups.into_iter().filter(|gssi| !attached.contains(gssi)).collect();
        if left.is_empty() {
            return;
        }
        {
            let mut state = self.config.state_write();
            for &gssi in &left {
                state.subscribers.deaffiliate(issi, gssi);
            }
        }
        self.emit_subscriber_update(queue, issi, left, BrewSubscriberAction::Deaffiliate);
    }

    fn emit_subscriber_update(&self, queue: &mut MessageQueue, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
        // If brew is active, forward subscriber updates to the Brew entity.
        // Register/Deregister must always be sent for brew-routable ISSIs,
        // even when there are no group affiliations yet. The Brew worker
        // decides whether to send REGISTER or REREGISTER based on its own state.
        // Affiliate/Deaffiliate only sent when there are brew-routable groups.
        if net_brew::is_site_linked(&self.config) {
            // Linked multi-cell: the site switch tracks every radio and group for cross-cell
            // routing, and applies the Brew filters below before anything reaches the network.
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mm,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                    issi,
                    groups: groups.clone(),
                    action,
                }),
            });
        } else if net_brew::is_active(&self.config) {
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

    /// ISSI whitelist gate. The dashboard can override the config whitelist at runtime (the state
    /// override takes precedence so edits apply without a restart); the configured
    /// `whitelist_mode` governs how whichever list is effective is read — in particular whether an
    /// empty list means "open network" or "deny all".
    fn issi_allowed(&self, issi: u32) -> bool {
        // A visitor's (V)ASSI: its home ISSI passed this gate at the first location update.
        if self.visitors.is_visitor(issi) {
            return true;
        }
        let state = self.config.state_read();
        self.config.config().security.allows(issi, state.issi_whitelist_override.as_deref())
    }

    /// May an unauthenticated teardown (U-ITSI-DETACH, or the migrating-location-update release)
    /// claiming `issi` on L2 `handle` be honoured?
    ///
    /// TETRA air-interface authentication (EN 300 392-7) is not implemented, so the source SSI of
    /// an uplink PDU is unverifiable and ISSIs are observable on air — a forged detach carrying a
    /// victim's ISSI would otherwise knock that radio off Brew, local call/SDS delivery and the
    /// dashboard, and a replay keeps it off. Two gates remain available: the operator may refuse
    /// to honour unauthenticated teardown at all, and the claimed ISSI must currently be
    /// registered on the same L2 context its registration used.
    fn teardown_authorized(&self, issi: u32, handle: u32, what: &str) -> bool {
        if !self.config.config().security.honour_unauthenticated_detach {
            tracing::warn!(
                "MM: ignoring {} for ISSI {} — [security] honour_unauthenticated_detach = false",
                what,
                issi
            );
            return false;
        }
        if !self.client_mgr.is_registered_on_handle(issi, handle) {
            tracing::warn!(
                "MM: ignoring {} for ISSI {} — not registered on this L2 context (forged or replayed teardown?)",
                what,
                issi
            );
            return false;
        }
        true
    }

    /// ISSIs that must never be evicted to make room in the client registry: anything CMCE
    /// currently has on a traffic channel (individual-call participants are keyed by ISSI in
    /// `active_call_ts`, so a radio mid-call/PTT is protected).
    fn call_protected_issis(&self) -> HashSet<u32> {
        self.config.state_read().active_call_ts.keys().copied().collect()
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
        // Access control BEFORE any state mutation: a non-whitelisted ISSI must never be able to
        // touch the registry, Brew or the dashboard, not even to tear something down.
        if !self.issi_allowed(ssi) {
            tracing::warn!("MM: ISSI {} not in whitelist, ignoring UItsiDetach", ssi);
            return;
        }
        // Unauthenticated teardown gate — see teardown_authorized.
        if !self.teardown_authorized(ssi, prim.handle, "U-ITSI-DETACH") {
            return;
        }

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
        // Deregistration ends the (V)ASSI (clause 16.4.7.2).
        if let Some(v) = self.visitors.remove(ssi) {
            self.config.state_write().multi_network.visitors.remove(&ssi);
            tracing::info!("MM: visitor {}:{} detached, (V)ASSI {} released", v.mni, v.issi, ssi);
        }
        self.group_report_requested_at.remove(&ssi);
        self.attach_overflow_remainder.remove(&ssi);
        self.recovery_mark_dirty();
    }

    fn rx_u_location_update_demand(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        tracing::trace!("rx_location_update_demand");
        // Kept unparsed so it can be replayed once the radio has authenticated.
        let unparsed = message.clone();
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

        let issi = prim.received_address.ssi;
        let handle = prim.handle;

        // ISSI whitelist check â€” FIRST, ahead of every Brew/registry mutation below (including the
        // migration release), so a non-whitelisted ISSI can never mutate state. It used to sit
        // after the migration branch, which let a barred radio tear a victim down before the gate
        // was ever consulted.
        if !self.issi_allowed(issi) {
            tracing::warn!("MM: ISSI {} not in whitelist, rejecting registration", issi);
            // Access-control cause, NOT MigrationNotSupported: telling a barred radio "migration
            // not supported" sends a conformant terminal hunting for another cell instead of
            // staying off, and never shows the user an access-denied.
            Self::send_d_location_update_reject_cause(
                queue,
                issi,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::ItsiAtsiUnknown,
            );
            return;
        }

        // The terminal answered with a location update â€” stop the restart-recovery replay to it
        // regardless of how this update is handled below (migration reject, whitelist reject, or
        // normal registration). Hoisted above all early-returns so a migrating/rejected terminal
        // isn't replayed to forever. No-op when recovery is disabled / ISSI not pending.
        self.recovery_confirm(issi);

        // Migrating location update (clause 16.4.1.1): case b) a radio of another network that
        // Multi Network accepts gets a (V)ASSI by identity exchange, case c) a radio of this
        // network coming back registers as usual. Everything else keeps the old answer:
        // "Migration not supported" (12, table 16.81) so the MS can act on it.
        let migrating = matches!(
            pdu.location_update_type,
            LocationUpdateType::MigratingLocationUpdating | LocationUpdateType::ServiceRestorationMigratingLocationUpdating
        );
        let migration = if migrating { self.migration_case(pdu.address_extension) } else { Migration::Refused };
        if let Migration::Visitor(mni) = migration {
            self.start_identity_exchange(queue, prim.received_address, handle, &pdu, mni);
            return;
        }
        // The second U-LOCATION UPDATE DEMAND of case b) (and every later one) comes on the
        // visitor's (V)ASSI and names its ITSI again: MNI in the address extension, ISSI in the
        // SSI element.
        if let Some(visitor) = self.visitors.get(issi).cloned() {
            let mni_ok = pdu.address_extension.is_none_or(|ext| Mni::from_address_extension(ext) == visitor.mni);
            let issi_ok = pdu.ssi.is_none_or(|ssi| ssi as u32 == visitor.issi);
            if !mni_ok || !issi_ok {
                tracing::warn!(
                    "MM: (V)ASSI {} belongs to {}:{}, location update names {:?}:{:?} — rejecting",
                    issi,
                    visitor.mni,
                    visitor.issi,
                    pdu.address_extension.map(Mni::from_address_extension),
                    pdu.ssi
                );
                self.visitors.remove(issi);
                self.config.state_write().multi_network.visitors.remove(&issi);
                Self::send_d_location_update_reject_cause(
                    queue,
                    issi,
                    handle,
                    pdu.location_update_type,
                    pdu.address_extension,
                    RejectCause::ItsiAtsiUnknown,
                );
                return;
            }
            // Visible to the Brew worker before anything of this radio goes to the network:
            // there it is the home ISSI (extension `mni`).
            self.config.state_write().multi_network.visitors.insert(issi, (visitor.mni, visitor.issi));
        } else if !migrating && self.config.config().multi_network.is_vassi(issi) {
            // A (V)ASSI this station did not give (another station or cell, or before a restart):
            // never a radio of this network. "ITSI/ATSI unknown" makes it invalid in the MS
            // (clause 16.4.7.3), so it has to register again with its own ITSI.
            tracing::info!("MM: unknown (V)ASSI {} — rejecting with ITSI/ATSI unknown", issi);
            Self::send_d_location_update_reject_cause(
                queue,
                issi,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::ItsiAtsiUnknown,
            );
            return;
        } else if !migrating && self.names_other_network(pdu.address_extension) {
            // An ITSI of another network that is not migrating: it holds no valid registration
            // here, so case b) of clause 16.4.1.1 applies (identity exchange), never a
            // registration under its bare ISSI, which here is a radio of this network. A radio
            // does this when it answers a D-LOCATION UPDATE COMMAND after its home changed.
            // "ITSI/ATSI unknown" makes the registration invalid in the MS (clause 16.4.7.3), so
            // it registers again, as a migrating radio.
            tracing::info!(
                "MM: ISSI {} of network {} sent a {:?} — not registered here, rejecting with ITSI/ATSI unknown",
                issi,
                pdu.address_extension.map(Mni::from_address_extension).map(|m| m.to_string()).unwrap_or_default(),
                pdu.location_update_type
            );
            Self::send_d_location_update_reject_to(
                queue,
                prim.received_address,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::ItsiAtsiUnknown,
                None,
            );
            return;
        }
        if migrating && migration == Migration::Refused {
            // Terminal wants to migrate to another network (e.g. SmartConnect).
            // We don't implement D-LOCATION-UPDATE-PROCEEDING identity exchange (ETSI Â§16.4.1.1 case b),
            // so we can't accept migration formally. But we MUST release the terminal from Brew
            // so the destination network can register it without identity conflict.
            // Send REJECT so terminal knows to try the other network, but first deregister from Brew.
            //
            // The release is a teardown keyed purely off the (unauthenticated) source SSI, so it
            // goes through the same gate as U-ITSI-DETACH: a forged migrating update carrying a
            // victim's ISSI must not release that victim. The REJECT itself is always sent â€” it
            // mutates nothing.
            if self.teardown_authorized(issi, handle, "migrating location update") {
                tracing::info!("MM: ISSI {} migrating to another network â€” releasing from Brew", issi);
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
            }
            // Layer 2 answers on the address the request came on: the USSI (clause 16.4.1.1 b)).
            Self::send_d_location_update_reject_to(
                queue,
                prim.received_address,
                handle,
                pdu.location_update_type,
                pdu.address_extension,
                RejectCause::MigrationNotSupported,
                None,
            );
            return;
        }

        // Per-source-ISSI registration rate limit. A flood cannot churn the registry (or the
        // dashboard, or Brew) faster than this even while it stays under the size cap. Dropped
        // silently: answering would itself be an amplifier.
        if !self.client_mgr.allow_registration(issi) {
            tracing::warn!("MM: ISSI {} exceeded the registration rate limit â€” dropping this location update", issi);
            return;
        }

        // Authentication (EN 300 392-7 clause 4): challenge the radio first when configured; the
        // registration is held and replayed here once it has proved its K.
        if self.authentication_gate(queue, issi, handle, pdu.location_update_type, pdu.address_extension, unparsed) {
            return;
        }

        // Air interface encryption (EN 300 392-7 clause 6.5): class 2 radios register with the
        // cell's SCK; radios without AIE register in clear (mixed cell).
        let (ciphering, preferred) = {
            let cfg = self.config.config();
            let aie = crate::aie::effective(&cfg);
            (
                crate::aie::registration_decision(aie, pdu.cipher_control, pdu.ciphering_parameters),
                aie.map(crate::aie::preferred_parameters),
            )
        };
        let ciphering = match ciphering {
            crate::aie::CipherDecision::Clear => None,
            crate::aie::CipherDecision::Encrypted(params) => Some(params),
            crate::aie::CipherDecision::Reject(cause) => {
                tracing::warn!(
                    "MM: ISSI {} asked for ciphering {:?} the cell does not offer — rejecting with {:?}",
                    issi,
                    pdu.ciphering_parameters.map(tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters::from_bits),
                    cause
                );
                Self::send_d_location_update_reject_ciphering(
                    queue,
                    issi,
                    handle,
                    pdu.location_update_type,
                    pdu.address_extension,
                    cause,
                    preferred,
                );
                return;
            }
            crate::aie::CipherDecision::Unsupported => {
                tracing::error!("MM: ISSI {} asked for ciphering but AIE is not active on this cell", issi);
                return;
            }
        };

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
        // stored ESM â€” client_mgr retains it because we no longer remove_client at T351 expiry.
        // Preserve energy saving mode across re-registrations.
        // If the terminal omits ESM from the PDU (common after T351 expiry),
        // reuse the previously granted mode so the terminal stays in EE mode.
        // We no longer filter out StayAlive â€” if that's what was granted before, keep it.
        let prior_esm = self.client_mgr.get_client_by_issi(issi).map(|c| c.energy_saving_mode);
        let effective_esm_request = pdu.energy_saving_mode.or(prior_esm);

        let esi = effective_esm_request.map(|esm| Self::grant_energy_saving(issi, esm));

        // Registry cap. A new ISSI may only be admitted once there is room: first make room by
        // evicting idle unaffiliated clients (LRU, never a radio mid-call or with affiliations),
        // and if the registry is still full refuse with Congestion rather than growing the heap
        // until the cell is OOM-killed. Known ISSIs are unaffected â€” they re-register in place.
        if !self.client_mgr.client_is_known(issi) {
            let protected = self.call_protected_issis();
            for evicted in self.client_mgr.enforce_client_cap(&protected) {
                self.config.state_write().subscribers.deregister(evicted);
                self.emit_subscriber_update(queue, evicted, Vec::new(), BrewSubscriberAction::Deregister);
            }
            if self.client_mgr.at_capacity() {
                tracing::warn!(
                    "MM: client registry full ({} clients) â€” refusing registration of ISSI {} with Congestion",
                    self.client_mgr.client_count(),
                    issi
                );
                Self::send_d_location_update_reject_cause(
                    queue,
                    issi,
                    handle,
                    pdu.location_update_type,
                    pdu.address_extension,
                    RejectCause::Congestion,
                );
                return;
            }
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
            // The terminal is re-registering while already known: a RoamingLocationUpdating after
            // a PTT/RF blip (Sepura/Motorola do this routinely), a PeriodicLocationUpdating renewing
            // T351, or a DemandLocationUpdating answering our COMMAND. In every case the radio is
            // PRESENT, so we must NOT tear down its registration or group affiliations here.
            //
            // A previous version treated a RoamingLocationUpdating older than 120s as a reboot and
            // ran a Deaffiliate -> Deregister -> Register -> Affiliate "reset". That reset was
            // net-harmful: the transient group_listeners -> 0 made CMCE drop the radio's ACTIVE
            // group call (drop_group_calls_if_unlistened), and the brief "unknown subscriber" window
            // made a PTT in that gap hit "no listeners" -- which the terminal reads as a network
            // error and fully disconnects (the "radio constantly disconnects from the BTS" report).
            // It re-affiliated the SAME groups immediately afterwards, so the reset only ever flapped
            // the call. Genuine group-set changes are reconciled by the group-identity processing
            // further down and by explicit U-ATTACH-DETACH, so no reset is needed here.
            if was_pending {
                tracing::info!("MM: ISSI {} re-registered after T351 COMMAND (Brew already holds it)", issi);
            }
            // Always reset the registration timer on any re-registration
            self.client_mgr.reset_registration_timer(issi);
        }
        // Determine if we need to emit Register toward Brew. Only when Brew was actually torn down
        // (or never had this ISSI):
        //   A) Terminal is genuinely new (never seen before).
        //   B) Terminal is known but re-attaching via ItsiAttach â€” migrated from another network.
        //   C) Terminal was dropped from Brew at a T351 confirmed-gone (second) expiry and is now
        //      back (was_dropped); the re-add path above already restored it locally.
        // A plain T351 re-registration (was_pending) is deliberately NOT here: teardown no longer
        // happens at first expiry, so Brew still holds the subscriber â€” re-registering would be a
        // needless REGISTER every interval (the Brew flap this avoids).
        let is_itsi_attach = pdu.location_update_type == LocationUpdateType::ItsiAttach;
        // Case c) (a radio of this network back from another one) is a fresh registration too.
        let needs_brew_register = is_new || (!is_new && is_itsi_attach) || was_dropped || migration == Migration::Home;

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
        // register() resets attached_groups â€” the group-identity processing / coverage-return
        // re-affiliation below rebuilds them, so this must run before either.
        if was_dropped {
            self.config.state_write().subscribers.register(issi);
            if let Some(sink) = &self.telemetry {
                sink.send(crate::net_telemetry::TelemetryEvent::MsRegistration { issi });
            }
            tracing::info!(
                "MM: ISSI {} re-appeared after T351 drop â€” re-registered in dashboard + subscriber registry",
                issi
            );
        }
        if needs_brew_register {
            if !is_new && is_itsi_attach {
                tracing::info!(
                    "MM: ISSI {} re-attaching via ItsiAttach (returned from another network) â€” re-registering in Brew",
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
        let gila = if let Some(mut gild) = pdu.group_identity_location_demand {
            let mut forms = HashMap::new();
            if let Some(giu) = gild.group_identity_uplink.take() {
                let (local, f) = self.localize_group_uplink(issi, giu);
                forms = f;
                gild.group_identity_uplink = Some(local);
            }
            gild.group_identity_uplink = gild
                .group_identity_uplink
                .map(|giu| self.aie_filter_group_attach(issi, ciphering.is_some(), giu));
            let mut refused_gid = Vec::new();
            if let Some(giu) = gild.group_identity_uplink.take() {
                let (admitted, refused) = self.group_access_split(issi, giu);
                gild.group_identity_uplink = Some(admitted);
                refused_gid = refused;
            }
            // ETSI Table 16.49 (clause 16.10.17): mode=1 means "detach all currently
            // attached group identities and attach group identities defined in the
            // group identity uplink element."
            let mut detached_prior = Vec::new();
            if gild.group_identity_attach_detach_mode == 1 {
                let prior_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|client| client.groups.iter().copied().collect())
                    .unwrap_or_default();
                if let Err(e) = self.client_mgr.client_detach_all_groups(issi) {
                    tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
                } else {
                    detached_prior = prior_groups;
                }
            }

            // Try to attach to requested groups, then build GroupIdentityLocationAccept element
            let mut accepted_groups = if let Some(giu) = &gild.group_identity_uplink {
                Some(self.try_attach_detach_groups(queue, issi, &giu))
            } else {
                None
            };
            self.deaffiliate_groups_left_after_detach_all(queue, issi, detached_prior);
            // Refused by the group-access rules: listed with their reason (clauses 16.8.2, 16.10.23).
            let any_refused = !refused_gid.is_empty();
            if let Some(list) = accepted_groups.as_mut() {
                self.address_groups_for(issi, list, &forms, true);
            }
            self.address_groups_for(issi, &mut refused_gid, &forms, false);
            if any_refused {
                accepted_groups.get_or_insert_with(Vec::new).extend(refused_gid);
            }
            let gila = GroupIdentityLocationAccept {
                group_identity_accept_reject: u8::from(any_refused),
                group_identity_downlink: accepted_groups,
            };

            Some(gila)
        } else {
            // No GroupIdentityLocationAccept element present
            None
        };

        // Coverage-return re-affiliation (fixes "PTT no longer works after leaving and
        // returning to coverage", workaround = DMOâ†’TMO).
        //
        // Sequence that breaks PTT:
        //   1. MS affiliates to a GSSI â†’ CMCE group_listeners[gssi] += 1. PTT works.
        //   2. MS leaves coverage; BS T351 expires and emits Deregister to CMCE, which
        //      does dec_group_listener() â†’ the GSSI now has 0 listeners.
        //   3. MS returns. Because we hand out attachment_lifetime=0 (persistent), the MS
        //      believes it is still affiliated and sends a plain location update WITHOUT a
        //      group identity report.
        //   4. MM re-registers the MS but never re-affiliates the groups â†’ CMCE still has
        //      0 listeners for the GSSI â†’ the next PTT is rejected with "no listeners"
        //      ("please wait" on the radio). DMOâ†’TMO forces an ItsiAttach with a full group
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
                    "MM: ISSI {} re-registered without group report but has {} stored group(s) {:?} â€” re-affiliating to resync CMCE/Brew (coverage-return fix)",
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
                self.emit_group_snapshots(issi);
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
        match ciphering {
            Some(params) => tracing::info!("MS {} registered encrypted ({})", issi, params),
            None => tracing::debug!("MS {} registered in clear", issi),
        }
        let _ = self.client_mgr.set_client_ciphering(issi, ciphering);
        self.config.state_write().subscribers.set_encrypted(issi, ciphering.is_some());

        // Reset periodic registration timer on every successful registration.
        self.client_mgr.reset_registration_timer(issi);

        // Registration / affiliation / EE state changed â€” persist for restart recovery (debounced).
        self.recovery_mark_dirty();

        // The location update accept type "shall indicate the type of registration" (TS 100 392-2
        // clause 16.10.35a, table 16.68), so the accept names the registration the MS asked for.
        // Tables 16.67 and 16.68 differ only in the migrating values: 001 is "temporary
        // registration" in the accept, and 101 stands for both migrating requests (case c) here).
        // Some radios key attach-completion off the accept type. The periodic interval is not
        // carried in D-LOCATION-UPDATE-ACCEPT; this stack re-attracts radios with an explicit
        // D-LOCATION-UPDATE-COMMAND at T351 expiry (see tick_start).
        let accept_type = if migrating {
            LocationUpdateType::ServiceRestorationMigratingLocationUpdating
        } else {
            pdu.location_update_type
        };
        // A visitor's accept carries its MNI (clause 16.4.1.1: the MS checks it against its ITSI)
        // and its (V)ASSI.
        let visitor_mni = self.visitors.get(issi).map(|v| v.mni);

        // Build D-LOCATION UPDATE ACCEPT pdu
        let pdu_response = DLocationUpdateAccept {
            location_update_accept_type: accept_type,
            ssi: Some(issi as u64),
            address_extension: visitor_mni.map(|m| m.address_extension() as u64),
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

        if let Some(mni) = visitor_mni {
            if let Some(v) = self.visitors.get(issi)
                && !v.registered
            {
                tracing::info!("MM: visitor {}:{} registered on (V)ASSI {}", mni, v.issi, issi);
            }
            self.visitors.confirm(issi);
        } else if migration == Migration::Home {
            tracing::info!("MM: ISSI {} back from another network (migration case c)", issi);
        }

        // A plain roaming registration: an MS with a circuit mode call would have asked for
        // service restoration roaming (clause 16.4 a)), so an individual call that moved here
        // with it on the network will not be restored. Demand location updating (an answer to
        // our COMMAND) says nothing about calls and is not reported.
        if pdu.location_update_type == LocationUpdateType::RoamingLocationUpdating
            && (net_brew::is_site_linked(&self.config)
                || (net_brew::is_active(&self.config) && net_brew::is_brew_issi_routable(&self.config, issi)))
        {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mm,
                dest: TetraEntity::Brew,
                msg: SapMsgInner::CmceCallControl(CallControl::RegisteredWithoutCall { issi }),
            });
        }

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

    fn emit_group_snapshots(&self, issi: u32) {
        let Some(sink) = &self.telemetry else {
            return;
        };
        let state = self.config.state_read();
        let gssis = state.subscribers.attached_groups_of(issi);
        let groups = state
            .subscribers
            .device_groups_of(issi)
            .into_iter()
            .map(|group| crate::net_telemetry::events::MsGroupInfo {
                gssi: group.gssi,
                mnemonic: group.mnemonic,
                attachment_mode: group.attachment_mode,
                is_dynamic: group.is_dynamic,
                is_attached: group.is_attached,
            })
            .collect();
        drop(state);
        sink.send(crate::net_telemetry::TelemetryEvent::MsGroupsSnapshot { issi, gssis });
        sink.send(crate::net_telemetry::TelemetryEvent::MsGroupCatalogSnapshot { issi, groups });
    }

    fn emit_dgna_status(&self, issi: u32, gssi: u32, attach: bool, accepted: bool, source: &str, detail: String) {
        let Some(sink) = &self.telemetry else {
            return;
        };
        sink.send(crate::net_telemetry::TelemetryEvent::DgnaStatus(
            crate::net_telemetry::events::DgnaStatusInfo {
                issi,
                gssi,
                attach,
                accepted,
                source: source.to_string(),
                detail,
            },
        ));
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
            None => (None, None), // StayAlive â€” no monitoring window
            Some(cycle) => {
                // Frame-based start point (ETSI EN 300 392-2 Table 23.9 / clause 23.7.6): the MS
                // wakes for one TDMA frame every `cycle` frames. Spread MSs across the cycle by
                // ISSI so they don't all wake in the same frame. The start point's absolute frame
                // index (m-1)*18+(f-1) must be â‰¡ phase (mod cycle); anchoring it in multiframe 1
                // yields a valid Frame Number (1..=18) and Multiframe Number (1..=60 â€” never the
                // StayAlive-reserved 0 the old `(issi/18)%cycle` formula produced).
                let phase = (issi % cycle as u32) as u8; // 0..cycle-1, â‰¤ 5 for capped Eg1..Eg3
                let frame_num = (phase % 18) + 1; // 1..=18
                let mframe_num = (phase / 18) + 1; // 1..=60 (== 1 for the supported cycles â‰¤ 6)
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
                // energy economy on/off at the radio takes effect mid-session â€” both for actual
                // paging (monitoring window) and for the dashboard, which mirrors the granted
                // mode via the MsEnergySaving telemetry emitted by set_client_energy_saving_mode.
                // Without this the handler used to force StayAlive, so a re-activation never
                // reached the dashboard until the terminal fully re-registered (power-cycle).
                let esi = Self::grant_energy_saving(issi, esm);
                // If the client was concurrently removed (T351 second-expiry race), the
                // setters return ClientNotFound â€” log it so the silent no-op is at least
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
                // as unimplemented rather than asserting â€” assert_warn made it look like
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

    /// AIE mixed cell: drop attach requests for groups of the other encryption mode, so a clear
    /// radio never joins an encrypted group or the reverse. Detach requests always pass.
    fn aie_filter_group_attach(&self, issi: u32, encrypted: bool, giu: Vec<GroupIdentityUplink>) -> Vec<GroupIdentityUplink> {
        let cfg = self.config.config();
        let aie = crate::aie::effective(&cfg);
        giu.into_iter()
            .filter(|g| {
                let ok = g.group_identity_detachment_uplink.is_some()
                    || g.gssi.is_none_or(|gssi| crate::aie::may_use_group(aie, encrypted, gssi));
                if !ok {
                    tracing::info!(
                        "MM: ISSI {} ({}) may not attach GSSI {:?}: group of the other encryption mode",
                        issi,
                        if encrypted { "encrypted" } else { "clear" },
                        g.gssi
                    );
                }
                ok
            })
            .collect()
    }

    /// Who may attach which group (Brew server rules, `group-access`): splits the requested
    /// attachments into the admitted ones (and every detachment) and, for each group the radio
    /// may not attach, a refusal entry for the acknowledgement. ETSI TS 100 392-2 clause 16.8.2:
    /// the acknowledgement "shall indicate those group identities which were not accepted with a
    /// rejection reason in the group identity detachment downlink"; the reason is "Temporary 1
    /// detachment" (table 16.52, values network dependent), not "Permanent": the rules can change
    /// and an MS should not retry a permanently detached group.
    fn group_access_split(&self, issi: u32, giu: Vec<GroupIdentityUplink>) -> (Vec<GroupIdentityUplink>, Vec<GroupIdentityDownlink>) {
        let state = self.config.state_read();
        // A visitor from the server's own network is named by its home ISSI in the server's
        // rules; during its registration it is not yet in the shared visitor table.
        let server = state.multi_network.server.as_ref().map(|(this, _)| *this);
        let rule_issi = match self.visitors.get(issi) {
            Some(v) if Some(v.mni) == server => v.issi,
            _ => issi,
        };
        let mut refused = Vec::new();
        let admitted = giu
            .into_iter()
            .filter(|g| {
                let Some(gssi) = g.gssi else { return true };
                if g.group_identity_detachment_uplink.is_some() || state.group_may_join(gssi, rule_issi) {
                    return true;
                }
                tracing::info!("MM: ISSI {} may not attach GSSI {} (group-access), refused", issi, gssi);
                refused.push(GroupIdentityDownlink {
                    group_identity_attachment: None,
                    group_identity_detachment_uplink: Some(GROUP_DETACHMENT_TEMPORARY_1),
                    gssi: Some(gssi),
                    address_extension: None,
                    vgssi: None,
                });
                false
            })
            .collect();
        (admitted, refused)
    }

    /// A visitor's group identity uplink (clause 16.8.7) in this station's group ids: a group
    /// of this network keeps its GSSI (GIAT 0, or GIAT 1 with this network's MNI); a group of
    /// the visitor's home network named by GSSI + address extension (GIAT 1) gets a (V)GSSI
    /// (allocated on attachment); a (V)GSSI (GIAT 2) stands for itself. The map remembers how
    /// each group was named so the downlink answers in the form the clause asks for. Groups of
    /// a third network and unknown (V)GSSIs are left out (logged). Radios of this network are
    /// passed through unchanged.
    fn localize_group_uplink(
        &mut self,
        issi: u32,
        giu: Vec<GroupIdentityUplink>,
    ) -> (Vec<GroupIdentityUplink>, HashMap<u32, GroupForm>) {
        let mut forms = HashMap::new();
        let Some(home) = self.visitors.get(issi).map(|v| v.mni) else {
            return (giu, forms);
        };
        let cfg = self.config.config();
        let own = Mni { mcc: cfg.net.mcc, mnc: cfg.net.mnc };
        let mut out = Vec::with_capacity(giu.len());
        for mut g in giu {
            let detach = g.group_identity_detachment_uplink.is_some();
            match (g.gssi, g.address_extension, g.vgssi) {
                (Some(_), None, None) => {}
                (Some(gssi), Some(ext), None) => {
                    let mni = Mni::from_address_extension(ext as u64);
                    if mni == own {
                        g.address_extension = None;
                    } else if mni == home {
                        let vgssi = if detach {
                            self.visitors.known_vgssi(mni, gssi)
                        } else {
                            let client_mgr = &self.client_mgr;
                            self.visitors
                                .vgssi_for(&cfg.multi_network, mni, gssi, |v| client_mgr.client_is_known(v))
                        };
                        let Some(vgssi) = vgssi else {
                            tracing::warn!("MM: visitor {} group {}:{} has no (V)GSSI — skipped", issi, mni, gssi);
                            continue;
                        };
                        self.config.state_write().multi_network.groups.insert(vgssi, (mni, gssi));
                        forms.insert(vgssi, GroupForm::Home { mni, gssi });
                        g.gssi = Some(vgssi);
                        g.address_extension = None;
                    } else {
                        tracing::info!("MM: visitor {} names group {}:{} of a third network — skipped", issi, mni, gssi);
                        continue;
                    }
                }
                (None, None, Some(vgssi)) => {
                    if self.visitors.home_group(vgssi).is_none() {
                        tracing::info!("MM: visitor {} names unknown (V)GSSI {} — skipped", issi, vgssi);
                        continue;
                    }
                    forms.insert(vgssi, GroupForm::Vgssi);
                    g.gssi = Some(vgssi);
                    g.vgssi = None;
                }
                _ => {
                    tracing::info!("MM: visitor {} sent a group identity uplink without a valid address — skipped", issi);
                    continue;
                }
            }
            out.push(g);
        }
        (out, forms)
    }

    /// Puts a visitor's home groups back into the address form of clause 16.8.7 in a group
    /// identity downlink: (V)GSSI alone (GIAT 2) when the uplink named it so, GSSI + address
    /// extension (GIAT 1) for a refused GIAT 1 attachment, else GSSI + address extension +
    /// (V)GSSI (GIAT 3). Groups of this network stay GIAT 0.
    fn address_groups_for(&self, issi: u32, gids: &mut [GroupIdentityDownlink], forms: &HashMap<u32, GroupForm>, accepted: bool) {
        if !self.visitors.is_visitor(issi) {
            return;
        }
        for gid in gids.iter_mut() {
            let Some(local) = gid.gssi else { continue };
            let Some((mni, gssi)) = self.visitors.home_group(local) else { continue };
            match forms.get(&local) {
                Some(GroupForm::Vgssi) => {
                    gid.gssi = None;
                    gid.address_extension = None;
                    gid.vgssi = Some(local);
                }
                Some(GroupForm::Home { .. }) if !accepted => {
                    gid.gssi = Some(gssi);
                    gid.address_extension = Some(mni.address_extension());
                    gid.vgssi = None;
                }
                _ => {
                    gid.gssi = Some(gssi);
                    gid.address_extension = Some(mni.address_extension());
                    gid.vgssi = Some(local);
                }
            }
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
                tracing::warn!(
                    "Failed parsing UAttachDetachGroupIdentity: {:?} {}",
                    e,
                    prim.sdu.dump_bin()
                );
                // MXP600 often re-sends the full scan list in a follow-up U-ATTACH that
                // exceeds the uplink TM-SDU and arrives truncated. If we still hold the
                // ACK-capped remainder from the previous PDU, push it via SwMI D-ATTACH.
                if let Some(rem) = self.attach_overflow_remainder.remove(&issi) {
                    if !rem.is_empty() {
                        let gssis: Vec<u32> = rem.iter().filter_map(|g| g.gssi).collect();
                        tracing::warn!(
                            "ISSI {} U-ATTACH parse failed with {} stashed overflow group(s) {:?} — SwMI D-ATTACH remainder",
                            issi,
                            gssis.len(),
                            gssis
                        );
                        self.swmi_attach_overflow_remainder(queue, issi, &rem);
                        self.group_report_requested_at.remove(&issi);
                    }
                }
                return;
            }
        };

        // Access control: apply the same [security] issi_whitelist gate as registration
        // (rx_u_location_update_demand). Without this, an unknown ISSI can self-register
        // via the group-attach path below, bypassing the whitelist and growing the client
        // registry without bound.
        if !self.issi_allowed(issi) {
            tracing::warn!("MM: ISSI {} not in whitelist, rejecting group attach/detach", issi);
            self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
            return;
        }

        // Check if we can satisfy this request, print unsupported stuff
        if !Self::feature_check_u_attach_detach_group_identity(&pdu) {
            // group_identity_uplink missing â€” terminal is sending a group report response
            // without requesting any group changes. Send ACK with current groups so
            // terminal knows it's affiliated and can use PTT.
            tracing::info!(
                "UAttachDetachGroupIdentity from {} has no uplink groups â€” sending ACK with current groups",
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
        let mut detached_prior = Vec::new();
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
                        // ETSI EN 300 392-2 Â§16.3.4: if MS cannot be registered,
                        // send D-ATTACH-DETACH-GROUP-IDENTITY-ACKNOWLEDGEMENT with reject.
                        tracing::warn!("Failed re-registering MS {} on group attach: {:?} â€” sending reject", issi, e);
                        self.send_d_attach_detach_ack_reject(queue, issi, prim.handle);
                        return;
                    }
                }
            } else {
                // Client is known â€” detach all existing groups first
                let prior_groups: Vec<u32> = self
                    .client_mgr
                    .get_client_by_issi(issi)
                    .map(|client| client.groups.iter().copied().collect())
                    .unwrap_or_default();
                match self.client_mgr.client_detach_all_groups(issi) {
                    Ok(_) => detached_prior = prior_groups,
                    Err(e) => {
                        tracing::warn!("Failed detaching all groups for MS {}: {:?}", issi, e);
                        return;
                    }
                }
            }
        }

        // ETSI EN 300 392-2 §16.9.2.2: the ACK PDU travels in a single TM-SDU
        // and there is no MM-level segmentation. Empirically MXP600 and MTP3550
        // start losing the ACK around 12-15 GroupIdentityDownlink entries —
        // the PDU exceeds what fits in a FACCH/SACCH burst, the MS times out, and on
        // subsequent retries it eventually de-registers ("Unit not attached").
        //
        // We have to cap the request *before* affiliating on the BS side. A previous
        // version of this code affiliated everything and then truncated only the ACK
        // response — that desynced the MS and the BS: the BS thought N groups were
        // active, but the MS only saw confirmations for the first 12. Inbound calls
        // on the un-confirmed groups would deliver to the BS but never notify the MS
        // (FH-BUG-022 reopened, FH-BUG-025). Now the BS only affiliates what it can
        // confirm.
        //
        // When the MS stuffs more than MAX_GROUPS_PER_ATTACH into one U-ATTACH (e.g.
        // CPS scan list >12), it typically does *not* retry on its own. ETSI §16.8.3
        // defines the SwMI-initiated group report: after ACK of the first PDU, we
        // request a group report so the MS re-affiliates in multipacket form
        // (detach-all + amendments, ACK between each, last with report complete).
        const MAX_GROUPS_PER_ATTACH: usize = 12;
        // feature_check_u_attach_detach_group_identity above guarantees this is Some,
        // but use let-else instead of .unwrap() so a future refactor that loosens that
        // check doesn't crash the MM worker on a malformed PDU.
        let Some(giu) = pdu.group_identity_uplink else {
            tracing::warn!("rx_u_attach_detach_group_identity: group_identity_uplink missing after feature_check; ignoring");
            return;
        };
        let (giu, forms) = self.localize_group_uplink(issi, giu);
        let encrypted = self.config.state_read().subscribers.encrypted(issi).unwrap_or(false);
        let giu = self.aie_filter_group_attach(issi, encrypted, giu);
        let (giu, mut refused_gid) = self.group_access_split(issi, giu);
        let requested_n = giu.len();
        let (giu_clamped, dropped_giu) = if requested_n > MAX_GROUPS_PER_ATTACH {
            let (head, tail) = giu.split_at(MAX_GROUPS_PER_ATTACH);
            (head.to_vec(), tail.to_vec())
        } else {
            (giu, Vec::new())
        };
        let dropped = dropped_giu.len();

        if pdu.group_identity_attach_detach_mode {
            tracing::debug!(
                "ISSI {} U-ATTACH detach-all + {} group IE(s){}",
                issi,
                requested_n,
                if dropped > 0 {
                    format!(" (ACK will cover first {MAX_GROUPS_PER_ATTACH})")
                } else {
                    String::new()
                }
            );
        } else if dropped > 0 || self.group_report_requested_at.contains_key(&issi) {
            tracing::info!(
                "ISSI {} U-ATTACH amendment: {} group IE(s){}",
                issi,
                requested_n,
                if dropped > 0 {
                    format!(", capped to {MAX_GROUPS_PER_ATTACH} for ACK")
                } else {
                    String::new()
                }
            );
        }

        let mut report_not_complete = false;
        if let Some(ref grr) = pdu.group_report_response {
            // Type3 Group report response: LSB 0 = not complete, 1 = complete (ETSI Table 16.83).
            let complete = (grr.data & 1) != 0;
            report_not_complete = !complete;
            tracing::info!(
                "ISSI {} group report response: {} (data={:#x}, len={})",
                issi,
                if complete { "complete" } else { "not complete" },
                grr.data,
                grr.len
            );
            if complete {
                self.group_report_requested_at.remove(&issi);
                self.attach_overflow_remainder.remove(&issi);
            }
        }

        // Try to attach to requested groups, and retrieve list of accepted GroupIdentityDownlink elements
        let mut accepted_gid = self.try_attach_detach_groups(queue, issi, &giu_clamped);
        self.deaffiliate_groups_left_after_detach_all(queue, issi, detached_prior);
        // Groups refused by the group-access rules: listed with their reason (clause 16.8.2) and
        // "at least one attachment/detachment rejected" (table 16.44).
        let any_refused = !refused_gid.is_empty();
        self.address_groups_for(issi, &mut accepted_gid, &forms, true);
        self.address_groups_for(issi, &mut refused_gid, &forms, false);
        accepted_gid.extend(refused_gid);

        // Group affiliations changed — persist for restart recovery (debounced).
        self.recovery_mark_dirty();

        // Build reply PDU
        let pdu_response = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: u8::from(any_refused),
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

        // After ACK of a capped attach: §16.8.3 group report, or SwMI amend of the
        // remainder if the MS is already mid-report / failed to multipacket cleanly.
        if !dropped_giu.is_empty() {
            self.handle_attach_overflow_after_ack(
                queue,
                issi,
                requested_n,
                &dropped_giu,
                MAX_GROUPS_PER_ATTACH,
                report_not_complete,
            );
        } else {
            self.attach_overflow_remainder.remove(&issi);
        }
    }

    /// After a capped U-ATTACH ACK:
    /// - Fresh overstuff → SwMI group report (§16.8.3) and stash the discarded IEs.
    /// - MS already mid-report (`report not complete`) or still overstuffs after a report →
    ///   SwMI D-ATTACH of the remainder (§16.8.1). Re-requesting report makes MXP600
    ///   re-send the full scan list in a PDU that truncates on the uplink.
    fn handle_attach_overflow_after_ack(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        requested_n: usize,
        dropped_giu: &[GroupIdentityUplink],
        max_per_pdu: usize,
        report_not_complete: bool,
    ) {
        let dropped = dropped_giu.len();
        let now = std::time::Instant::now();
        let report_pending = self
            .group_report_requested_at
            .get(&issi)
            .copied()
            .is_some_and(|at| now.duration_since(at) < GROUP_REPORT_REQUEST_COOLDOWN);

        self.attach_overflow_remainder
            .insert(issi, dropped_giu.to_vec());

        if report_not_complete || report_pending {
            tracing::warn!(
                "ISSI {} attach overflow: {} groups beyond ACK cap {} (requested {}) — {} → SwMI D-ATTACH of remainder (ETSI §16.8.1 fallback)",
                issi,
                dropped,
                max_per_pdu,
                requested_n,
                if report_not_complete {
                    "MS report not complete but PDU overstuffed"
                } else {
                    "MS did not multipacket after SwMI group report"
                }
            );
            self.swmi_attach_overflow_remainder(queue, issi, dropped_giu);
            self.attach_overflow_remainder.remove(&issi);
            self.group_report_requested_at.remove(&issi);
            return;
        }

        tracing::info!(
            "ISSI {} attach overflow: {} groups beyond ACK cap {} (requested {}) → SwMI group report requested (ETSI §16.8.3); remainder stashed for fallback",
            issi,
            dropped,
            max_per_pdu,
            requested_n
        );
        self.group_report_requested_at.insert(issi, now);
        self.send_d_group_report_request(queue, issi);
    }

    /// Affiliate the overflow tail on the BS and push a SwMI-initiated D-ATTACH/DETACH
    /// GROUP IDENTITY (amend, ack requested) so the MS can pick up groups that did not
    /// fit in the U-ATTACH ACK. Used only after §16.8.3 multipacket failed.
    fn swmi_attach_overflow_remainder(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        dropped_giu: &[GroupIdentityUplink],
    ) {
        let attach_tail: Vec<GroupIdentityUplink> = dropped_giu
            .iter()
            .filter(|g| g.class_of_usage.is_some() && g.gssi.is_some())
            .cloned()
            .collect();
        if attach_tail.is_empty() {
            tracing::debug!(
                "ISSI {} overflow remainder has no attach IEs — nothing to SwMI-push",
                issi
            );
            return;
        }

        let gssis: Vec<u32> = attach_tail.iter().filter_map(|g| g.gssi).collect();
        tracing::info!(
            "ISSI {} SwMI D-ATTACH remainder ({} group(s)): {:?}",
            issi,
            gssis.len(),
            gssis
        );

        let mut accepted = self.try_attach_detach_groups(queue, issi, &attach_tail);
        self.address_groups_for(issi, &mut accepted, &HashMap::new(), true);
        self.recovery_mark_dirty();
        if accepted.is_empty() {
            tracing::warn!("ISSI {} SwMI remainder attach produced no accepted groups", issi);
            return;
        }

        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: false,
            group_identity_acknowledgement_request: true,
            // Amend — do not wipe the 12 already ACKed.
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            group_identity_downlink: Some(accepted),
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(32);
        if pdu.to_bitbuf(&mut sdu).is_err() {
            tracing::error!(
                "MM: failed serializing SwMI D-ATTACH remainder for ISSI {}",
                issi
            );
            return;
        }
        sdu.seek(0);
        tracing::debug!(
            "-> DAttachDetachGroupIdentity (SwMI remainder amend) issi={} sdu {}",
            issi,
            sdu.dump_bin()
        );

        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: 0,
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

    /// Build and queue a SwMI-initiated group report request (ETSI EN 300 392-2 §16.8.3):
    /// D-ATTACH/DETACH GROUP IDENTITY with *group report* = requested, acknowledgement
    /// *not* requested, and no group identity downlink. The MS shall then report/re-attach
    /// its groups, possibly across multiple U-ATTACH PDUs.
    fn send_d_group_report_request(&self, queue: &mut MessageQueue, issi: u32) {
        let pdu = DAttachDetachGroupIdentity {
            group_identity_report: true,
            group_identity_acknowledgement_request: false,
            // Ignored by the MS when group_identity_report is set (ETSI §16.8.3).
            group_identity_attach_detach_mode: false,
            proprietary: None,
            group_report_response: None,
            // Shall not be present for a group report request.
            group_identity_downlink: None,
            group_identity_security_related_information: None,
        };

        let mut sdu = BitBuffer::new_autoexpand(16);
        if pdu.to_bitbuf(&mut sdu).is_err() {
            tracing::error!(
                "MM: failed serializing D-ATTACH group report request for ISSI {}",
                issi
            );
            return;
        }
        sdu.seek(0);
        tracing::debug!(
            "-> DAttachDetachGroupIdentity (group report request) issi={} sdu {}",
            issi,
            sdu.dump_bin()
        );

        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: 0, // unsolicited SwMI-initiated
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
            MmPduTypeUl::UAuthentication => self.rx_u_authentication(queue, message),
            MmPduTypeUl::UItsiDetach => self.rx_u_itsi_detach(queue, message),
            MmPduTypeUl::ULocationUpdateDemand => self.rx_u_location_update_demand(queue, message),
            MmPduTypeUl::UMmStatus => self.rx_u_mm_status(queue, message),
            MmPduTypeUl::UCkChangeResult => unimplemented_log!("UCkChangeResult"),
            MmPduTypeUl::UOtar => self.rx_u_otar(queue, message),
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
            // (vgssi, address extension, missing gssi) is unsupported â€” log and skip.
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
                        // group_identity_attachment_lifetime values (ETSI EN 300 392-2 Â§16.10.19):
                        //   0 = Attachment not needed â†’ MS keeps the group attached indefinitely
                        //                                until an explicit detach. This is what we want
                        //                                for scan lists / persistent group attachments.
                        //   1 = Attachment required for the next ITSI attach â†’ MS re-affiliates on next
                        //                                ITSI attach (rare event: reboot, cell reselect).
                        //   2 = Attachment not allowed for next ITSI attach â†’ SwMI denies.
                        //   3 = Attachment required for next location update â†’ MS re-affiliates at every
                        //                                LU (every few minutes), generating churn.
                        //
                        // We previously used 1 with a "good default" comment, but that interacted badly
                        // with Motorola MTP-series radios in scan-list mode: those radios send the scan
                        // list incrementally (2 GSSIs at a time, with one anchor + one new GSSI), and
                        // expect the BS-side affiliation to persist between batches. With lifetime=1 the
                        // MS internally drops the affiliation a few minutes later ("5-minute timer" per
                        // dk5ras), then PTT fails with "Unit not attached" until the user changes GSSI.
                        // Lifetime=0 makes the attachment persistent on the MS side â€” matching the BS
                        // side which already keeps affiliations across attach cycles â€” and resolves
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
                        // Terminal was removed (T351 second expiry) while PDU was in flight â€” ignore.
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

        // Emit a full snapshot so the dashboard can render both current affiliations and the
        // persistent DGNA catalog for this MS.
        self.emit_group_snapshots(issi);

        accepted_groups
    }

    /// Sends a D-LOCATION UPDATE COMMAND to force the radio to re-register
    /// with full group identity report
    /// Send D-ATTACH-DETACH-GROUP-IDENTITY-ACKNOWLEDGEMENT with reject.
    /// ETSI EN 300 392-2 Â§16.3.4: used when MS is not registered.
    fn send_d_attach_detach_ack_reject(&self, queue: &mut MessageQueue, issi: u32, handle: u32) {
        let pdu = DAttachDetachGroupIdentityAcknowledgement {
            group_identity_accept_reject: 1, // 1 = reject per ETSI Â§14.8.7
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
        let mut gid: Vec<GroupIdentityDownlink> = groups
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
        self.address_groups_for(issi, &mut gid, &HashMap::new(), true);
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

    /// DGNA (Dynamic Group Number Assignment) â€” BS-initiated group attach/detach for one terminal,
    /// driven from the dashboard. SS-DGNA = TS 100 392-12-22 V1.5.1; legacy fallback = EN 300 392-2
    /// V2.4.1 cl.16.8.
    ///
    /// Local-only: updates the BS-side affiliation (so local group calls and group SDS route to the
    /// terminal) and asks the air-interface to be updated. By default that is an SS-DGNA
    /// ASSIGN/DEASSIGN handed to CMCE for a D-FACILITY; with `dgna_use_ss_facility = false` it falls
    /// back to an unsolicited MM D-ATTACH/DETACH GROUP IDENTITY. Brew is intentionally not involved.
    ///
    /// Returns `true` if the command was accepted and the regroup was put on the air.
    fn do_dgna(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        gssi: u32,
        mnemonic: Option<String>,
        attachment_mode: u8,
        attach: bool,
    ) -> bool {
        let verb = if attach { "assign" } else { "deassign" };
        let is_dynamic = self.config.state_read().subscribers.has_dgna_group(issi, gssi);
        let is_attached = self.config.state_read().subscribers.attached_groups_of(issi).contains(&gssi);
        let route_gssi_hint = (!attach && is_attached).then_some(gssi);

        // A GSSI is a 24-bit TETRA SSI. The SS-DGNA D-FACILITY encodes it into a 24-bit field, so a
        // wider value would overflow the PDU serializer's `write_bits` assertion and panic the whole
        // cell (the entities run on one un-isolated stack thread). This is the single MM funnel for
        // every DGNA request — including unvalidated dashboard input — so reject an out-of-range
        // group identity here, before any state mutation or PDU build. (0 is the "no group"
        // sentinel used across the call/voice paths, so it is rejected too.)
        const MAX_TETRA_SSI: u32 = 0xFF_FFFF;
        if gssi == 0 || gssi > MAX_TETRA_SSI {
            tracing::warn!(
                "DGNA: refusing {} of out-of-range GSSI {} for ISSI {} (must be 1..=16777215)",
                verb,
                gssi,
                issi
            );
            self.emit_dgna_status(
                issi,
                gssi,
                attach,
                false,
                "MM",
                format!("Rejected: GSSI {} out of range (must be 1..=16777215)", gssi),
            );
            return false;
        }

        // The terminal must be registered on the cell â€” we cannot regroup a radio that is not here.
        if !self.client_mgr.client_is_known(issi) {
            tracing::warn!(
                "DGNA: ISSI {} is not registered on this cell â€” ignoring {} of GSSI {}",
                issi,
                verb,
                gssi
            );
            self.emit_dgna_status(
                issi,
                gssi,
                attach,
                false,
                "MM",
                format!("Rejected: ISSI {} is not registered on this cell", issi),
            );
            return false;
        }

        if attach && GroupIdentityAttachmentMode::try_from(attachment_mode as u64).is_err() {
            tracing::warn!(
                "DGNA: refusing assign of GSSI {} to ISSI {} with invalid attachment mode {}",
                gssi,
                issi,
                attachment_mode
            );
            self.emit_dgna_status(
                issi,
                gssi,
                attach,
                false,
                "MM",
                format!("Rejected: invalid attachment mode {}", attachment_mode),
            );
            return false;
        }

        if !attach && !is_dynamic && !is_attached {
            tracing::warn!("DGNA: refusing deassign of unknown GSSI {} from ISSI {}", gssi, issi);
            self.emit_dgna_status(
                issi,
                gssi,
                false,
                false,
                "MM",
                format!("Rejected: GSSI {} is neither attached nor in the DGNA registry", gssi),
            );
            return false;
        }

        // Apply to the MM client registry. client_group_attach also validates the GSSI is a legal
        // group address (range + is_group + may_attach); an invalid GSSI returns an error here.
        match self.client_mgr.client_group_attach(issi, gssi, attach) {
            Ok(_changed) => {
                // Mirror into the shared subscriber state used for local call/SDS routing and notify
                // CMCE. Done unconditionally on success so DGNA stays authoritative even if the BS
                // believed the affiliation already matched (no desync window â€” FH-BUG-022/025).
                if attach {
                    let mut state = self.config.state_write();
                    state.subscribers.remember_dgna_group(issi, gssi, mnemonic.clone(), attachment_mode);
                    state.subscribers.affiliate(issi, gssi);
                    self.emit_subscriber_update(queue, issi, vec![gssi], BrewSubscriberAction::Affiliate);
                } else {
                    let mut state = self.config.state_write();
                    state.subscribers.deaffiliate(issi, gssi);
                    if is_dynamic {
                        state.subscribers.forget_dgna_group(issi, gssi);
                    }
                    self.emit_subscriber_update(queue, issi, vec![gssi], BrewSubscriberAction::Deaffiliate);
                }
            }
            Err(e) => {
                tracing::warn!("DGNA: cannot {} GSSI {} on ISSI {}: {:?}", verb, gssi, issi, e);
                self.emit_dgna_status(issi, gssi, attach, false, "MM", format!("Rejected: {:?}", e));
                return false;
            }
        }

        self.emit_dgna_status(
            issi,
            gssi,
            attach,
            true,
            "MM",
            if attach {
                format!("Queued: GSSI {} assigned in MM state", gssi)
            } else if is_dynamic {
                format!("Queued: dynamic GSSI {} deassigned in MM state", gssi)
            } else {
                format!("Queued: static GSSI {} detached in MM state", gssi)
            },
        );

        // Put the regroup on the air. By default this is an SS-DGNA ASSIGN/DEASSIGN carried in a
        // CMCE D-FACILITY (TS 100 392-12-22 V1.5.1; transport EN 300 392-9 V1.7.1) - that SS PDU
        // both *defines* the group and attaches it (cl.6.5.2.1), which is what makes the dynamic
        // talkgroup appear and be selectable on a real terminal. The group attach/detach state and
        // affiliation above are unchanged; CMCE owns only the air-interface emission, so we hand the
        // request off over the Control SAP. The `dgna_use_ss_facility = false` rollback keeps the
        // legacy MM-only D-ATTACH/DETACH GROUP IDENTITY (EN 300 392-2 V2.4.1 cl.16.8), which only
        // toggles the L2 attachment of a group the radio already knows.
        if self.config.config().cell.dgna_use_ss_facility {
            queue.push_back(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mm,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::CmceSsDgnaAssign {
                    issi,
                    gssi,
                    mnemonic,
                    attachment_mode,
                    route_gssi_hint,
                    attach,
                },
            });
        } else {
            self.send_d_attach_detach_group_identity(queue, issi, gssi, attach);
        }

        // Persist for restart recovery (debounced) and refresh the dashboard with the full group set.
        self.recovery_mark_dirty();
        self.emit_group_snapshots(issi);

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
        let mut gid = GroupIdentityDownlink {
            group_identity_attachment: attach.then_some(GroupIdentityAttachment {
                // 0 = "attachment not needed" â†’ persistent on the MS until an explicit detach.
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
        self.address_groups_for(issi, std::slice::from_mut(&mut gid), &HashMap::new(), true);
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
        self.emit_dgna_status(
            issi,
            gssi,
            attach,
            true,
            "MM",
            format!(
                "Queued legacy MM D-ATTACH/DETACH GROUP IDENTITY ({})",
                if attach { "attach" } else { "detach" }
            ),
        );

        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle: 0, // unsolicited, BS-initiated â€” no inbound L2 handle to echo
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

    /// Handle U-ATTACH/DETACH GROUP IDENTITY ACKNOWLEDGEMENT â€” the terminal's reply to a BS-initiated
    /// D-ATTACH/DETACH GROUP IDENTITY (DGNA). BS-side group state is committed optimistically when the
    /// DGNA is issued, so this is confirmation/telemetry only: log the outcome.
    fn rx_u_attach_detach_group_identity_ack(&mut self, _queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let issi = prim.received_address.ssi;
        match UAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut prim.sdu) {
            Ok(pdu) => {
                tracing::info!("DGNA: ISSI {} acknowledged group identity change: {:?}", issi, pdu);
                if let Some(groups) = &pdu.group_identity_uplink {
                    for group in groups {
                        if let Some(gssi) = group.gssi {
                            let attach = group.class_of_usage.is_some();
                            self.emit_dgna_status(
                                issi,
                                gssi,
                                attach,
                                true,
                                "MM",
                                format!("Legacy MM ACK received ({})", if attach { "attach" } else { "detach" }),
                            );
                        }
                    }
                } else {
                    self.emit_dgna_status(issi, 0, true, true, "MM", "Legacy MM ACK received".to_string());
                }
            }
            Err(e) => {
                tracing::warn!(
                    "DGNA: failed parsing U-ATTACH/DETACH GROUP IDENTITY ACK from {}: {:?} {}",
                    issi,
                    e,
                    prim.sdu.dump_bin()
                );
                self.emit_dgna_status(issi, 0, true, false, "MM", format!("Legacy MM ACK parse failed: {:?}", e));
            }
        }
    }

    fn send_d_location_update_command(queue: &mut MessageQueue, issi: u32, handle: u32) {
        Self::send_d_location_update_command_options(queue, issi, handle, true, None, None);
    }

    fn send_d_location_update_command_options(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        group_identity_report: bool,
        ciphering: Option<tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters>,
        tx_reporter: Option<tetra_core::tx_receipt::TxReporter>,
    ) {
        let pdu = DLocationUpdateCommand {
            group_identity_report,
            cipher_control: ciphering.is_some(),
            ciphering_parameters: ciphering.map(|parameters| parameters.to_bits()),
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
                tx_reporter,
            }),
        };
        queue.push_back(msg);
    }

    /// Which case of clause 16.4.1.1 a migrating location update is. Migration needs Multi
    /// Network on and the cell broadcasting "migration supported" (`[cell_info] migration`,
    /// clause 18.5.2.1); the request has to carry the MNI of the MS (cases b) and c)).
    /// Whether a location update names an ITSI of another network (address extension, clause
    /// 16.10.1). Only with Multi Network on; off keeps the old handling.
    fn names_other_network(&self, address_extension: Option<u64>) -> bool {
        let cfg = self.config.config();
        if !cfg.multi_network.enabled {
            return false;
        }
        address_extension.is_some_and(|ext| {
            let mni = Mni::from_address_extension(ext);
            mni.mcc != cfg.net.mcc || mni.mnc != cfg.net.mnc
        })
    }

    fn migration_case(&self, address_extension: Option<u64>) -> Migration {
        let cfg = self.config.config();
        let mn = &cfg.multi_network;
        if !mn.enabled || !cfg.migration_supported() {
            return Migration::Refused;
        }
        let Some(ext) = address_extension else {
            return Migration::Refused;
        };
        let mni = Mni::from_address_extension(ext);
        if mni.mcc == cfg.net.mcc && mni.mnc == cfg.net.mnc {
            return Migration::Home;
        }
        // With a Brew server that offers `mni` the network has to be the server's own (a cell of
        // another network reached `via` it, whose radios come from the server's network) or one
        // it reaches and that is up (its `SERVICE_NETWORKS`), and, when `networks` is set, also
        // in that list. Without such a server only `networks` counts and visitors stay inside
        // this station.
        let server_ok = match &self.config.state_read().multi_network.server {
            Some((this, list)) => *this == mni || list.iter().any(|n| n.mni == mni && n.up),
            None => !mn.networks.is_empty(),
        };
        let listed = mn.networks.is_empty() || mn.networks.contains(&mni);
        if server_ok && listed {
            Migration::Visitor(mni)
        } else {
            tracing::info!("MM: migration from network {} refused: not a network this station accepts now", mni);
            Migration::Refused
        }
    }

    /// First step of case b) (clause 16.4.1.1): allocate a (V)ASSI and send it in a D-LOCATION
    /// UPDATE PROCEEDING with the MNI of the MS, on the USSI the request came on (clause
    /// 23.4.1.2.5). Nothing else changes: the radio registers with its second U-LOCATION UPDATE
    /// DEMAND on the (V)ASSI. No (V)ASSI left: D-LOCATION UPDATE REJECT with the MNI.
    fn start_identity_exchange(
        &mut self,
        queue: &mut MessageQueue,
        address: TetraAddress,
        handle: u32,
        pdu: &ULocationUpdateDemand,
        mni: Mni,
    ) {
        // "The USSI shall be equal to the ISSI used by the MS in its home network" (23.4.1.2.5);
        // the optional SSI element holds the same ISSI.
        let home_issi = pdu.ssi.map(|s| s as u32).unwrap_or(address.ssi);
        let cfg = self.config.config();
        let client_mgr = &self.client_mgr;
        let allocated = self.visitors.allocate(&cfg.multi_network, mni, home_issi, Instant::now(), |v| {
            client_mgr.client_is_known(v)
        });
        let Some((vassi, previous)) = allocated else {
            tracing::warn!("MM: no free (V)ASSI for visitor {}:{} — rejecting with Congestion", mni, home_issi);
            Self::send_d_location_update_reject_to(
                queue,
                address,
                handle,
                pdu.location_update_type,
                Some(mni.address_extension() as u64),
                RejectCause::Congestion,
                None,
            );
            return;
        };
        // The old (V)ASSI became invalid (clause 16.4.7.1): forget the registration on it.
        if let Some(old) = previous {
            self.config.state_write().multi_network.visitors.remove(&old);
            if let Some(client) = self.client_mgr.remove_client(old) {
                self.config.state_write().subscribers.deregister(old);
                if !client.groups.is_empty() {
                    let groups: Vec<u32> = client.groups.iter().copied().collect();
                    self.emit_subscriber_update(queue, old, groups, BrewSubscriberAction::Deaffiliate);
                }
                self.emit_subscriber_update(queue, old, Vec::new(), BrewSubscriberAction::Deregister);
                self.recovery_mark_dirty();
            }
        }
        tracing::info!(
            "MM: visitor {}:{} migrating ({:?}) — (V)ASSI {} in D-LOCATION UPDATE PROCEEDING",
            mni,
            home_issi,
            pdu.location_update_type,
            vassi
        );
        let response = DLocationUpdateProceeding {
            ssi: vassi,
            address_extension: mni.address_extension(),
            proprietary: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(56);
        response.to_bitbuf(&mut sdu).unwrap();
        sdu.seek(0);
        tracing::debug!("-> {:?} sdu {}", response, sdu.dump_bin());
        queue.push_back(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mm,
            dest: TetraEntity::Mle,
            msg: SapMsgInner::LmmMleUnitdataReq(LmmMleUnitdataReq {
                sdu,
                handle,
                address,
                layer2service: Layer2Service::Acknowledged,
                stealing_permission: false,
                stealing_repeats_flag: false,
                encryption_flag: false,
                is_null_pdu: false,
                tx_reporter: None,
            }),
        });
    }

    fn send_d_location_update_reject_cause(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        reject_cause: RejectCause,
    ) {
        Self::send_d_location_update_reject_ciphering(queue, issi, handle, location_update_type, address_extension, reject_cause, None);
    }

    /// D-LOCATION UPDATE REJECT that, for a ciphering refusal, carries the cell's preferred
    /// ciphering parameters (EN 300 392-7 clause 6.6.2.1.2).
    fn send_d_location_update_reject_ciphering(
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        reject_cause: RejectCause,
        preferred: Option<tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters>,
    ) {
        Self::send_d_location_update_reject_to(
            queue,
            TetraAddress::issi(issi),
            handle,
            location_update_type,
            address_extension,
            reject_cause,
            preferred,
        );
    }

    /// D-LOCATION UPDATE REJECT on a given layer 2 address (the USSI for a migrating radio,
    /// clause 16.4.1.1 b)).
    fn send_d_location_update_reject_to(
        queue: &mut MessageQueue,
        address: TetraAddress,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        reject_cause: RejectCause,
        preferred: Option<tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters>,
    ) {
        let pdu = DLocationUpdateReject {
            location_update_type,
            reject_cause: reject_cause as u8,
            cipher_control: preferred.is_some(),
            ciphering_parameters: preferred.map(|p| p.to_bits()),
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
                address,
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
        tracing::info!("MM: TEI received from ISSI {} â†’ TEI={} ({:060b})", issi, pdu.tei_hex(), pdu.tei,);

        // Store TEI in client state for future use (e.g. whitelist checking)
        if let Err(e) = self.client_mgr.set_client_tei(issi, pdu.tei) {
            tracing::warn!("MM: failed to store TEI for ISSI {}: {:?}", issi, e);
        }
    }

    fn feature_check_u_location_update_demand(pdu: &ULocationUpdateDemand) -> bool {
        let mut supported = true;
        // Migrating requests only get here as case c) of clause 16.4.1.1 (a radio of this network
        // coming back): refused ones and case b) are answered before this check.
        if pdu.location_update_type == LocationUpdateType::DisabledMsUpdating {
            unimplemented_log!("Unsupported {}", pdu.location_update_type);
            supported = false;
        }
        if pdu.request_to_append_la == true {
            unimplemented_log!("Unsupported request_to_append_la == true");
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

/// Explicit detach is immediate; an RF disappearance has no message, so challenge idle radios
/// and retain their registration throughout the full response grace period.
impl MmBs {
    fn registration_probe_call_busy(&mut self, issi: u32) -> bool {
        let groups = self
            .client_mgr
            .get_client_by_issi(issi)
            .map(|client| client.groups.clone())
            .unwrap_or_default();
        let state = self.config.state_read();
        state.registration_probe_busy_issis.contains(&issi)
            || state.active_call_ts.contains_key(&issi)
            || groups.iter().any(|group| state.active_call_ts.contains_key(group))
    }

    fn send_registration_check(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        presence_enabled: bool,
    ) -> Option<tetra_core::tx_receipt::TxReporter> {
        if presence_enabled {
            let ciphering = self.client_mgr.get_client_by_issi(issi).and_then(|client| client.ciphering);
            // A presence check must not detach all groups or change the agreed security class.
            let receipt = tetra_core::tx_receipt::TxReporter::new();
            Self::send_d_location_update_command_options(queue, issi, 0, false, ciphering, Some(receipt.clone()));
            Some(receipt)
        } else {
            Self::send_d_location_update_command(queue, issi, 0);
            None
        }
    }

    fn drive_registration_checks(&mut self, queue: &mut MessageQueue, ts: TdmaTime, now: Instant, rf_ready: bool) {
        let interval_secs = self.config.config().cell.periodic_registration_secs;
        let presence_secs = self.config.config().cell.presence_check_secs;
        let response_grace_secs = if presence_secs == 0 {
            60
        } else {
            self.config.config().cell.presence_response_secs
        };
        let local_stall = self
            .registration_check_last_tick
            .is_some_and(|previous| now.saturating_duration_since(previous) >= Duration::from_secs(5));
        self.registration_check_last_tick = Some(now);
        if !rf_ready {
            self.registration_checks_rf_ready = false;
            return;
        }
        if !self.registration_checks_rf_ready || local_stall {
            // Local RF downtime is not proof that any remote radio has gone away.
            self.client_mgr.resume_registration_checks();
            self.registration_checks_rf_ready = true;
            self.registration_command_last_frame = None;
            self.registration_probe_receipts.clear();
        }
        for issi in self.registration_probe_receipts.keys().copied().collect::<Vec<_>>() {
            let receipt = &self.registration_probe_receipts[&issi];
            if !self.client_mgr.is_pending_command(issi) {
                self.registration_probe_receipts.remove(&issi);
            } else if self.client_mgr.heard_since_registration_command(issi) || receipt.is_acknowledged() {
                // An ACK or a decoded uplink proves presence.
                self.client_mgr.reset_registration_timer_at(issi, now);
                self.registration_probe_receipts.remove(&issi);
            } else if receipt.has_been_transmitted() {
                self.client_mgr.arm_registration_response_grace(issi, response_grace_secs, now);
            } else if self.client_mgr.get_client_by_issi(issi).is_some_and(|client| {
                client.grace_expires_at.is_none()
                    && client.command_sent_at.is_some_and(|queued| now.saturating_duration_since(queued) >= Duration::from_secs(15))
            }) {
                // LLC can retry a locally discarded message. Once that retry/queue window is
                // stale, start a fresh observation window instead of leaving the probe pending
                // forever. No over-air transmission means no evidence that the radio is absent.
                self.client_mgr.reset_registration_timer_at(issi, now);
                self.registration_probe_receipts.remove(&issi);
                tracing::debug!("MM: ISSI {} registration check was not transmitted; deferring without marking unavailable", issi);
            }
        }
        let mut expired = self.client_mgr.collect_registration_checks(interval_secs, presence_secs, now);
        expired.sort_unstable();
        for issi in expired {
            if self.recovery_attempts.contains_key(&issi) || self.auth_pending.contains_key(&issi) || self.otar_pending.contains_key(&issi)
            {
                continue;
            }
            // Group listeners are not necessarily listening to MCCH, even if they never PTT.
            // Include individual setup/ringing, not only established traffic channels.
            if self.registration_probe_call_busy(issi) {
                self.client_mgr.reset_registration_timer_at(issi, now);
                continue;
            }
            if self.client_mgr.is_pending_command(issi) {
                let heard = if presence_secs != 0 {
                    self.client_mgr.heard_since_registration_command(issi)
                } else {
                    self.client_mgr.heard_on_air_within(issi, Duration::from_secs(interval_secs as u64))
                };
                if heard {
                    self.client_mgr.reset_registration_timer_at(issi, now);
                    continue;
                }
                // No valid uplink or location update after the challenge. Preserve the client's
                // affiliations for coverage return, but stop advertising it as reachable.
                if self.config.state_read().subscribers.is_registered(issi) {
                    let groups = self
                        .client_mgr
                        .get_client_by_issi(issi)
                        .map(|client| client.groups.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    if !groups.is_empty() {
                        self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                    }
                    self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                    self.config.state_write().subscribers.deregister(issi);
                    if let Some(sink) = &self.telemetry {
                        sink.send(crate::net_telemetry::TelemetryEvent::MsTimeoutDrop { issi });
                    }
                    tracing::info!(
                        "MM: ISSI {} did not answer the registration presence check; marking unavailable",
                        issi
                    );
                }
                self.client_mgr.reset_registration_timer_at(issi, now);
                self.recovery_mark_dirty();
                continue;
            }
            // Respect energy-economy monitoring windows. Bound a stale window to a six-second
            // scheduling wait, and start the response grace only after MAC confirms transmission.
            const EE_WINDOW_WAIT_SECS: u64 = 6;
            let frame = ts.to_int() / 4;
            if self.registration_command_last_frame == Some(frame) {
                continue;
            }
            if self
                .client_mgr
                .should_send_registration_command_now(issi, ts, interval_secs, presence_secs, EE_WINDOW_WAIT_SECS, now)
            {
                if net_brew::is_site_linked(&self.config) {
                    // Multi-cell: within its location area the radio may have changed cell
                    // without registering (TS 100 392-2 clause 18.3.4.7.1a); the site switch
                    // has the area's other cells ask too.
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Mm,
                        dest: TetraEntity::Brew,
                        msg: SapMsgInner::CmceCallControl(CallControl::SitePresenceProbe { issi }),
                    });
                }
                if let Some(receipt) = self.send_registration_check(queue, issi, presence_secs != 0) {
                    self.client_mgr.queue_registration_command(issi, now);
                    self.registration_probe_receipts.insert(issi, receipt);
                } else {
                    self.client_mgr.set_pending_command(issi, response_grace_secs);
                }
                self.registration_command_last_frame = Some(frame);
            }
        }
    }
}

impl TetraEntityTrait for MmBs {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Mm
    }

    fn set_config(&mut self, config: SharedConfig) {
        // Re-apply the [security] registry bounds so a config swap can't leave the caps behind.
        let sec = config.config();
        self.client_mgr
            .set_limits(sec.security.max_registered_clients, sec.security.registration_rate_limit_per_min);
        self.config = config;
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.expire_authentication();
        self.expire_otar();
        // Drain control commands addressed to the MM entity. We collect into a Vec first so the
        // immutable borrow on `self.control` is released before the handlers run â€” DGNA needs
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
                    ControlCommand::Dgna {
                        issi,
                        gssi,
                        mnemonic,
                        attachment_mode,
                        attach,
                    } => {
                        self.do_dgna(queue, issi, gssi, mnemonic, attachment_mode, attach);
                    }
                    ControlCommand::OtarSck { issi } => {
                        self.otar_sck_provide(queue, issi, "dashboard");
                    }
                    _ => {
                        tracing::warn!("MM: ignoring unsupported control command {:?}", cmd);
                    }
                }
            }
        }

        let rf_ready = crate::rf_status::get_all()
            .into_iter()
            .any(|(cell, status)| cell == self.config.config().cell_id && status.state == crate::rf_status::RfState::Online);
        self.drive_registration_checks(queue, ts, Instant::now(), rf_ready);

        // Restart recovery: replay D-LOCATION-UPDATE-COMMANDs to cached terminals awaiting
        // re-registration (no-op once the sweep drains / when recovery is disabled), then flush
        // the cache if dirty + debounce elapsed.
        self.drive_recovery_replay(queue, ts);
        self.recovery_maybe_flush();

        // Republish the per-MS energy-economy monitoring windows into shared state every tick, from
        // the authoritative client registry, so the downlink scheduler (CMCE/SDS) can gate
        // unsolicited traffic to a sleeping MS's wake window without reading stale data. Rebuilt
        // wholesale (like CMCE's active_call_ts) â€” empty when no MS is in energy economy.
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
                    SapMsgInner::MmRequestLocationUpdate { issi } => {
                        self.rx_request_location_update(queue, issi);
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
                    SapMsgInner::MmSubscriberUpdate(update)
                        if message.src == TetraEntity::Brew && update.action == BrewSubscriberAction::Deregister =>
                    {
                        // Multi-cell site switch: the MS registered on a sibling cell. Drop it here
                        // silently — it no longer listens to this cell, so nothing goes on air.
                        let issi = update.issi;
                        tracing::info!("MM: ISSI {} moved to another cell — dropping local registration", issi);
                        let groups: Vec<u32> = self
                            .client_mgr
                            .get_client_by_issi(issi)
                            .map(|c| c.groups.iter().copied().collect())
                            .unwrap_or_default();
                        if !groups.is_empty() {
                            self.emit_subscriber_update(queue, issi, groups, BrewSubscriberAction::Deaffiliate);
                        }
                        self.emit_subscriber_update(queue, issi, Vec::new(), BrewSubscriberAction::Deregister);
                        self.client_mgr.remove_client_quiet(issi);
                        self.config.state_write().subscribers.deregister(issi);
                        self.group_report_requested_at.remove(&issi);
                        self.attach_overflow_remainder.remove(&issi);
                        self.recovery_mark_dirty();
                    }
                    SapMsgInner::MmSubscriberUpdate(update) => {
                        // CMCE can ask MM to deregister an MS (e.g. kick from dashboard)
                        if update.action == BrewSubscriberAction::Deregister {
                            let issi = update.issi;
                            tracing::info!(
                                "MM: kicking ISSI {} â€” sending D-LOCATION-UPDATE-COMMAND to force re-registration",
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
                            // handle â€” that root cause is still open.)
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
                            self.group_report_requested_at.remove(&issi);
                            self.attach_overflow_remainder.remove(&issi);
                            self.recovery_mark_dirty();
                        }
                    }
                    SapMsgInner::MmDgnaRequest {
                        issi,
                        gssi,
                        mnemonic,
                        attachment_mode,
                        attach,
                    } => {
                        // Dashboard-originated DGNA, forwarded by CMCE (the dashboard control channel
                        // terminates there). The group machinery lives here in MM.
                        self.do_dgna(queue, issi, gssi, mnemonic, attachment_mode, attach);
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
    /// Selective SwMI-initiated LU for one ISSI (ETSI EN 300 392-2 §16.4.4 / TIP interrogation).
    /// Rate-limited with the same cooldown as reactive recovery so a burst of failed Brew setups
    /// does not flood the MCCH. Unlike reactive recovery, the ISSI may already be known locally.
    fn rx_request_location_update(&mut self, queue: &mut MessageQueue, issi: u32) {
        let cfg = self.config.config();
        let rec = &cfg.recovery;
        let cooldown = std::time::Duration::from_secs(rec.reactive_cooldown_secs);
        let now = std::time::Instant::now();
        if let Some(&last) = self.reactive_recovery_cooldown.get(&issi) {
            if now.duration_since(last) < cooldown {
                tracing::debug!(
                    "mm_bs: MmRequestLocationUpdate issi={} suppressed (cooldown)",
                    issi
                );
                return;
            }
        }
        if self.reactive_recovery_cooldown.len() >= REACTIVE_RECOVERY_COOLDOWN_CAP {
            self.reactive_recovery_cooldown.retain(|_, t| now.duration_since(*t) < cooldown);
        }
        self.reactive_recovery_cooldown.insert(issi, now);

        tracing::info!(
            "mm_bs: selective D-LOCATION-UPDATE-COMMAND for ISSI {} (Brew soft-recovery / on-demand)",
            issi
        );
        Self::send_d_location_update_command(queue, issi, 0);
    }
}

#[cfg(test)]
mod ee_tests {
    use super::*;
    use tetra_core::TdmaTime;

    #[test]
    fn grant_energy_saving_produces_spec_valid_start_point() {
        // ETSI Table 16.40: Frame Number âˆˆ 1..=18, Multiframe Number âˆˆ 1..=60 (MN=0 is reserved
        // ONLY for StayAlive). The old (issi/18)%cycle formula produced MN=0 for half the radios â€”
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
        // StayAlive â†’ no monitoring window.
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

// ─────────────────────── Air-interface authentication (EN 300 392-7) ───────────────────────
impl MmBs {
    /// Decide whether a registering radio must authenticate first. Returns true when the
    /// registration has been taken over (challenge sent and the message held, or rejected).
    fn authentication_gate(
        &mut self,
        queue: &mut MessageQueue,
        issi: u32,
        handle: u32,
        location_update_type: LocationUpdateType,
        address_extension: Option<u64>,
        held: SapMsg,
    ) -> bool {
        let (mode, k) = {
            let cfg = self.config.config();
            // A visitor's K lives in its home network (authentication at migration is not
            // implemented): it is a radio without a key here.
            let k = if self.visitors.is_visitor(issi) { None } else { cfg.security.subscriber_k(issi).copied() };
            (cfg.security.authentication, k)
        };
        if mode == AuthenticationMode::Off {
            return false;
        }
        // Just authenticated: let the replayed registration through.
        if self.auth_passed.get(&issi).is_some_and(|t| t.elapsed() < AUTH_PASS_GRACE) {
            self.auth_passed.remove(&issi);
            return false;
        }
        let Some(k) = k else {
            if mode == AuthenticationMode::Required {
                tracing::warn!("MM: ISSI {} has no authentication key and authentication is required — rejecting registration", issi);
                Self::send_d_location_update_reject_cause(queue, issi, handle, location_update_type, address_extension, RejectCause::IllegalMs);
                return true;
            }
            tracing::debug!("MM: ISSI {} has no authentication key — registering unauthenticated (authentication = optional)", issi);
            return false;
        };
        // A challenge is already out: keep the newest registration to replay, don't re-challenge.
        if let Some(p) = self.auth_pending.get_mut(&issi)
            && p.started.elapsed() < T354
        {
            p.held = Some(held);
            return true;
        }
        let challenge = MsChallenge::new(&AuthKey(k));
        let pdu = DAuthenticationDemand { rand1: challenge.rand1, rs: challenge.rs, proprietary: None };
        tracing::info!("MM: challenging ISSI {} (D-AUTHENTICATION DEMAND)", issi);
        self.send_auth_pdu(queue, issi, handle, "DAuthenticationDemand", |b| pdu.to_bitbuf(b));
        self.auth_pending.insert(issi, PendingAuth { challenge, handle, held: Some(held), started: Instant::now() });
        true
    }

    /// U-AUTHENTICATION: dispatch on the sub-type that follows the PDU type.
    fn rx_u_authentication(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let issi = prim.received_address.ssi;
        let handle = prim.handle;
        let Some(sub_type) = peek_sub_type(&prim.sdu) else {
            tracing::warn!("MM: truncated U-AUTHENTICATION from ISSI {}", issi);
            return;
        };
        match sub_type {
            AuthenticationSubType::Response => match UAuthenticationResponse::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_authentication_response(queue, issi, handle, pdu),
                Err(e) => tracing::warn!("MM: bad U-AUTHENTICATION RESPONSE from ISSI {}: {:?}", issi, e),
            },
            AuthenticationSubType::Demand => match UAuthenticationDemand::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_authentication_demand(queue, issi, handle, pdu),
                Err(e) => tracing::warn!("MM: bad U-AUTHENTICATION DEMAND from ISSI {}: {:?}", issi, e),
            },
            AuthenticationSubType::Result => match UAuthenticationResult::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_authentication_result(queue, issi, handle, pdu),
                Err(e) => tracing::warn!("MM: bad U-AUTHENTICATION RESULT from ISSI {}: {:?}", issi, e),
            },
            AuthenticationSubType::Reject => match UAuthenticationReject::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_authentication_reject(queue, issi, handle, pdu),
                Err(e) => tracing::warn!("MM: bad U-AUTHENTICATION REJECT from ISSI {}: {:?}", issi, e),
            },
        }
    }

    /// The radio answers our challenge (clause 4.1.2), possibly adding its own (4.1.4).
    /// Tell the dashboard (and other telemetry consumers) that this radio proved its K.
    fn notify_authenticated(&self, issi: u32) {
        if let Some(sink) = &self.telemetry {
            sink.send(crate::net_telemetry::TelemetryEvent::MsSecurity { issi, authenticated: Some(true), encrypting: None });
        }
    }

    fn rx_u_authentication_response(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32, pdu: UAuthenticationResponse) {
        let Some(pending) = self.auth_pending.remove(&issi) else {
            tracing::warn!("MM: U-AUTHENTICATION RESPONSE from ISSI {} but no challenge is outstanding — ignored", issi);
            return;
        };
        let dck1 = pending.challenge.verify(&pdu.res1);
        let r1 = dck1.is_some();
        // Mutual: the radio challenged us with RAND2 under the same RS.
        let (res2, dck2) = match (pdu.rand2, self.subscriber_key(issi)) {
            (Some(rand2), Some(k)) => {
                let r = answer_ms_challenge(&k, &pending.challenge.rs, &rand2);
                (Some(r.res2), Some(r.dck2))
            }
            _ => (None, None),
        };
        let result = DAuthenticationResult { r1, res2, proprietary: None };
        self.send_auth_pdu(queue, issi, handle, "DAuthenticationResult", |b| result.to_bitbuf(b));
        if r1 {
            tracing::info!("MM: ISSI {} authenticated{}", issi, if res2.is_some() { " (mutual)" } else { "" });
            self.dck.insert(issi, derive_dck(dck1.as_ref(), dck2.as_ref()).0);
            self.auth_passed.insert(issi, Instant::now());
            self.notify_authenticated(issi);
            if let Some(held) = pending.held {
                self.rx_u_location_update_demand(queue, held);
            }
        } else {
            tracing::warn!("MM: ISSI {} FAILED authentication (wrong RES1) — registration rejected", issi);
            if let Some(held) = pending.held {
                self.reject_held_registration(queue, issi, handle, held);
            }
        }
    }

    /// The radio challenges us (clause 4.1.3): answer with RES2 and, if configured, challenge
    /// back so the exchange becomes mutual.
    fn rx_u_authentication_demand(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32, pdu: UAuthenticationDemand) {
        let mutual = self.config.config().security.mutual_authentication;
        let Some(k) = self.subscriber_key(issi) else {
            tracing::info!("MM: ISSI {} asked us to authenticate but has no key configured — D-AUTHENTICATION REJECT", issi);
            let reject = DAuthenticationReject { reject_reason: AuthenticationRejectReason::AuthenticationNotSupported };
            self.send_auth_pdu(queue, issi, handle, "DAuthenticationReject", |b| reject.to_bitbuf(b));
            return;
        };
        let rs = random_nonce();
        let answer = answer_ms_challenge(&k, &rs, &pdu.rand2);
        let rand1 = if mutual {
            let challenge = MsChallenge::with_nonces(&k, rs, random_nonce());
            let rand1 = challenge.rand1;
            self.auth_pending.insert(issi, PendingAuth { challenge, handle, held: None, started: Instant::now() });
            Some(rand1)
        } else {
            self.dck.insert(issi, derive_dck(None, Some(&answer.dck2)).0);
            None
        };
        tracing::info!("MM: answering ISSI {}'s challenge (D-AUTHENTICATION RESPONSE{})", issi, if mutual { ", mutual" } else { "" });
        let response = DAuthenticationResponse { rs, res2: answer.res2, rand1, proprietary: None };
        self.send_auth_pdu(queue, issi, handle, "DAuthenticationResponse", |b| response.to_bitbuf(b));
    }

    /// The radio's verdict on our RES2 and, in a mutual exchange it started, its RES1.
    fn rx_u_authentication_result(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32, pdu: UAuthenticationResult) {
        if !pdu.r2 {
            tracing::warn!("MM: ISSI {} reports our authentication FAILED (R2 = false) — its K does not match ours", issi);
        }
        let Some(res1) = pdu.res1 else {
            return;
        };
        let Some(pending) = self.auth_pending.remove(&issi) else {
            tracing::warn!("MM: U-AUTHENTICATION RESULT with RES1 from ISSI {} but no challenge is outstanding", issi);
            return;
        };
        let dck1 = pending.challenge.verify(&res1);
        let r1 = dck1.is_some();
        let result = DAuthenticationResult { r1, res2: None, proprietary: None };
        self.send_auth_pdu(queue, issi, handle, "DAuthenticationResult", |b| result.to_bitbuf(b));
        if r1 {
            tracing::info!("MM: ISSI {} authenticated (mutual, radio-initiated)", issi);
            self.dck.insert(issi, derive_dck(dck1.as_ref(), None).0);
            self.auth_passed.insert(issi, Instant::now());
            self.notify_authenticated(issi);
        } else {
            tracing::warn!("MM: ISSI {} FAILED our challenge in a radio-initiated exchange", issi);
        }
    }

    /// The radio refuses to authenticate.
    fn rx_u_authentication_reject(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32, pdu: UAuthenticationReject) {
        let mode = self.config.config().security.authentication;
        tracing::warn!("MM: ISSI {} rejected authentication ({:?})", issi, pdu.reject_reason);
        let Some(pending) = self.auth_pending.remove(&issi) else {
            return;
        };
        if let Some(held) = pending.held {
            if mode == AuthenticationMode::Required {
                self.reject_held_registration(queue, issi, handle, held);
            } else {
                tracing::info!("MM: ISSI {} registers unauthenticated (authentication = optional)", issi);
                self.auth_passed.insert(issi, Instant::now());
                self.rx_u_location_update_demand(queue, held);
            }
        }
    }

    /// Give up on exchanges that have outrun T354 and forget stale passes.
    fn expire_authentication(&mut self) {
        let expired: Vec<u32> = self.auth_pending.iter().filter(|(_, p)| p.started.elapsed() > T354).map(|(i, _)| *i).collect();
        for issi in expired {
            tracing::warn!("MM: authentication of ISSI {} timed out (T354) — no response to the challenge", issi);
            self.auth_pending.remove(&issi);
        }
        self.auth_passed.retain(|_, t| t.elapsed() < AUTH_PASS_GRACE);
    }

    fn subscriber_key(&self, issi: u32) -> Option<AuthKey> {
        self.config.config().security.subscriber_k(issi).map(|k| AuthKey(*k))
    }

    // ───────────── OTAR: static cipher key delivery (EN 300 392-7 clause 4.5.2) ─────────────

    fn notify_otar(&self, issi: u32, sckn: u8, sck_vn: u16, status: &str) {
        if let Some(sink) = &self.telemetry {
            sink.send(crate::net_telemetry::TelemetryEvent::OtarSck { issi, sckn, sck_vn, status: status.to_string() });
        }
    }

    /// Send the cell's SCK to one radio, sealed under a fresh session key derived from the
    /// radio's K (clause 4.5.2.2): KSO = TA41(K, RSO), SSCK = TA51(SCK, SCK-VN, KSO, SCKN).
    /// Works whether or not the cell runs class 2 yet (`class = 1` keeps the key staged), so a fleet
    /// can be keyed before encryption is switched on.
    fn otar_sck_provide(&mut self, queue: &mut MessageQueue, issi: u32, why: &str) {
        let cfg = self.config.config();
        let Some(aie) = cfg.security.aie.as_ref() else {
            tracing::warn!("MM: OTAR to ISSI {} ({}) refused — no SCK configured under [security.aie]", issi, why);
            self.notify_otar(issi, 0, 0, "no SCK configured");
            return;
        };
        let Some(k) = self.subscriber_key(issi) else {
            tracing::warn!("MM: OTAR to ISSI {} ({}) refused — no authentication key K on file for it", issi, why);
            self.notify_otar(issi, aie.sckn, aie.sck_vn, "no K on file for this radio");
            return;
        };
        if !self.client_mgr.client_is_known(issi) {
            tracing::warn!("MM: OTAR to ISSI {} ({}) — radio is not registered on this cell", issi, why);
            self.notify_otar(issi, aie.sckn, aie.sck_vn, "radio not registered");
            return;
        }
        let rso = random_nonce();
        let kso = ta41(&k.0, &rso);
        // SCKN is 0-based on air (clause A.8.70: value 0 = SCK number 1).
        let sckn_air = aie.sckn.saturating_sub(1) & 0x1f;
        let ssck = ta51(&aie.sck.0, aie.sck_vn, &kso, sckn_air);
        let pdu = DOtarSckProvide {
            ack_required: true,
            explicit_response: true,
            max_response_timer: 0,
            session_key: OtarSessionKey::Individual { rso },
            keys: vec![SckKeyAndIdentifier { sckn: sckn_air, sck_vn: aie.sck_vn, dmo: false, ssck }],
            ksg_number: aie.ksg.saturating_sub(1),
            otar_retry_interval: 0,
            address_extension: None,
            proprietary: None,
        };
        tracing::info!("MM: OTAR ({}) — sending {} SCK {} v{} to ISSI {}", why, format!("TEA{}", aie.ksg), aie.sckn, aie.sck_vn, issi);
        self.send_auth_pdu(queue, issi, 0, "DOtarSckProvide", |b| pdu.to_bitbuf(b));
        self.otar_pending.insert(issi, PendingOtar { sckn: aie.sckn, sck_vn: aie.sck_vn, started: Instant::now() });
        self.notify_otar(issi, aie.sckn, aie.sck_vn, "sent");
    }

    fn rx_u_otar(&mut self, queue: &mut MessageQueue, mut message: SapMsg) {
        let SapMsgInner::LmmMleUnitdataInd(prim) = &mut message.msg else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        let issi = prim.received_address.ssi;
        let handle = prim.handle;
        let Some(sub_type) = otar_sck::peek_sub_type(&prim.sdu) else {
            tracing::warn!("MM: truncated U-OTAR from ISSI {}", issi);
            return;
        };
        match sub_type {
            OtarSubType::SckDemandProvide => match UOtarSckDemand::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_otar_sck_demand(queue, issi, handle, pdu),
                Err(e) => tracing::warn!("MM: bad U-OTAR SCK DEMAND from ISSI {}: {:?}", issi, e),
            },
            OtarSubType::SckResultReject => match UOtarSckResult::from_bitbuf(&mut prim.sdu) {
                Ok(pdu) => self.rx_u_otar_sck_result(issi, pdu),
                Err(e) => tracing::warn!("MM: bad U-OTAR SCK RESULT from ISSI {}: {:?}", issi, e),
            },
            other => tracing::info!("MM: U-OTAR {} from ISSI {} not supported (only SCK OTAR is implemented)", other, issi),
        }
    }

    /// The radio asks for SCK(s) by number (clause 4.5.2.1): provide ours if it is among
    /// them, otherwise reject each requested number.
    fn rx_u_otar_sck_demand(&mut self, queue: &mut MessageQueue, issi: u32, handle: u32, pdu: UOtarSckDemand) {
        let cfg = self.config.config();
        let ours = cfg.security.aie.as_ref().map(|a| (a.sckn.saturating_sub(1) & 0x1f, a.ksg.saturating_sub(1)));
        tracing::info!(
            "MM: ISSI {} requests SCK(s) {:?} for KSG {}",
            issi,
            pdu.sckns.iter().map(|n| n + 1).collect::<Vec<_>>(),
            pdu.ksg_number
        );
        match ours {
            Some((sckn_air, ksg)) if pdu.sckns.contains(&sckn_air) && pdu.ksg_number == ksg && self.subscriber_key(issi).is_some() => {
                self.otar_sck_provide(queue, issi, "radio request");
            }
            _ => {
                let reason = match ours {
                    Some((_, ksg)) if pdu.ksg_number != ksg => OtarRejectReason::KsgNotSupported,
                    Some(_) if self.subscriber_key(issi).is_none() => OtarRejectReason::InvalidAddress,
                    Some(_) => OtarRejectReason::InvalidKeyNumber,
                    None => OtarRejectReason::KeyNotAvailable,
                };
                let rejected: Vec<(u8, OtarRejectReason)> = pdu.sckns.iter().take(7).map(|n| (*n, reason)).collect();
                if rejected.is_empty() {
                    return;
                }
                tracing::info!("MM: rejecting SCK request from ISSI {}: {:?}", issi, reason);
                let reject = DOtarSckReject { rejected, otar_retry_interval: 0, address_extension: None };
                self.send_auth_pdu(queue, issi, handle, "DOtarSckReject", |b| reject.to_bitbuf(b));
            }
        }
    }

    /// The radio reports whether it could unseal and store the key.
    fn rx_u_otar_sck_result(&mut self, issi: u32, pdu: UOtarSckResult) {
        let pending = self.otar_pending.remove(&issi);
        for r in &pdu.results {
            let sckn = r.sckn + 1;
            let vn = pending.as_ref().map(|p| p.sck_vn).unwrap_or(0);
            match r.result {
                ProvisionResult::Accepted => {
                    tracing::info!("MM: ISSI {} accepted SCK {} (version {})", issi, sckn, vn);
                    self.notify_otar(issi, sckn, vn, "accepted");
                }
                other => {
                    let detail = match r.current_sck_vn {
                        Some(cur) => format!("{} (radio holds version {})", other.describe(), cur),
                        None => other.describe().to_string(),
                    };
                    tracing::warn!("MM: ISSI {} did not take SCK {}: {}", issi, sckn, detail);
                    self.notify_otar(issi, sckn, vn, &format!("failed: {detail}"));
                }
            }
        }
    }

    fn expire_otar(&mut self) {
        let expired: Vec<(u32, u8, u16)> = self
            .otar_pending
            .iter()
            .filter(|(_, p)| p.started.elapsed() > T_OTAR_RESULT)
            .map(|(i, p)| (*i, p.sckn, p.sck_vn))
            .collect();
        for (issi, sckn, vn) in expired {
            tracing::warn!("MM: no U-OTAR SCK RESULT from ISSI {} for SCK {} within {:?}", issi, sckn, T_OTAR_RESULT);
            self.otar_pending.remove(&issi);
            self.notify_otar(issi, sckn, vn, "timeout: no result from the radio");
        }
    }

    /// Reject the registration that was held for an authentication that failed.
    fn reject_held_registration(&self, queue: &mut MessageQueue, issi: u32, handle: u32, held: SapMsg) {
        let SapMsgInner::LmmMleUnitdataInd(prim) = held.msg else {
            return;
        };
        let mut sdu = prim.sdu;
        let (lut, addr) = match ULocationUpdateDemand::from_bitbuf(&mut sdu) {
            Ok(p) => (p.location_update_type, p.address_extension),
            Err(_) => (LocationUpdateType::RoamingLocationUpdating, None),
        };
        Self::send_d_location_update_reject_cause(queue, issi, handle, lut, addr, RejectCause::IllegalMs);
    }

    /// Send one MM PDU to a radio over the acknowledged L2 service.
    fn send_auth_pdu<F>(&self, queue: &mut MessageQueue, issi: u32, handle: u32, what: &str, write: F)
    where
        F: FnOnce(&mut BitBuffer) -> Result<(), tetra_core::pdu_parse_error::PduParseErr>,
    {
        let mut sdu = BitBuffer::new_autoexpand(256);
        if let Err(e) = write(&mut sdu) {
            tracing::error!("MM: failed to encode {} for ISSI {}: {:?}", what, issi, e);
            return;
        }
        sdu.seek(0);
        tracing::debug!("-> {} to ISSI {}", what, issi);
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

#[cfg(test)]
mod presence_tests {
    use super::*;
    use crate::net_telemetry::{TelemetryEvent, channel::telemetry_channel};

    fn station() -> (MmBs, crate::net_telemetry::channel::TelemetrySource) {
        station_linked(false)
    }

    fn station_linked(site_linked: bool) -> (MmBs, crate::net_telemetry::channel::TelemetrySource) {
        let mut cfg = tetra_config::bluestation::from_toml_str(include_str!("../../../../example_config/config.toml")).unwrap();
        cfg.cell.periodic_registration_secs = 3600;
        cfg.cell.presence_check_secs = 30;
        cfg.cell.presence_response_secs = 60;
        cfg.site_linked = site_linked;
        let shared = SharedConfig::from_parts(cfg, None);
        let (sink, events) = telemetry_channel();
        let mut mm = MmBs::new(shared, Some(sink), None);
        mm.registration_checks_rf_ready = true;
        mm.registration_check_last_tick = Some(Instant::now());
        (mm, events)
    }

    fn radio(mm: &mut MmBs, issi: u32) {
        mm.client_mgr.try_register_client(issi, true).unwrap();
        mm.config.state_write().subscribers.register(issi);
        mm.client_mgr.client_group_attach(issi, 268, true).unwrap();
    }

    fn actions(queue: &mut MessageQueue) -> Vec<BrewSubscriberAction> {
        let mut result = Vec::new();
        while let Some(message) = queue.pop_front() {
            if let SapMsgInner::MmSubscriberUpdate(update) = message.msg {
                result.push(update.action);
            }
        }
        result
    }

    #[test]
    fn unanswered_check_drops_presence_once_and_preserves_identity_and_groups() {
        let (mut mm, events) = station();
        radio(&mut mm, 100);
        while events.try_recv().is_some() {}
        mm.client_mgr.set_pending_command(100, 0);
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(!mm.config.state_read().subscribers.is_registered(100));
        assert!(actions(&mut queue).contains(&BrewSubscriberAction::Deregister));
        assert!(matches!(events.try_recv(), Some(TelemetryEvent::MsTimeoutDrop { issi: 100 })));
        assert!(mm.client_mgr.get_client_by_issi(100).unwrap().groups.contains(&268));
        assert_eq!(mm.client_mgr.snapshot_for_recovery()[0].1, vec![268]);
        mm.client_mgr.set_pending_command(100, 0);
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(actions(&mut queue).is_empty());
        assert!(events.try_recv().is_none());
    }

    #[test]
    fn decoded_uplink_after_probe_keeps_a_quiet_registered_radio_online() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        mm.client_mgr.set_pending_command(100, 0);
        mm.client_mgr.update_client_rssi(100, -45.0);
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        assert!(!mm.client_mgr.is_pending_command(100));
        assert!(actions(&mut queue).is_empty());
    }

    #[test]
    fn group_listeners_and_ringing_private_participants_are_not_dropped() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        radio(&mut mm, 101);
        mm.config.state_write().active_call_ts.insert(268, (1521, 2, 1));
        mm.client_mgr.set_pending_command(100, 0);
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        mm.config.state_write().active_call_ts.clear();
        mm.config.state_write().registration_probe_busy_issis.insert(101);
        mm.client_mgr.set_pending_command(101, 0);
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(mm.config.state_read().subscribers.is_registered(101));
        assert!(actions(&mut queue).is_empty());
    }

    #[test]
    fn unavailable_rf_and_local_stalls_restart_checks_without_offline_events() {
        let (mut mm, events) = station();
        radio(&mut mm, 100);
        while events.try_recv().is_some() {}
        mm.client_mgr.set_pending_command(100, 0);
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), false);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(!mm.client_mgr.is_pending_command(100));
        mm.client_mgr.set_pending_command(100, 0);
        mm.registration_check_last_tick = Some(Instant::now() - Duration::from_secs(20));
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        assert!(!mm.client_mgr.is_pending_command(100));
        assert!(events.try_recv().is_none());
        assert!(actions(&mut queue).is_empty());
    }

    #[test]
    fn presence_probe_preserves_group_attachment_and_ciphering_parameters() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        let cipher = tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters::class2(2, 3);
        mm.client_mgr.set_client_ciphering(100, Some(cipher)).unwrap();
        let mut queue = MessageQueue::new();
        let _ = mm.send_registration_check(&mut queue, 100, true);
        let message = queue.pop_front().unwrap();
        let SapMsgInner::LmmMleUnitdataReq(mut request) = message.msg else {
            panic!("presence command missing")
        };
        assert_eq!(
            request.sdu.read_field(4, "pdu_type").unwrap(),
            tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl::DLocationUpdateCommand.into_raw()
        );
        assert_eq!(request.sdu.read_field(1, "group_report").unwrap(), 0);
        assert_eq!(request.sdu.read_field(1, "cipher_control").unwrap(), 1);
        assert_eq!(request.sdu.read_field(10, "ciphering_parameters").unwrap(), cipher.to_bits());
        assert!(mm.client_mgr.get_client_by_issi(100).unwrap().groups.contains(&268));
    }

    #[test]
    fn new_presence_probes_are_limited_to_one_per_tdma_frame() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        radio(&mut mm, 101);
        let now = Instant::now() + Duration::from_secs(31);
        mm.registration_check_last_tick = Some(now - Duration::from_secs(1));
        let ts = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, ts, now, true);
        assert!(queue.pop_front().is_some());
        assert!(queue.pop_front().is_none());
        mm.drive_registration_checks(&mut queue, ts, now, true);
        assert!(queue.pop_front().is_none());
        let next = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        mm.drive_registration_checks(&mut queue, next, now, true);
        assert!(queue.pop_front().is_some());
        assert!(queue.pop_front().is_none());
    }

    fn probes(queue: &mut MessageQueue) -> Vec<u32> {
        std::iter::from_fn(|| queue.pop_front())
            .filter_map(|m| match m.msg {
                SapMsgInner::CmceCallControl(CallControl::SitePresenceProbe { issi }) if m.dest == TetraEntity::Brew => Some(issi),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_site_linked_cell_has_its_location_area_siblings_check_presence_too() {
        let now = Instant::now() + Duration::from_secs(31);
        let ts = TdmaTime { t: 1, f: 1, m: 1, h: 0 };
        for linked in [false, true] {
            let (mut mm, _) = station_linked(linked);
            radio(&mut mm, 100);
            mm.registration_check_last_tick = Some(now - Duration::from_secs(1));
            let mut queue = MessageQueue::new();
            mm.drive_registration_checks(&mut queue, ts, now, true);
            let expected: Vec<u32> = if linked { vec![100] } else { vec![] };
            assert_eq!(probes(&mut queue), expected, "linked = {linked}");
        }
    }

    #[test]
    fn unsent_or_discarded_local_probe_retries_without_marking_a_radio_offline() {
        for discarded in [false, true] {
            let (mut mm, events) = station();
            radio(&mut mm, 100);
            while events.try_recv().is_some() {}
            let mut queue = MessageQueue::new();
            let receipt = mm.send_registration_check(&mut queue, 100, true).unwrap();
            let queued_at = Instant::now();
            mm.client_mgr.queue_registration_command(100, queued_at);
            mm.registration_probe_receipts.insert(100, receipt.clone());
            if discarded {
                receipt.mark_discarded();
            }
            mm.drive_registration_checks(&mut queue, TdmaTime::default(), queued_at, true);
            assert!(mm.client_mgr.get_client_by_issi(100).unwrap().grace_expires_at.is_none());
            assert!(mm.client_mgr.is_pending_command(100));
            let stale_at = queued_at + Duration::from_secs(16);
            mm.registration_check_last_tick = Some(stale_at - Duration::from_secs(1));
            mm.drive_registration_checks(&mut queue, TdmaTime::default(), stale_at, true);
            assert!(!mm.client_mgr.is_pending_command(100));
            assert!(mm.registration_probe_receipts.is_empty());
            assert!(mm.config.state_read().subscribers.is_registered(100));
            assert!(events.try_recv().is_none());
            let retry_at = stale_at + Duration::from_secs(31);
            mm.registration_check_last_tick = Some(retry_at - Duration::from_secs(1));
            let _ = actions(&mut queue);
            mm.drive_registration_checks(&mut queue, TdmaTime::default(), retry_at, true);
            assert!(mm.client_mgr.is_pending_command(100));
            assert_eq!(mm.registration_probe_receipts.len(), 1);
            assert!(queue.pop_front().is_some());
            assert!(mm.config.state_read().subscribers.is_registered(100));
        }
    }

    #[test]
    fn one_minute_profile_preserves_registration_until_a_transmitted_probe_gets_full_grace() {
        let (mut mm, events) = station();
        radio(&mut mm, 100);
        while events.try_recv().is_some() {}
        // The fixture uses a longer grace for separate ACK tests. Set the shipped profile.
        let mut config = (*mm.config.config()).clone();
        config.cell.presence_response_secs = 30;
        mm.config = SharedConfig::from_parts(config, None);
        mm.config.state_write().subscribers.register(100);
        let probe_at = Instant::now() + Duration::from_secs(31);
        mm.registration_check_last_tick = Some(probe_at - Duration::from_secs(1));
        let mut queue = MessageQueue::new();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), probe_at, true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        let receipt = mm.registration_probe_receipts[&100].clone();
        receipt.mark_transmitted();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), probe_at, true);
        let before = probe_at + Duration::from_secs(29);
        mm.registration_check_last_tick = Some(before - Duration::from_secs(1));
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), before, true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        let expires = probe_at + Duration::from_secs(30);
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), expires, true);
        assert!(!mm.config.state_read().subscribers.is_registered(100));
        assert!(matches!(events.try_recv(), Some(TelemetryEvent::MsTimeoutDrop { issi: 100 })));
        assert!(actions(&mut queue).contains(&BrewSubscriberAction::Deregister));
    }

    #[test]
    fn returning_known_radio_is_prompted_to_register_without_trusting_rssi_as_registration() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        mm.config.state_write().subscribers.deregister(100);
        let cipher = tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters::class2(2, 3);
        mm.client_mgr.set_client_ciphering(100, Some(cipher)).unwrap();
        mm.client_mgr.update_client_rssi(100, -45.0);
        let mut queue = MessageQueue::new();
        mm.maybe_reactive_recovery(&mut queue, 100);
        let message = queue.pop_front().expect("returning radio needs an immediate location update request");
        let SapMsgInner::LmmMleUnitdataReq(mut request) = message.msg else {
            panic!("unexpected coverage-return message")
        };
        assert_eq!(
            request.sdu.read_field(4, "pdu_type").unwrap(),
            tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl::DLocationUpdateCommand.into_raw()
        );
        assert_eq!(request.sdu.read_field(1, "group_report").unwrap(), 0);
        assert_eq!(request.sdu.read_field(1, "cipher_control").unwrap(), 1);
        assert_eq!(request.sdu.read_field(10, "ciphering_parameters").unwrap(), cipher.to_bits());
        assert!(!mm.config.state_read().subscribers.is_registered(100));
        assert!(mm.client_mgr.get_client_by_issi(100).unwrap().groups.contains(&268));
        mm.maybe_reactive_recovery(&mut queue, 100);
        assert!(queue.pop_front().is_none(), "a burst of returning PTT samples must not flood MCCH");
    }

    #[test]
    fn transmitted_probe_arms_full_grace_and_link_ack_confirms_presence() {
        let (mut mm, _) = station();
        radio(&mut mm, 100);
        let mut queue = MessageQueue::new();
        let receipt = mm.send_registration_check(&mut queue, 100, true).unwrap();
        mm.client_mgr.queue_registration_command(100, Instant::now());
        mm.registration_probe_receipts.insert(100, receipt.clone());
        receipt.mark_transmitted();
        let now = Instant::now();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), now, true);
        assert_eq!(
            mm.client_mgr.get_client_by_issi(100).unwrap().grace_expires_at,
            Some(now + Duration::from_secs(60))
        );
        assert!(
            mm.client_mgr
                .collect_registration_checks(3600, 30, now + Duration::from_secs(59))
                .is_empty()
        );
        receipt.mark_acknowledged();
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), Instant::now(), true);
        assert!(!mm.client_mgr.is_pending_command(100));
        assert!(mm.config.state_read().subscribers.is_registered(100));
    }

    #[test]
    fn a_transmitted_probe_still_expires_when_a_discarded_retry_hid_its_current_state() {
        let (mut mm, events) = station();
        radio(&mut mm, 100);
        while events.try_recv().is_some() {}
        let mut queue = MessageQueue::new();
        let receipt = mm.send_registration_check(&mut queue, 100, true).unwrap();
        let queued_at = Instant::now();
        mm.client_mgr.queue_registration_command(100, queued_at);
        mm.registration_probe_receipts.insert(100, receipt.clone());
        // All transitions happen before MM polls, as during an LLC catch-up burst.
        receipt.mark_transmitted();
        receipt.reset();
        receipt.mark_discarded();
        assert!(!receipt.is_transmitted());
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), queued_at, true);
        let deadline = queued_at + Duration::from_secs(60);
        assert_eq!(mm.client_mgr.get_client_by_issi(100).unwrap().grace_expires_at, Some(deadline));
        let before = deadline - Duration::from_secs(1);
        mm.registration_check_last_tick = Some(before - Duration::from_secs(1));
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), before, true);
        assert!(mm.config.state_read().subscribers.is_registered(100));
        mm.drive_registration_checks(&mut queue, TdmaTime::default(), deadline, true);
        assert!(!mm.config.state_read().subscribers.is_registered(100));
        assert!(matches!(events.try_recv(), Some(TelemetryEvent::MsTimeoutDrop { issi: 100 })));
    }
}
