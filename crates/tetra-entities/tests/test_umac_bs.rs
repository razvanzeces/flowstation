mod common;

use tetra_config::bluestation::{CfgBrew, StackMode};
use tetra_core::Direction;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_entities::umac::umac_bs::UmacBs;
use tetra_pdus::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;
use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_pdus::umac::pdus::mac_u_signal::MacUSignal;
use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::lmm::LmmMleUnitdataReq;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tma::{TmaUnitdataInd, TmaUnitdataReq};
use tetra_saps::tmd::{TmdCircuitDataInd, TmdCircuitDataReq};
use tetra_saps::tmv::{TmvUnitdataInd, enums::logical_chans::LogicalChannel};

use crate::common::ComponentTest;

const MAIN_CARRIER: u16 = 1521;
const SECONDARY_CARRIER: u16 = 1522;

fn block_has_mac_resource_for(block: &Option<tetra_saps::tmv::TmvUnitdataReq>, addr: TetraAddress, expect_chan_alloc: bool) -> bool {
    let Some(block) = block else {
        return false;
    };
    let mut mac_block = block.mac_block.clone();
    // TS1/MCCH also carries Broadcast (mac_pdu_type 2) and MAC-FRAG (1) blocks. MacResource::from_bitbuf
    // asserts the 2-bit type is 0 (MAC-RESOURCE) and would panic on the others, so peek the type first.
    let mut peek = mac_block.clone();
    if peek.read_field(2, "mac_pdu_type").map(|t| t != 0).unwrap_or(true) {
        return false;
    }
    let Ok(pdu) = MacResource::from_bitbuf(&mut mac_block) else {
        return false;
    };
    // The MAC-RESOURCE air format has no ISSI/USSI/GSSI subtype field -- an individual address
    // round-trips as a bare SsiType::Ssi. Compare the SSI value only, not the ssi_type.
    pdu.addr.map(|a| a.ssi) == Some(addr.ssi) && pdu.chan_alloc_element.is_some() == expect_chan_alloc
}

fn block_mac_resource_for(block: &Option<tetra_saps::tmv::TmvUnitdataReq>, addr: TetraAddress) -> Option<MacResource> {
    let Some(block) = block else {
        return None;
    };
    let mut mac_block = block.mac_block.clone();
    // Skip non-MAC-RESOURCE blocks (Broadcast/MAC-FRAG) that share TS1/MCCH; from_bitbuf would panic.
    let mut peek = mac_block.clone();
    if peek.read_field(2, "mac_pdu_type").map(|t| t != 0).unwrap_or(true) {
        return None;
    }
    let Ok(pdu) = MacResource::from_bitbuf(&mut mac_block) else {
        return None;
    };
    // Match on SSI value only; the MAC-RESOURCE air format does not carry the ISSI/USSI/GSSI subtype.
    (pdu.addr.map(|a| a.ssi) == Some(addr.ssi)).then_some(pdu)
}

fn open_shared_voice_circuit(ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            carrier_num: MAIN_CARRIER,
            ts,
            peer_carrier_num: None,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::SwMI,
        })),
    }
}

fn new_secondary_umac_test(start_dl_time: TdmaTime) -> ComponentTest {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    ComponentTest::from_config(config, Some(start_dl_time))
}

#[test]
fn test_opening_shared_circuit_does_not_start_ul_inactivity_timer() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.run_stack(Some(3 * 18 * 4 + 10));
    let sink_msgs = test.dump_sinks();

    assert!(
        !sink_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout {
                carrier_num: MAIN_CARRIER,
                ts: 2
            })
        )),
        "Opening an UL-capable circuit must not imply that local uplink voice is expected"
    );
}

#[test]
fn test_floor_grant_starts_ul_inactivity_timer() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
            call_id: 1,
            source_issi: 1000001,
            dest_gssi: 1000002,
            carrier_num: MAIN_CARRIER,
            ts: 2,
        }),
    });
    test.run_stack(Some(3 * 18 * 4 + 10));
    let sink_msgs = test.dump_sinks();

    assert!(
        sink_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout {
                carrier_num: MAIN_CARRIER,
                ts: 2
            })
        )),
        "A local floor grant should still arm stuck-uplink detection"
    );
}

#[test]
fn test_network_downlink_voice_does_not_start_ul_inactivity_timer() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.submit_message(SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Brew,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
            carrier_num: MAIN_CARRIER,
            ts: 2,
            data: vec![0; 36],
        }),
    });
    test.run_stack(Some(3 * 18 * 4 + 10));
    let sink_msgs = test.dump_sinks();

    assert!(
        !sink_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout {
                carrier_num: MAIN_CARRIER,
                ts: 2
            })
        )),
        "Remote downlink media must not arm local stuck-uplink detection"
    );
}

#[test]
fn test_secondary_carrier_normal_signalling_falls_back_to_primary_mcch() {
    debug::setup_logging_verbose();

    let mut test = new_secondary_umac_test(TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    let dest = TetraAddress {
        ssi: 9012001,
        ssi_type: SsiType::Issi,
    };
    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr("1010101010101010"),
        main_address: dest,
        link_id: 1,
        endpoint_id: 0,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(SECONDARY_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(26),
            carrier: Some(SECONDARY_CARRIER),
            timeslots: [false, true, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        tx_reporter: None,
    };

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(12));
    let sink_msgs = test.dump_sinks();

    let mut found_on_primary = false;
    let mut found_on_secondary = false;

    for msg in sink_msgs {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            let contains_target = block_has_mac_resource_for(&slot.blk1, dest, true) || block_has_mac_resource_for(&slot.blk2, dest, true);
            if !contains_target {
                continue;
            }

            if slot.carrier_num == MAIN_CARRIER {
                found_on_primary = true;
                assert_eq!(slot.ts.t, 1, "primary MCCH fallback must transmit on ts1");
            }
            if slot.carrier_num == SECONDARY_CARRIER {
                found_on_secondary = true;
            }
        }
    }

    assert!(
        found_on_primary,
        "expected chan_alloc signalling to be transmitted on the primary MCCH"
    );
    assert!(
        !found_on_secondary,
        "secondary carrier without MCCH must not transmit normal chan_alloc signalling"
    );
}

#[test]
fn test_secondary_ts1_channel_allocation_encodes_secondary_carrier_without_css() {
    debug::setup_logging_verbose();

    let mut test = new_secondary_umac_test(TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    let dest = TetraAddress {
        ssi: 9012001,
        ssi_type: SsiType::Issi,
    };
    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr("1010101010101010"),
        main_address: dest,
        link_id: 1,
        endpoint_id: 0,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(SECONDARY_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(26),
            carrier: Some(SECONDARY_CARRIER),
            timeslots: [true, false, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        tx_reporter: None,
    };

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(12));

    let mut found = false;
    for msg in test.dump_sinks() {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            let Some(pdu) = block_mac_resource_for(&slot.blk1, dest).or_else(|| block_mac_resource_for(&slot.blk2, dest)) else {
                continue;
            };
            let Some(chan_alloc) = pdu.chan_alloc_element else {
                continue;
            };
            found = true;
            assert_eq!(
                slot.carrier_num, MAIN_CARRIER,
                "ordinary control signalling must still ride the primary MCCH"
            );
            assert_eq!(slot.ts.t, 1, "chan_alloc should be transmitted on the primary MCCH");
            assert_eq!(chan_alloc.carrier_num, SECONDARY_CARRIER);
            assert_eq!(chan_alloc.alloc_type, ChanAllocType::Replace);
            assert_ne!(chan_alloc.alloc_type, ChanAllocType::ReplaceWithCarrierSignalling);
            assert_eq!(chan_alloc.ts_assigned, [true, false, false, false]);
        }
    }

    assert!(found, "expected a MAC-RESOURCE carrying the secondary TS1 channel allocation");
}

#[test]
fn test_main_ts1_ordinary_traffic_allocation_is_rejected() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    let dest = TetraAddress {
        ssi: 9012001,
        ssi_type: SsiType::Issi,
    };
    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr("1010101010101010"),
        main_address: dest,
        link_id: 1,
        endpoint_id: 0,
        stealing_permission: false,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(MAIN_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(26),
            carrier: Some(MAIN_CARRIER),
            timeslots: [true, false, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        tx_reporter: None,
    };

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(12));

    let found = test.dump_sinks().into_iter().any(|msg| {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => return false,
        };
        slots
            .into_iter()
            .any(|slot| block_has_mac_resource_for(&slot.blk1, dest, true) || block_has_mac_resource_for(&slot.blk2, dest, true))
    });

    assert!(!found, "main-carrier TS1 ordinary traffic allocation should be rejected");
}

#[test]
fn test_random_access_on_secondary_ts1_is_ignored() {
    debug::setup_logging_verbose();

    let mut test = new_secondary_umac_test(TdmaTime { h: 0, m: 1, f: 1, t: 3 });
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    let dest = TetraAddress {
        ssi: 2200699,
        ssi_type: SsiType::Issi,
    };

    let mut uplink = BitBuffer::new_autoexpand(64);
    MacAccess {
        fill_bits: true,
        encrypted: false,
        addr: Some(dest),
        event_label: None,
        length_ind: Some(6),
        frag_flag: None,
        reservation_req: None,
    }
    .to_bitbuf(&mut uplink);
    uplink.write_bits(0, 12);
    uplink.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num: SECONDARY_CARRIER,
            pdu: uplink,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    });
    test.run_stack(Some(2));

    assert!(
        test.dump_sinks().is_empty(),
        "random access on a secondary TS1 without MCCH must be ignored"
    );
}

#[test]
fn test_remote_floor_grant_resumes_traffic_without_ul_inactivity_timer() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased {
            call_id: 1,
            carrier_num: MAIN_CARRIER,
            ts: 2,
        }),
    });
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::RemoteFloorGranted {
            call_id: 1,
            carrier_num: MAIN_CARRIER,
            ts: 2,
        }),
    });
    test.run_stack(Some(3 * 18 * 4 + 10));
    let sink_msgs = test.dump_sinks();

    assert!(
        !sink_msgs.iter().any(|msg| matches!(
            &msg.msg,
            SapMsgInner::CmceCallControl(CallControl::UlInactivityTimeout {
                carrier_num: MAIN_CARRIER,
                ts: 2
            })
        )),
        "Remote floor grants must resume traffic mode without arming local stuck-uplink detection"
    );
}

#[test]
fn test_ul_mac_u_signal_uses_floor_owner_and_timeslot_link() {
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 3 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc]);

    test.submit_message(open_shared_voice_circuit(2));
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
            call_id: 1,
            source_issi: 2200769,
            dest_gssi: 2200699,
            carrier_num: MAIN_CARRIER,
            ts: 2,
        }),
    });
    test.run_stack(Some(1));
    test.dump_sinks();

    let mut pdu = BitBuffer::new_autoexpand(16);
    MacUSignal { second_half_stolen: false }.to_bitbuf(&mut pdu);
    pdu.write_bits(0b1010_1010, 8);
    pdu.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::Stch,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    });
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    assert_eq!(sink_msgs.len(), 1);
    let SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
        main_address,
        link_id,
        pdu: Some(payload),
        ..
    }) = &sink_msgs[0].msg
    else {
        panic!("expected TMA-UNITDATA indication");
    };

    assert_eq!(main_address.ssi, 2200769);
    assert_eq!(*link_id, 2);
    assert_eq!(payload.get_len(), 8);
}

#[test]
fn test_in_fragmented_sch_hu_and_sch_f() {
    // Receive SCH/HU containing MAC-ACCESS with fragmentation start
    // Then receive SCH-F containing MAC-END (UL)
    debug::setup_logging_verbose();
    let test_vec1 = "00000000111111000001001111110111000100011001011100111000000011111100001000010000000000000000";
    let test_vec2 = "0110001110000000000010010000000000000000000000000100010000000000000000000000000110010000000000000000000000001000001000000111111000001001111110000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec1 = dltime_vec1.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim1 = TmvUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: BitBuffer::from_bitstr(test_vec1),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NEG_INFINITY,
    };
    let test_sapmsg1 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim1),
    };
    let test_prim2 = TmvUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: BitBuffer::from_bitstr(test_vec2),
        block_num: PhyBlockNum::Both,
        logical_channel: LogicalChannel::SchF,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NEG_INFINITY,
    };
    let test_sapmsg2 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim2),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![
        // TetraEntity::Lmac, // Simply discard
        TetraEntity::Mm,
    ];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg1);
    test.run_stack(Some(4));
    test.submit_message(test_sapmsg2);
    test.run_stack(Some(1));
    let sink_msgs = test.dump_sinks();

    // Evaluate results. We should have an MM message in the sink
    assert_eq!(sink_msgs.len(), 1);
    tracing::info!("We have the expected MM message, but full validation of result not implemented");
}

#[test]
fn test_in_fragmented_sch_hu_and_sch_hu() {
    // Receive SCH/HU containing MAC-ACCESS with fragmentation start
    // Then receive SCH-HU containing MAC-END-HU
    // Message ultimately contains CMCE SDS message
    debug::setup_logging_verbose();
    let test_vec1 = "00000000111110010001111101110111000000010010011110000010000001100010001001001111100001010100";
    let test_vec2 = "10011000000101000110000000000000000000000000000000000000000000000000111111111111110100000010";
    let dltime_vec1 = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec1 = dltime_vec1.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim1 = TmvUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: BitBuffer::from_bitstr(test_vec1),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NEG_INFINITY,
    };
    let test_sapmsg1 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim1),
    };
    let test_prim2 = TmvUnitdataInd {
        carrier_num: MAIN_CARRIER,
        pdu: BitBuffer::from_bitstr(test_vec2),
        block_num: PhyBlockNum::Block1,
        logical_channel: LogicalChannel::SchHu,
        crc_pass: true,
        scrambling_code: 864282631,
        rssi_dbfs: f32::NEG_INFINITY,
    };
    let test_sapmsg2 = SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(test_prim2),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec1));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![
        // TetraEntity::Lmac, // Simply discard
        TetraEntity::Cmce,
    ];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg1);
    test.run_stack(Some(4));
    test.submit_message(test_sapmsg2);
    test.run_stack(Some(1));

    // Evaluate results. We should have an CMCE message in the sink
    let sink_msgs = test.dump_sinks();
    assert_eq!(sink_msgs.len(), 1);
    tracing::info!("We have the expected CMCE message, but full validation of result not implemented");
}

#[test]
fn test_out_fragmented_resource() {
    // Test for UMAC (and LLC/MLE)
    // The vector is an MM DAttachDetachGroupIdentityAcknowledgement which contains a lot of groups.
    // As it is very large, it needs to be fragmented at the MAC layer.
    debug::setup_logging_verbose();
    let test_vec = "10110011011100110100110001101011100000000000011101010011001110110100000000000111010100111111101101000000000001110101010000000011010000000000011101010100000010110100000000000111010101000001001101000000000001110101010000011011010000000000011101010100001000110100000000000111010101000010101101000000000001110101010000110011010000000000011101010100001110110100000000000111010101000100001101000000000001110101010001001011010000000000011101010100010100";
    let dltime_vec = TdmaTime::default().add_timeslots(2); // Downlink time: 0/1/1/3
    // let ultime_vec = dltime_vec.add_timeslots(-2); // Uplink time: 0/1/1/1
    let test_prim = LmmMleUnitdataReq {
        sdu: BitBuffer::from_bitstr(test_vec),
        handle: 0,
        address: TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 30128,
        },
        layer2service: Layer2Service::Acknowledged,
        stealing_permission: false,
        stealing_repeats_flag: false,
        encryption_flag: false,
        is_null_pdu: false,
        tx_reporter: None,
    };
    let test_sapmsg = SapMsg {
        sap: Sap::LmmSap,
        src: TetraEntity::Mm,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::LmmMleUnitdataReq(test_prim),
    };

    // Setup testing stack
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime_vec));
    let components = vec![TetraEntity::Umac, TetraEntity::Llc, TetraEntity::Mle];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Lmac];
    test.populate_entities(components, sinks);

    // Submit and process message
    test.submit_message(test_sapmsg);
    test.run_stack(Some(8));

    tracing::info!("Validation of result not implemented");
}

#[test]
fn test_facch_stealing_does_not_set_random_access_flag_without_pending_ra() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let components = vec![TetraEntity::Umac];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Lmac];
    test.populate_entities(components, sinks);

    let ts = 2u8;
    let dest = TetraAddress {
        ssi: 2200699,
        ssi_type: SsiType::Issi,
    };

    test.submit_message(open_shared_voice_circuit(ts));
    test.run_stack(Some(1));
    test.dump_sinks();

    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr("1010101010101010"),
        main_address: dest,
        link_id: ts as u32,
        endpoint_id: 0,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(MAIN_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            carrier: Some(MAIN_CARRIER),
            timeslots: [false, true, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        tx_reporter: None,
    };

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();

    let mut found = false;
    for msg in sink_msgs {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            if slot.ts.t != ts {
                continue;
            }
            let Some(blk1) = slot.blk1 else {
                continue;
            };
            if blk1.logical_channel != LogicalChannel::Stch {
                continue;
            }

            let mut mac_block = blk1.mac_block.clone();
            let pdu = MacResource::from_bitbuf(&mut mac_block).expect("STCH blk1 should start with MAC-RESOURCE");
            assert!(
                !pdu.random_access_flag,
                "FACCH stealing without pending RA must not set random_access_flag"
            );
            assert_eq!(
                pdu.usage_marker, None,
                "FACCH stealing must not encode a usage marker in the MAC-RESOURCE header"
            );
            found = true;
            break;
        }
        if found {
            break;
        }
    }

    assert!(found, "expected an STCH downlink block on ts {}", ts);
}

#[test]
fn test_traffic_mac_access_does_not_mark_next_facch_as_random_access() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let components = vec![TetraEntity::Umac];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Lmac];
    test.populate_entities(components, sinks);

    let ts = 2u8;
    let dest = TetraAddress {
        ssi: 2200699,
        ssi_type: SsiType::Issi,
    };

    test.submit_message(open_shared_voice_circuit(ts));
    test.run_stack(Some(1));
    test.dump_sinks();

    let mut uplink = BitBuffer::new_autoexpand(64);
    MacAccess {
        fill_bits: true,
        encrypted: false,
        addr: Some(dest),
        event_label: None,
        length_ind: Some(6),
        frag_flag: None,
        reservation_req: None,
    }
    .to_bitbuf(&mut uplink);
    uplink.write_bits(0, 12);
    uplink.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu: uplink,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    });
    test.run_stack(Some(2));
    test.dump_sinks();

    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr("1010101010101010"),
        main_address: dest,
        link_id: ts as u32,
        endpoint_id: 0,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(MAIN_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(4),
            carrier: Some(MAIN_CARRIER),
            timeslots: [false, true, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        tx_reporter: None,
    };

    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(8));
    let sink_msgs = test.dump_sinks();

    let mut found = false;
    for msg in sink_msgs {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            if slot.ts.t != ts {
                continue;
            }
            let Some(blk1) = slot.blk1 else {
                continue;
            };
            if blk1.logical_channel != LogicalChannel::Stch {
                continue;
            }

            let mut mac_block = blk1.mac_block.clone();
            let pdu = MacResource::from_bitbuf(&mut mac_block).expect("STCH blk1 should start with MAC-RESOURCE");
            assert!(
                !pdu.random_access_flag,
                "traffic-slot MAC-ACCESS must not make the next FACCH look like a random-access response"
            );
            assert_eq!(
                pdu.usage_marker, None,
                "traffic-slot FACCH stealing must not encode a usage marker in the MAC-RESOURCE header"
            );
            found = true;
            break;
        }
        if found {
            break;
        }
    }

    assert!(found, "expected an STCH downlink block on ts {}", ts);
}

/// A MAC-ACCESS from `addr` on SCH/HU of `carrier_num` (a BL-ACK sized PDU), on the slot two
/// slots before the tick that takes it.
fn sch_hu_mac_access(carrier_num: u16, addr: TetraAddress) -> SapMsg {
    let mut uplink = BitBuffer::new_autoexpand(64);
    MacAccess {
        fill_bits: true,
        encrypted: false,
        addr: Some(addr),
        event_label: None,
        length_ind: Some(6),
        frag_flag: None,
        reservation_req: None,
    }
    .to_bitbuf(&mut uplink);
    uplink.write_bits(0, 12);
    uplink.seek(0);
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num,
            pdu: uplink,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    }
}

/// A test with a call in hangtime on `ts` of `carrier_num`, the next tick taking what was sent on
/// the uplink of `ts`.
fn call_in_hangtime(carrier_num: u16, ts: u8) -> ComponentTest {
    let mut test = new_secondary_umac_test(TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac, TetraEntity::Llc]);
    let mut open = open_shared_voice_circuit(ts);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut open.msg {
        circuit.carrier_num = carrier_num;
    }
    test.submit_message(open);
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorReleased { call_id: 1, carrier_num, ts }),
    });
    test.run_stack(Some(ts as usize + 1));
    test.dump_sinks();
    test
}

/// The MAC-RESOURCEs for `addr` that `msgs` put on `ts` of `carrier_num`.
fn mac_resources_on(msgs: Vec<SapMsg>, carrier_num: u16, ts: u8, addr: TetraAddress) -> Vec<MacResource> {
    msgs.into_iter()
        .flat_map(|msg| match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => Vec::new(),
        })
        .filter(|slot| slot.carrier_num == carrier_num && slot.ts.t == ts)
        .filter_map(|slot| block_mac_resource_for(&slot.blk1, addr))
        .collect()
}

/// A radio in a call that random-accesses on the call's slot in hangtime, where the AACH invites
/// it (Header 2, EN 300 392-2 23.5.1.4.8), gets its random-access acknowledgement on that slot.
/// Without it the radio repeats the MAC-ACCESS up to Nu times and the LLC takes every copy (field,
/// 2026-10-03: a BL-ACK six times on ts1 of a carrier without MCCH, the last copy acknowledging a
/// PDU the radio never got; a BL-DATA six times on ts2 of the main carrier).
#[test]
fn test_random_access_in_hangtime_is_acknowledged() {
    debug::setup_logging_verbose();
    let radio = TetraAddress::issi(2145007);
    for (carrier_num, ts) in [(MAIN_CARRIER, 2u8), (SECONDARY_CARRIER, 1)] {
        let mut test = call_in_hangtime(carrier_num, ts);
        test.submit_message(sch_hu_mac_access(carrier_num, radio));
        test.run_stack(Some(8));
        let acks: Vec<bool> = mac_resources_on(test.dump_sinks(), carrier_num, ts, radio)
            .iter()
            .map(|pdu| pdu.random_access_flag)
            .collect();
        assert_eq!(acks, vec![true], "carrier {carrier_num} ts {ts}");
    }
}

/// A PTT in hangtime granted in the tick its random access arrives: the D-TX GRANTED to the radio,
/// stolen once the slot is back to traffic, is not flagged as the answer to a random access (as
/// before random accesses in hangtime were acknowledged).
#[test]
fn test_floor_granted_in_hangtime_drops_the_random_access_ack() {
    debug::setup_logging_verbose();
    let radio = TetraAddress::issi(2145007);
    let ts = 2u8;
    let mut test = call_in_hangtime(MAIN_CARRIER, ts);
    test.submit_message(sch_hu_mac_access(MAIN_CARRIER, radio));
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
            call_id: 1,
            source_issi: radio.ssi,
            dest_gssi: 1000002,
            carrier_num: MAIN_CARRIER,
            ts,
        }),
    });
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr("1010101010101010"),
            main_address: radio,
            link_id: ts as u32,
            endpoint_id: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: Some(MAIN_CARRIER),
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(4),
                carrier: Some(MAIN_CARRIER),
                timeslots: [false, true, false, false],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(8));
    let flags: Vec<bool> = mac_resources_on(test.dump_sinks(), MAIN_CARRIER, ts, radio)
        .iter()
        .map(|pdu| pdu.random_access_flag)
        .collect();
    assert_eq!(flags, vec![false], "one stolen MAC-RESOURCE, not flagged");
}

/// FH-BUG-034 follow-up regression: a stealing TmaUnitdataReq whose MAC-RESOURCE + SDU does
/// not fit in one 124-bit STCH half-slot must be fragmented across consecutive stolen
/// half-slots — NOT written into a fixed 124-bit buffer, which panicked the whole stack
/// ("write would exceed buffer end") and was a remotely-triggerable crash: sending an SDS or
/// status longer than one half-slot to an MS engaged in a call took down the BS.
///
/// This test drives the exact UMAC path (rx_ul_tma_unitdata_req) with a large stealing SDU on
/// an open traffic circuit and asserts the run completes without panicking.
#[test]
fn test_stealing_large_sdu_fragments_without_panic() {
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let components = vec![TetraEntity::Umac];
    let sinks: Vec<TetraEntity> = vec![TetraEntity::Lmac];
    test.populate_entities(components, sinks);

    let ts = 2u8;
    let dest = TetraAddress {
        ssi: 2260575,
        ssi_type: SsiType::Issi,
    };

    // Open a DL+UL traffic circuit on ts 2 so the stealing path has an active circuit to steal
    // a half-slot from (otherwise it falls back to the MCCH and the bug isn't exercised).
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            carrier_num: MAIN_CARRIER,
            ts,
            peer_carrier_num: None,
            peer_ts: None,
            usage: 6,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
        })),
    });
    test.run_stack(Some(1));

    // A ~240-bit SDU: far larger than one 124-bit STCH half-slot, forcing fragmentation.
    let big_sdu = "0".repeat(120) + &"1".repeat(120);
    let tma = TmaUnitdataReq {
        req_handle: 0,
        pdu: BitBuffer::from_bitstr(&big_sdu),
        main_address: dest,
        link_id: 2,
        endpoint_id: 0,
        stealing_permission: true,
        subscriber_class: 0,
        air_interface_encryption: None,
        stealing_repeats_flag: None,
        data_category: None,
        carrier_num: Some(MAIN_CARRIER),
        chan_alloc: Some(CmceChanAllocReq {
            usage: Some(6),
            carrier: Some(MAIN_CARRIER),
            timeslots: [false, true, false, false], // ts 2
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Dl,
        }),
        tx_reporter: None,
    };

    // Before the fix this call panicked inside the UMAC stealing builder. The assertion is
    // simply that we get here and can keep running ticks — i.e. no panic, the stack survives.
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(tma),
    });
    test.run_stack(Some(8));

    tracing::info!("stealing large SDU fragmented across STCH half-slots without panic");
}

/// Number of reassembly contexts currently held by the BS defragmenter, across all timeslots.
fn defrag_context_count(test: &mut ComponentTest) -> usize {
    let umac = test
        .router
        .get_entity(TetraEntity::Umac)
        .expect("umac registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("entity is UmacBs");
    umac.defrag.buffers.iter().map(|map| map.len()).sum()
}

/// Builds a MAC-ACCESS fragmentation start (no MAC-END will ever follow) for `ssi`,
/// padded out to a SCH/HU block.
fn frag_start_access(ssi: u32) -> SapMsg {
    let addr = TetraAddress {
        ssi,
        ssi_type: SsiType::Issi,
    };
    let mut uplink = BitBuffer::new_autoexpand(92);
    MacAccess {
        fill_bits: false,
        encrypted: false,
        addr: Some(addr),
        event_label: None,
        length_ind: None,
        frag_flag: Some(true),
        reservation_req: Some(ReservationRequirement::Req1Subslot),
    }
    .to_bitbuf(&mut uplink);
    uplink.write_bits(0, 56);
    uplink.seek(0);

    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu: uplink,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    }
}

#[test]
fn test_defrag_map_is_bounded_under_frag_start_flood() {
    // An MS can start a fragmented uplink burst with any 24-bit SSI and never send the
    // MAC-END. The defrag map must stay bounded and must drop the stale contexts again.
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 3 }; // uplink message time is ts1 (MCCH)
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    for i in 0..200u32 {
        test.submit_message(frag_start_access(2200000 + i));
    }
    test.run_stack(Some(1));

    let held = defrag_context_count(&mut test);
    assert!(
        held > 0 && held <= 32,
        "defrag map must be capped per timeslot, holds {} contexts after 200 unfinished starts",
        held
    );

    // Nothing more arrives: every context must age out and have its map key removed.
    test.run_stack(Some(100));
    assert_eq!(
        defrag_context_count(&mut test),
        0,
        "stale reassembly contexts must be removed, not just reset"
    );
}

#[test]
fn test_granting_delay_never_encodes_reserved_or_truncated_code() {
    // The granting delay is a 4-bit field, valid N 1..13 (EN 300 392-2 cl. 21.5.6).
    // Fill the uplink schedule so the required delay runs past 13 and check that the BS
    // never emits 14/15 (reserved) or a value that truncates.
    debug::setup_logging_verbose();

    let dltime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };
    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    test.run_stack(Some(1)); // let the scheduler adopt the current time

    let umac = test
        .router
        .get_entity(TetraEntity::Umac)
        .expect("umac registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("entity is UmacBs");

    let mut deferred = false;
    for i in 0..18u32 {
        let addr = TetraAddress {
            ssi: 2300000 + i,
            ssi_type: SsiType::Issi,
        };
        match umac
            .channel_scheduler
            .ul_process_cap_req(1, addr, &ReservationRequirement::Req1Slot)
        {
            Some((grant, _)) => match grant.granting_delay {
                BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity => {}
                BasicSlotgrantGrantingDelay::DelayNOpportunities(n) => {
                    assert!((1..=13).contains(&n), "granting delay {} is outside the encodable range", n);
                }
                other => panic!("reserved granting delay code emitted: {}", other),
            },
            None => deferred = true,
        }
    }

    assert!(
        deferred,
        "a request needing a delay beyond the 4-bit field must be deferred, not encoded"
    );
}

/// CMCE's end-of-call close for a traffic timeslot (deferred by the UMAC until FACCH drains).
fn close_shared_voice_circuit(ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::CloseSlot {
            direction: Direction::Both,
            carrier_num: MAIN_CARRIER,
            ts,
        }),
    }
}

/// One 60 ms DL voice block from Brew, every byte set to `marker` so the block can be
/// identified again once the scheduler hands it to the LMAC.
fn brew_dl_voice(ts: u8, marker: u8) -> SapMsg {
    SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Brew,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq {
            carrier_num: MAIN_CARRIER,
            ts,
            data: vec![marker; 36],
        }),
    }
}

/// First byte of the first DL traffic block the UMAC sent to the LMAC for this timeslot.
fn first_dl_traffic_marker(msgs: &[SapMsg], ts: u8) -> Option<u8> {
    msgs.iter().find_map(|msg| {
        let SapMsgInner::TmvUnitdataReqSlots(req) = &msg.msg else {
            return None;
        };
        req.slots.iter().find_map(|slot| {
            if slot.ts.t != ts {
                return None;
            }
            [&slot.blk1, &slot.blk2]
                .into_iter()
                .flatten()
                .find(|blk| blk.logical_channel == LogicalChannel::TchS)
                .and_then(|blk| blk.mac_block.peek_bits_startoffset(0, 8))
                .map(|marker| marker as u8)
        })
    })
}

fn umac_of(test: &mut ComponentTest) -> &mut UmacBs {
    test.router
        .get_entity(TetraEntity::Umac)
        .expect("umac registered")
        .as_any_mut()
        .downcast_mut::<UmacBs>()
        .expect("entity is UmacBs")
}

#[test]
fn test_reopening_traffic_slot_cancels_deferred_close() {
    // FH-BUG-077: a close on a traffic timeslot is deferred to a later tick so queued
    // FACCH/STCH can drain. If the next call grabs that timeslot before the deferred close
    // runs, the stale close must not tear down the fresh circuit -- otherwise every Brew DL
    // voice frame is dropped as "inactive circuit" for the rest of that call.
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.run_stack(Some(1));
    assert!(
        umac_of(&mut test).channel_scheduler.circuit_is_active(Direction::Dl, 2),
        "first call should have an active DL circuit"
    );

    // Old call ends and the new one seizes the same timeslot before the next tick.
    test.submit_message(close_shared_voice_circuit(2));
    test.submit_message(open_shared_voice_circuit(2));
    test.run_stack(Some(4)); // ticks here are where the deferred close would have fired

    let umac = umac_of(&mut test);
    assert!(
        umac.channel_scheduler.circuit_is_active(Direction::Dl, 2),
        "re-opened DL circuit must survive the deferred close of the previous call"
    );
    assert!(
        umac.channel_scheduler.circuit_is_active(Direction::Ul, 2),
        "re-opened UL circuit must survive the deferred close of the previous call"
    );

    // ...and DL voice must actually be scheduled rather than dropped.
    test.submit_message(brew_dl_voice(2, 1));
    test.deliver_all_messages();
    assert_eq!(
        umac_of(&mut test).channel_scheduler.dl_queued_block_count(2),
        1,
        "DL voice on the re-opened circuit must be scheduled, not dropped"
    );
}

#[test]
fn test_deferred_close_still_runs_for_direction_not_reopened() {
    // Cancelling is per-direction: re-opening only the DL must leave the pending UL close
    // alone, or a stale uplink circuit from the previous call leaks.
    debug::setup_logging_verbose();

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Cmce]);

    test.submit_message(open_shared_voice_circuit(2));
    test.run_stack(Some(1));

    test.submit_message(close_shared_voice_circuit(2));
    let mut reopen_dl_only = open_shared_voice_circuit(2);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut reopen_dl_only.msg {
        circuit.direction = Direction::Dl;
    }
    test.submit_message(reopen_dl_only);
    test.run_stack(Some(4));

    let umac = umac_of(&mut test);
    assert!(
        umac.channel_scheduler.circuit_is_active(Direction::Dl, 2),
        "re-opened DL circuit must stay active"
    );
    assert!(
        !umac.channel_scheduler.circuit_is_active(Direction::Ul, 2),
        "a UL close nobody re-opened must still run"
    );
}

#[test]
fn test_dl_block_queue_is_bounded_under_burst() {
    // Brew and the SIP bridge can deliver a burst of voice blocks in a single tick, while the
    // downlink sends one block per timeslot per frame. The queue must hold a jitter allowance
    // only -- an unbounded one turns a burst into permanent latency and unbounded memory.
    debug::setup_logging_verbose();

    const BURST: u8 = 200;
    const MAX_QUEUED: u8 = 4;

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    test.submit_message(open_shared_voice_circuit(2));
    test.run_stack(Some(1));

    for marker in 0..BURST {
        test.submit_message(brew_dl_voice(2, marker));
    }
    test.deliver_all_messages();

    let queued = umac_of(&mut test).channel_scheduler.dl_queued_block_count(2);
    assert!(
        queued > 0 && queued <= MAX_QUEUED as usize,
        "DL voice queue must be capped at a few blocks of jitter, holds {} after a {}-block burst",
        queued,
        BURST
    );

    // Drop the silence sent while the circuit was idle, then check what actually goes out:
    // the oldest blocks must have been discarded, not the newest.
    let _ = test.dump_sinks();
    test.run_stack(Some(8));
    let msgs = test.dump_sinks();
    let marker = first_dl_traffic_marker(&msgs, 2).expect("a DL traffic block for ts2");
    assert!(
        marker >= BURST - MAX_QUEUED,
        "overflow must drop the OLDEST blocks; transmitted block {} instead of one of the newest",
        marker
    );
}

#[test]
fn test_secondary_ts1_close_is_deferred_so_facch_release_goes_out_on_channel() {
    // On a secondary carrier ts1 is an assigned traffic slot. At the end of a call CMCE queues the
    // on-channel D-RELEASE (FACCH/STCH) for the call's own timeslot and then the circuit close, and
    // the close reaches the UMAC first (one hop vs three). It must be deferred like the primary's
    // ts 2..=4, or the STCH finds no active DL circuit and falls back to the MCCH — which a terminal
    // parked on the secondary traffic channel never hears: it sits on the dead channel until its
    // own timer expires, then rescans and re-registers.
    debug::setup_logging_verbose();

    let mut test = new_secondary_umac_test(TdmaTime { h: 0, m: 1, f: 1, t: 1 });
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);

    let mut open = open_shared_voice_circuit(1);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut open.msg {
        circuit.carrier_num = SECONDARY_CARRIER;
    }
    test.submit_message(open);
    test.run_stack(Some(1));
    test.dump_sinks();
    assert!(
        umac_of(&mut test).circuit_is_active_on(SECONDARY_CARRIER, Direction::Dl, 1),
        "the secondary ts1 circuit should be open"
    );

    // End of call: close first, FACCH copy of the D-RELEASE right behind it, same tick.
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::CloseSlot {
            direction: Direction::Both,
            carrier_num: SECONDARY_CARRIER,
            ts: 1,
        }),
    });
    let dest = TetraAddress {
        ssi: 9012001,
        ssi_type: SsiType::Issi,
    };
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr("1010101010101010"),
            main_address: dest,
            link_id: 1,
            endpoint_id: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: Some(SECONDARY_CARRIER),
            chan_alloc: Some(CmceChanAllocReq {
                usage: Some(26),
                carrier: Some(SECONDARY_CARRIER),
                timeslots: [true, false, false, false],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(12));

    let mut stch_on_secondary_ts1 = false;
    let mut fell_back_to_primary_mcch = false;
    for msg in test.dump_sinks() {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            let is_stch = slot.blk1.as_ref().is_some_and(|b| b.logical_channel == LogicalChannel::Stch);
            if slot.carrier_num == SECONDARY_CARRIER && slot.ts.t == 1 && is_stch {
                stch_on_secondary_ts1 = true;
            }
            if slot.carrier_num == MAIN_CARRIER
                && slot.ts.t == 1
                && (block_mac_resource_for(&slot.blk1, dest).is_some() || block_mac_resource_for(&slot.blk2, dest).is_some())
            {
                fell_back_to_primary_mcch = true;
            }
        }
    }
    assert!(stch_on_secondary_ts1, "the FACCH D-RELEASE must go out as an STCH on the secondary ts1");
    assert!(!fell_back_to_primary_mcch, "it must not have fallen back to the primary MCCH");

    // ...and the deferred close still ran once the STCH drained.
    let umac = umac_of(&mut test);
    assert!(
        !umac.circuit_is_active_on(SECONDARY_CARRIER, Direction::Dl, 1),
        "the deferred DL close must still run"
    );
    assert!(
        !umac.circuit_is_active_on(SECONDARY_CARRIER, Direction::Ul, 1),
        "the deferred UL close must still run"
    );
}

// ---------------------------------------------------------------------------------------------
// The secondary carrier: what goes on the air, recorded on miura before the packet-data
// carrier (c4fc51a); a different value is a change on the air, never a reason to update it.
// ---------------------------------------------------------------------------------------------

/// Fingerprint of every slot `scenario` put on the air and every PDU between its MAC and LLC.
fn fingerprint(scenario: fn()) -> u64 {
    crate::common::component_test::trace_arm();
    scenario();
    let hash = crate::common::component_test::trace_hash();
    println!("fingerprint {hash:#018x}");
    hash
}

const SECONDARY_IDENTITY_MCCH_FALLBACK: u64 = 0x9fb4_e644_d687_7645;
const SECONDARY_IDENTITY_TS1_CHAN_ALLOC: u64 = 0x26ca_16a9_4a01_fe80;
const SECONDARY_IDENTITY_TS1_CLOSE: u64 = 0x8161_e654_6d07_aabe;

#[test]
fn secondary_identity_mcch_fallback() {
    assert_eq!(
        fingerprint(test_secondary_carrier_normal_signalling_falls_back_to_primary_mcch),
        SECONDARY_IDENTITY_MCCH_FALLBACK
    );
}

#[test]
fn secondary_identity_ts1_chan_alloc() {
    assert_eq!(
        fingerprint(test_secondary_ts1_channel_allocation_encodes_secondary_carrier_without_css),
        SECONDARY_IDENTITY_TS1_CHAN_ALLOC
    );
}

#[test]
fn secondary_identity_ts1_close() {
    assert_eq!(
        fingerprint(test_secondary_ts1_close_is_deferred_so_facch_release_goes_out_on_channel),
        SECONDARY_IDENTITY_TS1_CLOSE
    );
}

/// Packet data on a PDCH (`[packet_data] bearer = "pdch"`) in every configuration of the scenario.
fn packet_data_on_a_pdch(cfg: &mut tetra_config::bluestation::StackConfig) {
    cfg.cell.sndcp_service = true;
    cfg.packet_data.enabled = true;
    cfg.packet_data.bearer = tetra_config::bluestation::PacketDataBearer::Pdch;
}

const SECONDARY_IDENTITY_PDCH_MCCH_FALLBACK: u64 = 0x047d_a5ac_24b2_5ba9;
const SECONDARY_IDENTITY_PDCH_TS1_CHAN_ALLOC: u64 = 0x0ddf_d5e0_dc2e_63de;
const SECONDARY_IDENTITY_PDCH_TS1_CLOSE: u64 = 0x2770_f1fc_3b23_fa55;

/// The secondary-carrier scenarios with packet data on a PDCH and no data session.
fn secondary_scenarios_with(tweak: fn(&mut tetra_config::bluestation::StackConfig)) -> [u64; 3] {
    [
        test_secondary_carrier_normal_signalling_falls_back_to_primary_mcch as fn(),
        test_secondary_ts1_channel_allocation_encodes_secondary_carrier_without_css,
        test_secondary_ts1_close_is_deferred_so_facch_release_goes_out_on_channel,
    ]
    .map(|scenario| crate::common::component_test::with_config_tweak(tweak, || fingerprint(scenario)))
}

#[test]
fn secondary_identity_with_packet_data_on_a_pdch() {
    assert_eq!(
        secondary_scenarios_with(packet_data_on_a_pdch),
        [
            SECONDARY_IDENTITY_PDCH_MCCH_FALLBACK,
            SECONDARY_IDENTITY_PDCH_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_PDCH_TS1_CLOSE
        ]
    );
}

/// With the secondary carrier as the packet-data carrier (`pdch_carrier`) and no data session,
/// the secondary carrier is what it was.
fn packet_data_carrier_without_a_session(cfg: &mut tetra_config::bluestation::StackConfig) {
    packet_data_on_a_pdch(cfg);
    cfg.packet_data.pdch_carrier = cfg.cell.secondary_carrier;
}

#[test]
fn secondary_identity_with_a_packet_data_carrier_and_no_session() {
    assert_eq!(
        secondary_scenarios_with(packet_data_carrier_without_a_session),
        [
            SECONDARY_IDENTITY_PDCH_MCCH_FALLBACK,
            SECONDARY_IDENTITY_PDCH_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_PDCH_TS1_CLOSE
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// A third carrier (`extra_carriers`): the main and secondary carriers are what they were, and the
// third one is what the secondary is.
// ---------------------------------------------------------------------------------------------

const THIRD_CARRIER: u16 = 1524;

fn third_carrier(cfg: &mut tetra_config::bluestation::StackConfig) {
    cfg.cell.extra_carriers = vec![THIRD_CARRIER];
}

fn packet_data_carrier_and_a_third(cfg: &mut tetra_config::bluestation::StackConfig) {
    packet_data_carrier_without_a_session(cfg);
    third_carrier(cfg);
}

/// The scenarios' carrier 1522 becomes the third carrier, behind another secondary.
fn scenarios_carrier_is_the_third(cfg: &mut tetra_config::bluestation::StackConfig) {
    cfg.cell.secondary_carrier = Some(THIRD_CARRIER);
    cfg.cell.extra_carriers = vec![SECONDARY_CARRIER];
}

/// With a third carrier, its slots left out, the secondary-carrier scenarios go on the air as
/// before: with and without a packet-data carrier.
#[test]
fn third_carrier_identity() {
    let skip = |tweak| crate::common::component_test::with_trace_skip_carrier(THIRD_CARRIER, || secondary_scenarios_with(tweak));
    assert_eq!(
        skip(third_carrier),
        [
            SECONDARY_IDENTITY_MCCH_FALLBACK,
            SECONDARY_IDENTITY_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_TS1_CLOSE
        ]
    );
    assert_eq!(
        skip(packet_data_carrier_and_a_third),
        [
            SECONDARY_IDENTITY_PDCH_MCCH_FALLBACK,
            SECONDARY_IDENTITY_PDCH_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_PDCH_TS1_CLOSE
        ]
    );
}

/// The secondary-carrier scenarios run on a third carrier (the MCCH fallback, a ts1 channel
/// allocation on the MCCH without CSS, the ts1 close deferred until the FACCH D-RELEASE went out
/// there): their checks pass, and with the secondary's slots left out the air is what it was
/// with that carrier as the secondary.
#[test]
fn a_third_carrier_is_what_the_secondary_was() {
    assert_eq!(
        crate::common::component_test::with_trace_skip_carrier(THIRD_CARRIER, || secondary_scenarios_with(
            scenarios_carrier_is_the_third
        )),
        [
            SECONDARY_IDENTITY_MCCH_FALLBACK,
            SECONDARY_IDENTITY_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_TS1_CLOSE
        ]
    );
}

/// An idle cell of three carriers: a slot of each per tick, the third one the same as the
/// secondary's (BCCH without MCCH, frame 18 included) over two multiframes.
#[test]
fn third_carrier_sends_what_the_secondary_sends() {
    debug::setup_logging_verbose();
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    config.cell.extra_carriers = vec![THIRD_CARRIER];
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    test.run_stack(Some(2 * 18 * 4 + 8));

    let ticks: Vec<Vec<tetra_saps::tmv::TmvUnitdataReqSlot>> = test
        .dump_sinks()
        .into_iter()
        .filter_map(|msg| match msg.msg {
            SapMsgInner::TmvUnitdataReqSlots(slots) => Some(slots.slots),
            _ => None,
        })
        .collect();
    assert!(ticks.len() >= 2 * 18 * 4, "{} ticks", ticks.len());
    let mut frame18 = 0;
    for slots in &ticks {
        let carriers: Vec<u16> = slots.iter().map(|s| s.carrier_num).collect();
        assert_eq!(carriers, vec![MAIN_CARRIER, SECONDARY_CARRIER, THIRD_CARRIER]);
        let mut third = slots[2].clone();
        third.carrier_num = SECONDARY_CARRIER;
        assert_eq!(format!("{third:?}"), format!("{:?}", slots[1]), "at {}", slots[0].ts);
        if slots[0].ts.f == 18 {
            frame18 += 1;
        }
    }
    assert!(frame18 >= 4, "frame 18 seen");
}

// ---------------------------------------------------------------------------------------------
// Carriers on demand (`secondary_carrier_on_demand`): every carrier but the main one on the air
// only while it is needed, off again a grace after.
// ---------------------------------------------------------------------------------------------

type Cfg = tetra_config::bluestation::StackConfig;
type Slot = tetra_saps::tmv::TmvUnitdataReqSlot;

/// The feature off, with a warm-up and a grace that would change everything if it were on.
fn on_demand_off(cfg: &mut Cfg) {
    cfg.cell.secondary_carrier_on_demand = false;
    cfg.cell.secondary_carrier_warmup_frames = 9;
    cfg.cell.secondary_carrier_inactivity_secs = 1;
}

fn on_demand_off_with_a_packet_data_carrier_and_a_third(cfg: &mut Cfg) {
    packet_data_carrier_and_a_third(cfg);
    on_demand_off(cfg);
}

/// With `secondary_carrier_on_demand = false` the carriers are what they were, whatever the
/// warm-up and the grace say.
#[test]
fn on_demand_off_is_today() {
    assert_eq!(
        secondary_scenarios_with(on_demand_off),
        [
            SECONDARY_IDENTITY_MCCH_FALLBACK,
            SECONDARY_IDENTITY_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_TS1_CLOSE
        ]
    );
    assert_eq!(
        crate::common::component_test::with_trace_skip_carrier(THIRD_CARRIER, || secondary_scenarios_with(
            on_demand_off_with_a_packet_data_carrier_and_a_third
        )),
        [
            SECONDARY_IDENTITY_PDCH_MCCH_FALLBACK,
            SECONDARY_IDENTITY_PDCH_TS1_CHAN_ALLOC,
            SECONDARY_IDENTITY_PDCH_TS1_CLOSE
        ]
    );
}

fn with_the_secondary(cfg: &mut Cfg) {
    cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
}

fn with_the_secondary_on_demand(cfg: &mut Cfg) {
    with_the_secondary(cfg);
    cfg.cell.secondary_carrier_on_demand = true;
}

/// Scenarios on the main carrier only: with the secondary on demand (off: nothing needs it) the
/// main carrier sends what it sends with the secondary always on, and without one.
#[test]
fn on_demand_leaves_the_main_carrier_as_it_was() {
    use crate::common::component_test::{with_config_tweak, with_trace_skip_carrier};
    for scenario in [
        test_dl_block_queue_is_bounded_under_burst as fn(),
        test_local_loopback_ul_still_loops_back_and_reaches_brew,
    ] {
        let main_only =
            |tweak: fn(&mut Cfg)| with_trace_skip_carrier(SECONDARY_CARRIER, || with_config_tweak(tweak, || fingerprint(scenario)));
        let on_demand = main_only(with_the_secondary_on_demand);
        assert_eq!(on_demand, main_only(with_the_secondary));
        assert_eq!(on_demand, fingerprint(scenario));
    }
}

/// A cell with the secondary carrier (and with `third` a third one) on demand.
fn on_demand_config(third: bool, warmup_frames: u8, inactivity_secs: u16) -> Cfg {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    if third {
        config.cell.extra_carriers = vec![THIRD_CARRIER];
    }
    config.cell.secondary_carrier_on_demand = true;
    config.cell.secondary_carrier_warmup_frames = warmup_frames;
    config.cell.secondary_carrier_inactivity_secs = inactivity_secs;
    config
}

/// The UMAC, with the LMAC, LLC and Brew as sinks, from downlink time `start`.
fn on_demand_test(config: Cfg, start: TdmaTime) -> ComponentTest {
    let mut test = ComponentTest::from_config(config, Some(start));
    test.populate_entities(
        vec![TetraEntity::Umac],
        vec![TetraEntity::Lmac, TetraEntity::Llc, TetraEntity::Brew],
    );
    test
}

const START: TdmaTime = TdmaTime { h: 0, m: 1, f: 1, t: 1 };

/// Run `ticks` ticks: the slots each one sent to the LMAC, and every other message.
fn run_ticks(test: &mut ComponentTest, ticks: usize) -> (Vec<Vec<Slot>>, Vec<SapMsg>) {
    let (mut batches, mut other) = (Vec::new(), Vec::new());
    for _ in 0..ticks {
        test.run_stack(Some(1));
        for msg in test.dump_sinks() {
            match msg.msg {
                SapMsgInner::TmvUnitdataReqSlots(slots) => batches.push(slots.slots),
                _ => other.push(msg),
            }
        }
    }
    (batches, other)
}

fn on_air(batch: &[Slot]) -> Vec<u16> {
    batch.iter().map(|s| s.carrier_num).collect()
}

/// How many batches from the first carry `carrier`; none after them does.
fn leading_with(batches: &[Vec<Slot>], carrier: u16) -> usize {
    let n = batches.iter().take_while(|b| on_air(b).contains(&carrier)).count();
    assert!(
        batches[n..].iter().all(|b| !on_air(b).contains(&carrier)),
        "carrier {carrier} back on the air after {n} slots"
    );
    n
}

fn open_on(carrier_num: u16, ts: u8) -> SapMsg {
    let mut open = open_shared_voice_circuit(ts);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut open.msg {
        circuit.carrier_num = carrier_num;
    }
    open
}

fn close_on(carrier_num: u16, ts: u8) -> SapMsg {
    SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::CloseSlot {
            direction: Direction::Both,
            carrier_num,
            ts,
        }),
    }
}

/// A PDU for `dest` on the MCCH, with a channel allocation to `timeslots` of a carrier if given.
fn mcch_pdu(dest: TetraAddress, alloc: Option<(u16, [bool; 4])>) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr("1010101010101010"),
            main_address: dest,
            link_id: 1,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: alloc.map(|a| a.0),
            chan_alloc: alloc.map(|(carrier, timeslots)| CmceChanAllocReq {
                usage: Some(26),
                carrier: Some(carrier),
                timeslots,
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            tx_reporter: None,
        }),
    }
}

/// Every MAC-RESOURCE in the SCH/F block of `slot`, in order.
fn mac_resources_in(slot: &Slot) -> Vec<MacResource> {
    let Some(blk) = slot.blk1.as_ref().filter(|b| b.logical_channel == LogicalChannel::SchF) else {
        return Vec::new();
    };
    let mut block = blk.mac_block.clone();
    block.seek(0);
    let bits = block.to_bitstr();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 16 <= bits.len() {
        let mut b = BitBuffer::from_bitstr(&bits[pos..]);
        b.seek(0);
        if b.peek_bits(2) != Some(0) {
            break;
        }
        let Ok(res) = MacResource::from_bitbuf(&mut b) else {
            break;
        };
        let li = res.length_ind;
        if res.addr.is_none() {
            break;
        }
        out.push(res);
        if li == 0 || li == 0b111111 {
            break;
        }
        pos += li as usize * 8;
    }
    out
}

/// Time of the first main-carrier ts1 slot with a MAC-RESOURCE for `ssi`, with a channel
/// allocation or without.
fn first_on_mcch(batches: &[Vec<Slot>], ssi: u32, chan_alloc: bool) -> Option<TdmaTime> {
    batches
        .iter()
        .flatten()
        .filter(|s| s.carrier_num == MAIN_CARRIER && s.ts.t == 1)
        .find(|s| {
            mac_resources_in(s)
                .iter()
                .any(|r| r.addr.is_some_and(|a| a.ssi == ssi) && r.chan_alloc_element.is_some() == chan_alloc)
        })
        .map(|s| s.ts)
}

/// The first ts1 slot that is not in frame 18 at or after `t`.
fn first_mcch_from(batches: &[Vec<Slot>], t: TdmaTime) -> TdmaTime {
    batches
        .iter()
        .map(|b| b[0].ts)
        .find(|ts| ts.diff(t) >= 0 && ts.t == 1 && ts.f != 18)
        .expect("an MCCH slot")
}

/// An idle cell of three carriers on demand sends the main carrier only.
#[test]
fn on_demand_idle_cell_sends_the_main_carrier_only() {
    debug::setup_logging_verbose();
    let mut test = on_demand_test(on_demand_config(true, 4, 180), START);
    let (batches, _) = run_ticks(&mut test, 2 * 18 * 4);
    assert_eq!(batches.len(), 2 * 18 * 4);
    assert!(batches.iter().all(|b| on_air(b) == vec![MAIN_CARRIER]));
}

/// The carrier the next call would go to is on the air as soon as the carriers before it are
/// full, without a call on it; with the packet-data carrier last in the voice order, the other
/// one. Once a slot of the main carrier is free again it stays on for the grace, then goes off.
#[test]
fn on_demand_the_next_call_carrier_is_warmed() {
    debug::setup_logging_verbose();
    let fill_main = |test: &ComponentTest, owner: Option<tetra_core::TimeslotOwner>| {
        let mut state = test.config.state_write();
        for ts in 2..=4 {
            match owner {
                Some(owner) => state.timeslot_alloc.reserve(owner, ts).unwrap(),
                None => state.timeslot_alloc.release(tetra_core::TimeslotOwner::Cmce, ts).unwrap(),
            }
        }
    };
    let mut test = on_demand_test(on_demand_config(false, 4, 1), START);
    let (batches, _) = run_ticks(&mut test, 8);
    assert!(batches.iter().all(|b| on_air(b) == vec![MAIN_CARRIER]));
    fill_main(&test, Some(tetra_core::TimeslotOwner::Cmce));
    let (batches, _) = run_ticks(&mut test, 40);
    assert!(batches.iter().all(|b| on_air(b) == vec![MAIN_CARRIER, SECONDARY_CARRIER]));
    let umac = umac_of(&mut test);
    assert!(
        (1..=4).all(|ts| !umac.circuit_is_active_on(SECONDARY_CARRIER, Direction::Dl, ts)),
        "no call on it"
    );
    fill_main(&test, None);
    let (batches, _) = run_ticks(&mut test, 100);
    assert_eq!(leading_with(&batches, SECONDARY_CARRIER), 72, "on for the grace (1 s)");

    // Three carriers, the secondary the packet-data carrier: voice goes to the third one first.
    let mut cfg = on_demand_config(true, 4, 180);
    packet_data_carrier_without_a_session(&mut cfg);
    let mut test = on_demand_test(cfg, START);
    run_ticks(&mut test, 4);
    fill_main(&test, Some(tetra_core::TimeslotOwner::Brew));
    let (batches, _) = run_ticks(&mut test, 40);
    assert!(
        batches.iter().all(|b| on_air(b) == vec![MAIN_CARRIER, THIRD_CARRIER]),
        "{:?}",
        on_air(&batches[0])
    );
}

/// A circuit puts its carrier on the air; after its close it stays on for the grace and goes off.
/// A call on it within the grace keeps it on and starts the grace again.
#[test]
fn on_demand_carrier_on_with_a_circuit_off_after_the_grace() {
    debug::setup_logging_verbose();
    let mut test = on_demand_test(on_demand_config(false, 4, 1), START);
    run_ticks(&mut test, 4);
    test.submit_message(open_on(SECONDARY_CARRIER, 1));
    let (batches, _) = run_ticks(&mut test, 2);
    // The open is taken after the first tick's slots were built.
    assert_eq!(on_air(&batches[0]), vec![MAIN_CARRIER]);
    assert_eq!(on_air(&batches[1]), vec![MAIN_CARRIER, SECONDARY_CARRIER]);
    let (batches, _) = run_ticks(&mut test, 100);
    assert!(batches.iter().all(|b| on_air(b).contains(&SECONDARY_CARRIER)));

    // The tick of the close, the one its deferred close runs in, then the grace.
    test.submit_message(close_on(SECONDARY_CARRIER, 1));
    let (batches, _) = run_ticks(&mut test, 200);
    assert_eq!(leading_with(&batches, SECONDARY_CARRIER), 2 + 72);

    // Another call within the grace.
    test.submit_message(open_on(SECONDARY_CARRIER, 1));
    run_ticks(&mut test, 10);
    test.submit_message(close_on(SECONDARY_CARRIER, 1));
    let (batches, _) = run_ticks(&mut test, 40);
    assert!(batches.iter().all(|b| on_air(b).contains(&SECONDARY_CARRIER)));
    test.submit_message(open_on(SECONDARY_CARRIER, 2));
    let (batches, _) = run_ticks(&mut test, 60);
    assert!(
        batches.iter().all(|b| on_air(b).contains(&SECONDARY_CARRIER)),
        "kept on by the second call"
    );
    test.submit_message(close_on(SECONDARY_CARRIER, 2));
    let (batches, _) = run_ticks(&mut test, 200);
    assert_eq!(leading_with(&batches, SECONDARY_CARRIER), 2 + 72, "the grace again from its close");
}

/// A channel allocation to a carrier switched on for it waits until the carrier has been on the
/// air for the warm-up, then goes on the next MCCH slot; a PDU for another radio queued behind it
/// goes first. Without a warm-up it goes when it would with the carrier always on.
#[test]
fn on_demand_channel_allocation_waits_for_the_warm_up() {
    debug::setup_logging_verbose();
    let (x, y) = (TetraAddress::issi(9012001), TetraAddress::issi(9012002));
    for warmup in [4u8, 0] {
        let mut test = on_demand_test(on_demand_config(false, warmup, 180), START);
        test.submit_message(mcch_pdu(x, Some((SECONDARY_CARRIER, [false, true, false, false]))));
        test.submit_message(mcch_pdu(y, None));
        let (batches, _) = run_ticks(&mut test, 3 * 72);
        let t_on = batches
            .iter()
            .find(|b| on_air(b).contains(&SECONDARY_CARRIER))
            .map(|b| b[0].ts)
            .expect("the carrier went on the air");
        let t_ca = first_on_mcch(&batches, x.ssi, true).expect("the allocation went out");
        let t_y = first_on_mcch(&batches, y.ssi, false).expect("the other PDU went out");
        let warmup_slots = 4 * i32::from(warmup);
        assert!(t_ca.diff(t_on) >= warmup_slots, "warm-up {warmup}: on {t_on}, allocation {t_ca}");
        assert_eq!(
            t_ca,
            first_mcch_from(&batches, t_on.add_timeslots(warmup_slots)),
            "warm-up {warmup}"
        );
        if warmup > 0 {
            assert!(t_y.diff(t_ca) < 0, "the other radio's PDU first: {t_y}, {t_ca}");
        }
    }
}

/// A carrier on the air (a call on ts1) takes a channel allocation to it on the next MCCH slot.
#[test]
fn on_demand_a_warm_carrier_takes_an_allocation_at_once() {
    debug::setup_logging_verbose();
    let x = TetraAddress::issi(9012001);
    let mut test = on_demand_test(on_demand_config(false, 4, 180), START);
    test.submit_message(open_on(SECONDARY_CARRIER, 1));
    run_ticks(&mut test, 30);
    test.submit_message(mcch_pdu(x, Some((SECONDARY_CARRIER, [false, true, false, false]))));
    let (batches, _) = run_ticks(&mut test, 12);
    // The PDU is taken after the first tick's slot was built.
    let next_mcch = first_mcch_from(&batches, batches[1][0].ts);
    assert_eq!(first_on_mcch(&batches, x.ssi, true), Some(next_mcch));
}

/// Three carriers: each goes on and off on its own.
#[test]
fn on_demand_carriers_are_independent() {
    debug::setup_logging_verbose();
    let mut test = on_demand_test(on_demand_config(true, 4, 1), START);
    test.submit_message(open_on(SECONDARY_CARRIER, 1));
    test.submit_message(open_on(THIRD_CARRIER, 1));
    let (batches, _) = run_ticks(&mut test, 30);
    assert_eq!(
        on_air(batches.last().unwrap()),
        vec![MAIN_CARRIER, SECONDARY_CARRIER, THIRD_CARRIER]
    );
    test.submit_message(close_on(SECONDARY_CARRIER, 1));
    let (batches, _) = run_ticks(&mut test, 150);
    assert_eq!(leading_with(&batches, SECONDARY_CARRIER), 2 + 72);
    assert!(
        batches.iter().all(|b| on_air(b).contains(&THIRD_CARRIER)),
        "the third stays on its call"
    );
}

/// What is received on a carrier not on the air long enough is a copy: a random access on a
/// carrier that is off reaches no one and does not put it on the air (with the feature off it is
/// taken); the uplink voice of a circuit on a carrier warming up reaches neither Brew nor the
/// downlink, and once the carrier is on the air it does.
#[test]
fn on_demand_drops_the_uplink_of_a_cold_carrier() {
    debug::setup_logging_verbose();
    let radio = TetraAddress::issi(2200699);
    // Uplink slot ts2: the tick at ts4 takes it.
    let ts2_start = TdmaTime { h: 0, m: 1, f: 1, t: 4 };
    for on_demand in [true, false] {
        let mut cfg = on_demand_config(false, 4, 180);
        cfg.cell.secondary_carrier_on_demand = on_demand;
        let mut test = on_demand_test(cfg, ts2_start);
        test.submit_message(sch_hu_mac_access(SECONDARY_CARRIER, radio));
        let (batches, other) = run_ticks(&mut test, 8);
        let to_llc = other.iter().filter(|m| matches!(m.msg, SapMsgInner::TmaUnitdataInd(_))).count();
        if on_demand {
            assert_eq!(to_llc, 0, "dropped");
            assert!(
                batches.iter().all(|b| on_air(b) == vec![MAIN_CARRIER]),
                "no acknowledgement put it on the air"
            );
        } else {
            assert_eq!(to_llc, 1, "taken with the feature off");
        }
    }

    let mut cfg = on_demand_config(false, 4, 180);
    cfg.brew = Some(CfgBrew {
        host: "127.0.0.1".into(),
        port: 0,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        pbx_gateway_issis: None,
    });
    let mut test = on_demand_test(cfg, START);
    let mut open = open_on(SECONDARY_CARRIER, 1);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut open.msg {
        circuit.dl_media_source = CircuitDlMediaSource::LocalLoopback;
    }
    test.submit_message(open);
    run_ticks(&mut test, 1);
    let ul_bits: Vec<u8> = (0..274).map(|idx| ((idx * 7 + idx / 3) % 2) as u8).collect();
    let voice = || SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            carrier_num: SECONDARY_CARRIER,
            ts: 1,
            data: ul_bits.clone(),
        }),
    };
    let heard = |batches: &[Vec<Slot>], other: &[SapMsg]| {
        let to_brew = other
            .iter()
            .filter(|m| m.dest == TetraEntity::Brew && matches!(m.msg, SapMsgInner::TmdCircuitDataInd(_)))
            .count();
        let looped_back = batches
            .iter()
            .flatten()
            .filter(|s| s.carrier_num == SECONDARY_CARRIER && s.ts.t == 1)
            .flat_map(|s| [&s.blk1, &s.blk2].into_iter().flatten())
            .filter(|blk| blk.logical_channel == LogicalChannel::TchS)
            .any(|blk| (0..274).all(|i| blk.mac_block.peek_bits_startoffset(i, 1) == Some(ul_bits[i] as u64)));
        (to_brew, looped_back)
    };
    // Warming up (on in the tick after the open, 16 slots).
    test.submit_message(voice());
    let (batches, other) = run_ticks(&mut test, 12);
    assert_eq!(heard(&batches, &other), (0, false), "a copy while warming up");
    run_ticks(&mut test, 12);
    test.submit_message(voice());
    let (batches, other) = run_ticks(&mut test, 12);
    assert_eq!(heard(&batches, &other), (1, true), "on the air: the radio's own");
}

#[test]
fn test_local_loopback_ul_still_loops_back_and_reaches_brew() {
    // Only a LocalParrot circuit hands its uplink to CMCE: an ordinary simplex circuit keeps
    // looping it back to the DL and forwarding it to Brew, and CMCE gets nothing.
    debug::setup_logging_verbose();

    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.brew = Some(CfgBrew {
        host: "127.0.0.1".into(),
        port: 0,
        tls: false,
        username: None,
        password: None,
        reconnect_delay: std::time::Duration::from_secs(1),
        jitter_initial_latency_frames: 0,
        feature_sds_enabled: true,
        whitelisted_ssis: None,
        feature_rssi_export: false,
        pbx_gateway_issis: None,
    });
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 4 }));
    test.populate_entities(
        vec![TetraEntity::Umac],
        vec![TetraEntity::Cmce, TetraEntity::Lmac, TetraEntity::Brew],
    );

    let mut open = open_shared_voice_circuit(2);
    if let SapMsgInner::CmceCallControl(CallControl::Open(circuit)) = &mut open.msg {
        circuit.dl_media_source = CircuitDlMediaSource::LocalLoopback;
    }
    test.submit_message(open);
    test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::FloorGranted {
            call_id: 1,
            source_issi: 1000001,
            dest_gssi: 91,
            carrier_num: MAIN_CARRIER,
            ts: 2,
        }),
    });
    test.run_stack(Some(1));
    let _ = test.dump_sinks();

    let ul_bits: Vec<u8> = (0..274).map(|idx| ((idx * 7 + idx / 3) % 2) as u8).collect();
    test.submit_message(SapMsg {
        sap: Sap::TmdSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmdCircuitDataInd(TmdCircuitDataInd {
            carrier_num: MAIN_CARRIER,
            ts: 2,
            data: ul_bits.clone(),
        }),
    });
    test.run_stack(Some(12));
    let msgs = test.dump_sinks();

    let tmd_inds_to = |dest: TetraEntity| {
        msgs.iter()
            .filter(|msg| msg.dest == dest && matches!(&msg.msg, SapMsgInner::TmdCircuitDataInd(_)))
            .count()
    };
    assert_eq!(tmd_inds_to(TetraEntity::Brew), 1, "the uplink is still forwarded to Brew");
    assert_eq!(tmd_inds_to(TetraEntity::Cmce), 0, "only parrot circuits send their uplink to CMCE");

    let looped_back = msgs
        .iter()
        .filter_map(|msg| match &msg.msg {
            SapMsgInner::TmvUnitdataReqSlots(req) => Some(req),
            _ => None,
        })
        .flat_map(|req| req.slots.iter())
        .filter(|slot| slot.carrier_num == MAIN_CARRIER && slot.ts.t == 2)
        .flat_map(|slot| [&slot.blk1, &slot.blk2].into_iter().flatten())
        .filter(|blk| blk.logical_channel == LogicalChannel::TchS)
        .any(|blk| (0..274).all(|i| blk.mac_block.peek_bits_startoffset(i, 1) == Some(ul_bits[i] as u64)));
    assert!(looped_back, "the uplink is still looped back to the DL");
}
