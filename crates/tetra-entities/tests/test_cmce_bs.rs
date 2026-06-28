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
    d_connect::DConnect, d_connect_acknowledge::DConnectAcknowledge, d_release::DRelease, d_setup::DSetup, d_tx_ceased::DTxCeased,
    d_tx_granted::DTxGranted, u_connect::UConnect, u_disconnect::UDisconnect, u_setup::USetup, u_tx_ceased::UTxCeased,
    u_tx_demand::UTxDemand,
};
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::call_control::{CallControl, CircuitDlMediaSource, NetworkCircuitCall};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use crate::common::ComponentTest;

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
fn test_local_echo_999_bypasses_registration_and_brew() {
    debug::setup_logging_verbose();

    const LOCAL_ECHO_ISSI: u32 = 999;
    let calling_issi = 1000001;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    // The virtual echo party is deliberately not registered.
    test.submit_message(build_individual_u_setup_msg_with_mode(calling_issi, LOCAL_ECHO_ISSI, true));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let (mut connect_sdu, connect_alloc) =
        find_lcmc_req(&msgs, calling_issi, CmcePduTypeDl::DConnect).expect("local echo must answer the caller immediately");
    let d_connect = DConnect::from_bitbuf(&mut connect_sdu).expect("local echo D-CONNECT must parse");
    assert_eq!(d_connect.transmission_grant, TransmissionGrant::Granted);
    assert_eq!(connect_alloc, Some(UlDlAssignment::Both));

    assert!(
        find_lcmc_req(&msgs, LOCAL_ECHO_ISSI, CmcePduTypeDl::DSetup).is_none(),
        "the virtual echo party must not receive a radio-side D-SETUP"
    );
    assert!(
        !msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { .. })
        )),
        "ISSI 999 must never be routed to Brew"
    );

    let open_circuits: Vec<_> = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(circuit)) => Some(circuit),
            _ => None,
        })
        .collect();
    assert_eq!(open_circuits.len(), 1, "local echo must use exactly one traffic circuit");
    assert_eq!(open_circuits[0].direction, Direction::Both);
    assert_eq!(open_circuits[0].dl_media_source, CircuitDlMediaSource::LocalLoopback);
    assert_eq!(open_circuits[0].peer_ts, None);
}

#[test]
fn test_local_echo_999_release_skips_virtual_radio_leg() {
    debug::setup_logging_verbose();

    const LOCAL_ECHO_ISSI: u32 = 999;
    let calling_issi = 1000001;
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );

    test.submit_message(build_individual_u_setup_msg_with_mode(calling_issi, LOCAL_ECHO_ISSI, true));
    test.run_stack(Some(1));
    let setup_msgs = test.dump_sinks();
    let (mut connect_sdu, _) = find_lcmc_req(&setup_msgs, calling_issi, CmcePduTypeDl::DConnect).expect("expected local echo D-CONNECT");
    let call_id = DConnect::from_bitbuf(&mut connect_sdu)
        .expect("local echo D-CONNECT must parse")
        .call_identifier;

    test.submit_message(build_u_disconnect_msg(calling_issi, call_id));
    test.run_stack(Some(1));
    let release_msgs = test.dump_sinks();

    assert!(
        find_lcmc_req(&release_msgs, calling_issi, CmcePduTypeDl::DRelease).is_some(),
        "the caller must receive D-RELEASE"
    );
    assert!(
        find_lcmc_req(&release_msgs, LOCAL_ECHO_ISSI, CmcePduTypeDl::DRelease).is_none(),
        "the virtual echo party must not receive a radio-side D-RELEASE"
    );
    assert!(
        release_msgs
            .iter()
            .any(|msg| matches!(&msg.msg, SapMsgInner::CmceCallControl(CallControl::CloseSlot { .. }))),
        "the local echo traffic circuit must be closed"
    );
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
        pbx_gateway_issis: None,
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

    // Collect initial output — should contain D-SETUP (initial send with no tracked receipt)
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
    // A fourth (emergency) talkgroup with its own listener — no free slot remains for it.
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

    // Incoming EMERGENCY group call (priority 15) on the fourth GSSI — must pre-empt to proceed.
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

    // Ordinary-priority (priority 0) group call into a full cell — must NOT pre-empt anything.
    test.submit_message(build_u_setup_msg_prio(3_000_099, extra_gssi, 0));
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    let call_ended = msgs
        .iter()
        .filter(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::CallEnded { .. })))
        .count();
    assert_eq!(call_ended, 0, "an ordinary-priority call must not pre-empt any active call");
    // The three original calls are untouched — still no free slot.
    assert_eq!(test.config.state_read().timeslot_alloc.free_count(), 0);
}

// Energy-Economy D-SETUP gate (clause 16.7): individual-call setup resends to a sleeping EE MS
// are held for the MS's downlink monitoring window, with a bounded fallback (EE_DSETUP_FALLBACK_TS
// ≈ 423 timeslots / ~105 frames) to the historical blind resend. The empirically-observed resend
// cadence (initial + setup-retry) fires several individual D-SETUPs to the called MS within the
// fallback window, which the tests below rely on.

/// A sleeping EE MS (monitoring window closed for the whole sub-fallback run) must NOT receive
/// any D-SETUP resend — they are held for its window.
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

    // ~100 frames (400 ts) — comfortably under the ~423 ts fallback, so any resend here is held.
    test.run_stack(Some(400));
    let held = count_individual_dsetup_to(&test.dump_sinks(), called);
    assert_eq!(
        held, 0,
        "D-SETUP resends to an asleep EE MS must be held while its monitoring window is closed"
    );
}

/// A non-EE MS (absent from the published window map) is always reachable — the gate must not
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
/// resends must resume once the setup has been pending longer than the fallback — so call setup
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

// ─── FH FIX 2 guard: group-addressed PDUs must use unacknowledged LLC ──────────────────────────
//
// D-SETUP / D-RELEASE to a GSSI have no single peer to acknowledge, so they must go out as
// unacknowledged BL-UDATA (`Layer2Service::Unacknowledged`), not the acknowledged default. MLE
// routes Unacknowledged → TlaTlUnitdataReqBl (BL-UDATA) and everything else → TlaTlDataReqBl
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

    // The call owner (the original caller) disconnects → release_group_call sends the group
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

// ─── FH FIX 1 guard: call-lifecycle telemetry must reach the dashboard sink ────────────────────

#[test]
fn test_group_call_emits_started_and_ended_telemetry() {
    use tetra_entities::cmce::cmce_bs::CmceBs;
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};

    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    // Build the sinks but NOT the CMCE — we register a telemetry-wired CmceBs ourselves below.
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
/// with D-CMCE-FUNCTION-NOT-SUPPORTED (ETSI EN 300 392-2 §14.7.2.5). The CMCE rewrite
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

// ── SS-DGNA over CMCE D-FACILITY (TS 100 392-12-22 V1.5.1; EN 300 392-9 V1.7.1) ────────────────
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

    /// Build a MM(+control)/CMCE stack with the Mle sink, register the terminal, drive a DGNA, and
    /// return everything captured at the sinks.
    fn run_dgna(attach: bool) -> (ComponentTest, Vec<SapMsg>) {
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
                attach: true,
            });
            test.run_stack(Some(6));
            let _ = test.dump_sinks();
        }

        dispatcher.send(ControlCommand::Dgna {
            issi: DGNA_ISSI,
            gssi: DGNA_GSSI,
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
        let (test, msgs) = run_dgna(true);

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

    /// Operator DGNA deassign emits a D-FACILITY{DEASSIGN} naming the GSSI and removes the
    /// affiliation.
    #[test]
    fn test_dgna_deassign_emits_d_facility_deassign() {
        debug::setup_logging_verbose();
        let (test, msgs) = run_dgna(false);

        let (addr_ssi, facility) = find_d_facility(&msgs)
            .unwrap_or_else(|| panic!("expected a D-FACILITY after DGNA deassign, got {} msgs", msgs.len()));
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

        // No registration first — MM must drop the command, so CMCE never emits a D-FACILITY.
        dispatcher.send(ControlCommand::Dgna {
            issi: 9_999_002,
            gssi: DGNA_GSSI,
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
        test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew]);

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
