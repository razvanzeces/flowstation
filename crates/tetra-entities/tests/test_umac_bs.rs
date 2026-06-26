mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::Direction;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, debug};
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
use tetra_saps::tmd::TmdCircuitDataReq;
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
