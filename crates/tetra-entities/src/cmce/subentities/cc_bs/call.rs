use super::*;

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
    pub(super) created_at: TdmaTime,
    pub(super) call_timeout: CallTimeout,
    pub(super) ts: u8,
    pub(super) usage: u8,
    pub(super) tx_active: bool,
    pub(super) hangtime_start: Option<TdmaTime>,
    pub(super) queued_tx_demand: Option<TetraAddress>,
    pub(super) brew_uuid: Option<uuid::Uuid>,
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
    ) -> Self {
        Self {
            origin: CallOrigin::Local { caller_addr },
            dest_gssi,
            source_issi,
            created_at,
            call_timeout,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_tx_demand: None,
            brew_uuid: None,
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
    ) -> Self {
        Self {
            origin: CallOrigin::Network { brew_uuid },
            dest_gssi,
            source_issi,
            created_at,
            call_timeout,
            ts,
            usage,
            tx_active: true,
            hangtime_start: None,
            queued_tx_demand: None,
            brew_uuid: Some(brew_uuid),
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
        let Some(started) = self.active_timer_started else {
            return false;
        };
        let Some(limit) = call_timeout_to_timeslots(self.call_timeout) else {
            return false;
        };
        started.age(now) > limit
    }
}
