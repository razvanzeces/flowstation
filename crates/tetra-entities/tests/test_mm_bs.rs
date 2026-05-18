mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_pdus::mm::enums::energy_saving_mode::EnergySavingMode;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use crate::common::ComponentTest;

const TEST_ISSI: u32 = 2260082;

fn make_location_update_msg(issi: u32, handle: u32, location_update_type: LocationUpdateType) -> SapMsg {
    let pdu = ULocationUpdateDemand {
        location_update_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: None,
        address_extension: None,
        group_identity_location_demand: None,
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(16);
    pdu.to_bitbuf(&mut sdu).unwrap();
    sdu.seek(0);

    SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle,
            received_address: TetraAddress::issi(issi),
        }),
    }
}

fn lmm_downlink_pdu_types(msgs: &[SapMsg]) -> Vec<MmPduTypeDl> {
    msgs.iter()
        .filter_map(|msg| {
            let SapMsgInner::LmmMleUnitdataReq(ref prim) = msg.msg else {
                return None;
            };
            let mut sdu = BitBuffer::from_bitstr(&prim.sdu.to_bitstr());
            Some(MmPduTypeDl::try_from(sdu.read_field(4, "pdu_type").unwrap()).unwrap())
        })
        .collect()
}

fn first_location_update_accept(msgs: &[SapMsg]) -> DLocationUpdateAccept {
    let response = msgs
        .iter()
        .find_map(|msg| {
            if let SapMsgInner::LmmMleUnitdataReq(ref prim) = msg.msg {
                Some(prim)
            } else {
                None
            }
        })
        .expect("expected D-LOCATION UPDATE ACCEPT");
    let mut resp_sdu = BitBuffer::from_bitstr(&response.sdu.to_bitstr());
    DLocationUpdateAccept::from_bitbuf(&mut resp_sdu).expect("failed parsing D-LOCATION UPDATE ACCEPT")
}

#[test]
fn test_u_mm_status_energy_saving() {
    // Motorola requesting power management (ChangeOfEnergySavingModeRequest)
    debug::setup_logging_verbose();
    let test_vec1 = "00110000010010";
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec1 = dltime_vec1.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim1 = LmmMleUnitdataInd {
        sdu: BitBuffer::from_bitstr(test_vec1),
        handle: 0,
        received_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2040814,
        },
    };
    let test_sapmsg1 = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(test_prim1),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Mm];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Mle];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg1);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // FlowStation explicitly allocates StayAlive until addressed downlink EE
    // scheduling is complete.
    assert_eq!(sink_msgs.len(), 1);

    // Parse the response and verify it's a D-MM-STATUS
    let SapMsgInner::LmmMleUnitdataReq(ref resp_prim) = sink_msgs[0].msg else {
        panic!("Expected LmmMleUnitdataReq");
    };
    let mut resp_sdu = BitBuffer::from_bitstr(&resp_prim.sdu.to_bitstr());
    let resp_pdu = DMmStatus::from_bitbuf(&mut resp_sdu).expect("Failed parsing D-MM-STATUS response");
    assert_eq!(
        resp_pdu.status_downlink,
        tetra_pdus::mm::enums::status_downlink::StatusDownlink::ChangeOfEnergySavingModeResponse
    );
    let esi = resp_pdu.energy_saving_information.expect("expected energy saving information");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert!(esi.frame_number.is_none());
    assert!(esi.multiframe_number.is_none());
}

#[test]
fn test_itsi_attach_emits_only_location_update_accept_no_automatic_command() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.submit_message(make_location_update_msg(TEST_ISSI, 17, LocationUpdateType::ItsiAttach));
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(lmm_downlink_pdu_types(&sink_msgs), vec![MmPduTypeDl::DLocationUpdateAccept]);

    let resp_pdu = first_location_update_accept(&sink_msgs);
    assert_eq!(resp_pdu.location_update_accept_type, LocationUpdateType::ItsiAttach);
    assert_eq!(resp_pdu.ssi, Some(TEST_ISSI as u64));
}

#[test]
fn test_force_location_update_keeps_explicit_command_but_no_second_automatic_command() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::MmForceLocationUpdate {
            issi: TEST_ISSI,
            handle: 17,
        },
    });
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert_eq!(lmm_downlink_pdu_types(&command_msgs), vec![MmPduTypeDl::DLocationUpdateCommand]);

    test.submit_message(make_location_update_msg(TEST_ISSI, 17, LocationUpdateType::ItsiAttach));
    test.run_stack(Some(1));
    let response_msgs = test.dump_sinks();
    assert_eq!(lmm_downlink_pdu_types(&response_msgs), vec![MmPduTypeDl::DLocationUpdateAccept]);
}

#[test]
fn test_brew_reconnected_emits_location_update_command_for_registered_ms() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.submit_message(make_location_update_msg(TEST_ISSI, 17, LocationUpdateType::ItsiAttach));
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::BrewReconnected,
    });
    test.run_stack(Some(1));
    let command_msgs = test.dump_sinks();
    assert_eq!(lmm_downlink_pdu_types(&command_msgs), vec![MmPduTypeDl::DLocationUpdateCommand]);
}

#[test]
fn test_location_update_accept_preserves_request_type_when_periodic_enabled() {
    debug::setup_logging_verbose();

    let lud_with_eg1 =
        "0010000001100010010010100000010000010010001001100000111000001110000000010010000000101000000000000000000000001101000";
    let test_prim = LmmMleUnitdataInd {
        sdu: BitBuffer::from_bitstr(lud_with_eg1),
        handle: 0,
        received_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2260616,
        },
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(test_prim),
    };

    let dltime = TdmaTime::default().add_timeslots(2);
    let mut cfg = ComponentTest::get_default_test_config(StackMode::Bs);
    cfg.cell.periodic_registration_secs = 3600;
    let mut test = ComponentTest::from_config(cfg, Some(dltime));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle]);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let response = sink_msgs
        .iter()
        .find_map(|msg| {
            if let SapMsgInner::LmmMleUnitdataReq(ref prim) = msg.msg {
                Some(prim)
            } else {
                None
            }
        })
        .expect("expected D-LOCATION UPDATE ACCEPT");

    let mut resp_sdu = BitBuffer::from_bitstr(&response.sdu.to_bitstr());
    let resp_pdu = DLocationUpdateAccept::from_bitbuf(&mut resp_sdu).expect("failed parsing D-LOCATION UPDATE ACCEPT");
    assert_eq!(resp_pdu.location_update_accept_type, LocationUpdateType::RoamingLocationUpdating);
}

#[test]
fn test_location_update_energy_saving_request_is_forced_to_stay_alive_until_ee_scheduler_exists() {
    debug::setup_logging_verbose();

    let lud_with_eg1 =
        "0010000001100010010010100000010000010010001001100000111000001110000000010010000000101000000000000000000000001101000";
    let test_prim = LmmMleUnitdataInd {
        sdu: BitBuffer::from_bitstr(lud_with_eg1),
        handle: 0,
        received_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2260616,
        },
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(test_prim),
    };

    let dltime = TdmaTime::default().add_timeslots(2);
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Umac]);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    let response = sink_msgs
        .iter()
        .find_map(|msg| {
            if let SapMsgInner::LmmMleUnitdataReq(ref prim) = msg.msg {
                Some(prim)
            } else {
                None
            }
        })
        .expect("expected D-LOCATION UPDATE ACCEPT");

    let mut resp_sdu = BitBuffer::from_bitstr(&response.sdu.to_bitstr());
    let resp_pdu = DLocationUpdateAccept::from_bitbuf(&mut resp_sdu).expect("failed parsing D-LOCATION UPDATE ACCEPT");
    let esi = resp_pdu
        .energy_saving_information
        .expect("expected explicit StayAlive while real EE scheduling is disabled");
    assert_eq!(esi.energy_saving_mode, EnergySavingMode::StayAlive);
    assert!(esi.frame_number.is_none());
    assert!(esi.multiframe_number.is_none());

    let umac_update = sink_msgs
        .iter()
        .find_map(|msg| {
            if let SapMsgInner::MmEnergySavingUpdate { issi, mode, start_time } = msg.msg {
                Some((issi, mode, start_time))
            } else {
                None
            }
        })
        .expect("expected MM energy-saving update to UMAC");

    assert_eq!(umac_update.0, 2260616);
    assert_eq!(umac_update.1, EnergySavingMode::StayAlive as u8);
    assert!(umac_update.2.is_none());
}
