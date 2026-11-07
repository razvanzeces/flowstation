use std::collections::{HashMap, HashSet};

use tetra_config::bluestation::SharedConfig;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{
    BitBuffer, CarrierSlot, Direction, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, TxReporter,
    tetra_entities::TetraEntity, unimplemented_log,
};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::{
    enums::{
        call_status::CallStatus, call_timeout::CallTimeout, call_timeout_setup_phase::CallTimeoutSetupPhase,
        cmce_pdu_type_ul::CmcePduTypeUl, party_type_identifier::PartyTypeIdentifier, transmission_grant::TransmissionGrant,
        type3_elem_id::CmceType3ElemId,
    },
    fields::basic_service_information::BasicServiceInformation,
    pdus::{
        d_alert::DAlert, d_call_proceeding::DCallProceeding, d_call_restore::DCallRestore, d_connect::DConnect,
        d_connect_acknowledge::DConnectAcknowledge, d_disconnect::DDisconnect, d_info::DInfo, d_release::DRelease, d_setup::DSetup,
        d_tx_ceased::DTxCeased, d_tx_granted::DTxGranted, u_alert::UAlert, u_call_restore::UCallRestore, u_connect::UConnect,
        u_disconnect::UDisconnect, u_info::UInfo, u_release::URelease, u_setup::USetup, u_tx_ceased::UTxCeased, u_tx_demand::UTxDemand,
    },
    structs::cmce_circuit::CmceCircuit,
};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::{
        brew::{BrewSubscriberAction, MmSubscriberUpdate},
        call_control::{CallControl, Circuit, CircuitDlMediaSource, NetworkCircuitCall, NetworkRestoredCall},
        enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType},
    },
    lcmc::{
        LcmcMleUnitdataReq,
        enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
        fields::chan_alloc_req::CmceChanAllocReq,
    },
};

use crate::net_brew as brew;
use crate::{
    MessageQueue,
    cmce::components::circuit_mgr::{CircuitMgr, CircuitMgrCmd},
};

mod dtmf;
mod lifecycle;
mod pdu;
mod procedures;
mod routes;
mod state;
mod timers;

use lifecycle::{
    BrewNotification, CallTimeslot, GroupFloorGrant, PreemptPendingReady, PREEMPT_CEASE_INTERVAL_TS,
    PREEMPT_MIN_HOLD_TS, PREEMPT_POST_CEASE_TS, PREEMPT_READY_DEADLINE_TS,
};
use pdu::is_emergency_priority;
use procedures::{DeferredIndividualRestore, DeferredRestore, GroupTransitionError, HandoverReservation, IndividualTransitionError};
use state::{
    ActiveCall, CachedSetup, CallOrigin, CcFormalEvent, CcFormalState, GroupCallState, IndividualCall, IndividualCallState,
    TxDemandQueueResult,
};

/// Clause 11 Call Control CMCE sub-entity
pub struct CcBsSubentity {
    config: SharedConfig,
    dltime: TdmaTime,
    /// Cached D-SETUP PDUs for late-entry re-sends: call_id -> cached setup
    cached_setups: HashMap<u16, CachedSetup>,
    circuits: CircuitMgr,
    /// Active group calls: call_id -> call info
    active_calls: HashMap<u16, ActiveCall>,
    /// Active or pending individual calls (P2P)
    individual_calls: HashMap<u16, IndividualCall>,
    /// Registered subscriber groups (ISSI -> set of GSSIs)
    subscriber_groups: HashMap<u32, HashSet<u32>>,
    /// Listener counts per GSSI
    group_listeners: HashMap<u32, usize>,
    /// After hard-preempt of a local MS by LST/Brew: hold hangtime, cease, defer Ready.
    preempt_pending: HashMap<u16, PreemptPendingReady>,
    /// Slots with UL still hot (TrafficUlActivity) — blocks soft Ready after NetworkCallEnd.
    ul_slot_hot: HashMap<(u16, u8), bool>,
    /// Last local ISSI preempted on a slot (for soft-restart cease targeting).
    ul_slot_preempted_issi: HashMap<(u16, u8), u32>,
    /// Dashboard telemetry sink (call-lifecycle events). `None` when telemetry is disabled.
    telemetry: Option<crate::net_telemetry::TelemetrySink>,
    /// ISSIs armed for ambience listening by the control channel ahead of the network setup
    /// that carries the speech path: ISSI -> time the command arrived.
    ambience_armed: HashMap<u32, TdmaTime>,
    /// U-CALL RESTOREs waiting for a Brew group call to come up on this cell
    deferred_restores: Vec<DeferredRestore>,
    /// U-CALL RESTOREs of individual calls waiting for the network's answer (separate station)
    deferred_individual_restores: Vec<DeferredIndividualRestore>,
    /// (ISSI, GSSI) listener counts added by a restore rather than an MM affiliation
    restore_listeners: HashSet<(u32, u32)>,
    /// Multi-cell: groups of radios registered on a sibling cell of the same location area, which
    /// may be here without registering (each (ISSI, GSSI) counts once as a listener)
    area_members: HashMap<u32, HashSet<u32>>,
    /// MSs arriving by announced type 1 cell reselection whose group call is not up here yet
    handover_reservations: Vec<HandoverReservation>,
}
