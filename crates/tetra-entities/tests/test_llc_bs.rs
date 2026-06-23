mod common;

use common::ComponentTest;
use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_pdus::cmce::pdus::u_connect::UConnect;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::{TlaTlDataIndBl, TlaTlDataReqBl};
use tetra_saps::tma::{TmaUnitdataInd, TmaUnitdataReq};

const MAIN_CARRIER: u16 = 1521;

#[test]
fn test_udata_with_broken_mm_payload() {
    // INCOMPLETE VECTOR replace with something more meaningful
    debug::setup_logging_verbose();

    // FIXME make proper vec here that can be passed onwards
    let test_vec = "00011001011100111000000011111100001000010000000000000000"; // INCOMPLETE
    let dltime_vec = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    let test_prim = TmaUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: Some(BitBuffer::from_bitstr(test_vec)),
        main_address: TetraAddress {
            ssi: 2065022,
            ssi_type: SsiType::Issi,
        },
        scrambling_code: 864282631,
        link_id: 0,
        endpoint_id: 0,
        new_endpoint_id: None,
        css_endpoint_id: None,
        air_interface_encryption: 0,
        chan_change_response_req: false,
        chan_change_handle: None,
        chan_info: None,
    };
    let test_sapmsg = SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(test_prim),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec));
    let components = vec![TetraEntity::Llc, TetraEntity::Mle, TetraEntity::Mm];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Umac];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // Evaluate results
    assert_eq!(sink_msgs.len(), 1);
    tracing::warn!("Validation of result not implemented");
}

#[test]
fn test_bl_ack_with_piggyback_cmce_payload_is_forwarded() {
    debug::setup_logging_verbose();

    let mut pdu = BitBuffer::new_autoexpand(32);
    BlAck { has_fcs: false, nr: 1 }.to_bitbuf(&mut pdu);
    pdu.write_bits(MleProtocolDiscriminator::Cmce.into_raw(), 3);
    UConnect {
        call_identifier: 5,
        hook_method_selection: true,
        simplex_duplex_selection: true,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    }
    .to_bitbuf(&mut pdu)
    .expect("failed to serialize U-CONNECT");
    pdu.seek(0);

    let test_prim = TmaUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: Some(pdu),
        main_address: TetraAddress::new(2200699, SsiType::Issi),
        scrambling_code: 0,
        link_id: 3,
        endpoint_id: 0,
        new_endpoint_id: None,
        css_endpoint_id: None,
        air_interface_encryption: 0,
        chan_change_response_req: false,
        chan_change_handle: None,
        chan_info: None,
    };
    let test_sapmsg = SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(test_prim),
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Mle]);

    test.submit_message(test_sapmsg);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
        link_id,
        tl_sdu: Some(mut sdu),
        ..
    }) = sink_msgs[0].msg.clone()
    else {
        panic!("expected TlaTlDataIndBl with piggyback payload");
    };
    assert_eq!(link_id, 0);
    assert_eq!(sdu.read_bits(3), Some(MleProtocolDiscriminator::Cmce.into_raw()));
    let parsed = UConnect::from_bitbuf(&mut sdu).expect("forwarded U-CONNECT should parse");
    assert_eq!(parsed.call_identifier, 5);
    assert!(parsed.hook_method_selection);
    assert!(parsed.simplex_duplex_selection);
}

#[test]
fn test_stealing_bl_udata_fallback_uses_unlinked_llc_context() {
    debug::setup_logging_verbose();

    let mut tl_sdu = BitBuffer::new_autoexpand(8);
    tl_sdu.write_bits(0b1010_1100, 8);
    tl_sdu.seek(0);

    let req = TlaTlDataReqBl {
        main_address: TetraAddress::new(2200699, SsiType::Issi),
        link_id: 3,
        endpoint_id: 0,
        tl_sdu,
        stealing_permission: true,
        subscriber_class: 0,
        fcs_flag: false,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_class_info: None,
        req_handle: 0,
        graceful_degradation: None,
        chan_alloc: None,
        tx_reporter: None,
    };

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac]);

    test.submit_message(SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlDataReqBl(req),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TmaUnitdataReq(TmaUnitdataReq { carrier_num, link_id, .. }) = &sink_msgs[0].msg else {
        panic!("expected TMA-UNITDATA request");
    };
    assert_eq!(*carrier_num, Some(MAIN_CARRIER));
    assert_eq!(*link_id, 0);
}
