mod common;

use tetra_config::bluestation::StackMode;
use tetra_core::Direction;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Layer2Service, PhyBlockNum, Sap, SsiType, TdmaTime, TetraAddress, TxReporter, debug};
use tetra_entities::umac::umac_bs::UmacBs;
use tetra_pdus::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;
use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
use tetra_pdus::umac::pdus::mac_access::MacAccess;
use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
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
            cell_change: false,
        }),
        stealing_slot: None,
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
            cell_change: false,
        }),
        stealing_slot: None,
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
            cell_change: false,
        }),
        stealing_slot: None,
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
            cell_change: false,
        }),
        stealing_slot: None,
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
            cell_change: false,
        }),
        stealing_slot: None,
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
/// not fit in one 124-bit STCH half-slot must NOT be written into a fixed 124-bit buffer, which
/// panicked the whole stack ("write would exceed buffer end") and was a remotely-triggerable
/// crash: sending an SDS or status longer than one half-slot to an MS engaged in a call took
/// down the BS. A PDU that does not fit one stolen slot (both halves) goes to the MCCH, since
/// fragmentation on the STCH is only permitted within one slot (TS 100 392-2 clause 23.4.2.1.7).
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
            cell_change: false,
        }),
        stealing_slot: None,
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
    assert!(
        stch_slots(test.dump_sinks(), ts).is_empty(),
        "a PDU longer than one stolen slot must not be spread over stolen half slots of several frames"
    );
}

fn stch_slots(sink_msgs: Vec<SapMsg>, ts: u8) -> Vec<tetra_saps::tmv::TmvUnitdataReqSlot> {
    let mut found = Vec::new();
    for msg in sink_msgs {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        found.extend(slots.into_iter().filter(|slot| {
            slot.ts.t == ts && slot.blk1.as_ref().is_some_and(|b| b.logical_channel == LogicalChannel::Stch)
        }));
    }
    found
}

fn stealing_req(dest: TetraAddress, sdu: &str, stealing_slot: Option<(u16, u8)>, chan_alloc: Option<CmceChanAllocReq>, tx_reporter: Option<TxReporter>) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr(sdu),
            main_address: dest,
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: true,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: Some(MAIN_CARRIER),
            chan_alloc,
            stealing_slot,
            tx_reporter,
        }),
    }
}

/// Channel allocation for a channel on another cell (announced cell reselection type 1).
fn other_cell_alloc() -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: Some(9),
        carrier: Some(1530),
        timeslots: [false, false, true, false],
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
        cell_change: true,
    }
}

fn sdu_bits(len: usize) -> String {
    (0..len).map(|i| if (i * 7) % 5 < 2 { '1' } else { '0' }).collect()
}

fn read_bits(buf: &mut BitBuffer, len: usize) -> String {
    (0..len).map(|_| if buf.read_bits(1).unwrap() == 1 { '1' } else { '0' }).collect()
}

/// A PDU too long for the first half slot is fragmented over both halves of one stolen slot:
/// MAC-RESOURCE with "Length indication" 111111 in the first half and the final fragment in a
/// MAC-END in the second half (TS 100 392-2 clause 23.8.4.2.1 iv).
#[test]
fn test_stealing_fragments_within_one_slot_using_both_half_slots() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    let ts = 3u8;
    let dest = TetraAddress { ssi: 2260577, ssi_type: SsiType::Issi };
    test.submit_message(open_shared_voice_circuit(ts));
    test.run_stack(Some(1));
    test.dump_sinks();

    let sdu = sdu_bits(150);
    let reporter = TxReporter::new_unacked();
    test.submit_message(stealing_req(dest, &sdu, Some((MAIN_CARRIER, ts)), None, Some(reporter.clone())));
    test.run_stack(Some(8));

    let slots = stch_slots(test.dump_sinks(), ts);
    assert_eq!(slots.len(), 1, "one stolen slot carries the whole PDU");
    let slot = &slots[0];
    let mut first = slot.blk1.as_ref().unwrap().mac_block.clone();
    let second = slot.blk2.as_ref().expect("second half slot");
    assert_eq!(second.logical_channel, LogicalChannel::Stch, "second half slot is stolen too");
    first.seek(0);
    let res = MacResource::from_bitbuf(&mut first).unwrap();
    assert_eq!(res.length_ind, 0b111111, "start of fragmentation, second half slot stolen");
    assert!(res.chan_alloc_element.is_none());
    let first_bits = 124 - res.compute_header_len();
    let mut got = read_bits(&mut first, first_bits);

    let mut second = second.mac_block.clone();
    second.seek(0);
    let end = MacEndDl::from_bitbuf(&mut second).unwrap();
    assert!(end.chan_alloc_element.is_none());
    got.push_str(&read_bits(&mut second, 150 - first_bits));
    assert_eq!(got, sdu, "both fragments together are the TM-SDU");
    assert!(reporter.is_transmitted());
}

/// A type 1 channel allocation is sent on the STCH of the traffic channel the MS is on, with the
/// "cell change flag" set and the channel of the other cell (clause 23.5.4.1).
#[test]
fn test_cell_change_allocation_goes_on_the_traffic_channel_stch() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    let ts = 2u8;
    let dest = TetraAddress { ssi: 2260578, ssi_type: SsiType::Issi };
    test.submit_message(open_shared_voice_circuit(ts));
    test.run_stack(Some(1));
    test.dump_sinks();

    let sdu = sdu_bits(24);
    test.submit_message(stealing_req(dest, &sdu, Some((MAIN_CARRIER, ts)), Some(other_cell_alloc()), None));
    test.run_stack(Some(8));

    let slots = stch_slots(test.dump_sinks(), ts);
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].blk2.as_ref().unwrap().logical_channel, LogicalChannel::TchS, "fits the first half slot");
    let mut block = slots[0].blk1.as_ref().unwrap().mac_block.clone();
    block.seek(0);
    let res = MacResource::from_bitbuf(&mut block).unwrap();
    let alloc = res.chan_alloc_element.expect("channel allocation");
    assert!(alloc.cell_change_flag);
    assert_eq!(alloc.carrier_num, 1530);
    assert_eq!(alloc.ts_assigned, [false, false, true, false], "timeslot numbering of the new cell");
    assert_eq!(res.usage_marker, Some(9));
    assert_eq!(read_bits(&mut block, 24), sdu);
}

/// With a fragmented message the channel allocation goes in the MAC-END, not in the
/// MAC-RESOURCE (clause 23.5.4.1).
#[test]
fn test_cell_change_allocation_of_a_fragmented_message_goes_in_the_mac_end() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    let ts = 2u8;
    let dest = TetraAddress { ssi: 2260579, ssi_type: SsiType::Issi };
    test.submit_message(open_shared_voice_circuit(ts));
    test.run_stack(Some(1));
    test.dump_sinks();

    let sdu = sdu_bits(100);
    test.submit_message(stealing_req(dest, &sdu, Some((MAIN_CARRIER, ts)), Some(other_cell_alloc()), None));
    test.run_stack(Some(8));

    let slots = stch_slots(test.dump_sinks(), ts);
    assert_eq!(slots.len(), 1);
    let mut first = slots[0].blk1.as_ref().unwrap().mac_block.clone();
    first.seek(0);
    let res = MacResource::from_bitbuf(&mut first).unwrap();
    assert_eq!(res.length_ind, 0b111111);
    assert!(res.chan_alloc_element.is_none(), "no channel allocation in the MAC-RESOURCE of a fragmented message");
    let mut second = slots[0].blk2.as_ref().unwrap().mac_block.clone();
    second.seek(0);
    let end = MacEndDl::from_bitbuf(&mut second).unwrap();
    let alloc = end.chan_alloc_element.expect("channel allocation in the MAC-END");
    assert!(alloc.cell_change_flag);
    assert_eq!(alloc.carrier_num, 1530);
}

/// Without a traffic channel to steal from, a type 1 allocation is not sent at all: it would
/// otherwise go out on the MCCH as an allocation for this cell's numbering.
#[test]
fn test_cell_change_allocation_without_traffic_channel_is_discarded() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    let dest = TetraAddress { ssi: 2260580, ssi_type: SsiType::Issi };
    let reporter = TxReporter::new_unacked();
    test.submit_message(stealing_req(dest, &sdu_bits(24), Some((MAIN_CARRIER, 2)), Some(other_cell_alloc()), Some(reporter.clone())));
    test.run_stack(Some(8));
    let sinks = test.dump_sinks();
    assert!(reporter.is_discarded());
    for msg in sinks {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            assert!(block_mac_resource_for(&slot.blk1, dest).is_none(), "nothing goes out for {}", dest);
        }
    }
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

/// Air interface encryption: an encrypted MAC-ACCESS (ESI address, TM-SDU encrypted with the
/// uplink key stream of its slot) reaches LLC with the real ISSI and the clear TM-SDU.
#[test]
fn test_encrypted_mac_access_is_decrypted() {
    use tetra_config::bluestation::{CfgAie, CipherKey};
    use tetra_entities::aie::cipher::CellCipher;

    debug::setup_logging_verbose();
    let dltime = TdmaTime::default().add_timeslots(2);
    let ul_time = dltime.add_timeslots(-2);
    let issi = 2200699;

    let mut test = ComponentTest::new(StackMode::Bs, Some(dltime));
    let cfg = test.get_shared_config();
    let c = cfg.config();
    let aie = CfgAie {
        enabled: true,
        ksg: 1,
        sckn: 1,
        sck: CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]),
        sck_vn: 0,
        clear_groups: vec![],
    };
    let cipher = std::sync::Arc::new(CellCipher::new(&aie, &[c.cell.main_carrier], c.cell.location_area, c.cell.colour_code));
    let carrier = c.cell.main_carrier;
    drop(c);
    let mut umac = UmacBs::new(cfg, None);
    umac.set_cipher(Some(cipher.clone()));
    test.register_entity(umac);
    test.populate_entities(vec![], vec![TetraEntity::Llc]);

    // MAC-ACCESS of 8 octets: header with the ESI, then an encrypted TM-SDU.
    let mut uplink = BitBuffer::new_autoexpand(64);
    MacAccess {
        fill_bits: false,
        encrypted: true,
        addr: Some(TetraAddress { ssi: cipher.esi(issi), ssi_type: SsiType::Esi }),
        event_label: None,
        length_ind: Some(8),
        frag_flag: None,
        reservation_req: None,
    }
    .to_bitbuf(&mut uplink);
    let hdr_len = uplink.get_pos();
    let sdu_len = 64 - hdr_len;
    let clear_sdu: Vec<u8> = (0..sdu_len).map(|i| ((i * 7 + 3) % 5 == 0) as u8).collect();
    for &b in &clear_sdu {
        uplink.write_bit(b);
    }
    cipher.kss(carrier, ul_time, true).apply(&mut uplink, hdr_len, sdu_len, 0);
    uplink.seek(0);

    test.submit_message(SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(TmvUnitdataInd {
            carrier_num: carrier,
            pdu: uplink,
            block_num: PhyBlockNum::Block1,
            logical_channel: LogicalChannel::SchHu,
            crc_pass: true,
            scrambling_code: 864282631,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    });
    test.run_stack(Some(1));

    let delivered: Vec<TmaUnitdataInd> = test
        .dump_sinks()
        .into_iter()
        .filter_map(|m| match m.msg {
            SapMsgInner::TmaUnitdataInd(ind) => Some(ind),
            _ => None,
        })
        .collect();
    assert_eq!(delivered.len(), 1, "one TM-SDU for LLC");
    let ind = &delivered[0];
    assert_eq!(ind.main_address.ssi, issi, "ESI mapped back to the ISSI");
    assert_eq!(ind.air_interface_encryption, 1);
    let mut sdu = BitBuffer::from_bitbuffer(ind.pdu.as_ref().unwrap());
    sdu.seek(0);
    let got: Vec<u8> = (0..sdu_len).map(|_| sdu.read_bit().unwrap()).collect();
    assert_eq!(got, clear_sdu, "TM-SDU decrypted");
}

/// Air interface encryption, downlink: signalling to a radio registered encrypted goes out with
/// its ESI, a non-zero encryption mode and an encrypted TM-SDU; the same PDU to a clear radio
/// stays clear.
#[test]
fn test_downlink_to_encrypted_radio_is_encrypted() {
    use tetra_config::bluestation::{CfgAie, CipherKey};
    use tetra_entities::aie::cipher::CellCipher;

    debug::setup_logging_verbose();
    let encrypted_issi = 2200701;
    let clear_issi = 2200702;
    let sdu_bits = "1010101011110000110011001010101011110000";

    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    let cfg = test.get_shared_config();
    {
        let mut state = cfg.state_write();
        for issi in [encrypted_issi, clear_issi] {
            state.subscribers.register(issi);
        }
        state.subscribers.set_encrypted(encrypted_issi, true);
    }
    let c = cfg.config();
    let aie = CfgAie {
        enabled: true,
        ksg: 1,
        sckn: 1,
        sck: CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]),
        sck_vn: 1,
        clear_groups: vec![],
    };
    let cipher = std::sync::Arc::new(CellCipher::new(&aie, &[c.cell.main_carrier], c.cell.location_area, c.cell.colour_code));
    drop(c);
    let mut umac = UmacBs::new(cfg, None);
    umac.set_cipher(Some(cipher.clone()));
    test.register_entity(umac);
    test.populate_entities(vec![], vec![TetraEntity::Lmac]);

    for issi in [encrypted_issi, clear_issi] {
        test.submit_message(SapMsg {
            sap: Sap::TmaSap,
            src: TetraEntity::Llc,
            dest: TetraEntity::Umac,
            msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
                req_handle: 0,
                pdu: BitBuffer::from_bitstr(sdu_bits),
                main_address: TetraAddress::issi(issi),
                link_id: 1,
                endpoint_id: 0,
                stealing_permission: false,
                subscriber_class: 0,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_category: None,
                carrier_num: None,
                chan_alloc: None,
                stealing_slot: None,
                tx_reporter: None,
            }),
        });
    }
    test.run_stack(Some(16));

    let mut encrypted_seen = None;
    let mut clear_seen = None;
    let esi = cipher.esi(encrypted_issi);
    for msg in test.dump_sinks() {
        let slots = match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => continue,
        };
        for slot in slots {
            let Some(blk) = &slot.blk1 else { continue };
            // Walk every MAC-RESOURCE of the block.
            let mut start = 0;
            while start + 16 <= blk.mac_block.get_len() {
                let mut b = blk.mac_block.clone();
                b.seek(start);
                if b.peek_bits(2) != Some(0) {
                    break;
                }
                let Ok(pdu) = MacResource::from_bitbuf(&mut b) else { break };
                if pdu.length_ind == 0 || pdu.length_ind > 0b111000 {
                    break;
                }
                match pdu.addr.map(|a| a.ssi) {
                    Some(ssi) if ssi == esi => encrypted_seen = Some((pdu.clone(), blk.mac_block.clone(), start, slot.ts, slot.carrier_num)),
                    Some(ssi) if ssi == clear_issi => clear_seen = Some(pdu.clone()),
                    _ => {}
                }
                start += pdu.length_ind as usize * 8;
            }
        }
    }

    let clear = clear_seen.expect("PDU to the clear radio sent with its ISSI");
    assert_eq!(clear.encryption_mode, 0);

    let (pdu, mut block, start, ts, carrier) = encrypted_seen.expect("PDU to the encrypted radio sent with its ESI");
    assert_eq!(pdu.encryption_mode, 0b11, "encrypted, odd SCK-VN");
    let sdu_start = start + pdu.compute_header_len();
    cipher.kss(carrier, ts, false).apply(&mut block, sdu_start, sdu_bits.len(), 0);
    block.seek(sdu_start);
    let clear_sdu: String = (0..sdu_bits.len()).map(|_| if block.read_bit().unwrap() == 1 { '1' } else { '0' }).collect();
    assert_eq!(clear_sdu, sdu_bits, "TM-SDU decrypts with the slot's key stream");
}

#[test]
fn sysinfo_carries_the_configured_rxlev_access_min() {
    use tetra_config::bluestation::SharedConfig;
    // Default config: -110 dBm, code 3 (TS 100 392-2 table 21.65), in both SYSINFO variants.
    let precomps = UmacBs::generate_precomps(&SharedConfig::from_parts(common::default_stack::default_test_config_bs(), None));
    assert_eq!(precomps.mac_sysinfo1.rxlev_access_min, 3);
    assert_eq!(precomps.mac_sysinfo2.rxlev_access_min, 3);

    let mut config = common::default_stack::default_test_config_bs();
    config.cell.rxlev_access_min_dbm = -75;
    let precomps = UmacBs::generate_precomps(&SharedConfig::from_parts(config, None));
    assert_eq!(precomps.mac_sysinfo1.rxlev_access_min, 10);
    assert_eq!(precomps.mac_sysinfo2.rxlev_access_min, 10);
}

#[test]
fn sysinfo_announces_migration_while_multi_network_is_on() {
    use tetra_config::bluestation::SharedConfig;
    let precomps = UmacBs::generate_precomps(&SharedConfig::from_parts(common::default_stack::default_test_config_bs(), None));
    assert!(!precomps.mle_sysinfo.bs_service_details.migration, "off by default");

    // [multi_network] on without [cell_info] migration: BS service details say migration
    // supported (TS 100 392-2 18.5.2.1), so radios of other networks may migrate (16.4.1.1 b).
    let mut config = common::default_stack::default_test_config_bs();
    config.cell.migration = false;
    config.multi_network.enabled = true;
    let precomps = UmacBs::generate_precomps(&SharedConfig::from_parts(config, None));
    assert!(precomps.mle_sysinfo.bs_service_details.migration);
}

/// Channel allocation moving the MS to a traffic channel, sent with `sdu` on the MCCH; returns
/// the MAC-RESOURCE that carried it.
fn mcch_resource_with_chan_alloc(sdu: &str) -> MacResource {
    let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Lmac]);
    let dest = TetraAddress {
        ssi: 2320972,
        ssi_type: SsiType::Issi,
    };
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr(sdu),
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
                usage: Some(8),
                carrier: Some(MAIN_CARRIER),
                timeslots: [false, true, false, false],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                cell_change: false,
            }),
            stealing_slot: None,
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(12));
    test.dump_sinks()
        .into_iter()
        .flat_map(|msg| match msg.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => vec![],
        })
        .flat_map(|slot| [block_mac_resource_for(&slot.blk1, dest), block_mac_resource_for(&slot.blk2, dest)])
        .flatten()
        .find(|pdu| pdu.chan_alloc_element.is_some())
        .expect("the channel allocation goes out on the MCCH")
}

/// Field case (M5a, D-RESTORE ACK): an acknowledged PDU that sends the MS to a traffic channel
/// was repeated on the MCCH after the MS had left, because the MS had no uplink to acknowledge
/// on. A subslot granted on the current channel carries the acknowledgement before the move
/// (TS 100 392-2 clause 23.5.4.3.1 b, "position of grant" 0 = current channel, clause 21.4.3.1).
#[test]
fn test_acknowledged_channel_change_grants_a_subslot_on_the_current_channel() {
    // BL-DATA (0001), N(S) 0, then an MLE PDU.
    let pdu = mcch_resource_with_chan_alloc("00010101100001110000000000100010001110000000000100001011000");
    assert!(pdu.slot_granting_element.is_some(), "the MS needs uplink capacity for its BL-ACK");
    assert_eq!(pdu.pos_of_grant, 0, "the grant is on the current channel");
}

#[test]
fn test_unacknowledged_channel_change_gets_no_grant() {
    // BL-UDATA (0010): nothing to acknowledge.
    let pdu = mcch_resource_with_chan_alloc("00100101100001110000000000100010001110000000000100001011000");
    assert!(pdu.slot_granting_element.is_none());
}

/// FACCH/STCH to a radio registered encrypted (lx-dxb, FACCH/STCH AIE fix): the MAC-RESOURCE on
/// the stolen half slot carries the ESI and a non-zero encryption mode, and its TM-SDU is
/// encrypted with the key stream of the slot it goes out in, from KSS(0) (TS 100 392-7 clause
/// 6.4.1, table 6.4; clause 6.7.1.2). A PDU over both halves of the slot has its MAC-END
/// encrypted from KSS(216). The same PDU to a clear radio stays clear.
#[test]
fn test_facch_to_encrypted_radio_is_encrypted_in_its_slot() {
    use tetra_config::bluestation::{CfgAie, CipherKey};
    use tetra_entities::aie::cipher::CellCipher;

    debug::setup_logging_verbose();
    let encrypted_issi = 2200711;
    let clear_issi = 2200712;
    // Both FACCH paths: the slot named by the request (`stealing_slot`) and the slot taken from
    // the channel allocation hint.
    for (sdu_len, ts, hint) in [(40usize, 2u8, false), (150, 3, false), (40, 2, true)] {
        let mut test = ComponentTest::new(StackMode::Bs, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
        let cfg = test.get_shared_config();
        {
            let mut state = cfg.state_write();
            for issi in [encrypted_issi, clear_issi] {
                state.subscribers.register(issi);
            }
            state.subscribers.set_encrypted(encrypted_issi, true);
        }
        let c = cfg.config();
        let aie = CfgAie {
            enabled: true,
            ksg: 1,
            sckn: 1,
            sck: CipherKey([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x01, 0x23]),
            sck_vn: 1,
            clear_groups: vec![],
        };
        let cipher = std::sync::Arc::new(CellCipher::new(&aie, &[c.cell.main_carrier], c.cell.location_area, c.cell.colour_code));
        let carrier = c.cell.main_carrier;
        drop(c);
        let mut umac = UmacBs::new(cfg, None);
        umac.set_cipher(Some(cipher.clone()));
        test.register_entity(umac);
        test.populate_entities(vec![], vec![TetraEntity::Lmac]);
        test.submit_message(open_shared_voice_circuit(ts));
        test.run_stack(Some(1));
        test.dump_sinks();

        let sdu = sdu_bits(sdu_len);
        for issi in [encrypted_issi, clear_issi] {
            let hint_alloc = CmceChanAllocReq {
                usage: Some(4),
                carrier: Some(MAIN_CARRIER),
                timeslots: std::array::from_fn(|i| i + 1 == ts as usize),
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
                cell_change: false,
            };
            let req = if hint {
                stealing_req(TetraAddress::issi(issi), &sdu, None, Some(hint_alloc), None)
            } else {
                stealing_req(TetraAddress::issi(issi), &sdu, Some((MAIN_CARRIER, ts)), None, None)
            };
            test.submit_message(req);
            test.run_stack(Some(8));
            let slots = stch_slots(test.dump_sinks(), ts);
            assert_eq!(slots.len(), 1, "one stolen slot for {issi}, {sdu_len} bits");
            let slot = &slots[0];
            let mut first = slot.blk1.as_ref().unwrap().mac_block.clone();
            first.seek(0);
            let res = MacResource::from_bitbuf(&mut first).unwrap();
            let hdr_len = first.get_pos();
            let first_bits = if sdu_len > 124 - hdr_len { 124 - hdr_len } else { sdu_len };
            let kss = cipher.kss(carrier, slot.ts, false);
            let mut dec1 = slot.blk1.as_ref().unwrap().mac_block.clone();
            if issi == encrypted_issi {
                assert_eq!(res.addr.unwrap().ssi, cipher.esi(issi), "ESI");
                assert_ne!(res.encryption_mode, 0);
                kss.apply(&mut dec1, hdr_len, first_bits, 0);
            } else {
                assert_eq!((res.addr.unwrap().ssi, res.encryption_mode), (issi, 0), "clear radio stays clear");
            }
            dec1.seek(hdr_len);
            let mut got = read_bits(&mut dec1, first_bits);
            if sdu_len > first_bits {
                let second = slot.blk2.as_ref().expect("second half slot");
                assert_eq!(second.logical_channel, LogicalChannel::Stch);
                let mut b = second.mac_block.clone();
                b.seek(0);
                MacEndDl::from_bitbuf(&mut b).unwrap();
                let end_hdr = b.get_pos();
                let mut dec2 = second.mac_block.clone();
                if issi == encrypted_issi {
                    kss.apply(&mut dec2, end_hdr, sdu_len - first_bits, 216);
                }
                dec2.seek(end_hdr);
                got.push_str(&read_bits(&mut dec2, sdu_len - first_bits));
            }
            assert_eq!(got, sdu, "TM-SDU of {issi} after decryption");
        }
    }
}
