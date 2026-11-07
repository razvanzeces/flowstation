//! Multi Network: migration into this SwMI (TS 100 392-2 clause 16.4.1.1 cases b) and c)),
//! (V)ASSI (16.4.7) and (V)GSSI (16.8.7).
mod common;

use tetra_config::bluestation::{CfgMultiNetwork, DEFAULT_VASSI_RANGE, DEFAULT_VGSSI_RANGE, Mni, StackConfig, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
use tetra_pdus::mm::enums::mm_pdu_type_dl::MmPduTypeDl;
use tetra_pdus::mm::enums::reject_cause::RejectCause;
use tetra_pdus::mm::fields::group_identity_downlink::GroupIdentityDownlink;
use tetra_pdus::mm::fields::group_identity_location_demand::GroupIdentityLocationDemand;
use tetra_pdus::mm::fields::group_identity_uplink::GroupIdentityUplink;
use tetra_pdus::mm::pdus::d_attach_detach_group_identity_acknowledgement::DAttachDetachGroupIdentityAcknowledgement;
use tetra_pdus::mm::pdus::d_location_update_accept::DLocationUpdateAccept;
use tetra_pdus::mm::pdus::d_location_update_proceeding::DLocationUpdateProceeding;
use tetra_pdus::mm::pdus::u_attach_detach_group_identity::UAttachDetachGroupIdentity;
use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
use tetra_saps::lmm::LmmMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};

use crate::common::ComponentTest;

/// The test stack's own network (default_stack.rs).
const OWN: Mni = Mni { mcc: 204, mnc: 1337 };
const HOME: Mni = Mni { mcc: 901, mnc: 1234 };
const OTHER: Mni = Mni { mcc: 232, mnc: 5 };

fn multi_network_config(cell: u8) -> StackConfig {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.migration = true;
    config.cell_id = tetra_core::CellId(cell);
    config.multi_network = CfgMultiNetwork {
        enabled: true,
        networks: vec![HOME],
        ..Default::default()
    };
    config
}

fn mm_test(config: StackConfig) -> ComponentTest {
    let mut test = ComponentTest::from_config(config, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Mm], vec![TetraEntity::Mle, TetraEntity::Cmce]);
    test
}

fn demand(lu_type: LocationUpdateType, ssi: Option<u32>, mni: Option<Mni>, groups: Vec<GroupIdentityUplink>) -> ULocationUpdateDemand {
    ULocationUpdateDemand {
        location_update_type: lu_type,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: ssi.map(u64::from),
        address_extension: mni.map(|m| m.address_extension() as u64),
        group_identity_location_demand: (!groups.is_empty()).then(|| GroupIdentityLocationDemand {
            group_identity_attach_detach_mode: 0,
            group_identity_uplink: Some(groups),
        }),
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    }
}

fn submit(test: &mut ComponentTest, address: TetraAddress, write: impl FnOnce(&mut BitBuffer)) {
    let mut sdu = BitBuffer::new_autoexpand(64);
    write(&mut sdu);
    sdu.seek(0);
    test.submit_message(SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Mm,
        msg: SapMsgInner::LmmMleUnitdataInd(LmmMleUnitdataInd {
            sdu,
            handle: 0,
            received_address: address,
        }),
    });
    test.run_stack(Some(2));
}

fn submit_demand(test: &mut ComponentTest, address: TetraAddress, pdu: ULocationUpdateDemand) {
    submit(test, address, |b| pdu.to_bitbuf(b).expect("serialize U-LOCATION UPDATE DEMAND"));
}

/// (layer 2 address, sdu) of the first MM downlink PDU of a type.
fn downlink(msgs: &[SapMsg], pdu_type: MmPduTypeDl) -> Option<(TetraAddress, BitBuffer)> {
    msgs.iter().find_map(|m| {
        let SapMsgInner::LmmMleUnitdataReq(ref req) = m.msg else {
            return None;
        };
        let sdu = BitBuffer::from_bitstr(&req.sdu.to_bitstr());
        (sdu.peek_bits(4) == Some(pdu_type.into_raw())).then_some((req.address, sdu))
    })
}

fn reject_cause(msgs: &[SapMsg]) -> Option<(TetraAddress, u8)> {
    let (addr, mut sdu) = downlink(msgs, MmPduTypeDl::DLocationUpdateReject)?;
    sdu.read_field(4, "pdu_type").ok()?;
    sdu.read_field(3, "location_update_type").ok()?;
    Some((addr, sdu.read_field(5, "reject_cause").ok()? as u8))
}

fn ussi(issi: u32) -> TetraAddress {
    TetraAddress::new(issi, SsiType::Ussi)
}

fn home_group(gssi: u32) -> GroupIdentityUplink {
    GroupIdentityUplink {
        class_of_usage: Some(4),
        group_identity_detachment_uplink: None,
        gssi: Some(gssi),
        address_extension: Some(HOME.address_extension()),
        vgssi: None,
    }
}

fn own_group(gssi: u32) -> GroupIdentityUplink {
    GroupIdentityUplink {
        class_of_usage: Some(4),
        group_identity_detachment_uplink: None,
        gssi: Some(gssi),
        address_extension: None,
        vgssi: None,
    }
}

/// First step of case b): returns the (V)ASSI from the D-LOCATION UPDATE PROCEEDING.
fn identity_exchange(test: &mut ComponentTest, issi: u32) -> u32 {
    submit_demand(test, ussi(issi), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(HOME), vec![]));
    let msgs = test.dump_sinks();
    let (addr, mut sdu) = downlink(&msgs, MmPduTypeDl::DLocationUpdateProceeding).expect("D-LOCATION UPDATE PROCEEDING");
    assert_eq!(addr, ussi(issi), "answered on the USSI (clause 16.4.1.1 b), 23.4.1.2.5)");
    let proceeding = DLocationUpdateProceeding::from_bitbuf(&mut sdu).expect("parses");
    assert_eq!(proceeding.address_extension, HOME.address_extension(), "carries the MNI of the MS");
    proceeding.ssi
}

/// Both steps of case b), the second with `groups`: returns (V)ASSI and the accept.
fn migrate(test: &mut ComponentTest, issi: u32, groups: Vec<GroupIdentityUplink>) -> (u32, DLocationUpdateAccept) {
    let vassi = identity_exchange(test, issi);
    submit_demand(
        test,
        TetraAddress::issi(vassi),
        demand(LocationUpdateType::DemandLocationUpdating, Some(issi), Some(HOME), groups),
    );
    let msgs = test.dump_sinks();
    let (addr, mut sdu) = downlink(&msgs, MmPduTypeDl::DLocationUpdateAccept).expect("D-LOCATION UPDATE ACCEPT");
    assert_eq!(addr.ssi, vassi);
    (vassi, DLocationUpdateAccept::from_bitbuf(&mut sdu).expect("parses"))
}

fn giat(g: &GroupIdentityDownlink) -> (Option<u32>, Option<u32>, Option<u32>) {
    (g.gssi, g.address_extension, g.vgssi)
}

#[test]
fn test_migrating_visitor_gets_a_vassi_and_registers_on_it() {
    debug::setup_logging_verbose();
    const VISITOR: u32 = 2320972;
    let mut test = mm_test(multi_network_config(0));
    let (vassi, accept) = migrate(&mut test, VISITOR, vec![]);
    assert!((DEFAULT_VASSI_RANGE.0..=DEFAULT_VASSI_RANGE.1).contains(&vassi));
    assert_eq!(accept.ssi, Some(vassi as u64));
    assert_eq!(accept.address_extension, Some(HOME.address_extension() as u64), "MNI checked by the MS");
    assert_eq!(accept.location_update_accept_type, LocationUpdateType::DemandLocationUpdating);
    let state = test.config.state_read();
    assert!(state.subscribers.is_registered(vassi));
    assert!(!state.subscribers.is_registered(VISITOR), "the home ISSI is not an identity here");
}

#[test]
fn test_visitor_home_groups_get_a_vgssi_and_own_groups_keep_their_gssi() {
    debug::setup_logging_verbose();
    const VISITOR: u32 = 2320972;
    let mut test = mm_test(multi_network_config(0));
    let (vassi, accept) = migrate(&mut test, VISITOR, vec![home_group(9), own_group(9)]);
    let gid = accept.group_identity_location_accept.expect("group identity location accept").group_identity_downlink.unwrap();
    let home = gid.iter().find(|g| g.vgssi.is_some()).expect("home group answered with GIAT 3");
    let vgssi = home.vgssi.unwrap();
    assert!((DEFAULT_VGSSI_RANGE.0..=DEFAULT_VGSSI_RANGE.1).contains(&vgssi));
    assert_eq!(giat(home), (Some(9), Some(HOME.address_extension()), Some(vgssi)));
    assert!(gid.iter().any(|g| giat(g) == (Some(9), None, None)), "group 9 of this network: GIAT 0");
    let attached = test.config.state_read().subscribers.attached_groups_of(vassi);
    assert!(attached.contains(&vgssi) && attached.contains(&9), "{attached:?}");

    // Attaching by the (V)GSSI (GIAT 2) is answered with the (V)GSSI alone.
    let pdu = UAttachDetachGroupIdentity {
        group_identity_report: false,
        group_identity_attach_detach_mode: false,
        group_report_response: None,
        group_identity_uplink: Some(vec![GroupIdentityUplink {
            class_of_usage: Some(4),
            group_identity_detachment_uplink: None,
            gssi: None,
            address_extension: None,
            vgssi: Some(vgssi),
        }]),
        proprietary: None,
    };
    submit(&mut test, TetraAddress::issi(vassi), |b| pdu.to_bitbuf(b).expect("serialize"));
    let msgs = test.dump_sinks();
    let (_, mut sdu) = downlink(&msgs, MmPduTypeDl::DAttachDetachGroupIdentityAcknowledgement).expect("acknowledgement");
    let ack = DAttachDetachGroupIdentityAcknowledgement::from_bitbuf(&mut sdu).expect("parses");
    let gid = ack.group_identity_downlink.unwrap();
    assert_eq!(gid.iter().map(giat).collect::<Vec<_>>(), vec![(None, None, Some(vgssi))]);
}

#[test]
fn test_migration_stays_refused_when_multi_network_is_off_and_answers_on_the_ussi() {
    debug::setup_logging_verbose();
    let mut config = multi_network_config(0);
    config.multi_network.enabled = false;
    let mut test = mm_test(config);
    submit_demand(&mut test, ussi(2320972), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(HOME), vec![]));
    let msgs = test.dump_sinks();
    assert!(downlink(&msgs, MmPduTypeDl::DLocationUpdateProceeding).is_none());
    let (addr, cause) = reject_cause(&msgs).expect("rejected");
    assert_eq!(cause, RejectCause::MigrationNotSupported as u8);
    assert_eq!(addr, ussi(2320972), "the reject goes to the USSI (clause 16.4.1.1 b))");
}

#[test]
fn test_multi_network_on_supports_migration_without_cell_migration() {
    debug::setup_logging_verbose();
    let mut config = multi_network_config(0);
    config.cell.migration = false;
    assert!(config.migration_supported(), "[multi_network] on turns migration on");
    let mut test = mm_test(config);
    let (vassi, _) = migrate(&mut test, 2320972, vec![]);
    assert!(test.config.state_read().subscribers.is_registered(vassi));
}

#[test]
fn test_migration_is_refused_from_an_unlisted_network() {
    debug::setup_logging_verbose();
    let mut test = mm_test(multi_network_config(0));
    submit_demand(&mut test, ussi(2), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(OTHER), vec![]));
    assert_eq!(reject_cause(&test.dump_sinks()).map(|r| r.1), Some(RejectCause::MigrationNotSupported as u8));
}

#[test]
fn test_radio_back_from_another_network_registers_as_case_c() {
    debug::setup_logging_verbose();
    const OWN_RADIO: u32 = 2320980;
    let mut test = mm_test(multi_network_config(0));
    submit_demand(
        &mut test,
        TetraAddress::issi(OWN_RADIO),
        demand(LocationUpdateType::MigratingLocationUpdating, None, Some(OWN), vec![own_group(9)]),
    );
    let msgs = test.dump_sinks();
    assert!(reject_cause(&msgs).is_none());
    let (addr, mut sdu) = downlink(&msgs, MmPduTypeDl::DLocationUpdateAccept).expect("accepted");
    assert_eq!(addr.ssi, OWN_RADIO);
    let accept = DLocationUpdateAccept::from_bitbuf(&mut sdu).expect("parses");
    // Table 16.68: 101 = "migrating or service restoration migrating location updating".
    assert_eq!(accept.location_update_accept_type.into_raw(), 0b101);
    assert!(test.config.state_read().subscribers.attached_groups_of(OWN_RADIO).contains(&9));
}

#[test]
fn test_second_demand_naming_another_itsi_is_rejected() {
    debug::setup_logging_verbose();
    let mut test = mm_test(multi_network_config(0));
    let vassi = identity_exchange(&mut test, 2320972);
    submit_demand(
        &mut test,
        TetraAddress::issi(vassi),
        demand(LocationUpdateType::DemandLocationUpdating, Some(2320973), Some(HOME), vec![]),
    );
    let msgs = test.dump_sinks();
    assert_eq!(reject_cause(&msgs).map(|r| r.1), Some(RejectCause::ItsiAtsiUnknown as u8));
    assert!(!test.config.state_read().subscribers.is_registered(vassi));
}

#[test]
fn test_whitelist_checks_the_home_issi_and_admits_the_vassi() {
    debug::setup_logging_verbose();
    let mut config = multi_network_config(0);
    config.security.issi_whitelist = vec![2320972];
    let mut test = mm_test(config);
    let (vassi, _) = migrate(&mut test, 2320972, vec![]);
    assert!(test.config.state_read().subscribers.is_registered(vassi));
    submit_demand(&mut test, ussi(2320973), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(HOME), vec![]));
    assert_eq!(reject_cause(&test.dump_sinks()).map(|r| r.1), Some(RejectCause::ItsiAtsiUnknown as u8));
}

#[test]
fn test_migrating_again_replaces_the_vassi() {
    debug::setup_logging_verbose();
    let mut test = mm_test(multi_network_config(0));
    let (first, _) = migrate(&mut test, 2320972, vec![]);
    let (second, _) = migrate(&mut test, 2320972, vec![]);
    assert_ne!(first, second, "clause 16.4.7.1: a migration makes the old (V)ASSI invalid");
    let state = test.config.state_read();
    assert!(!state.subscribers.is_registered(first));
    assert!(state.subscribers.is_registered(second));
}

/// Internal simulation: 10 stations (cells 0..7 of one station and two more stations), 25 radios
/// each, visitors of one network mixed with radios of this network, some of them back from
/// another network (case c). Every visitor gets its own (V)ASSI, never shared between cells,
/// and one (V)GSSI per home group within a cell.
#[test]
fn test_simulation_ten_stations_twenty_five_radios() {
    debug::setup_logging_verbose();
    let mut seen_vassi = std::collections::HashSet::new();
    for station in 0..10u8 {
        let cell = station % 8;
        let mut test = mm_test(multi_network_config(cell));
        let mut vgssi_of_9 = None;
        for radio in 0..25u32 {
            let issi = 2_320_000 + u32::from(station) * 100 + radio;
            match radio % 3 {
                0 | 1 => {
                    let (vassi, accept) = migrate(&mut test, issi, vec![home_group(9)]);
                    assert_eq!((vassi - DEFAULT_VASSI_RANGE.0) % 8, u32::from(cell), "cell block");
                    if station < 8 {
                        assert!(seen_vassi.insert(vassi), "cells of one station never share a (V)ASSI");
                    }
                    let gid = accept.group_identity_location_accept.unwrap().group_identity_downlink.unwrap();
                    let vgssi = gid[0].vgssi.expect("GIAT 3");
                    assert_eq!(*vgssi_of_9.get_or_insert(vgssi), vgssi, "one (V)GSSI per home group");
                }
                _ => {
                    submit_demand(
                        &mut test,
                        TetraAddress::issi(issi),
                        demand(LocationUpdateType::MigratingLocationUpdating, None, Some(OWN), vec![own_group(9)]),
                    );
                    let msgs = test.dump_sinks();
                    assert!(downlink(&msgs, MmPduTypeDl::DLocationUpdateAccept).is_some(), "case c accepted");
                    assert!(test.config.state_read().subscribers.attached_groups_of(issi).contains(&9));
                }
            }
        }
        assert_eq!(test.config.state_read().subscribers.attached_groups_of(2_320_000 + u32::from(station) * 100 + 2), vec![9]);
    }
}

/// With a Brew server that offers `mni` a visitor's network has to be in the server's list and
/// up (its `SERVICE_NETWORKS`); `networks` in the config, when set, still has to list it.
#[test]
fn test_migration_follows_the_server_network_list() {
    debug::setup_logging_verbose();
    use tetra_config::bluestation::BrewNetwork;
    let set = |test: &mut ComponentTest, up: bool| {
        test.config.state_write().multi_network.server = Some((OWN, vec![BrewNetwork { mni: HOME, name: "Home".into(), up }]));
    };
    let first = |test: &mut ComponentTest, issi: u32| {
        submit_demand(test, ussi(issi), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(HOME), vec![]));
        let msgs = test.dump_sinks();
        downlink(&msgs, MmPduTypeDl::DLocationUpdateProceeding).is_some()
    };
    let mut test = mm_test(multi_network_config(0));
    set(&mut test, false);
    assert!(!first(&mut test, 1), "home network down at the server: refused");
    set(&mut test, true);
    assert!(first(&mut test, 2));

    let mut config = multi_network_config(0);
    config.multi_network.networks.clear();
    let mut test = mm_test(config.clone());
    assert!(!first(&mut test, 3), "no list anywhere: refused");
    set(&mut test, true);
    assert!(first(&mut test, 4), "the server's list is enough");

    config.multi_network.networks = vec![OTHER];
    let mut test = mm_test(config);
    set(&mut test, true);
    assert!(!first(&mut test, 5), "not in the configured list");
}

/// A cell of another network reached `via` the server (its own network differs from the
/// server's): a radio of the server's own network migrates onto it even though the server's
/// network list names only its links.
#[test]
fn test_radio_of_the_servers_own_network_migrates_onto_a_cell_of_another_network() {
    use tetra_config::bluestation::BrewNetwork;
    let mut config = multi_network_config(0);
    config.multi_network.networks.clear();
    let mut test = mm_test(config);
    test.config.state_write().multi_network.server = Some((HOME, vec![BrewNetwork { mni: OTHER, name: "Other".into(), up: true }]));
    submit_demand(&mut test, ussi(7), demand(LocationUpdateType::MigratingLocationUpdating, None, Some(HOME), vec![]));
    let msgs = test.dump_sinks();
    assert!(downlink(&msgs, MmPduTypeDl::DLocationUpdateProceeding).is_some(), "identity exchange for a radio of the server's network");
}

/// A visitor is visible to the Brew side (shared state) once its second demand passes, and
/// leaves it when it detaches.
#[test]
fn test_visitor_is_shared_with_the_brew_side_until_it_detaches() {
    debug::setup_logging_verbose();
    let mut test = mm_test(multi_network_config(0));
    let (vassi, accept) = migrate(&mut test, 2320972, vec![home_group(9)]);
    let vgssi = accept.group_identity_location_accept.unwrap().group_identity_downlink.unwrap()[0].vgssi.unwrap();
    {
        let st = test.config.state_read();
        assert_eq!(st.multi_network.visitors.get(&vassi), Some(&(HOME, 2320972)));
        assert_eq!(st.multi_network.groups.get(&vgssi), Some(&(HOME, 9)));
    }
    let detach = tetra_pdus::mm::pdus::u_itsi_detach::UItsiDetach { address_extension: None, proprietary: None };
    submit(&mut test, TetraAddress::issi(vassi), |b| detach.to_bitbuf(b).expect("serialize"));
    assert!(!test.config.state_read().multi_network.visitors.contains_key(&vassi));
}

/// A (V)ASSI this station never gave (the visitor came from another station or cell, or the
/// station restarted) is not taken for a radio of this network: "ITSI/ATSI unknown" makes it
/// invalid in the MS (clause 16.4.7.3).
#[test]
fn test_unknown_vassi_is_rejected() {
    debug::setup_logging_verbose();
    let mut test = mm_test(multi_network_config(0));
    let stranger = DEFAULT_VASSI_RANGE.0 + 9;
    submit_demand(&mut test, TetraAddress::issi(stranger), demand(LocationUpdateType::RoamingLocationUpdating, None, None, vec![]));
    let msgs = test.dump_sinks();
    assert_eq!(reject_cause(&msgs).map(|r| r.1), Some(RejectCause::ItsiAtsiUnknown as u8));
    assert!(!test.config.state_read().subscribers.is_registered(stranger));
}

/// A radio whose home changed to another network answers a D-LOCATION UPDATE COMMAND with a
/// demand naming its new MNI (seen in the field, Motorola MTP6650). It holds no valid
/// registration here (clause 16.4.1.1 b)), so it is not registered under its bare ISSI but
/// rejected with "ITSI/ATSI unknown" (16.4.7.3) and then registers again as a migrating radio.
#[test]
fn test_non_migrating_update_of_another_network_is_rejected() {
    debug::setup_logging_verbose();
    const RADIO: u32 = 2320972;
    for lu_type in [LocationUpdateType::DemandLocationUpdating, LocationUpdateType::RoamingLocationUpdating] {
        let mut test = mm_test(multi_network_config(0));
        submit_demand(&mut test, TetraAddress::issi(RADIO), demand(lu_type, Some(RADIO), Some(HOME), vec![own_group(9)]));
        let msgs = test.dump_sinks();
        assert!(downlink(&msgs, MmPduTypeDl::DLocationUpdateAccept).is_none(), "{lu_type:?} not accepted");
        let (addr, cause) = reject_cause(&msgs).expect("rejected");
        assert_eq!(cause, RejectCause::ItsiAtsiUnknown as u8);
        assert_eq!(addr, TetraAddress::issi(RADIO), "answered on the address it came on");
        assert!(!test.config.state_read().subscribers.is_registered(RADIO));
    }
    // A radio of this network naming its own MNI registers as before.
    let mut test = mm_test(multi_network_config(0));
    submit_demand(
        &mut test,
        TetraAddress::issi(RADIO),
        demand(LocationUpdateType::DemandLocationUpdating, Some(RADIO), Some(OWN), vec![]),
    );
    assert!(downlink(&test.dump_sinks(), MmPduTypeDl::DLocationUpdateAccept).is_some());
    // Multi Network off: unchanged, the update is accepted as before.
    let mut config = multi_network_config(0);
    config.multi_network.enabled = false;
    let mut test = mm_test(config);
    submit_demand(
        &mut test,
        TetraAddress::issi(RADIO),
        demand(LocationUpdateType::DemandLocationUpdating, Some(RADIO), Some(HOME), vec![]),
    );
    assert!(downlink(&test.dump_sinks(), MmPduTypeDl::DLocationUpdateAccept).is_some());
}

/// The Brew server's group-access rules name a radio of its own network by its home ISSI: a
/// radio of that network that migrated onto a cell of another network is checked under that
/// ISSI, not its (V)ASSI, when it attaches and when it talks.
#[test]
fn test_group_access_checks_a_migrated_radio_of_the_servers_network_by_its_home_issi() {
    use tetra_config::bluestation::BrewGroupAccess;
    debug::setup_logging_verbose();
    const RADIO: u32 = 2320972;
    const GSSI: u32 = 2326406;
    let mut config = multi_network_config(0);
    config.multi_network.networks.clear();
    let mut test = mm_test(config);
    {
        let mut state = test.config.state_write();
        state.multi_network.server = Some((HOME, vec![]));
        state.brew_group_access.insert(GSSI, BrewGroupAccess { gssi: GSSI, issis: vec![RADIO], ..Default::default() });
    }
    let (vassi, accept) = migrate(&mut test, RADIO, vec![own_group(GSSI)]);
    let groups = accept.group_identity_location_accept.expect("groups answered").group_identity_downlink.unwrap_or_default();
    assert!(
        groups.iter().all(|g| g.group_identity_detachment_uplink.is_none()),
        "the group is not refused: {groups:?}"
    );
    let state = test.config.state_read();
    assert!(state.group_may_talk(GSSI, vassi) && state.group_may_join(GSSI, vassi), "checked by its home ISSI");
    assert!(!state.group_may_join(GSSI, 4013), "others stay refused");
}
