use super::*;

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

// TETRA TDMA timing: one slot is 170/12 milliseconds.
const TIMESLOT_DURATION_MS: f64 = 170.0 / 12.0;

#[inline]
fn seconds_to_timeslots(seconds: i32) -> i32 {
    debug_assert!(seconds >= 0);
    (f64::from(seconds) * 1_000.0 / TIMESLOT_DURATION_MS) as i32
}

#[inline]
fn setup_timeout_to_timeslots(timeout: CallTimeoutSetupPhase) -> Option<i32> {
    match timeout {
        CallTimeoutSetupPhase::Predefined => Some(seconds_to_timeslots(10)),
        CallTimeoutSetupPhase::T1s => Some(seconds_to_timeslots(1)),
        CallTimeoutSetupPhase::T2s => Some(seconds_to_timeslots(2)),
        CallTimeoutSetupPhase::T5s => Some(seconds_to_timeslots(5)),
        CallTimeoutSetupPhase::T10s => Some(seconds_to_timeslots(10)),
        CallTimeoutSetupPhase::T20s => Some(seconds_to_timeslots(20)),
        CallTimeoutSetupPhase::T30s => Some(seconds_to_timeslots(30)),
        CallTimeoutSetupPhase::T60s => Some(seconds_to_timeslots(60)),
    }
}

/// Energy-Economy D-SETUP gate (clause 16.7): individual-call setup resends are held for the
/// called MS's monitoring window, but if the window has not opened within this many timeslots of
/// setup start we fall back to the historical blind resend. ~6 s (a few EE cycles) — chosen to be
/// comfortably under the shortest setup timeout (`T10s`/`Predefined`) so a wrong granted window
/// phase degrades to "no worse than before", never to a setup that times out unanswered.
/// (6 s / (170/12 ms per slot) ≈ 423 timeslots.)
pub(super) const EE_DSETUP_FALLBACK_TS: i32 = 423;

#[inline]
pub(super) fn call_timeout_to_timeslots(timeout: CallTimeout) -> Option<i32> {
    match timeout {
        CallTimeout::Infinite | CallTimeout::Reserved => None,
        CallTimeout::T30s => Some(seconds_to_timeslots(30)),
        CallTimeout::T45s => Some(seconds_to_timeslots(45)),
        CallTimeout::T60s => Some(seconds_to_timeslots(60)),
        CallTimeout::T2m => Some(seconds_to_timeslots(120)),
        CallTimeout::T3m => Some(seconds_to_timeslots(180)),
        CallTimeout::T4m => Some(seconds_to_timeslots(240)),
        CallTimeout::T5m => Some(seconds_to_timeslots(300)),
        CallTimeout::T6m => Some(seconds_to_timeslots(360)),
        CallTimeout::T8m => Some(seconds_to_timeslots(480)),
        CallTimeout::T10m => Some(seconds_to_timeslots(600)),
        CallTimeout::T12m => Some(seconds_to_timeslots(720)),
        CallTimeout::T15m => Some(seconds_to_timeslots(900)),
        CallTimeout::T20m => Some(seconds_to_timeslots(1200)),
        CallTimeout::T30m => Some(seconds_to_timeslots(1800)),
    }
}

/// Origin of a group call
#[derive(Clone)]
pub(super) enum CallOrigin {
    /// Local MS-initiated call
    Local {
        caller_addr: TetraAddress,
    },
    /// Network-initiated call from TetraPack/Brew
    Network {
        brew_uuid: uuid::Uuid,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GroupCallState {
    /// An active speaker is currently transmitting.
    Transmitting,
    /// No active speaker; call is still alive during hangtime.
    NoActiveSpeaker { since: TdmaTime },
}

/// Tracks an active group call (local or network-initiated)
#[derive(Clone)]
pub(super) struct ActiveCall {
    pub(super) origin: CallOrigin,
    pub(super) dest_gssi: u32,
    pub(super) source_issi: u32,
    /// ETSI call priority (0..=15) requested in the originating U-SETUP / network call start.
    /// Retained so a later emergency / pre-emptive call can compare against it when deciding
    /// which active call to pre-empt for a free traffic channel. See [`is_preemptive_priority`].
    pub(super) priority: u8,
    pub(super) created_at: TdmaTime,
    pub(super) call_timeout: CallTimeout,
    pub(super) ts: u8,
    pub(super) usage: u8,
    pub(super) tx_active: bool,
    pub(super) hangtime_start: Option<TdmaTime>,
    pub(super) queued_tx_demand: Option<TetraAddress>,
    pub(super) brew_uuid: Option<uuid::Uuid>,
    /// Energy-economy announce coverage: affiliated member ISSIs that have had a downlink wake
    /// frame since the call started (so they will have received the group D-SETUP). Members on
    /// different EE phases wake at different frames, so the BS re-emits the announcement until
    /// every EE member is covered. See `drive_group_ee_announce`.
    pub(super) ee_announce_covered: std::collections::HashSet<u32>,
    /// Set once every affiliated EE member is covered (or the bounded announce window elapses),
    /// after which the per-frame re-emit stops and the normal late-entry cadence takes over.
    pub(super) ee_announce_done: bool,
}

impl ActiveCall {
    pub(super) fn new_local(
        caller_addr: TetraAddress,
        dest_gssi: u32,
        source_issi: u32,
        ts: u8,
        usage: u8,
        created_at: TdmaTime,
        call_timeout: CallTimeout,
        priority: u8,
    ) -> Self {
        Self {
            origin: CallOrigin::Local { caller_addr },
            dest_gssi,
            source_issi,
            priority,
            created_at,
            call_timeout,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_tx_demand: None,
            brew_uuid: None,
            ee_announce_covered: std::collections::HashSet::new(),
            ee_announce_done: false,
        }
    }

    pub(super) fn new_network(
        brew_uuid: uuid::Uuid,
        dest_gssi: u32,
        source_issi: u32,
        ts: u8,
        usage: u8,
        created_at: TdmaTime,
        call_timeout: CallTimeout,
        priority: u8,
    ) -> Self {
        Self {
            origin: CallOrigin::Network { brew_uuid },
            dest_gssi,
            source_issi,
            priority,
            created_at,
            call_timeout,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_tx_demand: None,
            brew_uuid: Some(brew_uuid),
            ee_announce_covered: std::collections::HashSet::new(),
            ee_announce_done: false,
        }
    }

    #[inline]
    pub(super) fn state(&self) -> GroupCallState {
        if self.tx_active {
            GroupCallState::Transmitting
        } else {
            GroupCallState::NoActiveSpeaker {
                since: self.hangtime_start.unwrap_or_default(),
            }
        }
    }

    #[inline]
    pub(super) fn is_tx_active(&self) -> bool {
        matches!(self.state(), GroupCallState::Transmitting)
    }

    #[inline]
    pub(super) fn is_current_speaker(&self, issi: u32) -> bool {
        self.tx_active && self.source_issi == issi
    }

    #[inline]
    pub(super) fn call_timeout_expired(&self, now: TdmaTime) -> bool {
        match call_timeout_to_timeslots(self.call_timeout) {
            Some(timeout) => self.created_at.age(now) > timeout,
            None => false,
        }
    }

    pub(super) fn enter_hangtime(&mut self, now: TdmaTime) {
        self.tx_active = false;
        self.hangtime_start = Some(now);
    }

    /// Reset the call timeout clock. Called when a new network speaker takes the floor so that
    /// the 120s (T2m) window is measured from the latest transmission, not from call creation.
    /// Without this, a conversation with multiple back-to-back speakers always expires at
    /// `created_at + timeout` regardless of how recently the last speaker started talking.
    pub(super) fn reset_timeout(&mut self, now: TdmaTime) {
        self.created_at = now;
    }

    pub(super) fn grant_floor(&mut self, source_issi: u32, speaker_addr: Option<TetraAddress>) {
        self.source_issi = source_issi;
        self.tx_active = true;
        self.hangtime_start = None;
        self.queued_tx_demand = None;

        if let (CallOrigin::Local { caller_addr }, Some(addr)) = (&mut self.origin, speaker_addr) {
            *caller_addr = addr;
        }
    }

    pub(super) fn queue_tx_demand(&mut self, requester: TetraAddress) -> TxDemandQueueResult {
        if self.is_current_speaker(requester.ssi) {
            return TxDemandQueueResult::FromCurrentSpeaker;
        }

        match self.queued_tx_demand {
            Some(existing) if existing.ssi == requester.ssi => TxDemandQueueResult::AlreadyQueuedBySameUser,
            Some(_) => TxDemandQueueResult::QueueBusy,
            None => {
                self.queued_tx_demand = Some(requester);
                TxDemandQueueResult::Queued
            }
        }
    }

    pub(super) fn take_queued_tx_demand(&mut self) -> Option<TetraAddress> {
        self.queued_tx_demand.take()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TxDemandQueueResult {
    Queued,
    AlreadyQueuedBySameUser,
    QueueBusy,
    FromCurrentSpeaker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum IndividualCallState {
    /// Generic setup state for locally initiated individual calls.
    CallSetupPending,
    /// Setup state for incoming call leg while awaiting local user/app response.
    IncomingSetupPending,
    /// Incoming call has alerted the destination side.
    IncomingAlerting,
    /// Incoming call setup is waiting for backhaul/network confirmation.
    IncomingSetupWaitNetworkAck,
    /// Call is established.
    Active,
}

#[derive(Clone)]
pub(super) struct IndividualCall {
    pub(super) calling_addr: TetraAddress,
    pub(super) called_addr: TetraAddress,
    pub(super) calling_handle: u32,
    pub(super) calling_link_id: u32,
    pub(super) calling_endpoint_id: u32,
    pub(super) called_handle: Option<u32>,
    pub(super) called_link_id: Option<u32>,
    pub(super) called_endpoint_id: Option<u32>,
    pub(super) calling_ts: u8,
    pub(super) called_ts: u8,
    pub(super) calling_usage: u8,
    pub(super) called_usage: u8,
    /// true = full duplex (ETSI 14.8.17), false = simplex
    pub(super) simplex_duplex: bool,
    /// ETSI call priority (0..=15) from the originating U-SETUP. Retained so an emergency /
    /// pre-emptive call can pre-empt this one for a free traffic channel. See
    /// [`is_preemptive_priority`].
    pub(super) priority: u8,
    pub(super) state: IndividualCallState,
    /// Start instant for setup timeout (T301/T302 equivalent on BS side).
    pub(super) setup_timer_started: Option<TdmaTime>,
    /// Setup timeout value while the call is not active.
    pub(super) setup_timeout: Option<CallTimeoutSetupPhase>,
    /// Start instant for active call timeout (T310 equivalent).
    pub(super) active_timer_started: Option<TdmaTime>,
    /// Active call timeout value.
    pub(super) call_timeout: CallTimeout,
    /// True when the called party lives behind Brew/TetraPack.
    pub(super) called_over_brew: bool,
    /// True when the calling party lives behind Brew/TetraPack.
    pub(super) calling_over_brew: bool,
    /// Brew UUID when this call is bridged to TetraPack.
    pub(super) brew_uuid: Option<uuid::Uuid>,
    /// Cached network call metadata for Brew bridged legs.
    pub(super) network_call: Option<NetworkCircuitCall>,
    /// True once CONNECT_REQUEST has been sent for Brew-originated setup.
    pub(super) connect_request_sent: bool,
    /// SSI of the party currently holding the floor (simplex P2P only).
    /// None until the call is active. Used by UL inactivity timeout to force TX-CEASED.
    pub(super) floor_holder: Option<u32>,
}

impl IndividualCall {
    #[inline]
    pub(super) fn is_alerted(&self) -> bool {
        matches!(
            self.state,
            IndividualCallState::IncomingAlerting
                | IndividualCallState::IncomingSetupWaitNetworkAck
                | IndividualCallState::Active
        )
    }

    pub(super) fn mark_alerted(&mut self, now: TdmaTime, setup_timeout: CallTimeoutSetupPhase) {
        if matches!(
            self.state,
            IndividualCallState::CallSetupPending | IndividualCallState::IncomingSetupPending
        ) {
            self.state = IndividualCallState::IncomingAlerting;
        }
        self.setup_timer_started = Some(now);
        self.setup_timeout = Some(setup_timeout);
    }

    #[inline]
    pub(super) fn is_active(&self) -> bool {
        self.state == IndividualCallState::Active
    }

    pub(super) fn activate(&mut self, now: TdmaTime) {
        self.state = IndividualCallState::Active;
        self.setup_timer_started = None;
        self.setup_timeout = None;
        self.active_timer_started = Some(now);
        self.connect_request_sent = false;
    }

    #[inline]
    pub(super) fn setup_timeout_expired(&self, now: TdmaTime) -> bool {
        if self.is_active() {
            return false;
        }
        let Some(started) = self.setup_timer_started else {
            return false;
        };
        let Some(timeout) = self.setup_timeout else {
            return false;
        };
        let Some(limit) = setup_timeout_to_timeslots(timeout) else {
            return false;
        };
        started.age(now) > limit
    }

    #[inline]
    pub(super) fn active_timeout_expired(&self, now: TdmaTime) -> bool {
        if !self.is_active() {
            return false;
        }
        // Full-duplex individual calls (normal voice calls) have no timeout —
        // participants may talk for as long as they want.
        // Only simplex (half-duplex PTT) calls are subject to call_timeout,
        // to release the slot if an MS disappears without disconnecting.
        if self.simplex_duplex {
            return false;
        }
        let Some(started) = self.active_timer_started else {
            return false;
        };
        let Some(limit) = call_timeout_to_timeslots(self.call_timeout) else {
            return false;
        };
        started.age(now) > limit
    }
}
