use super::*;

// TETRA TDMA timing: one slot is 170/12 milliseconds.
const TIMESLOT_DURATION_MS: f64 = 170.0 / 12.0;

#[inline]
fn seconds_to_timeslots(seconds: i32) -> i32 {
    debug_assert!(seconds >= 0);
    // slots = total_ms / slot_duration_ms
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
fn call_timeout_to_timeslots(timeout: CallTimeout) -> Option<i32> {
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

pub(super) const LOCAL_ECHO_ISSI: u32 = 999;

pub(super) struct CachedSetup {
    pub(super) pdu: DSetup,
    pub(super) dest_addr: TetraAddress,
    pub(super) resend: bool,
    pub(super) tx_receipt: Option<TxReporter>,
}

/// Origin of a group call
#[derive(Clone)]
pub(super) enum CallOrigin {
    /// Local MS-initiated call, needs MLE routing for individual addressing
    Local {
        caller_addr: TetraAddress, // For D-CALL-PROCEEDING, D-CONNECT routing
    },
    /// Network-initiated call from TetraPack/Brew
    Network {
        brew_uuid: uuid::Uuid, // For Brew tracking
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum GroupCallState {
    /// An active speaker is currently transmitting.
    Transmitting,
    /// No active speaker; call is still alive during hangtime.
    NoActiveSpeaker { since: TdmaTime },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CcFormalState {
    Idle,
    Setup,
    Active,
    Disconnect,
    Release,
    Restore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CcFormalEvent {
    SetupRequest,
    SetupComplete,
    DisconnectRequest,
    ModifyRequest,
    ReleaseRequest,
    RestoreRequest,
    RestoreComplete,
    RestoreReject,
    TimerExpired,
    CleanupComplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CcFormalTransitionError {
    pub(super) state: CcFormalState,
    pub(super) event: CcFormalEvent,
}

impl CcFormalState {
    pub(super) fn transition(self, event: CcFormalEvent) -> Result<CcFormalState, CcFormalTransitionError> {
        let next = match (self, event) {
            (CcFormalState::Idle, CcFormalEvent::SetupRequest) => CcFormalState::Setup,
            (CcFormalState::Setup, CcFormalEvent::SetupComplete) => CcFormalState::Active,
            (CcFormalState::Setup, CcFormalEvent::ReleaseRequest | CcFormalEvent::TimerExpired) => CcFormalState::Release,
            (CcFormalState::Active, CcFormalEvent::ModifyRequest) => CcFormalState::Active,
            (CcFormalState::Active, CcFormalEvent::DisconnectRequest) => CcFormalState::Disconnect,
            (CcFormalState::Active, CcFormalEvent::ReleaseRequest | CcFormalEvent::TimerExpired) => CcFormalState::Release,
            (CcFormalState::Active, CcFormalEvent::RestoreRequest) => CcFormalState::Restore,
            (CcFormalState::Disconnect, CcFormalEvent::ReleaseRequest | CcFormalEvent::TimerExpired) => CcFormalState::Release,
            (CcFormalState::Restore, CcFormalEvent::RestoreComplete) => CcFormalState::Active,
            (CcFormalState::Restore, CcFormalEvent::RestoreReject | CcFormalEvent::ReleaseRequest | CcFormalEvent::TimerExpired) => {
                CcFormalState::Release
            }
            (CcFormalState::Release, CcFormalEvent::CleanupComplete) => CcFormalState::Idle,
            _ => return Err(CcFormalTransitionError { state: self, event }),
        };

        Ok(next)
    }

    #[inline]
    pub(super) fn after(self, event: CcFormalEvent) -> CcFormalState {
        self.transition(event)
            .unwrap_or_else(|err| panic!("invalid CMCE CC formal transition: {:?} + {:?}", err.state, err.event))
    }
}

impl GroupCallState {
    #[allow(dead_code)]
    #[inline]
    pub(super) fn formal_state(self) -> CcFormalState {
        CcFormalState::Active
    }
}

/// Tracks an active group call (local or network-initiated)
#[derive(Clone)]
pub(super) struct ActiveCall {
    pub(super) origin: CallOrigin,
    pub(super) dest_gssi: u32,   // Destination group
    pub(super) source_issi: u32, // Current speaker
    pub(super) created_at: TdmaTime,
    pub(super) call_timeout: CallTimeout,
    /// ETSI EN 300 392-2 clause 14.8 call priority (0..=15; 15 = emergency). Used for
    /// pre-emptive priority handling: a higher-priority set-up may release this call.
    pub(super) priority: u8,
    pub(super) carrier_num: u16,
    pub(super) ts: u8,
    pub(super) usage: u8,
    /// True if someone is currently transmitting
    pub(super) tx_active: bool,
    /// Energy-economy group-announce batching: set once every affiliated EE member has had a
    /// downlink wake frame covered by an announce re-send (or the bounded window elapsed).
    pub(super) ee_announce_done: bool,
    /// ISSIs already covered by an announce re-send (StayAlive members, or EE members whose
    /// window has opened since set-up). Drives `ee_announce_done`.
    pub(super) ee_announce_covered: std::collections::HashSet<u32>,
    /// Formal CMCE CC state for this call leg. Absence from active_calls means Idle.
    pub(super) formal_state: CcFormalState,
    /// When PTT was released (for hangtime). None if transmitting.
    pub(super) hangtime_start: Option<TdmaTime>,
    /// One pending floor request while another user is transmitting.
    pub(super) queued_tx_demand: Option<TetraAddress>,
    /// Brew session UUID — set when a network speaker is active on this call,
    /// regardless of call origin. Cleared when the network speaker ends.
    pub(super) brew_uuid: Option<uuid::Uuid>,
}

impl ActiveCall {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_local(
        caller_addr: TetraAddress,
        dest_gssi: u32,
        source_issi: u32,
        carrier_num: u16,
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
            created_at,
            call_timeout,
            priority,
            carrier_num,
            ts,
            usage,
            tx_active: true,
            ee_announce_done: false,
            ee_announce_covered: std::collections::HashSet::new(),
            formal_state: CcFormalState::Idle
                .after(CcFormalEvent::SetupRequest)
                .after(CcFormalEvent::SetupComplete),
            hangtime_start: None,
            queued_tx_demand: None,
            brew_uuid: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_network(
        brew_uuid: uuid::Uuid,
        dest_gssi: u32,
        source_issi: u32,
        carrier_num: u16,
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
            created_at,
            call_timeout,
            priority,
            carrier_num,
            ts,
            usage,
            tx_active: true,
            ee_announce_done: false,
            ee_announce_covered: std::collections::HashSet::new(),
            formal_state: CcFormalState::Idle
                .after(CcFormalEvent::SetupRequest)
                .after(CcFormalEvent::SetupComplete),
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

    pub(super) fn begin_release(&mut self, cause: DisconnectCause) {
        let event = match cause {
            DisconnectCause::ExpiryOfTimer => CcFormalEvent::TimerExpired,
            _ => CcFormalEvent::ReleaseRequest,
        };
        self.formal_state = match self.formal_state.transition(event) {
            Ok(next) => next,
            Err(_) if self.formal_state == CcFormalState::Release => CcFormalState::Release,
            Err(_) => CcFormalState::Release,
        };
    }

    pub(super) fn begin_disconnect(&mut self) {
        if self.formal_state == CcFormalState::Active {
            self.formal_state = self.formal_state.after(CcFormalEvent::DisconnectRequest);
        }
    }

    pub(super) fn begin_restore(&mut self) -> Result<(), CcFormalTransitionError> {
        self.formal_state = self.formal_state.transition(CcFormalEvent::RestoreRequest)?;
        Ok(())
    }

    pub(super) fn complete_restore(&mut self) {
        self.formal_state = self.formal_state.after(CcFormalEvent::RestoreComplete);
    }

    pub(super) fn apply_modify(&mut self) -> Result<(), CcFormalTransitionError> {
        self.formal_state = self.formal_state.transition(CcFormalEvent::ModifyRequest)?;
        Ok(())
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

impl IndividualCallState {
    #[inline]
    pub(super) fn formal_state(self) -> CcFormalState {
        match self {
            IndividualCallState::CallSetupPending
            | IndividualCallState::IncomingSetupPending
            | IndividualCallState::IncomingSetupWaitNetworkAck
            | IndividualCallState::IncomingAlerting => CcFormalState::Setup,
            IndividualCallState::Active => CcFormalState::Active,
        }
    }
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
    pub(super) calling_carrier_num: u16,
    pub(super) calling_ts: u8,
    pub(super) called_carrier_num: u16,
    pub(super) called_ts: u8,
    pub(super) calling_usage: u8,
    pub(super) called_usage: u8,
    pub(super) simplex_duplex: bool,
    /// ETSI EN 300 392-2 clause 14.8 call priority (0..=15; 15 = emergency). Used for
    /// pre-emptive priority handling: a higher-priority set-up may release this call.
    pub(super) priority: u8,
    pub(super) state: IndividualCallState,
    /// Formal CMCE CC state for this call leg. Absence from individual_calls means Idle.
    pub(super) formal_state: CcFormalState,
    /// Start instant for setup timeout (T301/T302 equivalent on BS side).
    pub(super) setup_timer_started: Option<TdmaTime>,
    /// Setup timeout value used while the call is not active.
    pub(super) setup_timeout: Option<CallTimeoutSetupPhase>,
    /// Start instant for active call timeout (T310).
    pub(super) active_timer_started: Option<TdmaTime>,
    /// Active call timeout value.
    pub(super) call_timeout: CallTimeout,
    /// True when the called party lives behind Brew/TetraPack (PBX/phone/non-local ISSI).
    pub(super) called_over_brew: bool,
    /// True when the calling party lives behind Brew/TetraPack.
    pub(super) calling_over_brew: bool,
    /// Network entity bridging this call (Brew or Asterisk). Call-control messages and
    /// floor/DTMF signalling for the network leg are routed to this entity rather than
    /// hardcoding `TetraEntity::Brew`, so Brew calls reach Brew and Asterisk calls reach
    /// the SIP/RTP bridge.
    pub(super) network_entity: TetraEntity,
    /// Brew UUID when this call is bridged to TetraPack.
    pub(super) brew_uuid: Option<uuid::Uuid>,
    /// Cached network call metadata for Brew bridged legs.
    pub(super) network_call: Option<NetworkCircuitCall>,
    /// True once CONNECT_REQUEST has been sent for Brew-originated setup.
    pub(super) connect_request_sent: bool,
    /// SSI of the current simplex floor holder. None for duplex calls or when no MS currently has the floor.
    pub(super) floor_holder: Option<u32>,
    /// One pending simplex floor request while another party is transmitting.
    pub(super) queued_tx_demand: Option<TetraAddress>,
}

impl IndividualCall {
    #[inline]
    pub(super) fn is_local_echo_call(&self) -> bool {
        self.called_addr.ssi == LOCAL_ECHO_ISSI
            && self.called_handle.is_none()
            && self.called_carrier_num == self.calling_carrier_num
            && self.called_ts == self.calling_ts
            && !self.calling_over_brew
            && !self.called_over_brew
    }

    #[inline]
    pub(super) fn is_alerted(&self) -> bool {
        matches!(
            self.state,
            IndividualCallState::IncomingAlerting | IndividualCallState::IncomingSetupWaitNetworkAck | IndividualCallState::Active
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

    #[inline]
    pub(super) fn is_simplex(&self) -> bool {
        !self.simplex_duplex
    }

    #[inline]
    pub(super) fn is_floor_held_by(&self, issi: u32) -> bool {
        self.floor_holder == Some(issi)
    }

    pub(super) fn grant_floor(&mut self, holder: TetraAddress) {
        self.floor_holder = Some(holder.ssi);
        self.queued_tx_demand = None;
    }

    pub(super) fn release_floor(&mut self) {
        self.floor_holder = None;
        self.queued_tx_demand = None;
    }

    pub(super) fn queue_tx_demand(&mut self, requester: TetraAddress) -> TxDemandQueueResult {
        if self.is_floor_held_by(requester.ssi) {
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

    pub(super) fn cancel_queued_tx_demand(&mut self, requester: TetraAddress) -> bool {
        if self.queued_tx_demand.is_some_and(|existing| existing.ssi == requester.ssi) {
            self.queued_tx_demand = None;
            true
        } else {
            false
        }
    }

    pub(super) fn take_queued_tx_demand(&mut self) -> Option<TetraAddress> {
        self.queued_tx_demand.take()
    }

    pub(super) fn activate(&mut self, now: TdmaTime) {
        self.formal_state = self.formal_state.after(CcFormalEvent::SetupComplete);
        self.state = IndividualCallState::Active;
        self.setup_timer_started = None;
        self.setup_timeout = None;
        self.active_timer_started = Some(now);
        self.connect_request_sent = false;
    }

    pub(super) fn begin_disconnect(&mut self) {
        if self.formal_state == CcFormalState::Active {
            self.formal_state = self.formal_state.after(CcFormalEvent::DisconnectRequest);
        }
    }

    pub(super) fn begin_release(&mut self, cause: DisconnectCause) {
        let event = match cause {
            DisconnectCause::ExpiryOfTimer => CcFormalEvent::TimerExpired,
            _ => CcFormalEvent::ReleaseRequest,
        };
        self.formal_state = match self.formal_state.transition(event) {
            Ok(next) => next,
            Err(_) if self.formal_state == CcFormalState::Release => CcFormalState::Release,
            Err(_) => CcFormalState::Release,
        };
    }

    pub(super) fn begin_restore(&mut self) -> Result<(), CcFormalTransitionError> {
        self.formal_state = self.formal_state.transition(CcFormalEvent::RestoreRequest)?;
        Ok(())
    }

    pub(super) fn complete_restore(&mut self) {
        self.formal_state = self.formal_state.after(CcFormalEvent::RestoreComplete);
    }

    pub(super) fn apply_modify(&mut self) -> Result<(), CcFormalTransitionError> {
        self.formal_state = self.formal_state.transition(CcFormalEvent::ModifyRequest)?;
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{CcFormalEvent, CcFormalState};

    #[test]
    fn formal_cc_setup_active_release_flow() {
        let setup = CcFormalState::Idle.transition(CcFormalEvent::SetupRequest).unwrap();
        assert_eq!(setup, CcFormalState::Setup);

        let active = setup.transition(CcFormalEvent::SetupComplete).unwrap();
        assert_eq!(active, CcFormalState::Active);

        let release = active.transition(CcFormalEvent::ReleaseRequest).unwrap();
        assert_eq!(release, CcFormalState::Release);

        let idle = release.transition(CcFormalEvent::CleanupComplete).unwrap();
        assert_eq!(idle, CcFormalState::Idle);
    }

    #[test]
    fn formal_cc_disconnect_release_flow() {
        let disconnect = CcFormalState::Active.transition(CcFormalEvent::DisconnectRequest).unwrap();
        assert_eq!(disconnect, CcFormalState::Disconnect);

        let release = disconnect.transition(CcFormalEvent::ReleaseRequest).unwrap();
        assert_eq!(release, CcFormalState::Release);
    }

    #[test]
    fn formal_cc_restoration_success_and_failure_flows() {
        let restore = CcFormalState::Active.transition(CcFormalEvent::RestoreRequest).unwrap();
        assert_eq!(restore, CcFormalState::Restore);
        assert_eq!(restore.transition(CcFormalEvent::RestoreComplete).unwrap(), CcFormalState::Active);
        assert_eq!(restore.transition(CcFormalEvent::RestoreReject).unwrap(), CcFormalState::Release);
    }

    #[test]
    fn formal_cc_rejects_invalid_shortcuts() {
        assert!(CcFormalState::Idle.transition(CcFormalEvent::SetupComplete).is_err());
        assert!(CcFormalState::Setup.transition(CcFormalEvent::RestoreRequest).is_err());
        assert!(CcFormalState::Release.transition(CcFormalEvent::RestoreComplete).is_err());
    }
}
