mod common;

use common::ComponentTest;
use tetra_config::bluestation::{CfgWapIp, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap, SsiType, TdmaTime, TetraAddress, debug};
use tetra_entities::sndcp::ip;
use tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage;
use tetra_pdus::umac::enums::access_assign_ul_usage::AccessAssignUlUsage;
use tetra_pdus::umac::pdus::access_assign::AccessAssign;
use tetra_pdus::llc::pdus::bl_ack::BlAck;
use tetra_pdus::umac::pdus::mac_resource::MacResource;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tma::TmaUnitdataInd;
use tetra_saps::tmv::{TmvUnitdataReq, TmvUnitdataReqSlot};

const MAIN_CARRIER: u16 = 1521;
/// The timeslot SNDCP assigns as the packet data channel.
const PDCH_TS: u8 = 4;
/// MLE protocol discriminator for SNDCP.
const MLE_PD_SNDCP: u64 = 0b100;
const SN_UNITDATA: u64 = 4;
const SN_DATA_TRANSMIT_REQUEST: u64 = 6;
const SN_RECONNECT: u64 = 9;

fn ms_address() -> TetraAddress {
    TetraAddress {
        ssi: 2065022,
        ssi_type: SsiType::Issi,
    }
}

/// A BS stack with the WAP gateway enabled and the given entities live.
fn wap_stack(components: Vec<TetraEntity>, sinks: Vec<TetraEntity>) -> ComponentTest {
    let mut config = ComponentTest::get_default_test_config(StackMode::Bs);
    config.cell.wap_ip = Some(CfgWapIp {
        enabled: true,
        address: [10, 0, 0, 1],
        port: 9200,
        response_ttl: 32,
        max_request_payload_bytes: 1024,
    });
    let mut test = ComponentTest::from_config(config, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
    test.populate_entities(components, sinks);
    test
}

/// An uplink SN-PDU as MLE delivers it to SNDCP: the 3-bit MLE discriminator, then the PDU.
fn uplink(link_id: u32, write_pdu: impl FnOnce(&mut BitBuffer)) -> SapMsg {
    let mut sdu = BitBuffer::new_autoexpand(32);
    sdu.write_bits(MLE_PD_SNDCP, 3);
    write_pdu(&mut sdu);
    sdu.seek(0);
    SapMsg {
        sap: Sap::TlpdSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Sndcp,
        msg: SapMsgInner::LtpdMleUnitdataInd(LtpdMleUnitdataInd {
            sdu,
            endpoint_id: 0,
            link_id,
            received_tetra_address: ms_address(),
            chan_change_resp_req: false,
            chan_change_handle: None,
        }),
    }
}

/// SN-DATA TRANSMIT REQUEST or SN-RECONNECT: the MS asks for a channel to send packet data on.
fn uplink_channel_request(link_id: u32, sn_type: u64) -> SapMsg {
    uplink(link_id, |sdu| {
        sdu.write_bits(sn_type, 4);
        sdu.write_bits(1, 4); // NSAPI
    })
}

/// SN-UNITDATA carrying a plain-text WAP GET for the status page in an IPv4/UDP N-PDU.
fn uplink_wap_get(link_id: u32) -> SapMsg {
    let npdu = ip::build_ipv4_udp([10, 0, 0, 226], [10, 0, 0, 1], 49152, 9200, 7, 32, b"GET /status.xhtml");
    uplink(link_id, |sdu| {
        sdu.write_bits(SN_UNITDATA, 4);
        sdu.write_bits(1, 4); // NSAPI
        sdu.write_bits(0, 4); // PCOMP
        sdu.write_bits(0, 4); // DCOMP
        for byte in npdu {
            sdu.write_bits(u64::from(byte), 8);
        }
    })
}

/// The MS's LLC acknowledgement of a downlink BL-DATA, as the MAC delivers it to LLC. LLC sends one
/// acknowledged message per MS at a time, so nothing further goes out until this arrives.
fn uplink_bl_ack(nr: u8, link_id: u32) -> SapMsg {
    let mut pdu = BitBuffer::new_autoexpand(8);
    BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
    pdu.seek(0);
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu: Some(pdu),
            main_address: ms_address(),
            scrambling_code: 0,
            link_id,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

/// Every downlink slot the MAC handed to the (sink) LMAC since the last call.
fn downlink_slots(test: &mut ComponentTest) -> Vec<TmvUnitdataReqSlot> {
    test.dump_sinks()
        .into_iter()
        .flat_map(|m| match m.msg {
            SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
            SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
            _ => vec![],
        })
        .collect()
}

/// The MAC-RESOURCE addressed to `addr` in this block, if the block starts with one.
fn mac_resource_for(block: &Option<TmvUnitdataReq>, addr: TetraAddress) -> Option<MacResource> {
    let mut mac_block = block.as_ref()?.mac_block.clone();
    // Broadcast and MAC-FRAG blocks share the slot; only type 0 is a MAC-RESOURCE.
    let mut peek = mac_block.clone();
    if peek.read_field(2, "mac_pdu_type").map(|t| t != 0).unwrap_or(true) {
        return None;
    }
    let pdu = MacResource::from_bitbuf(&mut mac_block).ok()?;
    // The air format carries no address subtype: compare the SSI value only.
    (pdu.addr.map(|a| a.ssi) == Some(addr.ssi)).then_some(pdu)
}

/// The timeslots on which a MAC-RESOURCE for the test MS went out, with and without a channel
/// allocation.
fn ms_resource_timeslots(slots: &[TmvUnitdataReqSlot]) -> (Vec<u8>, Vec<u8>) {
    let (mut with_alloc, mut without_alloc) = (vec![], vec![]);
    for slot in slots {
        for block in [&slot.blk1, &slot.blk2] {
            if let Some(pdu) = mac_resource_for(block, ms_address()) {
                if pdu.chan_alloc_element.is_some() {
                    with_alloc.push(slot.ts.t);
                } else {
                    without_alloc.push(slot.ts.t);
                }
            }
        }
    }
    (with_alloc, without_alloc)
}

fn aach(slot: &TmvUnitdataReqSlot) -> AccessAssign {
    let mut bbk = slot.bbk.as_ref().expect("every slot carries an AACH").mac_block.clone();
    AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse")
}

/// Both ways an MS asks for a packet data channel get one: a transmit request, and a reconnect
/// after it changed cell with data outstanding (the handover case).
#[test]
fn channel_requests_are_answered_with_the_packet_data_channel() {
    debug::setup_logging_verbose();

    for sn_type in [SN_DATA_TRANSMIT_REQUEST, SN_RECONNECT] {
        let mut test = wap_stack(vec![TetraEntity::Sndcp], vec![TetraEntity::Llc]);
        test.submit_message(uplink_channel_request(1, sn_type));
        test.run_stack(Some(1));

        let reqs: Vec<_> = test
            .dump_sinks()
            .into_iter()
            .filter_map(|m| match m.msg {
                SapMsgInner::TlaTlDataReqBl(req) => Some(req),
                _ => None,
            })
            .collect();
        assert_eq!(reqs.len(), 1, "SN type {sn_type}: one SN-DATA TRANSMIT RESPONSE expected");
        let alloc = reqs[0].chan_alloc.as_ref().expect("the response assigns a channel");
        assert_eq!(alloc.alloc_type, ChanAllocType::Replace);
        assert!(alloc.usage.is_none(), "a control channel carries no traffic usage marker");
        let mut expected = [false; 4];
        expected[PDCH_TS as usize - 1] = true;
        assert_eq!(alloc.timeslots, expected);
    }
}

/// The whole packet data path down to the air interface. Once the MS has been moved to the packet
/// data channel it no longer listens to the MCCH, so the channel must be advertised as assigned
/// control and the WAP response must go out there.
#[test]
fn wap_response_reaches_the_ms_on_its_packet_data_channel() {
    debug::setup_logging_verbose();
    let mut test = wap_stack(vec![TetraEntity::Sndcp, TetraEntity::Llc, TetraEntity::Umac], vec![TetraEntity::Lmac]);

    // Before any packet data the slot is idle.
    test.run_stack(Some(8));
    let idle = downlink_slots(&mut test);
    assert!(
        idle.iter().filter(|s| s.ts.t == PDCH_TS && s.ts.f != 18).all(|s| aach(s).dl_usage == AccessAssignDlUsage::Unallocated),
        "the PDCH slot starts out unallocated"
    );

    // The MS asks for a channel on the MCCH: the allocation goes out there, where the MS still is.
    test.submit_message(uplink_channel_request(1, SN_DATA_TRANSMIT_REQUEST));
    test.run_stack(Some(16));
    let slots = downlink_slots(&mut test);
    let (with_alloc, _) = ms_resource_timeslots(&slots);
    assert!(!with_alloc.is_empty(), "the channel allocation was transmitted");
    assert!(with_alloc.iter().all(|&t| t == 1), "the allocation goes out on the MCCH, got ts {with_alloc:?}");

    // From then on the slot is an assigned control channel the MS can random-access on.
    test.run_stack(Some(8));
    let slots = downlink_slots(&mut test);
    let pdch_slots: Vec<_> = slots.iter().filter(|s| s.ts.t == PDCH_TS && s.ts.f != 18).collect();
    assert!(!pdch_slots.is_empty());
    for slot in pdch_slots {
        let aach = aach(slot);
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::AssignedControl, "at {}", slot.ts);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly, "at {}", slot.ts);
    }

    // The MS acknowledges the allocation from its new channel.
    test.submit_message(uplink_bl_ack(0, u32::from(PDCH_TS)));
    test.run_stack(Some(4));
    downlink_slots(&mut test);

    // The WAP request arrives with no link (as an unacknowledged uplink does). The response must
    // still follow the MS to the packet data channel, never the MCCH.
    test.submit_message(uplink_wap_get(0));
    test.run_stack(Some(24));
    let slots = downlink_slots(&mut test);
    let (_, without_alloc) = ms_resource_timeslots(&slots);
    assert!(!without_alloc.is_empty(), "the WAP response was transmitted");
    assert!(
        without_alloc.iter().all(|&t| t == PDCH_TS),
        "the WAP response goes out on the packet data channel, got ts {without_alloc:?}"
    );
}
