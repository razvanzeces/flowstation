mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, TxState, debug};
use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
use tetra_pdus::cmce::pdus::u_setup::USetup;
use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::control::enums::communication_type::CommunicationType;
use tetra_saps::control::call_control::CallControl;
use tetra_saps::lcmc::LcmcMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

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

/// Helper: build a U-SETUP SAP message for a group call (ordinary priority 0).
fn build_u_setup_msg(calling_issi: u32, dest_gssi: u32) -> SapMsg {
    build_u_setup_msg_prio(calling_issi, dest_gssi, 0)
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

/// Extract tx_reporters from D-SETUP messages in the sink output.
/// D-SETUPs are identified as LcmcMleUnitdataReq with a chan_alloc that has a usage field.
fn extract_d_setup_reporters(msgs: &mut Vec<SapMsg>) -> Vec<tetra_core::TxReporter> {
    let mut reporters = vec![];
    for msg in msgs.iter_mut() {
        if msg.dest == TetraEntity::Mle
            && let SapMsgInner::LcmcMleUnitdataReq(ref mut prim) = msg.msg
                && prim.chan_alloc.as_ref().is_some_and(|ca| ca.usage.is_some())
                    && let Some(reporter) = prim.tx_reporter.take() {
                        reporters.push(reporter);
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

/// Test that late-entry D-SETUP re-sends are throttled when the previous
/// D-SETUP's TxReceipt is still in Pending state (UMAC hasn't transmitted it yet),
/// and that they resume once the receipt reaches a final state.
///
/// IGNORED: this covers a receipt-based throttle that no longer exists. `circuit_mgr`
/// now resends late-entry D-SETUP on a fixed ~5s schedule (1 initial + 1 backup, then
/// every LATE_ENTRY_INTERVAL) with no tx_reporter on the resends and no Pending-receipt
/// suppression — see `circuit_mgr::tick_start` and `cc_bs::timers` (resends built with
/// `tx_reporter = None`). The current unthrottled behaviour is intentional and verified
/// in production. Re-enable only if receipt-based throttling is reintroduced.
#[ignore = "throttle feature removed; late-entry D-SETUP now resends on a fixed schedule"]
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

/// Helper: build a U-SETUP SAP message for a P2P (individual) call to `called_issi`.
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

// Energy-Economy D-SETUP gate (clause 16.7): individual-call setup resends to a sleeping EE MS
// are held for the MS's downlink monitoring window, with a bounded fallback (EE_DSETUP_FALLBACK_TS
// ≈ 423 timeslots / ~105 frames) to the historical blind resend. The empirically-observed resend
// cadence (initial + late-entry) fires several individual D-SETUPs to the called MS within the
// fallback window (around frames 0/44/89), which the tests below rely on.

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
    register_subscriber(&mut test, called, 9); // local registration -> local P2P (not Brew)

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
    register_subscriber(&mut test, called, 9);
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
    register_subscriber(&mut test, called, 9);

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
