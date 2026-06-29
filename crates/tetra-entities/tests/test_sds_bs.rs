mod common;

use std::time::Duration;

use tetra_config::bluestation::{CfgBrew, CfgSdsCommandControl, CfgSdsCommandEntry, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::enums::pre_coded_status::PreCodedStatus;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
use tetra_pdus::cmce::pdus::u_sds_data::USdsData;
use tetra_pdus::cmce::pdus::u_setup::USetup;
use tetra_pdus::cmce::pdus::u_status::UStatus;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::control::enums::sds_user_data::SdsUserData;
use tetra_saps::control::sds::CmceSdsData;
use tetra_saps::lcmc::{LcmcMleUnitdataInd, LcmcMleUnitdataReq};
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::net_control::{ControlCommand, make_control_link};
use tetra_entities::tpg2200::{build_tpg2200_callout_payload, default_tpg2200_ric};
use tetra_pdus::cmce::pdus::d_sds_data::DSdsData;

use crate::common::ComponentTest;

/// Helper: register a subscriber ISSI in the StackState subscriber registry
fn register_subscriber(test: &mut ComponentTest, issi: u32) {
    test.config.state_write().subscribers.register(issi);
}

/// Helper: affiliate a subscriber with a GSSI in the StackState subscriber registry
fn affiliate_subscriber(test: &mut ComponentTest, issi: u32, gssi: u32) {
    test.config.state_write().subscribers.affiliate(issi, gssi);
}

/// Helper: build a U-SDS-DATA message from a source ISSI to a dest SSI with 16-bit payload
fn build_u_sds_data_msg(source_issi: u32, dest_ssi: u32, payload: u16) -> SapMsg {
    let u_sds = USdsData {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: None,
        user_defined_data: SdsUserData::Type1(payload),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_sds.to_bitbuf(&mut sdu).expect("Failed to serialize U-SDS-DATA");
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
            received_tetra_address: TetraAddress::new(source_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Count D-SDS-DATA messages (LcmcMleUnitdataReq to Mle) in sink output
fn count_d_sds_data(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|m| m.dest == TetraEntity::Mle && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
        .count()
}

/// Count CmceSdsData messages to Brew in sink output
fn count_brew_sds(msgs: &[SapMsg]) -> usize {
    msgs.iter()
        .filter(|m| m.dest == TetraEntity::Brew && matches!(&m.msg, SapMsgInner::CmceSdsData(_)))
        .count()
}

#[test]
fn test_sds_local_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register dest ISSI in StackState
    register_subscriber(&mut test, 2000001);

    // Send U-SDS-DATA from source ISSI to registered dest ISSI
    let msg = build_u_sds_data_msg(1000001, 2000001, 0xABCD);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected 1 D-SDS-DATA at Mle sink for local delivery");

    // Verify the address is ISSI
    for m in &sink_msgs {
        if m.dest == TetraEntity::Mle
            && let SapMsgInner::LcmcMleUnitdataReq(ref prim) = m.msg
        {
            assert_eq!(prim.main_address.ssi, 2000001);
            assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
        }
    }
}

/// FH-BUG-052: a TPG2200 / DAPNET Call-Out arrives as ControlCommand::SendRawSdsType4 — an
/// already-built Type-4 SDU whose first byte is its own protocol id (0xC3). CMCE used to have no
/// match arm for it and silently dropped it. It must now be delivered verbatim, with NO SDS-TL
/// [0x82,..] wrap, as a D-SDS-DATA to the local destination.
#[test]
fn test_raw_sds_type4_from_control_delivered_verbatim() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Brew]);

    // CMCE wired to a control endpoint so we can inject the raw-SDS command (mirrors test_mm_bs).
    let (dispatcher, endpoint) = make_control_link();
    let cmce = CmceBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(cmce);

    const DEST: u32 = 2628191;
    register_subscriber(&mut test, DEST);

    let payload = build_tpg2200_callout_payload(default_tpg2200_ric(), 0x11, 0x0F, "ALARM");
    assert_eq!(payload[0], 0xC3, "sanity: a TPG2200 Call-Out SDU starts with PID 0xC3");

    dispatcher.send(ControlCommand::SendRawSdsType4 {
        handle: 0,
        source_ssi: 9999,
        dest_ssi: DEST,
        dest_is_group: false,
        len_bits: (payload.len() * 8) as u16,
        payload: payload.clone(),
    });
    test.run_stack(Some(1));
    let msgs = test.dump_sinks();

    // The fix: the command is no longer dropped — exactly one D-SDS-DATA is delivered locally.
    assert_eq!(
        count_d_sds_data(&msgs),
        1,
        "raw Type-4 SDS must be delivered, not ignored (FH-BUG-052)"
    );
    assert_eq!(count_brew_sds(&msgs), 0, "a local dest must not be forwarded to Brew");

    // And the Type-4 SDU is delivered byte-for-byte, with NO SDS-TL wrap prepended.
    let prim = msgs
        .iter()
        .find_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(p) if m.dest == TetraEntity::Mle => Some(p),
            _ => None,
        })
        .expect("a D-SDS-DATA at the Mle sink");
    assert_eq!(prim.main_address.ssi, DEST);
    assert_eq!(prim.main_address.ssi_type, SsiType::Issi);

    let mut sdu = prim.sdu.clone();
    sdu.seek(0);
    let d_sds = DSdsData::from_bitbuf(&mut sdu).expect("decode D-SDS-DATA");
    match d_sds.user_defined_data {
        SdsUserData::Type4(_, data) => {
            assert_eq!(data, payload, "raw Type-4 payload must be delivered verbatim");
            assert_eq!(data[0], 0xC3, "no SDS-TL 0x82 wrap may be added to a raw Type-4 SDU");
        }
        other => panic!("expected Type-4 user data, got {:?}", other),
    }
}

#[test]
fn test_sds_brew_forward() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
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
        feature_rssi_export: false,
        whitelisted_ssis: None,
        pbx_gateway_issis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Do NOT register dest ISSI — should forward to Brew
    let msg = build_u_sds_data_msg(1000001, 5000001, 0x1234);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let brew_count = count_brew_sds(&sink_msgs);
    assert!(brew_count > 0, "Expected CmceSdsData at Brew sink for non-local ISSI");

    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver locally when dest is not registered");
}

#[test]
fn test_sds_from_brew_to_local() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register dest ISSI in StackState
    register_subscriber(&mut test, 2000001);

    // Submit CmceSdsData from Brew on Control SAP
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 2000001,
            user_defined_data: SdsUserData::Type1(0xCAFE),
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected D-SDS-DATA at Mle sink from Brew");
}

#[test]
fn test_sds_from_brew_unregistered() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Do NOT register dest ISSI
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: 9999999,
            user_defined_data: SdsUserData::Type1(0xDEAD),
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver D-SDS-DATA when dest is not registered");
}

#[test]
fn test_sds_group_delivery() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let gssi = 100;

    // Register 3 ISSIs and affiliate them with the GSSI in StackState
    for issi in [1000001, 1000002, 1000003] {
        register_subscriber(&mut test, issi);
        affiliate_subscriber(&mut test, issi, gssi);
    }

    // Send U-SDS-DATA to the GSSI
    let msg = build_u_sds_data_msg(1000001, gssi, 0xBEEF);
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 1, "Expected exactly 1 GSSI-addressed D-SDS-DATA (not per-member)");

    // Verify the address is GSSI
    for m in &sink_msgs {
        if m.dest == TetraEntity::Mle
            && let SapMsgInner::LcmcMleUnitdataReq(ref prim) = m.msg
        {
            assert_eq!(prim.main_address.ssi, gssi);
            assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
        }
    }
}

#[test]
fn test_sds_from_brew_to_group() {
    // FH-FEAT-032 R2: a group-addressed (GSSI) SDS arriving from Brew must be delivered to the MS
    // camped on that group. Previously dropped, because rx_sds_from_brew only checked is_registered()
    // (individual ISSIs) and a GSSI never matches.
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let gssi = 100;
    for issi in [1000001, 1000002, 1000003] {
        register_subscriber(&mut test, issi);
        affiliate_subscriber(&mut test, issi, gssi);
    }

    // CmceSdsData from Brew addressed to the GSSI.
    let msg = SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceSdsData(CmceSdsData {
            source_issi: 3000001,
            dest_issi: gssi,
            user_defined_data: SdsUserData::Type1(0xBEEF),
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    assert_eq!(
        count_d_sds_data(&sink_msgs),
        1,
        "Expected exactly 1 GSSI-addressed D-SDS-DATA from Brew (not dropped, not per-member)"
    );
    for m in &sink_msgs {
        if m.dest == TetraEntity::Mle
            && let SapMsgInner::LcmcMleUnitdataReq(ref prim) = m.msg
        {
            assert_eq!(prim.main_address.ssi, gssi);
            assert_eq!(prim.main_address.ssi_type, SsiType::Gssi);
        }
    }
}

#[test]
fn test_brew_inbound_allowed_bypasses_whitelist_but_honors_local_ranges() {
    // FH-FEAT-032 R3: inbound admission must ignore the outbound-only `whitelisted_ssis`, but still
    // honour `local_ssi_ranges` (documented as local-only / "incoming brew traffic ... rejected").
    use tetra_core::ranges::SortedDisjointSsiRanges;
    use tetra_entities::net_brew::{is_brew_gssi_routable, is_brew_inbound_allowed};

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.local_ssi_ranges = SortedDisjointSsiRanges::from_vec_tuple(vec![(0, 90)]);
    config.brew = Some(CfgBrew {
        host: "test.local".into(),
        port: 3000,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: Some(vec![91]), // only GSSI 91 is whitelisted for OUTBOUND forwarding
        pbx_gateway_issis: None,
    });
    let test = ComponentTest::from_config(config, None);

    // Foreign GSSI 500: not local, not whitelisted.
    // Outbound stays blocked by the whitelist; inbound is now ADMITTED (the R3 fix).
    assert!(
        !is_brew_gssi_routable(&test.config, 500),
        "outbound: non-whitelisted GSSI not routable"
    );
    assert!(is_brew_inbound_allowed(&test.config, 500), "inbound: foreign GSSI must be admitted");

    // GSSI 50 inside local_ssi_ranges: rejected on BOTH directions.
    assert!(!is_brew_gssi_routable(&test.config, 50));
    assert!(
        !is_brew_inbound_allowed(&test.config, 50),
        "inbound: local-only range must stay rejected"
    );

    // Whitelisted GSSI 91: allowed both ways.
    assert!(is_brew_gssi_routable(&test.config, 91));
    assert!(is_brew_inbound_allowed(&test.config, 91));
}

#[test]
fn test_u_status_forwarded_as_d_status() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Register both source and dest
    register_subscriber(&mut test, 1000001);
    register_subscriber(&mut test, 2000001);

    // Build a U-STATUS PDU from 1000001 to 2000001 with pre-coded status 0x8210
    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(2000001),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(0x8210),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    let msg = SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();

    // Should produce exactly 1 D-STATUS at Mle sink
    let mle_msgs: Vec<_> = sink_msgs
        .iter()
        .filter(|m| m.dest == TetraEntity::Mle && matches!(&m.msg, SapMsgInner::LcmcMleUnitdataReq(_)))
        .collect();
    assert_eq!(mle_msgs.len(), 1, "Expected 1 D-STATUS at Mle sink");

    // Verify addressed to 2000001
    if let SapMsgInner::LcmcMleUnitdataReq(ref prim) = mle_msgs[0].msg {
        assert_eq!(prim.main_address.ssi, 2000001);
        assert_eq!(prim.main_address.ssi_type, SsiType::Issi);
    }
}

#[test]
fn test_u_status_brew_forward() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
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
        feature_rssi_export: false,
        whitelisted_ssis: None,
        pbx_gateway_issis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Only register source, NOT dest — should forward to Brew
    register_subscriber(&mut test, 1000001);

    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(5000001),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(0x8210),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    let msg = SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();

    // Should forward to Brew as CmceSdsData with Type1 payload
    let brew_count = count_brew_sds(&sink_msgs);
    assert_eq!(brew_count, 1, "Expected 1 CmceSdsData at Brew sink for U-STATUS");

    // Verify the payload is Type1 with the original pre-coded status value
    let brew_msg = sink_msgs.iter().find(|m| m.dest == TetraEntity::Brew).unwrap();
    if let SapMsgInner::CmceSdsData(ref sds) = brew_msg.msg {
        assert_eq!(sds.source_issi, 1000001);
        assert_eq!(sds.dest_issi, 5000001);
        assert_eq!(sds.user_defined_data, SdsUserData::Type1(0x8210));
    } else {
        panic!("Expected CmceSdsData message at Brew sink");
    }

    // Should NOT deliver locally
    let d_sds_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_sds_count, 0, "Should not deliver locally when dest is not registered");
}

#[test]
fn test_u_status_unregistered_dest_dropped() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));

    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    // Only register source, NOT dest
    register_subscriber(&mut test, 1000001);

    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(9999999),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(0x8210),
        external_subscriber_number: None,
        dm_ms_address: None,
    };

    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
    sdu.seek(0);

    let msg = SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(1000001, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    };
    test.submit_message(msg);
    test.run_stack(Some(1));

    let sink_msgs = test.dump_sinks();
    let d_status_count = count_d_sds_data(&sink_msgs);
    assert_eq!(d_status_count, 0, "Should not deliver D-STATUS when dest is not registered");
}

/// Build a U-SETUP for a group call from `calling_issi` to `dest_gssi`.
fn build_u_setup_group_msg(calling_issi: u32, dest_gssi: u32) -> SapMsg {
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

/// Build a U-DISCONNECT for `call_id` from the call owner, to release a group call.
fn build_u_disconnect_msg(owner_issi: u32, call_id: u16) -> SapMsg {
    let pdu = UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(48);
    pdu.to_bitbuf(&mut sdu).expect("Failed to serialize UDisconnect");
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
            received_tetra_address: TetraAddress::new(owner_issi, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// Count D-SDS-DATA (LcmcMleUnitdataReq to Mle) addressed to a specific ISSI.
fn d_sds_to(msgs: &[SapMsg], issi: u32) -> Vec<&LcmcMleUnitdataReq> {
    msgs.iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(p) if m.dest == TetraEntity::Mle && p.main_address.ssi == issi => Some(p),
            _ => None,
        })
        .collect()
}

/// FH-BUG-034 (final): the field radios do not accept an SDS in-band on the traffic channel
/// (verified on-air against FACCH stealing with fragmentation, single-block STCH, and a full-slot
/// SCH/F in the hangtime gap — the BS transmits all of them per ETSI, the radios receive none).
/// They only accept an SDS on the MCCH. So an SDS to an in-call MS is DEFERRED and delivered on the
/// MCCH once the call releases, which is acknowledged end-to-end.
///
/// This test drives the field scenario (ISSI talker of a group call) and asserts:
///   (1) while the talker is in the call, the SDS is deferred (nothing addressed to it is emitted);
///   (2) after the call releases, it is delivered on the MCCH (no stealing / no chan_alloc).
#[test]
fn test_sds_to_in_call_ms_is_deferred_then_delivered_on_mcch() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let talker = 1000001;
    let listener = 1000002;
    let gssi = 100;

    // A listener affiliated via the MM control path so the BS accepts the group call.
    for action in [BrewSubscriberAction::Register, BrewSubscriberAction::Affiliate] {
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                issi: listener,
                groups: vec![gssi],
                action,
            }),
        });
        test.run_stack(Some(1));
    }
    // Talker registered for SDS routing (local delivery), keys up the group call (call_id starts at 4).
    register_subscriber(&mut test, talker);
    test.submit_message(build_u_setup_group_msg(talker, gssi));
    test.run_stack(Some(2));
    test.dump_sinks();

    // Sanity: the talker is on a traffic timeslot.
    assert!(
        test.config.state_read().active_call_ts.contains_key(&talker),
        "talker should be mapped onto a traffic timeslot while in the call"
    );

    // (1) SDS to the in-call talker -> deferred: nothing addressed to the talker is emitted.
    test.submit_message(build_u_sds_data_msg(2000002, talker, 0x1234));
    test.run_stack(Some(2));
    let during = test.dump_sinks();
    assert!(
        d_sds_to(&during, talker).is_empty(),
        "SDS to an in-call MS must be deferred (not transmitted) while the call is up"
    );

    // (2) Release the call (owner disconnects). The talker leaves active_call_ts and the deferred
    // SDS is delivered on the MCCH.
    test.submit_message(build_u_disconnect_msg(talker, 4));
    test.run_stack(Some(3));
    assert!(
        !test.config.state_read().active_call_ts.contains_key(&talker),
        "after release the talker must no longer be on a traffic timeslot"
    );
    let after = test.dump_sinks();
    let delivered = d_sds_to(&after, talker);
    assert!(!delivered.is_empty(), "deferred SDS must be delivered once the call releases");
    assert!(
        delivered.iter().all(|p| !p.stealing_permission && p.chan_alloc.is_none()),
        "deferred SDS must be delivered on the MCCH (no FACCH stealing) after the call ends"
    );
}

/// FH-BUG-036: an SDS deferred while its destination is BOTH in a (short) call AND an energy-economy
/// MS is, after the call releases, delivered on the destination's next monitoring window — on the
/// MCCH, never off-window — provided this happens within `SDS_DEFER_DEADLINE`. (Past the deadline the
/// SDS fails cleanly instead, which is wall-clock-timed and covered by the field log rather than a
/// unit test.)
#[test]
fn test_sds_to_in_call_ee_ms_waits_for_window_after_call() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    let talker = 1000001; // the EE destination, also the group-call talker
    let listener = 1000002;
    let gssi = 100;

    // Listener affiliated via MM control path so the BS accepts the group call.
    for action in [BrewSubscriberAction::Register, BrewSubscriberAction::Affiliate] {
        test.submit_message(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Mm,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate {
                issi: listener,
                groups: vec![gssi],
                action,
            }),
        });
        test.run_stack(Some(1));
    }
    register_subscriber(&mut test, talker);

    // Talker is an EE MS: frame-based window, start point (frame 5, multiframe 1), cycle 6 (Eg3 —
    // ETSI Table 23.9, the BS's cap). The MS wakes every 6 frames; with this phase the open frames
    // (abs ≡ 4 mod 6 → frames 5, 11, 17…) are clear of frames 1–4 that the call setup/release pass
    // through, so the window stays shut across the release and the SDS must wait for it.
    // (MM is absent from this test, so the published map is not overwritten.)
    test.config.state_write().ee_monitoring_windows.insert(talker, (5, 1, 6));

    // Talker keys up the group call (call_id starts at 4).
    test.submit_message(build_u_setup_group_msg(talker, gssi));
    test.run_stack(Some(2));
    test.dump_sinks();
    assert!(
        test.config.state_read().active_call_ts.contains_key(&talker),
        "talker should be on a traffic timeslot while in the call"
    );

    // SDS to the in-call EE talker -> deferred.
    test.submit_message(build_u_sds_data_msg(2000002, talker, 0x1234));
    test.run_stack(Some(2));
    assert!(
        d_sds_to(&test.dump_sinks(), talker).is_empty(),
        "SDS to an in-call MS must be deferred while the call is up"
    );

    // Release the call. The talker leaves active_call_ts but its EE window is still shut.
    test.submit_message(build_u_disconnect_msg(talker, 4));
    test.run_stack(Some(3));
    assert!(
        !test.config.state_read().active_call_ts.contains_key(&talker),
        "after release the talker must no longer be on a traffic timeslot"
    );
    // KEY (FH-BUG-036): the SDS must NOT be flushed off-window the instant the call ends — it waits
    // for the EE monitoring window. (The window is shut at these frames.)
    assert!(
        d_sds_to(&test.dump_sinks(), talker).is_empty(),
        "after call release with the window shut, the SDS must wait for the window, not go off-window"
    );

    // Advance until the monitoring window opens -> delivered on the MCCH (no FACCH stealing).
    test.run_stack(Some(160));
    let after = test.dump_sinks();
    let delivered = d_sds_to(&after, talker);
    assert!(
        !delivered.is_empty(),
        "deferred SDS must be delivered once the EE monitoring window opens after the call"
    );
    assert!(
        delivered.iter().all(|p| !p.stealing_permission && p.chan_alloc.is_none()),
        "EE-deferred SDS must be delivered on the MCCH"
    );
}

/// FH-BUG-034 / EE compliance: an SDS to an energy-economy MS that is asleep outside its monitoring
/// window must be DEFERRED until the window opens, then delivered on the MCCH — so the BS respects
/// the MS's reduced monitoring schedule (ETSI EN 300 392-2 §16.7) instead of transmitting blindly.
#[test]
fn test_sds_to_ee_ms_defers_until_monitoring_window() {
    debug::setup_logging_verbose();

    // Start on frame 1, multiframe 1 (multiframe_index 0).
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let components = vec![TetraEntity::Cmce];
    let sinks = vec![TetraEntity::Mle, TetraEntity::Brew];
    test.populate_entities(components, sinks);

    let dest = 2000001;
    register_subscriber(&mut test, dest); // registered, idle (not in a call)

    // Publish a frame-based EE monitoring window for dest: start point (frame 2, multiframe 1),
    // cycle 2 (Eg1 — ETSI Table 23.9). The absolute frame index (m-1)*18+(f-1) is odd at the start
    // point, so the window is CLOSED at frame 1 (abs 0, even) and opens at frame 2 (abs 1, odd).
    // (MM is not in this test, so the published map is not overwritten.)
    test.config.state_write().ee_monitoring_windows.insert(dest, (2, 1, 2));

    // SDS arrives while dest is asleep (frame 1, window closed) -> must be deferred, not emitted.
    test.submit_message(build_u_sds_data_msg(3000001, dest, 0xABCD));
    test.run_stack(Some(2)); // still within frame 1
    let during = test.dump_sinks();
    assert!(
        count_d_sds_data(&during) == 0,
        "SDS to an asleep EE MS must be deferred (not transmitted) outside its monitoring window"
    );

    // Advance to the MS's monitoring window (frame 2). The deferred SDS must now be delivered on
    // the MCCH (no stealing).
    test.run_stack(Some(10));
    let after = test.dump_sinks();
    let delivered: Vec<&LcmcMleUnitdataReq> = after
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::LcmcMleUnitdataReq(p) if m.dest == TetraEntity::Mle && p.main_address.ssi == dest => Some(p),
            _ => None,
        })
        .collect();
    assert!(
        !delivered.is_empty(),
        "deferred SDS must be delivered once the EE monitoring window opens"
    );
    assert!(
        delivered.iter().all(|p| !p.stealing_permission && p.chan_alloc.is_none()),
        "EE-deferred SDS must be delivered on the MCCH"
    );
}

/// Helper: build a U-STATUS SAP message from `src` to `dest` carrying a pre-coded status value.
fn build_u_status_msg(src: u32, dest: u32, status: u16) -> SapMsg {
    let u_status = UStatus {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest as u64),
        called_party_extension: None,
        pre_coded_status: PreCodedStatus::from(status),
        external_subscriber_number: None,
        dm_ms_address: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    u_status.to_bitbuf(&mut sdu).expect("Failed to serialize U-STATUS");
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
            received_tetra_address: TetraAddress::new(src, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// FH-FEAT-014: an authorized ISSI sending a U-STATUS to 9999 whose status code maps to the "ip"
/// action gets a text-SDS reply addressed back to it.
#[test]
fn test_u_status_command_ip_replies_to_authorized() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sds_command_control = Some(CfgSdsCommandControl {
        authorized_issis: vec![1000001],
        commands: vec![CfgSdsCommandEntry {
            status_code: 50,
            action: "ip".to_string(),
        }],
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, 1000001);

    test.submit_message(build_u_status_msg(1000001, 9999, 50));
    test.run_stack(Some(2));

    let after = test.dump_sinks();
    let reply = d_sds_to(&after, 1000001);
    assert!(
        !reply.is_empty(),
        "authorized U-STATUS (action=ip) to 9999 must produce an SDS reply to the requester"
    );
}

/// FH-FEAT-014: a U-STATUS from an ISSI that is NOT in authorized_issis must be ignored — no reply.
#[test]
fn test_u_status_command_unauthorized_no_reply() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.sds_command_control = Some(CfgSdsCommandControl {
        authorized_issis: vec![1000001], // 1000002 is NOT authorized
        commands: vec![CfgSdsCommandEntry {
            status_code: 50,
            action: "ip".to_string(),
        }],
    });
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);
    register_subscriber(&mut test, 1000002);

    test.submit_message(build_u_status_msg(1000002, 9999, 50));
    test.run_stack(Some(2));

    let after = test.dump_sinks();
    assert!(
        d_sds_to(&after, 1000002).is_empty(),
        "unauthorized U-STATUS to 9999 must be ignored (no reply)"
    );
}

// ── Emergency status handling ───────────────────────────────────────────────
// (reuses the existing `build_u_status_msg(src, dest, status: u16)` helper above; the emergency
//  pre-coded status is raw value 0, ETSI EN 300 392-2 table 14.72.)

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
        feature_rssi_export: false,
        whitelisted_ssis: None,
        pbx_gateway_issis: None,
    });
    config
}

/// By default (forward_to_brew = false) an emergency status is LOCAL-only: it must NOT be
/// forwarded to Brew even with Brew active and the destination not locally registered. A
/// subsequent NON-emergency status from the same ISSI clears the session and forwards normally,
/// proving the gate only affects emergency and the normal status path is intact.
#[test]
fn test_emergency_status_not_forwarded_to_brew_by_default() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let config = brew_test_config();
    assert!(!config.emergency.forward_to_brew, "default must be LOCAL-only");
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    // Emergency status (pre-coded status 0) to an unregistered dest (ISSI 9, the emergency address).
    test.submit_message(build_u_status_msg(1000001, 9, 0));
    test.run_stack(Some(1));
    let after_emergency = test.dump_sinks();
    assert_eq!(
        count_brew_sds(&after_emergency),
        0,
        "emergency status must NOT be forwarded to Brew when forward_to_brew is false (LOCAL-only)"
    );

    // A non-emergency status (Network/User-Specific) from the same ISSI clears the session and
    // forwards normally.
    test.submit_message(build_u_status_msg(1000001, 9, 0x8001));
    test.run_stack(Some(1));
    let after_normal = test.dump_sinks();
    assert!(
        count_brew_sds(&after_normal) > 0,
        "a non-emergency status to an unregistered dest must still forward to Brew"
    );
}

/// With forward_to_brew = true the operator opts the emergency status back into Brew forwarding.
#[test]
fn test_emergency_status_forwarded_to_brew_when_opted_in() {
    debug::setup_logging_verbose();
    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = brew_test_config();
    config.emergency.forward_to_brew = true;
    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Brew]);

    test.submit_message(build_u_status_msg(1000001, 9, 0));
    test.run_stack(Some(1));
    let after = test.dump_sinks();
    assert!(
        count_brew_sds(&after) > 0,
        "emergency status must be forwarded to Brew when forward_to_brew is true"
    );
}
