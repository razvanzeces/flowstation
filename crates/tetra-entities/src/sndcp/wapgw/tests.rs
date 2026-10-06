//! End-to-end tests: a simulated WTP initiator (the terminal) against the gateway.

use std::collections::{BTreeMap, VecDeque};
use std::net::UdpSocket;

use super::fetcher::UnavailableFetcher;
use super::wtp::{GTR, TTR, initiator};
use super::*;
use crate::sndcp::wap_ip::write_uintvar;

const ISSI: u32 = 2_260_618;
const MS_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);
const MS_PORT: u16 = 2049;

fn test_snapshot() -> WapStatusSnapshot {
    WapStatusSnapshot {
        title: "FlowStation".to_string(),
        stack_version: "v0.4.0".to_string(),
        service_state: "OK".to_string(),
        registered_ms: 2,
        active_calls: 0,
        active_group_calls: 0,
        active_private_calls: 0,
        queued_sds: 0,
        uptime_secs: 90,
        last_activity: None,
        health_summary: Some("OK".to_string()),
        health_lines: vec!["service ok".to_string()],
        radio_lines: Vec::new(),
        call_lines: Vec::new(),
    }
}

fn cfg() -> CfgWap {
    let mut cfg = CfgWap {
        enabled: true,
        ..Default::default()
    };
    cfg.browse.enabled = true;
    cfg.browse.allowed_issis = vec![ISSI];
    cfg
}

/// Answers every request with a page of `size` bytes (capped to the request's budget), or never
/// when `hold` is set. Records the budgets it was given.
struct PageFetcher {
    size: usize,
    hold: bool,
    ready: VecDeque<FetchReply>,
    budgets: std::sync::Arc<std::sync::Mutex<Vec<usize>>>,
}

impl PageFetcher {
    fn boxed(size: usize, hold: bool) -> (Box<dyn Fetcher>, std::sync::Arc<std::sync::Mutex<Vec<usize>>>) {
        let budgets = std::sync::Arc::default();
        let fetcher = PageFetcher {
            size,
            hold,
            ready: VecDeque::new(),
            budgets: std::sync::Arc::clone(&budgets),
        };
        (Box::new(fetcher), budgets)
    }
}

impl Fetcher for PageFetcher {
    fn submit(&mut self, req: FetchRequest) -> Result<(), FetchRequest> {
        self.budgets.lock().unwrap().push(req.budget);
        if !self.hold {
            let body: Vec<u8> = b"0123456789".iter().copied().cycle().take(self.size.min(req.budget)).collect();
            self.ready.push_back(FetchReply {
                id: req.id,
                page: Page {
                    status: status::OK,
                    kind: ContentKind::Xhtml,
                    body,
                },
                domain: None,
            });
        }
        Ok(())
    }

    fn try_recv(&mut self) -> Option<FetchReply> {
        self.ready.pop_front()
    }
}

fn gateway_with(cfg: CfgWap, fetcher: Box<dyn Fetcher>) -> WapGateway {
    WapGateway::new(cfg, fetcher, Box::new(test_snapshot))
}

fn gateway() -> WapGateway {
    gateway_with(cfg(), Box::new(UnavailableFetcher::default()))
}

fn peer() -> WapPeer {
    WapPeer {
        issi: ISSI,
        ip: MS_IP,
        via: WapVia::Air,
    }
}

/// Deliver one datagram from the terminal and collect what the gateway sends back.
fn send(gw: &mut WapGateway, payload: Vec<u8>, now: Instant) -> Vec<Vec<u8>> {
    gw.on_udp(
        UdpIn {
            peer: peer(),
            src_port: MS_PORT,
            dst_port: WAP_PORT_CONNECTION,
            payload,
        },
        now,
    );
    collect(gw.poll(now))
}

fn collect(out: Vec<UdpOut>) -> Vec<Vec<u8>> {
    for o in &out {
        assert_eq!((o.peer, o.src_port, o.dst_port), (peer(), WAP_PORT_CONNECTION, MS_PORT));
    }
    out.into_iter().map(|o| o.payload).collect()
}

fn get_pdu(uri: &str) -> Vec<u8> {
    let mut wsp = vec![0x40];
    write_uintvar(uri.len(), &mut wsp);
    wsp.extend_from_slice(uri.as_bytes());
    wsp
}

fn get(tid: u16, uri: &str) -> Vec<u8> {
    initiator::invoke(tid, 2, TTR, false, false, &get_pdu(uri))
}

/// WSP Connect asking for Client-SDU and Server-SDU of 327680 octets, like the MXP600.
fn connect(tid: u16) -> Vec<u8> {
    let caps = [0x04, 0x80, 0x94, 0x80, 0x00, 0x04, 0x81, 0x94, 0x80, 0x00];
    let mut wsp = vec![0x01, 0x10, caps.len() as u8, 0x00];
    wsp.extend_from_slice(&caps);
    initiator::invoke(tid, 2, TTR, true, false, &wsp)
}

/// Fields of a Result / Segmented Result packet: (type, psn, flags, rid, tid, data).
fn result(pkt: &[u8]) -> (u8, u8, u8, bool, u16, &[u8]) {
    let ty = (pkt[0] >> 3) & 0x0f;
    let tid = u16::from_be_bytes([pkt[1], pkt[2]]);
    match ty {
        2 => (ty, 0, pkt[0] & (GTR | TTR), pkt[0] & 1 != 0, tid, &pkt[3..]),
        6 => (ty, pkt[3], pkt[0] & (GTR | TTR), pkt[0] & 1 != 0, tid, &pkt[4..]),
        _ => panic!("not a result packet: {pkt:02x?}"),
    }
}

/// Receive a whole segmented result like a terminal: Ack every group end, return the message.
fn receive_all(gw: &mut WapGateway, tid: u16, mut pkts: Vec<Vec<u8>>, now: Instant) -> Vec<u8> {
    let mut parts = BTreeMap::new();
    loop {
        let mut end = None;
        for p in &pkts {
            let (_, psn, flags, _, rtid, data) = result(p);
            assert_eq!(rtid, tid | 0x8000);
            parts.insert(psn, data.to_vec());
            if flags != 0 {
                end = Some((psn, flags));
            }
        }
        let (psn, flags) = end.expect("a group ends with GTR or TTR");
        let next = send(gw, initiator::ack(tid, Some(psn)), now);
        if flags & TTR != 0 {
            assert!(next.is_empty(), "nothing after the final Ack");
            return parts.into_values().flatten().collect();
        }
        pkts = next;
    }
}

#[test]
fn connect_reply_matches_spec_vector() {
    // Docs/wap-port-spec.md 7.4, with both SDU limits at 545 octets.
    let mut cfg = cfg();
    cfg.max_message_bytes = 545;
    cfg.max_request_bytes = 545;
    let mut gw = gateway_with(cfg, Box::new(UnavailableFetcher::default()));
    let out = send(&mut gw, connect(0x13cc), Instant::now());
    assert_eq!(
        out,
        vec![vec![
            0x12, 0x93, 0xcc, 0x02, 0x01, 0x08, 0x00, 0x03, 0x80, 0x84, 0x21, 0x03, 0x81, 0x84, 0x21
        ]]
    );
    assert_eq!(gw.sessions.get(&(MS_IP, MS_PORT)).map(|s| s.client_sdu), Some(545));
}

#[test]
fn resume_answers_empty_ok() {
    let mut gw = gateway();
    let out = send(
        &mut gw,
        initiator::invoke(0x13f5, 2, TTR, false, false, &[0x09, 0x01, 0x00]),
        Instant::now(),
    );
    assert_eq!(out, vec![vec![0x12, 0x93, 0xf5, 0x04, 0x20, 0x00]]);
}

#[test]
fn get_small_single_packet() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(0x1234, "/status.wml"), t0);
    assert_eq!(out.len(), 1);
    assert_eq!(&out[0][..7], &[0x12, 0x92, 0x34, 0x04, 0x20, 0x06, 0x03]);
    assert_eq!(&out[0][7..10], &[0x88, 0x81, 0xea], "WML with UTF-8 charset");
    assert_eq!(&out[0][10..12], &[0x88, 0x80], "Cache-Control: no-cache");
    assert!(String::from_utf8_lossy(&out[0][12..]).starts_with("<wml>"));
    assert!(send(&mut gw, initiator::ack(0x1234, None), t0).is_empty());
    // A late copy of the Invoke is ignored while the transaction is remembered...
    assert!(send(&mut gw, initiator::invoke(0x1234, 2, TTR, true, false, &get_pdu("/")), t0).is_empty());
    // ... and forgotten after it.
    let later = t0 + DONE_HOLD + Duration::from_secs(1);
    assert!(gw.poll(later).is_empty());
    assert!(gw.txs.is_empty());
}

#[test]
fn home_page_over_two_packets() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let first = send(&mut gw, get(7, "http://10.0.0.1/"), t0);
    let message = receive_all(&mut gw, 7, first, t0);
    assert_eq!(&message[..9], &[0x04, 0x20, 0x06, 0x03, 0xc5, 0x81, 0xea, 0x88, 0x80]);
    let body = String::from_utf8(message[9..].to_vec()).unwrap();
    assert!(body.contains("action=\"/go\""), "browsing ISSI gets the forms: {body}");
    assert!(body.len() <= home::HOME_MAX_BYTES);
}

#[test]
fn get_5kb_segments_and_groups() {
    let t0 = Instant::now();
    let (fetcher, _) = PageFetcher::boxed(5000, false);
    let mut gw = gateway_with(cfg(), fetcher);
    // The page is ready at once: the Result acknowledges the Invoke, no hold-on Ack.
    let first = send(&mut gw, get(0x1234, "http://68k.news/"), t0);
    let heads: Vec<_> = first.iter().map(|p| (result(p).0, result(p).1, result(p).2)).collect();
    assert_eq!(heads, vec![(2, 0, 0), (6, 1, 0), (6, 2, GTR)], "first group of 3");
    assert!(first.iter().all(|p| p.len() <= 576 - 28));
    let message = receive_all(&mut gw, 0x1234, first, t0);
    // Reply header, Content-Type and Cache-Control, then the page.
    assert_eq!(message.len(), 9 + 5000);
    assert!(message[9..].starts_with(b"0123456789"));
    assert!(matches!(gw.txs.values().next().map(|t| &t.state), Some(TxState::Done { .. })));
}

#[test]
fn nack_retransmits_only_missing_with_rid() {
    let t0 = Instant::now();
    let (fetcher, _) = PageFetcher::boxed(3000, false);
    let mut gw = gateway_with(cfg(), fetcher);
    let group = send(&mut gw, get(9, "http://68k.news/"), t0);
    assert_eq!(group.len(), 3);
    // Packet 1 was lost: the terminal Nacks it after the group end.
    let again = send(&mut gw, initiator::nack(9, &[1]), t0);
    assert_eq!(again.len(), 1);
    let (_, psn, flags, rid, _, data) = result(&again[0]);
    assert_eq!((psn, flags, rid), (1, 0, true));
    assert_eq!(data, result(&group[1]).5);
    // The group is now complete: the rest follows.
    let next = send(&mut gw, initiator::ack(9, Some(2)), t0);
    assert_eq!(result(&next[0]).1, 3);
    let rest = receive_all(&mut gw, 9, next, t0);
    assert!(!rest.is_empty());
}

#[test]
fn timer_retransmits_gtr_then_aborts() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(0x1234, "/status.wml"), t0);
    assert_eq!(out.len(), 1);
    let mut t = t0;
    for _ in 0..cfg().wtp.max_retries {
        t += Duration::from_secs(10);
        let again = collect(gw.poll(t));
        assert_eq!(again.len(), 1);
        let (_, psn, flags, rid, _, _) = result(&again[0]);
        assert_eq!((psn, flags, rid), (0, TTR, true));
    }
    t += Duration::from_secs(10);
    assert_eq!(collect(gw.poll(t)), vec![vec![0x20, 0x92, 0x34, 0x08]], "Abort NORESPONSE");
    assert!(gw.txs.is_empty());
}

#[test]
fn duplicate_invoke_during_fetch_sends_hold_on_only() {
    let t0 = Instant::now();
    let (fetcher, _) = PageFetcher::boxed(100, true);
    let mut gw = gateway_with(cfg(), fetcher);
    assert!(send(&mut gw, get(3, "/go?u=wiby.me"), t0).is_empty());
    let dup = initiator::invoke(3, 2, TTR, true, false, &get_pdu("/go?u=wiby.me"));
    assert_eq!(send(&mut gw, dup, t0), vec![vec![0x19, 0x80, 0x03]]);
    assert!(collect(gw.poll(t0 + Duration::from_secs(3))).is_empty(), "hold-on already sent");
    // A fetch that never ends is answered with 504.
    let out = collect(gw.poll(t0 + FETCH_GUARD + Duration::from_secs(1)));
    assert_eq!(out.len(), 1);
    assert_eq!(&result(&out[0]).5[..2], &[0x04, status::GATEWAY_TIMEOUT]);
}

#[test]
fn abort_notimplementedsar_falls_back() {
    let t0 = Instant::now();
    let (fetcher, budgets) = PageFetcher::boxed(5000, false);
    let mut gw = gateway_with(cfg(), fetcher);
    let out = send(&mut gw, get(1, "http://68k.news/"), t0);
    assert_eq!(out.len(), 3, "first group");
    assert!(send(&mut gw, initiator::abort(1, 0, wtp::ABORT_REASON_NOTIMPLEMENTEDSAR), t0).is_empty());
    let out = send(&mut gw, get(2, "http://68k.news/"), t0);
    assert_eq!(out.len(), 1, "one datagram");
    let (_, psn, flags, _, _, _) = result(&out[0]);
    assert_eq!((psn, flags), (0, TTR));
    assert!(out[0].len() <= 576 - 28);
    let budgets = budgets.lock().unwrap();
    assert_eq!(budgets[1], 576 - 28 - 3 - wsp::REPLY_OVERHEAD_MAX, "one Result packet");
    assert!(budgets[0] > budgets[1]);
}

#[test]
fn sar_off_keeps_home_in_one_datagram() {
    let mut cfg = cfg();
    cfg.wtp.sar = WapSarMode::Off;
    let mut gw = gateway_with(cfg, Box::new(UnavailableFetcher::default()));
    let out = send(&mut gw, get(4, "/"), Instant::now());
    assert_eq!(out.len(), 1);
    assert_eq!(result(&out[0]).2, TTR);
    assert!(out[0].len() <= 576 - 28);
}

#[test]
fn class0_disconnect_no_response() {
    let t0 = Instant::now();
    let mut gw = gateway();
    send(&mut gw, connect(1), t0);
    assert_eq!(gw.sessions.len(), 1);
    let out = send(&mut gw, initiator::invoke(2, 0, TTR, false, false, &[0x05, 0x01]), t0);
    assert!(out.is_empty());
    assert!(gw.sessions.is_empty());
}

#[test]
fn tidnew_resets_cache() {
    let t0 = Instant::now();
    let mut gw = gateway();
    send(&mut gw, get(5, "/status.wml"), t0);
    send(&mut gw, initiator::ack(5, None), t0);
    let dup = initiator::invoke(5, 2, TTR, true, false, &get_pdu("/status.wml"));
    assert!(send(&mut gw, dup, t0).is_empty(), "cached TID: duplicate");
    let renewed = initiator::invoke(5, 2, TTR, false, true, &get_pdu("/status.wml"));
    assert_eq!(send(&mut gw, renewed, t0).len(), 1, "TIDnew: a new transaction");
}

/// A retransmission is the same PDU with RID set, TIDnew included: it must not restart the
/// transaction it repeats.
#[test]
fn retransmitted_invoke_with_tidnew_is_a_duplicate() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let connect_new = |tid, rid| {
        let mut pdu = connect(tid);
        pdu[0] = (pdu[0] & !1) | u8::from(rid);
        pdu[3] |= 0x20; // TIDnew: first transaction after power-up
        pdu
    };
    let first = send(&mut gw, connect_new(0x100, false), t0);
    assert_eq!(&first[0][..5], &[0x12, 0x81, 0x00, 0x02, 0x01], "ConnectReply of session 1");
    // No Ack yet: the copy gets the same ConnectReply again, not a second session.
    let again = send(&mut gw, connect_new(0x100, true), t0);
    assert_eq!(again, vec![[&[0x13][..], &first[0][1..]].concat()]);
    assert_eq!((gw.sessions.len(), gw.next_session_id), (1, 2));
    // Acknowledged, then a late copy: nothing.
    assert!(send(&mut gw, initiator::ack(0x100, None), t0).is_empty());
    assert!(send(&mut gw, connect_new(0x100, true), t0).is_empty());
    assert_eq!(gw.next_session_id, 2);
    // The copy of an Invoke whose original was lost does restart the TIDs.
    let other = send(&mut gw, connect_new(0x300, true), t0);
    assert_eq!(&other[0][3..5], &[0x02, 0x02], "session 2");
    assert!(!gw.txs.contains_key(&TxKey {
        ip: MS_IP,
        port: MS_PORT,
        tid: 0x100
    }));
}

#[test]
fn repeated_invoke_while_sending_resends_at_once() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(0x40, "/status.wml"), t0);
    assert_eq!(out.len(), 1);
    let dup = initiator::invoke(0x40, 2, TTR, true, false, &get_pdu("/status.wml"));
    // The Result was lost: every copy of the Invoke gets it again, until the retries run out.
    for _ in 0..cfg().wtp.max_retries {
        let again = send(&mut gw, dup.clone(), t0 + Duration::from_millis(500));
        assert_eq!(again, vec![[&[out[0][0] | 1][..], &out[0][1..]].concat()], "Result with RID");
    }
    assert!(send(&mut gw, dup, t0 + Duration::from_secs(1)).is_empty());
    // The retries were used up: the timer gives up.
    assert_eq!(collect(gw.poll(t0 + Duration::from_secs(20))), vec![vec![0x20, 0x80, 0x40, 0x08]]);
}

#[test]
fn retransmit_rid_off_sends_copies_with_rid_clear() {
    let t0 = Instant::now();
    let mut cfg = cfg();
    cfg.wtp.retransmit_rid = false;
    let mut gw = gateway_with(cfg, Box::new(UnavailableFetcher::default()));
    let out = send(&mut gw, get(0x41, "/status.wml"), t0);
    let dup = initiator::invoke(0x41, 2, TTR, true, false, &get_pdu("/status.wml"));
    assert_eq!(send(&mut gw, dup, t0), out, "the same Result, RID clear (as Nexus-BS)");
    assert_eq!(collect(gw.poll(t0 + Duration::from_secs(20))), out, "timer copy, RID clear");
}

/// GTR and TTR both set in an Invoke: the terminal does not do SAR.
#[test]
fn invoke_without_sar_gets_one_datagram() {
    for sar in [WapSarMode::Auto, WapSarMode::On] {
        let t0 = Instant::now();
        let mut cfg = cfg();
        cfg.wtp.sar = sar;
        let mut gw = gateway_with(cfg, Box::new(UnavailableFetcher::default()));
        let no_sar = initiator::invoke(0x50, 2, GTR | TTR, false, false, &get_pdu("/"));
        let out = send(&mut gw, no_sar, t0);
        assert_eq!(out.len(), 1, "{sar:?}");
        assert_eq!(result(&out[0]).2, TTR);
        assert!(out[0].len() <= 576 - 28);
        // Auto remembers the radio; On only follows what each Invoke says.
        let home = send(&mut gw, get(0x51, "/"), t0);
        assert_eq!(home.len(), if sar == WapSarMode::Auto { 1 } else { 2 }, "{sar:?}");
    }
}

#[test]
fn silent_segmented_result_falls_back_to_single_datagrams() {
    let t0 = Instant::now();
    let mut gw = gateway();
    assert_eq!(send(&mut gw, get(0x60, "/"), t0).len(), 2, "home page in two packets");
    let mut t = t0;
    let mut last = Vec::new();
    for _ in 0..=cfg().wtp.max_retries {
        t += Duration::from_secs(20);
        last = collect(gw.poll(t));
    }
    assert_eq!(last, vec![vec![0x20, 0x80, 0x60, 0x08]], "Abort NORESPONSE");
    assert_eq!(send(&mut gw, get(0x61, "/"), t).len(), 1, "no more segmented results");
    // An acknowledged segmented result keeps SAR on.
    let mut gw = gateway();
    let first = send(&mut gw, get(0x62, "/"), t0);
    receive_all(&mut gw, 0x62, first, t0);
    assert_eq!(send(&mut gw, get(0x63, "/"), t0).len(), 2);
    let mut t = t0;
    for _ in 0..=cfg().wtp.max_retries {
        t += Duration::from_secs(20);
        gw.poll(t);
    }
    assert!(gw.no_sar.is_empty(), "this radio answered segmented results before");
}

#[test]
fn measured_round_trip_stretches_the_timer() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(0x70, "/status.wml"), t0);
    let air = Duration::from_millis(out[0].len() as u64 * 1000 / 450);
    // The Ack comes back just before the fixed 4 s timer: the next Result waits longer.
    assert!(send(&mut gw, initiator::ack(0x70, None), t0 + Duration::from_millis(3900)).is_empty());
    let t1 = t0 + Duration::from_secs(10);
    let out = send(&mut gw, get(0x71, "/status.wml"), t1);
    assert_eq!(out.len(), 1);
    assert!(
        collect(gw.poll(t1 + Duration::from_secs(6) + air)).is_empty(),
        "the static timer fired here"
    );
    let again = collect(gw.poll(t1 + MAX_ADAPTIVE_TIMER + air * 2));
    assert_eq!(again.len(), 1);
    assert!(result(&again[0]).3, "retransmission");
}

#[test]
fn paused_timers_wait_for_the_bearer() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(0x72, "/status.wml"), t0);
    assert_eq!(out.len(), 1);
    gw.pause_timers(ISSI);
    // A long group call: no retransmission and no Abort while the bearer holds the data.
    assert!(collect(gw.poll(t0 + Duration::from_secs(60))).is_empty());
    let t1 = t0 + Duration::from_secs(61);
    gw.resume_timers(ISSI, t1);
    assert!(collect(gw.poll(t1 + Duration::from_secs(3))).is_empty(), "a fresh timer");
    assert!(send(&mut gw, initiator::ack(0x72, None), t1 + Duration::from_secs(3)).is_empty());
    assert!(matches!(gw.txs.values().next().map(|t| &t.state), Some(TxState::Done { .. })));
}

#[test]
fn connect_without_client_sdu_uses_wsp_default() {
    let t0 = Instant::now();
    let (fetcher, budgets) = PageFetcher::boxed(5000, false);
    let mut gw = gateway_with(cfg(), fetcher);
    let reply = send(
        &mut gw,
        initiator::invoke(0x80, 2, TTR, false, false, &[0x01, 0x10, 0x00, 0x00]),
        t0,
    );
    assert_eq!(
        reply,
        vec![vec![0x12, 0x80, 0x80, 0x02, 0x01, 0x00, 0x00]],
        "no capabilities echoed"
    );
    assert_eq!(gw.sessions.get(&(MS_IP, MS_PORT)).map(|s| s.client_sdu), Some(wsp::DEFAULT_SDU));
    let first = send(&mut gw, get(0x81, "http://68k.news/"), t0);
    let message = receive_all(&mut gw, 0x81, first, t0);
    assert!(message.len() <= wsp::DEFAULT_SDU, "{} octets", message.len());
    assert_eq!(budgets.lock().unwrap()[0], wsp::DEFAULT_SDU - wsp::REPLY_OVERHEAD_MAX);
    // A Resume of a session this gateway does not know gets the same default.
    let mut gw = gateway();
    send(&mut gw, initiator::invoke(0x82, 2, TTR, false, false, &[0x09, 0x01, 0x00]), t0);
    assert_eq!(gw.sessions.get(&(MS_IP, MS_PORT)).map(|s| s.client_sdu), Some(wsp::DEFAULT_SDU));
}

#[test]
fn small_mtu_is_flagged() {
    assert!(fragments_requests(296));
    assert!(!fragments_requests(576));
}

#[test]
fn hold_on_only_when_the_download_is_slow() {
    let t0 = Instant::now();
    let (fetcher, _) = PageFetcher::boxed(100, true);
    let mut gw = gateway_with(cfg(), fetcher);
    assert!(send(&mut gw, get(0x90, "/go?u=wiby.me"), t0).is_empty());
    assert!(collect(gw.poll(t0 + Duration::from_millis(1900))).is_empty());
    assert_eq!(
        collect(gw.poll(t0 + Duration::from_millis(2000))),
        vec![vec![0x18, 0x80, 0x90]],
        "hold-on after hold_on_ms"
    );
    assert!(collect(gw.poll(t0 + Duration::from_secs(5))).is_empty(), "only once");
}

#[test]
fn latin1_uri_gets_a_page_not_an_abort() {
    let mut gw = gateway();
    let wsp = [&[0x40, 0x0c][..], b"/go?u=Espa", &[0xf1], b"a"].concat();
    let out = send(&mut gw, initiator::invoke(0x91, 2, TTR, false, false, &wsp), Instant::now());
    assert_eq!(out.len(), 1);
    assert_eq!(&result(&out[0]).5[..2], &[0x04, status::BAD_REQUEST]);
}

/// A WTP Invoke (class 2, U/P) carrying a WSP Get with a Referer, byte for byte as the MXP600
/// sends it (the .75's journal, 06-10).
fn mxp600_get(tid: u16, uri: &str, referer: &str) -> Vec<u8> {
    let [hi, lo] = tid.to_be_bytes();
    let mut datagram = vec![0x0a, hi, lo, 0x12, 0x40];
    write_uintvar(uri.len(), &mut datagram);
    datagram.extend_from_slice(uri.as_bytes());
    datagram.push(0xa4);
    datagram.extend_from_slice(referer.as_bytes());
    datagram.push(0);
    datagram
}

/// 06-10 on the .75: the radio asked for the weather of Madrid (twice) and of LELC, but for
/// "Murcia" and "Alguazas" it sent nothing at all and showed its own copy of "/wx?l=Murcia" and
/// "/wx?l=Alguazas", pages it got before they were sent no-cache. The forms of every weather page
/// now go to a "/wx/N" no earlier page had, and what the radio builds from them reaches the
/// download pool.
#[test]
fn weather_forms_from_the_mxp600_build_new_urls_that_are_fetched() {
    let t0 = Instant::now();
    let (fetcher, budgets) = PageFetcher::boxed(40, false);
    let mut gw = gateway_with(cfg(), fetcher);
    // The radio's own request for the form page (TID 0x0d6b at 20:18:09).
    let mut actions = Vec::new();
    for tid in [0x0d6b_u16, 0x0d6e] {
        let first = send(&mut gw, mxp600_get(tid, "http://10.0.0.1/wx", "http://10.0.0.1/"), t0);
        let message = receive_all(&mut gw, tid, first, t0);
        assert_eq!(&message[..2], &[0x04, status::OK]);
        let body = String::from_utf8(message[9..].to_vec()).unwrap();
        let action = body
            .split("<form action=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("a weather form: {body}"))
            .to_string();
        assert!(action.starts_with("/wx/"), "{action}");
        actions.push(action);
    }
    assert_ne!(actions[0], actions[1], "a new URL on every page");
    assert!(budgets.lock().unwrap().is_empty(), "the forms are local");
    // What the radio builds from those forms goes out, the old URL too.
    for (tid, uri) in [
        (0x0d70_u16, format!("http://10.0.0.1{}?l=Murcia", actions[0])),
        (0x0d71, format!("http://10.0.0.1{}?l=Alguazas", actions[1])),
        (0x0d72, format!("http://10.0.0.1{}?i=Lelc", actions[1])),
        (0x0d73, "http://10.0.0.1/wx?l=Madrid".to_string()),
    ] {
        let sent = budgets.lock().unwrap().len();
        let first = send(&mut gw, mxp600_get(tid, &uri, "http://10.0.0.1/wx"), t0);
        let first = if first.is_empty() { collect(gw.poll(t0)) } else { first };
        let message = receive_all(&mut gw, tid, first, t0);
        assert_eq!(budgets.lock().unwrap().len(), sent + 1, "{uri}: one download");
        assert_eq!(&message[..2], &[0x04, status::OK], "{uri}");
        assert!(message.ends_with(b"0123456789"), "{uri}: the downloaded page");
    }
}

#[test]
fn other_wtp_version_is_aborted() {
    let mut gw = gateway();
    let mut pdu = get(0x92, "/");
    pdu[3] |= 0x40; // version 1
    assert_eq!(
        send(&mut gw, pdu, Instant::now()),
        vec![vec![0x20, 0x80, 0x92, 0x06]],
        "WTPVERSIONONE"
    );
    assert!(gw.txs.is_empty());
}

#[test]
fn segmented_invoke_flood_is_bounded() {
    let t0 = Instant::now();
    let mut gw = gateway();
    // Segments that never close a group, each with a new TID: a few per radio at most.
    for tid in 0..MAX_REASSEMBLING_PER_PEER as u16 {
        assert!(send(&mut gw, initiator::segmented_invoke(tid, 1, 0, false, b"x"), t0).is_empty());
    }
    let refused = send(&mut gw, initiator::segmented_invoke(0x99, 1, 0, false, b"x"), t0);
    assert_eq!(refused, vec![vec![0x20, 0x80, 0x99, 0x07]], "CAPTEMPEXCEEDED");
    assert_eq!(gw.txs.len(), MAX_REASSEMBLING_PER_PEER);
    // Many radios: the gateway-wide bound holds.
    for issi in 0..(MAX_TRANSACTIONS as u32) {
        let peer = WapPeer {
            issi: 1000 + issi,
            ip: Ipv4Addr::from(0x0a01_0000 + issi),
            via: WapVia::Air,
        };
        for tid in 0..2 {
            gw.on_udp(
                UdpIn {
                    peer,
                    src_port: MS_PORT,
                    dst_port: WAP_PORT_CONNECTION,
                    payload: initiator::segmented_invoke(tid, 1, 0, false, b"x"),
                },
                t0,
            );
        }
    }
    assert_eq!(gw.txs.len(), MAX_TRANSACTIONS);
}

#[test]
fn unauthorized_issi_gets_403_without_fetch() {
    let (fetcher, budgets) = PageFetcher::boxed(100, false);
    let mut cfg = cfg();
    cfg.browse.allowed_issis = vec![1];
    let mut gw = gateway_with(cfg, fetcher);
    let out = send(&mut gw, get(6, "http://68k.news/"), Instant::now());
    assert_eq!(out.len(), 1);
    assert_eq!(&result(&out[0]).5[..2], &[0x04, status::FORBIDDEN]);
    assert!(budgets.lock().unwrap().is_empty());
}

#[test]
fn unavailable_fetcher_answers_503() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, get(8, "/s?q=tetra"), t0);
    let message = receive_all(&mut gw, 8, out, t0);
    assert_eq!(&message[..2], &[0x04, status::SERVICE_UNAVAILABLE]);
    assert!(String::from_utf8_lossy(&message).contains("navegación no está disponible"));
}

#[test]
fn post_gets_405() {
    let mut gw = gateway();
    let post = initiator::invoke(10, 2, TTR, false, false, &[0x60, 0x01, b'/', 0x01, 0x83]);
    let out = send(&mut gw, post, Instant::now());
    assert_eq!(&result(&out[0]).5[..2], &[0x04, status::METHOD_NOT_ALLOWED]);
}

#[test]
fn segmented_invoke_with_lost_segment() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let uri = format!("/status.wml?pad={}", "x".repeat(700));
    let wsp = get_pdu(&uri);
    let (a, rest) = wsp.split_at(300);
    let (b, c) = rest.split_at(300);
    assert!(send(&mut gw, initiator::invoke(11, 2, 0, false, false, a), t0).is_empty());
    // Packet 1 is lost; packet 2 ends the message.
    let out = send(&mut gw, initiator::segmented_invoke(11, 2, TTR, false, c), t0);
    assert_eq!(out, vec![vec![0x38, 0x80, 0x0b, 0x01, 0x01]], "Nack of PSN 1");
    let out = send(&mut gw, initiator::segmented_invoke(11, 1, 0, true, b), t0);
    assert_eq!(out.len(), 1, "the retransmission completes the message: result");
    assert_eq!(&result(&out[0]).5[..2], &[0x04, status::OK]);
}

#[test]
fn segmented_invoke_group_ack_and_size_limit() {
    let t0 = Instant::now();
    let mut gw = gateway();
    let out = send(&mut gw, initiator::invoke(12, 2, GTR, false, false, &[0x40, 0x05, b'/']), t0);
    assert_eq!(out, vec![vec![0x98, 0x80, 0x0c, 0x19, 0x00]], "group Ack with PSN 0");
    let big = vec![b'x'; 600];
    send(&mut gw, initiator::segmented_invoke(12, 1, 0, false, &big), t0);
    let out = send(&mut gw, initiator::segmented_invoke(12, 2, TTR, false, &big), t0);
    assert_eq!(
        out,
        vec![vec![0x20, 0x80, 0x0c, 0x09]],
        "Abort MESSAGETOOLARGE over max_request_bytes"
    );
}

#[test]
fn ports_9200_9201() {
    let t0 = Instant::now();
    let mut gw = gateway();
    for port in [WAP_PORT_CONNECTIONLESS, WAP_PORT_CONNECTION] {
        let npdu = build_ipv4_udp_npdu(MS_IP.octets(), [10, 0, 0, 1], MS_PORT, port, &get(port, "/status.wml"), 1, 64).unwrap();
        gw.on_ipv4(ISSI, &npdu, Some(296), t0).unwrap();
        let out = gw.poll(t0);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].src_port, out[0].dst_port, out[0].peer.via), (port, MS_PORT, WapVia::Air));
        let ip = out[0].to_ipv4(Ipv4Addr::new(10, 0, 0, 1), 2).unwrap();
        let parsed = parse_ipv4_packet(&ip).unwrap();
        assert_eq!((parsed.source, parsed.destination), ([10, 0, 0, 1], MS_IP.octets()));
        let udp = parse_udp_datagram(parsed.payload).unwrap();
        assert_eq!((udp.source_port, udp.destination_port), (port, MS_PORT));
        assert!(ip.len() <= 296, "path limit from the bearer");
    }
    let to_http = build_ipv4_udp_npdu(MS_IP.octets(), [10, 0, 0, 1], MS_PORT, 80, b"x", 1, 64).unwrap();
    assert_eq!(gw.on_ipv4(ISSI, &to_http, None, t0), Err(WapInputError::WrongPort { port: 80 }));
    let elsewhere = build_ipv4_udp_npdu(MS_IP.octets(), [8, 8, 8, 8], MS_PORT, 9201, b"x", 1, 64).unwrap();
    assert!(matches!(
        gw.on_ipv4(ISSI, &elsewhere, None, t0),
        Err(WapInputError::WrongDestination { .. })
    ));
}

#[test]
fn peer_lost_drops_state() {
    let t0 = Instant::now();
    let mut gw = gateway();
    send(&mut gw, connect(1), t0);
    send(&mut gw, get(2, "/status.wml"), t0);
    gw.set_path_limits(ISSI, 296);
    gw.on_peer_lost(ISSI);
    assert!(gw.sessions.is_empty() && gw.txs.is_empty() && gw.path_limits.is_empty());
}

#[test]
fn open_transactions_while_fetching_and_sending() {
    let t0 = Instant::now();
    let (fetcher, _) = PageFetcher::boxed(100, true);
    let mut gw = gateway_with(cfg(), fetcher);
    assert!(!gw.has_open_transactions(ISSI));
    assert!(send(&mut gw, get(3, "/go?u=wiby.me"), t0).is_empty());
    assert!(gw.has_open_transactions(ISSI), "download under way");
    gw.on_peer_lost(ISSI);
    assert!(!gw.has_open_transactions(ISSI));

    let mut gw = gateway();
    assert_eq!(send(&mut gw, get(4, "/status.wml"), t0).len(), 1);
    assert!(gw.has_open_transactions(ISSI), "result not acknowledged yet");
    assert!(send(&mut gw, initiator::ack(4, None), t0).is_empty());
    assert!(!gw.has_open_transactions(ISSI), "done");
}

/// The SNDCP bearer's side of the service: a datagram in through `on_air_ipv4` sets the path
/// limit, and the answer comes out of `tick` for the air.
#[test]
fn air_datagram_answer_comes_out_of_tick() {
    let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[wap]
enabled = true
"#;
    let config = SharedConfig::from_parts(tetra_config::bluestation::from_toml_str(toml).unwrap(), None);
    let mut svc = WapService::start(&config).expect("[wap] enabled");
    assert_eq!(svc.gateway_ipv4(), Ipv4Addr::new(10, 0, 0, 1));
    let t0 = Instant::now();
    let npdu = build_ipv4_udp_npdu(MS_IP.octets(), [10, 0, 0, 1], MS_PORT, WAP_PORT_CONNECTION, &get(9, "/"), 1, 64).unwrap();
    svc.on_air_ipv4(&config, ISSI, &npdu, Some(296), t0).unwrap();
    let out = svc.tick(&config, t0);
    assert!(!out.is_empty());
    for o in &out {
        assert_eq!((o.peer.issi, o.peer.ip, o.peer.via), (ISSI, MS_IP, WapVia::Air));
        assert!(o.to_ipv4(svc.gateway_ipv4(), 1).unwrap().len() <= 296, "path limit from the bearer");
    }
    assert!(svc.has_open_transactions(ISSI));
    svc.pause_timers(ISSI);
    assert!(
        svc.tick(&config, t0 + Duration::from_secs(60)).is_empty(),
        "paused: no retransmission"
    );
    svc.resume_timers(ISSI, t0 + Duration::from_secs(60));
    svc.peer_lost(ISSI);
    assert!(!svc.has_open_transactions(ISSI));
    let to_http = build_ipv4_udp_npdu(MS_IP.octets(), [10, 0, 0, 1], MS_PORT, 80, b"x", 1, 64).unwrap();
    assert_eq!(
        svc.on_air_ipv4(&config, ISSI, &to_http, None, t0),
        Err(WapInputError::WrongPort { port: 80 })
    );
}

/// Debug UDP bearer in-process: a UDP client on 127.0.0.1 against the service the SNDCP entity
/// runs, with Connect, the home page in two groups and a lost packet recovered by a Nack.
#[test]
fn debug_udp_end_to_end() {
    let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[wap]
enabled = true
mtu = 296
debug_udp_listen = "127.0.0.1:0"
debug_issi = 9990

[wap.wtp]
group_size = 2

[wap.browse]
enabled = true
allowed_issis = [9990]
"#;
    let config = SharedConfig::from_parts(tetra_config::bluestation::from_toml_str(toml).unwrap(), None);
    let mut svc = WapService::start(&config).expect("[wap] enabled");
    let addr = svc.debug_addr().expect("debug bearer running");
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    // Non-blocking: on Windows a datagram arriving as a receive timeout expires can be lost.
    client.set_nonblocking(true).unwrap();

    let mut exchange = |payload: &[u8], expect: usize| -> Vec<Vec<u8>> {
        client.send_to(payload, addr).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 2048];
        let until = Instant::now() + Duration::from_secs(3);
        while got.len() < expect && Instant::now() < until {
            svc.tick(&config, Instant::now());
            match client.recv_from(&mut buf) {
                Ok((n, _)) => got.push(buf[..n].to_vec()),
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        got
    };

    let reply = exchange(&connect(0x13cc), 1);
    assert_eq!(&reply[0][..4], &[0x12, 0x93, 0xcc, 0x02]);

    let group = exchange(&get(0x20, "/"), 2);
    assert_eq!(group.len(), 2);
    assert!(group.iter().all(|p| p.len() <= 296 - 28));
    // Packet 0 "lost": Nack it.
    let again = exchange(&initiator::nack(0x20, &[0]), 1);
    assert_eq!((result(&again[0]).1, result(&again[0]).3), (0, true));
    let mut parts: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    for p in group.iter().chain(&again) {
        parts.insert(result(p).1, result(p).5.to_vec());
    }
    let mut last = result(&group[1]).1;
    loop {
        let next = exchange(&initiator::ack(0x20, Some(last)), 2);
        if next.is_empty() {
            break;
        }
        for p in &next {
            parts.insert(result(p).1, result(p).5.to_vec());
            last = result(p).1;
        }
        if result(next.last().unwrap()).2 == TTR {
            exchange(&initiator::ack(0x20, Some(last)), 0);
            break;
        }
    }
    let message: Vec<u8> = parts.into_values().flatten().collect();
    let body = String::from_utf8_lossy(&message);
    assert!(body.contains("FlowStation") && body.contains("action=\"/go\""), "{body}");
    assert!(body.ends_with("</html>"));
}

/// HTTP server on 127.0.0.1 for the end-to-end test: `routes` maps a path to an HTML body.
fn http_server(routes: Vec<(&'static str, String)>) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let (code, body) = match routes.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => ("200 OK", body.as_str()),
                None => ("404 Not Found", "<p>no</p>"),
            };
            let response = format!(
                "HTTP/1.1 {code}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// Browsing end to end: a UDP client on 127.0.0.1 asks the debug bearer for a web page served by
/// a local HTTP server, gets the hold-on Ack and then the first converted page, walks to the next
/// page and follows a shortened link.
#[test]
fn debug_udp_fetches_a_web_page() {
    let mut article = String::from("<html><head><title>Articulo</title><script>track()</script></head><body>");
    article.push_str("<p><a href=\"/dos\">Otra noticia</a></p>");
    for i in 0..40 {
        article.push_str(&format!("<p>Parrafo {i} del articulo de prueba, con algo de texto.</p>"));
    }
    article.push_str("</body></html>");
    let port = http_server(vec![("/articulo", article), ("/dos", "<p>Pagina dos</p>".to_string())]);
    let toml = format!(
        r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[wap]
enabled = true
mtu = 1500
debug_udp_listen = "127.0.0.1:0"
debug_issi = 9990

[wap.wtp]
hold_on_ms = 30000

[wap.browse]
enabled = true
allowed_issis = [9990]
allowed_ports = [{port}]
"#
    );
    let config = SharedConfig::from_parts(tetra_config::bluestation::from_toml_str(&toml).unwrap(), None);
    // The test server is on loopback, which the real policy refuses.
    let policy = NetPolicy::new(&config.config().wap.browse).allowing_loopback();
    let mut svc = WapService::start_with_policy(&config, policy).expect("[wap] enabled");
    let addr = svc.debug_addr().expect("debug bearer running");
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_nonblocking(true).unwrap();

    let mut exchange = |payload: &[u8], expect: usize| -> Vec<Vec<u8>> {
        client.send_to(payload, addr).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 2048];
        let until = Instant::now() + Duration::from_secs(10);
        while got.len() < expect && Instant::now() < until {
            svc.tick(&config, Instant::now());
            match client.recv_from(&mut buf) {
                Ok((n, _)) => got.push(buf[..n].to_vec()),
                Err(_) => std::thread::sleep(Duration::from_millis(1)),
            }
        }
        got
    };
    assert_eq!(&exchange(&connect(1), 1)[0][..4], &[0x12, 0x80, 0x01, 0x02]);

    // The page in one Result (MTU 1500 carries a 1200-octet page; the hold-on Ack is held back
    // for 30 s); returns the XHTML body after acknowledging the Result.
    let mut fetch = |tid: u16, uri: &str| -> String {
        let got = exchange(&get(tid, uri), 1);
        assert_eq!(got.len(), 1, "Result for {uri}");
        let (ty, _, flags, _, _, wsp) = result(&got[0]);
        assert_eq!((ty, flags), (2, TTR));
        assert_eq!(&wsp[..2], &[0x04, 0x20], "WSP Reply 200");
        let body = String::from_utf8(wsp[3 + wsp[2] as usize..].to_vec()).unwrap();
        assert!(exchange(&initiator::ack(tid, None), 0).is_empty());
        body
    };

    let url = format!("http%3A%2F%2F127.0.0.1%3A{port}%2Farticulo");
    let first = fetch(2, &format!("/go?u={url}"));
    assert!(first.contains("<title>Articulo (1/"), "{first}");
    assert!(first.contains("<a href=\"/l/1/0\">Otra noticia</a>") && first.contains("Parrafo 0 del articulo"));
    assert!(first.contains("<a href=\"/p/1/2\">Siguiente</a>") && !first.contains("track()"));
    assert!(first.len() <= 1200, "{} octets", first.len());

    let second = fetch(3, "/p/1/2");
    assert!(
        second.contains("<a href=\"/p/1/1\">Anterior</a>") && second.contains("Parrafo"),
        "{second}"
    );

    let linked = fetch(4, "/l/1/0");
    assert!(linked.contains("Pagina dos"), "{linked}");

    // The dashboard gets the session and the domain (never the URL), at most once a second.
    std::thread::sleep(STATUS_INTERVAL);
    svc.tick(&config, Instant::now());
    let status = config.state_read().wap_status.clone();
    assert!(status.running && status.browse_enabled);
    assert_eq!(status.fetches, 3);
    assert_eq!(status.recent_domains, vec![("127.0.0.1".to_string(), 2)]);
    assert_eq!(status.sessions.len(), 1);
    assert_eq!(
        (
            status.sessions[0].issi,
            status.sessions[0].via.as_str(),
            status.sessions[0].last_domain.as_deref()
        ),
        (9990, "debug-udp", Some("127.0.0.1"))
    );
}

#[test]
fn idle_sessions_expire() {
    let mut gw = gateway();
    let t0 = Instant::now();
    send(&mut gw, connect(1), t0);
    let status = gw.status();
    assert_eq!(status.sessions.len(), 1);
    assert_eq!((status.sessions[0].issi, status.sessions[0].via.as_str()), (ISSI, "air"));
    let seen = status.sessions[0].last_seen_unix;
    gw.expire_sessions(seen + SESSION_IDLE_SECS - 1, SESSION_IDLE_SECS);
    assert_eq!(gw.status().sessions.len(), 1);
    gw.expire_sessions(seen + SESSION_IDLE_SECS, SESSION_IDLE_SECS);
    assert!(gw.status().sessions.is_empty());
}
