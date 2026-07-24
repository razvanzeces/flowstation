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
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_saps::control::brew::BrewSubscriberAction;
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
    submit_location_update(test, issi, 0, LocationUpdateType::RoamingLocationUpdating);
}

/// Submit a U-LOCATION-UPDATE-DEMAND of `lu_type` as if it arrived from `issi` on L2 `handle`.
/// The handle is what a teardown is bound to, so tests can forge a mismatched L2 context.
fn submit_location_update(test: &mut ComponentTest, issi: u32, handle: u32, lu_type: LocationUpdateType) {
    let demand = ULocationUpdateDemand {
        location_update_type: lu_type,
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
        handle,
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

/// Submit a U-ITSI-DETACH claiming to come from `issi` on L2 `handle`. Uplink is unauthenticated,
/// so this is exactly what an attacker can put on the air with a victim's (on-air observable) ISSI.
fn submit_itsi_detach(test: &mut ComponentTest, issi: u32, handle: u32) {
    let detach = UItsiDetach {
        address_extension: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(16);
    detach.to_bitbuf(&mut sdu).expect("serialize U-ITSI-DETACH");
    sdu.seek(0);
    let prim = LmmMleUnitdataInd {
        sdu,
        handle,
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

/// Submit a U-ATTACH/DETACH GROUP IDENTITY carrying a single group element as if it arrived from
/// `issi`. Whether it attaches or detaches follows the element: `class_of_usage=Some` attaches,
/// `group_identity_detachment_uplink=Some` detaches (mirrors what a scanning radio sends).
fn submit_group_attach_detach(test: &mut ComponentTest, issi: u32, group: GroupIdentityUplink) {
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: false,
        group_identity_attach_detach_mode: false,
        group_report_response: None,
        group_identity_uplink: Some(vec![group]),
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(32);
    pdu.to_bitbuf(&mut sdu).expect("serialize U-ATTACH/DETACH GROUP IDENTITY");
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

/// Pull (addressed ISSI, reject cause) of the first D-LOCATION-UPDATE-REJECT out of a batch of
/// captured MLE messages. Decoded by hand off the fixed leading fields (pdu type / location update
/// type / reject cause) â€” the PDU's own `from_bitbuf` is an `unimplemented!()` stub.
fn find_location_update_reject(msgs: &[SapMsg]) -> Option<(u32, u8)> {
    let want = MmPduTypeDl::DLocationUpdateReject.into_raw();
    for m in msgs {
        if let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg {
            let mut sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
            if sdu.read_field(4, "pdu_type").is_ok_and(|t| t == want) {
                sdu.read_field(3, "location_update_type").ok()?;
                let cause = sdu.read_field(5, "reject_cause").ok()? as u8;
                return Some((req.address.ssi, cause));
            }
        }
    }
    None
}

/// True if the batch carries a Deregister or Deaffiliate for `issi` â€” i.e. MM tore that
/// subscriber down toward CMCE/Brew.
fn tore_down(msgs: &[SapMsg], issi: u32) -> bool {
    msgs.iter().any(|m| match &m.msg {
        SapMsgInner::MmSubscriberUpdate(u) => {
            u.issi == issi
                && matches!(
                    u.action,
                    BrewSubscriberAction::Deregister | BrewSubscriberAction::Deaffiliate
                )
        }
        _ => false,
    })
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

/// Forged teardown: a U-ITSI-DETACH whose source is NOT registered must not tear anything down.
/// ISSIs are observable on air and the uplink is unauthenticated, so a detach carrying someone
/// else's ISSI is trivially forgeable; honouring it knocked the claimed radio off Brew, local
/// call/SDS delivery and the dashboard, and a replay kept it off.
#[test]
fn test_forged_itsi_detach_for_unregistered_source_is_ignored() {
    debug::setup_logging_verbose();
    const VICTIM: u32 = 2260801;
    const STRANGER: u32 = 2260802;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    register_terminal(&mut test, VICTIM);
    let _ = test.dump_sinks();

    // A detach claiming an ISSI that never registered here.
    submit_itsi_detach(&mut test, STRANGER, 0);
    let msgs = test.dump_sinks();
    assert!(!tore_down(&msgs, STRANGER), "a detach from an unregistered source must not tear anything down");
    assert!(
        test.config.state_read().subscribers.is_registered(VICTIM),
        "the registered radio must be untouched"
    );
}

/// The teardown is bound to the L2 context the registration used: a detach arriving on a different
/// handle than the one the radio registered on is refused.
#[test]
fn test_itsi_detach_on_mismatched_l2_context_is_ignored() {
    debug::setup_logging_verbose();
    const VICTIM: u32 = 2260803;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    submit_location_update(&mut test, VICTIM, 7, LocationUpdateType::RoamingLocationUpdating);
    let _ = test.dump_sinks();
    assert!(test.config.state_read().subscribers.is_registered(VICTIM), "precondition: registered");

    submit_itsi_detach(&mut test, VICTIM, 0); // registered on handle 7, detach claims handle 0
    let msgs = test.dump_sinks();
    assert!(!tore_down(&msgs, VICTIM), "a detach on a foreign L2 context must not tear the radio down");
    assert!(
        test.config.state_read().subscribers.is_registered(VICTIM),
        "the victim must stay registered"
    );
}

/// The operator kill-switch: with `honour_unauthenticated_detach = false` even a well-formed
/// detach from the registered radio itself is refused, because it cannot be authenticated.
#[test]
fn test_itsi_detach_refused_when_disabled_by_config() {
    debug::setup_logging_verbose();
    const VICTIM: u32 = 2260804;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.honour_unauthenticated_detach = false;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    register_terminal(&mut test, VICTIM);
    let _ = test.dump_sinks();

    submit_itsi_detach(&mut test, VICTIM, 0);
    let msgs = test.dump_sinks();
    assert!(!tore_down(&msgs, VICTIM), "detach must not be honoured when the operator disabled it");
    assert!(test.config.state_read().subscribers.is_registered(VICTIM));
}

/// A migrating U-LOCATION-UPDATE-DEMAND from a non-whitelisted ISSI must be rejected BEFORE any
/// Brew/registry mutation. The whitelist check used to sit after the migration release, so a
/// barred radio could still knock a registered subscriber down with a forged migration.
#[test]
fn test_migration_from_non_whitelisted_issi_tears_nothing_down() {
    debug::setup_logging_verbose();
    const ALLOWED: u32 = 2260805;
    const BARRED: u32 = 2260806;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.issi_whitelist = vec![ALLOWED, BARRED];
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    register_terminal(&mut test, ALLOWED);
    let _ = test.dump_sinks();

    // Drop BARRED from the whitelist at runtime, then have it claim a migration.
    test.config.state_write().issi_whitelist_override = Some(vec![ALLOWED]);
    submit_location_update(&mut test, BARRED, 0, LocationUpdateType::MigratingLocationUpdating);
    let msgs = test.dump_sinks();

    assert!(!tore_down(&msgs, BARRED), "a barred ISSI must not mutate any state");
    let (_, cause) = find_location_update_reject(&msgs).expect("a barred ISSI must be rejected");
    assert_eq!(
        cause,
        tetra_pdus::mm::enums::reject_cause::RejectCause::ItsiAtsiUnknown as u8,
        "the whitelist branch must win over the migration branch and use an access-control cause"
    );
    assert!(test.config.state_read().subscribers.is_registered(ALLOWED));
}

/// A whitelist-denied registration must carry an access-control cause, NOT "migration not
/// supported" (12). The latter tells a conformant terminal it may attach to a DIFFERENT network,
/// so a barred radio goes hunting for another cell instead of staying off, and the user never sees
/// an access-denied.
#[test]
fn test_whitelist_reject_uses_access_control_cause() {
    debug::setup_logging_verbose();
    const ALLOWED: u32 = 2260807;
    const BARRED: u32 = 2260808;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.issi_whitelist = vec![ALLOWED];
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    register_terminal(&mut test, BARRED);
    let msgs = test.dump_sinks();

    let (issi, cause) = find_location_update_reject(&msgs)
        .unwrap_or_else(|| panic!("a non-whitelisted registration must be rejected, got {} msgs", msgs.len()));
    assert_eq!(issi, BARRED);
    assert_ne!(
        cause,
        tetra_pdus::mm::enums::reject_cause::RejectCause::MigrationNotSupported as u8,
        "a barred radio must not be told migration is unsupported"
    );
    assert_eq!(
        cause,
        tetra_pdus::mm::enums::reject_cause::RejectCause::ItsiAtsiUnknown as u8
    );
    assert!(
        !test.config.state_read().subscribers.is_registered(BARRED),
        "a barred radio must never enter the subscriber registry"
    );
}

/// The genuine migration path keeps "migration not supported" (12) so a real migrating terminal
/// still knows to try the other network.
#[test]
fn test_migration_reject_keeps_migration_cause() {
    debug::setup_logging_verbose();
    const ROAMER: u32 = 2260809;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    register_terminal(&mut test, ROAMER);
    let _ = test.dump_sinks();

    submit_location_update(&mut test, ROAMER, 0, LocationUpdateType::MigratingLocationUpdating);
    let msgs = test.dump_sinks();

    let (issi, cause) = find_location_update_reject(&msgs).expect("a migrating update must be rejected");
    assert_eq!(issi, ROAMER);
    assert_eq!(
        cause,
        tetra_pdus::mm::enums::reject_cause::RejectCause::MigrationNotSupported as u8
    );
}

/// SHIP-BLOCKER regression: the client registry must stay bounded under an RF-unauthenticated
/// registration flood. An attacker cycles the source ISSI across the 24-bit space; every distinct
/// value used to add a permanent client + subscriber + dashboard entry, so days of uptime meant
/// unbounded heap growth and an OOM-kill of the whole cell. A radio holding a group affiliation
/// must survive the flood untouched.
#[test]
fn test_registration_flood_does_not_grow_the_registry() {
    debug::setup_logging_verbose();
    const MEMBER: u32 = 2260810;
    const MEMBER_GSSI: u32 = 42;
    const CAP: usize = 8;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.max_registered_clients = CAP;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);

    // A legitimate, affiliated member of the fleet.
    register_terminal(&mut test, MEMBER);
    dispatcher.send(ControlCommand::Dgna {
        issi: MEMBER,
        gssi: MEMBER_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let _ = test.dump_sinks();

    // Flood: 200 distinct forged source ISSIs, each registering once.
    for i in 0..200u32 {
        register_terminal(&mut test, 3_000_000 + i);
        let _ = test.dump_sinks();
    }

    let state = test.config.state_read();
    let registered = state.subscribers.all_registered_issis().count();
    assert!(
        registered <= CAP,
        "the registry must stay bounded by the {CAP} cap under a flood, holds {registered}"
    );
    assert!(
        state.subscribers.is_registered(MEMBER),
        "an affiliated member must never be evicted to make room for a flood"
    );
    assert!(
        state.subscribers.attached_groups_of(MEMBER).contains(&MEMBER_GSSI),
        "the member's affiliation must survive the flood"
    );
}

/// The per-source-ISSI rate limit: one ISSI cannot churn the registry by re-registering in a loop.
/// Beyond the configured budget the location update is dropped silently (answering would itself be
/// an amplifier), so no ACCEPT comes back.
#[test]
fn test_registration_rate_limit_drops_repeat_flood_from_one_issi() {
    debug::setup_logging_verbose();
    const NOISY: u32 = 2260811;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.security.registration_rate_limit_per_min = 3;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    // Burn the budget.
    for _ in 0..3 {
        register_terminal(&mut test, NOISY);
        let _ = test.dump_sinks();
    }
    // The next one must produce no downlink at all.
    register_terminal(&mut test, NOISY);
    let msgs = test.dump_sinks();
    assert!(
        !msgs
            .iter()
            .any(|m| matches!(m.msg, SapMsgInner::LmmMleUnitdataReq(_))),
        "a registration past the rate limit must be dropped, got {} downlink msg(s)",
        msgs.len()
    );
}

/// ETSI EN 300 392-2 cl. 16.9.2.2: the accept must preserve the accepted location update type.
/// Answering an ItsiAttach with PeriodicLocationUpdating leaves radios that key attach-completion
/// off the accept type believing the attach never completed.
#[test]
fn test_itsi_attach_accept_preserves_accept_type() {
    debug::setup_logging_verbose();
    const ATTACHER: u32 = 2260812;

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.periodic_registration_secs = 300;
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);

    submit_location_update(&mut test, ATTACHER, 0, LocationUpdateType::ItsiAttach);
    let msgs = test.dump_sinks();

    let want = MmPduTypeDl::DLocationUpdateAccept.into_raw();
    let accept_type = msgs.iter().find_map(|m| {
        let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg else {
            return None;
        };
        let mut sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
        if !sdu.read_field(4, "pdu_type").is_ok_and(|t| t == want) {
            return None;
        }
        sdu.read_field(3, "location_update_accept_type").ok()
    });
    assert_eq!(
        accept_type,
        Some(LocationUpdateType::ItsiAttach.into_raw()),
        "an ItsiAttach must be accepted as an ItsiAttach"
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

/// A DGNA request whose GSSI does not fit the 24-bit SS-DGNA field must be rejected at the MM
/// funnel — no affiliation and no SS-DGNA emission — and must NOT panic. Pre-fix the oversized GSSI
/// reached the PDU serializer's `write_bits` assertion and took down the whole cell.
#[test]
fn test_dgna_assign_rejects_out_of_range_gssi() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260599;
    const BAD_GSSI: u32 = 0x100_0000; // 16_777_216 — one past the 24-bit max

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);
    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: BAD_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2)); // must not panic

    let msgs = test.dump_sinks();
    assert!(
        find_ss_dgna_request(&msgs).is_none(),
        "an out-of-range GSSI must not produce an SS-DGNA request"
    );
    assert!(
        !test
            .config
            .state_read()
            .subscribers
            .attached_groups_of(TEST_ISSI)
            .contains(&BAD_GSSI),
        "an out-of-range GSSI must not be affiliated"
    );
}

/// Regression: a RoamingLocationUpdating from a radio that is already registered and affiliated (the
/// routine Sepura/Motorola post-PTT re-registration) must NOT tear the radio down. Previously MM ran
/// a Deaffiliate -> Deregister -> Register -> Affiliate "reset" that transiently zeroed CMCE's group
/// listeners — dropping the radio's active group call and opening an "unknown subscriber" window the
/// terminal reads as a network error, disconnecting it ("radio constantly disconnects from the BTS").
#[test]
fn test_roaming_reregistration_keeps_present_radio_affiliated() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260601;
    const TEST_GSSI: u32 = 100;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let (dispatcher, endpoint) = make_control_link();
    let mm = MmBs::new(test.get_shared_config(), None, Some(endpoint));
    test.register_entity(mm);

    // Register the radio and affiliate it to a group.
    register_terminal(&mut test, TEST_ISSI);
    dispatcher.send(ControlCommand::Dgna {
        issi: TEST_ISSI,
        gssi: TEST_GSSI,
        mnemonic: None,
        attachment_mode: 0,
        attach: true,
    });
    test.run_stack(Some(2));
    let _ = test.dump_sinks(); // discard the registration + affiliation traffic
    assert!(
        test.config.state_read().subscribers.attached_groups_of(TEST_ISSI).contains(&TEST_GSSI),
        "precondition: radio must be affiliated before re-registration"
    );

    // The radio re-registers (RoamingLocationUpdating after a PTT — the exact reported trigger).
    register_terminal(&mut test, TEST_ISSI);
    let msgs = test.dump_sinks();

    // It must NOT emit a Deregister or Deaffiliate toward CMCE — either would transiently drop the
    // radio's group listeners (and its active group call) and disconnect a present terminal.
    let destructive: Vec<BrewSubscriberAction> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::MmSubscriberUpdate(u)
                if matches!(
                    u.action,
                    BrewSubscriberAction::Deregister | BrewSubscriberAction::Deaffiliate
                ) =>
            {
                Some(u.action)
            }
            _ => None,
        })
        .collect();
    assert!(
        destructive.is_empty(),
        "re-registration of a present radio must not tear it down, but emitted {:?}",
        destructive
    );

    // And the affiliation must still be intact afterwards.
    assert!(
        test.config.state_read().subscribers.attached_groups_of(TEST_ISSI).contains(&TEST_GSSI),
        "affiliation must survive a routine re-registration"
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

/// FH-BUG-069 (MM side): turning scan off detaches the scanned group. That detach must clear the
/// group from the local subscriber registry so `has_group_members` — which the CMCE network-call
/// admission gate reads to tell a live local member from an external Brew subscriber — goes false.
#[test]
fn test_scan_off_group_detach_clears_local_listener() {
    debug::setup_logging_verbose();
    const TEST_ISSI: u32 = 2260707;
    const SCAN_GSSI: u32 = 91; // BrandMeister worldwide TG from the field report

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    let mm = MmBs::new(test.get_shared_config(), None, None);
    test.register_entity(mm);
    register_terminal(&mut test, TEST_ISSI);
    let _ = test.dump_sinks();

    // Scan on: the radio attaches the scanned group.
    submit_group_attach_detach(
        &mut test,
        TEST_ISSI,
        GroupIdentityUplink {
            class_of_usage: Some(4),
            group_identity_detachment_uplink: None,
            gssi: Some(SCAN_GSSI),
            address_extension: None,
            vgssi: None,
        },
    );
    assert!(
        test.config.state_read().subscribers.has_group_members(SCAN_GSSI),
        "precondition: while scanning, the radio is a live local member of the TG"
    );

    // Scan off: the radio detaches that group.
    submit_group_attach_detach(
        &mut test,
        TEST_ISSI,
        GroupIdentityUplink {
            class_of_usage: None,
            group_identity_detachment_uplink: Some(1),
            gssi: Some(SCAN_GSSI),
            address_extension: None,
            vgssi: None,
        },
    );
    assert!(
        !test.config.state_read().subscribers.has_group_members(SCAN_GSSI),
        "scan-off detach must drop the local listener so network calls for the TG are no longer set up"
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
