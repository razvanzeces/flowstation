mod common;

use std::time::Duration;

use tetra_config::bluestation::{CfgBrew, CfgManualIdentity, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, TxState, debug};
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::enums::transmission_grant::TransmissionGrant;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::d_setup::DSetup;
use tetra_pdus::cmce::pdus::d_tx_granted::DTxGranted;
use tetra_pdus::cmce::pdus::u_facility::UFacility;
use tetra_pdus::cmce::pdus::u_setup::USetup;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::call_control::CallControl;
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use uuid::Uuid;

use crate::common::ComponentTest;

const TEST_GSSI: u32 = 91;
const TEST_ISSI: u32 = 1000001;

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
                    if prim.chan_alloc.as_ref().is_some_and(|ca| ca.usage.is_some()))
        })
        .count()
}

fn build_network_call_start_msg(brew_uuid: Uuid, source_issi: u32, dest_gssi: u32) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallStart {
            brew_uuid,
            source_issi,
            dest_gssi,
            priority: 0,
        }),
    }
}

fn build_network_call_end_msg(brew_uuid: Uuid) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Brew,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCallEnd { brew_uuid }),
    }
}

fn decode_d_setups(msgs: &[SapMsg]) -> Vec<DSetup> {
    msgs.iter()
        .filter_map(|msg| {
            let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
                return None;
            };
            let mut sdu = prim.sdu.clone();
            sdu.seek(0);
            DSetup::from_bitbuf(&mut sdu).ok()
        })
        .collect()
}

fn decode_d_tx_granted(msgs: &[SapMsg]) -> Vec<DTxGranted> {
    msgs.iter()
        .filter_map(|msg| {
            let SapMsgInner::LcmcMleUnitdataReq(prim) = &msg.msg else {
                return None;
            };
            let mut sdu = prim.sdu.clone();
            sdu.seek(0);
            DTxGranted::from_bitbuf(&mut sdu).ok()
        })
        .collect()
}

#[test]
fn test_network_hangtime_reuse_refreshes_group_dsetup_with_tpi_mnemonic() {
    debug::setup_logging_verbose();

    const FIRST_SPEAKER: u32 = 2_260_571;
    const SECOND_SPEAKER: u32 = 2_260_580;

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "test.invalid".to_string(),
        port: 443,
        tls: true,
        username: None,
        password: None,
        reconnect_delay: Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        feature_rssi_export: false,
        whitelisted_ssis: None,
    });
    config.identity.enabled = true;
    config.identity.manual.push(CfgManualIdentity {
        ssi: SECOND_SPEAKER,
        mnemonic: Some("YO3TCO".to_string()),
        label: None,
    });

    let mut test = ComponentTest::from_config(config, Some(dltime));
    test.populate_entities(
        vec![TetraEntity::Cmce],
        vec![TetraEntity::Mle, TetraEntity::Umac, TetraEntity::Brew],
    );
    register_subscriber(&mut test, TEST_ISSI, TEST_GSSI);

    let first_uuid = Uuid::new_v4();
    test.submit_message(build_network_call_start_msg(first_uuid, FIRST_SPEAKER, TEST_GSSI));
    test.run_stack(Some(2));
    test.dump_sinks();

    test.submit_message(build_network_call_end_msg(first_uuid));
    test.run_stack(Some(2));
    test.dump_sinks();

    let second_uuid = Uuid::new_v4();
    test.submit_message(build_network_call_start_msg(second_uuid, SECOND_SPEAKER, TEST_GSSI));
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    let refreshed_setup = decode_d_setups(&msgs)
        .into_iter()
        .find(|setup| setup.calling_party_address_ssi == Some(SECOND_SPEAKER))
        .expect("hangtime reuse must immediately refresh group D-SETUP for the new Brew speaker");
    assert_eq!(refreshed_setup.transmission_grant, TransmissionGrant::GrantedToOtherUser);
    let facility = refreshed_setup
        .facility
        .expect("refreshed group D-SETUP must carry SS-TPI for RX display");
    assert_eq!(facility.mnemonic_name.as_deref(), Some("YO3TCO"));
    assert_eq!(
        facility.talking_sending_party_ssi, None,
        "group D-SETUP already carries the caller SSI; SS-TPI should add mnemonic only"
    );

    let granted = decode_d_tx_granted(&msgs)
        .into_iter()
        .find(|granted| granted.transmitting_party_address_ssi == Some(SECOND_SPEAKER as u64))
        .expect("speaker change must still send D-TX GRANTED");
    assert!(
        granted.facility.is_none(),
        "D-TX GRANTED is sent over FACCH/STCH and must remain facility-free to stay within signalling capacity"
    );
}

#[test]
fn test_u_facility_probe_has_no_error_response() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Cmce], vec![TetraEntity::Mle, TetraEntity::Mm]);

    let mut sdu = BitBuffer::new_autoexpand(16);
    UFacility {}.to_bitbuf(&mut sdu).expect("Failed to serialize UFacility");
    sdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::LcmcSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Cmce,
        msg: SapMsgInner::LcmcMleUnitdataInd(LcmcMleUnitdataInd {
            sdu,
            handle: 1,
            endpoint_id: 1,
            link_id: 1,
            received_tetra_address: TetraAddress::new(TEST_ISSI, SsiType::Issi),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    });
    test.run_stack(Some(1));

    let msgs = test.dump_sinks();
    assert!(
        !msgs
            .iter()
            .any(|msg| msg.dest == TetraEntity::Mle && matches!(msg.msg, SapMsgInner::LcmcMleUnitdataReq(_))),
        "U-FACILITY probes must not get D-CMCE-FUNCTION-NOT-SUPPORTED"
    );
    assert!(
        msgs.iter().any(|msg| {
            msg.dest == TetraEntity::Mm
                && matches!(
                    &msg.msg,
                    SapMsgInner::MmForceLocationUpdate { issi, .. } if *issi == TEST_ISSI
                )
        }),
        "Unknown U-FACILITY probes should force MM location update"
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
