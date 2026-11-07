use std::collections::{HashMap, HashSet};

use crate::net_telemetry::{TelemetryEvent, channel::TelemetrySink};
use tetra_core::TdmaTime;
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::fields::ciphering_parameters::CipheringParameters;
use tetra_pdus::mm::fields::class_of_ms::ClassOfMs;

/// Frame-based energy-economy monitoring cycle length, in TDMA frames, per ETSI EN 300 392-2
/// Table 23.9 (an EE MS wakes for 1 frame then sleeps N: EG1 sleeps 1 → cycle 2, EG2 sleeps 2 →
/// cycle 3, EG3 sleeps 5 → cycle 6). The BS caps Eg4–Eg7 to Eg3 to keep call-setup latency bounded,
/// so only StayAlive / Eg1 / Eg2 / Eg3 are ever granted or stored. Returns `None` for StayAlive
/// (no monitoring window — the MS is always reachable). Single source of truth for both the grant
/// (`grant_energy_saving`) and the republished window, so the two can never drift apart.
pub(crate) const fn ee_cycle_frames(mode: EnergySavingMode) -> Option<u8> {
    match mode {
        EnergySavingMode::StayAlive => None,
        EnergySavingMode::Eg1 => Some(2),
        EnergySavingMode::Eg2 => Some(3),
        // Eg3..Eg7 capped to Eg3 (sleep 5 frames → cycle 6).
        _ => Some(6),
    }
}

#[derive(Debug)]
pub enum ClientMgrErr {
    ClientNotFound { issi: u32 },
    GroupNotFound { gssi: u32 },
    IssiInGroupRange { issi: u32 },
    GssiInClientRange { gssi: u32 },
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum MmClientState {
    Unknown,
    Attached,
    Detached,
}

pub struct MmClientProperties {
    pub issi: u32,
    pub state: MmClientState,
    pub groups: HashSet<u32>,
    pub energy_saving_mode: EnergySavingMode,
    /// TDMA frame number (1..=18) at which this MS wakes up to monitor the MCCH.
    /// Set to None for StayAlive MSs. Used to gate/schedule unsolicited downlink (D-SETUP, SDS)
    /// to the MS's energy-economy monitoring window.
    pub monitoring_frame: Option<u8>,
    /// Multiframe offset within the Eg cycle at which this MS wakes up.
    /// Set to None for StayAlive MSs.
    pub monitoring_multiframe: Option<u8>,
    /// Last measured RSSI from this MS in dBFS (dB relative to ADC full scale).
    /// Updated on every UL burst received from this ISSI.
    /// None until first burst received after registration.
    pub last_rssi: Option<f32>,
    /// Monotonic timestamp of the last uplink burst heard from this MS (updated on every RSSI
    /// measurement — i.e. whenever the MS is observed transmitting). Distinct from
    /// `last_registration_time`: a present MS keeps transmitting (PTT / SDS / signalling bursts)
    /// without necessarily re-registering, so this is the authoritative "still on the air"
    /// signal. Used at T351 expiry to avoid dropping a radio that is demonstrably present but
    /// did not answer an unsolicited D-LOCATION-UPDATE-COMMAND (e.g. an energy-economy radio
    /// asleep when the COMMAND was sent) — which made present stations vanish from the dashboard.
    pub last_uplink_time: std::time::Instant,
    /// Timestamp (system time) when this MS last registered or re-registered.
    /// Used to enforce periodic registration expiry (T351).
    pub last_registration_time: std::time::Instant,
    pub class_of_ms: Option<ClassOfMs>,
    /// Air interface encryption agreed at the last registration; `None` = clear (class 1).
    pub ciphering: Option<CipheringParameters>,
    /// Layer-2 handle from the last successful location update.
    /// Required for sending downlink MM PDUs (D-LOCATION-UPDATE-COMMAND etc.)
    /// to this MS. Set to 0 until the first location update is received.
    pub last_handle: u32,
    /// Terminal Equipment Identity (60-bit hardware ID, like IMEI).
    /// Set when the MS sends U-TEI-PROVIDE. None if not yet received.
    pub tei: Option<u64>,
    /// True after BS sends D-LOCATION-UPDATE-COMMAND at T351 expiry.
    /// If the terminal re-registers, this is cleared. If T351 expires again
    /// while this is still true, the terminal is silently removed (no response).
    pub pending_command_sent: bool,
    /// When the current presence challenge was sent; only later RF activity can answer it.
    pub command_sent_at: Option<std::time::Instant>,
    /// When Some, terminal has until this instant to respond to D-LOCATION-UPDATE-COMMAND.
    pub grace_expires_at: Option<std::time::Instant>,
    // pub last_seen: TdmaTime,
}

impl MmClientProperties {
    pub fn new(ssi: u32) -> Self {
        MmClientProperties {
            issi: ssi,
            state: MmClientState::Unknown,
            groups: HashSet::new(),
            energy_saving_mode: EnergySavingMode::StayAlive,
            monitoring_frame: None,
            monitoring_multiframe: None,
            last_rssi: None,
            last_uplink_time: std::time::Instant::now(),
            pending_command_sent: false,
            command_sent_at: None,
            grace_expires_at: None,
            last_registration_time: std::time::Instant::now(),
            class_of_ms: None,
            ciphering: None,
            last_handle: 0,
            tei: None,
            // last_seen: TdmaTime::default(),
        }
    }
}

/// Stub function, to be replaced with checks based on configuration file
fn is_individual(_issi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn in_group_range(_gssi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn is_group(_gssi: u32) -> bool {
    return true;
}
/// Stub function, to be replaced with checks based on configuration file
fn may_attach(_issi: u32, _gssi: u32) -> bool {
    return true;
}

/// Safety cap on the per-ISSI registration-rate table so a churn of distinct forged ISSIs can't
/// grow it without bound; lapsed windows are pruned once this many are held.
const REGISTRATION_RATE_TABLE_CAP: usize = 8192;
/// Window the per-ISSI registration counter is measured over.
const REGISTRATION_RATE_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);

pub struct MmClientMgr {
    clients: HashMap<u32, MmClientProperties>,
    telemetry_sink: Option<TelemetrySink>,
    /// Hard cap on `clients` (0 = unlimited). Uplink is unauthenticated, so any radio can claim
    /// any of the 2^24 ISSIs: without a cap a registration flood grows this map (plus the
    /// subscriber registry and the dashboard) until the cell is OOM-killed. See
    /// `enforce_client_cap` / `at_capacity`.
    max_clients: usize,
    /// Per-ISSI accepted-registration counter: (window start, count in window). Bounds the churn
    /// one source ISSI can inflict on the table even when it stays under the cap.
    registration_rate: HashMap<u32, (std::time::Instant, u32)>,
    /// Accepted registrations per ISSI per [`REGISTRATION_RATE_WINDOW`] (0 = disabled).
    registrations_per_window: u32,
}

impl MmClientMgr {
    pub fn telemetry_sink(&self) -> Option<&TelemetrySink> {
        self.telemetry_sink.as_ref()
    }

    pub fn new(telemetry_sink: Option<TelemetrySink>) -> Self {
        MmClientMgr {
            clients: HashMap::new(),
            telemetry_sink,
            max_clients: tetra_config::bluestation::DEFAULT_MAX_REGISTERED_CLIENTS,
            registration_rate: HashMap::new(),
            registrations_per_window: tetra_config::bluestation::DEFAULT_REGISTRATION_RATE_LIMIT_PER_MIN,
        }
    }

    /// Apply the operator's `[security]` bounds. Called by MmBs at construction and whenever the
    /// config is swapped, so the caps are never left at the compiled-in defaults.
    pub fn set_limits(&mut self, max_clients: usize, registrations_per_window: u32) {
        self.max_clients = max_clients;
        self.registrations_per_window = registrations_per_window;
    }

    /// Number of clients currently held. Exposed for the flood-bound regression test and logging.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// True when the registry is full and a *new* ISSI must not be admitted. Only meaningful
    /// after `enforce_client_cap` has had its chance to make room.
    pub fn at_capacity(&self) -> bool {
        self.max_clients != 0 && self.clients.len() >= self.max_clients
    }

    /// Per-source-ISSI registration rate limit. Returns false when this registration must be
    /// dropped; counts the attempt when it is accepted. A real radio registers a handful of times
    /// a minute (T351 plus a post-PTT roaming update), so this only bites on a flood.
    pub fn allow_registration(&mut self, issi: u32) -> bool {
        if self.registrations_per_window == 0 {
            return true;
        }
        let now = std::time::Instant::now();
        // Prune lapsed windows before growing the table past its safety cap.
        if self.registration_rate.len() >= REGISTRATION_RATE_TABLE_CAP {
            self.registration_rate
                .retain(|_, (start, _)| now.duration_since(*start) < REGISTRATION_RATE_WINDOW);
        }
        let entry = self.registration_rate.entry(issi).or_insert((now, 0));
        if now.duration_since(entry.0) >= REGISTRATION_RATE_WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= self.registrations_per_window {
            return false;
        }
        entry.1 += 1;
        true
    }

    /// LRU-style eviction so the registry stays bounded under an RF-unauthenticated registration
    /// flood. Evicts the least-recently-heard clients that are demonstrably doing nothing:
    ///   - no group affiliation (nothing for the coverage-return re-affiliation to restore), and
    ///   - not in `protected` — the caller passes the ISSIs it knows are mid-call/PTT.
    /// An affiliated or in-call radio is never evicted, however old. Returns the evicted ISSIs so
    /// the caller can tear their subscriber-registry / Brew / dashboard entries down too.
    ///
    /// NB: a present but never-affiliated radio (e.g. a receive-only pager) is evictable under a
    /// flood. It is re-admitted on its next registration or by reactive recovery — the alternative
    /// is unbounded growth and an OOM-kill of the whole cell.
    pub fn enforce_client_cap(&mut self, protected: &HashSet<u32>) -> Vec<u32> {
        let mut evicted = Vec::new();
        if self.max_clients == 0 || self.clients.len() < self.max_clients {
            return evicted;
        }
        // Evict a batch (~6% of the cap, at least one) so the O(n) scan is amortised over many
        // registrations rather than run for every PDU of a flood.
        let batch = (self.max_clients / 16).max(1);
        let target = self.max_clients.saturating_sub(batch).max(1);
        let mut candidates: Vec<(std::time::Instant, u32)> = self
            .clients
            .values()
            .filter(|c| c.groups.is_empty() && !protected.contains(&c.issi))
            .map(|c| (c.last_uplink_time, c.issi))
            .collect();
        candidates.sort_unstable_by_key(|&(t, _)| t); // least recently heard first
        for (_, issi) in candidates {
            if self.clients.len() <= target {
                break;
            }
            self.remove_client(issi);
            evicted.push(issi);
        }
        if !evicted.is_empty() {
            tracing::warn!(
                "MM: client registry hit the {} cap — evicted {} idle unaffiliated client(s)",
                self.max_clients,
                evicted.len()
            );
        }
        evicted
    }

    pub fn get_client_by_issi(&mut self, issi: u32) -> Option<&MmClientProperties> {
        self.clients.get(&issi)
    }

    pub fn client_is_known(&self, issi: u32) -> bool {
        self.clients.contains_key(&issi)
    }

    /// True when `issi` is currently attached AND the request arrived on the same L2 handle the
    /// registration used. This is the strongest binding available without air-interface
    /// authentication (EN 300 392-7 TEA is not implemented, so the source SSI of an uplink PDU is
    /// unverifiable): a teardown may only act on live state that this same link established.
    /// The handle is inert in this stack today (MLE addresses by ISSI and hands up handle 0), so
    /// in practice this is the "is actually registered" half — but it binds to the link the day
    /// MLE carries a real one.
    pub fn is_registered_on_handle(&self, issi: u32, handle: u32) -> bool {
        self.clients
            .get(&issi)
            .map(|c| c.state == MmClientState::Attached && c.last_handle == handle)
            .unwrap_or(false)
    }

    pub fn set_client_state(&mut self, issi: u32, state: MmClientState) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.state = state;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_energy_saving_mode(&mut self, issi: u32, mode: EnergySavingMode) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.energy_saving_mode = mode;
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsEnergySaving { issi, mode: mode as u8 });
            }
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    pub fn set_client_monitoring_window(&mut self, issi: u32, frame: Option<u8>, multiframe: Option<u8>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.monitoring_frame = frame;
            client.monitoring_multiframe = multiframe;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Update RSSI for a known MS. Silently ignored if MS is not registered.
    pub fn update_client_rssi(&mut self, issi: u32, rssi_dbfs: f32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            // Every RSSI measurement is an uplink burst we just heard → the MS is on the air now.
            client.last_uplink_time = std::time::Instant::now();
            let should_log = match client.last_rssi {
                None => true,                                  // First measurement
                Some(prev) => (rssi_dbfs - prev).abs() >= 3.0, // Log on >=3dB change
            };
            client.last_rssi = Some(rssi_dbfs);
            if should_log {
                tracing::info!("RSSI: ISSI {} = {:.1} dBFS", issi, rssi_dbfs);
            }
        }
    }

    /// True if an uplink burst from this MS was heard within `within` — a direct sign the radio
    /// is still on the air, independent of whether it answered an unsolicited
    /// D-LOCATION-UPDATE-COMMAND. Used to keep a present radio from being torn down at T351
    /// expiry (it may be an energy-economy radio that was asleep when the COMMAND was sent).
    pub fn heard_on_air_within(&self, issi: u32, within: std::time::Duration) -> bool {
        self.clients
            .get(&issi)
            .map(|c| c.last_uplink_time.elapsed() < within)
            .unwrap_or(false)
    }

    /// Reset the periodic registration timer for a MS (called on each U-LOCATION-UPDATING-DEMAND).
    pub fn reset_registration_timer(&mut self, issi: u32) {
        self.reset_registration_timer_at(issi, std::time::Instant::now());
    }

    pub fn reset_registration_timer_at(&mut self, issi: u32, now: std::time::Instant) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.last_registration_time = now;
            client.pending_command_sent = false;
            client.command_sent_at = None;
            client.grace_expires_at = None;
        }
    }

    /// Returns true if a D-LOCATION-UPDATE-COMMAND was sent and terminal hasn't responded yet.
    pub fn is_pending_command(&self, issi: u32) -> bool {
        self.clients.get(&issi).map(|c| c.pending_command_sent).unwrap_or(false)
    }

    /// Mark that we sent D-LOCATION-UPDATE-COMMAND at T351 expiry.
    /// Terminal has grace_secs to respond before being removed.
    pub fn set_pending_command(&mut self, issi: u32, grace_secs: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.pending_command_sent = true;
            client.command_sent_at = Some(std::time::Instant::now());
            // Set last_registration_time so elapsed() > interval after grace_secs.
            // Achieved by back-dating: last_registration_time = now - (interval - grace_secs)
            // But we don't know interval here, so we use a simpler approach:
            // collect_expired_registrations checks pending_command_sent + grace separately.
            client.grace_expires_at = Some(std::time::Instant::now() + std::time::Duration::from_secs(grace_secs as u64));
        }
    }

    /// Reserve one challenge while it waits in the downlink queue. No response deadline
    /// runs until MAC confirms that the command was actually transmitted.
    pub fn queue_registration_command(&mut self, issi: u32, now: std::time::Instant) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.pending_command_sent = true;
            client.command_sent_at = Some(now);
            client.grace_expires_at = None;
        }
    }

    pub fn arm_registration_response_grace(&mut self, issi: u32, grace_secs: u32, now: std::time::Instant) {
        if let Some(client) = self.clients.get_mut(&issi) {
            if client.pending_command_sent && client.grace_expires_at.is_none() {
                client.grace_expires_at = Some(now + std::time::Duration::from_secs(grace_secs as u64));
            }
        }
    }

    /// Return expired T351 registrations, preserving the existing public interface.
    pub fn collect_expired_registrations(&self, interval_secs: u32) -> Vec<u32> {
        self.collect_registration_checks(interval_secs, 0, std::time::Instant::now())
    }

    fn next_registration_check(client: &MmClientProperties, interval_secs: u32, presence_secs: u32) -> Option<std::time::Instant> {
        let periodic = (interval_secs != 0).then(|| client.last_registration_time + std::time::Duration::from_secs(interval_secs as u64));
        let presence = (presence_secs != 0)
            .then(|| client.last_registration_time.max(client.last_uplink_time) + std::time::Duration::from_secs(presence_secs as u64));
        match (periodic, presence) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Due checks are challenges, not evidence of disconnection. Only an unanswered challenge
    /// reaching its full grace period may make a registered radio unavailable.
    pub fn collect_registration_checks(&self, interval_secs: u32, presence_secs: u32, now: std::time::Instant) -> Vec<u32> {
        if interval_secs == 0 && presence_secs == 0 {
            return Vec::new();
        }
        self.clients
            .iter()
            .filter_map(|(&issi, client)| {
                let due = if client.pending_command_sent {
                    client.grace_expires_at.is_some_and(|deadline| now >= deadline)
                } else {
                    Self::next_registration_check(client, interval_secs, presence_secs).is_some_and(|deadline| now >= deadline)
                };
                due.then_some(issi)
            })
            .collect()
    }

    /// A valid decoded uplink after a challenge proves that the radio is still present.
    pub fn heard_since_registration_command(&self, issi: u32) -> bool {
        self.clients.get(&issi).is_some_and(|client| {
            client
                .command_sent_at
                .is_some_and(|sent| client.last_uplink_time >= sent || client.last_registration_time >= sent)
        })
    }

    /// An unavailable local RF path cannot establish that remote radios have disconnected.
    /// Start fresh observation windows after the path resumes, without removing registrations.
    pub fn resume_registration_checks(&mut self) {
        let now = std::time::Instant::now();
        for client in self.clients.values_mut() {
            client.last_registration_time = now;
            client.pending_command_sent = false;
            client.command_sent_at = None;
            client.grace_expires_at = None;
        }
    }

    /// Decide whether the T351 D-LOCATION-UPDATE-COMMAND for `issi` should be sent on this tick.
    ///
    /// The COMMAND is an unsolicited downlink PDU. An energy-economy MS only listens to the
    /// downlink during its monitoring window, so a COMMAND sent while it sleeps is missed — it
    /// then never answers and is torn down at the second expiry, making a present radio vanish
    /// from the dashboard. So:
    ///   - StayAlive MS (always listening) or an EE MS whose monitoring window is not yet known
    ///     → send now (`true`);
    ///   - EE MS during its monitoring window → send now (`true`);
    ///   - EE MS asleep outside its window → defer (`false`) so the caller retries next tick and
    ///     sends in the wake window — UNLESS it is overdue by more than `window_wait_secs` beyond
    ///     the interval, in which case send blind (`true`) so a stale/incorrect window can never
    ///     suppress the probe forever.
    pub fn should_send_t351_command_now(&self, issi: u32, ts: TdmaTime, interval_secs: u32, window_wait_secs: u64) -> bool {
        self.should_send_registration_command_now(issi, ts, interval_secs, 0, window_wait_secs, std::time::Instant::now())
    }

    pub fn should_send_registration_command_now(
        &self,
        issi: u32,
        ts: TdmaTime,
        interval_secs: u32,
        presence_secs: u32,
        window_wait_secs: u64,
        now: std::time::Instant,
    ) -> bool {
        let Some(c) = self.clients.get(&issi) else {
            // Unknown client — let the caller proceed; the send is addressed by ISSI and harmless.
            return true;
        };
        let reachable_now = match ee_cycle_frames(c.energy_saving_mode) {
            None => true, // StayAlive — always reachable
            Some(cycle_len) => match (c.monitoring_frame, c.monitoring_multiframe) {
                (Some(f), Some(mf)) => ts.in_ee_monitoring_window(f, mf, cycle_len),
                _ => true, // EE but the window is not known yet — don't defer
            },
        };
        if reachable_now {
            return true;
        }
        // Sleeping EE MS outside its window: defer to a wake window, but bound the wait.
        Self::next_registration_check(c, interval_secs, presence_secs)
            .is_some_and(|due| now.saturating_duration_since(due).as_secs() >= window_wait_secs)
    }

    pub fn set_client_class_of_ms(&mut self, issi: u32, class: Option<ClassOfMs>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.class_of_ms = class;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Store the ciphering agreed at registration (`None` = clear).
    pub fn set_client_ciphering(&mut self, issi: u32, ciphering: Option<CipheringParameters>) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.ciphering = ciphering;
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Whether a registered radio is encrypted; `None` when the ISSI is not registered here.
    pub fn client_encrypted(&self, issi: u32) -> Option<bool> {
        self.clients.get(&issi).map(|c| c.ciphering.is_some())
    }

    /// Store the TEI (Terminal Equipment Identity) received from U-TEI-PROVIDE.
    /// If the ISSI is not registered yet, the TEI is silently ignored (can't fail critically).
    pub fn set_client_tei(&mut self, issi: u32, tei: u64) -> Result<(), ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.tei = Some(tei);
            Ok(())
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Registers a fresh state for a client, based on ssi
    /// If client is already registered, previous state is discarded.
    pub fn try_register_client(&mut self, issi: u32, attached: bool) -> Result<bool, ClientMgrErr> {
        if !is_individual(issi) {
            return Err(ClientMgrErr::IssiInGroupRange { issi });
        };

        // discard previous state if any
        self.clients.remove(&issi);

        // Create and insert new client state
        let mut elem = MmClientProperties::new(issi);
        elem.state = if attached {
            MmClientState::Attached
        } else {
            MmClientState::Unknown
        };
        self.clients.insert(issi, elem);

        // Send telemetry event
        if let Some(sink) = &self.telemetry_sink {
            sink.send(TelemetryEvent::MsRegistration { issi });
        }

        Ok(true)
    }

    /// Removes a client from the registry, returning its properties if found
    /// Returns every known client's ISSI. Used by mm_bs to re-register all MS by ISSI (the L2
    /// handle is inert: MLE addresses downlink MM PDUs by ISSI; `last_handle` is always 0).
    pub fn all_known_issis(&self) -> Vec<u32> {
        self.clients.keys().copied().collect()
    }

    /// Per-MS energy-economy monitoring windows, for publishing into shared state so the downlink
    /// scheduler can defer unsolicited traffic to a sleeping MS's wake window. Yields
    /// (issi, monitoring_frame, monitoring_multiframe, cycle_len) for every client that is actually
    /// in an energy-saving mode (not StayAlive) and has a valid monitoring window. StayAlive MSs are
    /// omitted (their absence means "always reachable"). cycle_len is the FRAME-based cycle from
    /// [`ee_cycle_frames`] (Eg1=2, Eg2=3, Eg3=6 — ETSI Table 23.9).
    pub fn ee_monitoring_windows(&self) -> impl Iterator<Item = (u32, u8, u8, u8)> + '_ {
        self.clients.values().filter_map(|c| {
            let cycle_len = ee_cycle_frames(c.energy_saving_mode)?;
            let frame = c.monitoring_frame?;
            let mframe = c.monitoring_multiframe?;
            // Guard against a malformed window so the scheduler never gates on garbage.
            if cycle_len < 2 || !(1..=18).contains(&frame) || !(1..=60).contains(&mframe) {
                return None;
            }
            Some((c.issi, frame, mframe, cycle_len))
        })
    }

    /// Update the last known L2 handle for a registered client.
    pub fn set_client_handle(&mut self, issi: u32, handle: u32) {
        if let Some(client) = self.clients.get_mut(&issi) {
            client.last_handle = handle;
        }
    }

    /// Project the persistable recovery state of every known client: (issi, groups, energy mode).
    /// Used by restart recovery to snapshot the registry to disk. The L2 handle is intentionally
    /// omitted — it is inert (always 0) in this stack, so there is nothing to persist.
    pub fn snapshot_for_recovery(&self) -> Vec<(u32, Vec<u32>, EnergySavingMode)> {
        self.clients
            .values()
            .map(|c| (c.issi, c.groups.iter().copied().collect(), c.energy_saving_mode))
            .collect()
    }

    /// Restore a client loaded from the recovery cache after a BS restart. The client is inserted
    /// as Detached with its persisted groups + energy mode, a FRESH registration timer, and
    /// cleared pending-command flags. This makes it `client_is_known()` — so the coverage-return
    /// re-affiliation fires when it answers our replayed D-LOCATION-UPDATE-COMMAND — while NOT
    /// flagging it for T351 expiry before the replay sweep completes (which would let the
    /// second-expiry REJECT+remove path wipe the restored groups). No CMCE/Brew affiliation is
    /// emitted here; that happens only when the MS actually re-registers.
    pub fn restore_client(&mut self, issi: u32, groups: &[u32], esm: EnergySavingMode) {
        let mut client = MmClientProperties::new(issi);
        client.state = MmClientState::Detached;
        client.energy_saving_mode = esm;
        for &g in groups {
            client.groups.insert(g);
        }
        self.clients.insert(issi, client);
    }

    pub fn remove_client(&mut self, ssi: u32) -> Option<MmClientProperties> {
        if let Some(client) = self.clients.remove(&ssi) {
            // Send telemetry event
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsDeregistration { issi: ssi });
            }
            Some(client)
        } else {
            None
        }
    }

    /// Remove a client without a deregistration telemetry event: the MS moved to a sibling cell
    /// (multi-cell) or another station and is still registered there, so it must not vanish from
    /// the dashboard. `MsMovedAway` tells the attached-radio count it left this cell.
    pub fn remove_client_quiet(&mut self, ssi: u32) -> Option<MmClientProperties> {
        let client = self.clients.remove(&ssi)?;
        if let Some(sink) = &self.telemetry_sink {
            sink.send(TelemetryEvent::MsMovedAway { issi: ssi, cell: 0 });
        }
        Some(client)
    }

    /// Detaches all groups from a client
    pub fn client_detach_all_groups(&mut self, issi: u32) -> Result<bool, ClientMgrErr> {
        if let Some(client) = self.clients.get_mut(&issi) {
            // Send telemetry event
            if let Some(sink) = &self.telemetry_sink {
                sink.send(TelemetryEvent::MsGroupDetach {
                    issi: client.issi,
                    gssis: client.groups.iter().cloned().collect(),
                });
            }
            client.groups.clear();
            Ok(true)
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }

    /// Attaches or detaches a client from a group
    pub fn client_group_attach(&mut self, issi: u32, gssi: u32, do_attach: bool) -> Result<bool, ClientMgrErr> {
        // Checks
        if !in_group_range(gssi) {
            return Err(ClientMgrErr::GssiInClientRange { gssi });
        };
        if !is_group(gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };
        if !may_attach(issi, gssi) {
            return Err(ClientMgrErr::GroupNotFound { gssi });
        };

        if let Some(client) = self.clients.get_mut(&issi) {
            if do_attach {
                // Send telemetry event
                if let Some(sink) = &self.telemetry_sink {
                    sink.send(TelemetryEvent::MsGroupAttach {
                        issi: client.issi,
                        gssis: vec![gssi].into_iter().collect(),
                    });
                }

                Ok(client.groups.insert(gssi))
            } else {
                Ok(client.groups.remove(&gssi))
            }
        } else {
            Err(ClientMgrErr::ClientNotFound { issi })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A radio dropped because it moved to another cell or station is reported as moved away,
    /// not deregistered: it stays registered on the network.
    #[test]
    fn quiet_removal_reports_moved_away_not_deregistration() {
        let (sink, source) = crate::net_telemetry::telemetry_channel();
        let mut mgr = MmClientMgr::new(Some(sink));
        mgr.try_register_client(100, true).unwrap();
        while source.try_recv().is_some() {}

        assert!(mgr.remove_client_quiet(100).is_some());
        assert!(matches!(source.try_recv(), Some(TelemetryEvent::MsMovedAway { issi: 100, cell: 0 })));
        assert!(source.try_recv().is_none());
        // Nothing to report for a radio that is not here.
        assert!(mgr.remove_client_quiet(100).is_none());
        assert!(source.try_recv().is_none());
    }

    /// The T351 presence guard: a radio heard transmitting recently is reported "on air" so it is
    /// never torn down at expiry (FH-BUG-044 — present stations vanishing from the dashboard).
    #[test]
    fn heard_on_air_tracks_uplink_presence() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();

        // A freshly-registered MS counts as just heard on the air.
        assert!(mgr.heard_on_air_within(100, Duration::from_secs(300)));
        // An unknown ISSI is never "heard".
        assert!(!mgr.heard_on_air_within(999, Duration::from_secs(300)));

        // After a little real time, a very short window reports "not heard" (would fall through to
        // the COMMAND/teardown path), while a generous window still reports "present".
        std::thread::sleep(Duration::from_millis(15));
        assert!(!mgr.heard_on_air_within(100, Duration::from_millis(5)));
        assert!(mgr.heard_on_air_within(100, Duration::from_secs(300)));

        // A fresh uplink burst (RSSI measurement) re-stamps the radio as heard right now.
        std::thread::sleep(Duration::from_millis(15));
        assert!(!mgr.heard_on_air_within(100, Duration::from_millis(5)));
        mgr.update_client_rssi(100, -60.0);
        assert!(mgr.heard_on_air_within(100, Duration::from_millis(5)));
    }

    /// The T351 COMMAND is gated to a sleeping EE radio's wake window so it is never missed
    /// (FH-BUG-044 follow-up). StayAlive radios are always reachable; EE radios are sent only
    /// in their monitoring window.
    #[test]
    fn t351_command_gated_to_ee_monitoring_window() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        let ts = TdmaTime::default();

        // StayAlive: always reachable → send now.
        assert!(mgr.should_send_t351_command_now(100, ts, 300, 6));
        // Unknown ISSI: caller proceeds.
        assert!(mgr.should_send_t351_command_now(999, ts, 300, 6));

        // Eg1 radio (cycle 2 frames) with a known window at frame 1 / multiframe 1.
        mgr.set_client_energy_saving_mode(100, EnergySavingMode::Eg1).unwrap();
        mgr.set_client_monitoring_window(100, Some(1), Some(1)).unwrap();
        // m=1,f=1 → cur_abs 0, in window (cycle 2) → send.
        assert!(mgr.should_send_t351_command_now(100, TdmaTime { t: 1, f: 1, m: 1, h: 0 }, 300, 6));
        // m=1,f=2 → cur_abs 1, out of window, freshly registered (not overdue) → defer.
        assert!(!mgr.should_send_t351_command_now(100, TdmaTime { t: 1, f: 2, m: 1, h: 0 }, 300, 6));

        // EE radio whose monitoring window is unknown is never deferred (sent immediately).
        mgr.set_client_monitoring_window(100, None, None).unwrap();
        assert!(mgr.should_send_t351_command_now(100, TdmaTime { t: 1, f: 2, m: 1, h: 0 }, 300, 6));
    }

    #[test]
    fn idle_presence_challenge_precedes_an_hour_long_t351_and_disabled_stays_legacy() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        let now = std::time::Instant::now();
        let client = mgr.clients.get_mut(&100).unwrap();
        client.last_registration_time = now - Duration::from_secs(61);
        client.last_uplink_time = client.last_registration_time;
        assert_eq!(mgr.collect_registration_checks(3600, 60, now), vec![100]);
        assert!(mgr.collect_registration_checks(3600, 0, now).is_empty());
        assert!(mgr.collect_registration_checks(0, 0, now + Duration::from_secs(90000)).is_empty());
    }

    #[test]
    fn quiet_radio_keeps_registration_during_grace_and_a_response_cancels_the_probe() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        mgr.set_pending_command(100, 30);
        let deadline = mgr.clients[&100].grace_expires_at.unwrap();
        assert!(
            mgr.collect_registration_checks(3600, 30, deadline - Duration::from_nanos(1))
                .is_empty()
        );
        assert_eq!(mgr.collect_registration_checks(3600, 30, deadline), vec![100]);
        mgr.reset_registration_timer(100);
        assert!(!mgr.is_pending_command(100));
        assert!(mgr.collect_registration_checks(3600, 30, std::time::Instant::now()).is_empty());
        assert!(mgr.client_is_known(100));
    }

    #[test]
    fn old_voice_is_not_a_probe_response_but_new_decoded_uplink_is() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        let old = std::time::Instant::now() - Duration::from_secs(100);
        let client = mgr.clients.get_mut(&100).unwrap();
        client.last_uplink_time = old;
        client.last_registration_time = old;
        mgr.set_pending_command(100, 30);
        assert!(!mgr.heard_since_registration_command(100));
        mgr.update_client_rssi(100, -40.0);
        assert!(mgr.heard_since_registration_command(100));
    }

    #[test]
    fn recent_uplink_defers_idle_probe_and_ee_wait_uses_its_actual_deadline() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        mgr.set_client_energy_saving_mode(100, EnergySavingMode::Eg1).unwrap();
        mgr.set_client_monitoring_window(100, Some(1), Some(1)).unwrap();
        let now = std::time::Instant::now();
        let client = mgr.clients.get_mut(&100).unwrap();
        client.last_registration_time = now - Duration::from_secs(1000);
        client.last_uplink_time = now - Duration::from_secs(10);
        assert!(mgr.collect_registration_checks(3600, 60, now).is_empty());
        mgr.clients.get_mut(&100).unwrap().last_uplink_time = now - Duration::from_secs(61);
        let asleep = TdmaTime { t: 1, f: 2, m: 1, h: 0 };
        assert!(!mgr.should_send_registration_command_now(100, asleep, 3600, 60, 6, now));
        assert!(mgr.should_send_registration_command_now(100, asleep, 3600, 60, 6, now + Duration::from_secs(6)));
    }

    #[test]
    fn rf_resume_cancels_old_challenges_without_losing_affiliations() {
        let mut mgr = MmClientMgr::new(None);
        mgr.try_register_client(100, true).unwrap();
        mgr.client_group_attach(100, 268, true).unwrap();
        mgr.set_pending_command(100, 0);
        mgr.resume_registration_checks();
        assert!(!mgr.is_pending_command(100));
        assert!(mgr.clients[&100].groups.contains(&268));
        assert!(mgr.collect_registration_checks(3600, 30, std::time::Instant::now()).is_empty());
    }
}
