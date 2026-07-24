use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use tetra_config::bluestation::{AsteriskRuntimeStatus, CfgAsterisk, SharedConfig};
use tetra_core::{Sap, TdmaTime, tetra_entities::TetraEntity};
use tetra_pdus::cmce::enums::call_timeout::CallTimeout;
use tetra_saps::{
    SapMsg, SapMsgInner,
    control::call_control::{CallControl, NetworkCircuitCall},
    tmd::{TmdCircuitDataInd, TmdCircuitDataReq},
};
use uuid::Uuid;

use crate::{MessageQueue, TetraEntityTrait};

use super::audio::{AsteriskAudioTranscoder, DownlinkBacklog, PCMU_PAYLOAD_TYPE, RtpPacketizer, rtp_payload};

const SIP_MAX_DATAGRAM: usize = 8192;
const RTP_MAX_DATAGRAM: usize = 1720;
/// Datagrams read per socket per pass. Well above the ~1 packet a 20 ms stream produces per
/// tick, and bounded so a flooded socket cannot hold the tick hostage.
const RTP_READS_PER_TICK: usize = 32;
/// Reads used to empty a socket in one go. A default receive buffer holds a few hundred
/// 172-byte datagrams; whatever is left over goes on the next tick's drain.
const RTP_DISCARD_READS: usize = 1024;
/// Frame 18 carries no traffic, so the downlink drains 17 blocks per multiframe.
const TDMA_CONTROL_FRAME: u8 = 18;
/// RFC 3261 timer H: how long a cancelled INVITE may wait for its final response before we
/// give up on it and free the RTP port.
const CANCEL_REAP_AFTER: Duration = Duration::from_secs(32);
/// Silence longer than this counts as a new talkspurt, so the next RTP packet gets a marker.
const TALKSPURT_GAP: Duration = Duration::from_millis(100);
/// Duration we advertise in a DTMF INFO body. CMCE drops the tone-end U-INFO upstream, so we
/// never learn the real key-press length and just quote a sane fixed value (FH-BUG-080).
const DTMF_INFO_DURATION_MS: u32 = 250;

#[derive(Clone, Debug)]
struct DigestChallenge {
    realm: String,
    nonce: String,
    qop: Option<String>,
    opaque: Option<String>,
    algorithm: Option<String>,
    proxy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DialogState {
    Inviting,
    Ringing,
    /// CANCEL sent, still waiting for the INVITE's final response (487, or a 2xx that raced us).
    Cancelling,
    Established,
    Released,
}

struct RtpSession {
    socket: UdpSocket,
    local_port: u16,
    remote: Option<SocketAddr>,
    packetizer: RtpPacketizer,
    dl_backlog: DownlinkBacklog,
    last_tx: Option<Instant>,
}

impl RtpSession {
    /// Throw away whatever the socket is holding. Audio that arrived before playout starts is
    /// already stale by the time TETRA could carry it, and left in the kernel receive buffer it
    /// becomes a permanent head start over the live stream (FH-BUG-074).
    fn discard_pending(&mut self) {
        let mut buf = [0u8; RTP_MAX_DATAGRAM];
        for _ in 0..RTP_DISCARD_READS {
            if self.socket.recv_from(&mut buf).is_err() {
                break;
            }
        }
    }
}

struct SipDialog {
    uuid: Uuid,
    call: NetworkCircuitCall,
    number: String,
    call_id_header: String,
    local_uri: String,
    local_tag: String,
    remote_tag: Option<String>,
    cseq: u32,
    /// Top Via branch of the INVITE. CANCEL and non-2xx ACK belong to that same transaction.
    invite_branch: String,
    /// Contact of the answer, i.e. where in-dialog requests must actually go.
    remote_target: Option<String>,
    auth: Option<DigestChallenge>,
    auth_retry_sent: bool,
    state: DialogState,
    rtp: RtpSession,
    audio: AsteriskAudioTranscoder,
    media_ready: Option<(u16, u16, u8)>,
    inbound: bool,
    request_context: Option<SipRequestContext>,
    released_at: Option<Instant>,
}

#[derive(Debug)]
struct SipMessage {
    start_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl SipMessage {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
        let (head, body) = text.split_once("\n\n").unwrap_or((&text, ""));
        let mut lines = head.lines();
        let start_line = lines.next()?.trim().to_string();
        let mut headers = Vec::new();
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
        Some(Self {
            start_line,
            headers,
            body: body.to_string(),
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn status_code(&self) -> Option<u16> {
        if !self.start_line.starts_with("SIP/2.0 ") {
            return None;
        }
        self.start_line.split_whitespace().nth(1).and_then(|code| code.parse().ok())
    }

    fn method(&self) -> Option<&str> {
        if self.start_line.starts_with("SIP/2.0 ") {
            None
        } else {
            self.start_line.split_whitespace().next()
        }
    }

    fn request_uri(&self) -> Option<&str> {
        if self.start_line.starts_with("SIP/2.0 ") {
            None
        } else {
            self.start_line.split_whitespace().nth(1)
        }
    }

    fn cseq_method(&self) -> Option<&str> {
        self.header("CSeq")?.split_whitespace().nth(1)
    }

    fn call_id(&self) -> Option<&str> {
        self.header("Call-ID")
    }
}

#[derive(Clone)]
struct SipRequestContext {
    via: String,
    from: String,
    to: String,
    call_id: String,
    cseq: String,
    addr: SocketAddr,
}

/// Extra hosts allowed to speak SIP to us. Unresolvable entries are dropped rather than
/// failing startup - a stale allowlist entry must not take the whole bridge down.
fn resolve_allowed_peers(hosts: &[String]) -> Vec<IpAddr> {
    let mut peers = Vec::new();
    for host in hosts {
        if let Ok(ip) = host.parse::<IpAddr>() {
            peers.push(ip);
            continue;
        }
        match (host.as_str(), 0u16).to_socket_addrs() {
            Ok(addrs) => peers.extend(addrs.map(|addr| addr.ip())),
            Err(err) => tracing::warn!("AsteriskEntity: allow_from entry '{}' did not resolve: {}", host, err),
        }
    }
    peers
}

struct TeardownParams<'a> {
    cancel: bool,
    invite_uri: &'a str,
    remote_target: Option<&'a str>,
    via_host: &'a str,
    via_port: u16,
    invite_branch: &'a str,
    fresh_branch: &'a str,
    from_uri: &'a str,
    local_tag: &'a str,
    remote_tag: Option<&'a str>,
    call_id: &'a str,
    invite_cseq: u32,
    contact: &'a str,
}

/// CANCEL and BYE look alike but belong to different transactions, and getting that wrong is
/// FH-BUG-071: a CANCEL is part of the INVITE transaction (RFC 3261 §9.1), so Request-URI,
/// To (still untagged, exactly as sent), Call-ID, From and the CSeq number must be identical
/// to the INVITE's and the top Via branch must be the INVITE's branch - otherwise the proxy
/// cannot match it and never stops ringing the callee. A BYE is a brand new transaction in an
/// established dialog: fresh branch, next CSeq, negotiated To tag, sent to the remote target.
fn build_teardown_request(p: &TeardownParams) -> String {
    let method = if p.cancel { "CANCEL" } else { "BYE" };
    let branch = if p.cancel { p.invite_branch } else { p.fresh_branch };
    let cseq = if p.cancel { p.invite_cseq } else { p.invite_cseq.saturating_add(1) };
    let request_uri = if p.cancel {
        p.invite_uri
    } else {
        p.remote_target.unwrap_or(p.invite_uri)
    };
    let to = match (p.cancel, p.remote_tag) {
        (false, Some(tag)) => format!("<{}>;tag={}", p.invite_uri, tag),
        _ => format!("<{}>", p.invite_uri),
    };
    // A CANCEL is not a dialog-forming request, so it carries no Contact.
    let contact_line = if p.cancel {
        String::new()
    } else {
        format!("Contact: <{}>\r\n", p.contact)
    };
    format!(
        "{} {} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <{}>;tag={}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {} {}\r\n\
         {}\
         Content-Length: 0\r\n\r\n",
        method, request_uri, p.via_host, p.via_port, branch, p.from_uri, p.local_tag, to, p.call_id, cseq, method, contact_line
    )
}

/// Map one DTMF byte received from CMCE to its ASCII symbol. CMCE already decodes the U-INFO
/// DTMF field to ASCII digit chars (cc_bs/dtmf.rs) and forwards one NetworkCircuitDtmf per digit
/// with `data = [ascii]` (cc_bs/procedures/uplink.rs), so this is really a sanity filter: relay
/// the byte only if it is a legal DTMF symbol (0-9, *, #, A-D), normalising A-D to upper case.
fn dtmf_digit_from_byte(byte: u8) -> Option<char> {
    match byte {
        b'0'..=b'9' | b'*' | b'#' => Some(char::from(byte)),
        b'A'..=b'D' => Some(char::from(byte)),
        b'a'..=b'd' => Some(char::from(byte.to_ascii_uppercase())),
        _ => None,
    }
}

struct DtmfInfoParams<'a> {
    /// Peer address-of-record, used in the To header and as the fallback Request-URI.
    request_uri: &'a str,
    /// Where the request actually goes: the Contact the answer advertised.
    remote_target: Option<&'a str>,
    via_host: &'a str,
    via_port: u16,
    branch: &'a str,
    from_uri: &'a str,
    local_tag: &'a str,
    remote_tag: Option<&'a str>,
    call_id: &'a str,
    cseq: u32,
    contact: &'a str,
    digit: char,
    duration_ms: u32,
}

/// FH-BUG-080: relay a subscriber's DTMF digit to the SIP peer as an in-dialog INFO carrying an
/// `application/dtmf-relay` body. INFO needs no SDP negotiation (unlike RFC 2833 telephone-event,
/// which this bridge never negotiates), so FreePBX/Asterisk accept it on any established call.
/// Like the BYE it is a fresh transaction inside the dialog: new branch, next CSeq, the
/// negotiated To tag, aimed at the remote target.
fn build_dtmf_info(p: &DtmfInfoParams) -> String {
    let request_uri = p.remote_target.unwrap_or(p.request_uri);
    let to = match p.remote_tag {
        Some(tag) => format!("<{}>;tag={}", p.request_uri, tag),
        None => format!("<{}>", p.request_uri),
    };
    let body = format!("Signal={}\r\nDuration={}\r\n", p.digit, p.duration_ms);
    format!(
        "INFO {} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <{}>;tag={}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {} INFO\r\n\
         Contact: <{}>\r\n\
         Content-Type: application/dtmf-relay\r\n\
         Content-Length: {}\r\n\r\n\
         {}",
        request_uri, p.via_host, p.via_port, p.branch, p.from_uri, p.local_tag, to, p.call_id, p.cseq, p.contact, body.len(), body
    )
}

pub struct AsteriskEntity {
    config: SharedConfig,
    asterisk_config: CfgAsterisk,
    sip_socket: UdpSocket,
    remote: SocketAddr,
    allowed_peers: Vec<IpAddr>,
    invite_rate: HashMap<IpAddr, (Instant, u32)>,
    dialogs: HashMap<Uuid, SipDialog>,
    rtp_by_ts: HashMap<(u16, u8), Uuid>,
    next_rtp_port: u16,
    branch_counter: u64,
    register_call_id: String,
    register_cseq: u32,
    register_auth: Option<DigestChallenge>,
    register_status: String,
    last_register: Option<Instant>,
    last_options: Option<Instant>,
    last_rx: Option<String>,
    last_tx: Option<String>,
    last_error: Option<String>,
}

impl AsteriskEntity {
    pub fn new(config: SharedConfig) -> io::Result<Self> {
        let asterisk_config = config.config().asterisk.clone();
        let bind = format!("{}:{}", asterisk_config.bind_addr, asterisk_config.bind_port);
        let sip_socket = UdpSocket::bind(bind)?;
        sip_socket.set_nonblocking(true)?;

        let remote = (asterisk_config.remote_host.as_str(), asterisk_config.remote_port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "asterisk remote address did not resolve"))?;

        let allowed_peers = resolve_allowed_peers(&asterisk_config.allow_from);

        let entity = Self {
            config,
            next_rtp_port: asterisk_config.rtp_port_min,
            register_call_id: format!("flow-reg-{}@{}", Uuid::new_v4(), asterisk_config.contact_host),
            asterisk_config,
            sip_socket,
            remote,
            allowed_peers,
            invite_rate: HashMap::new(),
            dialogs: HashMap::new(),
            rtp_by_ts: HashMap::new(),
            branch_counter: 1,
            register_cseq: 1,
            register_auth: None,
            register_status: "not registered".to_string(),
            last_register: None,
            last_options: None,
            last_rx: None,
            last_tx: None,
            last_error: None,
        };
        entity.refresh_status();
        Ok(entity)
    }

    fn sip_listen(&self) -> String {
        format!("{}:{}", self.asterisk_config.bind_addr, self.asterisk_config.bind_port)
    }

    fn remote_display(&self) -> String {
        format!("{}:{}", self.asterisk_config.remote_host, self.asterisk_config.remote_port)
    }

    fn rtp_range(&self) -> String {
        format!("{}-{}", self.asterisk_config.rtp_port_min, self.asterisk_config.rtp_port_max)
    }

    fn refresh_status(&self) {
        let mut state = self.config.state_write();
        state.asterisk_status = AsteriskRuntimeStatus {
            configured: true,
            enabled: self.asterisk_config.enabled,
            register_status: self.register_status.clone(),
            sip_listen: self.sip_listen(),
            remote: self.remote_display(),
            rtp_port_range: self.rtp_range(),
            codec: self.asterisk_config.codec.clone(),
            active_dialogs: self
                .dialogs
                .values()
                .filter(|d| !matches!(d.state, DialogState::Released | DialogState::Cancelling))
                .count(),
            last_rx: self.last_rx.clone(),
            last_tx: self.last_tx.clone(),
            last_error: self.last_error.clone(),
        };
    }

    fn set_error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::warn!("AsteriskEntity: {}", msg);
        self.last_error = Some(msg);
    }

    fn next_branch(&mut self) -> String {
        let branch = format!("z9hG4bKflow{:08x}", self.branch_counter);
        self.branch_counter = self.branch_counter.wrapping_add(1);
        branch
    }

    fn local_uri(&self) -> String {
        format!("sip:{}@{}", self.asterisk_config.local_user, self.asterisk_config.from_domain)
    }

    fn uri_for_user(&self, user: &str) -> String {
        format!("sip:{}@{}", user, self.asterisk_config.from_domain)
    }

    fn asserted_identity_headers(&self, uri: &str, display: &str) -> String {
        if display.is_empty() {
            return String::new();
        }
        format!(
            "P-Asserted-Identity: \"{}\" <{}>\r\nRemote-Party-ID: \"{}\" <{}>;party=calling;screen=yes;privacy=off\r\n",
            display, uri, display, uri
        )
    }

    fn contact_uri(&self) -> String {
        format!(
            "sip:{}@{}:{}",
            self.asterisk_config.local_user, self.asterisk_config.contact_host, self.asterisk_config.bind_port
        )
    }

    fn request_uri(&self, number: &str) -> String {
        format!("sip:{}@{}", number, self.asterisk_config.remote_host)
    }

    fn send_sip(&mut self, payload: String, summary: impl Into<String>) {
        let summary = summary.into();
        match self.sip_socket.send_to(payload.as_bytes(), self.remote) {
            Ok(_) => {
                self.last_tx = Some(summary);
            }
            Err(err) => {
                self.set_error(format!("SIP send failed: {}", err));
            }
        }
    }

    fn send_sip_to(&mut self, payload: String, addr: SocketAddr, summary: impl Into<String>) {
        let summary = summary.into();
        match self.sip_socket.send_to(payload.as_bytes(), addr) {
            Ok(_) => {
                self.last_tx = Some(summary);
            }
            Err(err) => {
                self.set_error(format!("SIP send failed: {}", err));
            }
        }
    }

    fn send_register(&mut self) {
        if !self.asterisk_config.register {
            self.register_status = "disabled".to_string();
            return;
        }

        let uri = format!("sip:{}", self.asterisk_config.remote_host);
        let branch = self.next_branch();
        let cseq = self.register_cseq;
        self.register_cseq = self.register_cseq.saturating_add(1);
        let auth = self
            .register_auth
            .as_ref()
            .map(|challenge| self.authorization_header("REGISTER", &uri, challenge));
        let auth_line = auth.map(|line| format!("{}\r\n", line)).unwrap_or_default();
        let request = format!(
            "REGISTER {} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
             Max-Forwards: 70\r\n\
             From: <{}>;tag=flowreg\r\n\
             To: <{}>\r\n\
             Call-ID: {}\r\n\
             CSeq: {} REGISTER\r\n\
             Contact: <{}>\r\n\
             Expires: 120\r\n\
             {}\
             User-Agent: FlowStation\r\n\
             Content-Length: 0\r\n\r\n",
            uri,
            self.asterisk_config.contact_host,
            self.asterisk_config.bind_port,
            branch,
            self.local_uri(),
            self.local_uri(),
            self.register_call_id,
            cseq,
            self.contact_uri(),
            auth_line
        );
        self.register_status = "registering".to_string();
        self.last_register = Some(Instant::now());
        self.send_sip(request, "REGISTER");
    }

    fn send_options(&mut self) {
        let uri = format!("sip:{}", self.asterisk_config.remote_host);
        let branch = self.next_branch();
        let request = format!(
            "OPTIONS {} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
             Max-Forwards: 70\r\n\
             From: <{}>;tag=flowopt\r\n\
             To: <{}>\r\n\
             Call-ID: flow-options-{}@{}\r\n\
             CSeq: 1 OPTIONS\r\n\
             Contact: <{}>\r\n\
             Accept: application/sdp\r\n\
             Content-Length: 0\r\n\r\n",
            uri,
            self.asterisk_config.contact_host,
            self.asterisk_config.bind_port,
            branch,
            self.local_uri(),
            uri,
            Uuid::new_v4(),
            self.asterisk_config.contact_host,
            self.contact_uri()
        );
        self.last_options = Some(Instant::now());
        self.send_sip(request, "OPTIONS");
    }

    fn build_sdp(&self, rtp_port: u16) -> String {
        format!(
            "v=0\r\n\
             o=flowstation 0 0 IN IP4 {}\r\n\
             s=FlowStation\r\n\
             c=IN IP4 {}\r\n\
             t=0 0\r\n\
             m=audio {} RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=ptime:{}\r\n\
             a=maxptime:{}\r\n\
             a=sendrecv\r\n",
            self.asterisk_config.contact_host,
            self.asterisk_config.contact_host,
            rtp_port,
            self.asterisk_config.ptime_ms,
            self.asterisk_config.ptime_ms
        )
    }

    fn build_invite(&mut self, uuid: Uuid) -> Option<String> {
        let snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog)?;
        let (rtp_port, auth) = self.dialogs.get(&uuid).map(|dialog| (dialog.rtp.local_port, dialog.auth.clone()))?;
        let request_uri = self.request_uri(&snapshot.number);
        let branch = self.next_branch();
        // CANCEL and any non-2xx ACK have to run in this very transaction, so remember the branch.
        if let Some(dialog) = self.dialogs.get_mut(&uuid) {
            dialog.invite_branch = branch.clone();
        }
        let body = self.build_sdp(rtp_port);
        let auth = auth
            .as_ref()
            .map(|challenge| self.authorization_header("INVITE", &request_uri, challenge));
        let auth_line = auth.map(|line| format!("{}\r\n", line)).unwrap_or_default();
        let to_uri = request_uri.clone();
        let from_uri = snapshot.local_uri.clone();
        let caller_id = snapshot
            .source_issi
            .filter(|source| *source != 0)
            .map(|source| source.to_string())
            .unwrap_or_else(|| self.asterisk_config.local_user.clone());
        let identity_uri = snapshot
            .source_issi
            .filter(|source| *source != 0)
            .map(|source| self.uri_for_user(&source.to_string()))
            .unwrap_or_else(|| from_uri.clone());
        let identity_headers = self.asserted_identity_headers(&identity_uri, &caller_id);
        Some(format!(
            "INVITE {} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
             Max-Forwards: 70\r\n\
             From: \"{}\" <{}>;tag={}\r\n\
             To: <{}>\r\n\
             Call-ID: {}\r\n\
             CSeq: {} INVITE\r\n\
             Contact: <{}>\r\n\
             Allow: INVITE, ACK, CANCEL, OPTIONS, BYE, INFO\r\n\
             Supported: replaces\r\n\
             {}\
             {}\
             Content-Type: application/sdp\r\n\
             Content-Length: {}\r\n\r\n{}",
            request_uri,
            self.asterisk_config.contact_host,
            self.asterisk_config.bind_port,
            branch,
            caller_id,
            from_uri,
            snapshot.local_tag,
            to_uri,
            snapshot.call_id_header,
            snapshot.cseq,
            self.contact_uri(),
            identity_headers,
            auth_line,
            body.as_bytes().len(),
            body
        ))
    }

    fn send_invite(&mut self, uuid: Uuid) {
        if let Some(request) = self.build_invite(uuid) {
            self.send_sip(request, format!("INVITE {}", uuid));
        }
    }

    fn send_bye_or_cancel(&mut self, uuid: Uuid, cancel: bool) {
        let Some(dialog) = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog) else {
            return;
        };
        let fresh_branch = self.next_branch();
        let request = build_teardown_request(&TeardownParams {
            cancel,
            invite_uri: &self.request_uri(&dialog.number),
            remote_target: dialog.remote_target.as_deref(),
            via_host: &self.asterisk_config.contact_host,
            via_port: self.asterisk_config.bind_port,
            invite_branch: &dialog.invite_branch,
            fresh_branch: &fresh_branch,
            from_uri: &dialog.local_uri,
            local_tag: &dialog.local_tag,
            remote_tag: dialog.remote_tag.as_deref(),
            call_id: &dialog.call_id_header,
            invite_cseq: dialog.cseq,
            contact: &self.contact_uri(),
        });
        let method = if cancel { "CANCEL" } else { "BYE" };
        // Inbound dialogs answer back to whoever sent us the INVITE, not to the configured peer.
        match dialog.peer_addr {
            Some(addr) => self.send_sip_to(request, addr, format!("{} {}", method, uuid)),
            None => self.send_sip(request, format!("{} {}", method, uuid)),
        }
    }

    /// Relay one decoded DTMF digit to the SIP peer as an in-dialog INFO (FH-BUG-080).
    fn send_dtmf_info(&mut self, uuid: Uuid, digit: char) {
        // Only an established call has a remote party to signal. A digit for a leg that is still
        // ringing, already gone, or an unknown uuid has no dialog to carry it - drop it quietly.
        let established = self
            .dialogs
            .get(&uuid)
            .is_some_and(|dialog| matches!(dialog.state, DialogState::Established));
        if !established {
            tracing::debug!("AsteriskEntity: DTMF '{}' for uuid={} with no established dialog, dropping", digit, uuid);
            return;
        }
        let fresh_branch = self.next_branch();
        // Each INFO is a new transaction inside the dialog, so bump and persist the CSeq: back-to-
        // back digits (and the eventual BYE) must stay strictly increasing (RFC 3261 §12.2.1.1).
        let snapshot = {
            let Some(dialog) = self.dialogs.get_mut(&uuid) else {
                return;
            };
            dialog.cseq = dialog.cseq.saturating_add(1);
            SipDialogSnapshot::from_dialog(dialog)
        };
        let request = build_dtmf_info(&DtmfInfoParams {
            request_uri: &self.request_uri(&snapshot.number),
            remote_target: snapshot.remote_target.as_deref(),
            via_host: &self.asterisk_config.contact_host,
            via_port: self.asterisk_config.bind_port,
            branch: &fresh_branch,
            from_uri: &snapshot.local_uri,
            local_tag: &snapshot.local_tag,
            remote_tag: snapshot.remote_tag.as_deref(),
            call_id: &snapshot.call_id_header,
            cseq: snapshot.cseq,
            contact: &self.contact_uri(),
            digit,
            duration_ms: DTMF_INFO_DURATION_MS,
        });
        // Inbound dialogs answer back to whoever sent us the INVITE, not to the configured peer.
        match snapshot.peer_addr {
            Some(addr) => self.send_sip_to(request, addr, format!("INFO DTMF {} {}", digit, uuid)),
            None => self.send_sip(request, format!("INFO DTMF {} {}", digit, uuid)),
        }
    }

    fn tagged_to(to: &str, tag: Option<&str>) -> String {
        let mut to = to.to_string();
        if let Some(tag) = tag
            && !to.to_ascii_lowercase().contains(";tag=")
        {
            to.push_str(";tag=");
            to.push_str(tag);
        }
        to
    }

    fn build_response(&self, ctx: &SipRequestContext, code: u16, reason: &str, to_tag: Option<&str>, body: Option<(&str, &str)>) -> String {
        let to = Self::tagged_to(&ctx.to, to_tag);
        let (content_type, body_text) = body.unwrap_or(("", ""));
        let content_type_line = if content_type.is_empty() {
            String::new()
        } else {
            format!("Content-Type: {}\r\n", content_type)
        };
        let contact_line = if code >= 180 {
            format!(
                "Contact: <{}>\r\nAllow: INVITE, ACK, CANCEL, OPTIONS, BYE, INFO\r\n",
                self.contact_uri()
            )
        } else {
            String::new()
        };
        format!(
            "SIP/2.0 {} {}\r\n\
             Via: {}\r\n\
             From: {}\r\n\
             To: {}\r\n\
             Call-ID: {}\r\n\
             CSeq: {}\r\n\
             {}\
             {}\
             Content-Length: {}\r\n\r\n{}",
            code,
            reason,
            ctx.via,
            ctx.from,
            to,
            ctx.call_id,
            ctx.cseq,
            contact_line,
            content_type_line,
            body_text.as_bytes().len(),
            body_text
        )
    }

    fn request_context(msg: &SipMessage, addr: SocketAddr) -> Option<SipRequestContext> {
        Some(SipRequestContext {
            via: msg.header("Via")?.to_string(),
            from: msg.header("From")?.to_string(),
            to: msg.header("To")?.to_string(),
            call_id: msg.header("Call-ID")?.to_string(),
            cseq: msg.header("CSeq")?.to_string(),
            addr,
        })
    }

    fn answer_request(&mut self, msg: &SipMessage, addr: SocketAddr, code: u16, reason: &str) {
        let Some(ctx) = Self::request_context(msg, addr) else {
            return;
        };
        let tag = (code != 100).then_some("flowstation");
        let response = self.build_response(&ctx, code, reason, tag, None);
        self.send_sip_to(response, addr, format!("{} {}", code, reason));
    }

    fn send_invite_response(&mut self, uuid: Uuid, code: u16, reason: &str, body: Option<String>) {
        let Some((ctx, tag)) = self
            .dialogs
            .get(&uuid)
            .and_then(|dialog| dialog.request_context.clone().map(|ctx| (ctx, dialog.local_tag.clone())))
        else {
            return;
        };
        let body_ref = body.as_deref().map(|b| ("application/sdp", b));
        let response = self.build_response(&ctx, code, reason, Some(&tag), body_ref);
        self.send_sip_to(response, ctx.addr, format!("{} {} {}", code, reason, uuid));
    }

    fn authorization_header(&self, method: &str, uri: &str, challenge: &DigestChallenge) -> String {
        let username = &self.asterisk_config.auth_user;
        let password = self.asterisk_config.password.as_ref();
        let realm = if challenge.realm.is_empty() {
            &self.asterisk_config.realm
        } else {
            &challenge.realm
        };
        let ha1 = format!("{:x}", md5::compute(format!("{}:{}:{}", username, realm, password)));
        let ha2 = format!("{:x}", md5::compute(format!("{}:{}", method, uri)));
        let cnonce = format!("{:x}", md5::compute(Uuid::new_v4().as_bytes()));
        let nc = "00000001";
        let response = if let Some(qop) = challenge.qop.as_deref() {
            let qop_token = qop.split(',').map(str::trim).find(|v| *v == "auth").unwrap_or(qop);
            format!(
                "{:x}",
                md5::compute(format!("{}:{}:{}:{}:{}:{}", ha1, challenge.nonce, nc, cnonce, qop_token, ha2))
            )
        } else {
            format!("{:x}", md5::compute(format!("{}:{}:{}", ha1, challenge.nonce, ha2)))
        };
        let header_name = if challenge.proxy { "Proxy-Authorization" } else { "Authorization" };
        let mut line = format!(
            "{}: Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            header_name, username, realm, challenge.nonce, uri, response
        );
        if let Some(qop) = challenge.qop.as_deref() {
            let qop_token = qop.split(',').map(str::trim).find(|v| *v == "auth").unwrap_or(qop);
            line.push_str(&format!(", qop={}, nc={}, cnonce=\"{}\"", qop_token, nc, cnonce));
        }
        if let Some(opaque) = challenge.opaque.as_deref() {
            line.push_str(&format!(", opaque=\"{}\"", opaque));
        }
        if let Some(algorithm) = challenge.algorithm.as_deref() {
            line.push_str(&format!(", algorithm={}", algorithm));
        }
        line
    }

    fn parse_challenge(msg: &SipMessage) -> Option<DigestChallenge> {
        let (header, proxy) = msg
            .header("WWW-Authenticate")
            .map(|h| (h, false))
            .or_else(|| msg.header("Proxy-Authenticate").map(|h| (h, true)))?;
        let mut value = header.trim();
        if value.to_ascii_lowercase().starts_with("digest") {
            value = value[6..].trim();
        }
        let mut params = HashMap::new();
        for part in value.split(',') {
            let Some((key, val)) = part.trim().split_once('=') else {
                continue;
            };
            params.insert(key.trim().to_ascii_lowercase(), val.trim().trim_matches('"').to_string());
        }
        Some(DigestChallenge {
            realm: params.remove("realm").unwrap_or_default(),
            nonce: params.remove("nonce")?,
            qop: params.remove("qop"),
            opaque: params.remove("opaque"),
            algorithm: params.remove("algorithm"),
            proxy,
        })
    }

    fn parse_to_tag(header: Option<&str>) -> Option<String> {
        header?.split(';').find_map(|part| {
            let part = part.trim();
            part.strip_prefix("tag=").map(|tag| tag.trim_matches('"').to_string())
        })
    }

    /// Remote target for in-dialog requests: the bare URI inside the peer's Contact header.
    fn parse_contact_uri(header: Option<&str>) -> Option<String> {
        let value = header?.trim();
        let uri = if let Some(start) = value.find('<') {
            let rest = &value[start + 1..];
            rest.split_once('>')?.0
        } else {
            value.split(';').next()?.trim()
        };
        (!uri.is_empty() && uri.to_ascii_lowercase().starts_with("sip")).then(|| uri.to_string())
    }

    fn sip_uri_user(value: &str) -> Option<String> {
        let trimmed = value.trim();
        let after_scheme = if let Some(idx) = trimmed.to_ascii_lowercase().find("sip:") {
            &trimmed[idx + 4..]
        } else {
            trimmed
        };
        let user = after_scheme
            .split(|c| matches!(c, '@' | ';' | '?' | '>'))
            .next()?
            .trim()
            .trim_matches('"');
        (!user.is_empty()).then(|| user.to_string())
    }

    fn inbound_destination_issi(&self, msg: &SipMessage) -> Option<u32> {
        let prefix = self.asterisk_config.inbound_prefix.trim();
        [msg.request_uri(), msg.header("To")]
            .into_iter()
            .flatten()
            .filter_map(Self::sip_uri_user)
            .find_map(|user| {
                let digits = if !prefix.is_empty() {
                    user.strip_prefix(prefix).unwrap_or(&user)
                } else {
                    user.as_str()
                };
                digits
                    .chars()
                    .all(|c| c.is_ascii_digit())
                    .then(|| digits.parse::<u32>().ok())
                    .flatten()
            })
    }

    fn inbound_caller_number(msg: &SipMessage) -> String {
        ["P-Asserted-Identity", "Remote-Party-ID", "From"]
            .into_iter()
            .filter_map(|header| msg.header(header))
            .filter_map(Self::sip_uri_user)
            .next()
            .unwrap_or_else(|| "0".to_string())
    }

    fn parse_sdp_remote(&self, body: &str) -> Option<SocketAddr> {
        let mut ip: Option<IpAddr> = None;
        let mut port: Option<u16> = None;
        for line in body.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("c=IN IP4 ") {
                ip = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            }
            if let Some(rest) = line.strip_prefix("m=audio ") {
                port = rest.split_whitespace().next().and_then(|s| s.parse().ok());
            }
        }
        Some(SocketAddr::new(ip.unwrap_or_else(|| self.remote.ip()), port?))
    }

    fn allocate_rtp(&mut self) -> io::Result<RtpSession> {
        let min = self.asterisk_config.rtp_port_min;
        let max = self.asterisk_config.rtp_port_max;
        let mut port = self.next_rtp_port.max(min);
        let attempts = max.saturating_sub(min).saturating_add(1);
        for _ in 0..attempts {
            if port > max {
                port = min;
            }
            let bind = format!("{}:{}", self.asterisk_config.bind_addr, port);
            match UdpSocket::bind(&bind) {
                Ok(socket) => {
                    socket.set_nonblocking(true)?;
                    self.next_rtp_port = if port == max { min } else { port + 1 };
                    let seed = md5::compute(Uuid::new_v4().as_bytes()).0;
                    let ssrc = u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]);
                    return Ok(RtpSession {
                        socket,
                        local_port: port,
                        remote: None,
                        packetizer: RtpPacketizer::new(ssrc, self.asterisk_config.ptime_ms),
                        dl_backlog: DownlinkBacklog::new(self.asterisk_config.dl_jitter_ms),
                        last_tx: None,
                    });
                }
                Err(_) => {
                    port = port.saturating_add(1);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "no RTP port available"))
    }

    fn peer_allowed(&self, addr: SocketAddr) -> bool {
        // Port is not pinned: Asterisk answers from whatever source port it likes.
        addr.ip() == self.remote.ip() || self.allowed_peers.contains(&addr.ip())
    }

    /// Sliding-ish window per source. Each INVITE costs an RTP port, a codec and a CMCE setup,
    /// so an unthrottled peer can exhaust the port pool and mass-ring the cell.
    fn invite_rate_ok(&mut self, addr: SocketAddr) -> bool {
        let limit = self.asterisk_config.max_invites_per_minute;
        if limit == 0 {
            return true;
        }
        let now = Instant::now();
        let entry = self.invite_rate.entry(addr.ip()).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(60) {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
        entry.1 <= limit
    }

    fn pending_dialogs(&self) -> usize {
        self.dialogs
            .values()
            .filter(|d| matches!(d.state, DialogState::Inviting | DialogState::Ringing))
            .count()
    }

    fn start_inbound_call(&mut self, queue: &mut MessageQueue, msg: &SipMessage, addr: SocketAddr) {
        let Some(ctx) = Self::request_context(msg, addr) else {
            return;
        };
        if !self.invite_rate_ok(addr) {
            tracing::warn!("AsteriskEntity: INVITE rate limit hit for {}, rejecting", addr);
            let response = self.build_response(&ctx, 503, "Service Unavailable", Some("flowstation"), None);
            self.send_sip_to(response, addr, "503 Service Unavailable");
            return;
        }
        if self.pending_dialogs() >= self.asterisk_config.max_pending_dialogs {
            tracing::warn!("AsteriskEntity: too many pending dialogs, rejecting INVITE from {}", addr);
            let response = self.build_response(&ctx, 486, "Busy Here", Some("flowstation"), None);
            self.send_sip_to(response, addr, "486 Busy Here");
            return;
        }
        let Some(destination) = self.inbound_destination_issi(msg) else {
            tracing::info!(
                "AsteriskEntity: rejecting inbound INVITE without TETRA destination: {}",
                msg.start_line
            );
            let response = self.build_response(&ctx, 404, "Not Found", Some("flowstation"), None);
            self.send_sip_to(response, addr, "404 Not Found");
            return;
        };
        if self.find_dialog_by_call_id(Some(ctx.call_id.as_str())).is_some() {
            tracing::info!("AsteriskEntity: rejecting duplicate inbound INVITE call-id={}", ctx.call_id);
            let response = self.build_response(&ctx, 486, "Busy Here", Some("flowstation"), None);
            self.send_sip_to(response, addr, "486 Busy Here");
            return;
        }

        let Some(remote_rtp) = self.parse_sdp_remote(&msg.body) else {
            tracing::info!("AsteriskEntity: rejecting inbound INVITE to {} without usable SDP", destination);
            let response = self.build_response(&ctx, 488, "Not Acceptable Here", Some("flowstation"), None);
            self.send_sip_to(response, addr, "488 Not Acceptable Here");
            return;
        };

        let mut rtp = match self.allocate_rtp() {
            Ok(rtp) => rtp,
            Err(err) => {
                self.set_error(format!("RTP allocation failed for inbound INVITE to {}: {}", destination, err));
                let response = self.build_response(&ctx, 503, "Service Unavailable", Some("flowstation"), None);
                self.send_sip_to(response, addr, "503 Service Unavailable");
                return;
            }
        };
        rtp.remote = Some(remote_rtp);
        let Some(audio) = AsteriskAudioTranscoder::new() else {
            self.set_error("TETRA codec allocation failed for inbound Asterisk call".to_string());
            let response = self.build_response(&ctx, 503, "Service Unavailable", Some("flowstation"), None);
            self.send_sip_to(response, addr, "503 Service Unavailable");
            return;
        };

        let uuid = Uuid::new_v4();
        let caller_number = Self::inbound_caller_number(msg);
        let call = NetworkCircuitCall {
            source_issi: 0,
            destination,
            number: caller_number.clone(),
            priority: 0,
            service: 0,
            mode: 0,
            duplex: 1,
            method: 0,
            communication: 0,
            grant: 0,
            permission: 0,
            timeout: CallTimeout::Infinite.into_raw() as u8,
            ownership: 0,
            queued: 0,
        };
        let local_tag = format!("flow{}", &uuid.to_string()[..8]);
        let remote_tag = Self::parse_to_tag(msg.header("From"));
        let dialog = SipDialog {
            uuid,
            call: call.clone(),
            number: caller_number,
            call_id_header: ctx.call_id.clone(),
            local_uri: self.local_uri(),
            local_tag,
            remote_tag,
            cseq: 1,
            invite_branch: String::new(),
            remote_target: Self::parse_contact_uri(msg.header("Contact")),
            auth: None,
            auth_retry_sent: false,
            state: DialogState::Inviting,
            rtp,
            audio,
            media_ready: None,
            inbound: true,
            request_context: Some(ctx.clone()),
            released_at: None,
        };
        self.dialogs.insert(uuid, dialog);

        let response = self.build_response(&ctx, 100, "Trying", None, None);
        self.send_sip_to(response, addr, format!("100 Trying {}", uuid));
        tracing::info!(
            "AsteriskEntity: inbound INVITE uuid={} caller='{}' -> ISSI {}",
            uuid,
            Self::inbound_caller_number(msg),
            destination
        );
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid: uuid, call }),
        });
    }

    fn handle_inbound_setup_accept(&mut self, uuid: Uuid) {
        if self.dialogs.get(&uuid).is_some_and(|dialog| dialog.inbound) {
            tracing::info!("AsteriskEntity: inbound setup accepted by CMCE uuid={}", uuid);
        }
    }

    fn handle_inbound_setup_reject(&mut self, uuid: Uuid, cause: u8) {
        let Some(inbound) = self.dialogs.get(&uuid).map(|dialog| dialog.inbound) else {
            return;
        };
        if !inbound {
            return;
        }
        let (code, reason) = match cause {
            2 => (486, "Busy Here"),
            3 => (404, "Not Found"),
            5 => (503, "Service Unavailable"),
            _ => (480, "Temporarily Unavailable"),
        };
        tracing::info!(
            "AsteriskEntity: inbound setup rejected by CMCE uuid={} cause={} -> SIP {} {}",
            uuid,
            cause,
            code,
            reason
        );
        self.send_invite_response(uuid, code, reason, None);
        self.release_dialog(uuid, false);
    }

    fn handle_inbound_alert(&mut self, uuid: Uuid) {
        let Some(dialog) = self.dialogs.get_mut(&uuid) else {
            return;
        };
        if !dialog.inbound {
            return;
        }
        dialog.state = DialogState::Ringing;
        tracing::info!("AsteriskEntity: inbound call ringing uuid={}", uuid);
        self.send_invite_response(uuid, 180, "Ringing", None);
    }

    fn handle_inbound_connect_request(&mut self, queue: &mut MessageQueue, uuid: Uuid, call: NetworkCircuitCall) {
        let Some((inbound, rtp_port)) = self.dialogs.get(&uuid).map(|dialog| (dialog.inbound, dialog.rtp.local_port)) else {
            return;
        };
        if !inbound {
            return;
        }

        let body = self.build_sdp(rtp_port);
        if let Some(dialog) = self.dialogs.get_mut(&uuid) {
            dialog.call = call;
            dialog.state = DialogState::Established;
        }
        tracing::info!("AsteriskEntity: inbound call answered uuid={} -> SIP 200 OK", uuid);
        self.send_invite_response(uuid, 200, "OK", Some(body));
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm {
                brew_uuid: uuid,
                grant: 0,
                permission: 0,
            }),
        });
    }

    fn start_outbound_call(&mut self, queue: &mut MessageQueue, brew_uuid: Uuid, call: NetworkCircuitCall) {
        let number = call.number.trim().to_string();
        if number.is_empty() {
            self.set_error(format!("empty Asterisk destination for uuid={}", brew_uuid));
            self.reject_setup(queue, brew_uuid, 34);
            return;
        }
        let rtp = match self.allocate_rtp() {
            Ok(rtp) => rtp,
            Err(err) => {
                self.set_error(format!("RTP allocation failed for uuid={}: {}", brew_uuid, err));
                self.reject_setup(queue, brew_uuid, 34);
                return;
            }
        };
        let Some(audio) = AsteriskAudioTranscoder::new() else {
            self.set_error(format!("TETRA codec allocation failed for uuid={}", brew_uuid));
            self.reject_setup(queue, brew_uuid, 34);
            return;
        };

        let dialog = SipDialog {
            uuid: brew_uuid,
            local_uri: self.local_uri(),
            call,
            number,
            call_id_header: format!("flow-{}@{}", brew_uuid, self.asterisk_config.contact_host),
            local_tag: format!("flow{}", &brew_uuid.to_string()[..8]),
            remote_tag: None,
            cseq: 1,
            invite_branch: String::new(),
            remote_target: None,
            auth: None,
            auth_retry_sent: false,
            state: DialogState::Inviting,
            rtp,
            audio,
            media_ready: None,
            inbound: false,
            request_context: None,
            released_at: None,
        };
        self.dialogs.insert(brew_uuid, dialog);
        self.send_setup_accept(queue, brew_uuid);
        self.send_invite(brew_uuid);
    }

    fn reject_setup(&self, queue: &mut MessageQueue, brew_uuid: Uuid, cause: u8) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }),
        });
    }

    fn send_setup_accept(&self, queue: &mut MessageQueue, brew_uuid: Uuid) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }),
        });
    }

    fn send_alert(&self, queue: &mut MessageQueue, brew_uuid: Uuid) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }),
        });
    }

    fn send_release_to_cmce(&self, queue: &mut MessageQueue, brew_uuid: Uuid, cause: u8) {
        queue.push_back(SapMsg {
            sap: Sap::Control,
            src: TetraEntity::Asterisk,
            dest: TetraEntity::Cmce,
            msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, cause }),
        });
    }

    fn mark_media_ready(&mut self, brew_uuid: Uuid, call_id: u16, carrier_num: u16, ts: u8) {
        if let Some(dialog) = self.dialogs.get_mut(&brew_uuid) {
            // Playout starts now, not seconds ago: drop anything that arrived while the circuit
            // was still coming up, plus the part-filled block behind it (FH-BUG-074).
            dialog.rtp.discard_pending();
            dialog.rtp.dl_backlog.clear();
            dialog.audio.reset_downlink();
            dialog.media_ready = Some((call_id, carrier_num, ts));
            self.rtp_by_ts.insert((carrier_num, ts), brew_uuid);
            tracing::info!(
                "AsteriskEntity: media ready uuid={} call_id={} carrier={} ts={}",
                brew_uuid,
                call_id,
                carrier_num,
                ts
            );
        }
    }

    fn release_dialog(&mut self, brew_uuid: Uuid, from_cmce: bool) {
        let Some((cancel, inbound)) = self
            .dialogs
            .get(&brew_uuid)
            .map(|dialog| (!matches!(dialog.state, DialogState::Established), dialog.inbound))
        else {
            return;
        };
        if from_cmce {
            if inbound && cancel {
                self.send_invite_response(brew_uuid, 480, "Temporarily Unavailable", None);
            } else {
                self.send_bye_or_cancel(brew_uuid, cancel);
            }
        }
        // An outbound INVITE we just CANCELled is still alive at the far end until its final
        // response arrives. Forgetting the dialog here loses that response - and if the callee
        // answers in the meantime, the 200 OK matches nothing, never gets ACKed and the phone
        // stays up forever (FH-BUG-071). Keep it until the INVITE transaction really ends.
        if from_cmce && cancel && !inbound {
            if let Some((_, carrier_num, ts)) = self.dialogs.get(&brew_uuid).and_then(|d| d.media_ready) {
                self.rtp_by_ts.remove(&(carrier_num, ts));
            }
            if let Some(dialog) = self.dialogs.get_mut(&brew_uuid) {
                dialog.media_ready = None;
                dialog.state = DialogState::Cancelling;
                dialog.released_at = Some(Instant::now());
            }
            return;
        }
        self.drop_dialog(brew_uuid);
    }

    fn drop_dialog(&mut self, brew_uuid: Uuid) {
        if let Some(dialog) = self.dialogs.get_mut(&brew_uuid) {
            dialog.state = DialogState::Released;
            if let Some((_, carrier_num, ts)) = dialog.media_ready.take() {
                self.rtp_by_ts.remove(&(carrier_num, ts));
            }
        }
        self.dialogs.remove(&brew_uuid);
    }

    fn handle_ul_voice(&mut self, prim: TmdCircuitDataInd) {
        let Some(uuid) = self.rtp_by_ts.get(&(prim.carrier_num, prim.ts)).copied() else {
            return;
        };
        let mut send_error: Option<io::Error> = None;
        let mut drop_reason = None;
        'send: {
            let Some(dialog) = self.dialogs.get_mut(&uuid) else {
                return;
            };
            let Some(remote) = dialog.rtp.remote else {
                return;
            };
            let Some(payload) = dialog.audio.decode_tmd_to_pcmu(&prim.data) else {
                drop_reason = Some(format!(
                    "AsteriskEntity: dropping unsupported TETRA audio block uuid={} ts={} len={}",
                    uuid,
                    prim.ts,
                    prim.data.len()
                ));
                break 'send;
            };

            // One TETRA block is 60 ms of audio; the packetizer cuts it into ptime-sized RTP
            // packets and keeps whatever is left over for the next block (FH-BUG-074).
            let now = Instant::now();
            if dialog
                .rtp
                .last_tx
                .map(|last| now.duration_since(last) > TALKSPURT_GAP)
                .unwrap_or(true)
            {
                dialog.rtp.packetizer.mark_talkspurt();
            }
            dialog.rtp.last_tx = Some(now);

            for packet in dialog.rtp.packetizer.push(&payload) {
                if let Err(err) = dialog.rtp.socket.send_to(&packet, remote) {
                    send_error = Some(err);
                    break;
                }
            }
        }
        if let Some(reason) = drop_reason {
            self.set_error(reason);
            return;
        }
        if let Some(err) = send_error {
            self.set_error(format!("RTP send failed uuid={} ts={}: {}", uuid, prim.ts, err));
        };
    }

    fn poll_rtp(&mut self, queue: &mut MessageQueue, now: TdmaTime) {
        let mut downlink = Vec::new();
        let mut last_error = None;
        let mut buf = [0u8; RTP_MAX_DATAGRAM];
        for dialog in self.dialogs.values_mut() {
            let Some((_, carrier_num, ts)) = dialog.media_ready else {
                // Asterisk is often already sending - early media, ringback, or just the gap
                // before the circuit is up. Not reading the socket does not stop that audio, it
                // only parks it in the kernel receive buffer to be played out seconds late once
                // the circuit opens (FH-BUG-074), so read it and drop it.
                dialog.rtp.discard_pending();
                continue;
            };
            for _ in 0..RTP_READS_PER_TICK {
                match dialog.rtp.socket.recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        let Some((payload_type, payload)) = rtp_payload(&buf[..len]) else {
                            continue;
                        };
                        if payload_type != PCMU_PAYLOAD_TYPE {
                            tracing::trace!(
                                "AsteriskEntity: dropping unsupported RTP payload type {} uuid={}",
                                payload_type,
                                dialog.uuid
                            );
                            continue;
                        }
                        // Only the SDP-negotiated peer may feed this call. Latching onto any
                        // sender let anyone who found the port inject or steal audio mid-call;
                        // a changed port from the same host is still allowed (symmetric RTP).
                        match dialog.rtp.remote {
                            Some(expected) if expected.ip() == addr.ip() => dialog.rtp.remote = Some(addr),
                            _ => {
                                tracing::trace!("AsteriskEntity: dropping RTP from unexpected source {} uuid={}", addr, dialog.uuid);
                                continue;
                            }
                        }
                        for frame in dialog.audio.encode_pcmu_to_tmd(payload) {
                            let dropped = dialog.rtp.dl_backlog.push(frame);
                            if dropped > 0 {
                                tracing::debug!(
                                    "AsteriskEntity: dropped {} stale downlink block(s) uuid={} ts={}",
                                    dropped,
                                    dialog.uuid,
                                    ts
                                );
                            }
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => {
                        last_error = Some(format!("RTP receive failed uuid={}: {}", dialog.uuid, err));
                        break;
                    }
                }
            }

            // Hand over one block per traffic frame, i.e. exactly what UMAC takes off this
            // timeslot. Pushing any faster only moves the backlog into UMAC's queue, where this
            // entity can no longer trim it and it becomes permanent delay (FH-BUG-074).
            if now.t == ts && now.f != TDMA_CONTROL_FRAME {
                if let Some(data) = dialog.rtp.dl_backlog.take() {
                    downlink.push((carrier_num, ts, data));
                }
            }
        }
        if last_error.is_some() {
            self.last_error = last_error;
        }

        for (carrier_num, ts, data) in downlink {
            queue.push_back(SapMsg {
                sap: Sap::TmdSap,
                src: TetraEntity::Asterisk,
                dest: TetraEntity::Umac,
                msg: SapMsgInner::TmdCircuitDataReq(TmdCircuitDataReq { carrier_num, ts, data }),
            });
        }
    }

    fn poll_sip(&mut self, queue: &mut MessageQueue) {
        let mut buf = [0u8; SIP_MAX_DATAGRAM];
        for _ in 0..32 {
            match self.sip_socket.recv_from(&mut buf) {
                Ok((len, addr)) => {
                    // Anything that reaches this port could otherwise INVITE an arbitrary ISSI
                    // or tear down a dialog by guessing its Call-ID.
                    if !self.peer_allowed(addr) {
                        tracing::debug!("AsteriskEntity: dropping SIP datagram from unexpected source {}", addr);
                        continue;
                    }
                    if let Some(msg) = SipMessage::parse(&buf[..len]) {
                        self.last_rx = Some(format!("{} from {}", msg.start_line, addr));
                        self.handle_sip_message(queue, msg, addr);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) => {
                    self.set_error(format!("SIP receive failed: {}", err));
                    break;
                }
            }
        }
    }

    fn handle_sip_message(&mut self, queue: &mut MessageQueue, msg: SipMessage, addr: SocketAddr) {
        if let Some(method) = msg.method() {
            match method {
                "INVITE" => self.start_inbound_call(queue, &msg, addr),
                "OPTIONS" => self.answer_request(&msg, addr, 200, "OK"),
                "BYE" | "CANCEL" => {
                    self.answer_request(&msg, addr, 200, "OK");
                    if let Some(uuid) = self.find_dialog_by_call_id(msg.call_id()) {
                        // A dialog we already cancelled has been released towards CMCE.
                        if self.dialogs.get(&uuid).is_some_and(|d| d.state != DialogState::Cancelling) {
                            self.send_release_to_cmce(queue, uuid, 16);
                        }
                        self.release_dialog(uuid, false);
                    }
                }
                "ACK" => {}
                _ => self.answer_request(&msg, addr, 501, "Not Implemented"),
            }
            return;
        }

        let Some(code) = msg.status_code() else {
            return;
        };
        match msg.cseq_method() {
            Some("REGISTER") => self.handle_register_response(&msg, code),
            Some("OPTIONS") => {
                if (200..300).contains(&code) {
                    self.last_rx = Some(format!("OPTIONS {} from {}", code, addr));
                }
            }
            Some("INVITE") => self.handle_invite_response(queue, &msg, code),
            Some("BYE") | Some("CANCEL") => {}
            _ => {}
        }
    }

    fn handle_register_response(&mut self, msg: &SipMessage, code: u16) {
        match code {
            200..=299 => {
                self.register_status = "registered".to_string();
            }
            401 | 407 => {
                if let Some(challenge) = Self::parse_challenge(msg) {
                    self.register_auth = Some(challenge);
                    self.register_status = "auth challenge".to_string();
                    self.send_register();
                }
            }
            _ => {
                self.register_status = format!("failed {}", code);
                self.last_error = Some(format!("REGISTER failed with SIP {}", code));
            }
        }
    }

    fn handle_invite_response(&mut self, queue: &mut MessageQueue, msg: &SipMessage, code: u16) {
        let Some(uuid) = self.find_dialog_by_call_id(msg.call_id()) else {
            return;
        };
        // The radio already hung up and we CANCELled; CMCE has been told, so from here on we
        // only have to close the SIP leg down cleanly.
        let cancelling = self.dialogs.get(&uuid).is_some_and(|d| d.state == DialogState::Cancelling);

        match code {
            100 => {}
            180 | 183 => {
                if cancelling {
                    return;
                }
                let remote_rtp = self.parse_sdp_remote(&msg.body);
                if let Some(dialog) = self.dialogs.get_mut(&uuid) {
                    dialog.state = DialogState::Ringing;
                    dialog.remote_tag = Self::parse_to_tag(msg.header("To"));
                    // 183 carries early media; without this the answer SDP is our only source.
                    if let Some(remote_rtp) = remote_rtp {
                        dialog.rtp.remote = Some(remote_rtp);
                    }
                }
                self.send_alert(queue, uuid);
            }
            200..=299 => {
                let remote_rtp = self.parse_sdp_remote(&msg.body);
                let remote_target = Self::parse_contact_uri(msg.header("Contact"));
                let connect_call = {
                    let Some(dialog) = self.dialogs.get_mut(&uuid) else {
                        return;
                    };
                    dialog.remote_tag = Self::parse_to_tag(msg.header("To"));
                    dialog.remote_target = remote_target.or(dialog.remote_target.take());
                    if let Some(remote_rtp) = remote_rtp {
                        dialog.rtp.remote = Some(remote_rtp);
                    }
                    if !cancelling {
                        dialog.state = DialogState::Established;
                    }
                    dialog.call.clone()
                };
                // An ACK to a 2xx is its own transaction, so a fresh branch is correct here.
                // Skipping it makes the far end retransmit the 200 OK forever (FH-BUG-071).
                let ack_snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog);
                if let Some(snapshot) = ack_snapshot {
                    let branch = self.next_branch();
                    let ack_text = self.build_ack(&snapshot, &branch, snapshot.cseq);
                    self.send_sip(ack_text, format!("ACK {}", uuid));
                }
                if cancelling {
                    // The answer raced our CANCEL: the leg is up now, so ACK it and BYE it,
                    // otherwise the callee stays connected with nobody on our side.
                    tracing::info!("AsteriskEntity: 200 OK raced our CANCEL uuid={}, hanging up with BYE", uuid);
                    self.send_bye_or_cancel(uuid, false);
                    self.drop_dialog(uuid);
                    return;
                }
                let connect_snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog);
                if let Some(snapshot) = connect_snapshot {
                    let mut call = connect_call;
                    call.grant = 0;
                    call.permission = 0;
                    queue.push_back(SapMsg {
                        sap: Sap::Control,
                        src: TetraEntity::Asterisk,
                        dest: TetraEntity::Cmce,
                        msg: SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest {
                            brew_uuid: snapshot.uuid,
                            call,
                        }),
                    });
                }
            }
            401 | 407 => {
                // ACK before bumping the CSeq: the ACK belongs to the challenged INVITE.
                self.ack_non_2xx(uuid, msg, &format!("ACK auth {}", uuid));
                if cancelling {
                    self.drop_dialog(uuid);
                    return;
                }
                if let Some(challenge) = Self::parse_challenge(msg) {
                    let mut should_retry = false;
                    if let Some(dialog) = self.dialogs.get_mut(&uuid)
                        && !dialog.auth_retry_sent
                    {
                        dialog.auth = Some(challenge);
                        dialog.auth_retry_sent = true;
                        dialog.cseq = dialog.cseq.saturating_add(1);
                        should_retry = true;
                    }
                    if should_retry {
                        self.send_invite(uuid);
                    }
                }
            }
            300..=699 => {
                self.ack_non_2xx(uuid, msg, &format!("ACK failure {}", uuid));
                if cancelling {
                    // 487 for the CANCEL we sent - expected, CMCE already knows.
                    self.drop_dialog(uuid);
                    return;
                }
                self.set_error(format!("INVITE uuid={} failed with SIP {}", uuid, code));
                self.send_release_to_cmce(queue, uuid, 34);
                self.release_dialog(uuid, false);
            }
            _ => {}
        }
    }

    fn find_dialog_by_call_id(&self, call_id: Option<&str>) -> Option<Uuid> {
        let call_id = call_id?;
        self.dialogs
            .iter()
            .find(|(_, dialog)| dialog.call_id_header.eq_ignore_ascii_case(call_id))
            .map(|(uuid, _)| *uuid)
    }

    fn maybe_periodic_sip(&mut self) {
        let now = Instant::now();

        // A cancelled INVITE whose final response never showed up would otherwise pin its RTP port.
        let stale: Vec<Uuid> = self
            .dialogs
            .iter()
            .filter(|(_, d)| {
                d.released_at
                    .is_some_and(|released| now.duration_since(released) >= CANCEL_REAP_AFTER)
            })
            .map(|(uuid, _)| *uuid)
            .collect();
        for uuid in stale {
            tracing::info!("AsteriskEntity: reaping cancelled dialog uuid={} with no final response", uuid);
            self.drop_dialog(uuid);
        }
        self.invite_rate
            .retain(|_, (seen, _)| now.duration_since(*seen) < Duration::from_secs(120));

        if self.asterisk_config.register
            && self
                .last_register
                .map(|last| now.duration_since(last) >= Duration::from_secs(60))
                .unwrap_or(true)
        {
            self.send_register();
        }

        let interval = Duration::from_secs(self.asterisk_config.options_interval_secs.max(5));
        if self.last_options.map(|last| now.duration_since(last) >= interval).unwrap_or(true) {
            self.send_options();
        }
    }
}

#[derive(Clone)]
struct SipDialogSnapshot {
    uuid: Uuid,
    number: String,
    call_id_header: String,
    local_uri: String,
    source_issi: Option<u32>,
    local_tag: String,
    remote_tag: Option<String>,
    cseq: u32,
    invite_branch: String,
    remote_target: Option<String>,
    peer_addr: Option<SocketAddr>,
}

impl SipDialogSnapshot {
    fn from_dialog(dialog: &SipDialog) -> Self {
        Self {
            uuid: dialog.uuid,
            number: dialog.number.clone(),
            call_id_header: dialog.call_id_header.clone(),
            local_uri: dialog.local_uri.clone(),
            source_issi: Some(dialog.call.source_issi),
            local_tag: dialog.local_tag.clone(),
            remote_tag: dialog.remote_tag.clone(),
            cseq: dialog.cseq,
            invite_branch: dialog.invite_branch.clone(),
            remote_target: dialog.remote_target.clone(),
            peer_addr: dialog.request_context.as_ref().map(|ctx| ctx.addr),
        }
    }
}

impl AsteriskEntity {
    /// `branch`/`cseq` are the caller's business: an ACK to a 2xx is its own transaction and
    /// gets a fresh branch, while an ACK to a non-2xx final response must repeat the INVITE's
    /// branch and CSeq number (RFC 3261 §17.1.1.3) or the UAS keeps retransmitting.
    fn build_ack(&self, dialog: &SipDialogSnapshot, branch: &str, cseq: u32) -> String {
        let request_uri = self.request_uri(&dialog.number);
        let to = if let Some(tag) = &dialog.remote_tag {
            format!("<{}>;tag={}", request_uri, tag)
        } else {
            format!("<{}>", request_uri)
        };
        format!(
            "ACK {} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
             Max-Forwards: 70\r\n\
             From: <{}>;tag={}\r\n\
             To: {}\r\n\
             Call-ID: {}\r\n\
             CSeq: {} ACK\r\n\
             Contact: <{}>\r\n\
             Content-Length: 0\r\n\r\n",
            request_uri,
            self.asterisk_config.contact_host,
            self.asterisk_config.bind_port,
            branch,
            dialog.local_uri,
            dialog.local_tag,
            to,
            dialog.call_id_header,
            cseq,
            self.contact_uri()
        )
    }

    /// ACK a final response that ended the INVITE transaction (401/407, 3xx-6xx).
    fn ack_non_2xx(&mut self, uuid: Uuid, msg: &SipMessage, summary: &str) {
        let Some(mut snapshot) = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog) else {
            return;
        };
        // The response carries a To tag we never saw before; the ACK has to echo it back.
        snapshot.remote_tag = Self::parse_to_tag(msg.header("To")).or(snapshot.remote_tag);
        let ack = self.build_ack(&snapshot, &snapshot.invite_branch, snapshot.cseq);
        self.send_sip(ack, summary.to_string());
    }
}

impl TetraEntityTrait for AsteriskEntity {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Asterisk
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        match message.msg {
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupRequest { brew_uuid, call }) => {
                self.start_outbound_call(queue, brew_uuid, call);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupAccept { brew_uuid }) => {
                self.handle_inbound_setup_accept(brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitSetupReject { brew_uuid, cause }) => {
                self.handle_inbound_setup_reject(brew_uuid, cause);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitAlert { brew_uuid }) => {
                self.handle_inbound_alert(brew_uuid);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectRequest { brew_uuid, call }) => {
                self.handle_inbound_connect_request(queue, brew_uuid, call);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitConnectConfirm { .. }) => {}
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitMediaReady {
                brew_uuid,
                call_id,
                carrier_num,
                ts,
            }) => {
                self.mark_media_ready(brew_uuid, call_id, carrier_num, ts);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitRelease { brew_uuid, .. }) => {
                self.release_dialog(brew_uuid, true);
            }
            SapMsgInner::CmceCallControl(CallControl::NetworkCircuitDtmf { brew_uuid, data, .. }) => {
                // CMCE hands us the already-decoded ASCII digits, one message per key-press, with
                // tone-end filtered out upstream (cc_bs/procedures/uplink.rs). Relay each valid
                // symbol as its own SIP INFO with a fixed nominal duration; skip any junk byte.
                for &byte in &data {
                    match dtmf_digit_from_byte(byte) {
                        Some(digit) => self.send_dtmf_info(brew_uuid, digit),
                        None => tracing::debug!("AsteriskEntity: uuid={} dropping non-DTMF byte 0x{:02x}", brew_uuid, byte),
                    }
                }
            }
            SapMsgInner::TmdCircuitDataInd(prim) => {
                self.handle_ul_voice(prim);
            }
            _ => {}
        }
        self.refresh_status();
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, ts: TdmaTime) {
        self.maybe_periodic_sip();
        self.poll_sip(queue);
        self.poll_rtp(queue, ts);
        self.refresh_status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(cancel: bool) -> TeardownParams<'static> {
        TeardownParams {
            cancel,
            invite_uri: "sip:1001@pbx.local",
            remote_target: Some("sip:1001@10.0.0.9:5060"),
            via_host: "10.0.0.2",
            via_port: 5062,
            invite_branch: "z9hG4bKflow00000001",
            fresh_branch: "z9hG4bKflow00000009",
            from_uri: "sip:flowstation@pbx.local",
            local_tag: "flowabcd",
            remote_tag: Some("as1234"),
            call_id: "flow-uuid@10.0.0.2",
            invite_cseq: 2,
            contact: "sip:flowstation@10.0.0.2:5062",
        }
    }

    #[test]
    fn cancel_stays_inside_the_invite_transaction() {
        let request = build_teardown_request(&params(true));
        assert!(request.starts_with("CANCEL sip:1001@pbx.local SIP/2.0\r\n"), "{}", request);
        assert!(request.contains("branch=z9hG4bKflow00000001;"), "{}", request);
        assert!(request.contains("\r\nCSeq: 2 CANCEL\r\n"), "{}", request);
        // RFC 3261 §9.1: To is copied from the INVITE, which went out untagged.
        assert!(request.contains("\r\nTo: <sip:1001@pbx.local>\r\n"), "{}", request);
        assert!(!request.contains("Contact:"), "{}", request);
    }

    #[test]
    fn bye_opens_a_new_transaction_towards_the_remote_target() {
        let request = build_teardown_request(&params(false));
        assert!(request.starts_with("BYE sip:1001@10.0.0.9:5060 SIP/2.0\r\n"), "{}", request);
        assert!(request.contains("branch=z9hG4bKflow00000009;"), "{}", request);
        assert!(request.contains("\r\nCSeq: 3 BYE\r\n"), "{}", request);
        assert!(request.contains("\r\nFrom: <sip:flowstation@pbx.local>;tag=flowabcd\r\n"), "{}", request);
        assert!(request.contains("\r\nTo: <sip:1001@pbx.local>;tag=as1234\r\n"), "{}", request);
    }

    #[test]
    fn contact_uri_is_unwrapped_from_angle_brackets() {
        assert_eq!(
            AsteriskEntity::parse_contact_uri(Some("<sip:1001@10.0.0.9:5060>;expires=60")).as_deref(),
            Some("sip:1001@10.0.0.9:5060")
        );
        assert_eq!(
            AsteriskEntity::parse_contact_uri(Some("sip:1001@10.0.0.9;transport=udp")).as_deref(),
            Some("sip:1001@10.0.0.9")
        );
        assert_eq!(AsteriskEntity::parse_contact_uri(Some("*")), None);
    }

    #[test]
    fn dtmf_byte_maps_to_ascii_symbols() {
        assert_eq!(dtmf_digit_from_byte(b'4'), Some('4'));
        assert_eq!(dtmf_digit_from_byte(b'*'), Some('*'));
        assert_eq!(dtmf_digit_from_byte(b'#'), Some('#'));
        assert_eq!(dtmf_digit_from_byte(b'A'), Some('A'));
        // A-D arrive case-normalised.
        assert_eq!(dtmf_digit_from_byte(b'd'), Some('D'));
        // 0x04 is ASCII EOT, not the digit '4': CMCE sends ASCII, so a raw nibble is junk here.
        assert_eq!(dtmf_digit_from_byte(0x04), None);
        assert_eq!(dtmf_digit_from_byte(b'E'), None);
    }

    fn dtmf_params(digit: char) -> DtmfInfoParams<'static> {
        DtmfInfoParams {
            request_uri: "sip:1001@pbx.local",
            remote_target: Some("sip:1001@10.0.0.9:5060"),
            via_host: "10.0.0.2",
            via_port: 5062,
            branch: "z9hG4bKflow0000000a",
            from_uri: "sip:flowstation@pbx.local",
            local_tag: "flowabcd",
            remote_tag: Some("as1234"),
            call_id: "flow-uuid@10.0.0.2",
            cseq: 3,
            contact: "sip:flowstation@10.0.0.2:5062",
            digit,
            duration_ms: 250,
        }
    }

    #[test]
    fn dtmf_info_is_an_in_dialog_request_to_the_remote_target() {
        let request = build_dtmf_info(&dtmf_params('4'));
        // Request-URI is the answer's Contact (remote target), not the AOR.
        assert!(request.starts_with("INFO sip:1001@10.0.0.9:5060 SIP/2.0\r\n"), "{}", request);
        assert!(request.contains("branch=z9hG4bKflow0000000a;"), "{}", request);
        assert!(request.contains("\r\nCSeq: 3 INFO\r\n"), "{}", request);
        assert!(request.contains("\r\nFrom: <sip:flowstation@pbx.local>;tag=flowabcd\r\n"), "{}", request);
        // To keeps the AOR plus the negotiated remote tag, exactly like the BYE.
        assert!(request.contains("\r\nTo: <sip:1001@pbx.local>;tag=as1234\r\n"), "{}", request);
    }

    #[test]
    fn dtmf_info_body_is_a_dtmf_relay_signal() {
        let request = build_dtmf_info(&dtmf_params('#'));
        assert!(request.contains("\r\nContent-Type: application/dtmf-relay\r\n"), "{}", request);
        let body = "Signal=#\r\nDuration=250\r\n";
        // Body follows the blank line unchanged and Content-Length has to count its bytes.
        assert!(request.ends_with(&format!("\r\n\r\n{}", body)), "{}", request);
        assert!(request.contains(&format!("\r\nContent-Length: {}\r\n", body.len())), "{}", request);
    }
}
