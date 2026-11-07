mod common;

use std::time::Duration;

use tetra_config::bluestation::{CfgBrew, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Direction, Sap, SsiType, TdmaTime, TetraAddress, TxState, debug};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::{
    call_timeout::CallTimeout, cmce_pdu_type_dl::CmcePduTypeDl, party_type_identifier::PartyTypeIdentifier,
    transmission_grant::TransmissionGrant,
};
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::{
    d_call_restore::DCallRestore, d_connect::DConnect, d_connect_acknowledge::DConnectAcknowledge, d_release::DRelease, d_setup::DSetup,
    d_tx_ceased::DTxCeased, d_tx_granted::DTxGranted, u_call_restore::UCallRestore, u_connect::UConnect, u_disconnect::UDisconnect,
    u_setup::USetup, u_tx_ceased::UTxCeased, u_tx_demand::UTxDemand,
};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::call_control::{CallControl, NetworkCircuitCall, NetworkRestoredCall};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use crate::common::ComponentTest;
use crate::common::default_stack::default_test_config_bs;

const TEST_GSSI: u32 = 91;
const TEST_ISSI: u32 = 1000001;
const SECONDARY_CARRIER: u16 = 1522;

/// Helper: register a subscriber on a GSSI so CMCE accepts calls for that group.
fn register_subscriber(test: &mut ComponentTest, issi: u32, gssi: u32) {
    let register = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![],
            action: BrewSubscriberAction::Register,
        }),
    };
    test.submit_message(register);
    test.run_stack(Some(1));

    let affiliate = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        }),
    };
    test.submit_message(affiliate);
    test.run_stack(Some(1));
    test.dump_sinks();
}

/// Helper: build a U-SETUP SAP message for a group call.
fn build_u_setup_msg(calling_issi: u32, dest_gssi: u32) -> SapMsg {
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(dest_gssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("Failed to serialize USetup");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-SETUP SAP message for an individual call.
fn build_individual_u_setup_msg(calling_issi: u32, called_issi: u32) -> SapMsg {
    build_individual_u_setup_msg_with_mode(calling_issi, called_issi, true)
}

fn build_individual_u_setup_msg_with_mode(calling_issi: u32, called_issi: u32, simplex_duplex_selection: bool) -> SapMsg {
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: true,
        simplex_duplex_selection,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(called_issi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("Failed to serialize USetup");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-SETUP SAP message for a group call with an explicit ETSI call priority
/// (0..=15; 15 = emergency). Used to exercise emergency / pre-emptive call handling.
fn build_u_setup_msg_prio(calling_issi: u32, dest_gssi: u32, call_priority: u8) -> SapMsg {
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2Mp,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(dest_gssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("Failed to serialize USetup");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Helper: build a U-SETUP SAP message for a simplex P2P (individual) call to `called_issi`.
fn build_u_setup_p2p_msg(calling_issi: u32, called_issi: u32) -> SapMsg {
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: false,
        simplex_duplex_selection: false,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority: 0,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(called_issi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("Failed to serialize USetup");
    sdu.seek(0);

    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(calling_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Count individual-call D-SETUP resends addressed to `ssi` on the MCCH (no chan_alloc).
fn count_individual_dsetup_to(msgs: &[SapMsg], ssi: u32) -> usize {
    msgs.iter()
        .filter(|m| {
            m.dest == TetraEntity::Mle
                && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(p)
                    if p.main_address.ssi == ssi && p.chan_alloc.is_none())
        })
        .count()
}

fn lcmc_ind(sender_issi: u32, sdu: BitBuffer) -> SapMsg {
    SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(sender_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

fn build_u_connect_msg(sender_issi: u32, call_id: u16, simplex_duplex_selection: bool) -> SapMsg {
    let u_connect = UConnect {
        call_identifier: call_id,
        hook_method_selection: true,
        simplex_duplex_selection,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_connect.to_bitbuf(&mut sdu).expect("Failed to serialize UConnect");
    sdu.seek(0);
    lcmc_ind(sender_issi, sdu)
}

fn build_u_tx_demand_msg(sender_issi: u32, call_id: u16) -> SapMsg {
    let u_tx_demand = UTxDemand {
        call_identifier: call_id,
        tx_demand_priority: 0,
        encryption_control: false,
        reserved: false,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_tx_demand.to_bitbuf(&mut sdu).expect("Failed to serialize UTxDemand");
    sdu.seek(0);
    lcmc_ind(sender_issi, sdu)
}

fn build_u_disconnect_msg(sender_issi: u32, call_id: u16) -> SapMsg {
    let u_disconnect = UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_disconnect.to_bitbuf(&mut sdu).expect("Failed to serialize UDisconnect");
    sdu.seek(0);
    lcmc_ind(sender_issi, sdu)
}

/// Helper: build a U-CALL RESTORE for `call_id`, optionally asking for the floor ("request to
/// transmit/send data" 0 asks to transmit, 1 lets another MS transmit, table 14.74).
fn build_u_call_restore_msg(sender_issi: u32, call_id: u16, other_party_ssi: u32, request_to_transmit: bool) -> SapMsg {
    let u_call_restore = UCallRestore {
        call_identifier: call_id,
        request_to_transmit_send_data: !request_to_transmit,
        other_party_type_identifier: 1, // SSI
        other_party_short_number_address: None,
        other_party_ssi: Some(other_party_ssi as u64),
        other_party_extension: None,
        basic_service_information: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_call_restore.to_bitbuf(&mut sdu).expect("Failed to serialize UCallRestore");
    sdu.seek(0);
    lcmc_ind(sender_issi, sdu)
}

fn build_u_tx_ceased_msg(sender_issi: u32, call_id: u16) -> SapMsg {
    let u_tx_ceased = UTxCeased {
        call_identifier: call_id,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_tx_ceased.to_bitbuf(&mut sdu).expect("Failed to serialize UTxCeased");
    sdu.seek(0);
    lcmc_ind(sender_issi, sdu)
}

fn dl_pdu_type(sdu: &BitBuffer) -> Option<CmcePduTypeDl> {
    CmcePduTypeDl::try_from(sdu.peek_bits(5)?).ok()
}

/// The D-CALL RESTORE sent to `issi` and the (carrier, timeslot) of the channel allocation that
/// goes with it, if any.
fn d_call_restore_to(msgs: &[SapMsg], issi: u32) -> Option<(DCallRestore, Option<(u16, usize)>)> {
    msgs.iter().find_map(|msg| {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return None;
        };
        if msg.dest != TetraEntity::Mle || prim.main_address.ssi != issi || dl_pdu_type(&prim.sdu) != Some(CmcePduTypeDl::DCallRestore) {
            return None;
        }
        let mut sdu = prim.sdu.clone();
        let pdu = DCallRestore::from_bitbuf(&mut sdu).expect("Failed to parse DCallRestore");
        let chan = prim.chan_alloc.as_ref().map(|c| {
            (c.carrier.expect("carrier"), c.timeslots.iter().position(|&t| t).expect("timeslot") + 1)
        });
        Some((pdu, chan))
    })
}

fn find_lcmc_req(msgs: &[SapMsg], address_issi: u32, pdu_type: CmcePduTypeDl) -> Option<(BitBuffer, Option<UlDlAssignment>)> {
    msgs.iter().find_map(|msg| {
        if msg.dest != TetraEntity::Mle {
            return None;
        }

        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return None;
        };

        if prim.main_address.ssi != address_issi || dl_pdu_type(&prim.sdu) != Some(pdu_type) {
            return None;
        }

        Some((
            prim.sdu.clone(),
            prim.chan_alloc.as_ref().map(|chan_alloc| chan_alloc.ul_dl_assigned),
        ))
    })
}

fn first_d_setup_call_id(msgs: &[SapMsg], called_issi: u32) -> u16 {
    let (mut sdu, _) = find_lcmc_req(msgs, called_issi, CmcePduTypeDl::DSetup).expect("Expected D-SETUP to called ISSI");
    let d_setup = DSetup::from_bitbuf(&mut sdu).expect("Failed to parse DSetup");
    d_setup.call_identifier
}

/// (carrier, timeslot) of the channel allocation sent with the first D-SETUP to `gssi`.
fn d_setup_channel(msgs: &[SapMsg], gssi: u32) -> (u16, usize) {
    msgs.iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::LcmcMleUnitdataReq(prim)
                if msg.dest == TetraEntity::Mle
                    && prim.main_address.ssi == gssi
                    && dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DSetup) =>
            {
                prim.chan_alloc.as_ref().map(|c| {
                    (c.carrier.expect("carrier"), c.timeslots.iter().position(|&t| t).expect("timeslot") + 1)
                })
            }
            _ => None,
        })
        .expect("D-SETUP with channel allocation")
}

fn connected_simplex_individual_call(calling_issi: u32, called_issi: u32) -> (ComponentTest, u16, Vec<SapMsg>) {
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);
    test.config.state_write().subscribers.register(called_issi);

    test.submit_message(build_individual_u_setup_msg_with_mode(calling_issi, called_issi, false));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, called_issi);

    test.submit_message(build_u_connect_msg(called_issi, call_id, false));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    (test, call_id, connect_msgs)
}

fn connected_duplex_individual_call(calling_issi: u32, called_issi: u32) -> (ComponentTest, u16, Vec<SapMsg>) {
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);
    test.config.state_write().subscribers.register(called_issi);

    test.submit_message(build_individual_u_setup_msg_with_mode(calling_issi, called_issi, true));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, called_issi);

    test.submit_message(build_u_connect_msg(called_issi, call_id, true));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    (test, call_id, connect_msgs)
}

fn connected_brew_originated_simplex_call(remote_issi: u32, local_issi: u32) -> (ComponentTest, u16, uuid::Uuid) {
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);
    test.config.state_write().subscribers.register(local_issi);

    let brew_uuid = uuid::Uuid::parse_str("a9661625-c1f2-42bb-b256-c44e14677307").unwrap();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: NetworkCircuitCall {
                source_issi: remote_issi,
                destination: local_issi,
                number: String::new(),
                priority: 1,
                service: 0,
                mode: 0,
                duplex: 0,
                method: 0,
                communication: 0,
                grant: TransmissionGrant::NotGranted.into_raw() as u8,
                permission: 0,
                timeout: 0,
                ownership: 0,
                queued: 0,
            },
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, local_issi);

    test.submit_message(build_u_connect_msg(local_issi, call_id, false));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    (test, call_id, brew_uuid)
}

/// Extract tx_reporters from D-SETUP messages in the sink output.
/// D-SETUPs are identified as LcmcMleUnitdataReq with a chan_alloc that has a usage field.
fn extract_d_setup_reporters(msgs: &mut Vec<SapMsg>) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle {
            if let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg {
                if prim.chan_alloc.as_ref().is_some_and(|ca| ca.usage.is_some()) {
                    if let Some(reporter) = prim.tx_reporter.take() {
                        reporters.push(reporter);
                    }
                }
            }
        }
    }
    reporters
}

/// Count D-SETUP messages in sink output without taking reporters.
fn count_d_setups(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|msg| {
            msg.dest == TetraEntity::Mle
                && matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim)
                    if dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DSetup))
        })
        .count()
}

#[test]
fn test_individual_setup_uses_central_subscriber_registry_for_local_destination() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let calling_issi = 1000001;
    let called_issi = 1000002;
    test.config.state_write().subscribers.register(called_issi);

    test.submit_message(build_individual_u_setup_msg(calling_issi, called_issi));
    test.run_stack(Some(1));

    let msgs = test.dump_sinks();
    assert!(
        count_d_setups(&msgs) > 0,
        "Expected local D-SETUP for centrally registered called ISSI"
    );
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "Local registered ISSI should not be routed over Brew"
    );
}

#[test]
fn test_duplex_individual_uses_infinite_timeout() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let calling_issi = 1000001;
    let called_issi = 1000002;
    test.config.state_write().subscribers.register(called_issi);

    test.submit_message(build_individual_u_setup_msg(calling_issi, called_issi));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let (mut setup_sdu, _) = find_lcmc_req(&setup_msgs, called_issi, CmcePduTypeDl::DSetup).expect("Expected D-SETUP to called ISSI");
    let d_setup = DSetup::from_bitbuf(&mut setup_sdu).expect("Failed to parse DSetup");
    assert_eq!(d_setup.call_time_out, CallTimeout::Infinite);
    assert!(d_setup.simplex_duplex_selection);
    let call_id = d_setup.call_identifier;

    test.submit_message(build_u_connect_msg(called_issi, call_id, true));
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let (mut connect_sdu, _) =
        find_lcmc_req(&connect_msgs, calling_issi, CmcePduTypeDl::DConnect).expect("Expected D-CONNECT to calling ISSI");
    let d_connect = DConnect::from_bitbuf(&mut connect_sdu).expect("Failed to parse DConnect");
    assert_eq!(d_connect.call_time_out, CallTimeout::Infinite);
    assert!(d_connect.simplex_duplex_selection);

    let (mut ack_sdu, _) = find_lcmc_req(&connect_msgs, called_issi, CmcePduTypeDl::DConnectAcknowledge)
        .expect("Expected D-CONNECT ACKNOWLEDGE to called ISSI");
    let d_ack = DConnectAcknowledge::from_bitbuf(&mut ack_sdu).expect("Failed to parse DConnectAcknowledge");
    assert_eq!(CallTimeout::try_from(d_ack.call_time_out as u64).ok(), Some(CallTimeout::Infinite));
}

#[test]
fn test_simplex_individual_connect_grants_calling_ms_initial_floor() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (_test, call_id, msgs) = connected_simplex_individual_call(calling_issi, called_issi);

    let (mut connect_sdu, connect_alloc) =
        find_lcmc_req(&msgs, calling_issi, CmcePduTypeDl::DConnect).expect("Expected D-CONNECT to calling ISSI");
    let d_connect = DConnect::from_bitbuf(&mut connect_sdu).expect("Failed to parse DConnect");
    assert_eq!(d_connect.call_identifier, call_id);
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted);
    assert_eq!(connect_alloc, Some(UlDlAssignment::Both));

    let (mut ack_sdu, ack_alloc) =
        find_lcmc_req(&msgs, called_issi, CmcePduTypeDl::DConnectAcknowledge).expect("Expected D-CONNECT ACKNOWLEDGE to called ISSI");
    let d_ack = DConnectAcknowledge::from_bitbuf(&mut ack_sdu).expect("Failed to parse DConnectAcknowledge");
    assert_eq!(d_ack.call_identifier, call_id);
    assert_eq!(d_ack.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    assert_eq!(ack_alloc, Some(UlDlAssignment::Both));

    assert!(msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, dest_gssi, .. })
            if *source_issi == calling_issi && *dest_gssi == called_issi
    )));
}

#[test]
fn test_individual_connect_mcch_fallback_uses_linkless_delivery() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (_test, _call_id, msgs) = connected_simplex_individual_call(calling_issi, called_issi);

    let caller_mcch_connect = msgs.iter().find(|msg| {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return false;
        };
        prim.main_address.ssi == calling_issi
            && !prim.stealing_permission
            && prim.chan_alloc.is_some()
            && prim.link_id == 0
            && dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DConnect)
    });
    assert!(
        caller_mcch_connect.is_some(),
        "expected D-CONNECT MCCH fallback for caller to be sent linkless"
    );

    let called_mcch_ack = msgs.iter().find(|msg| {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return false;
        };
        prim.main_address.ssi == called_issi
            && !prim.stealing_permission
            && prim.chan_alloc.is_some()
            && prim.link_id == 0
            && dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DConnectAcknowledge)
    });
    assert!(
        called_mcch_ack.is_some(),
        "expected D-CONNECT-ACK MCCH fallback for callee to be sent linkless"
    );
}

#[test]
#[ignore = "Brew connect-signaling rider, not dual-carrier: the Brew-routed setup is rejected before \
NetworkCircuitSetupRequest because this harness doesn't wire the backhaul state + source/destination \
ISSI whitelist the Brew gates require (see setup.rs). Belongs in its own PR with on-radio validation; \
re-enable it there."]
fn test_brew_connect_request_mcch_fallback_uses_linkless_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let calling_issi = 1000001;
    let remote_issi = 16777184;

    test.submit_message(build_individual_u_setup_msg_with_mode(calling_issi, remote_issi, true));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let (brew_uuid, network_call) = setup_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => Some((*brew_uuid, call.clone())),
            _ => None,
        })
        .expect("expected Brew setup request for non-local individual destination");

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }),
    });
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
            brew_uuid,
            call: network_call,
        }),
    });
    test.run_stack(Some(1));
    let connect_msgs = test.dump_sinks();

    let caller_mcch_connect = connect_msgs.iter().find(|msg| {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return false;
        };
        prim.main_address.ssi == calling_issi
            && !prim.stealing_permission
            && prim.chan_alloc.is_some()
            && prim.link_id == 0
            && dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DConnect)
    });
    assert!(
        caller_mcch_connect.is_some(),
        "expected Brew-routed D-CONNECT MCCH fallback for caller to be sent linkless"
    );
}

#[test]
fn test_brew_originated_simplex_connect_confirm_makes_local_ms_listener() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let remote_issi = 2200699;
    let local_issi = 2200769;
    let brew_uuid = uuid::Uuid::parse_str("a9661625-c1f2-42bb-b256-c44e14677307").unwrap();
    test.config.state_write().subscribers.register(local_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: NetworkCircuitCall {
                source_issi: remote_issi,
                destination: local_issi,
                number: String::new(),
                priority: 1,
                service: 0,
                mode: 0,
                duplex: 0,
                method: 0,
                communication: 0,
                grant: 1,
                permission: 0,
                timeout: 0,
                ownership: 0,
                queued: 0,
            },
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    assert!(setup_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid: accepted_uuid })
            if *accepted_uuid == brew_uuid
    )));
    let (mut setup_sdu, _) = find_lcmc_req(&setup_msgs, local_issi, CmcePduTypeDl::DSetup).expect("Expected D-SETUP to local ISSI");
    let d_setup = DSetup::from_bitbuf(&mut setup_sdu).expect("Failed to parse DSetup");
    assert_eq!(d_setup.calling_party_address_ssi, Some(remote_issi));
    assert!(!d_setup.hook_method_selection);
    assert_eq!(d_setup.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    let call_id = d_setup.call_identifier;

    test.submit_message(build_u_connect_msg(local_issi, call_id, false));
    test.run_stack(Some(1));
    let connect_request_msgs = test.dump_sinks();
    assert!(connect_request_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid: request_uuid, .. })
            if *request_uuid == brew_uuid
    )));

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();

    let (mut ack_sdu, ack_alloc) =
        find_lcmc_req(&confirm_msgs, local_issi, CmcePduTypeDl::DConnectAcknowledge).expect("Expected D-CONNECT ACKNOWLEDGE to local ISSI");
    let d_ack = DConnectAcknowledge::from_bitbuf(&mut ack_sdu).expect("Failed to parse DConnectAcknowledge");
    assert_eq!(d_ack.call_identifier, call_id);
    assert_eq!(d_ack.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    assert_eq!(ack_alloc, Some(UlDlAssignment::Both));

    assert!(confirm_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::Open(circuit))
            if circuit.direction == Direction::Both && circuit.ts == 2
    )));
    assert!(
        !confirm_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::Open(circuit))
                if (circuit.direction == Direction::Ul || circuit.direction == Direction::Dl) && circuit.ts == 2
        )),
        "Brew-originated simplex media should open the shared traffic circuit once"
    );

    assert!(confirm_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
            brew_uuid: ready_uuid,
            call_id: ready_call_id,
            carrier_num: _,
            ts: 2,
        }) if *ready_uuid == brew_uuid && *ready_call_id == call_id
    )));
    assert!(
        !confirm_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. })
                if *source_issi == local_issi
        )),
        "Brew-originated media must not grant the local called MS uplink floor"
    );
}

#[test]
fn test_brew_connect_confirm_mcch_fallback_uses_linkless_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let remote_issi = 2200699;
    let local_issi = 2200769;
    let brew_uuid = uuid::Uuid::parse_str("a9661625-c1f2-42bb-b256-c44e14677307").unwrap();
    test.config.state_write().subscribers.register(local_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: NetworkCircuitCall {
                source_issi: remote_issi,
                destination: local_issi,
                number: String::new(),
                priority: 1,
                service: 0,
                mode: 0,
                duplex: 0,
                method: 0,
                communication: 0,
                grant: TransmissionGrant::NotGranted.into_raw() as u8,
                permission: 0,
                timeout: 0,
                ownership: 0,
                queued: 0,
            },
        }),
    });
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, local_issi);

    test.submit_message(build_u_connect_msg(local_issi, call_id, false));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let confirm_msgs = test.dump_sinks();

    let called_mcch_ack = confirm_msgs.iter().find(|msg| {
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return false;
        };
        prim.main_address.ssi == local_issi
            && !prim.stealing_permission
            && prim.chan_alloc.is_some()
            && prim.link_id == 0
            && dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DConnectAcknowledge)
    });
    assert!(
        called_mcch_ack.is_some(),
        "expected Brew-routed D-CONNECT-ACK MCCH fallback for local callee to be sent linkless"
    );
}

#[test]
fn test_brew_originated_simplex_remote_idle_hands_floor_to_queued_local_ms() {
    debug::setup_logging_verbose();

    let remote_issi = 2200699;
    let local_issi = 2200769;
    let (mut test, call_id, brew_uuid) = connected_brew_originated_simplex_call(remote_issi, local_issi);

    test.submit_message(build_u_tx_demand_msg(local_issi, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();
    let (mut queued_sdu, queued_alloc) =
        find_lcmc_req(&demand_msgs, local_issi, CmcePduTypeDl::DTxGranted).expect("Expected queued D-TX GRANTED");
    let queued = DTxGranted::from_bitbuf(&mut queued_sdu).expect("Failed to parse queued DTxGranted");
    assert_eq!(queued.call_identifier, call_id);
    assert_eq!(queued.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_eq!(queued_alloc, Some(UlDlAssignment::Dl));
    assert!(
        !demand_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted { .. })
        )),
        "Local queued demand must not tell Brew that local already has the floor"
    );

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
            brew_uuid,
            grant: TransmissionGrant::NotGranted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let idle_msgs = test.dump_sinks();

    let (mut grant_sdu, grant_alloc) =
        find_lcmc_req(&idle_msgs, local_issi, CmcePduTypeDl::DTxGranted).expect("Expected local floor grant after Brew idle");
    let granted = DTxGranted::from_bitbuf(&mut grant_sdu).expect("Failed to parse granted DTxGranted");
    assert_eq!(granted.call_identifier, call_id);
    assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(grant_alloc, Some(UlDlAssignment::Ul));

    assert!(idle_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. })
            if *source_issi == local_issi
    )));
    assert!(idle_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
            brew_uuid: msg_uuid,
            grant,
            permission: 0,
        }) if *msg_uuid == brew_uuid && *grant == TransmissionGrant::Granted.into_raw() as u8
    )));
}

#[test]
fn test_brew_originated_simplex_local_tx_ceased_notifies_brew_idle() {
    debug::setup_logging_verbose();

    let remote_issi = 2200699;
    let local_issi = 2200769;
    let (mut test, call_id, brew_uuid) = connected_brew_originated_simplex_call(remote_issi, local_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
            brew_uuid,
            grant: TransmissionGrant::NotGranted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(build_u_tx_demand_msg(local_issi, call_id));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(build_u_tx_ceased_msg(local_issi, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    assert!(ceased_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
            brew_uuid: msg_uuid,
            grant,
            permission: 0,
        }) if *msg_uuid == brew_uuid && *grant == TransmissionGrant::NotGranted.into_raw() as u8
    )));
}

#[test]
fn test_brew_simplex_granted_resumes_remote_downlink_without_ul_timer() {
    debug::setup_logging_verbose();

    let remote_issi = 2200699;
    let local_issi = 2200769;
    let (mut test, call_id, brew_uuid) = connected_brew_originated_simplex_call(remote_issi, local_issi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let granted_msgs = test.dump_sinks();

    let (mut grant_sdu, grant_alloc) =
        find_lcmc_req(&granted_msgs, local_issi, CmcePduTypeDl::DTxGranted).expect("Expected listener D-TX GRANTED");
    let granted = DTxGranted::from_bitbuf(&mut grant_sdu).expect("Failed to parse listener DTxGranted");
    assert_eq!(granted.call_identifier, call_id);
    assert_eq!(granted.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    assert_eq!(grant_alloc, Some(UlDlAssignment::Dl));

    assert!(granted_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted {
            call_id: msg_call_id,
            carrier_num: _,
            ts: 2,
        })
            if *msg_call_id == call_id
    )));
    assert!(
        !granted_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. })
                if *source_issi == remote_issi
        )),
        "Remote Brew floor must not use local FloorGranted because that arms UL inactivity"
    );
}

#[test]
fn test_network_group_speaker_change_uses_remote_floor_grant() {
    debug::setup_logging_verbose();

    let gssi = 220;
    let local_issi = 2200699;
    let first_speaker = 2200107;
    let second_speaker = 2200061;
    let first_uuid = uuid::Uuid::parse_str("9179c03c-0489-4106-a246-5ccddf75e657").unwrap();
    let second_uuid = uuid::Uuid::parse_str("ad740a0d-8ab9-43c1-a09c-72590f4d39de").unwrap();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        feature_lip_forward: false,
        lip_forward_issi: None,
        pbx_gateway_issis: None,
        backhaul_hysteresis_secs: 0,
    });
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, local_issi, gssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid: first_uuid,
            source_issi: first_speaker,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(1));
    let initial_msgs = test.dump_sinks();
    let (call_id, ts) = initial_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid,
                call_id,
                carrier_num: _,
                ts,
                ..
            }) if *brew_uuid == first_uuid => Some((*call_id, *ts)),
            _ => None,
        })
        .expect("Expected first network call to become ready");

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid: second_uuid,
            source_issi: second_speaker,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(1));
    let speaker_change_msgs = test.dump_sinks();

    assert!(speaker_change_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted {
            call_id: msg_call_id,
            carrier_num: _,
            ts: msg_ts,
        })
            if *msg_call_id == call_id && *msg_ts == ts
    )));
    assert!(
        !speaker_change_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. })
                if *source_issi == second_speaker
        )),
        "Network group speakers must not use local FloorGranted because that arms UL inactivity"
    );
}

/// Local MS holds the group floor; NetworkCallStart (LST/Brew) must hard-preempt: cease/deny
/// the MS, FloorReleased to UMAC/Brew, then NetworkCallReady for the network speaker.
#[test]
fn test_network_preempts_local_group_speaker() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::from_config(brew_test_config(), Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, TEST_GSSI);

    assert!(
        setup_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. })
                if *source_issi == TEST_ISSI
        )),
        "local setup should grant floor to the MS"
    );

    let brew_uuid = uuid::Uuid::parse_str("b7e5c03c-0489-4106-a246-5ccddf75e657").unwrap();
    let lst_issi = 9_990_001u32;
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: lst_issi,
            dest_gssi: TEST_GSSI,
            priority: 0,
        }),
    });
    test.run_stack(Some(1));
    let preempt_msgs = test.dump_sinks();

    assert!(
        preempt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: released,
                ..
            }) if *released == call_id
        )),
        "preempt must FloorReleased so hangtime/AssignedControl stays on for cease"
    );
    assert!(
        find_lcmc_req(&preempt_msgs, TEST_ISSI, CmcePduTypeDl::DTxGranted).is_some()
            || find_lcmc_req(&preempt_msgs, TEST_GSSI, CmcePduTypeDl::DTxCeased).is_some(),
        "preempt must signal NotGranted to MS and/or D-TX CEASED on the GSSI"
    );
    assert!(
        preempt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::SetDlMediaSource {
                source,
                ..
            }) if *source == tetra_saps::control::call_control::CircuitDlMediaSource::SwMI
        )),
        "preempt must SetDlMediaSource(SwMI) in-place so walkie UL is not looped over LST DL"
    );
    assert!(
        !preempt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::Open(_))
        )),
        "preempt must NOT Open/teardown the existing group circuit"
    );
    assert!(
        !preempt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { .. })
        )),
        "hard preempt must defer NetworkCallReady until UL quiet / deadline"
    );
    assert!(
        !preempt_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted { .. })
        )),
        "hard preempt must defer RemoteFloorGranted while hangtime holds for cease"
    );

    let (carrier_num, ts) = preempt_msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::FloorReleased {
                call_id: released,
                carrier_num,
                ts,
                ..
            }) if *released == call_id => Some((*carrier_num, *ts)),
            _ => None,
        })
        .expect("FloorReleased must carry carrier/ts");

    // Early quiet must not Ready before min_hold (~400 ms).
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Umac,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::TrafficUlActivity {
            carrier_num,
            ts,
            active: false,
        }),
    });
    test.run_stack(Some(1));
    let early = test.dump_sinks();
    assert!(
        !early.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { .. })
        )),
        "must not NetworkCallReady before preempt min_hold"
    );

    // Advance past min_hold with UL still quiet → Ready.
    test.run_stack(Some(40));
    let ready_msgs = test.dump_sinks();
    assert!(
        ready_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady {
                brew_uuid: ready_uuid,
                call_id: ready_call,
                ..
            }) if *ready_uuid == brew_uuid && *ready_call == call_id
        )),
        "LST/Brew must receive NetworkCallReady after UL quiet + min_hold"
    );
    assert!(
        ready_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted {
                call_id: granted_call,
                ..
            }) if *granted_call == call_id
        )),
        "UMAC must see RemoteFloorGranted once Ready is allowed"
    );
}

/// Build a CfgBrew suitable for the network-call timeout tests.
fn test_brew_cfg() -> CfgBrew {
    CfgBrew {
        host: "127.0.0.1".into(),
        port: 0,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        feature_lip_forward: false,
        lip_forward_issi: None,
        pbx_gateway_issis: None,
        backhaul_hysteresis_secs: 0,
    }
}

/// ETSI EN 300 392-2 §14: the call time-out timer is an ABSOLUTE maximum call duration measured
/// from call establishment. Ongoing network media must NOT push it out — a previous change measured
/// it from the last media frame, letting an endlessly-streaming Brew call pin the traffic timeslot
/// forever. Here we keep media flowing (gaps below the media-inactivity window) and assert the call
/// is still released at created_at + call_timeout, never extended.
#[test]
fn test_network_group_call_absolute_timeout_is_not_extended_by_media() {
    debug::setup_logging_verbose();

    let gssi = 220;
    let local_issi = 9012003;
    let source_issi = 2200010;
    let brew_uuid = uuid::Uuid::new_v4();

    let mut config = default_test_config_bs();
    config.cell.call_timeout_secs = 30; // 30 s = 2160 timeslots
    config.brew = Some(test_brew_cfg());

    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, local_issi, gssi);

    let media_activity = |test: &mut ComponentTest| {
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallMediaActivity { brew_uuid }),
        });
    };

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(5));
    test.dump_sinks();

    // Keep media flowing with gaps (600 ts ≈ 8.3 s) below the 720 ts media-inactivity window, so the
    // only thing that can end the call is the absolute cap. After ~1800 ts we are still below the
    // 2160 ts cap → the call must still be up.
    for _ in 0..3 {
        media_activity(&mut test);
        test.run_stack(Some(600));
    }
    let mid = test.dump_sinks();
    assert!(
        !mid.iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. }))),
        "call must stay up while media flows and the absolute call time-out has not yet elapsed"
    );

    // Keep media flowing and cross the 2160 ts absolute cap. Pre-fix the late media would have
    // pushed release out to ~1800+2160 ts; ETSI-compliant behaviour releases at the absolute cap.
    media_activity(&mut test);
    test.run_stack(Some(600));
    let after = test.dump_sinks();
    assert!(
        after
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. }))),
        "ETSI: call must be released at the absolute call time-out regardless of ongoing media"
    );
}

/// A network (Brew) group call carries no local uplink, so UMAC never arms a UL-inactivity timer
/// and the call only drops to hang-time on a GROUP_IDLE. If the backhaul media simply dies (lost
/// GROUP_IDLE / zombie gateway stream), the downlink media-inactivity guard must reclaim the
/// traffic timeslot — this is the "timeslot stays busy after the radio/stream is gone" regression.
#[test]
fn test_network_group_call_released_when_backhaul_media_dies() {
    debug::setup_logging_verbose();

    let gssi = 220;
    let local_issi = 9012003;
    let source_issi = 2200010;
    let brew_uuid = uuid::Uuid::new_v4();

    let mut config = default_test_config_bs();
    config.cell.call_timeout_secs = 120; // long, so the absolute cap can't mask the inactivity guard
    config.brew = Some(test_brew_cfg());

    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, local_issi, gssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(5));
    test.dump_sinks();

    // The traffic slot is busy while the call is up.
    {
        let shared = test.get_shared_config();
        assert!(
            shared.state_read().active_call_ts.contains_key(&gssi),
            "network group call should occupy a traffic timeslot once established"
        );
    }

    // No further media arrives (lost GROUP_IDLE). Run past the 720 ts (10 s) inactivity window.
    test.run_stack(Some(800));
    let msgs = test.dump_sinks();
    assert!(
        msgs.iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. }))),
        "network call with dead backhaul media must be released by the media-inactivity guard"
    );

    // …and the timeslot must be reclaimed — the actual operator-visible symptom.
    let shared = test.get_shared_config();
    assert!(
        !shared.state_read().active_call_ts.contains_key(&gssi),
        "traffic timeslot must be reclaimed (active_call_ts cleared) after media-inactivity release"
    );
}

#[test]
fn test_simplex_individual_tx_ceased_without_queued_demand_releases_floor() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (mut test, call_id, _) = connected_simplex_individual_call(calling_issi, called_issi);

    test.submit_message(build_u_tx_ceased_msg(calling_issi, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    let (mut ceased_sdu, ceased_alloc) =
        find_lcmc_req(&ceased_msgs, calling_issi, CmcePduTypeDl::DTxCeased).expect("Expected D-TX CEASED to former speaker");
    let ceased = DTxCeased::from_bitbuf(&mut ceased_sdu).expect("Failed to parse DTxCeased");
    assert_eq!(ceased.call_identifier, call_id);
    assert!(!ceased.transmission_request_permission);
    assert_eq!(ceased_alloc, Some(UlDlAssignment::Dl));

    let (mut listener_ceased_sdu, listener_ceased_alloc) =
        find_lcmc_req(&ceased_msgs, called_issi, CmcePduTypeDl::DTxCeased).expect("Expected D-TX CEASED to listener");
    let listener_ceased = DTxCeased::from_bitbuf(&mut listener_ceased_sdu).expect("Failed to parse listener DTxCeased");
    assert_eq!(listener_ceased.call_identifier, call_id);
    assert_eq!(listener_ceased_alloc, Some(UlDlAssignment::Dl));

    assert!(
        find_lcmc_req(&ceased_msgs, calling_issi, CmcePduTypeDl::DTxGranted).is_none(),
        "U-TX CEASED without a queued requester must not auto-grant the peer"
    );
    assert!(
        find_lcmc_req(&ceased_msgs, called_issi, CmcePduTypeDl::DTxGranted).is_none(),
        "U-TX CEASED without a queued requester must not send a listener grant"
    );

    assert!(ceased_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id: released_call_id, .. })
            if *released_call_id == call_id
    )));
}

#[test]
fn test_simplex_individual_tx_demand_queues_and_hands_off_on_ceased() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (mut test, call_id, _) = connected_simplex_individual_call(calling_issi, called_issi);

    test.submit_message(build_u_tx_demand_msg(called_issi, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let (mut queued_sdu, queued_alloc) =
        find_lcmc_req(&demand_msgs, called_issi, CmcePduTypeDl::DTxGranted).expect("Expected queued D-TX GRANTED");
    let queued = DTxGranted::from_bitbuf(&mut queued_sdu).expect("Failed to parse queued DTxGranted");
    assert_eq!(queued.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);
    assert_eq!(queued_alloc, Some(UlDlAssignment::Dl));
    assert_eq!(queued.transmitting_party_address_ssi, Some(calling_issi as u64));

    test.submit_message(build_u_tx_ceased_msg(calling_issi, call_id));
    test.run_stack(Some(1));
    let ceased_msgs = test.dump_sinks();

    let (mut grant_sdu, grant_alloc) =
        find_lcmc_req(&ceased_msgs, called_issi, CmcePduTypeDl::DTxGranted).expect("Expected granted D-TX GRANTED");
    let grant = DTxGranted::from_bitbuf(&mut grant_sdu).expect("Failed to parse granted DTxGranted");
    assert_eq!(grant.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(grant_alloc, Some(UlDlAssignment::Ul));
    assert_eq!(grant.transmitting_party_address_ssi, Some(called_issi as u64));

    let (mut listener_sdu, listener_alloc) =
        find_lcmc_req(&ceased_msgs, calling_issi, CmcePduTypeDl::DTxGranted).expect("Expected listener D-TX GRANTED");
    let listener = DTxGranted::from_bitbuf(&mut listener_sdu).expect("Failed to parse listener DTxGranted");
    assert_eq!(listener.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    assert_eq!(listener_alloc, Some(UlDlAssignment::Dl));
    assert_eq!(listener.transmitting_party_address_ssi, Some(called_issi as u64));

    assert!(ceased_msgs.iter().any(|msg| matches!(
        &msg.msg,
        SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, dest_gssi, .. })
            if *source_issi == called_issi && *dest_gssi == calling_issi
    )));
}

#[test]
fn test_simplex_individual_current_speaker_tx_demand_is_granted() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (mut test, call_id, _) = connected_simplex_individual_call(calling_issi, called_issi);

    test.submit_message(build_u_tx_demand_msg(calling_issi, call_id));
    test.run_stack(Some(1));
    let demand_msgs = test.dump_sinks();

    let (mut grant_sdu, grant_alloc) =
        find_lcmc_req(&demand_msgs, calling_issi, CmcePduTypeDl::DTxGranted).expect("Expected granted D-TX GRANTED");
    let grant = DTxGranted::from_bitbuf(&mut grant_sdu).expect("Failed to parse granted DTxGranted");
    assert_eq!(grant.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(grant_alloc, Some(UlDlAssignment::Ul));
    assert_eq!(grant.transmitting_party_address_ssi, Some(calling_issi as u64));

    assert!(
        find_lcmc_req(&demand_msgs, called_issi, CmcePduTypeDl::DTxGranted).is_none(),
        "Current-speaker demand should not re-announce a listener grant"
    );
    assert!(
        !demand_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. }))),
        "Current-speaker demand should not emit a duplicate floor grant"
    );
}

#[test]
fn test_duplex_individual_ul_inactivity_releases_circuit_call() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    let (mut test, call_id, connect_msgs) = connected_duplex_individual_call(calling_issi, called_issi);

    let mut open_slots: Vec<(u16, u8)> = connect_msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some((circuit.carrier_num, circuit.ts)),
            _ => None,
        })
        .collect();
    open_slots.sort_unstable();
    let (failed_carrier, failed_ts) = open_slots
        .first()
        .copied()
        .expect("Expected duplex connect to open at least one traffic circuit");

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Umac,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout {
            carrier_num: failed_carrier,
            ts: failed_ts,
        }),
    });
    test.run_stack(Some(1));
    let timeout_msgs = test.dump_sinks();

    let (mut calling_release_sdu, _) =
        find_lcmc_req(&timeout_msgs, calling_issi, CmcePduTypeDl::DRelease).expect("Expected D-RELEASE to calling ISSI");
    let calling_release = DRelease::from_bitbuf(&mut calling_release_sdu).expect("Failed to parse calling DRelease");
    assert_eq!(calling_release.call_identifier, call_id);
    assert_eq!(calling_release.disconnect_cause, DisconnectCause::ExpiryOfTimer);

    let (mut called_release_sdu, _) =
        find_lcmc_req(&timeout_msgs, called_issi, CmcePduTypeDl::DRelease).expect("Expected D-RELEASE to called ISSI");
    let called_release = DRelease::from_bitbuf(&mut called_release_sdu).expect("Failed to parse called DRelease");
    assert_eq!(called_release.call_identifier, call_id);
    assert_eq!(called_release.disconnect_cause, DisconnectCause::ExpiryOfTimer);
}

#[test]
fn test_dual_carrier_supports_two_simultaneous_duplex_individual_calls() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);
    test.config.state_write().subscribers.register(9012002);
    test.config.state_write().subscribers.register(9012004);

    test.submit_message(build_individual_u_setup_msg_with_mode(9012001, 9012002, true));
    test.run_stack(Some(1));
    let first_setup_msgs = test.dump_sinks();
    let first_call_id = first_d_setup_call_id(&first_setup_msgs, 9012002);

    test.submit_message(build_u_connect_msg(9012002, first_call_id, true));
    test.run_stack(Some(1));
    let first_connect_msgs = test.dump_sinks();

    test.submit_message(build_individual_u_setup_msg_with_mode(9012003, 9012004, true));
    test.run_stack(Some(1));
    let second_setup_msgs = test.dump_sinks();
    let second_call_id = first_d_setup_call_id(&second_setup_msgs, 9012004);

    test.submit_message(build_u_connect_msg(9012004, second_call_id, true));
    test.run_stack(Some(1));
    let second_connect_msgs = test.dump_sinks();

    let opened: Vec<(u16, u8)> = first_connect_msgs
        .iter()
        .chain(second_connect_msgs.iter())
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some((circuit.carrier_num, circuit.ts)),
            _ => None,
        })
        .collect();

    assert_eq!(opened.len(), 4, "two duplex calls should open four traffic circuits");
    assert!(
        opened.iter().any(|(carrier_num, _)| *carrier_num == SECONDARY_CARRIER),
        "the second duplex call should spill onto the secondary carrier when the primary runs out of slots"
    );
    assert!(
        first_connect_msgs.iter().chain(second_connect_msgs.iter()).all(
            |msg| !matches!(&msg.msg, SapMsgInner::LcmcMleUnitdataReq(prim) if dl_pdu_type(&prim.sdu) == Some(CmcePduTypeDl::DRelease))
        ),
        "establishing two duplex calls on a dual-carrier cell must not reject either call"
    );
    assert_eq!(
        test.config.state_read().timeslot_alloc.free_slot_count(),
        3,
        "two simultaneous duplex calls should consume four of the seven available traffic slots"
    );
}

/// Test that late-entry D-SETUP re-sends are throttled when the previous
/// D-SETUP's TxReceipt is still in Pending state (UMAC hasn't transmitted it yet),
/// and that they resume once the receipt reaches a final state.
#[test]
fn test_dsetup_late_entry_throttle() {
    debug::setup_logging_verbose();

    // Start at timeslot 1 so circuit creation aligns cleanly with tick_start checks
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    // Send U-SETUP to start a group call
    let u_setup_msg = build_u_setup_msg(TEST_ISSI, TEST_GSSI);
    test.submit_message(u_setup_msg);
    test.run_stack(Some(1));

    // Collect initial output â€” should contain D-SETUP (initial send with no tracked receipt)
    let initial_msgs = test.dump_sinks();
    let initial_setups = count_d_setups(&initial_msgs);
    assert!(initial_setups > 0, "Expected initial D-SETUP after U-SETUP");

    // Run a few more ticks to get through the D_SETUP_REPEATS backup window.
    // The backup send goes through (receipt is None) and creates a tracked receipt.
    test.run_stack(Some(8));
    let mut backup_msgs = test.dump_sinks();
    let backup_reporters = extract_d_setup_reporters(&mut backup_msgs);

    // We should have at least one reporter from the backup send
    assert!(
        !backup_reporters.is_empty(),
        "Expected backup D-SETUP with tx_reporter in initial window"
    );
    let last_reporter = &backup_reporters[backup_reporters.len() - 1];
    assert_eq!(last_reporter.get_state(), TxState::Pending);

    // Run for 2 full late-entry intervals (720 ticks). With the receipt still Pending,
    // ALL late-entry D-SETUPs should be suppressed.
    test.run_stack(Some(720));
    let throttled_msgs = test.dump_sinks();
    let throttled_count = count_d_setups(&throttled_msgs);
    assert_eq!(
        throttled_count, 0,
        "Late-entry D-SETUPs should be suppressed while receipt is Pending"
    );

    // Now mark the previous D-SETUP as transmitted (simulating UMAC sending it over the air)
    last_reporter.mark_transmitted();

    // Run for 2 more late-entry intervals. Now D-SETUPs should go through.
    test.run_stack(Some(720));
    let mut unthrottled_msgs = test.dump_sinks();
    let unthrottled_count = count_d_setups(&unthrottled_msgs);
    assert!(
        unthrottled_count > 0,
        "Late-entry D-SETUPs should resume once receipt reaches final state"
    );

    // Each re-send that went through should have created a fresh reporter
    let new_reporters = extract_d_setup_reporters(&mut unthrottled_msgs);
    assert_eq!(
        new_reporters.len(),
        unthrottled_count,
        "Each re-sent D-SETUP should carry a fresh tx_reporter"
    );
}

/// Emergency pre-emption (ETSI EN 300 392-2 clause 14.8 "Call priority"): when every traffic
/// channel is busy with ordinary-priority calls, an incoming emergency (priority 15) group call
/// pre-empts the lowest-priority active call to obtain a channel. We assert the cell first fills
/// its three traffic slots (TS2..TS4), then that the emergency set-up both tears an existing call
/// down (`CallControl::CallEnded` toward UMAC) and opens a fresh circuit for itself
/// (`CallControl::Open`), leaving the cell still fully utilised by the freed slot.
#[test]
fn test_emergency_call_preempts_when_cell_full() {
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // Three distinct talkgroups, each with a registered listener, will fill TS2..TS4.
    let gssis = [101u32, 102, 103];
    for (i, &g) in gssis.iter().enumerate() {
        register_subscriber(&mut test, 2_000_001 + i as u32, g);
    }
    // A fourth (emergency) talkgroup with its own listener â€” no free slot remains for it.
    let emergency_gssi = 199u32;
    register_subscriber(&mut test, 2_000_099, emergency_gssi);

    // Fill the three traffic channels with ordinary-priority (priority 0) group calls.
    for (i, &g) in gssis.iter().enumerate() {
        test.submit_message(build_u_setup_msg_prio(3_000_001 + i as u32, g, 0));
        test.run_stack(Some(1));
    }
    test.dump_sinks(); // discard the set-up traffic for the three ordinary calls

    assert_eq!(
        test.config.state_read().timeslot_alloc.free_count(),
        0,
        "expected the cell to be full (0 free slots) after three group calls"
    );

    // Incoming EMERGENCY group call (priority 15) on the fourth GSSI â€” must pre-empt to proceed.
    test.submit_message(build_u_setup_msg_prio(3_000_099, emergency_gssi, 15));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // A victim call is torn down (CallEnded toward UMAC) and the emergency call opens its own
    // circuit (Open). Both being present proves pre-emption happened and the emergency was admitted.
    let call_ended = msgs
        .iter()
        .filter(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. })))
        .count();
    let opened = msgs
        .iter()
        .filter(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::Open(_))))
        .count();

    assert!(
        call_ended >= 1,
        "emergency call should have pre-empted (torn down) at least one active call; saw none"
    );
    assert!(
        opened >= 1,
        "emergency call should have opened its own traffic circuit after pre-emption"
    );
    // The cell remains fully utilised: the victim freed a slot, the emergency call took it.
    assert_eq!(
        test.config.state_read().timeslot_alloc.free_count(),
        0,
        "emergency call should occupy the slot freed by pre-emption"
    );
}

/// Verify a non-pre-emptive (ordinary priority) call does NOT pre-empt when the cell is full:
/// it is rejected with a D-RELEASE instead, and all three existing calls stay up.
#[test]
fn test_ordinary_call_does_not_preempt_when_cell_full() {
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut test = ComponentTest::from_config(config, None);
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let gssis = [101u32, 102, 103];
    for (i, &g) in gssis.iter().enumerate() {
        register_subscriber(&mut test, 2_000_001 + i as u32, g);
    }
    let extra_gssi = 199u32;
    register_subscriber(&mut test, 2_000_099, extra_gssi);

    for (i, &g) in gssis.iter().enumerate() {
        test.submit_message(build_u_setup_msg_prio(3_000_001 + i as u32, g, 0));
        test.run_stack(Some(1));
    }
    test.dump_sinks();

    // Ordinary-priority (priority 0) group call into a full cell â€” must NOT pre-empt anything.
    test.submit_message(build_u_setup_msg_prio(3_000_099, extra_gssi, 0));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let call_ended = msgs
        .iter()
        .filter(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. })))
        .count();
    assert_eq!(call_ended, 0, "an ordinary-priority call must not pre-empt any active call");
    // The three original calls are untouched â€” still no free slot.
    assert_eq!(test.config.state_read().timeslot_alloc.free_count(), 0);
}

// Energy-Economy D-SETUP gate (clause 16.7): individual-call setup resends to a sleeping EE MS
// are held for the MS's downlink monitoring window, with a bounded fallback (EE_DSETUP_FALLBACK_TS
// â‰ˆ 423 timeslots / ~105 frames) to the historical blind resend. The empirically-observed resend
// cadence (initial + setup-retry) fires several individual D-SETUPs to the called MS within the
// fallback window, which the tests below rely on.

/// A sleeping EE MS (monitoring window closed for the whole sub-fallback run) must NOT receive
/// any D-SETUP resend â€” they are held for its window.
#[test]
fn test_dsetup_to_ee_ms_held_outside_monitoring_window() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let calling = 3000001;
    let called = 2000002;
    test.config.state_write().subscribers.register(called); // local registration -> local P2P (not Brew)

    // Window = frame 1, offset 30, cycle 60: open only when multiframe_index % 60 == 30. The run
    // below spans multiframe_index 0..~6, so the window is CLOSED for its entire duration.
    test.config.state_write().ee_monitoring_windows.insert(called, (1, 30, 60));

    test.submit_message(build_u_setup_p2p_msg(calling, called));
    test.run_stack(Some(1));
    test.dump_sinks(); // discard the initial (ungated) D-SETUP page

    // ~100 frames (400 ts) â€” comfortably under the ~423 ts fallback, so any resend here is held.
    test.run_stack(Some(400));
    let held = count_individual_dsetup_to(&test.dump_sinks(), called);
    assert_eq!(
        held, 0,
        "D-SETUP resends to an asleep EE MS must be held while its monitoring window is closed"
    );
}

/// A non-EE MS (absent from the published window map) is always reachable â€” the gate must not
/// suppress its D-SETUP resends.
#[test]
fn test_dsetup_to_non_ee_ms_resends_normally() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let calling = 3000001;
    let called = 2000002;
    test.config.state_write().subscribers.register(called);
    // No ee_monitoring_windows entry for `called` -> not in EE -> always reachable.

    test.submit_message(build_u_setup_p2p_msg(calling, called));
    test.run_stack(Some(1));
    test.dump_sinks(); // discard initial page

    test.run_stack(Some(400));
    let resends = count_individual_dsetup_to(&test.dump_sinks(), called);
    assert!(
        resends >= 1,
        "D-SETUP resends to a non-EE MS must continue normally (gate inactive), got {resends}"
    );
}

/// Bounded-fallback safety net: even if the granted window phase is wrong (window never opens),
/// resends must resume once the setup has been pending longer than the fallback â€” so call setup
/// is never worse than the historical blind resend.
#[test]
fn test_dsetup_ee_fallback_resends_after_timeout() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let calling = 3000001;
    let called = 2000002;
    test.config.state_write().subscribers.register(called);

    // Window that never opens during the run (closed throughout) -> only the fallback can release.
    test.config.state_write().ee_monitoring_windows.insert(called, (1, 30, 60));

    test.submit_message(build_u_setup_p2p_msg(calling, called));
    test.run_stack(Some(1));
    test.dump_sinks(); // discard initial page

    // Run well past the ~423 ts fallback (600 ts). Pre-fallback resends are held; once the fallback
    // expires, resends resume on the MCCH despite the still-closed window.
    test.run_stack(Some(600));
    let resends = count_individual_dsetup_to(&test.dump_sinks(), called);
    assert!(
        resends >= 1,
        "after the EE fallback expires, D-SETUP resends must resume (never worse than before), got {resends}"
    );
}

/// Energy-economy group-call announce batching: a group with a member that is asleep (EE) at
/// announce time must receive EXTRA group D-SETUP re-sends (covering the member's later wake
/// frame) compared to an identical all-StayAlive group. Both runs see the same late-entry
/// cadence, so the difference isolates the batching contribution.
#[test]
fn test_group_ee_announce_adds_resends_for_sleeping_member() {
    debug::setup_logging_verbose();

    fn run_group_call(with_ee_member: bool) -> usize {
        let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
        let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );
        let caller = 1000001;
        let member = 1000003;
        register_subscriber(&mut test, caller, TEST_GSSI);
        register_subscriber(&mut test, member, TEST_GSSI);
        if with_ee_member {
            // Awake only on frames where (f-1) % 6 == 3 (f = 4/10/16): asleep at the setup frame
            // (f=1), wakes within the first EE cycle. (frame, multiframe, cycle_len).
            test.config.state_write().ee_monitoring_windows.insert(member, (4, 1, 6));
        }
        test.submit_message(build_u_setup_msg(caller, TEST_GSSI));
        test.run_stack(Some(1));
        test.dump_sinks(); // discard the initial group D-SETUP
        // Span several EE cycles, but stay well under the ~5 s (360 ts) late-entry interval so the
        // only source of additional group D-SETUPs is the EE announce batching.
        test.run_stack(Some(40));
        count_d_setups(&test.dump_sinks())
    }

    let with_ee = run_group_call(true);
    let stayalive_only = run_group_call(false);
    assert!(
        with_ee > stayalive_only,
        "a group with a sleeping EE member must get extra batched group D-SETUPs ({with_ee}) vs an all-StayAlive group ({stayalive_only})"
    );
}

/// The transmitting speaker is awake by definition, so its own EE window must NOT drive announce
/// batching. A group whose only non-speaker member is StayAlive must produce zero batched group
/// D-SETUPs even when the EE-subscriber caller's window opens mid-run (regression for the
/// speaker-counted-as-uncovered bug).
#[test]
fn test_group_ee_announce_excludes_speaker() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    let caller = 1000001;
    let stayalive_listener = 1000002;
    register_subscriber(&mut test, caller, TEST_GSSI);
    register_subscriber(&mut test, stayalive_listener, TEST_GSSI);
    // Caller is itself an EE subscriber, asleep at setup but waking within the run (f = 4/10/16).
    // With the fix it is excluded from coverage (it is the speaker); the only non-speaker member is
    // StayAlive, so no batched re-send should ever fire.
    test.config.state_write().ee_monitoring_windows.insert(caller, (4, 1, 6));

    test.submit_message(build_u_setup_msg(caller, TEST_GSSI));
    test.run_stack(Some(1));
    test.dump_sinks(); // discard initial group D-SETUP
    test.run_stack(Some(40));
    let batched = count_d_setups(&test.dump_sinks());
    assert_eq!(
        batched, 0,
        "the speaker's own EE window must not trigger batched re-sends (got {batched})"
    );
}

// â”€â”€â”€ FH FIX 2 guard: group-addressed PDUs must use unacknowledged LLC â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// D-SETUP / D-RELEASE to a GSSI have no single peer to acknowledge, so they must go out as
// unacknowledged BL-UDATA (`Layer2Service::Unacknowledged`), not the acknowledged default. MLE
// routes Unacknowledged â†’ TlaTlUnitdataReqBl (BL-UDATA) and everything else â†’ TlaTlDataReqBl
// (acknowledged BL-DATA), so the LcmcMleUnitdataReq.layer2service is the load-bearing field.

/// Find the LcmcMleUnitdataReq carrying `pdu_type` addressed to `address` and return its
/// `layer2service`. `None` if no such PDU was emitted.
fn lcmc_req_layer2service(msgs: &[SapMsg], address: u32, pdu_type: CmcePduTypeDl) -> Option<tetra_core::Layer2Service> {
    msgs.iter().find_map(|msg| {
        if msg.dest != TetraEntity::Mle {
            return None;
        }
        let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
            return None;
        };
        if prim.main_address.ssi != address || dl_pdu_type(&prim.sdu) != Some(pdu_type) {
            return None;
        }
        Some(prim.layer2service)
    })
}

#[test]
fn test_group_d_setup_uses_unacknowledged_llc() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();

    let l2 = lcmc_req_layer2service(&setup_msgs, TEST_GSSI, CmcePduTypeDl::DSetup).expect("Expected a group D-SETUP addressed to the GSSI");
    assert_eq!(
        l2,
        tetra_core::Layer2Service::Unacknowledged,
        "group D-SETUP to a GSSI must use unacknowledged BL-UDATA (no single peer to ACK), got {l2:?}"
    );
}

#[test]
fn test_group_d_release_uses_unacknowledged_llc() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, TEST_GSSI);

    // The call owner (the original caller) disconnects â†’ release_group_call sends the group
    // D-RELEASE addressed to the GSSI.
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();

    let l2 = lcmc_req_layer2service(&release_msgs, TEST_GSSI, CmcePduTypeDl::DRelease)
        .expect("Expected a group D-RELEASE addressed to the GSSI");
    assert_eq!(
        l2,
        tetra_core::Layer2Service::Unacknowledged,
        "group D-RELEASE to a GSSI must use unacknowledged BL-UDATA, got {l2:?}"
    );
}

// â”€â”€â”€ FH FIX 1 guard: call-lifecycle telemetry must reach the dashboard sink â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// Regression: a local group-call U-SETUP must notify UMAC of the floor grant (a
/// `CallControl::FloorGranted` addressed to `TetraEntity::Umac`), so UMAC arms the originator's
/// UL-inactivity ("stuck transmitter") timer at set-up. This grant previously passed
/// `notify_umac = false`, so a caller who set up a group call and then dropped BEFORE transmitting a
/// single UL frame was never timed out and the traffic slot leaked — permanently with
/// `call_timeout_secs = 0`.
#[test]
fn test_group_setup_notifies_umac_floor_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(
        msgs.iter().any(|msg| msg.dest == TetraEntity::Umac
            && matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, dest_gssi, .. })
                    if *source_issi == TEST_ISSI && *dest_gssi == TEST_GSSI
            )),
        "local group setup must send FloorGranted to UMAC so the originator's UL-inactivity timer is armed"
    );
}

#[test]
fn test_group_call_emits_started_and_ended_telemetry() {
    use tetra_entities::cmce::cmce_bs::CmceBs;
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};

    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    // Build the sinks but NOT the CMCE â€” we register a telemetry-wired CmceBs ourselves below.
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);

    let (sink, source) = telemetry_channel();
    let cmce = CmceBs::new(test.config.clone(), Some(sink), None);
    test.register_entity(cmce);

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    // Drain any telemetry produced by registration (there should be none for call lifecycle).
    while source.try_recv().is_some() {}

    // Start a group call.
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, TEST_GSSI);

    let mut started = None;
    while let Some(ev) = source.try_recv() {
        if let TelemetryEvent::GroupCallStarted {
            call_id: ev_call_id,
            gssi,
            caller_issi,
            ..
        } = ev
        {
            started = Some((ev_call_id, gssi, caller_issi));
        }
    }
    let (ev_call_id, gssi, caller_issi) = started.expect("Expected a GroupCallStarted telemetry event on call setup");
    assert_eq!(ev_call_id, call_id, "GroupCallStarted call_id mismatch");
    assert_eq!(gssi, TEST_GSSI, "GroupCallStarted gssi mismatch");
    assert_eq!(caller_issi, TEST_ISSI, "GroupCallStarted caller_issi mismatch");

    // End the group call (owner disconnects).
    test.submit_message(build_u_disconnect_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    test.dump_sinks();

    let mut ended = false;
    while let Some(ev) = source.try_recv() {
        if let TelemetryEvent::GroupCallEnded { call_id: ev_call_id, .. } = ev {
            if ev_call_id == call_id {
                ended = true;
            }
        }
    }
    assert!(ended, "Expected a GroupCallEnded telemetry event on call release");
}

/// Regression: a U-FACILITY (supplementary service) request from an MS must be answered
/// with D-CMCE-FUNCTION-NOT-SUPPORTED (ETSI EN 300 392-2 Â§14.7.2.5). The CMCE rewrite
/// routed U-FACILITY to ss_bs::route_re_deliver which only logged and sent nothing, so the
/// MS received no response to its SS request. The BS must reply, not stay silent (and must
/// not crash).
#[test]
fn test_u_facility_answered_with_function_not_supported() {
    use tetra_pdus::cmce::enums::cmce_pdu_type_ul::CmcePduTypeUl;
    use tetra_pdus::cmce::pdus::cmce_function_not_supported::CmceFunctionNotSupported;
    use tetra_pdus::cmce::pdus::u_facility::UFacility;

    let calling_issi = TEST_ISSI;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Build and submit a U-FACILITY uplink PDU.
    let mut sdu = BitBuffer::new_autoexpand(16);
    UFacility { facility: None }
        .to_bitbuf(&mut sdu)
        .expect("Failed to serialize UFacility");
    sdu.seek(0);
    test.submit_message(lcmc_ind(calling_issi, sdu));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // Expect a D-CMCE-FUNCTION-NOT-SUPPORTED back to the requesting MS.
    let (mut resp_sdu, _) = find_lcmc_req(&msgs, calling_issi, CmcePduTypeDl::CmceFunctionNotSupported)
        .expect("Expected D-CMCE-FUNCTION-NOT-SUPPORTED to requesting ISSI");

    let pdu = CmceFunctionNotSupported::from_bitbuf(&mut resp_sdu).expect("Failed to parse D-CMCE-FUNCTION-NOT-SUPPORTED");
    assert_eq!(
        pdu.not_supported_pdu_type,
        CmcePduTypeUl::UFacility.into_raw() as u8,
        "not_supported_pdu_type should identify U-FACILITY"
    );
    assert_eq!(
        pdu.function_not_supported_pointer, 0,
        "pointer 0 = the whole PDU type is not supported"
    );
}

// â”€â”€ SS-DGNA over CMCE D-FACILITY (TS 100 392-12-22 V1.5.1; EN 300 392-9 V1.7.1) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
//
// These exercise the full operator-DGNA path: a dashboard `ControlCommand::Dgna` reaches MM, which
// (SS-DGNA default) affiliates the GSSI and hands the air emission to CMCE, which puts an SS-DGNA
// ASSIGN/DEASSIGN on the wire inside a D-FACILITY. Plus the uplink U-FACILITY ASSIGN ACK handling.
mod ss_dgna_tests {
    use super::*;
    use tetra_config::bluestation::StackMode;
    use tetra_core::BitBuffer;
    use tetra_entities::cmce::cmce_bs::CmceBs;
    use tetra_entities::mm::mm_bs::MmBs;
    use tetra_entities::net_control::{ControlCommand, make_control_link};
    use tetra_pdus::cmce::pdus::d_facility::DFacility;
    use tetra_pdus::cmce::pdus::u_facility::{UFacility, UFacilitySsBody};
    use tetra_pdus::cmce::ss_dgna::enums::results::{GroupIdentityAttachmentMode, ResultOfAssignment, ResultOfAttachment};
    use tetra_pdus::cmce::ss_dgna::fields::group_assignment_ack::GroupAssignmentAck;
    use tetra_pdus::cmce::ss_dgna::pdus::assign_ack::AssignAck;
    use tetra_pdus::cmce::ss_dgna::ss_dgna_pdu::SsDgnaPdu;
    use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
    use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
    use tetra_saps::lmm::LmmMleUnitdataInd;

    const DGNA_ISSI: u32 = 2260601;
    const DGNA_GSSI: u32 = 4242;

    /// Register a terminal in MM by feeding a minimal U-LOCATION-UPDATE-DEMAND, so it is "known"
    /// and eligible for DGNA. (Self-contained copy of the MM-test helper; the two test binaries
    /// don't share helpers.)
    fn register_terminal_mm(test: &mut ComponentTest, issi: u32) {
        let demand = ULocationUpdateDemand {
            location_update_type: LocationUpdateType::RoamingLocationUpdating,
            request_to_append_la: false,
            cipher_control: false,
            ciphering_parameters: None,
            class_of_ms: None,
            energy_saving_mode: None,
            la_information: None,
            ssi: Some(issi as u64),
            address_extension: None,
            group_identity_location_demand: None,
            group_report_response: None,
            authentication_uplink: None,
            extended_capabilities: None,
            proprietary: None,
        };
        let mut sdu = BitBuffer::new_autoexpand(32);
        demand.to_bitbuf(&mut sdu).expect("serialize U-LOCATION-UPDATE-DEMAND");
        sdu.seek(0);
        test.submit_message(SapMsg {
            sap: Sap::LmmSap,
            src: TetraEntity::Mle,
            dest: TetraEntity::Mm,
            msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
                sdu,
                handle: 0,
                received_address: TetraAddress::new(issi, SsiType::Issi),
            }),
        });
        test.run_stack(Some(2));
    }

    /// Pull the first D-FACILITY (and its addressed ISSI) out of a batch of captured MLE messages.
    fn find_d_facility(msgs: &[SapMsg]) -> Option<(u32, DFacility)> {
        for m in msgs {
            if let SapMsgInner::LcmcMleUnitdataReq(ref req) = m.msg {
                if dl_pdu_type(&req.sdu) != Some(CmcePduTypeDl::DFacility) {
                    continue;
                }
                let mut sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
                if let Ok(pdu) = DFacility::from_bitbuf(&mut sdu) {
                    return Some((req.main_address.ssi, pdu));
                }
            }
        }
        None
    }

    fn find_d_facility_req(msgs: &[SapMsg]) -> Option<&tetra_saps::lcmc::LcmcMleUnitdataReq> {
        msgs.iter().find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(req) if dl_pdu_type(&req.sdu) == Some(CmcePduTypeDl::DFacility) => Some(req),
            _ => None,
        })
    }

    /// Build a MM(+control)/CMCE stack with the Mle sink, register the terminal, drive a DGNA, and
    /// return everything captured at the sinks.
    fn run_dgna(attach: bool, mnemonic: Option<&str>) -> (ComponentTest, Vec<SapMsg>) {
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![], vec![TetraEntity::Mle]);

        // CMCE wired exactly like the binary wires the dashboard control link.
        let (dispatcher, endpoint) = make_control_link();
        let cmce = CmceBs::new(test.get_shared_config(), None, Some(endpoint));
        test.register_entity(cmce);
        // MM owns the group registry; it receives the forwarded MmDgnaRequest over the SAP.
        let mm = MmBs::new(test.get_shared_config(), None, None);
        test.register_entity(mm);

        register_terminal_mm(&mut test, DGNA_ISSI);
        let _ = test.dump_sinks();

        if !attach {
            // Assign first so there is something to deassign.
            dispatcher.send(ControlCommand::Dgna {
                issi: DGNA_ISSI,
                gssi: DGNA_GSSI,
                mnemonic: None,
                attachment_mode: 0,
                attach: true,
            });
            test.run_stack(Some(6));
            let _ = test.dump_sinks();
        }

        dispatcher.send(ControlCommand::Dgna {
            issi: DGNA_ISSI,
            gssi: DGNA_GSSI,
            mnemonic: mnemonic.map(str::to_string),
            attachment_mode: 0,
            attach,
        });
        // CMCE drains control -> MmDgnaRequest -> MM affiliates + CmceSsDgnaAssign -> CMCE D-FACILITY.
        test.run_stack(Some(6));
        let msgs = test.dump_sinks();
        (test, msgs)
    }

    /// Operator DGNA assign on the SS-DGNA default emits exactly one D-FACILITY{ASSIGN} addressed to
    /// the target ISSI, with the GSSI and attachment mode 000, and the registry affiliates.
    #[test]
    fn test_dgna_assign_emits_d_facility() {
        debug::setup_logging_verbose();
        let (test, msgs) = run_dgna(true, None);

        let (addr_ssi, facility) =
            find_d_facility(&msgs).unwrap_or_else(|| panic!("expected a D-FACILITY after DGNA assign, got {} msgs", msgs.len()));
        assert_eq!(addr_ssi, DGNA_ISSI, "D-FACILITY must be addressed to the target ISSI");

        let body = facility.facility.expect("D-FACILITY must carry an SS-DGNA body");
        let SsDgnaPdu::Assign(assign) = body.ss_pdu else {
            panic!("expected an ASSIGN, got {}", body.ss_pdu);
        };
        assert_eq!(assign.groups.len(), 1, "exactly one group assigned");
        assert_eq!(assign.groups[0].group_ssi, DGNA_GSSI);
        assert_eq!(
            assign.groups[0].attachment_mode,
            GroupIdentityAttachmentMode::AttachedPermanently,
            "attachment mode 000 (attached permanently)"
        );
        assert!(assign.ack_requested, "ASSIGN must request an ACK");

        assert!(
            test.config
                .state_read()
                .subscribers
                .attached_groups_of(DGNA_ISSI)
                .contains(&DGNA_GSSI),
            "DGNA assign must affiliate the GSSI in the subscriber registry"
        );
    }

    /// Operator DGNA assign with a mnemonic carries the TG name in the SS-DGNA Group assignment IE.
    #[test]
    fn test_dgna_assign_emits_mnemonic_name() {
        debug::setup_logging_verbose();
        let (_test, msgs) = run_dgna(true, Some("OPS 42"));

        let (_addr_ssi, facility) =
            find_d_facility(&msgs).unwrap_or_else(|| panic!("expected a D-FACILITY after DGNA assign, got {} msgs", msgs.len()));
        let body = facility.facility.expect("D-FACILITY must carry an SS-DGNA body");
        let SsDgnaPdu::Assign(assign) = body.ss_pdu else {
            panic!("expected an ASSIGN, got {}", body.ss_pdu);
        };
        assert_eq!(assign.groups.len(), 1, "exactly one group assigned");
        assert_eq!(assign.groups[0].mnemonic.as_deref(), Some("OPS 42"));
    }

    #[test]
    fn test_dgna_assign_to_idle_ms_uses_mcch_acknowledged_llc() {
        debug::setup_logging_verbose();
        let (_test, msgs) = run_dgna(true, None);

        let req = find_d_facility_req(&msgs).expect("expected D-FACILITY request");
        assert_eq!(req.main_address.ssi, DGNA_ISSI);
        assert_eq!(req.layer2service, tetra_core::Layer2Service::Acknowledged);
        assert!(!req.stealing_permission, "idle DGNA should stay on MCCH");
        assert!(req.chan_alloc.is_none(), "idle DGNA should not carry chan_alloc");
    }

    /// Put DGNA_ISSI into a live group call (so it is parked on a real traffic channel), then
    /// drive a DGNA to it. The "in call" condition must be a genuine call: `active_call_ts` is a
    /// projection CMCE rebuilds from the live call tables on every tick (`publish_active_call_ts`),
    /// so poking it directly does not survive into the DGNA delivery decision. Returns the stack
    /// and the messages captured during the DGNA phase only.
    fn run_dgna_to_ms_in_group_call() -> (ComponentTest, Vec<SapMsg>) {
        const IN_CALL_GSSI: u32 = 7777;
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);

        let (dispatcher, endpoint) = make_control_link();
        let cmce = CmceBs::new(test.get_shared_config(), None, Some(endpoint));
        test.register_entity(cmce);
        let mm = MmBs::new(test.get_shared_config(), None, None);
        test.register_entity(mm);

        register_terminal_mm(&mut test, DGNA_ISSI);
        // CC needs the subscriber + its affiliation before it will set up the group call.
        register_subscriber(&mut test, DGNA_ISSI, IN_CALL_GSSI);

        // DGNA_ISSI opens a group call, becoming the speaker on an allocated traffic channel, so the
        // live-call projection lists it. This is exactly what the in-call DGNA path keys on.
        test.submit_message(build_u_setup_msg(DGNA_ISSI, IN_CALL_GSSI));
        test.run_stack(Some(2));
        let _ = test.dump_sinks();

        dispatcher.send(ControlCommand::Dgna {
            issi: DGNA_ISSI,
            gssi: DGNA_GSSI,
            mnemonic: None,
            attachment_mode: 0,
            attach: true,
        });
        test.run_stack(Some(6));
        let msgs = test.dump_sinks();
        (test, msgs)
    }

    #[test]
    fn test_dgna_assign_to_in_call_ms_uses_facch_stealing() {
        debug::setup_logging_verbose();
        let (test, msgs) = run_dgna_to_ms_in_group_call();

        // The traffic channel the call was actually allocated — the DGNA must steal onto exactly it.
        let (carrier, ts, usage) = test
            .config
            .state_read()
            .active_call_ts
            .get(&DGNA_ISSI)
            .copied()
            .expect("DGNA_ISSI must be parked on a traffic channel while in a group call");

        let req = find_d_facility_req(&msgs).expect("expected D-FACILITY request");
        assert_eq!(req.main_address.ssi, DGNA_ISSI);
        assert_eq!(
            req.layer2service,
            tetra_core::Layer2Service::Unacknowledged,
            "in-call DGNA should use FACCH/STCH path"
        );
        assert!(req.stealing_permission, "in-call DGNA should request stealing");
        let chan_alloc = req.chan_alloc.as_ref().expect("in-call DGNA should carry chan_alloc");
        assert_eq!(chan_alloc.carrier, Some(carrier), "DGNA must steal onto the call's carrier");
        assert_eq!(chan_alloc.usage, Some(usage), "DGNA must steal onto the call's usage marker");
        assert_eq!(chan_alloc.ul_dl_assigned, UlDlAssignment::Dl);
        let mut expected_ts = [false; 4];
        expected_ts[(ts - 1) as usize] = true;
        assert_eq!(chan_alloc.timeslots, expected_ts, "DGNA must steal onto the call's timeslot");
    }

    /// Operator DGNA deassign emits a D-FACILITY{DEASSIGN} naming the GSSI and removes the
    /// affiliation.
    #[test]
    fn test_dgna_deassign_emits_d_facility_deassign() {
        debug::setup_logging_verbose();
        let (test, msgs) = run_dgna(false, None);

        let (addr_ssi, facility) =
            find_d_facility(&msgs).unwrap_or_else(|| panic!("expected a D-FACILITY after DGNA deassign, got {} msgs", msgs.len()));
        assert_eq!(addr_ssi, DGNA_ISSI);

        let body = facility.facility.expect("D-FACILITY must carry an SS-DGNA body");
        let SsDgnaPdu::Deassign(deassign) = body.ss_pdu else {
            panic!("expected a DEASSIGN, got {}", body.ss_pdu);
        };
        assert_eq!(deassign.groups.len(), 1);
        assert_eq!(deassign.groups[0].group_ssi, DGNA_GSSI);

        assert!(
            !test
                .config
                .state_read()
                .subscribers
                .attached_groups_of(DGNA_ISSI)
                .contains(&DGNA_GSSI),
            "DGNA deassign must remove the GSSI from the subscriber registry"
        );
    }

    /// DGNA to a terminal MM does not know is refused before any air emission: no D-FACILITY.
    #[test]
    fn test_dgna_to_unregistered_issi_emits_no_d_facility() {
        debug::setup_logging_verbose();
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(vec![], vec![TetraEntity::Mle]);

        let (dispatcher, endpoint) = make_control_link();
        let cmce = CmceBs::new(test.get_shared_config(), None, Some(endpoint));
        test.register_entity(cmce);
        let mm = MmBs::new(test.get_shared_config(), None, None);
        test.register_entity(mm);

        // No registration first â€” MM must drop the command, so CMCE never emits a D-FACILITY.
        dispatcher.send(ControlCommand::Dgna {
            issi: 9_999_002,
            gssi: DGNA_GSSI,
            mnemonic: None,
            attachment_mode: 0,
            attach: true,
        });
        test.run_stack(Some(6));
        let msgs = test.dump_sinks();

        assert!(
            find_d_facility(&msgs).is_none(),
            "DGNA to an unregistered ISSI must not emit a D-FACILITY"
        );
    }

    /// An uplink U-FACILITY carrying an SS-DGNA ASSIGN ACK is recognised and consumed (it confirms a
    /// regroup whose BS state is already committed). It must NOT trigger D-CMCE-FUNCTION-NOT-SUPPORTED.
    #[test]
    fn test_u_facility_assign_ack_handled() {
        debug::setup_logging_verbose();

        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
        test.populate_entities(
            vec![TetraEntity::Cmce],
            vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
        );

        // Build a U-FACILITY{ASSIGN ACK} as the affected MS would send back.
        let ack = AssignAck {
            acks: vec![GroupAssignmentAck {
                group_ssi: DGNA_GSSI,
                group_extension: None,
                result_of_assignment: ResultOfAssignment::Accepted,
                result_of_attachment: ResultOfAttachment::Attached,
            }],
        };
        let mut sdu = BitBuffer::new_autoexpand(32);
        UFacility {
            facility: Some(UFacilitySsBody {
                routeing: 0,
                ss_pdu: SsDgnaPdu::AssignAck(ack),
            }),
        }
        .to_bitbuf(&mut sdu)
        .expect("serialize U-FACILITY ASSIGN ACK");
        sdu.seek(0);

        test.submit_message(lcmc_ind(DGNA_ISSI, sdu));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();

        assert!(
            find_lcmc_req(&msgs, DGNA_ISSI, CmcePduTypeDl::CmceFunctionNotSupported).is_none(),
            "an SS-DGNA ASSIGN ACK must be consumed, not answered with D-CMCE-FUNCTION-NOT-SUPPORTED"
        );
    }
}

/// Regression: a group U-CALL RESTORE that asks for the floor flips the call back to Transmitting,
/// so it must also tell UMAC (FloorGranted) — that is what arms the UL-inactivity watchdog. Without
/// it a restored talker that goes silent pins the traffic timeslot until the absolute call time-out
/// (never, for a duplex/Infinite call), and repeats exhaust every traffic channel on the cell.
#[test]
fn test_group_call_restore_grant_notifies_umac_floor_grant() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, TEST_GSSI);

    // The talking MS restores the call after a break and asks to keep transmitting.
    test.submit_message(build_u_call_restore_msg(TEST_ISSI, call_id, TEST_GSSI, true));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (mut sdu, chan) =
        find_lcmc_req(&msgs, TEST_ISSI, CmcePduTypeDl::DCallRestore).expect("Expected D-CALL RESTORE to the restoring MS");
    let d_call_restore = DCallRestore::from_bitbuf(&mut sdu).expect("Failed to parse DCallRestore");
    assert_eq!(d_call_restore.call_identifier, call_id);
    assert_eq!(d_call_restore.new_call_identifier, None, "same call identifier, no new one");
    assert!(chan.is_some(), "U-plane goes on: a channel allocation must accompany it (clause 14.5.2.2.4)");
    assert_eq!(
        d_call_restore.transmission_grant,
        TransmissionGrant::Granted.into_raw() as u8,
        "the restoring speaker should get the floor back"
    );

    assert!(
        msgs.iter().any(|msg| msg.dest == TetraEntity::Umac
            && matches!(
                &msg.msg,
                SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, dest_gssi, .. })
                    if *source_issi == TEST_ISSI && *dest_gssi == TEST_GSSI
            )),
        "a group call restore that grants the floor must send FloorGranted to UMAC, else the UL-inactivity timer never arms"
    );
}

/// Regression: an individual simplex U-CALL RESTORE must not hand the floor to a party while the
/// peer still holds it — two simultaneous transmitters and a stale floor holder the UL-inactivity
/// watchdog would never cease.
#[test]
fn test_individual_restore_does_not_steal_floor_from_peer() {
    debug::setup_logging_verbose();

    let calling_issi = 1000001;
    let called_issi = 1000002;
    // After connect the calling MS holds the floor (see the initial-floor test above).
    let (mut test, call_id, _msgs) = connected_simplex_individual_call(calling_issi, called_issi);

    // The other party restores and asks to transmit while the caller is still talking.
    test.submit_message(build_u_call_restore_msg(called_issi, call_id, calling_issi, true));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (mut sdu, _) =
        find_lcmc_req(&msgs, called_issi, CmcePduTypeDl::DCallRestore).expect("Expected D-CALL RESTORE to the restoring MS");
    let d_call_restore = DCallRestore::from_bitbuf(&mut sdu).expect("Failed to parse DCallRestore");
    assert_eq!(d_call_restore.call_identifier, call_id);
    assert_eq!(
        d_call_restore.transmission_grant,
        TransmissionGrant::GrantedToOtherUser.into_raw() as u8,
        "restoring while the peer holds the floor must not grant transmission"
    );
}

/// Regression: a second U-SETUP to a GSSI that already has an active call is LATE ENTRY
/// (ETSI EN 300 392-2 clause 14, one call per group), not a second call. Allocating a parallel
/// call burns another scarce traffic timeslot and orphans one of the two GSSI-keyed circuits.
#[test]
fn test_second_group_setup_to_active_gssi_is_late_entry() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    let joiner_issi = TEST_ISSI + 1;
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    register_subscriber(&mut test, joiner_issi, TEST_GSSI);

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup_msgs, TEST_GSSI);
    let free_slots_after_first = test.config.state_read().timeslot_alloc.free_count();

    // Second U-SETUP from another member of the same, already-active group.
    test.submit_message(build_u_setup_msg(joiner_issi, TEST_GSSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert_eq!(
        test.config.state_read().timeslot_alloc.free_count(),
        free_slots_after_first,
        "late entry must not allocate a second traffic timeslot for the same GSSI"
    );
    let opened = msgs
        .iter()
        .filter(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::Open(_))))
        .count();
    assert_eq!(opened, 0, "late entry must not open a second circuit");

    // The joiner is attached to the ongoing call: same call_id, and it does not own the call.
    let (mut sdu, _) = find_lcmc_req(&msgs, joiner_issi, CmcePduTypeDl::DConnect).expect("Expected D-CONNECT to the late joiner");
    let d_connect = DConnect::from_bitbuf(&mut sdu).expect("Failed to parse DConnect");
    assert_eq!(d_connect.call_identifier, call_id, "the joiner must be attached to the ongoing call");
    assert!(!d_connect.call_ownership, "a late joiner does not take call ownership");
    assert_eq!(
        d_connect.transmission_grant,
        TransmissionGrant::GrantedToOtherUser,
        "the ongoing speaker keeps the floor"
    );
}

fn brew_test_config() -> tetra_config::bluestation::StackConfig {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        feature_lip_forward: false,
        lip_forward_issi: None,
        pbx_gateway_issis: None,
        backhaul_hysteresis_secs: 0,
    });
    config
}

/// Remote GROUP_TX with no local listeners must Hold (pending) instead of NetworkCallEnd.
#[test]
fn test_network_call_start_without_listeners_holds() {
    debug::setup_logging_verbose();

    let gssi = 330;
    let brew_uuid = uuid::Uuid::parse_str("c1a1e03c-0489-4106-a246-5ccddf75e657").unwrap();
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2200107,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(
        msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallHold {
                brew_uuid: u,
                dest_gssi,
            }) if *u == brew_uuid && *dest_gssi == gssi
        )),
        "expected NetworkCallHold when no local listeners"
    );
    assert!(
        !msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: u }) if *u == brew_uuid
        )),
        "must not tear down with NetworkCallEnd (late-entry hold path)"
    );
    assert!(
        !msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { brew_uuid: u, .. }) if *u == brew_uuid
        )),
        "must not allocate a circuit while held"
    );
}

/// First Affiliate after a held inbound promotes via GroupListenersAvailable.
#[test]
fn test_affiliate_promotes_held_inbound_with_group_listeners_available() {
    debug::setup_logging_verbose();

    let gssi = 331;
    let brew_uuid = uuid::Uuid::parse_str("c1a1e03c-0489-4106-a246-5ccddf75e658").unwrap();
    let local_issi = 2200700;
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // Hold path first (no listeners).
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2200107,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(1));
    let held = test.dump_sinks();
    assert!(held.iter().any(|m| matches!(
        &m.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCallHold { .. })
    )));

    // First local ear: Register + Affiliate → GroupListenersAvailable.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: local_issi,
            groups: vec![],
            action: BrewSubscriberAction::Register,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: local_issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        }),
    });
    test.run_stack(Some(1));
    let after = test.dump_sinks();

    assert!(
        after.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { gssi: g }) if *g == gssi
        )),
        "first Affiliate must signal GroupListenersAvailable for Brew promote"
    );
}

/// Mid-QSO Affiliate on an active GSSI: event LE D-SETUP + OngoingGroupCall for LST console RX.
#[test]
fn test_affiliate_during_active_group_call_emits_late_entry_snapshot() {
    debug::setup_logging_verbose();

    let gssi = 332;
    let first_issi = 2200701;
    let joiner_issi = 2200702;
    let brew_uuid = uuid::Uuid::parse_str("c1a1e03c-0489-4106-a246-5ccddf75e659").unwrap();
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, first_issi, gssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2200107,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(2));
    let mut ready = test.dump_sinks();
    assert!(ready.iter().any(|m| matches!(
        &m.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { brew_uuid: u, .. }) if *u == brew_uuid
    )));

    // Mark initial D-SETUP transmitted so event LE is not throttled by Pending receipt.
    for r in extract_d_setup_reporters(&mut ready) {
        if r.get_state() == TxState::Pending {
            r.mark_transmitted();
        }
    }

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: joiner_issi,
            groups: vec![],
            action: BrewSubscriberAction::Register,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: joiner_issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    assert!(
        msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::OngoingGroupCall { gssi: g, .. }) if *g == gssi
        )),
        "Affiliate during active QSO must emit OngoingGroupCall for LST RX"
    );
    assert!(
        count_d_setups(&msgs) > 0,
        "Affiliate during active QSO must trigger immediate late-entry D-SETUP"
    );
    assert!(
        !msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { .. })
        )),
        "second listener must not re-promote (GroupListenersAvailable is first-listener only)"
    );
}

/// A radio that arrives on a cell during a group call is registered first (answer to the
/// station's D-LOCATION UPDATE COMMAND, which already joins it to the call), then does its own
/// roaming registration and attaches its groups again (clause 16.4.1.1 a)). The late-entry
/// D-SETUP sent before that is ignored by the radio, so the re-attach must repeat it at once
/// instead of leaving the radio until the periodic one (field test 2026-11-02, F1).
#[test]
fn test_reattach_during_active_group_call_repeats_late_entry_d_setup() {
    debug::setup_logging_verbose();

    let gssi = 334;
    let issi = 2200704;
    let brew_uuid = uuid::Uuid::parse_str("c1a1e03c-0489-4106-a246-5ccddf75e65b").unwrap();
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // Re-attach without a call on the group: nothing to send.
    register_subscriber(&mut test, issi, gssi);
    let affiliate = || SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Affiliate,
        }),
    };
    test.submit_message(affiliate());
    test.run_stack(Some(1));
    assert_eq!(count_d_setups(&test.dump_sinks()), 0, "no call on the group, no D-SETUP");

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2200107,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(2));
    let mut ready = test.dump_sinks();
    for r in extract_d_setup_reporters(&mut ready) {
        if r.get_state() == TxState::Pending {
            r.mark_transmitted();
        }
    }

    // The radio attaches the group it is already counted in once more.
    test.submit_message(affiliate());
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(count_d_setups(&msgs) > 0, "re-attach during the call must repeat the late-entry D-SETUP at once");
    assert!(
        !msgs.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::OngoingGroupCall { .. } | CallControl::GroupListenersAvailable { .. })
        )),
        "a re-attach is no new listener for Brew or the LST console"
    );
}

/// Mid-QSO deaffiliate of the last listener must Hold (not End) so re-Affiliate can late-enter.
#[test]
fn test_deaffiliate_last_listener_holds_network_call() {
    debug::setup_logging_verbose();

    let gssi = 333;
    let local_issi = 2200703;
    let brew_uuid = uuid::Uuid::parse_str("c1a1e03c-0489-4106-a246-5ccddf75e65a").unwrap();
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, local_issi, gssi);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2200107,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(2));
    let ready = test.dump_sinks();
    assert!(ready.iter().any(|m| matches!(
        &m.msg,
        SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { brew_uuid: u, .. }) if *u == brew_uuid
    )));

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
            issi: local_issi,
            groups: vec![gssi],
            action: BrewSubscriberAction::Deaffiliate,
        }),
    });
    test.run_stack(Some(1));
    let after = test.dump_sinks();

    assert!(
        after.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallHold {
                brew_uuid: u,
                dest_gssi,
            }) if *u == brew_uuid && *dest_gssi == gssi
        )),
        "last listener leaving must Hold the Brew session for late entry"
    );
    assert!(
        !after.iter().any(|m| matches!(
            &m.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: u }) if *u == brew_uuid
        )),
        "must not End the Brew session when the last listener leaves the TG"
    );
}

/// Multi-cell: a radio that reselected from a sibling cell restores the group call with that
/// cell's call identifier. The linked cell finds the call by GSSI and answers with its own id.
#[test]
fn test_site_linked_restore_from_sibling_cell_matches_group() {
    debug::setup_logging_verbose();

    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    const MOVER: u32 = 1_000_777;
    let sibling_call_id = call_id + 100;
    test.submit_message(build_u_call_restore_msg(MOVER, sibling_call_id, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (restore, chan) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE, not D-RELEASE");
    // Clause 14.5.2.2.4: the identifier the radio knows, and this cell's as the new one.
    assert_eq!(restore.call_identifier, sibling_call_id);
    assert_eq!(restore.new_call_identifier, Some(call_id as u64), "the radio adopts this cell's call id");
    assert!(chan.is_some(), "sent to the call's traffic channel");
}

/// An MS that reselected mid individual (private) call restores with the sibling cell's call id;
/// the private call is matched here by the (sender, peer) ISSI pair, not by the id.
#[test]
fn test_site_linked_restore_from_sibling_cell_matches_individual() {
    debug::setup_logging_verbose();

    const CALLER: u32 = 1_000_111;
    const CALLED: u32 = 1_000_222;

    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    test.config.state_write().subscribers.register(CALLED);

    // Active simplex individual call CALLER -> CALLED.
    test.submit_message(build_individual_u_setup_msg_with_mode(CALLER, CALLED, false));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), CALLED);
    test.submit_message(build_u_connect_msg(CALLED, call_id, false));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    // CALLED reselects to a sibling cell and restores with the OLD cell's id + the peer (CALLER).
    let sibling_call_id = call_id + 100;
    test.submit_message(build_u_call_restore_msg(CALLED, sibling_call_id, CALLER, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (mut sdu, _) = find_lcmc_req(&msgs, CALLED, CmcePduTypeDl::DCallRestore)
        .expect("D-CALL RESTORE, not D-RELEASE — the private call is matched by the ISSI pair");
    let restore = DCallRestore::from_bitbuf(&mut sdu).expect("parse DCallRestore");
    // Clause 14.5.1.2.4 / table 14.6: the identifier the radio restores, this cell's as the new one.
    assert_eq!(restore.call_identifier, sibling_call_id);
    assert_eq!(restore.new_call_identifier, Some(call_id as u64), "the radio adopts this cell's local call id");
}

/// Multi-cell individual-call handover, target side: the site switch pre-provisions a restoring
/// leg on this cell (SiteHandoverIndividualCall) for an MS reselecting in mid-call. When the MS
/// then sends U-CALL RESTORE with the old cell's id, it is matched by ISSI pair and answered with
/// a D-CALL RESTORE that *carries a channel allocation* — the MS is arriving from a sibling cell
/// and must be told this cell's traffic channel (an ordinary same-cell restore carries none).
#[test]
fn test_site_handover_preprovisions_individual_leg_and_restores_with_channel() {
    debug::setup_logging_verbose();

    const MOVER: u32 = 1_000_333;
    const PEER: u32 = 1_000_444;
    // High fixed id for the "old cell" so it never collides with the small id auto-allocated to
    // the pre-provisioned leg; the point is that the reply uses this cell's id, not this one.
    // (Call identifier is a 14-bit field, so this must stay <= 16383.)
    const SIBLING_CALL_ID: u16 = 9_000;

    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // Switch pre-provisions the restoring leg ahead of the MS arriving (duplex call MOVER <-> PEER).
    let brew_uuid = uuid::Uuid::parse_str("b1c2d3e4-f5a6-47b8-9c0d-1e2f3a4b5c6d").unwrap();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::SiteHandoverIndividualCall {
            brew_uuid,
            issi: MOVER,
            peer_issi: PEER,
            simplex_duplex: true,
            priority: 1,
        }),
    });
    test.run_stack(Some(1));
    // Pre-provision opens a traffic channel for the leg before the radio ever appears.
    let provision_msgs = test.dump_sinks();
    assert!(
        provision_msgs
            .iter()
            .any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::Open(_)))),
        "pre-provision must open a traffic channel so U-CALL RESTORE lands on a real circuit"
    );

    // The MS arrives on this cell and restores with the OLD cell id + the peer; matched by pair.
    test.submit_message(build_u_call_restore_msg(MOVER, SIBLING_CALL_ID, PEER, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (mut sdu, chan) = find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore)
        .expect("D-CALL RESTORE, not D-RELEASE — the pre-provisioned leg is matched by the ISSI pair");
    let restore = DCallRestore::from_bitbuf(&mut sdu).expect("parse DCallRestore");
    // Clause 14.5.1.2.4 / table 14.6: the identifier the radio restores, this cell's as the new one.
    assert_eq!(restore.call_identifier, SIBLING_CALL_ID);
    let new_id = restore.new_call_identifier.expect("this cell's call id as the new call identifier");
    assert_ne!(new_id, SIBLING_CALL_ID as u64, "the radio is moved onto this cell's pre-provisioned call id");
    assert!(
        chan.is_some(),
        "cross-cell restore must carry a channel allocation to move the MS onto this cell's traffic channel"
    );
}

/// Without Brew an unknown call identifier is still rejected (single-cell behaviour): no other
/// cell shares the call.
#[test]
fn test_restore_with_unknown_call_id_rejected_without_brew() {
    debug::setup_logging_verbose();

    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    test.submit_message(build_u_call_restore_msg(1_000_777, call_id + 100, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, 1_000_777, CmcePduTypeDl::DCallRestore).is_none());
    assert!(find_lcmc_req(&msgs, 1_000_777, CmcePduTypeDl::DRelease).is_some(), "rejected at once");
    assert!(!msgs.iter().any(|m| matches!(m.msg, SapMsgInner::MmRequestLocationUpdate { .. })));
}

// ── Restore after reselection between separate stations on one Brew network ──────────────────
// TS 100 392-2 clause 18.4.1.4.7 (U-RESTORE) / 14.7.2.2 (U-CALL RESTORE): the MS restores its
// group call on the new cell; clause 14.7.1.3 lets D-CALL RESTORE give it this cell's call id.

const MOVER: u32 = 1_000_777;

fn brew_restore_test() -> ComponentTest {
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );
    test
}

fn network_call_start(test: &mut ComponentTest, brew_uuid: uuid::Uuid, gssi: u32) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: 2_200_107,
            dest_gssi: gssi,
            priority: 0,
        }),
    });
}

fn mm_update(test: &mut ComponentTest, issi: u32, groups: Vec<u32>, action: BrewSubscriberAction) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Mm,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
    });
}

fn held(msgs: &[SapMsg], gssi: u32) -> bool {
    msgs.iter().any(|m| {
        matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCallHold { dest_gssi, .. }) if *dest_gssi == gssi)
    })
}

fn lu_requested(msgs: &[SapMsg], issi: u32) -> bool {
    msgs.iter()
        .any(|m| m.dest == TetraEntity::Mm && matches!(m.msg, SapMsgInner::MmRequestLocationUpdate { issi: i } if i == issi))
}

/// Separate Brew station, group call already running here (another local radio listens): the MS
/// brings the other station's call id and is restored into the local call by GSSI.
#[test]
fn test_brew_restore_from_other_station_matches_group() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
    test.run_stack(Some(1));
    test.dump_sinks();
    test.config.state_write().subscribers.register(MOVER);

    test.submit_message(build_u_call_restore_msg(MOVER, call_id + 100, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (restore, _) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE, not D-RELEASE");
    assert_eq!(restore.call_identifier, call_id + 100);
    assert_eq!(restore.new_call_identifier, Some(call_id as u64), "the radio adopts this cell's call id");
    assert!(!lu_requested(&msgs, MOVER), "a registered radio needs no location update");
}

/// Same location area, call already running here: the restore is answered at once, and the
/// radio, which has no registration on this station, is asked to register (clause 16).
#[test]
fn test_brew_restore_into_running_call_asks_unregistered_radio_to_register() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    test.submit_message(build_u_call_restore_msg(MOVER, call_id + 100, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_some());
    assert!(lu_requested(&msgs, MOVER));
}

/// Call identifiers are per station: the identifier the radio brings can belong to a local call
/// to another group. The radio is restored into the call to its own group, not that one.
#[test]
fn test_brew_restore_ignores_call_id_of_another_group() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    const OTHER_GSSI: u32 = 92;

    register_subscriber(&mut test, TEST_ISSI, OTHER_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, OTHER_GSSI));
    test.run_stack(Some(1));
    let other_call = first_d_setup_call_id(&test.dump_sinks(), OTHER_GSSI);

    register_subscriber(&mut test, 1_000_002, TEST_GSSI);
    test.submit_message(build_u_setup_msg(1_000_002, TEST_GSSI));
    test.run_stack(Some(1));
    let own_call = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);
    assert_ne!(own_call, other_call);

    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(build_u_call_restore_msg(MOVER, other_call, TEST_GSSI, false));
    test.run_stack(Some(1));
    let (restore, _) = d_call_restore_to(&test.dump_sinks(), MOVER).expect("restored");
    assert_eq!(restore.call_identifier, other_call);
    assert_eq!(restore.new_call_identifier, Some(own_call as u64));
}

/// Without Brew the same mismatch is refused instead of joining the wrong group's call.
#[test]
fn test_restore_refuses_call_id_of_another_group_without_brew() {
    debug::setup_logging_verbose();
    let config = ComponentTest::get_default_test_config(StackMode::Bs);
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );
    register_subscriber(&mut test, TEST_ISSI, 92);
    test.submit_message(build_u_setup_msg(TEST_ISSI, 92));
    test.run_stack(Some(1));
    let other_call = first_d_setup_call_id(&test.dump_sinks(), 92);

    test.submit_message(build_u_call_restore_msg(MOVER, other_call, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_none());
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_some());
}

// ── Individual call restore between separate stations (milestone 5a) ──────────────────────
// TS 100 392-2 clause 14.5.1.2.4: an MS in an individual call restores it on the new cell with
// U-CALL RESTORE (old call identifier, other party, basic service information point-to-point).
// The station asks the network for the call (`call-restore`) and answers with D-CALL RESTORE on a
// traffic channel of its own, the old identifier kept and its own as the new one.

const RESTORE_PEER: u32 = 2_200_107;
const OLD_CALL_ID: u16 = 4321;

fn p2p_call_restore(issi: u32, other_may_transmit: bool) -> SapMsg {
    p2p_call_restore_of(issi, OLD_CALL_ID, other_may_transmit)
}

fn p2p_call_restore_of(issi: u32, call_identifier: u16, other_may_transmit: bool) -> SapMsg {
    let pdu = UCallRestore {
        call_identifier,
        request_to_transmit_send_data: other_may_transmit,
        other_party_type_identifier: 1,
        other_party_short_number_address: None,
        other_party_ssi: Some(RESTORE_PEER as u64),
        other_party_extension: None,
        basic_service_information: Some(BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        }),
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);
    lcmc_ind(issi, sdu)
}

fn restore_answer(issi: u32, call: Option<NetworkRestoredCall>) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreAnswer {
            issi,
            peer: RESTORE_PEER,
            call,
        }),
    }
}

fn moved_call(duplex: bool, floor_issi: Option<u32>) -> NetworkRestoredCall {
    NetworkRestoredCall {
        brew_uuid: uuid::Uuid::parse_str("5a7c22f6-5c8c-4008-ac84-6fb79023888f").unwrap(),
        simplex_duplex: duplex,
        priority: 0,
        floor_issi,
    }
}

fn restore_requests(msgs: &[SapMsg]) -> Vec<(u32, u32)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::IndividualCallRestoreRequest { issi, peer }) => Some((*issi, *peer)),
            _ => None,
        })
        .collect()
}

/// Restore `MOVER`'s individual call after the network answered `call`; returns all messages.
fn restore_moved_call(other_may_transmit: bool, call: NetworkRestoredCall) -> Vec<SapMsg> {
    let mut test = brew_restore_test();
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, other_may_transmit));
    test.run_stack(Some(1));
    test.dump_sinks();
    test.submit_message(restore_answer(MOVER, Some(call)));
    test.run_stack(Some(1));
    test.dump_sinks()
}

/// Not held here: the station asks the network instead of refusing at once, and refuses only when
/// no answer came within its 2 s (well inside the MS's T306 of at least 4 s).
#[test]
fn test_brew_restore_of_unknown_individual_call_asks_the_network() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert_eq!(restore_requests(&msgs), vec![(MOVER, RESTORE_PEER)]);
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none(), "waits for the network");
    assert!(!msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { .. }))));

    // A repeated U-CALL RESTORE does not ask again.
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(100));
    let msgs = test.dump_sinks();
    assert!(restore_requests(&msgs).is_empty());
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none());

    test.run_stack(Some(60));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_some(), "refused after 2 s without an answer");
}

/// Call identifiers are per station: the identifier the MS brings from the station it left can be
/// a local call here between two other radios. That call is not the MS's; the station asks the
/// network as for any other unknown call and leaves the local call alone.
#[test]
fn test_restore_with_an_identifier_used_here_by_another_call_asks_the_network() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    let (remote_issi, local_issi) = (2_200_699, 2_200_769);
    test.config.state_write().subscribers.register(local_issi);
    test.config.state_write().subscribers.register(MOVER);
    let brew_uuid = uuid::Uuid::parse_str("a9661625-c1f2-42bb-b256-c44e14677307").unwrap();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: NetworkCircuitCall {
                source_issi: remote_issi,
                destination: local_issi,
                number: String::new(),
                priority: 0,
                service: 0,
                mode: 0,
                duplex: 1,
                method: 0,
                communication: 0,
                grant: 0,
                permission: 0,
                timeout: 0,
                ownership: 0,
                queued: 0,
            },
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut setup_sdu, _) = find_lcmc_req(&msgs, local_issi, CmcePduTypeDl::DSetup).expect("D-SETUP to the local radio");
    let local_call_id = DSetup::from_bitbuf(&mut setup_sdu).unwrap().call_identifier;
    test.submit_message(build_u_connect_msg(local_issi, local_call_id, true));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(p2p_call_restore_of(MOVER, local_call_id, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert_eq!(restore_requests(&msgs), vec![(MOVER, RESTORE_PEER)], "asks the network");
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none(), "not refused at once");
    assert!(find_lcmc_req(&msgs, local_issi, CmcePduTypeDl::DRelease).is_none(), "the local call runs on");
}

/// Multi-cell station (milestone 7): a cell of a linked station asks the site switch the same way
/// for an individual call it does not hold (the call may run with the network from a sibling
/// cell), and restores it on a channel here when the switch moves it.
#[test]
fn test_site_linked_restore_of_unknown_individual_call_asks_the_switch() {
    debug::setup_logging_verbose();
    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert_eq!(restore_requests(&msgs), vec![(MOVER, RESTORE_PEER)], "asks the switch");
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none(), "waits for the answer");

    test.submit_message(restore_answer(MOVER, Some(moved_call(true, None))));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (restore, chan) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE, not D-RELEASE");
    assert_eq!(restore.call_identifier, OLD_CALL_ID);
    assert!(restore.new_call_identifier.is_some() && chan.is_some());
    assert!(msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { .. }))));
}

/// The field case of 2026-10-30 (duplex, BS1 → BS2): D-CALL RESTORE keeps the old identifier,
/// gives this station's as the new one, grants (duplex) and sends the MS to a traffic channel here;
/// the voice of the call is then bridged to the network.
#[test]
fn test_network_moved_duplex_call_is_restored_on_a_channel_here() {
    debug::setup_logging_verbose();
    let call = moved_call(true, None);
    let msgs = restore_moved_call(false, call);
    let (restore, chan) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE, not D-RELEASE");
    // Even with the request bit clear: "not granted" is only applicable for simplex calls.
    let (relaxed, _) = d_call_restore_to(&restore_moved_call(true, call), MOVER).expect("D-CALL RESTORE");
    assert_eq!(relaxed.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(restore.call_identifier, OLD_CALL_ID, "clause 14.5.1.2.4: the identifier the MS restores");
    let new_id = restore.new_call_identifier.expect("this station's identifier as the new one") as u16;
    assert_ne!(new_id, OLD_CALL_ID);
    assert!(chan.is_some(), "the U-plane is switched on, so a channel allocation accompanies it");
    assert_eq!(restore.transmission_grant, TransmissionGrant::Granted.into_raw() as u8, "a duplex call is always granted");
    assert!(msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::Open(_)))));
    let media = msgs.iter().find_map(|m| match &m.msg {
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { brew_uuid, call_id, .. }) => Some((*brew_uuid, *call_id)),
        _ => None,
    });
    assert_eq!(media, Some((call.brew_uuid, new_id)));
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none());
}

/// Simplex with the other party talking: the MS is "granted to another user" so it hears the
/// peer, even when it did not ask to transmit.
#[test]
fn test_network_moved_simplex_call_with_the_peer_talking_is_granted_to_the_other_user() {
    debug::setup_logging_verbose();
    for other_may_transmit in [true, false] {
        let msgs = restore_moved_call(other_may_transmit, moved_call(false, Some(RESTORE_PEER)));
        let (restore, chan) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE");
        assert_eq!(restore.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
        assert!(chan.is_some());
        assert!(msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted { .. }))));
    }
}

/// Simplex with the other party talking and the MS asking to transmit: it assumes its request is
/// queued (clause 14.5.1.2.4), so the station queues it and grants it once the peer stops.
#[test]
fn test_network_moved_simplex_request_while_the_peer_talks_is_granted_when_the_peer_stops() {
    debug::setup_logging_verbose();
    let call = moved_call(false, Some(RESTORE_PEER));
    let mut test = brew_restore_test();
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(1));
    test.dump_sinks();
    test.submit_message(restore_answer(MOVER, Some(call)));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (restore, _) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE");
    assert_eq!(restore.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle {
            brew_uuid: call.brew_uuid,
            grant: 0,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DTxGranted).expect("D-TX GRANTED for the queued request");
    let granted = DTxGranted::from_bitbuf(&mut sdu).unwrap();
    assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
}

/// Simplex with the floor free: an MS that asks to transmit gets it, one that does not is
/// "not granted".
#[test]
fn test_network_moved_simplex_call_with_a_free_floor_follows_the_request() {
    debug::setup_logging_verbose();
    let to_brew = |msgs: &[SapMsg]| {
        msgs.iter()
            .filter_map(|m| match &m.msg {
                SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexGranted { .. }) => Some("granted"),
                SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSimplexIdle { .. }) => Some("idle"),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Floor free on the network: granted, and the other party is told this MS talks.
    let msgs = restore_moved_call(false, moved_call(false, None));
    let (restore, _) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE");
    assert_eq!(restore.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert_eq!(to_brew(&msgs), vec!["granted"]);
    // It talked before the break and goes on: the network already knows.
    let msgs = restore_moved_call(false, moved_call(false, Some(MOVER)));
    assert_eq!(d_call_restore_to(&msgs, MOVER).unwrap().0.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(to_brew(&msgs).is_empty());
    // It talked and does not go on: its permission ended, the floor is free for the other party.
    let msgs = restore_moved_call(true, moved_call(false, Some(MOVER)));
    let (restore, _) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE");
    assert_eq!(restore.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert_eq!(to_brew(&msgs), vec!["idle"]);
    // Neither talked: nothing to tell.
    let msgs = restore_moved_call(true, moved_call(false, None));
    assert_eq!(d_call_restore_to(&msgs, MOVER).unwrap().0.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert!(to_brew(&msgs).is_empty());
}

/// No call on the network: refused (D-RELEASE, which MLE turns into D-RESTORE-FAIL).
#[test]
fn test_restore_the_network_has_no_call_for_is_refused() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(1));
    test.dump_sinks();
    test.submit_message(restore_answer(MOVER, None));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_some());
    assert!(d_call_restore_to(&msgs, MOVER).is_none());
}

/// The answer came after the MS was refused: the call the network moved here is released there.
#[test]
fn test_a_late_network_answer_releases_the_moved_call() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(p2p_call_restore(MOVER, false));
    test.run_stack(Some(160));
    test.dump_sinks();
    let call = moved_call(true, None);
    test.submit_message(restore_answer(MOVER, Some(call)));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(d_call_restore_to(&msgs, MOVER).is_none());
    let released = msgs.iter().find_map(|m| match &m.msg {
        SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }) => Some((*brew_uuid, *cause)),
        _ => None,
    });
    assert_eq!(released, Some((call.brew_uuid, DisconnectCause::CallRestorationOfTheOtherUserFailed.into_raw() as u8)));
}

/// A local-only group (inside `local_ssi_ranges`) is never shared over Brew: a call with the same
/// GSSI here is a different call, so an unknown call id is rejected.
#[test]
fn test_brew_restore_rejected_for_local_only_group() {
    debug::setup_logging_verbose();
    use tetra_core::ranges::SortedDisjointSsiRanges;

    let mut config = brew_test_config();
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(0, 100)]);
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    test.submit_message(build_u_call_restore_msg(MOVER, call_id + 100, TEST_GSSI, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_none());
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_some());
}

/// The milestone 0 baseline: Brew holds the group call as pending inbound because nobody on this
/// station listens yet. The restore must not be rejected; it brings the call up and is answered
/// once the call runs here.
#[test]
fn test_brew_restore_promotes_pending_network_call() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    let gssi = 330;
    let brew_uuid = uuid::Uuid::parse_str("d2b2e03c-0489-4106-a246-5ccddf75e657").unwrap();

    // The network call arrives first and is held (no local listener).
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(1));
    assert!(held(&test.dump_sinks(), gssi));

    // The radio arrives (registered here already, different LA) and restores with the old id.
    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
    test.config.state_write().subscribers.register(MOVER);
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(build_u_call_restore_msg(MOVER, 4321, gssi, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none(), "not rejected");
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_none(), "no call to restore into yet");
    assert!(
        msgs.iter().any(|m| m.dest == TetraEntity::Brew
            && matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { gssi: g }) if *g == gssi)),
        "the restoring radio is the first listener: Brew promotes the held call"
    );
    assert!(!lu_requested(&msgs, MOVER));

    // Brew promotes: the call comes up and the parked restore is answered with the new id.
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();
    assert!(!held(&msgs, gssi), "the restore counts as a listener");
    let call_id = first_d_setup_call_id(&msgs, gssi);
    let (restore, chan) = d_call_restore_to(&msgs, MOVER).expect("D-CALL RESTORE once the call is up");
    assert_eq!(restore.call_identifier, 4321);
    assert_eq!(restore.new_call_identifier, Some(call_id as u64));
    // Someone talks on the network: the listener's U-plane goes on to receive, and a layer 2
    // channel allocation must accompany the D-CALL RESTORE (clause 14.5.2.2.4).
    assert_eq!(restore.transmission_grant, TransmissionGrant::GrantedToOtherUser.into_raw() as u8);
    let (carrier, ts) = chan.expect("channel allocation with the restore");
    let (setup_carrier, setup_ts) = d_setup_channel(&msgs, gssi);
    assert_eq!((carrier, ts), (setup_carrier, setup_ts), "the call's traffic channel");
}

/// A listener restoring between two overs (nobody talks): "not granted", the U-plane stays off,
/// and the radio is still sent to the call's traffic channel, which clause 14.5.2.2.4 allows.
#[test]
fn test_brew_restore_between_overs_is_not_granted_but_sent_to_the_channel() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let call_id = first_d_setup_call_id(&msgs, TEST_GSSI);
    let channel = d_setup_channel(&msgs, TEST_GSSI);
    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.config.state_write().subscribers.register(MOVER);
    test.submit_message(build_u_call_restore_msg(MOVER, call_id + 100, TEST_GSSI, false));
    test.run_stack(Some(1));
    let (restore, chan) = d_call_restore_to(&test.dump_sinks(), MOVER).expect("restored");
    assert_eq!(restore.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert_eq!(chan, Some(channel));
}

/// Same location area: the radio did not register here (clause 16), so the cell asks it to
/// (D-LOCATION UPDATE COMMAND via MM). Its registration moves the ISSI to this station on Brew.
#[test]
fn test_brew_restore_from_unregistered_radio_requests_location_update() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    let gssi = 331;

    test.submit_message(build_u_call_restore_msg(MOVER, 4321, gssi, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(lu_requested(&msgs, MOVER), "unknown radio is asked to register");
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_none());

    // It registers and reports its group; the network then hands over the call.
    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
    test.config.state_write().subscribers.register(MOVER);
    mm_update(&mut test, MOVER, vec![gssi], BrewSubscriberAction::Affiliate);
    test.run_stack(Some(1));
    test.dump_sinks();
    let brew_uuid = uuid::Uuid::parse_str("e3c3e03c-0489-4106-a246-5ccddf75e657").unwrap();
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_some());
}

/// Nothing comes up in time: the restore is rejected (MLE answers D-RESTORE-FAIL) and the
/// listener count it added is gone again, so a later network call is held as before.
#[test]
fn test_brew_restore_times_out_and_drops_its_listener() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    let gssi = 332;
    test.config.state_write().subscribers.register(MOVER);
    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(build_u_call_restore_msg(MOVER, 4321, gssi, false));
    test.run_stack(Some(300));
    assert!(find_lcmc_req(&test.dump_sinks(), MOVER, CmcePduTypeDl::DRelease).is_none(), "still waiting after ~4 s");
    test.run_stack(Some(100));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DRelease).is_some(), "rejected after 5 s");
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_none());

    let brew_uuid = uuid::Uuid::parse_str("f4d4e03c-0489-4106-a246-5ccddf75e657").unwrap();
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(1));
    assert!(held(&test.dump_sinks(), gssi), "no listener left behind");
}

/// The listener a restore added lasts as long as the restored call; an MM affiliation to the group
/// in between takes it over (counted once), so the radio keeps getting later calls.
#[test]
fn test_brew_restore_listener_ends_with_call_unless_affiliated() {
    debug::setup_logging_verbose();
    for affiliate in [false, true] {
        let mut test = brew_restore_test();
        let gssi = 333;
        let first = uuid::Uuid::parse_str("a5e5e03c-0489-4106-a246-5ccddf75e657").unwrap();
        test.config.state_write().subscribers.register(MOVER);
        mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
        test.run_stack(Some(1));

        test.submit_message(build_u_call_restore_msg(MOVER, 4321, gssi, false));
        test.run_stack(Some(1));
        network_call_start(&mut test, first, gssi);
        test.run_stack(Some(2));
        let msgs = test.dump_sinks();
        let call_id = first_d_setup_call_id(&msgs, gssi);
        assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_some());

        if affiliate {
            mm_update(&mut test, MOVER, vec![gssi], BrewSubscriberAction::Affiliate);
            test.run_stack(Some(1));
        }
        // The network ends the call; it is released after hang time.
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Brew,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid: first }),
        });
        test.run_stack(Some(2000));
        let msgs = test.dump_sinks();
        assert!(find_lcmc_req(&msgs, gssi, CmcePduTypeDl::DRelease).is_some(), "call {} released", call_id);

        let second = uuid::Uuid::parse_str("b6f6e03c-0489-4106-a246-5ccddf75e657").unwrap();
        network_call_start(&mut test, second, gssi);
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();
        assert_eq!(held(&msgs, gssi), !affiliate, "affiliated={affiliate}");

        if affiliate {
            // Counted once: deaffiliating leaves no listener behind.
            mm_update(&mut test, MOVER, vec![gssi], BrewSubscriberAction::Deaffiliate);
            test.run_stack(Some(1));
            assert!(held(&test.dump_sinks(), gssi), "single count, last listener gone");
        }
    }
}

/// Deregistering the radio drops a parked restore and its listener.
#[test]
fn test_brew_restore_dropped_on_deregister() {
    debug::setup_logging_verbose();
    let mut test = brew_restore_test();
    let gssi = 334;
    test.config.state_write().subscribers.register(MOVER);
    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Register);
    test.run_stack(Some(1));

    test.submit_message(build_u_call_restore_msg(MOVER, 4321, gssi, false));
    test.run_stack(Some(1));
    mm_update(&mut test, MOVER, vec![], BrewSubscriberAction::Deregister);
    test.run_stack(Some(1));
    test.dump_sinks();

    let brew_uuid = uuid::Uuid::parse_str("c7a7e03c-0489-4106-a246-5ccddf75e657").unwrap();
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(2));
    assert!(held(&test.dump_sinks(), gssi), "its listener is gone");

    // Someone else brings the call up: the gone radio's restore must not be answered.
    register_subscriber(&mut test, TEST_ISSI, gssi);
    network_call_start(&mut test, brew_uuid, gssi);
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();
    first_d_setup_call_id(&msgs, gssi);
    assert!(find_lcmc_req(&msgs, MOVER, CmcePduTypeDl::DCallRestore).is_none());
}

/// Multi-cell: a raised-priority local group call tells the site switch its priority right before
/// the floor grant, so the other cells get it as an emergency call too.
#[test]
fn test_site_linked_group_call_reports_priority_to_switch() {
    debug::setup_logging_verbose();

    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);
    test.submit_message(build_u_setup_msg_prio(TEST_ISSI, TEST_GSSI, 15));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let to_brew: Vec<&CallControl> = msgs
        .iter()
        .filter(|m| m.dest == TetraEntity::Brew)
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(cc) => Some(cc),
            _ => None,
        })
        .collect();
    let prio = to_brew
        .iter()
        .position(|cc| matches!(cc, CallControl::SiteCallPriority { priority, .. } if *priority > 0))
        .expect("SiteCallPriority sent to the switch");
    let grant = to_brew
        .iter()
        .position(|cc| matches!(cc, CallControl::FloorGranted { .. }))
        .expect("FloorGranted sent to the switch");
    assert!(prio < grant, "priority comes first");
}

/// Network over on `gssi` whose talker `talker` is registered here (it was heard through the
/// network and then came to this cell unannounced). Returns the test and the call id.
fn network_over_of_moved_talker(gssi: u32, talker: u32, listener: u32, brew_uuid: uuid::Uuid) -> (ComponentTest, u16) {
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, listener, gssi);
    register_subscriber(&mut test, talker, gssi);
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi: talker,
            dest_gssi: gssi,
            priority: 0,
        }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let call_id = msgs
        .iter()
        .find_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCallReady { brew_uuid: u, call_id, .. }) if *u == brew_uuid => {
                Some(*call_id)
            }
            _ => None,
        })
        .expect("Expected the network over to become ready");
    (test, call_id)
}

fn local_floor_granted_to(msgs: &[SapMsg], dest: TetraEntity, issi: u32) -> bool {
    msgs.iter().any(|m| {
        m.dest == dest && matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { source_issi, .. }) if *source_issi == issi)
    })
}

/// The old station's late end of the over: must not take the floor from the talker here.
fn end_old_session_keeps_over(test: &mut ComponentTest, gssi: u32, brew_uuid: uuid::Uuid) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(
        find_lcmc_req(&msgs, gssi, CmcePduTypeDl::DTxCeased).is_none(),
        "the old session's end must not end the moved talker's over here"
    );
    assert!(
        !msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::FloorReleased { .. }))),
        "the old session's end must not release the floor here"
    );
}

/// Case C: a talker heard here through the network comes over unannounced and presses PTT again
/// (U-SETUP, late entry). It keeps its floor (TS 100 392-2 clause 14.5.2.2.1 a)); another
/// member still gets "granted to another user".
#[test]
fn test_moved_network_talker_late_entry_keeps_floor() {
    debug::setup_logging_verbose();
    let gssi = 340;
    let talker = 3400001;
    let listener = 3400002;
    let brew_uuid = uuid::Uuid::from_u128(0x3400);
    let (mut test, call_id) = network_over_of_moved_talker(gssi, talker, listener, brew_uuid);

    test.submit_message(build_u_setup_msg(listener, gssi));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, listener, CmcePduTypeDl::DConnect).expect("Expected D-CONNECT to the listener");
    let d_connect = DConnect::from_bitbuf(&mut sdu).unwrap();
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(!local_floor_granted_to(&msgs, TetraEntity::Brew, listener));

    test.submit_message(build_u_setup_msg(talker, gssi));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, talker, CmcePduTypeDl::DConnect).expect("Expected D-CONNECT to the talker");
    let d_connect = DConnect::from_bitbuf(&mut sdu).unwrap();
    assert_eq!(d_connect.call_identifier, call_id);
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted, "the moved talker keeps its floor");
    assert!(local_floor_granted_to(&msgs, TetraEntity::Umac, talker), "UMAC must arm UL inactivity");
    assert!(local_floor_granted_to(&msgs, TetraEntity::Brew, talker), "the over now goes out from here");

    end_old_session_keeps_over(&mut test, gssi, brew_uuid);
}

/// Case C with U-TX DEMAND: the moved talker is granted at once instead of being queued.
#[test]
fn test_moved_network_talker_tx_demand_is_granted() {
    debug::setup_logging_verbose();
    let gssi = 341;
    let talker = 3410001;
    let listener = 3410002;
    let brew_uuid = uuid::Uuid::from_u128(0x3410);
    let (mut test, call_id) = network_over_of_moved_talker(gssi, talker, listener, brew_uuid);

    test.submit_message(build_u_tx_demand_msg(listener, call_id));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, listener, CmcePduTypeDl::DTxGranted).expect("Expected D-TX GRANTED to the listener");
    let granted = DTxGranted::from_bitbuf(&mut sdu).unwrap();
    assert_eq!(granted.transmission_grant, TransmissionGrant::RequestQueued.into_raw() as u8);

    test.submit_message(build_u_tx_demand_msg(talker, call_id));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, talker, CmcePduTypeDl::DTxGranted).expect("Expected D-TX GRANTED to the talker");
    let granted = DTxGranted::from_bitbuf(&mut sdu).unwrap();
    assert_eq!(granted.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(local_floor_granted_to(&msgs, TetraEntity::Umac, talker));
    assert!(local_floor_granted_to(&msgs, TetraEntity::Brew, talker));

    end_old_session_keeps_over(&mut test, gssi, brew_uuid);
}

/// Restore by the network talker: granted as before, and now also the old session is forgotten.
#[test]
fn test_moved_network_talker_restore_forgets_old_session() {
    debug::setup_logging_verbose();
    let gssi = 342;
    let talker = 3420001;
    let listener = 3420002;
    let brew_uuid = uuid::Uuid::from_u128(0x3420);
    let (mut test, call_id) = network_over_of_moved_talker(gssi, talker, listener, brew_uuid);

    test.submit_message(build_u_call_restore_msg(talker, call_id, gssi, true));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (restore, _) = d_call_restore_to(&msgs, talker).expect("Expected D-CALL RESTORE to the talker");
    assert_eq!(restore.transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
    assert!(local_floor_granted_to(&msgs, TetraEntity::Brew, talker));

    end_old_session_keeps_over(&mut test, gssi, brew_uuid);
}

fn site_area_members(issi: u32, groups: Option<Vec<u32>>) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::SiteAreaMembers { issi, groups }),
    }
}

/// Multi-cell, one location area: a radio registered on a sibling cell may be here without
/// registering (TS 100 392-2 clause 18.3.4.7.1a), so the switch makes it a listener of its groups
/// here; a network call to such a group runs here, and ends when the last such listener goes.
#[test]
fn test_site_area_member_makes_a_group_call_run_on_this_cell() {
    debug::setup_logging_verbose();
    let gssi = 230;
    let mut config = brew_test_config();
    config.site_linked = true;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );
    test.submit_message(site_area_members(MOVER, Some(vec![gssi])));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(
        msgs.iter()
            .any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::GroupListenersAvailable { gssi: g }) if *g == gssi)),
        "the group has a listener here now"
    );
    // A repeat changes nothing (counted once).
    test.submit_message(site_area_members(MOVER, Some(vec![gssi])));
    test.run_stack(Some(1));
    test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid: uuid::Uuid::new_v4(),
            source_issi: 2300010,
            dest_gssi: gssi,
            priority: 1,
        }),
    });
    test.run_stack(Some(5));
    test.dump_sinks();
    assert!(test.get_shared_config().state_read().active_call_ts.contains_key(&gssi), "call runs here");

    test.submit_message(site_area_members(MOVER, None));
    test.run_stack(Some(5));
    test.dump_sinks();
    assert!(
        !test.get_shared_config().state_read().active_call_ts.contains_key(&gssi),
        "no listener left: the call ends here"
    );
}

// ── Calls between two radios across a station change (extension `call-handoff`) ──────────────
// Both radios on one station, one goes to another: this station keeps the call and hands the
// leg that stays to the network, which gives it to the other station for the radio's U-CALL
// RESTORE (TS 100 392-2 clause 14.5.1.2.4). The other way round, a radio comes to the station of
// its peer: the station joins both legs into a local call.

const HANDOFF_A: u32 = 2_320_972;
const HANDOFF_B: u32 = 2_320_980;

fn handoff_test(extensions: &[&str]) -> ComponentTest {
    let mut test = ComponentTest::from_config(brew_test_config(), Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew, TetraEntity::Mm],
    );
    test.config.state_write().brew_server_extensions = extensions.iter().map(|e| e.to_string()).collect();
    test
}

const HANDOFF_EXTENSIONS: [&str; 4] = ["bts-info", "handover-v1", "call-restore", "call-handoff"];

/// A local individual call `HANDOFF_A` -> `HANDOFF_B`, connected; returns the call id and the
/// opened circuits as (carrier, ts).
fn local_call_for_handoff(test: &mut ComponentTest, duplex: bool) -> (u16, Vec<(u16, u8)>) {
    test.config.state_write().subscribers.register(HANDOFF_A);
    test.config.state_write().subscribers.register(HANDOFF_B);
    test.submit_message(build_individual_u_setup_msg_with_mode(HANDOFF_A, HANDOFF_B, duplex));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), HANDOFF_B);
    test.submit_message(build_u_connect_msg(HANDOFF_B, call_id, duplex));
    test.run_stack(Some(1));
    let mut slots = opened(&test.dump_sinks());
    slots.sort_unstable();
    slots.dedup();
    (call_id, slots)
}

fn opened(msgs: &[SapMsg]) -> Vec<(u16, u8)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(c)) => Some((c.carrier_num, c.ts)),
            _ => None,
        })
        .collect()
}

fn opened_circuits(msgs: &[SapMsg]) -> Vec<tetra_saps::control::call_control::Circuit> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(c)) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

fn ul_inactivity(test: &mut ComponentTest, (carrier_num, ts): (u16, u8)) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Umac,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout { carrier_num, ts }),
    });
}

fn handoff_request(issi: u32, peer: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::IndividualCallHandoffRequest { issi, peer }),
    }
}

fn handoff_answers(msgs: &[SapMsg]) -> Vec<(u32, u32, Option<tetra_saps::control::call_control::HandedOffCall>)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::IndividualCallHandoffAnswer { issi, peer, call }) => Some((*issi, *peer, *call)),
            _ => None,
        })
        .collect()
}

fn media_ready(msgs: &[SapMsg]) -> Vec<(uuid::Uuid, u8)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady { brew_uuid, ts, .. }) => Some((*brew_uuid, *ts)),
            _ => None,
        })
        .collect()
}

fn network_releases(msgs: &[SapMsg]) -> Vec<(uuid::Uuid, u8)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }) if m.dest == TetraEntity::Brew => {
                Some((*brew_uuid, *cause))
            }
            _ => None,
        })
        .collect()
}

fn released_slots(msgs: &[SapMsg]) -> Vec<(u16, u8)> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::CloseSlot { carrier_num, ts, .. }) => Some((*carrier_num, *ts)),
            _ => None,
        })
        .collect()
}

/// The slot of `issi`'s leg in a local duplex call: the one whose D-SETUP / D-CONNECT went to it.
fn duplex_slots_of(slots: &[(u16, u8)]) -> ((u16, u8), (u16, u8)) {
    assert_eq!(slots.len(), 2, "a local duplex call has two traffic channels: {slots:?}");
    (slots[0], slots[1])
}

/// Direction 1, duplex: the radio that leaves loses its uplink, the call is held instead of
/// released, and the network's handoff request bridges the leg that stays.
#[test]
fn test_handoff_holds_a_local_duplex_call_and_bridges_the_staying_leg() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    let (call_id, slots) = local_call_for_handoff(&mut test, true);
    let (first, second) = duplex_slots_of(&slots);

    // One leg goes silent (the radio is on its way to the other station): held, not released.
    for slot in [first, second] {
        ul_inactivity(&mut test, slot);
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();
        assert!(find_lcmc_req(&msgs, HANDOFF_A, CmcePduTypeDl::DRelease).is_none(), "held, not released");
        assert!(find_lcmc_req(&msgs, HANDOFF_B, CmcePduTypeDl::DRelease).is_none(), "held, not released");
    }

    // B restores on the other station; the network asks for the call.
    test.submit_message(handoff_request(HANDOFF_B, HANDOFF_A));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let answers = handoff_answers(&msgs);
    assert_eq!(answers.len(), 1);
    let call = answers[0].2.expect("the call is handed over");
    assert_eq!((call.calling_issi, call.simplex_duplex, call.floor_issi), (HANDOFF_A, true, None));
    // A stays on its own channel, its downlink now from the network.
    let reopened = opened_circuits(&msgs);
    assert_eq!(reopened.len(), 1, "only the staying leg is reopened: {reopened:?}");
    assert_eq!(reopened[0].dl_media_source, tetra_saps::control::call_control::CircuitDlMediaSource::SwMI);
    assert_eq!(reopened[0].peer_ts, None);
    let stay = (reopened[0].carrier_num, reopened[0].ts);
    assert_eq!(media_ready(&msgs), vec![(call.brew_uuid, stay.1)]);
    // B's channel here ends, without signalling to either radio.
    let gone = if stay == first { second } else { first };
    assert!(released_slots(&msgs).contains(&gone), "B's channel is closed: {:?}", released_slots(&msgs));
    for issi in [HANDOFF_A, HANDOFF_B] {
        assert!(find_lcmc_req(&msgs, issi, CmcePduTypeDl::DRelease).is_none());
    }

    // The hold is over: the call stays past the 8 s.
    test.run_stack(Some(8 * 72 + 20));
    let msgs = test.dump_sinks();
    assert!(find_lcmc_req(&msgs, HANDOFF_A, CmcePduTypeDl::DRelease).is_none(), "the bridged call runs on");
    let _ = call_id;
}

/// Without the server's `call-handoff` the old behaviour stays: the silent leg ends the call, and
/// a handoff request gets no call.
#[test]
fn test_without_call_handoff_a_silent_duplex_leg_releases_the_call() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&["bts-info", "handover-v1", "call-restore"]);
    let (call_id, slots) = local_call_for_handoff(&mut test, true);
    ul_inactivity(&mut test, slots[0]);
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, HANDOFF_A, CmcePduTypeDl::DRelease).expect("released");
    assert_eq!(DRelease::from_bitbuf(&mut sdu).unwrap().call_identifier, call_id);

    let mut test = handoff_test(&["bts-info", "handover-v1", "call-restore"]);
    local_call_for_handoff(&mut test, true);
    test.submit_message(handoff_request(HANDOFF_B, HANDOFF_A));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert_eq!(handoff_answers(&msgs), vec![(HANDOFF_B, HANDOFF_A, None)]);
    assert!(opened(&msgs).is_empty());
}

/// A held call nobody takes over ends after 8 s, as the silent leg would have ended it before.
#[test]
fn test_a_held_call_without_handoff_ends_after_the_hold() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    let (call_id, slots) = local_call_for_handoff(&mut test, true);
    ul_inactivity(&mut test, slots[1]);
    test.run_stack(Some(7 * 72));
    assert!(find_lcmc_req(&test.dump_sinks(), HANDOFF_A, CmcePduTypeDl::DRelease).is_none(), "still held at 7 s");
    test.run_stack(Some(2 * 72));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, HANDOFF_A, CmcePduTypeDl::DRelease).expect("released after the hold");
    let release = DRelease::from_bitbuf(&mut sdu).unwrap();
    assert_eq!((release.call_identifier, release.disconnect_cause), (call_id, DisconnectCause::ExpiryOfTimer));
}

/// No such call, the wrong peer, or one party asking for itself: no call.
#[test]
fn test_a_handoff_request_for_another_pair_gets_no_call() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    local_call_for_handoff(&mut test, true);
    for (issi, peer) in [(HANDOFF_B, 1234), (4321, HANDOFF_A), (HANDOFF_A, HANDOFF_A)] {
        test.submit_message(handoff_request(issi, peer));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();
        assert_eq!(handoff_answers(&msgs), vec![(issi, peer, None)]);
        assert!(opened(&msgs).is_empty() && media_ready(&msgs).is_empty());
    }
}

/// Direction 1, simplex: the talker leaves. Its permission ended at the break (clause
/// 14.5.1.2.4): the radio that stays is told the transmission ceased, and the call is handed over
/// with the floor free on the one channel both used.
#[test]
fn test_handoff_of_a_simplex_call_ends_the_leaving_talkers_floor() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    let (_call_id, slots) = local_call_for_handoff(&mut test, false);
    assert_eq!(slots.len(), 1, "a local simplex call shares one channel");
    // The caller has the floor after the connect (it pressed PTT to call).
    test.submit_message(handoff_request(HANDOFF_A, HANDOFF_B));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let call = handoff_answers(&msgs)[0].2.expect("handed over");
    assert_eq!((call.calling_issi, call.simplex_duplex, call.floor_issi), (HANDOFF_A, false, None));
    assert!(find_lcmc_req(&msgs, HANDOFF_B, CmcePduTypeDl::DTxCeased).is_some(), "B hears the talker is gone");
    let reopened = opened_circuits(&msgs);
    assert_eq!(reopened.iter().map(|c| (c.carrier_num, c.ts)).collect::<Vec<_>>(), slots);
    assert_eq!(reopened[0].dl_media_source, tetra_saps::control::call_control::CircuitDlMediaSource::SwMI);
    assert!(released_slots(&msgs).is_empty(), "the channel stays for B");
    assert_eq!(media_ready(&msgs), vec![(call.brew_uuid, slots[0].1)]);
}

/// A call over Brew from `remote` (another station) to `HANDOFF_B` here, connected.
fn network_call_for_join(test: &mut ComponentTest, remote: u32, duplex: bool) -> (u16, uuid::Uuid, (u16, u8)) {
    test.config.state_write().subscribers.register(HANDOFF_B);
    let brew_uuid = uuid::Uuid::parse_str("0b5f1d2e-9a47-4c1e-8c55-3e1f00c0ffee").unwrap();
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest {
            brew_uuid,
            call: NetworkCircuitCall {
                source_issi: remote,
                destination: HANDOFF_B,
                number: String::new(),
                priority: 0,
                service: 0,
                mode: 0,
                duplex: duplex as u8,
                method: 0,
                communication: 0,
                grant: TransmissionGrant::NotGranted.into_raw() as u8,
                permission: 0,
                timeout: 0,
                ownership: 0,
                queued: 0,
            },
        }),
    });
    test.run_stack(Some(1));
    let setup = test.dump_sinks();
    let call_id = first_d_setup_call_id(&setup, HANDOFF_B);
    let mut slots = opened(&setup);
    test.submit_message(build_u_connect_msg(HANDOFF_B, call_id, duplex));
    test.run_stack(Some(1));
    slots.extend(opened(&test.dump_sinks()));
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
            brew_uuid,
            grant: TransmissionGrant::Granted.into_raw() as u8,
            permission: 0,
        }),
    });
    test.run_stack(Some(1));
    slots.extend(opened(&test.dump_sinks()));
    (call_id, brew_uuid, *slots.first().expect("B's channel"))
}

/// U-CALL RESTORE of a point-to-point call from `issi` with `peer` (clause 14.5.1.2.4).
fn p2p_restore(issi: u32, call_identifier: u16, peer: u32, other_may_transmit: bool) -> SapMsg {
    let pdu = UCallRestore {
        call_identifier,
        request_to_transmit_send_data: other_may_transmit,
        other_party_type_identifier: 1,
        other_party_short_number_address: None,
        other_party_ssi: Some(peer as u64),
        other_party_extension: None,
        basic_service_information: Some(BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: CommunicationType::P2p,
            slots_per_frame: None,
            speech_service: Some(0),
        }),
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);
    lcmc_ind(issi, sdu)
}

/// Direction 2, duplex: the radio on the other station comes here, to its peer, and restores the
/// call. Both legs are joined into a local call: the radio gets its own channel cross-routed with
/// B's, and the network call ends with "SwMI requested disconnection".
#[test]
fn test_a_radio_coming_to_its_peer_is_joined_into_a_local_duplex_call() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    let (call_id, brew_uuid, b_slot) = network_call_for_join(&mut test, HANDOFF_A, true);
    test.config.state_write().subscribers.register(HANDOFF_A);

    test.submit_message(p2p_restore(HANDOFF_A, 77, HANDOFF_B, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(restore_requests(&msgs).is_empty(), "nothing to ask the network");
    let (restore, chan) = d_call_restore_to(&msgs, HANDOFF_A).expect("D-CALL RESTORE");
    assert_eq!(restore.call_identifier, 77);
    assert_eq!(restore.new_call_identifier, Some(call_id as u64), "the radio adopts this station's call id");
    assert_eq!(restore.transmission_grant, TransmissionGrant::Granted.into_raw() as u8, "duplex: always granted");
    let (_, a_ts) = chan.expect("a channel of its own");
    assert_ne!(a_ts as u8, b_slot.1, "duplex: its own channel");
    // Both channels cross-routed locally.
    let circuits = opened_circuits(&msgs);
    let loopback = tetra_saps::control::call_control::CircuitDlMediaSource::LocalLoopback;
    let a = circuits.iter().find(|c| c.ts == a_ts as u8).expect("A's channel opened");
    let b = circuits.iter().find(|c| (c.carrier_num, c.ts) == b_slot).expect("B's channel reopened");
    assert_eq!((a.dl_media_source, a.peer_ts), (loopback, Some(b.ts)));
    assert_eq!((b.dl_media_source, b.peer_ts), (loopback, Some(a.ts)));
    assert_eq!(network_releases(&msgs), vec![(brew_uuid, DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8)]);
    assert!(find_lcmc_req(&msgs, HANDOFF_B, CmcePduTypeDl::DRelease).is_none(), "B notices nothing");
}

/// Direction 2, simplex: both legs share B's channel; the arriving radio that asks to talk gets
/// the floor there.
#[test]
fn test_a_radio_coming_to_its_peer_is_joined_into_a_local_simplex_call() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&HANDOFF_EXTENSIONS);
    let (call_id, brew_uuid, b_slot) = network_call_for_join(&mut test, HANDOFF_A, false);
    test.config.state_write().subscribers.register(HANDOFF_A);

    test.submit_message(p2p_restore(HANDOFF_A, 77, HANDOFF_B, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (restore, chan) = d_call_restore_to(&msgs, HANDOFF_A).expect("D-CALL RESTORE");
    assert_eq!(restore.new_call_identifier, Some(call_id as u64));
    assert_eq!(chan.map(|(_, ts)| ts as u8), Some(b_slot.1), "simplex: B's channel");
    let circuits = opened_circuits(&msgs);
    assert_eq!(circuits.len(), 1);
    assert_eq!(circuits[0].dl_media_source, tetra_saps::control::call_control::CircuitDlMediaSource::LocalLoopback);
    assert_eq!(network_releases(&msgs), vec![(brew_uuid, DisconnectCause::SwmiRequestedDisconnection.into_raw() as u8)]);
}

/// Without `call-handoff` the arriving radio's restore goes to the network as before.
#[test]
fn test_without_call_handoff_a_radio_coming_to_its_peer_asks_the_network() {
    debug::setup_logging_verbose();
    let mut test = handoff_test(&["bts-info", "handover-v1", "call-restore"]);
    network_call_for_join(&mut test, HANDOFF_A, true);
    test.config.state_write().subscribers.register(HANDOFF_A);
    test.submit_message(p2p_restore(HANDOFF_A, 77, HANDOFF_B, false));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert_eq!(restore_requests(&msgs), vec![(HANDOFF_A, HANDOFF_B)]);
    assert!(network_releases(&msgs).is_empty());
}

// ── Group access (Brew server `group-access` rules) ─────────────────────────────────────────────

fn limit_group(test: &mut ComponentTest, rule: tetra_config::bluestation::BrewGroupAccess) {
    test.config.state_write().brew_group_access.insert(rule.gssi, rule);
}

fn group_access_test() -> ComponentTest {
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);
    test
}

fn d_release_cause_to(msgs: &[SapMsg], issi: u32) -> Option<DisconnectCause> {
    let (mut sdu, _) = find_lcmc_req(msgs, issi, CmcePduTypeDl::DRelease)?;
    Some(DRelease::from_bitbuf(&mut sdu).expect("Failed to parse DRelease").disconnect_cause)
}

/// A radio the group does not admit, or a listener-only one, cannot start a call on it: the
/// U-SETUP is answered with D-RELEASE (clause 14.5.1.1; cause value "Requested service not
/// available", table 14.55, chosen by us) and no call is set up. An emergency call passes.
#[test]
fn test_group_access_refuses_setup_from_radios_that_may_not_talk() {
    debug::setup_logging_verbose();
    use tetra_config::bluestation::BrewGroupAccess;
    const OUTSIDER: u32 = TEST_ISSI + 1;
    const LISTENER: u32 = TEST_ISSI + 2;
    let mut test = group_access_test();
    for issi in [TEST_ISSI, OUTSIDER, LISTENER] {
        register_subscriber(&mut test, issi, TEST_GSSI);
    }
    limit_group(&mut test, BrewGroupAccess { gssi: TEST_GSSI, block: true, issis: vec![OUTSIDER], rx_only: true, rx_only_issis: vec![LISTENER] });

    for issi in [OUTSIDER, LISTENER] {
        test.submit_message(build_u_setup_msg(issi, TEST_GSSI));
        test.run_stack(Some(1));
        let msgs = test.dump_sinks();
        assert_eq!(d_release_cause_to(&msgs, issi), Some(DisconnectCause::RequestedServiceNotAvailable), "{issi}");
        assert_eq!(count_d_setups(&msgs), 0, "no call for {issi}");
    }

    // An emergency call is never limited.
    test.submit_message(build_u_setup_msg_prio(LISTENER, TEST_GSSI, 15));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    assert!(d_release_cause_to(&msgs, LISTENER).is_none());
    assert!(find_lcmc_req(&msgs, TEST_GSSI, CmcePduTypeDl::DSetup).is_some(), "emergency call set up");
}

/// A listener-only radio joining a running call listens: its individually addressed D-CONNECT
/// says "not allowed to request for transmission" (clause 14.8.43, table 14.81) and does not
/// grant it the floor, even in hangtime. A radio the group does not admit is refused.
#[test]
fn test_group_access_listener_joins_running_call_without_the_floor() {
    debug::setup_logging_verbose();
    use tetra_config::bluestation::BrewGroupAccess;
    const LISTENER: u32 = TEST_ISSI + 1;
    const TALKER: u32 = TEST_ISSI + 2;
    const OUTSIDER: u32 = TEST_ISSI + 3;
    let mut test = group_access_test();
    for issi in [TEST_ISSI, LISTENER, TALKER, OUTSIDER] {
        register_subscriber(&mut test, issi, TEST_GSSI);
    }
    limit_group(&mut test, BrewGroupAccess { gssi: TEST_GSSI, block: true, issis: vec![OUTSIDER], rx_only: true, rx_only_issis: vec![LISTENER] });

    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);

    let joined = |test: &mut ComponentTest, issi: u32| -> DConnect {
        test.submit_message(build_u_setup_msg(issi, TEST_GSSI));
        test.run_stack(Some(1));
        let (mut sdu, _) = find_lcmc_req(&test.dump_sinks(), issi, CmcePduTypeDl::DConnect).expect("D-CONNECT to the joiner");
        DConnect::from_bitbuf(&mut sdu).expect("Failed to parse DConnect")
    };
    let listener = joined(&mut test, LISTENER);
    assert_eq!(listener.call_identifier, call_id);
    assert_eq!(listener.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    assert!(listener.transmission_request_permission, "not allowed to request for transmission");

    test.submit_message(build_u_setup_msg(OUTSIDER, TEST_GSSI));
    test.run_stack(Some(1));
    assert_eq!(d_release_cause_to(&test.dump_sinks(), OUTSIDER), Some(DisconnectCause::RequestedServiceNotAvailable));

    // Hangtime: a talker joining gets the floor as before, a listener does not.
    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    test.dump_sinks();
    let listener = joined(&mut test, LISTENER);
    assert_eq!(listener.transmission_grant, TransmissionGrant::NotGranted);
    assert!(listener.transmission_request_permission);
    let talker = joined(&mut test, TALKER);
    assert_eq!(talker.transmission_grant, TransmissionGrant::Granted);
    assert!(!talker.transmission_request_permission);
}

/// A U-TX DEMAND from a radio that may not talk is answered "transmission not granted" to that
/// radio alone and the call goes on (clause 14.5.2.2.1 b), table 14.80); nobody else hears of
/// it. With the rules lifted the same radio gets the floor.
#[test]
fn test_group_access_tx_demand_from_listener_is_not_granted() {
    debug::setup_logging_verbose();
    use tetra_config::bluestation::BrewGroupAccess;
    const LISTENER: u32 = TEST_ISSI + 1;
    let mut test = group_access_test();
    for issi in [TEST_ISSI, LISTENER] {
        register_subscriber(&mut test, issi, TEST_GSSI);
    }
    // RX only without a list: every radio listens only. The call was set up before the rule.
    test.submit_message(build_u_setup_msg(TEST_ISSI, TEST_GSSI));
    test.run_stack(Some(1));
    let call_id = first_d_setup_call_id(&test.dump_sinks(), TEST_GSSI);
    test.submit_message(build_u_tx_ceased_msg(TEST_ISSI, call_id));
    test.run_stack(Some(1));
    test.dump_sinks();
    limit_group(&mut test, BrewGroupAccess { gssi: TEST_GSSI, rx_only: true, ..Default::default() });

    test.submit_message(build_u_tx_demand_msg(LISTENER, call_id));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();
    let (mut sdu, _) = find_lcmc_req(&msgs, LISTENER, CmcePduTypeDl::DTxGranted).expect("D-TX GRANTED to the radio");
    let answer = DTxGranted::from_bitbuf(&mut sdu).expect("Failed to parse DTxGranted");
    assert_eq!(answer.call_identifier, call_id);
    assert_eq!(answer.transmission_grant, TransmissionGrant::NotGranted.into_raw() as u8);
    assert!(find_lcmc_req(&msgs, TEST_GSSI, CmcePduTypeDl::DTxGranted).is_none(), "no group grant");
    assert!(find_lcmc_req(&msgs, TEST_GSSI, CmcePduTypeDl::DRelease).is_none(), "the call goes on");
    assert!(!msgs.iter().any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::FloorGranted { .. }))));

    test.config.state_write().brew_group_access.clear();
    test.submit_message(build_u_tx_demand_msg(LISTENER, call_id));
    test.run_stack(Some(1));
    let (mut sdu, _) = find_lcmc_req(&test.dump_sinks(), LISTENER, CmcePduTypeDl::DTxGranted).expect("D-TX GRANTED");
    assert_eq!(DTxGranted::from_bitbuf(&mut sdu).unwrap().transmission_grant, TransmissionGrant::Granted.into_raw() as u8);
}
