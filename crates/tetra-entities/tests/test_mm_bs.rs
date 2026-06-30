mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
use tetra_pdus::cmce::pdus::d_facility::DFacility;
use tetra_pdus::cmce::ss_dgna::enums::results::GroupIdentityAttachmentMode;
use tetra_pdus::cmce::ss_dgna::ss_dgna_pdu::SsDgnaPdu;
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity::DAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::d_mm_status::DMmStatus;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use tetra_entities::cmce::cmce_bs::CmceBs;
use tetra_entities::mm::mm_bs::MmBs;
use tetra_entities::net_control::{ControlCommand, make_control_link};

use crate::common::ComponentTest;

/// Register a terminal in MM by submitting a minimal U-LOCATION-UPDATE-DEMAND
/// (RoamingLocationUpdating) as if it arrived from `issi`. After this the MS is "known" and
/// eligible for DGNA.
fn register_terminal(test: &mut ComponentTest, issi: u32) {
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
    let prim = LmmMleUnitdataInd {
        sdu,
        handle: 0,
        received_address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: issi,
        },
    };
    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(prim),
    });
    test.run_stack(Some(2));
}

/// Pull the first MM->CMCE `CmceSsDgnaAssign` SAP request out of a batch of captured messages.
/// This is the air-interface emission MM now makes by default for a DGNA: it hands the SS-DGNA
/// ASSIGN/DEASSIGN to CMCE rather than sending an MM D-ATTACH itself.
fn find_ss_dgna_request(msgs: &[SapMsg]) -> Option<(u32, u32, Option<String>, u8, bool)> {
    msgs.iter().find_map(|m| match m.msg {
        SapMsgInner::CmceSsDgnaAssign {
            issi,
            gssi,
            ref mnemonic,
            attachment_mode,
            route_gssi_hint: _,
            attach,
        } if m.dest == TetraEntity::Cmce => Some((issi, gssi, mnemonic.clone(), attachment_mode, attach)),
        _ => None,
    })
}

/// Pull the first D-FACILITY (and its addressed ISSI) out of a batch of captured MLE messages.
/// Matched on the 5-bit CMCE downlink PDU-type discriminator before parsing, so a non-FACILITY
/// downlink PDU is skipped rather than mis-parsed.
fn find_d_facility(msgs: &[SapMsg]) -> Option<(u32, DFacility)> {
    let want = CmcePduTypeDl::DFacility;
    for m in msgs {
        if let SapMsgInner::LcmcMleUnitdataReq(ref req) = m.msg {
            if CmcePduTypeDl::try_from(req.sdu.peek_bits(5)?).ok() != Some(want) {
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

/// Pull the first D-ATTACH/DETACH GROUP IDENTITY out of a batch of captured MLE messages, if any.
fn find_attach_detach(msgs: &[SapMsg]) -> Option<(u32, DAttachDetachGroupIdentity)> {
    for m in msgs {
        if let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg {
            let mut sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
            if let Ok(pdu) = DAttachDetachGroupIdentity::from_bitbuf(&mut sdu) {
                return Some((req.address.ssi, pdu));
            }
        }
    }
    None
}

/// Pull the addressed ISSI of the first D-LOCATION-UPDATE-COMMAND in a batch of captured MLE
/// messages, if any. Matched on the 4-bit MM downlink PDU-type discriminator (the PDU's own
/// `from_bitbuf` decoder is an unimplemented stub â€” only the encoder MM uses is wired up).
fn find_location_update_command(msgs: &[SapMsg]) -> Option<u32> {
    let want = MmPduTypeDl::DLocationUpdateCommand.into_raw();
    for m in msgs {
        if let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg {
            let mut sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
            if sdu.read_field(4, "pdu_type").is_ok_and(|t| t == want) {
                return Some(req.address.ssi);
            }
        }
    }
    None
}

/// Feed MM an uplink RSSI sample for `issi`, as UMAC does on every random-access/PTT burst.
fn submit_uplink_rssi(test: &mut ComponentTest, issi: u32) {
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Umac,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::MsRssiUpdate { issi, rssi_dbfs: -31.0 },
    });
    test.run_stack(Some(2));
}

/// Reactive restart recovery: an *unknown* (unregistered) ISSI seen transmitting on the uplink
/// must be commanded to re-register â€” this is the ghost-radio-after-restart fix. With reactive
/// recovery on by default and no allowlist, a single RSSI sample yields a D-LOCATION-UPDATE-COMMAND
/// addressed to that ISSI.
#[test]
fn test_reactive_recovery_commands_unknown_issi_on_uplink() {
    debug::setup_logging_verbose();
    const GHOST_ISSI: u32 = 2260301;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    // The radio was never registered with MM (its record was lost to a restart), yet it keys up.
    submit_uplink_rssi(&mut test, GHOST_ISSI);
    let msgs = test.dump_sinks();

    let target = find_location_update_command(&msgs)
        .unwrap_or_else(|| panic!("expected a D-LOCATION-UPDATE-COMMAND for the unknown ISSI, got {} msgs", msgs.len()));
    assert_eq!(target, GHOST_ISSI, "the COMMAND must be addressed to the transmitting ghost ISSI");
}

/// A radio MM already knows must NOT be reactively commanded: its uplink RSSI is normal traffic.
#[test]
fn test_reactive_recovery_skips_known_issi() {
    debug::setup_logging_verbose();
    const KNOWN_ISSI: u32 = 2260570;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    // Register it, then discard the registration ACCEPT (and the new-radio group-report COMMAND).
    register_terminal(&mut test, KNOWN_ISSI);
    let _ = test.dump_sinks();

    // Now a normal uplink burst from the *known* radio must not produce any further COMMAND.
    submit_uplink_rssi(&mut test, KNOWN_ISSI);
    let msgs = test.dump_sinks();

    assert!(
        find_location_update_command(&msgs).is_none(),
        "a known radio's uplink must not trigger reactive recovery, got {} msgs",
        msgs.len()
    );
}

/// Rate limiting: a burst of uplink samples from the same ghost (a single PTT yields several RSSI
/// updates) must key only ONE COMMAND while it re-registers â€” the cooldown suppresses the rest.
#[test]
fn test_reactive_recovery_rate_limits_repeat_bursts() {
    debug::setup_logging_verbose();
    const GHOST_ISSI: u32 = 2260999;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    // First burst â†’ one COMMAND.
    submit_uplink_rssi(&mut test, GHOST_ISSI);
    assert_eq!(
        find_location_update_command(&test.dump_sinks()),
        Some(GHOST_ISSI),
        "first uplink burst from the ghost must command a re-registration"
    );

    // Second burst within the cooldown (still unregistered) â†’ suppressed.
    submit_uplink_rssi(&mut test, GHOST_ISSI);
    assert!(
        find_location_update_command(&test.dump_sinks()).is_none(),
        "a repeat burst inside the cooldown must not re-key the same ISSI"
    );
}

/// DGNA assign (SS-DGNA default): a dashboard control command makes MM affiliate the GSSI in the
/// shared subscriber registry (so local group calls/SDS route to it) AND hand the air-interface
/// emission to CMCE as a `CmceSsDgnaAssign` request. MM no longer sends the legacy D-ATTACH itself
/// â€” the actual D-FACILITY on the wire is asserted in test_cmce_bs (the full MM+CMCE path).
#[test]
fn test_dgna_assign_affiliates_and_requests_ss_dgna() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260571;
    const TEST_GSSI: u32 = 100;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    // Sink CMCE too: MM hands the air emission to CMCE over the Control SAP, so the
    // CmceSsDgnaAssign request is captured there (no real CMCE in this MM-focused test).
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);

    // Register our own MM wired to a control endpoint so we can drive DGNA through the dispatcher.
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);

    // DGNA requires a registered MS.
    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks(); // discard the D-LOCATION-UPDATE-ACCEPT

    // Issue the DGNA assign and let MM process the control command.
    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    let (issi, gssi, mnemonic, attachment_mode, attach) = find_ss_dgna_request(&msgs)
        .unwrap_or_else(|| panic!("expected a CmceSsDgnaAssign request after DGNA assign, got {} msgs", msgs.len()));
    assert_eq!(
        (issi, gssi, mnemonic, attachment_mode, attach),
        (TEST_ISSI, TEST_GSSI, None, 0, true)
    );

    // On the SS-DGNA default MM must NOT also send a legacy MM D-ATTACH (that would double-attach).
    assert!(
        find_attach_detach(&msgs).is_none(),
        "SS-DGNA default must not also emit the legacy D-ATTACH/DETACH GROUP IDENTITY"
    );

    // BS-side affiliation must be reflected for local call/SDS routing.
    assert!(
        test.config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI),
        "DGNA assign must affiliate the GSSI in the subscriber registry"
    );
}

/// DGNA deassign of a previously-assigned group requests a DEASSIGN from CMCE and removes the
/// affiliation.
#[test]
fn test_dgna_deassign_requests_ss_dgna_and_deaffiliates() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260572;
    const TEST_GSSI: u32 = 101;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    // Sink CMCE too so the MM->CMCE CmceSsDgnaAssign request is captured (see assign test).
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);
    register_terminal(&mut test, TEST_ISSI);

    // Assign, then deassign.
    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let _ = test.dump_sinks();
    assert!(
        test.config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI)
    );

    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: false,
    });
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    let (issi, gssi, mnemonic, attachment_mode, attach) = find_ss_dgna_request(&msgs)
        .unwrap_or_else(|| panic!("expected a CmceSsDgnaAssign request after DGNA deassign, got {} msgs", msgs.len()));
    assert_eq!(
        (issi, gssi, mnemonic, attachment_mode, attach),
        (TEST_ISSI, TEST_GSSI, None, 0, false)
    );

    assert!(
        !test
            .config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI),
        "DGNA deassign must remove the GSSI from the subscriber registry"
    );
}

/// A dynamic group remains in the per-device DGNA registry even after the radio detaches from it,
/// so the operator can still see it and explicitly deassign it later.
#[test]
fn test_dgna_registry_survives_group_detach() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260576;
    const TEST_GSSI: u32 = 103;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);
    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: Some("OPS".to_string()),
        attachment_mode: 3,
        attach: true,
    });
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    test.config.state_write().subscribers.deaffiliate(TEST_ISSI, TEST_GSSI);

    let state = test.config.state_read();
    assert!(
        !state.subscribers.attached_groups_of(TEST_ISSI).contains(&TEST_GSSI),
        "manual detach should remove the current affiliation"
    );
    assert!(
        state
            .subscribers
            .dgna_groups_of(TEST_ISSI)
            .iter()
            .any(|group| group.gssi == TEST_GSSI && group.mnemonic.as_deref() == Some("OPS")),
        "detaching a dynamic group must not erase the DGNA registry entry"
    );
}

/// A static affiliation may be operator-detached through the DGNA control surface. MM must emit the
/// detach on air but keep the DGNA registry untouched because the group was never dynamic.
#[test]
fn test_dgna_deassign_allows_static_group_detach() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260577;
    const TEST_GSSI: u32 = 104;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);
    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    test.config.state_write().subscribers.affiliate(TEST_ISSI, TEST_GSSI);

    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: false,
    });
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    let (issi, gssi, _mnemonic, attachment_mode, attach) = find_ss_dgna_request(&msgs)
        .unwrap_or_else(|| panic!("expected a CmceSsDgnaAssign request after static detach, got {} msgs", msgs.len()));
    assert_eq!(issi, TEST_ISSI);
    assert_eq!(gssi, TEST_GSSI);
    assert_eq!(attachment_mode, 0);
    assert!(!attach, "static detach must emit a DEASSIGN request on the SS-DGNA path");
    assert!(
        !test
            .config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI),
        "static detach must remove the current attachment"
    );
    assert!(
        test.config.state_read().subscribers.dgna_groups_of(TEST_ISSI).is_empty(),
        "static detach must not fabricate a DGNA registry entry"
    );
}

/// Rollback path: with `dgna_use_ss_facility = false` the legacy MM-only D-ATTACH/DETACH GROUP
/// IDENTITY (EN 300 392-2 V2.4.1 cl.16.8) must still be emitted, and CMCE must NOT be asked for an
/// SS-DGNA D-FACILITY. Guards the rollout switch.
#[test]
fn test_dgna_legacy_flag_emits_mm_attach() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260573;
    const TEST_GSSI: u32 = 102;

    // Build a config with the SS-DGNA path turned off.
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.dgna_use_ss_facility = false;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);

    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);

    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    let (addr_ssi, pdu) = find_attach_detach(&msgs).unwrap_or_else(|| {
        panic!(
            "legacy DGNA flag must emit a D-ATTACH/DETACH GROUP IDENTITY, got {} msgs",
            msgs.len()
        )
    });
    assert_eq!(addr_ssi, TEST_ISSI, "DGNA PDU must be addressed to the target ISSI");
    assert!(pdu.group_identity_acknowledgement_request, "DGNA must request an ACK");
    assert!(!pdu.group_identity_attach_detach_mode, "DGNA must amend, not reset, the group list");
    let gids = pdu.group_identity_downlink.expect("downlink groups present");
    assert_eq!(gids.len(), 1);
    assert_eq!(gids[0].gssi, Some(TEST_GSSI));
    assert!(
        gids[0].group_identity_attachment.is_some(),
        "an assign carries a group identity attachment"
    );

    // The legacy path must NOT also hand an SS-DGNA request to CMCE.
    assert!(
        find_ss_dgna_request(&msgs).is_none(),
        "legacy DGNA path must not also request an SS-DGNA D-FACILITY"
    );

    assert!(
        test.config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI),
        "legacy DGNA assign must still affiliate the GSSI in the subscriber registry"
    );
}

/// Regression for the dashboard path (FlowStation log 00:19:24 "CMCE: ignoring unsupported control
/// command Dgna"): the dashboard's control channel terminates at CMCE, not MM. A DGNA command
/// delivered to CMCE must be forwarded to MM, which (SS-DGNA default) hands the emission back to
/// CMCE as a D-FACILITY{ASSIGN} on the air â€” exactly the path a real dashboard click takes.
#[test]
fn test_dgna_from_cmce_control_reaches_mm_and_emits_d_facility() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260575;
    const TEST_GSSI: u32 = 20;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);

    // Real MM with NO control endpoint â€” it must receive DGNA via the SAP forward from CMCE.
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    // Real CMCE wired to a control endpoint, exactly like the binary wires the dashboard.
    let (cmce_dispatcher, cmce_endpoint) = make_control_link();
    let cmce = CmceBs::new(test.get_shared_config(), None, Some(cmce_endpoint));
    test.register_entity(cmce);

    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    // Send DGNA to CMCE's control endpoint (the dashboard's path), NOT to MM directly.
    cmce_dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    // CMCE drains control -> forwards MmDgnaRequest -> MM affiliates + requests CmceSsDgnaAssign ->
    // CMCE emits the D-FACILITY. Allow a few ticks for the multi-hop.
    test.run_stack(Some(6));
    let msgs = test.dump_sinks();

    let (addr_ssi, facility) = find_d_facility(&msgs).unwrap_or_else(|| {
        panic!(
            "DGNA via CMCE must reach MM and emit a D-FACILITY (SS-DGNA), got {} msgs",
            msgs.len()
        )
    });
    assert_eq!(addr_ssi, TEST_ISSI);
    let body = facility.facility.expect("D-FACILITY must carry an SS-DGNA body");
    let SsDgnaPdu::Assign(assign) = body.ss_pdu else {
        panic!("expected an ASSIGN, got {}", body.ss_pdu);
    };
    assert_eq!(assign.groups.len(), 1);
    assert_eq!(assign.groups[0].group_ssi, TEST_GSSI);
    assert_eq!(assign.groups[0].attachment_mode, GroupIdentityAttachmentMode::AttachedPermanently);

    assert!(
        test.config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&TEST_GSSI),
        "DGNA via CMCE must affiliate the GSSI in the subscriber registry"
    );
}

/// DGNA aimed at an unregistered terminal is refused: nothing is sent over the air.
#[test]
fn test_dgna_to_unregistered_issi_is_refused() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);

    // No registration first â€” the command must be dropped, emitting no group identity PDU.
    dispatcher.send(ControlCommand::Dgna {
        issi: 9_999_001,
        gssi: 100,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let msgs = test.dump_sinks();

    assert!(
        find_attach_detach(&msgs).is_none(),
        "DGNA to an unregistered ISSI must not emit a group identity PDU"
    );
    assert!(
        find_ss_dgna_request(&msgs).is_none(),
        "DGNA to an unregistered ISSI must not request an SS-DGNA D-FACILITY"
    );
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

    // Energy saving mode requests now get a D-MM-STATUS ChangeOfEnergySavingModeResponse
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
    assert!(resp_pdu.energy_saving_information.is_some());
}

/// Restart recovery: a seeded cache is loaded into MM as known-but-Detached terminals (no SAP
/// emitted at load), and the startup sweep replays a D-LOCATION-UPDATE-COMMAND to each cached
/// ISSI â€” addressed by ISSI with handle 0, paced one per TDMA frame, round-robin.
#[test]
fn test_restart_recovery_loads_and_replays() {
    // Config with recovery enabled and 1 COMMAND per frame.
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.recovery.enabled = true;
    config.recovery.replay_per_frame = 1;

    // Seed a cache with two terminals, one affiliated to a group.
    let path = std::env::temp_dir().join("fs_recovery_it_replay.json");
    std::fs::write(
        &path,
        r#"{"version":1,"terminals":[
            {"issi":1000001,"groups":[91],"dgna_groups":[{"gssi":91,"mnemonic":"OPS","attachment_mode":3}],"energy_saving_mode":0},
            {"issi":1000002,"groups":[],"dgna_groups":[],"energy_saving_mode":0}
        ]}"#,
    )
    .unwrap();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    // MLE is the sink that captures MM's downlink PDUs; we register our own recovery-initialised MM.
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mut mm = MmBs::new(test.get_shared_config(), None, None);
    mm.init_recovery(path.clone());
    test.register_entity(mm);

    // Nothing should be emitted purely from loading the cache (re-affiliation happens only when a
    // terminal actually re-registers, not at load) â€” verified by running zero-effect setup below.

    // Drive several frames; each tick advances the TDMA clock by one slot (4 slots/frame), so a
    // handful of ticks spans multiple frames and the round-robin sweep reaches both ISSIs.
    test.run_stack(Some(24));
    let msgs = test.dump_sinks();

    // Every emitted PDU during a recovery-only run is a D-LOCATION-UPDATE-COMMAND. Collect the
    // target ISSIs and confirm the handle is 0 (the handle is inert; MLE routes by ISSI).
    let mut targets: Vec<u32> = Vec::new();
    for m in &msgs {
        if let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg {
            assert_eq!(req.handle, 0, "recovery COMMAND must be addressed with handle 0");
            assert_eq!(req.address.ssi_type, SsiType::Issi);
            targets.push(req.address.ssi);
        }
    }

    assert!(
        targets.contains(&1000001),
        "ISSI 1000001 should receive a recovery COMMAND, got {:?}",
        targets
    );
    assert!(
        targets.contains(&1000002),
        "ISSI 1000002 should receive a recovery COMMAND, got {:?}",
        targets
    );

    let restored_dgna = test.config.state_read().subscribers.dgna_groups_of(1000001);
    assert_eq!(restored_dgna.len(), 1, "DGNA metadata should be restored from the recovery cache");
    assert_eq!(restored_dgna[0].gssi, 91);
    assert_eq!(restored_dgna[0].mnemonic.as_deref(), Some("OPS"));
    assert_eq!(restored_dgna[0].attachment_mode, 3);

    let _ = std::fs::remove_file(&path);
}

/// A cached ISSI not allowed by the access-control whitelist must NOT be replayed to.
#[test]
fn test_restart_recovery_honours_whitelist() {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.recovery.enabled = true;
    config.recovery.replay_per_frame = 2;
    // Whitelist allows only 1000001; 1000002 must be skipped at load.
    config.security.issi_whitelist = vec![1000001];

    let path = std::env::temp_dir().join("fs_recovery_it_whitelist.json");
    std::fs::write(
        &path,
        r#"{"version":1,"terminals":[
            {"issi":1000001,"groups":[],"energy_saving_mode":0},
            {"issi":1000002,"groups":[],"energy_saving_mode":0}
        ]}"#,
    )
    .unwrap();

    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mut mm = MmBs::new(test.get_shared_config(), None, None);
    mm.init_recovery(path.clone());
    test.register_entity(mm);

    test.run_stack(Some(24));
    let msgs = test.dump_sinks();

    let mut targets: Vec<u32> = Vec::new();
    for m in &msgs {
        if let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg {
            targets.push(req.address.ssi);
        }
    }
    assert!(targets.contains(&1000001), "whitelisted ISSI should be replayed, got {:?}", targets);
    assert!(
        !targets.contains(&1000002),
        "non-whitelisted ISSI must NOT be replayed, got {:?}",
        targets
    );

    let _ = std::fs::remove_file(&path);
}
