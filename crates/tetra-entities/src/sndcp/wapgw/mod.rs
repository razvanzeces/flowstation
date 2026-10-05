//! WAP gateway for packet-data terminals: WTP (with segmentation and reassembly) and
//! connection-mode WSP over UDP, a local router with the station's home and status pages, and an
//! interface to an Internet fetcher.
//!
//! Threads: [`WapGateway`] runs on the TETRA stack thread and never blocks nor does I/O. It takes
//! the clock as a parameter; datagrams go in through [`WapGateway::on_udp`] (or
//! [`WapGateway::on_ipv4`] for the SNDCP bearer) and the answers come out of
//! [`WapGateway::poll`], which also runs the WTP timers and collects finished fetches. The debug
//! UDP bearer and the fetcher have their own threads and talk to it through channels.
//!
//! Bearer contract: the SNDCP bearer hands over each uplink IPv4 N-PDU of a radio with its ISSI
//! and the largest datagram it can carry back (`on_ipv4`), turns each [`UdpOut`] of that ISSI
//! into a downlink N-PDU ([`UdpOut::to_ipv4`]), and reports a radio that deregisters or drops its
//! PDP context (`on_peer_lost`). While it cannot carry data to a radio (voice took the data slot)
//! it calls `pause_timers`, and `resume_timers` when it can again, so the WTP retransmission
//! timers do not run out while its datagrams wait.

pub mod convert;
pub mod debug_udp;
pub mod doc_cache;
pub mod fetch;
pub mod fetcher;
pub mod home;
pub mod netpolicy;
pub mod paginate;
pub mod rid;
pub mod router;
pub mod snapshot;
pub mod wsp;
pub mod wtp;
pub mod wtp_sar;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tetra_config::bluestation::{CfgWap, CfgWapBrowse, CfgWapWtp, SharedConfig, WapRuntimeStatus, WapSarMode, WapSessionStatus};

use self::debug_udp::DebugUdp;
use self::fetch::PoolFetcher;
use self::fetcher::{FetchReply, FetchRequest, FetchTarget, Fetcher, Page, UnavailableFetcher};
use self::netpolicy::NetPolicy;
use self::router::{Route, RouteCtx};
use self::wsp::{ContentKind, WspRequest, status};
use self::wtp::{Invoke, WtpPdu};
use self::wtp_sar::{AckOutcome, InMessage, MAX_PACKETS, OutMessage, Reassembly, Rtt, TimerOutcome};
use crate::sndcp::ip::{IPV4_PROTOCOL_UDP, IpPrimitiveError, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use crate::sndcp::wap_ip::{WtpAbortInfo, wtp_abort_reason_name};
use crate::sndcp::wap_status::WapStatusSnapshot;

/// IANA ports: WSP connectionless (9200) and connection-oriented over WTP (9201). Terminals use
/// WTP on either, so both are served the same way.
pub const WAP_PORT_CONNECTIONLESS: u16 = 9200;
pub const WAP_PORT_CONNECTION: u16 = 9201;

/// IPv4 (20) + UDP (8) headers.
const IPV4_UDP_HEADERS: usize = 28;
/// Segmented Result header: fixed header (3) + PSN (1).
const SEGMENTED_RESULT_HEADER: usize = 4;
/// A finished transaction is remembered this long, so late duplicates of its Invoke are ignored.
const DONE_HOLD: Duration = Duration::from_secs(60);
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(30);
/// A fetch that never finishes is answered with 504 after this long.
const FETCH_GUARD: Duration = Duration::from_secs(45);
/// Bound on open transactions (a flood must not grow memory).
const MAX_TRANSACTIONS: usize = 256;
/// Bound on half-received segmented Invokes of one radio (each waits REASSEMBLY_TIMEOUT).
const MAX_REASSEMBLING_PER_PEER: usize = 4;
/// Ceiling of the measured part of the retransmission timer.
const MAX_ADAPTIVE_TIMER: Duration = Duration::from_secs(30);
/// IPv4 size of the MXP600's captured WSP Connect (394 octets of UDP payload): a smaller MTU
/// needs the terminal to segment it with WTP SAR, as IPv4 fragments are not reassembled.
const CONNECT_IPV4_BYTES: usize = 422;
/// Bound on WSP sessions; the least recently used one goes first.
const MAX_SESSIONS: usize = 256;
/// A session with no request for this long is forgotten (the terminal starts a new one).
const SESSION_IDLE_SECS: u64 = 3600;
/// Domains listed in the dashboard status.
const RECENT_DOMAINS: usize = 10;
/// The dashboard status is refreshed at most this often.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
const ABORT_REASON_CAPTEMPEXCEEDED: u8 = 7;
/// Debug datagrams taken per stack tick.
const MAX_DEBUG_IN_PER_TICK: usize = 16;

/// How a peer reaches the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WapVia {
    /// Over the air (SNDCP bearer).
    Air,
    /// Through the debug UDP bearer.
    DebugUdp,
}

/// A terminal talking to the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WapPeer {
    pub issi: u32,
    pub ip: Ipv4Addr,
    pub via: WapVia,
}

/// A UDP datagram from a terminal. `dst_port` is the gateway port it was sent to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpIn {
    pub peer: WapPeer,
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: Vec<u8>,
}

/// A UDP datagram for a terminal, from gateway port `src_port` to its port `dst_port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpOut {
    pub peer: WapPeer,
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: Vec<u8>,
}

impl UdpOut {
    /// The datagram as an IPv4 N-PDU from the gateway to the terminal.
    pub fn to_ipv4(&self, gateway: Ipv4Addr, identification: u16) -> Result<Vec<u8>, IpPrimitiveError> {
        build_ipv4_udp_npdu(
            gateway.octets(),
            self.peer.ip.octets(),
            self.src_port,
            self.dst_port,
            &self.payload,
            identification,
            64,
        )
    }
}

/// Why an uplink N-PDU was not for the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapInputError {
    Ip(IpPrimitiveError),
    Fragment,
    NotUdp { protocol: u8 },
    WrongDestination { destination: Ipv4Addr },
    WrongPort { port: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TxKey {
    ip: Ipv4Addr,
    port: u16,
    tid: u16,
}

enum TxState {
    /// Segmented Invoke being collected; the class is known once packet 0 (the Invoke) arrived.
    Reassembling {
        msg: InMessage,
        class: Option<u8>,
    },
    /// Waiting for the fetcher; the hold-on Ack goes out if it takes longer than `hold_on_ms`.
    Fetching {
        id: u64,
        since: Instant,
        held: bool,
    },
    Sending(OutMessage),
    Done {
        until: Instant,
    },
}

struct Tx {
    peer: WapPeer,
    server_port: u16,
    state: TxState,
}

struct Session {
    issi: u32,
    client_sdu: usize,
    peer: WapPeer,
    last_seen_unix: u64,
    /// Host of the last page fetched in this session.
    last_domain: Option<String>,
}

impl Session {
    fn new(peer: WapPeer, client_sdu: usize) -> Self {
        Self {
            issi: peer.issi,
            client_sdu,
            peer,
            last_seen_unix: unix_now(),
            last_domain: None,
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

type StatusSource = Box<dyn Fn() -> WapStatusSnapshot + Send>;

pub struct WapGateway {
    cfg: CfgWap,
    fetcher: Box<dyn Fetcher>,
    status: StatusSource,
    sessions: HashMap<(Ipv4Addr, u16), Session>,
    txs: HashMap<TxKey, Tx>,
    path_limits: HashMap<u32, usize>,
    /// ISSIs whose terminal refused a segmented result (`sar = "auto"`).
    no_sar: HashSet<u32>,
    /// Transactions whose Invoke said the terminal does no SAR (GTR and TTR both set).
    no_sar_txs: HashSet<TxKey>,
    /// ISSIs that answered a segmented result: a later silent one means a lost radio, not SAR.
    sar_answered: HashSet<u32>,
    /// Round trip measured with each radio (adaptive retransmission timer).
    rtt: HashMap<u32, Rtt>,
    /// Radios the bearer cannot reach for now: their retransmission timers wait.
    paused: HashSet<u32>,
    next_session_id: u32,
    next_fetch_id: u64,
    out: Vec<UdpOut>,
    /// Hosts fetched recently, newest first, with a count (dashboard).
    recent_domains: VecDeque<(String, u32)>,
    fetches: u64,
}

fn push(out: &mut Vec<UdpOut>, peer: WapPeer, server_port: u16, client_port: u16, payload: Vec<u8>) {
    out.push(UdpOut {
        peer,
        src_port: server_port,
        dst_port: client_port,
        payload,
    });
}

/// Expected air time of `bytes` octets at `rate` octets per second.
fn air_time(rate: u32, bytes: usize) -> Duration {
    Duration::from_millis(bytes as u64 * 1000 / u64::from(rate.max(1)))
}

/// WTP retransmission timer (R): a fixed part plus the air time of what was sent. The fixed part
/// is `retry_base_ms`, or the wait the round trips measured with the terminal call for when that
/// is longer (air time taken out of them, capped at MAX_ADAPTIVE_TIMER).
fn retry_timer(wtp: &CfgWapWtp, rtt: Option<&Rtt>) -> impl Fn(usize) -> Duration + use<> {
    let measured = rtt.map_or(Duration::ZERO, |rtt| rtt.timeout().min(MAX_ADAPTIVE_TIMER));
    let base = Duration::from_millis(wtp.retry_base_ms).max(measured);
    let rate = wtp.air_rate_bytes_per_sec;
    move |bytes| base + air_time(rate, bytes)
}

/// Domain-level log of what a radio asked the Internet side for (never the full URL).
fn target_for_log(target: &FetchTarget) -> String {
    match target {
        FetchTarget::Url(url) => url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(url)
            .split(['/', '?', '#', ':'])
            .next()
            .unwrap_or("")
            .to_string(),
        FetchTarget::Search(_) => "(search)".to_string(),
        FetchTarget::Doc(path) if path.starts_with("/wx") => "(weather)".to_string(),
        FetchTarget::Doc(path) if path.starts_with("/rid") => "(radioid)".to_string(),
        FetchTarget::Doc(_) => "(document page)".to_string(),
    }
}

impl WapGateway {
    pub fn new(cfg: CfgWap, fetcher: Box<dyn Fetcher>, status: StatusSource) -> Self {
        Self {
            cfg,
            fetcher,
            status,
            sessions: HashMap::new(),
            txs: HashMap::new(),
            path_limits: HashMap::new(),
            no_sar: HashSet::new(),
            no_sar_txs: HashSet::new(),
            sar_answered: HashSet::new(),
            rtt: HashMap::new(),
            paused: HashSet::new(),
            next_session_id: 1,
            next_fetch_id: 1,
            out: Vec::new(),
            recent_domains: VecDeque::new(),
            fetches: 0,
        }
    }

    /// Sessions and recently fetched domains, for the dashboard.
    pub fn status(&self) -> WapRuntimeStatus {
        let mut sessions: Vec<WapSessionStatus> = self
            .sessions
            .values()
            .map(|s| WapSessionStatus {
                issi: s.issi,
                ip: s.peer.ip.to_string(),
                via: match s.peer.via {
                    WapVia::Air => "air",
                    WapVia::DebugUdp => "debug-udp",
                }
                .to_string(),
                last_seen_unix: s.last_seen_unix,
                last_domain: s.last_domain.clone(),
            })
            .collect();
        sessions.sort_by(|a, b| (a.issi, &a.ip).cmp(&(b.issi, &b.ip)));
        WapRuntimeStatus {
            running: true,
            browse_enabled: self.cfg.browse.enabled,
            sessions,
            recent_domains: self.recent_domains.iter().cloned().collect(),
            fetches: self.fetches,
        }
    }

    /// Forget sessions that have been idle for `idle_secs`.
    pub fn expire_sessions(&mut self, now_unix: u64, idle_secs: u64) {
        self.sessions.retain(|_, s| now_unix.saturating_sub(s.last_seen_unix) < idle_secs);
    }

    fn note_domain(&mut self, key: TxKey, domain: String) {
        if let Some(session) = self.sessions.get_mut(&(key.ip, key.port)) {
            session.last_domain = Some(domain.clone());
        }
        let count = match self.recent_domains.iter().position(|(d, _)| *d == domain) {
            Some(i) => self.recent_domains.remove(i).map(|(_, n)| n).unwrap_or(0),
            None => 0,
        };
        self.recent_domains.push_front((domain, count.saturating_add(1)));
        self.recent_domains.truncate(RECENT_DOMAINS);
    }

    /// Apply a new browse switch / ISSI list (dashboard override).
    pub fn set_browse(&mut self, browse: CfgWapBrowse) {
        self.cfg.browse = browse;
    }

    /// Largest IPv4 datagram the bearer can carry to `issi` (min of the MTU and the link's N.271).
    pub fn set_path_limits(&mut self, issi: u32, max_datagram_bytes: usize) {
        self.path_limits.insert(issi, max_datagram_bytes.clamp(96, 65_535));
    }

    /// The radio is gone (deregistered, PDP context deactivated): drop its sessions and transactions.
    pub fn on_peer_lost(&mut self, issi: u32) {
        self.sessions.retain(|_, s| s.issi != issi);
        self.txs.retain(|_, t| t.peer.issi != issi);
        self.path_limits.remove(&issi);
        self.no_sar.remove(&issi);
        self.sar_answered.remove(&issi);
        self.rtt.remove(&issi);
        self.paused.remove(&issi);
    }

    /// Whether `issi` has a transaction under way: a request being reassembled, a download or a
    /// result not yet acknowledged.
    pub fn has_open_transactions(&self, issi: u32) -> bool {
        self.txs
            .values()
            .any(|t| t.peer.issi == issi && !matches!(t.state, TxState::Done { .. }))
    }

    /// The bearer cannot carry data to `issi` for now (voice took the data slot): the
    /// retransmission timers of its results stop until [`Self::resume_timers`].
    pub fn pause_timers(&mut self, issi: u32) {
        self.paused.insert(issi);
    }

    /// The bearer carries data to `issi` again: the timers of its results start afresh, as the
    /// datagrams it held go out now.
    pub fn resume_timers(&mut self, issi: u32, now: Instant) {
        if !self.paused.remove(&issi) {
            return;
        }
        let timer = self.timer_for(issi);
        for tx in self.txs.values_mut().filter(|t| t.peer.issi == issi) {
            if let TxState::Sending(out) = &mut tx.state {
                out.restart_timer(now, &timer);
            }
        }
    }

    fn timer_for(&self, issi: u32) -> impl Fn(usize) -> Duration + use<> {
        retry_timer(&self.cfg.wtp, self.rtt.get(&issi))
    }

    /// Feed a round trip (and the octets of its group) into the radio's estimate.
    fn note_rtt(&mut self, issi: u32, sample: Option<(Duration, usize)>) {
        let Some((rtt, bytes)) = sample else { return };
        let rtt = rtt.saturating_sub(air_time(self.cfg.wtp.air_rate_bytes_per_sec, bytes));
        match self.rtt.get_mut(&issi) {
            Some(estimate) => estimate.update(rtt),
            None => {
                self.rtt.insert(issi, Rtt::new(rtt));
            }
        }
    }

    /// An uplink IPv4 N-PDU from `issi`. Only unfragmented UDP to the gateway address on the WAP
    /// ports is accepted; `max_reply_bytes` is the largest datagram the bearer can send back.
    pub fn on_ipv4(&mut self, issi: u32, npdu: &[u8], max_reply_bytes: Option<usize>, now: Instant) -> Result<(), WapInputError> {
        let ip = parse_ipv4_packet(npdu).map_err(WapInputError::Ip)?;
        if ip.flags_fragment & 0x3fff != 0 {
            return Err(WapInputError::Fragment);
        }
        if ip.protocol != IPV4_PROTOCOL_UDP {
            return Err(WapInputError::NotUdp { protocol: ip.protocol });
        }
        let destination = Ipv4Addr::from(ip.destination);
        if destination != self.cfg.gateway_ipv4 {
            return Err(WapInputError::WrongDestination { destination });
        }
        let udp = parse_udp_datagram(ip.payload).map_err(WapInputError::Ip)?;
        if !matches!(udp.destination_port, WAP_PORT_CONNECTIONLESS | WAP_PORT_CONNECTION) {
            return Err(WapInputError::WrongPort {
                port: udp.destination_port,
            });
        }
        if let Some(limit) = max_reply_bytes {
            self.set_path_limits(issi, limit);
        }
        self.on_udp(
            UdpIn {
                peer: WapPeer {
                    issi,
                    ip: Ipv4Addr::from(ip.source),
                    via: WapVia::Air,
                },
                src_port: udp.source_port,
                dst_port: udp.destination_port,
                payload: udp.payload.to_vec(),
            },
            now,
        );
        Ok(())
    }

    /// A WTP datagram from a terminal (the bearer has checked address and port).
    pub fn on_udp(&mut self, input: UdpIn, now: Instant) {
        let pdus = match wtp::parse_datagram(&input.payload) {
            Ok(pdus) => pdus,
            Err(wtp::WtpError::UnsupportedVersion { tid }) => {
                tracing::debug!("WAP: ISSI {} used another WTP version (TID {})", input.peer.issi, tid);
                let abort = wtp::abort(tid, wtp::ABORT_PROVIDER, wtp::ABORT_REASON_WTPVERSIONONE);
                push(&mut self.out, input.peer, input.dst_port, input.src_port, abort);
                return;
            }
            Err(e) => {
                tracing::debug!("WAP: ISSI {} sent a datagram that is not WTP ({:?})", input.peer.issi, e);
                return;
            }
        };
        for pdu in pdus {
            let key = |tid| TxKey {
                ip: input.peer.ip,
                port: input.src_port,
                tid,
            };
            match pdu {
                WtpPdu::Invoke(inv) => self.on_invoke(input.peer, input.src_port, input.dst_port, inv, now),
                WtpPdu::SegmentedInvoke {
                    tid, gtr, ttr, psn, data, ..
                } => {
                    let flags = if gtr { wtp::GTR } else { 0 } | if ttr { wtp::TTR } else { 0 };
                    if !self.txs.contains_key(&key(tid)) {
                        if !self.has_room(input.peer.issi, true) {
                            self.refuse(input.peer, input.dst_port, input.src_port, tid);
                            continue;
                        }
                        // The first packet (the Invoke) was lost: collect anyway, the Nack asks for it.
                        self.insert_tx(
                            key(tid),
                            input.peer,
                            input.dst_port,
                            TxState::Reassembling {
                                msg: InMessage::new(now),
                                class: None,
                            },
                        );
                    }
                    self.on_segment(key(tid), psn, flags, data, now);
                }
                WtpPdu::Ack { tid, tve_tok, psn, .. } => {
                    if !tve_tok {
                        self.on_ack(key(tid), psn, now);
                    }
                }
                WtpPdu::Nack { tid, missing, .. } => self.on_nack(key(tid), &missing, now),
                WtpPdu::Abort { tid, abort_type, reason } => self.on_abort(key(tid), abort_type, reason),
                WtpPdu::Unexpected { pdu_type, tid } => {
                    tracing::debug!("WAP: ISSI {} sent WTP PDU type {} (TID {})", input.peer.issi, pdu_type, tid);
                }
            }
        }
    }

    /// Finished fetches, WTP timers and expiries; returns the datagrams to send.
    pub fn poll(&mut self, now: Instant) -> Vec<UdpOut> {
        while let Some(reply) = self.fetcher.try_recv() {
            self.on_fetch_reply(reply, now);
        }
        if !self.txs.is_empty() {
            let keys: Vec<TxKey> = self.txs.keys().copied().collect();
            for key in keys {
                self.run_timer(key, now);
            }
        }
        if !self.no_sar_txs.is_empty() {
            let txs = &self.txs;
            self.no_sar_txs.retain(|key| txs.contains_key(key));
        }
        std::mem::take(&mut self.out)
    }

    fn insert_tx(&mut self, key: TxKey, peer: WapPeer, server_port: u16, state: TxState) {
        self.txs.insert(key, Tx { peer, server_port, state });
    }

    /// Whether a new transaction of `issi` fits: the gateway-wide bound and, for a segmented
    /// Invoke (which may wait half-received for REASSEMBLY_TIMEOUT), a bound per radio.
    fn has_room(&self, issi: u32, reassembling: bool) -> bool {
        self.txs.len() < MAX_TRANSACTIONS
            && (!reassembling
                || self
                    .txs
                    .values()
                    .filter(|t| t.peer.issi == issi && matches!(t.state, TxState::Reassembling { .. }))
                    .count()
                    < MAX_REASSEMBLING_PER_PEER)
    }

    fn refuse(&mut self, peer: WapPeer, server_port: u16, port: u16, tid: u16) {
        tracing::warn!("WAP: too many open transactions, ISSI {} refused", peer.issi);
        let abort = wtp::abort(tid, wtp::ABORT_PROVIDER, ABORT_REASON_CAPTEMPEXCEEDED);
        push(&mut self.out, peer, server_port, port, abort);
    }

    fn sar_enabled(&self, issi: u32) -> bool {
        match self.cfg.wtp.sar {
            WapSarMode::Off => false,
            WapSarMode::On => true,
            WapSarMode::Auto => !self.no_sar.contains(&issi),
        }
    }

    /// Payload octets of one Result packet to `issi`.
    fn segment_bytes(&self, issi: u32) -> usize {
        let datagram = self.path_limits.get(&issi).copied().unwrap_or(self.cfg.mtu as usize);
        datagram.saturating_sub(IPV4_UDP_HEADERS + SEGMENTED_RESULT_HEADER).max(1)
    }

    /// Whether the result of this transaction may be segmented.
    fn sar_for(&self, key: TxKey, issi: u32) -> bool {
        self.sar_enabled(issi) && !self.no_sar_txs.contains(&key)
    }

    /// Largest WSP message to this terminal: one packet without SAR, else the negotiated
    /// Client-SDU and `max_message_bytes`.
    fn message_limit(&self, key: TxKey, issi: u32) -> usize {
        let seg = self.segment_bytes(issi);
        let client_sdu = self
            .sessions
            .get(&(key.ip, key.port))
            .map(|s| s.client_sdu)
            .unwrap_or(self.cfg.max_message_bytes);
        if self.sar_for(key, issi) {
            client_sdu.min(self.cfg.max_message_bytes).min(seg * MAX_PACKETS)
        } else {
            // A Result carries one octet more than a Segmented Result.
            client_sdu.min(seg + 1)
        }
    }

    fn body_budget(&self, key: TxKey, issi: u32) -> usize {
        self.message_limit(key, issi).saturating_sub(wsp::REPLY_OVERHEAD_MAX)
    }

    fn on_invoke(&mut self, peer: WapPeer, port: u16, server_port: u16, inv: Invoke<'_>, now: Instant) {
        let key = TxKey {
            ip: peer.ip,
            port,
            tid: inv.tid,
        };
        let flags = if inv.gtr { wtp::GTR } else { 0 } | if inv.ttr { wtp::TTR } else { 0 };
        // A retransmitted Invoke (RID) keeps the TIDnew of the original: it only resets the cache
        // when its own transaction is not known (the original was lost).
        if inv.tid_new && (!inv.rid || !self.txs.contains_key(&key)) {
            // The terminal restarted its TIDs: forget the ones cached for it.
            self.txs.retain(|k, _| !(k.ip == peer.ip && k.port == port));
        }
        if inv.class == 0 {
            // Unreliable, no result (Disconnect, Suspend).
            self.handle_request(key, peer, server_port, 0, inv.data, now);
            return;
        }
        let timer = self.timer_for(peer.issi);
        let max_retries = self.cfg.wtp.max_retries;
        if let Some(tx) = self.txs.get_mut(&key) {
            match &mut tx.state {
                TxState::Done { .. } if inv.rid => return, // late duplicate
                TxState::Done { .. } => {
                    self.txs.remove(&key); // TID reused for a new transaction
                }
                TxState::Fetching { held, .. } => {
                    // The terminal did not get a hold-on Ack: send it now, nothing else.
                    *held = true;
                    push(&mut self.out, peer, server_port, port, wtp::ack(inv.tid, None, true));
                    return;
                }
                TxState::Sending(out) => {
                    // Before any Ack a repeated Invoke means the Result was lost: send it again
                    // at once instead of waiting for the timer.
                    if inv.rid
                        && let Some(packets) = out.on_repeated_invoke(max_retries, now, &timer)
                    {
                        for packet in packets {
                            push(&mut self.out, peer, server_port, port, packet);
                        }
                    }
                    return;
                }
                TxState::Reassembling { class, .. } => {
                    *class = Some(inv.class);
                    self.on_segment(key, 0, flags, inv.data, now);
                    return;
                }
            }
        }
        if !self.has_room(peer.issi, !inv.ttr) {
            self.refuse(peer, server_port, port, inv.tid);
            return;
        }
        if inv.data.len() > self.cfg.max_request_bytes {
            let abort = wtp::abort(inv.tid, wtp::ABORT_PROVIDER, wtp::ABORT_REASON_MESSAGETOOLARGE);
            push(&mut self.out, peer, server_port, port, abort);
            return;
        }
        // GTR and TTR together: the terminal does not do SAR (WAP-224, GTR/TTR flags).
        if inv.gtr && inv.ttr {
            self.no_sar_txs.insert(key);
            if self.cfg.wtp.sar == WapSarMode::Auto && self.no_sar.insert(peer.issi) {
                tracing::info!("WAP: ISSI {} does not do SAR, single datagrams from now on", peer.issi);
            }
        } else {
            self.no_sar_txs.remove(&key);
        }
        if inv.ttr {
            if inv.class == 1 {
                push(&mut self.out, peer, server_port, port, wtp::ack(inv.tid, None, false));
            }
            self.handle_request(key, peer, server_port, inv.class, inv.data, now);
        } else {
            // First packet of a segmented Invoke.
            let state = TxState::Reassembling {
                msg: InMessage::new(now),
                class: Some(inv.class),
            };
            self.insert_tx(key, peer, server_port, state);
            self.on_segment(key, 0, flags, inv.data, now);
        }
    }

    fn on_segment(&mut self, key: TxKey, psn: u8, flags: u8, data: &[u8], now: Instant) {
        let max = self.cfg.max_request_bytes;
        let Some(tx) = self.txs.get_mut(&key) else { return };
        let (peer, server_port) = (tx.peer, tx.server_port);
        let TxState::Reassembling { msg, class } = &mut tx.state else {
            return;
        };
        let class = *class;
        match msg.add(psn, flags, data, max, now) {
            Reassembly::Incomplete => {}
            Reassembly::GroupAck(psn) => push(&mut self.out, peer, server_port, key.port, wtp::ack(key.tid, Some(psn), false)),
            Reassembly::Nack(missing) => push(&mut self.out, peer, server_port, key.port, wtp::nack(key.tid, &missing)),
            Reassembly::TooLarge => {
                self.txs.remove(&key);
                let abort = wtp::abort(key.tid, wtp::ABORT_PROVIDER, wtp::ABORT_REASON_MESSAGETOOLARGE);
                push(&mut self.out, peer, server_port, key.port, abort);
            }
            Reassembly::Complete(message) => {
                self.txs.remove(&key);
                let class = class.unwrap_or(2);
                if class == 1 {
                    push(&mut self.out, peer, server_port, key.port, wtp::ack(key.tid, None, false));
                }
                self.handle_request(key, peer, server_port, class, &message, now);
            }
        }
    }

    fn handle_request(&mut self, key: TxKey, peer: WapPeer, server_port: u16, class: u8, wsp_pdu: &[u8], now: Instant) {
        if let Some(session) = self.sessions.get_mut(&(peer.ip, key.port)) {
            session.last_seen_unix = unix_now();
        }
        match wsp::parse_request(wsp_pdu) {
            WspRequest::Connect(connect) => {
                let id = self.next_session_id;
                self.next_session_id = self.next_session_id.wrapping_add(1).max(1);
                // Not negotiated: the WSP default.
                let client_sdu = wsp::requested_client_sdu(&connect)
                    .unwrap_or(wsp::DEFAULT_SDU)
                    .min(self.cfg.max_message_bytes);
                if self.sessions.len() >= MAX_SESSIONS
                    && !self.sessions.contains_key(&(peer.ip, key.port))
                    && let Some(oldest) = self.sessions.iter().min_by_key(|(_, s)| s.last_seen_unix).map(|(k, _)| *k)
                {
                    self.sessions.remove(&oldest);
                }
                self.sessions.insert((peer.ip, key.port), Session::new(peer, client_sdu));
                tracing::info!("WAP: ISSI {} connected (session {}, Client-SDU {})", peer.issi, id, client_sdu);
                let reply = wsp::connect_reply(id, &connect, self.cfg.max_message_bytes, self.cfg.max_request_bytes);
                self.respond(key, peer, server_port, class, reply, now);
            }
            WspRequest::Resume { session_id } => {
                // A session this gateway does not know: nothing negotiated, the WSP default.
                let sdu = wsp::DEFAULT_SDU.min(self.cfg.max_message_bytes);
                self.sessions.entry((peer.ip, key.port)).or_insert_with(|| Session::new(peer, sdu));
                tracing::info!("WAP: ISSI {} resumed session {}", peer.issi, session_id);
                self.respond(key, peer, server_port, class, wsp::empty_reply(status::OK), now);
            }
            WspRequest::Disconnect => {
                self.sessions.remove(&(peer.ip, key.port));
                tracing::info!("WAP: ISSI {} disconnected", peer.issi);
                self.respond(key, peer, server_port, class, wsp::empty_reply(status::OK), now);
            }
            WspRequest::Suspend => self.respond(key, peer, server_port, class, wsp::empty_reply(status::OK), now),
            WspRequest::Get { uri, referer } => {
                let budget = self.body_budget(key, peer.issi);
                let ctx = RouteCtx {
                    cfg: &self.cfg,
                    issi: peer.issi,
                    budget,
                    snapshot: &*self.status,
                };
                match router::route(&uri, &ctx) {
                    Route::Page(page) => self.respond_page(key, peer, server_port, class, page, now),
                    Route::Fetch(target) => {
                        // A document link or page: the page it was on, in case the document is gone.
                        let back = match (&target, referer) {
                            (FetchTarget::Doc(path), Some(referer)) if path.starts_with("/l/") || path.starts_with("/p/") => {
                                match router::route(referer, &ctx) {
                                    Route::Fetch(back @ (FetchTarget::Url(_) | FetchTarget::Search(_))) => Some(back),
                                    _ => None,
                                }
                            }
                            _ => None,
                        };
                        self.start_fetch(key, peer, server_port, class, target, back, budget, now)
                    }
                }
            }
            WspRequest::Unsupported { pdu_type } => {
                tracing::debug!("WAP: ISSI {} used WSP method 0x{:02x}", peer.issi, pdu_type);
                let page = self.notice(
                    status::METHOD_NOT_ALLOWED,
                    "No admitido",
                    "Solo se admiten peticiones GET.",
                    key,
                    peer.issi,
                );
                self.respond_page(key, peer, server_port, class, page, now);
            }
            WspRequest::Malformed => {
                tracing::debug!("WAP: ISSI {} sent a malformed WSP PDU", peer.issi);
                if class != 0 {
                    let abort = wtp::abort(key.tid, wtp::ABORT_USER, wsp::ABORT_PROTOERR);
                    push(&mut self.out, peer, server_port, key.port, abort);
                }
            }
        }
    }

    fn notice(&self, status: u8, title: &str, text: &str, key: TxKey, issi: u32) -> Page {
        let body = home::notice_page(title, text, &home::home_url(self.cfg.gateway_ipv4), self.body_budget(key, issi));
        Page {
            status,
            kind: ContentKind::Xhtml,
            body: body.into_bytes(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_fetch(
        &mut self,
        key: TxKey,
        peer: WapPeer,
        server_port: u16,
        class: u8,
        target: FetchTarget,
        back: Option<FetchTarget>,
        budget: usize,
        now: Instant,
    ) {
        if class != 2 {
            return; // nowhere to put the result
        }
        let id = self.next_fetch_id;
        self.next_fetch_id += 1;
        tracing::info!("WAP: ISSI {} -> {}", peer.issi, target_for_log(&target));
        let req = FetchRequest {
            id,
            issi: peer.issi,
            target,
            budget,
            home: home::home_url(self.cfg.gateway_ipv4),
            back,
        };
        if self.fetcher.submit(req).is_err() {
            let page = self.notice(
                status::SERVICE_UNAVAILABLE,
                "Ocupado",
                "La estación está atendiendo otras páginas.",
                key,
                peer.issi,
            );
            self.respond_page(key, peer, server_port, class, page, now);
            return;
        }
        self.fetches += 1;
        // A Result ready soon acknowledges the Invoke itself; the timer sends the hold-on Ack
        // (the WSP user's acknowledgement when U/P is set) if the download takes longer.
        let state = TxState::Fetching {
            id,
            since: now,
            held: false,
        };
        self.insert_tx(key, peer, server_port, state);
    }

    fn on_fetch_reply(&mut self, reply: FetchReply, now: Instant) {
        let found = self
            .txs
            .iter()
            .find(|(_, tx)| matches!(tx.state, TxState::Fetching { id, .. } if id == reply.id))
            .map(|(key, tx)| (*key, tx.peer, tx.server_port));
        match found {
            Some((key, peer, server_port)) => {
                if let Some(domain) = reply.domain {
                    self.note_domain(key, domain);
                }
                self.respond_page(key, peer, server_port, 2, reply.page, now)
            }
            None => tracing::debug!("WAP: fetch {} finished after its transaction ended", reply.id),
        }
    }

    fn respond_page(&mut self, key: TxKey, peer: WapPeer, server_port: u16, class: u8, page: Page, now: Instant) {
        let message = wsp::reply(page.status, page.kind, self.cfg.content_type, &page.body);
        self.respond(key, peer, server_port, class, message, now);
    }

    /// Send a WSP message as the Result of a class 2 transaction (classes 0 and 1 have none).
    fn respond(&mut self, key: TxKey, peer: WapPeer, server_port: u16, class: u8, message: Vec<u8>, now: Instant) {
        if class != 2 {
            return;
        }
        let limit = self.message_limit(key, peer.issi);
        let message = if message.len() > limit {
            tracing::warn!("WAP: reply of {} bytes exceeds {} for ISSI {}", message.len(), limit, peer.issi);
            wsp::empty_reply(status::INTERNAL_ERROR)
        } else {
            message
        };
        let segment = if self.sar_for(key, peer.issi) {
            self.segment_bytes(peer.issi)
        } else {
            message.len()
        };
        let mut out = OutMessage::new(key.tid, &message, segment, self.cfg.wtp.group_size as usize, now)
            .with_retransmit_rid(self.cfg.wtp.retransmit_rid);
        for packet in out.send_group(now, self.timer_for(peer.issi)) {
            push(&mut self.out, peer, server_port, key.port, packet);
        }
        self.insert_tx(key, peer, server_port, TxState::Sending(out));
    }

    fn on_ack(&mut self, key: TxKey, psn: Option<u8>, now: Instant) {
        let Some(issi) = self.txs.get(&key).map(|t| t.peer.issi) else {
            return;
        };
        let timer = self.timer_for(issi);
        let Some(tx) = self.txs.get_mut(&key) else { return };
        let TxState::Sending(out) = &mut tx.state else { return };
        let outcome = out.on_ack(psn, now, timer);
        let sample = out.take_rtt_sample();
        let sar_answered = out.packet_count() > 1 && out.heard_back();
        match outcome {
            AckOutcome::Done => tx.state = TxState::Done { until: now + DONE_HOLD },
            AckOutcome::Next(packets) => {
                for packet in packets {
                    push(&mut self.out, tx.peer, tx.server_port, key.port, packet);
                }
            }
            AckOutcome::Ignored => {}
        }
        self.note_rtt(issi, sample);
        if sar_answered {
            self.sar_answered.insert(issi);
        }
    }

    fn on_nack(&mut self, key: TxKey, missing: &[u8], now: Instant) {
        let Some(issi) = self.txs.get(&key).map(|t| t.peer.issi) else {
            return;
        };
        let timer = self.timer_for(issi);
        let max_retries = self.cfg.wtp.max_retries;
        let Some(tx) = self.txs.get_mut(&key) else { return };
        let (peer, server_port) = (tx.peer, tx.server_port);
        let TxState::Sending(out) = &mut tx.state else { return };
        if out.packet_count() > 1 {
            self.sar_answered.insert(issi);
        }
        match out.on_nack(missing, max_retries, now, timer) {
            Some(packets) => {
                for packet in packets {
                    push(&mut self.out, peer, server_port, key.port, packet);
                }
            }
            None => {
                self.txs.remove(&key);
                let abort = wtp::abort(key.tid, wtp::ABORT_PROVIDER, wtp::ABORT_REASON_NORESPONSE);
                push(&mut self.out, peer, server_port, key.port, abort);
            }
        }
    }

    fn on_abort(&mut self, key: TxKey, abort_type: u8, reason: u8) {
        let Some(tx) = self.txs.remove(&key) else { return };
        let info = WtpAbortInfo { abort_type, reason };
        tracing::info!(
            "WAP: ISSI {} aborted TID {} ({})",
            tx.peer.issi,
            key.tid,
            wtp_abort_reason_name(info)
        );
        let segmented = matches!(&tx.state, TxState::Sending(out) if out.packet_count() > 1);
        let sar_refused =
            abort_type == wtp::ABORT_PROVIDER && matches!(reason, wtp::ABORT_REASON_NOTIMPLEMENTEDSAR | wtp::ABORT_REASON_MESSAGETOOLARGE);
        if segmented && sar_refused && self.cfg.wtp.sar == WapSarMode::Auto && self.no_sar.insert(tx.peer.issi) {
            tracing::info!(
                "WAP: ISSI {} does not take segmented results, single datagrams from now on",
                tx.peer.issi
            );
        }
    }

    fn run_timer(&mut self, key: TxKey, now: Instant) {
        let Some(issi) = self.txs.get(&key).map(|t| t.peer.issi) else {
            return;
        };
        let timer = self.timer_for(issi);
        let paused = self.paused.contains(&issi);
        let max_retries = self.cfg.wtp.max_retries;
        let hold_on = Duration::from_millis(self.cfg.wtp.hold_on_ms);
        let Some(tx) = self.txs.get_mut(&key) else { return };
        let (peer, server_port) = (tx.peer, tx.server_port);
        match &mut tx.state {
            TxState::Sending(_) if paused => {}
            TxState::Sending(out) => match out.on_timer(max_retries, now, timer) {
                TimerOutcome::Idle => {}
                TimerOutcome::Retransmit(packet) => push(&mut self.out, peer, server_port, key.port, packet),
                TimerOutcome::GiveUp => {
                    tracing::info!("WAP: ISSI {} stopped answering TID {}, transaction aborted", peer.issi, key.tid);
                    // Not a single Ack or Nack of a segmented result, from a radio that never
                    // answered one: it may be dropping Segmented Results without saying so.
                    let unheard = out.packet_count() > 1 && !out.heard_back() && !self.sar_answered.contains(&issi);
                    self.txs.remove(&key);
                    let abort = wtp::abort(key.tid, wtp::ABORT_PROVIDER, wtp::ABORT_REASON_NORESPONSE);
                    push(&mut self.out, peer, server_port, key.port, abort);
                    if unheard && self.cfg.wtp.sar == WapSarMode::Auto && self.no_sar.insert(issi) {
                        tracing::info!(
                            "WAP: ISSI {} never acknowledged a segmented result, single datagrams from now on",
                            issi
                        );
                    }
                }
            },
            TxState::Done { until } => {
                if now >= *until {
                    self.txs.remove(&key);
                }
            }
            TxState::Reassembling { msg, .. } => {
                if now.duration_since(msg.last_activity) > REASSEMBLY_TIMEOUT {
                    self.txs.remove(&key);
                }
            }
            TxState::Fetching { since, held, .. } => {
                let waited = now.duration_since(*since);
                if !*held && waited >= hold_on {
                    *held = true;
                    push(&mut self.out, peer, server_port, key.port, wtp::ack(key.tid, None, false));
                }
                if waited > FETCH_GUARD {
                    let page = self.notice(status::GATEWAY_TIMEOUT, "Sin respuesta", "La página tarda demasiado.", key, issi);
                    self.respond_page(key, peer, server_port, 2, page, now);
                }
            }
        }
    }
}

/// Whether an `mtu` is too small for a terminal's WSP Connect to arrive unfragmented.
fn fragments_requests(mtu: u16) -> bool {
    usize::from(mtu) < CONNECT_IPV4_BYTES
}

/// Helper threads of the gateway leave the station's SCHED_FIFO (inherited from the stack): a
/// flood on a helper must not take CPU from the TETRA stack.
pub(crate) fn leave_realtime_scheduling() {
    #[cfg(target_os = "linux")]
    // SAFETY: one syscall on the calling thread with a zeroed sched_param.
    unsafe {
        let param: libc::sched_param = std::mem::zeroed();
        let _ = libc::sched_setscheduler(0, libc::SCHED_OTHER, &param);
    }
}

/// The gateway as the SNDCP entity runs it: built from `[wap]`, with the debug UDP bearer when
/// configured, driven from the stack tick.
pub struct WapService {
    gateway: WapGateway,
    debug: Option<DebugUdp>,
    debug_issi: u32,
    last_status: Option<Instant>,
    /// What the dashboard was last given.
    published: WapRuntimeStatus,
}

impl WapService {
    /// `None` unless `[wap] enabled`.
    pub fn start(config: &SharedConfig) -> Option<Self> {
        let policy = NetPolicy::new(&config.config().wap.browse);
        Self::start_with_policy(config, policy)
    }

    fn start_with_policy(config: &SharedConfig, policy: NetPolicy) -> Option<Self> {
        let cfg = config.effective_wap();
        if !cfg.enabled {
            return None;
        }
        if fragments_requests(cfg.mtu) {
            tracing::warn!(
                "WAP: mtu {} is below a WSP Connect ({} octets): the terminal must segment it with WTP SAR, IPv4 fragments are dropped",
                cfg.mtu,
                CONNECT_IPV4_BYTES
            );
        }
        let thresholds = snapshot::thresholds(&config.config());
        // The pool runs even with browsing off: the dashboard can switch browsing on at runtime.
        let fetcher: Box<dyn Fetcher> = match PoolFetcher::spawn(&cfg, policy) {
            Ok(pool) => Box::new(pool),
            Err(e) => {
                tracing::error!("WAP: download threads not started ({e}), browsing unavailable");
                Box::new(UnavailableFetcher::default())
            }
        };
        let gateway = WapGateway::new(cfg.clone(), fetcher, Box::new(move || snapshot::station_snapshot(&thresholds)));
        let debug = cfg.debug_udp_listen.and_then(|listen| match DebugUdp::spawn(listen, &cfg) {
            Ok(debug) => Some(debug),
            Err(e) => {
                tracing::error!("WAP: debug UDP bearer on {listen} not started: {e}");
                None
            }
        });
        tracing::info!(
            "WAP: gateway on {} (browsing {}, {} radio(s) allowed)",
            cfg.gateway_ipv4,
            if cfg.browse.enabled { "on" } else { "off" },
            cfg.browse.allowed_issis.len()
        );
        Some(Self {
            gateway,
            debug,
            debug_issi: cfg.debug_issi,
            last_status: None,
            published: WapRuntimeStatus::default(),
        })
    }

    /// Address of the debug UDP bearer, if it is running.
    pub fn debug_addr(&self) -> Option<SocketAddrV4> {
        self.debug.as_ref().map(DebugUdp::local_addr)
    }

    /// Address the radios send their WAP requests to.
    pub fn gateway_ipv4(&self) -> Ipv4Addr {
        self.gateway.cfg.gateway_ipv4
    }

    /// An uplink IPv4 N-PDU of `issi` from the SNDCP bearer, which can carry datagrams of up to
    /// `max_reply_bytes` back.
    pub fn on_air_ipv4(
        &mut self,
        config: &SharedConfig,
        issi: u32,
        npdu: &[u8],
        max_reply_bytes: Option<usize>,
        now: Instant,
    ) -> Result<(), WapInputError> {
        // The dashboard may have changed who can browse.
        self.gateway.set_browse(config.effective_wap().browse);
        self.gateway.on_ipv4(issi, npdu, max_reply_bytes, now)
    }

    /// The radio deregistered or dropped its PDP context.
    pub fn peer_lost(&mut self, issi: u32) {
        self.gateway.on_peer_lost(issi);
    }

    /// See [`WapGateway::pause_timers`].
    pub fn pause_timers(&mut self, issi: u32) {
        self.gateway.pause_timers(issi);
    }

    /// See [`WapGateway::resume_timers`].
    pub fn resume_timers(&mut self, issi: u32, now: Instant) {
        self.gateway.resume_timers(issi, now);
    }

    /// See [`WapGateway::has_open_transactions`].
    pub fn has_open_transactions(&self, issi: u32) -> bool {
        self.gateway.has_open_transactions(issi)
    }

    /// One stack tick: take the debug datagrams, run the gateway, send the debug bearer's answers
    /// and return the ones for the air, which the SNDCP bearer carries.
    pub fn tick(&mut self, config: &SharedConfig, now: Instant) -> Vec<UdpOut> {
        if let Some(debug) = &self.debug {
            let port = debug.local_addr().port();
            for i in 0..MAX_DEBUG_IN_PER_TICK {
                let Some((src, payload)) = debug.try_recv() else { break };
                if i == 0 {
                    // The dashboard may have changed who can browse.
                    self.gateway.set_browse(config.effective_wap().browse);
                }
                let peer = WapPeer {
                    issi: self.debug_issi,
                    ip: *src.ip(),
                    via: WapVia::DebugUdp,
                };
                self.gateway.on_udp(
                    UdpIn {
                        peer,
                        src_port: src.port(),
                        dst_port: port,
                        payload,
                    },
                    now,
                );
            }
        }
        let mut air = Vec::new();
        for out in self.gateway.poll(now) {
            match (&self.debug, out.peer.via) {
                (Some(debug), WapVia::DebugUdp) => debug.send(SocketAddrV4::new(out.peer.ip, out.dst_port), out.payload),
                (_, WapVia::Air) => air.push(out),
                (None, WapVia::DebugUdp) => tracing::debug!("WAP: debug bearer gone, reply to ISSI {} dropped", out.peer.issi),
            }
        }
        if self.last_status.is_none_or(|t| now.duration_since(t) >= STATUS_INTERVAL) {
            self.last_status = Some(now);
            // Pick up the dashboard's browse switch and ISSI list even without traffic.
            self.gateway.set_browse(config.effective_wap().browse);
            self.gateway.expire_sessions(unix_now(), SESSION_IDLE_SECS);
            let status = self.gateway.status();
            if status != self.published {
                config.state_write().wap_status = status.clone();
                self.published = status;
            }
        }
        air
    }
}

#[cfg(test)]
mod tests;
