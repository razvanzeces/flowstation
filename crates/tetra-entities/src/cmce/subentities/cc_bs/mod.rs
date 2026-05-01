use std::collections::{HashMap, HashSet};

use tetra_config::flowstation::SharedConfig;
use tetra_core::typed_pdu_fields::Type3FieldGeneric;
use tetra_core::{
    BitBuffer, Direction, Layer2Service, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner,
    TxReporter, tetra_entities::TetraEntity, unimplemented_log,
};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::{
    enums::{
        call_timeout::CallTimeout,
        call_timeout_setup_phase::CallTimeoutSetupPhase,
        cmce_pdu_type_ul::CmcePduTypeUl,
        transmission_grant::TransmissionGrant,
        type3_elem_id::CmceType3ElemId,
    },
    fields::basic_service_information::BasicServiceInformation,
    pdus::{
        d_alert::DAlert,
        d_call_proceeding::DCallProceeding,
        d_connect::DConnect,
        d_connect_acknowledge::DConnectAcknowledge,
        d_disconnect::DDisconnect,
        d_release::DRelease,
        d_setup::DSetup,
        d_tx_ceased::DTxCeased,
        d_tx_granted::DTxGranted,
        u_alert::UAlert,
        u_connect::UConnect,
        u_disconnect::UDisconnect,
        u_info::UInfo,
        u_release::URelease,
        u_setup::USetup,
        u_tx_ceased::UTxCeased,
        u_tx_demand::UTxDemand,
    },
    structs::cmce_circuit::CmceCircuit,
};
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::{
        brew::{BrewSubscriberAction, MmSubscriberUpdate},
        call_control::{CallControl, Circuit, CircuitDlMediaSource, NetworkCircuitCall},
        enums::{circuit_mode_type::CircuitModeType, communication_type::CommunicationType},
    },
    lcmc::{
        LcmcMleUnitdataReq,
        enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment},
        fields::chan_alloc_req::CmceChanAllocReq,
    },
};

use crate::net_brew;
use crate::{
    MessageQueue,
    cmce::components::circuit_mgr::{CircuitMgr, CircuitMgrCmd},
};

mod call;
mod dtmf;
mod fsm;
mod ingress;
mod network;
mod shared;
mod timers;

use call::{
    ActiveCall, CallOrigin, GroupCallState, IndividualCall, IndividualCallState,
    TxDemandQueueResult,
};
use fsm::{GroupTransitionError, IndividualTransitionError};

struct CachedSetup {
    pdu: DSetup,
    dest_addr: TetraAddress,
    resend: bool,
}

/// Clause 14 Call Control CMCE sub-entity (ETSI EN 300 392-2)
/// Supports group calls (simplex PTT), individual calls (full-duplex P2P),
/// and circuit-switched calls bridged over Brew/TetraPack.
pub struct CcBsSubentity {
    config: SharedConfig,
    dltime: TdmaTime,
    /// Cached D-SETUP PDUs for late-entry re-sends: call_id -> cached setup
    cached_setups: HashMap<u16, CachedSetup>,
    circuits: CircuitMgr,
    /// Active group calls: call_id -> call info
    active_calls: HashMap<u16, ActiveCall>,
    /// Active or pending individual calls (P2P / duplex)
    individual_calls: HashMap<u16, IndividualCall>,
    /// Registered subscriber groups (ISSI -> set of GSSIs)
    subscriber_groups: HashMap<u32, HashSet<u32>>,
    /// Listener counts per GSSI
    group_listeners: HashMap<u32, usize>,
}
