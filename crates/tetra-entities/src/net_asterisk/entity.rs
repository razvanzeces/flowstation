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

use super::audio::{AsteriskAudioTranscoder, PCMU_PAYLOAD_TYPE, rtp_payload};

const SIP_MAX_DATAGRAM: usize = 8192;

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
    Established,
    Released,
}

struct RtpSession {
    socket: UdpSocket,
    local_port: u16,
    remote: Option<SocketAddr>,
    seq: u16,
    timestamp: u32,
    ssrc: u32,
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
    auth: Option<DigestChallenge>,
    auth_retry_sent: bool,
    state: DialogState,
    rtp: RtpSession,
    audio: AsteriskAudioTranscoder,
    media_ready: Option<(u16, u16, u8)>,
    inbound: bool,
    request_context: Option<SipRequestContext>,
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

pub struct AsteriskEntity {
    config: SharedConfig,
    asterisk_config: CfgAsterisk,
    sip_socket: UdpSocket,
    remote: SocketAddr,
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

        let entity = Self {
            config,
            next_rtp_port: asterisk_config.rtp_port_min,
            register_call_id: format!("flow-reg-{}@{}", Uuid::new_v4(), asterisk_config.contact_host),
            asterisk_config,
            sip_socket,
            remote,
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
            active_dialogs: self.dialogs.values().filter(|d| d.state != DialogState::Released).count(),
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
             a=ptime:60\r\n\
             a=maxptime:60\r\n\
             a=sendrecv\r\n",
            self.asterisk_config.contact_host, self.asterisk_config.contact_host, rtp_port
        )
    }

    fn build_invite(&mut self, uuid: Uuid) -> Option<String> {
        let snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog)?;
        let (rtp_port, auth) = self.dialogs.get(&uuid).map(|dialog| (dialog.rtp.local_port, dialog.auth.clone()))?;
        let request_uri = self.request_uri(&snapshot.number);
        let branch = self.next_branch();
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
        let method = if cancel { "CANCEL" } else { "BYE" };
        let request_uri = self.request_uri(&dialog.number);
        let branch = self.next_branch();
        let to = if let Some(tag) = &dialog.remote_tag {
            format!("<{}>;tag={}", request_uri, tag)
        } else {
            format!("<{}>", request_uri)
        };
        let cseq = if cancel { dialog.cseq } else { dialog.cseq.saturating_add(1) };
        let request = format!(
            "{} {} SIP/2.0\r\n\
             Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
             Max-Forwards: 70\r\n\
             From: <{}>;tag={}\r\n\
             To: {}\r\n\
             Call-ID: {}\r\n\
             CSeq: {} {}\r\n\
             Contact: <{}>\r\n\
             Content-Length: 0\r\n\r\n",
            method,
            request_uri,
            self.asterisk_config.contact_host,
            self.asterisk_config.bind_port,
            branch,
            dialog.local_uri,
            dialog.local_tag,
            to,
            dialog.call_id_header,
            cseq,
            method,
            self.contact_uri()
        );
        self.send_sip(request, format!("{} {}", method, uuid));
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
                        seq: 1,
                        timestamp: 0,
                        ssrc,
                    });
                }
                Err(_) => {
                    port = port.saturating_add(1);
                }
            }
        }
        Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "no RTP port available"))
    }

    fn start_inbound_call(&mut self, queue: &mut MessageQueue, msg: &SipMessage, addr: SocketAddr) {
        let Some(ctx) = Self::request_context(msg, addr) else {
            return;
        };
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
            auth: None,
            auth_retry_sent: false,
            state: DialogState::Inviting,
            rtp,
            audio,
            media_ready: None,
            inbound: true,
            request_context: Some(ctx.clone()),
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
            auth: None,
            auth_retry_sent: false,
            state: DialogState::Inviting,
            rtp,
            audio,
            media_ready: None,
            inbound: false,
            request_context: None,
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
        let Some((cancel, media_ready, inbound)) = self.dialogs.get(&brew_uuid).map(|dialog| {
            (
                !matches!(dialog.state, DialogState::Established),
                dialog.media_ready,
                dialog.inbound,
            )
        }) else {
            return;
        };
        if from_cmce {
            if inbound && cancel {
                self.send_invite_response(brew_uuid, 480, "Temporarily Unavailable", None);
            } else {
                self.send_bye_or_cancel(brew_uuid, cancel);
            }
        }
        if let Some((_, carrier_num, ts)) = media_ready {
            self.rtp_by_ts.remove(&(carrier_num, ts));
        }
        if let Some(dialog) = self.dialogs.get_mut(&brew_uuid) {
            dialog.state = DialogState::Released;
        }
        self.dialogs.remove(&brew_uuid);
    }

    fn handle_ul_voice(&mut self, prim: TmdCircuitDataInd) {
        let Some(uuid) = self.rtp_by_ts.get(&(prim.carrier_num, prim.ts)).copied() else {
            return;
        };
        let mut send_result = None;
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

            let mut packet = Vec::with_capacity(12 + payload.len());
            packet.push(0x80);
            packet.push(PCMU_PAYLOAD_TYPE);
            packet.extend_from_slice(&dialog.rtp.seq.to_be_bytes());
            packet.extend_from_slice(&dialog.rtp.timestamp.to_be_bytes());
            packet.extend_from_slice(&dialog.rtp.ssrc.to_be_bytes());
            packet.extend_from_slice(&payload);
            let result = dialog.rtp.socket.send_to(&packet, remote);
            if result.is_ok() {
                dialog.rtp.seq = dialog.rtp.seq.wrapping_add(1);
                dialog.rtp.timestamp = dialog.rtp.timestamp.wrapping_add(payload.len().max(1) as u32);
            }
            send_result = Some(result);
        }
        if let Some(reason) = drop_reason {
            self.set_error(reason);
            return;
        }
        if let Some(Err(err)) = send_result {
            self.set_error(format!("RTP send failed uuid={} ts={}: {}", uuid, prim.ts, err));
        };
    }

    fn poll_rtp(&mut self, queue: &mut MessageQueue) {
        let mut downlink = Vec::new();
        let mut last_error = None;
        let mut buf = [0u8; 1720];
        for dialog in self.dialogs.values_mut() {
            let Some((_, carrier_num, ts)) = dialog.media_ready else {
                continue;
            };
            for _ in 0..32 {
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
                        dialog.rtp.remote = Some(addr);
                        for frame in dialog.audio.encode_pcmu_to_tmd(payload) {
                            downlink.push((carrier_num, ts, frame));
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                    Err(err) => {
                        last_error = Some(format!("RTP receive failed uuid={}: {}", dialog.uuid, err));
                        break;
                    }
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
                "BYE" => {
                    self.answer_request(&msg, addr, 200, "OK");
                    if let Some(uuid) = self.find_dialog_by_call_id(msg.call_id()) {
                        self.send_release_to_cmce(queue, uuid, 16);
                        self.release_dialog(uuid, false);
                    }
                }
                "CANCEL" => {
                    self.answer_request(&msg, addr, 200, "OK");
                    if let Some(uuid) = self.find_dialog_by_call_id(msg.call_id()) {
                        self.send_release_to_cmce(queue, uuid, 16);
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

        match code {
            100 => {}
            180 | 183 => {
                if let Some(dialog) = self.dialogs.get_mut(&uuid) {
                    dialog.state = DialogState::Ringing;
                    dialog.remote_tag = Self::parse_to_tag(msg.header("To"));
                }
                self.send_alert(queue, uuid);
            }
            200..=299 => {
                let remote_rtp = self.parse_sdp_remote(&msg.body);
                let connect_call = {
                    let Some(dialog) = self.dialogs.get_mut(&uuid) else {
                        return;
                    };
                    dialog.remote_tag = Self::parse_to_tag(msg.header("To"));
                    if let Some(remote_rtp) = remote_rtp {
                        dialog.rtp.remote = Some(remote_rtp);
                    }
                    dialog.state = DialogState::Established;
                    dialog.call.clone()
                };
                let ack_snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog);
                if let Some(dialog_for_ack) = ack_snapshot {
                    let ack_text = self.build_ack_from_snapshot(&dialog_for_ack);
                    self.send_sip(ack_text, format!("ACK {}", uuid));
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
                    let ack_snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog);
                    if let Some(snapshot) = ack_snapshot {
                        let ack_text = self.build_ack_from_snapshot(&snapshot);
                        self.send_sip(ack_text, format!("ACK auth {}", uuid));
                    }
                    if should_retry {
                        self.send_invite(uuid);
                    }
                }
            }
            300..=699 => {
                let ack_snapshot = self.dialogs.get(&uuid).map(SipDialogSnapshot::from_dialog);
                if let Some(snapshot) = ack_snapshot {
                    let ack_text = self.build_ack_from_snapshot(&snapshot);
                    self.send_sip(ack_text, format!("ACK failure {}", uuid));
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
        }
    }
}

impl AsteriskEntity {
    fn build_ack_from_snapshot(&mut self, dialog: &SipDialogSnapshot) -> String {
        let request_uri = self.request_uri(&dialog.number);
        let branch = self.next_branch();
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
            dialog.cseq,
            self.contact_uri()
        )
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
                tracing::debug!("AsteriskEntity: DTMF for uuid={} bytes={} currently ignored", brew_uuid, data.len());
            }
            SapMsgInner::TmdCircuitDataInd(prim) => {
                self.handle_ul_voice(prim);
            }
            _ => {}
        }
        self.refresh_status();
    }

    fn tick_start(&mut self, queue: &mut MessageQueue, _ts: TdmaTime) {
        self.maybe_periodic_sip();
        self.poll_sip(queue);
        self.poll_rtp(queue);
        self.refresh_status();
    }
}
