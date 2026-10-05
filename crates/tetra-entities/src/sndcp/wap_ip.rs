// SPDX-FileCopyrightText: 2026 Chris YO3TCO / Nexus-BS Project
// Owner: Chris YO3TCO / Nexus-BS Project
// SPDX-License-Identifier: PolyForm-Noncommercial-1.0.0
// SPDX-FileComment: Nexus-BS original WAP-over-UDP/IP adapter primitives for TETRA SNDCP experiments.
// SPDX-FileComment: Modified by flowstation-miura (EA5GVK): visibility only (pub(crate) on helpers, structs, fields and constants); tests follow the Spanish status pages.

use super::ip::{IPV4_PROTOCOL_UDP, IpPrimitiveError, build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use super::wap_status::{
    DEFAULT_WAP_STATUS_MAX_BYTES, WAP_STATUS_HTML_PATH, WAP_STATUS_LEGACY_WML_PATH, WAP_STATUS_REFRESH_PATH, WAP_STATUS_SECTOR_QUERY,
    WapStatusError, WapStatusSnapshot, render_wml_status_browser_index, render_wml_status_browser_sector, render_wml2_status,
    render_wml2_status_browser_index, render_wml2_status_browser_sector, render_wml2_status_sector,
};

pub const DEFAULT_WAP_UDP_REQUEST_MAX_BYTES: usize = 1024;
pub const DEFAULT_WAP_WSP_STATUS_MAX_BYTES: usize = WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES - WSP_REPLY_FIXED_HEADER_BYTES;
pub const IPV4_UDP_HEADER_BYTES: usize = 28;
pub(crate) const WTP_CON_FLAG: u8 = 0x80;
pub(crate) const WTP_RID_FLAG: u8 = 0x01;
pub(crate) const WTP_PDU_INVOKE: u8 = 1;
pub(crate) const WTP_PDU_RESULT: u8 = 2;
pub(crate) const WTP_PDU_ACK: u8 = 3;
pub(crate) const WTP_PDU_ABORT: u8 = 4;
pub(crate) const WTP_TRAILER_LAST_PACKET_OF_MESSAGE: u8 = 0x02;
pub(crate) const WTP_RESULT_LAST_PACKET_OF_MESSAGE: u8 = (WTP_PDU_RESULT << 3) | WTP_TRAILER_LAST_PACKET_OF_MESSAGE;
pub(crate) const WTP_TID_RESPONSE_FLAG: u16 = 0x8000;
pub(crate) const WTP_TID_VALUE_MASK: u16 = 0x7fff;
const WSP_PDU_CONNECT: u8 = 0x01;
const WSP_PDU_CONNECT_REPLY: u8 = 0x02;
const WSP_PDU_REPLY: u8 = 0x04;
const WSP_PDU_RESUME: u8 = 0x09;
const WSP_PDU_GET: u8 = 0x40;
const WSP_STATUS_OK: u8 = 0x20;
const WSP_SHORT_INTEGER_FLAG: u8 = 0x80;
const WSP_CT_TEXT_VND_WAP_WML_ASSIGNED_NUMBER: u8 = 0x08;
const WSP_CT_TEXT_VND_WAP_WML: u8 = WSP_SHORT_INTEGER_FLAG | WSP_CT_TEXT_VND_WAP_WML_ASSIGNED_NUMBER;
const WSP_CT_APP_VND_WAP_XHTML_XML_ASSIGNED_NUMBER: u8 = 0x45;
const WSP_CT_APP_VND_WAP_XHTML_XML: u8 = WSP_SHORT_INTEGER_FLAG | WSP_CT_APP_VND_WAP_XHTML_XML_ASSIGNED_NUMBER;
pub(crate) const WSP_CAP_CLIENT_SDU_SIZE: u8 = 0x80;
pub(crate) const WSP_CAP_SERVER_SDU_SIZE: u8 = 0x81;
pub(crate) const WSP_CAP_PROTOCOL_OPTIONS: u8 = 0x82;
pub(crate) const WSP_CAP_METHOD_MOR: u8 = 0x83;
pub(crate) const WSP_CAP_EXTENDED_METHODS: u8 = 0x85;
pub(crate) const WSP_CAP_HEADER_CODE_PAGES: u8 = 0x86;
// Keep default WSP SDUs inside a 576-octet SNDCP N-PDU on a one-slot PDCH.
// IPv4+UDP consumes 28 octets and WTP Result consumes 3 octets.
const WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES: usize = 545;
const WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES: usize = 545;
const WSP_REPLY_FIXED_HEADER_BYTES: usize = 4;
const WSP_OPENWAVE_WML_BROWSER_PAGE_MAX_BYTES: usize = 144;
const WSP_OPENWAVE_XHTML_BROWSER_INDEX_MAX_BYTES: usize = 104;
const WSP_OPENWAVE_XHTML_BROWSER_SECTOR_MAX_BYTES: usize = 144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WapIpEndpoint {
    pub address: [u8; 4],
    pub port: u16,
    pub response_ttl: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WapIpServicePolicy {
    pub status_enabled: bool,
    pub accept_empty_probe: bool,
    pub accept_root_path: bool,
    pub accept_status_path: bool,
    pub accept_status_wml_path: bool,
    pub max_request_payload_bytes: usize,
    pub allowed_issis: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapUdpRequestKind {
    Empty,
    Status {
        sector: Option<usize>,
        format: WapStatusFormat,
    },
    WtpWspConnect {
        transaction_id: u16,
        retransmission: bool,
    },
    WtpWspResume {
        transaction_id: u16,
        session_id: usize,
        retransmission: bool,
    },
    WtpWspStatus {
        transaction_id: u16,
        retransmission: bool,
        sector: Option<usize>,
        format: WapStatusFormat,
    },
    WtpControlNoResponse {
        transaction_id: u16,
        pdu_type: u8,
        abort: Option<WtpAbortInfo>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WapStatusFormat {
    Xhtml,
    Wml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WtpAbortInfo {
    pub abort_type: u8,
    pub reason: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WapIpError {
    Ip(IpPrimitiveError),
    Status(WapStatusError),
    UnsupportedIpProtocol { protocol: u8 },
    WrongDestination { expected: [u8; 4], actual: [u8; 4] },
    WrongUdpPort { expected: u16, actual: u16 },
    WrongTcpPort { expected: u16, actual: u16 },
    StatusServiceDisabled,
    EmptyProbeDisabled,
    UdpPayloadTooLarge { len: usize, max: usize },
    TcpPayloadTooLarge { len: usize, max: usize },
    UnsupportedWapUdpPayload { len: usize },
    UnsupportedHttpTcpPayload { len: usize },
    UnsupportedTcpSegment { flags: u16, payload_len: usize },
    UnsupportedWapPath { path: String },
    NoResponseRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WspCapability {
    pub(crate) id: u8,
    pub(crate) parameters: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WspConnectRequest {
    pub(crate) version: u8,
    pub(crate) capabilities: Vec<WspCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WspResumeRequest {
    pub(crate) session_id: usize,
}

#[derive(Debug, Clone, Copy)]
struct WtpInvoke<'a> {
    transaction_id: u16,
    retransmission: bool,
    wsp: &'a [u8],
}

impl From<IpPrimitiveError> for WapIpError {
    fn from(value: IpPrimitiveError) -> Self {
        WapIpError::Ip(value)
    }
}

impl From<WapStatusError> for WapIpError {
    fn from(value: WapStatusError) -> Self {
        WapIpError::Status(value)
    }
}

impl Default for WapIpServicePolicy {
    fn default() -> Self {
        Self {
            status_enabled: false,
            accept_empty_probe: false,
            accept_root_path: false,
            accept_status_path: false,
            accept_status_wml_path: false,
            max_request_payload_bytes: DEFAULT_WAP_UDP_REQUEST_MAX_BYTES,
            allowed_issis: None,
        }
    }
}

impl WapIpServicePolicy {
    pub fn experimental_status() -> Self {
        Self {
            status_enabled: true,
            accept_empty_probe: true,
            accept_root_path: true,
            accept_status_path: true,
            accept_status_wml_path: true,
            ..Self::default()
        }
    }

    pub fn experimental_status_for_issis(allowed_issis: Vec<u32>) -> Self {
        Self {
            allowed_issis: Some(allowed_issis),
            ..Self::experimental_status()
        }
    }

    pub fn allows_issi(&self, issi: u32) -> bool {
        self.allowed_issis.as_ref().map(|allowed| allowed.contains(&issi)).unwrap_or(true)
    }
}

pub fn build_wap_status_response_npdu(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
) -> Result<Vec<u8>, WapIpError> {
    required_response(build_wap_status_response_npdu_optional_with_npdu_budget(
        request_npdu,
        endpoint,
        policy,
        snapshot,
        None,
    )?)
}

pub fn build_wap_status_response_npdu_with_wml2_budget(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_wml2_bytes: usize,
) -> Result<Vec<u8>, WapIpError> {
    required_response(build_wap_status_response_npdu_optional_with_npdu_budget(
        request_npdu,
        endpoint,
        policy,
        snapshot,
        Some(max_wml2_bytes.saturating_add(IPV4_UDP_HEADER_BYTES)),
    )?)
}

pub fn build_wap_status_response_npdu_optional_with_npdu_budget(
    request_npdu: &[u8],
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_npdu_bytes: Option<usize>,
) -> Result<Option<Vec<u8>>, WapIpError> {
    let request_ip = parse_ipv4_packet(request_npdu)?;
    if request_ip.destination != endpoint.address {
        return Err(WapIpError::WrongDestination {
            expected: endpoint.address,
            actual: request_ip.destination,
        });
    }

    match request_ip.protocol {
        IPV4_PROTOCOL_UDP => build_udp_status_response(&request_ip, endpoint, policy, snapshot, max_npdu_bytes),
        protocol => Err(WapIpError::UnsupportedIpProtocol { protocol }),
    }
}

fn required_response(response: Option<Vec<u8>>) -> Result<Vec<u8>, WapIpError> {
    response.ok_or(WapIpError::NoResponseRequired)
}

fn build_udp_status_response(
    request_ip: &super::ip::Ipv4Packet<'_>,
    endpoint: WapIpEndpoint,
    policy: &WapIpServicePolicy,
    snapshot: &WapStatusSnapshot,
    max_npdu_bytes: Option<usize>,
) -> Result<Option<Vec<u8>>, WapIpError> {
    let request_udp = parse_udp_datagram(request_ip.payload)?;
    if request_udp.destination_port != endpoint.port {
        return Err(WapIpError::WrongUdpPort {
            expected: endpoint.port,
            actual: request_udp.destination_port,
        });
    }
    let request_kind = parse_wap_udp_request(request_udp.payload, policy)?;
    tracing::info!(
        "WAP/IP diag: IPv4/UDP request src={:?}:{} dst={:?}:{} kind={:?} payload_len={}",
        request_ip.source,
        request_udp.source_port,
        request_ip.destination,
        request_udp.destination_port,
        request_kind,
        request_udp.payload.len()
    );

    let response_payload = match request_kind {
        WapUdpRequestKind::Empty => {
            let max_wml2_bytes = max_npdu_bytes
                .map(|max| max.saturating_sub(IPV4_UDP_HEADER_BYTES))
                .unwrap_or(DEFAULT_WAP_STATUS_MAX_BYTES)
                .min(DEFAULT_WAP_STATUS_MAX_BYTES);
            render_wml2_status(snapshot, max_wml2_bytes)?.into_bytes()
        }
        WapUdpRequestKind::Status { sector, format } => {
            let max_wml2_bytes = max_npdu_bytes
                .map(|max| max.saturating_sub(IPV4_UDP_HEADER_BYTES))
                .unwrap_or(DEFAULT_WAP_STATUS_MAX_BYTES)
                .min(DEFAULT_WAP_STATUS_MAX_BYTES);
            match (format, sector) {
                (WapStatusFormat::Wml, Some(sector)) => render_wml_status_browser_sector(snapshot, max_wml2_bytes, sector)?.into_bytes(),
                (WapStatusFormat::Wml, None) => render_wml_status_browser_index(snapshot, max_wml2_bytes)?.into_bytes(),
                (WapStatusFormat::Xhtml, Some(sector)) => render_wml2_status_sector(snapshot, max_wml2_bytes, sector)?.into_bytes(),
                (WapStatusFormat::Xhtml, None) => render_wml2_status(snapshot, max_wml2_bytes)?.into_bytes(),
            }
        }
        WapUdpRequestKind::WtpWspConnect {
            transaction_id,
            retransmission: _,
        } => {
            let connect = parse_wtp_wsp_connect_request(request_udp.payload)?.ok_or(WapIpError::UnsupportedWapUdpPayload {
                len: request_udp.payload.len(),
            })?;
            tracing::info!(
                "WAP/IP diag: WSP Connect version={:#x} requested_capabilities={:?}",
                connect.version,
                connect.capabilities
            );
            build_wtp_wsp_result(transaction_id, &build_wsp_connect_reply(&connect))
        }
        WapUdpRequestKind::WtpWspResume {
            transaction_id,
            session_id,
            retransmission: _,
        } => {
            tracing::info!("WAP/IP diag: WSP Resume session_id={}", session_id);
            build_wtp_wsp_result(transaction_id, &build_wsp_session_ok_reply())
        }
        WapUdpRequestKind::WtpWspStatus {
            transaction_id,
            retransmission: _,
            sector,
            format,
        } => {
            let max_wsp_page_bytes = max_npdu_bytes
                .map(|max| max.saturating_sub(IPV4_UDP_HEADER_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES))
                .unwrap_or(DEFAULT_WAP_WSP_STATUS_MAX_BYTES)
                .min(DEFAULT_WAP_WSP_STATUS_MAX_BYTES);
            let (page, content_type) = match (format, sector) {
                (WapStatusFormat::Wml, Some(sector)) => (
                    render_wml_status_browser_sector(snapshot, max_wsp_page_bytes.min(WSP_OPENWAVE_WML_BROWSER_PAGE_MAX_BYTES), sector)?,
                    WSP_CT_TEXT_VND_WAP_WML,
                ),
                (WapStatusFormat::Wml, None) => (
                    render_wml_status_browser_index(snapshot, max_wsp_page_bytes.min(WSP_OPENWAVE_WML_BROWSER_PAGE_MAX_BYTES))?,
                    WSP_CT_TEXT_VND_WAP_WML,
                ),
                (WapStatusFormat::Xhtml, Some(sector)) => (
                    render_wml2_status_browser_sector(
                        snapshot,
                        max_wsp_page_bytes.min(WSP_OPENWAVE_XHTML_BROWSER_SECTOR_MAX_BYTES),
                        sector,
                    )?,
                    WSP_CT_APP_VND_WAP_XHTML_XML,
                ),
                (WapStatusFormat::Xhtml, None) => (
                    render_wml2_status_browser_index(snapshot, max_wsp_page_bytes.min(WSP_OPENWAVE_XHTML_BROWSER_INDEX_MAX_BYTES))?,
                    WSP_CT_APP_VND_WAP_XHTML_XML,
                ),
            };
            build_wtp_wsp_result(transaction_id, &build_wsp_reply(content_type, page.as_bytes()))
        }
        WapUdpRequestKind::WtpControlNoResponse {
            transaction_id,
            pdu_type,
            abort,
        } => {
            tracing::info!(
                "WAP/IP diag: IPv4/UDP WTP control no-response transaction_id={} pdu_type={} abort={:?} abort_type={} abort_reason={} abort_reason_name={}",
                transaction_id,
                pdu_type,
                abort,
                abort.map(|info| info.abort_type).unwrap_or_default(),
                abort.map(|info| info.reason).unwrap_or_default(),
                abort.map(wtp_abort_reason_name).unwrap_or("none")
            );
            return Ok(None);
        }
    };
    let response = build_ipv4_udp_npdu(
        endpoint.address,
        request_ip.source,
        endpoint.port,
        request_udp.source_port,
        &response_payload,
        request_ip.identification.wrapping_add(1),
        endpoint.response_ttl,
    )?;
    tracing::info!(
        "WAP/IP diag: IPv4/UDP response dst={:?}:{} payload_len={} npdu_len={}",
        request_ip.source,
        request_udp.source_port,
        response_payload.len(),
        response.len()
    );

    Ok(Some(response))
}

pub fn parse_wap_udp_request(payload: &[u8], policy: &WapIpServicePolicy) -> Result<WapUdpRequestKind, WapIpError> {
    // ETSI TS 100 392-2 clause 29.5.8 delegates WAP protocol details to WAP
    // 2.0. This diagnostic gateway intentionally admits only the configured
    // status resources over the WSP/WTP/UDP packet-data bearer.
    if !policy.status_enabled {
        return Err(WapIpError::StatusServiceDisabled);
    }
    if payload.len() > policy.max_request_payload_bytes {
        return Err(WapIpError::UdpPayloadTooLarge {
            len: payload.len(),
            max: policy.max_request_payload_bytes,
        });
    }
    if payload.is_empty() {
        if policy.accept_empty_probe {
            return Ok(WapUdpRequestKind::Empty);
        }
        return Err(WapIpError::EmptyProbeDisabled);
    }

    if let Some(kind) = parse_wtp_wsp_request(payload, policy)? {
        return Ok(kind);
    }

    let Ok(text) = std::str::from_utf8(payload) else {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    };
    let text = text.trim();

    if let Some(path) = text.strip_prefix("GET ") {
        let uri = path.split_whitespace().next().unwrap_or("/");
        let sector = status_sector_from_uri(uri);
        let path = normalize_get_path(uri);
        if is_status_path(&path, policy) {
            return Ok(WapUdpRequestKind::Status {
                sector,
                format: status_format_from_path(&path),
            });
        }
        return Err(WapIpError::UnsupportedWapPath { path });
    }

    Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() })
}

fn parse_wtp_wsp_request(payload: &[u8], policy: &WapIpServicePolicy) -> Result<Option<WapUdpRequestKind>, WapIpError> {
    if payload.len() < 3 {
        return Ok(None);
    }

    let wtp_pdu_type = (payload[0] >> 3) & 0x0f;
    let transaction_id = u16::from_be_bytes([payload[1], payload[2]]) & WTP_TID_VALUE_MASK;
    if matches!(wtp_pdu_type, WTP_PDU_ACK | WTP_PDU_ABORT) {
        return Ok(Some(WapUdpRequestKind::WtpControlNoResponse {
            transaction_id,
            pdu_type: wtp_pdu_type,
            abort: parse_wtp_abort_info(payload, wtp_pdu_type),
        }));
    }
    if wtp_pdu_type != WTP_PDU_INVOKE {
        return Ok(None);
    }

    let Some(invoke) = parse_wtp_invoke(payload)? else {
        return Ok(None);
    };
    let wsp = invoke.wsp;
    match wsp.first().copied() {
        Some(WSP_PDU_CONNECT) => Ok(Some(WapUdpRequestKind::WtpWspConnect {
            transaction_id: invoke.transaction_id,
            retransmission: invoke.retransmission,
        })),
        Some(WSP_PDU_RESUME) => {
            let resume = parse_wsp_resume_request(wsp).ok_or(WapIpError::UnsupportedWapUdpPayload { len: payload.len() })?;
            Ok(Some(WapUdpRequestKind::WtpWspResume {
                transaction_id: invoke.transaction_id,
                session_id: resume.session_id,
                retransmission: invoke.retransmission,
            }))
        }
        Some(pdu_type) if pdu_type == WSP_PDU_GET || (0x50..=0x5f).contains(&pdu_type) => {
            let (uri_len, len_octets) = read_uintvar(&wsp[1..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: payload.len() })?;
            let uri_start = 1 + len_octets;
            let uri_end = uri_start + uri_len;
            if uri_end > wsp.len() {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            }
            let Ok(uri) = std::str::from_utf8(&wsp[uri_start..uri_end]) else {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            };
            let sector = status_sector_from_uri(uri);
            let path = normalize_get_path(uri_path(uri));
            if is_status_path(&path, policy) {
                return Ok(Some(WapUdpRequestKind::WtpWspStatus {
                    transaction_id: invoke.transaction_id,
                    retransmission: invoke.retransmission,
                    sector,
                    format: status_format_from_path(&path),
                }));
            }
            Err(WapIpError::UnsupportedWapPath { path })
        }
        _ => Ok(None),
    }
}

fn parse_wtp_wsp_connect_request(payload: &[u8]) -> Result<Option<WspConnectRequest>, WapIpError> {
    let Some(invoke) = parse_wtp_invoke(payload)? else {
        return Ok(None);
    };
    let wsp = invoke.wsp;
    if wsp.first().copied() != Some(WSP_PDU_CONNECT) {
        return Ok(None);
    }
    parse_wsp_connect_request(wsp).map(Some)
}

fn parse_wtp_invoke(payload: &[u8]) -> Result<Option<WtpInvoke<'_>>, WapIpError> {
    if payload.len() < 4 || ((payload[0] >> 3) & 0x0f) != WTP_PDU_INVOKE {
        return Ok(None);
    }
    let invoke_header = payload[3];
    let version = (invoke_header >> 6) & 0x03;
    let reserved = invoke_header & 0x0c;
    let transaction_class = invoke_header & 0x03;
    if version != 0 || reserved != 0 || transaction_class != 2 {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    }

    let wsp_start = if payload[0] & WTP_CON_FLAG != 0 {
        parse_wtp_variable_header_end(payload, 4)?
    } else {
        4
    };
    if wsp_start >= payload.len() {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
    }

    Ok(Some(WtpInvoke {
        transaction_id: u16::from_be_bytes([payload[1], payload[2]]) & WTP_TID_VALUE_MASK,
        retransmission: payload[0] & WTP_RID_FLAG != 0,
        wsp: &payload[wsp_start..],
    }))
}

pub(crate) fn parse_wtp_variable_header_end(payload: &[u8], mut offset: usize) -> Result<usize, WapIpError> {
    loop {
        let Some(tpi_header) = payload.get(offset).copied() else {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
        };
        let tpi_continues = tpi_header & WTP_CON_FLAG != 0;
        let tpi_is_long = tpi_header & 0x04 != 0;
        offset = if tpi_is_long {
            let Some(tpi_len) = payload.get(offset + 1).copied() else {
                return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
            };
            offset + 2 + tpi_len as usize
        } else {
            offset + 1 + (tpi_header & 0x03) as usize
        };
        if offset > payload.len() {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: payload.len() });
        }
        if !tpi_continues {
            return Ok(offset);
        }
    }
}

pub(crate) fn parse_wsp_connect_request(wsp: &[u8]) -> Result<WspConnectRequest, WapIpError> {
    if wsp.len() < 4 || wsp[0] != WSP_PDU_CONNECT {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() });
    }

    let version = wsp[1];
    let (capabilities_len, cap_len_octets) = read_uintvar(&wsp[2..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() })?;
    let headers_len_start = 2 + cap_len_octets;
    let (headers_len, headers_len_octets) =
        read_uintvar(&wsp[headers_len_start..]).ok_or(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() })?;
    let capabilities_start = headers_len_start + headers_len_octets;
    let capabilities_end = capabilities_start + capabilities_len;
    let headers_end = capabilities_end + headers_len;
    if headers_end > wsp.len() {
        return Err(WapIpError::UnsupportedWapUdpPayload { len: wsp.len() });
    }

    let capabilities = parse_wsp_capabilities(&wsp[capabilities_start..capabilities_end])?;
    Ok(WspConnectRequest { version, capabilities })
}

pub(crate) fn parse_wsp_resume_request(wsp: &[u8]) -> Option<WspResumeRequest> {
    if wsp.first().copied() != Some(WSP_PDU_RESUME) {
        return None;
    }
    let (session_id, session_len_octets) = read_uintvar(&wsp[1..])?;
    let capabilities_len_start = 1 + session_len_octets;
    let (capabilities_len, cap_len_octets) = read_uintvar(&wsp[capabilities_len_start..])?;
    let capabilities_start = capabilities_len_start + cap_len_octets;
    let capabilities_end = capabilities_start + capabilities_len;
    if capabilities_end > wsp.len() {
        return None;
    }
    Some(WspResumeRequest { session_id })
}

pub(crate) fn parse_wsp_capabilities(mut buf: &[u8]) -> Result<Vec<WspCapability>, WapIpError> {
    let mut capabilities = Vec::new();
    while !buf.is_empty() {
        let (len, len_octets) = read_uintvar(buf).ok_or(WapIpError::UnsupportedWapUdpPayload { len: buf.len() })?;
        if len == 0 || len_octets + len > buf.len() {
            return Err(WapIpError::UnsupportedWapUdpPayload { len: buf.len() });
        }
        let body = &buf[len_octets..len_octets + len];
        capabilities.push(WspCapability {
            id: body[0],
            parameters: body[1..].to_vec(),
        });
        buf = &buf[len_octets + len..];
    }
    Ok(capabilities)
}

fn build_wtp_wsp_result(transaction_id: u16, wsp_payload: &[u8]) -> Vec<u8> {
    let response_tid = (transaction_id & WTP_TID_VALUE_MASK) | WTP_TID_RESPONSE_FLAG;
    let mut payload = Vec::with_capacity(3 + wsp_payload.len());
    payload.push(WTP_RESULT_LAST_PACKET_OF_MESSAGE);
    payload.extend_from_slice(&response_tid.to_be_bytes());
    payload.extend_from_slice(wsp_payload);
    payload
}

fn build_wsp_connect_reply(connect: &WspConnectRequest) -> Vec<u8> {
    let capabilities = build_wsp_connect_reply_capabilities(connect);
    let mut payload = Vec::with_capacity(4 + capabilities.len());
    payload.push(WSP_PDU_CONNECT_REPLY);
    payload.push(0x01); // Server session id.
    write_uintvar(capabilities.len(), &mut payload);
    write_uintvar(0, &mut payload); // Headers length.
    payload.extend_from_slice(&capabilities);
    payload
}

fn build_wsp_session_ok_reply() -> Vec<u8> {
    vec![WSP_PDU_REPLY, WSP_STATUS_OK, 0]
}

fn build_wsp_connect_reply_capabilities(connect: &WspConnectRequest) -> Vec<u8> {
    let mut capabilities = Vec::new();
    // Keep the negotiated session minimal for terminal WAP browsers: echo only
    // bounded SDU sizes and avoid synthesizing optional capabilities.
    if let Some(requested) = wsp_capability_uintvar(connect, WSP_CAP_CLIENT_SDU_SIZE) {
        push_wsp_uintvar_capability(
            &mut capabilities,
            WSP_CAP_CLIENT_SDU_SIZE,
            requested.min(WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES),
        );
    }
    if let Some(requested) = wsp_capability_uintvar(connect, WSP_CAP_SERVER_SDU_SIZE) {
        push_wsp_uintvar_capability(
            &mut capabilities,
            WSP_CAP_SERVER_SDU_SIZE,
            requested.min(WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES),
        );
    }
    capabilities
}

pub(crate) fn parse_wtp_abort_info(payload: &[u8], pdu_type: u8) -> Option<WtpAbortInfo> {
    if pdu_type != WTP_PDU_ABORT || payload.len() < 4 {
        return None;
    }
    // OMA WTP puts the Abort type in the low bits of octet 1 and the Abort
    // reason in octet 4. WSP user abort reasons use the full octet value.
    Some(WtpAbortInfo {
        abort_type: payload[0] & 0x07,
        reason: payload[3],
    })
}

pub(crate) fn wtp_abort_reason_name(info: WtpAbortInfo) -> &'static str {
    match (info.abort_type, info.reason) {
        (0, 0) => "provider/UNKNOWN",
        (0, 1) => "provider/PROTOERR",
        (0, 2) => "provider/INVALIDTID",
        (0, 3) => "provider/NOTIMPLEMENTEDCL2",
        (0, 4) => "provider/NOTIMPLEMENTEDSAR",
        (0, 5) => "provider/NOTIMPLEMENTEDUACK",
        (0, 6) => "provider/WTPVERSIONONE",
        (0, 7) => "provider/CAPTEMPEXCEEDED",
        (0, 8) => "provider/NORESPONSE",
        (0, 9) => "provider/MESSAGETOOLARGE",
        (0, 10) => "provider/NOTIMPLEMENTEDESAR",
        (1, 0xe0) => "user/WSP_PROTOERR",
        (1, 0xe1) => "user/WSP_DISCONNECT",
        (1, 0xe2) => "user/WSP_SUSPEND",
        (1, 0xe3) => "user/WSP_RESUME",
        (1, 0xe4) => "user/WSP_CONGESTION",
        (1, 0xe5) => "user/WSP_CONNECTERR",
        (1, 0xe6) => "user/WSP_MRUEXCEEDED",
        (1, 0xe7) => "user/WSP_MOREXCEEDED",
        (1, 0xe8) => "user/WSP_PEERREQ",
        (1, 0xe9) => "user/WSP_NETERR",
        (1, 0xea) => "user/WSP_USERREQ",
        (1, 0xeb) => "user/WSP_USERRFS",
        (1, 0xec) => "user/WSP_USERPND",
        (1, 0xed) => "user/WSP_USERDCR",
        (1, 0xee) => "user/WSP_USERDCU",
        (1, _) => "user/WSP_OR_APPLICATION",
        _ => "unknown",
    }
}

fn wsp_capability(connect: &WspConnectRequest, id: u8) -> Option<&WspCapability> {
    connect.capabilities.iter().find(|capability| capability.id == id)
}

pub(crate) fn wsp_capability_uintvar(connect: &WspConnectRequest, id: u8) -> Option<usize> {
    let capability = wsp_capability(connect, id)?;
    let (value, len) = read_uintvar(&capability.parameters)?;
    (len == capability.parameters.len()).then_some(value)
}

fn push_wsp_uintvar_capability(out: &mut Vec<u8>, capability_id: u8, value: usize) {
    let mut encoded_value = Vec::new();
    write_uintvar(value, &mut encoded_value);
    push_wsp_octets_capability(out, capability_id, &encoded_value);
}

fn push_wsp_octets_capability(out: &mut Vec<u8>, capability_id: u8, value: &[u8]) {
    write_uintvar(1 + value.len(), out);
    out.push(capability_id);
    out.extend_from_slice(value);
}

fn build_wsp_reply(content_type: u8, page: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(WSP_REPLY_FIXED_HEADER_BYTES + page.len());
    payload.push(WSP_PDU_REPLY);
    payload.push(WSP_STATUS_OK);
    write_uintvar(1, &mut payload);
    payload.push(content_type);
    payload.extend_from_slice(page);
    payload
}

pub(crate) fn read_uintvar(buf: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0usize;
    for (idx, octet) in buf.iter().copied().enumerate().take(5) {
        value = value.checked_shl(7)?.checked_add((octet & 0x7f) as usize)?;
        if octet & 0x80 == 0 {
            return Some((value, idx + 1));
        }
    }
    None
}

pub(crate) fn write_uintvar(mut value: usize, out: &mut Vec<u8>) {
    let mut stack = [0u8; 5];
    let mut idx = stack.len();
    loop {
        idx -= 1;
        stack[idx] = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    let continuation_end = stack.len() - 1;
    for byte in &mut stack[idx..continuation_end] {
        *byte |= 0x80;
    }
    out.extend_from_slice(&stack[idx..]);
}

fn normalize_get_path(path: &str) -> String {
    let path = path.trim();
    let path = path.split(['?', '#']).next().unwrap_or(path).trim();
    let path = path.strip_prefix("./").unwrap_or(path);
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn uri_path(uri: &str) -> &str {
    let uri = uri.trim();
    if let Some(rest) = uri.strip_prefix("http://").or_else(|| uri.strip_prefix("https://")) {
        return rest.find('/').map(|idx| &rest[idx..]).unwrap_or("/");
    }
    uri
}

fn uri_query(uri: &str) -> Option<&str> {
    let uri = uri.trim();
    let after_fragment = uri.split('#').next().unwrap_or(uri);
    after_fragment.split_once('?').map(|(_, query)| query)
}

fn status_sector_from_uri(uri: &str) -> Option<usize> {
    let query = uri_query(uri)?;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == WAP_STATUS_SECTOR_QUERY || key.eq_ignore_ascii_case("sector") || key.eq_ignore_ascii_case("block") {
            return value.parse::<usize>().ok();
        }
    }
    None
}

fn is_status_path(path: &str, policy: &WapIpServicePolicy) -> bool {
    match path {
        "/" => policy.accept_root_path,
        "/status" => policy.accept_status_path,
        WAP_STATUS_REFRESH_PATH | WAP_STATUS_HTML_PATH => policy.accept_status_path,
        WAP_STATUS_LEGACY_WML_PATH => policy.accept_status_wml_path,
        _ => false,
    }
}

fn status_format_from_path(path: &str) -> WapStatusFormat {
    match path {
        WAP_STATUS_LEGACY_WML_PATH => WapStatusFormat::Wml,
        _ => WapStatusFormat::Xhtml,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sndcp::ip::{IPV4_PROTOCOL_TCP, build_ipv4_tcp_npdu, parse_ipv4_packet, parse_udp_datagram};

    fn snapshot() -> WapStatusSnapshot {
        WapStatusSnapshot {
            title: "Nexus-BS".to_string(),
            stack_version: "v0.1.69_dev-test".to_string(),
            service_state: "ON AIR".to_string(),
            registered_ms: 4,
            active_calls: 0,
            active_group_calls: 0,
            active_private_calls: 0,
            queued_sds: 1,
            uptime_secs: 125,
            last_activity: None,
            health_summary: Some("OK".to_string()),
            health_lines: vec!["CORE OK".to_string(), "RF OK".to_string(), "SDS OK".to_string()],
            radio_lines: vec!["MS 2260618 -47dB G1 SA".to_string()],
            call_lines: Vec::new(),
        }
    }

    fn policy() -> WapIpServicePolicy {
        WapIpServicePolicy::experimental_status()
    }

    fn hex_octets(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0);
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let hi = (pair[0] as char).to_digit(16).expect("test hex high nibble");
                let lo = (pair[1] as char).to_digit(16).expect("test hex low nibble");
                ((hi << 4) | lo) as u8
            })
            .collect()
    }

    fn mxp600_wtp_wsp_connect_payload() -> Vec<u8> {
        hex_octets(concat!(
            "0b13cc1201101d8264048094800004819480000282f0028303028401098610782d75702d3100",
            "456e636f64696e672d76657273696f6e00312e33008094809580b380a3806170706c696361",
            "74696f6e2f766e642e70686f6e65636f6d2e6d6d632d7762786d6c00806170706c696361",
            "74696f6e2f6f637465742d73747265616d00808380746578742f6373730080696d616765",
            "2f626d7000809d809e80a080a180ae80b080b2806170706c69636174696f6e2f766e642e",
            "7761702e7868746d6c2b786d6c00801f3e6170706c69636174696f6e2f7868746d6c2b",
            "786d6c0070726f66696c650022687474703a2f2f7777772e776170666f72756d2e6f7267",
            "2f7868746d6c2200808280696d6167652f782d75702d77706e6700806170706c69636174",
            "696f6e2f766e642e75706c616e65742e6265617265722d63686f6963652d7762786d6c00",
            "01a94d4f542d4d58503630305c4d52323032362e312055502e42726f777365722f362e",
            "332e302e31202847554929204d4d502f322e3000bbea83656e2d67620083028001"
        ))
    }

    fn build_wtp_wsp_get_payload(transaction_id: u16, uri: &str) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.extend_from_slice(&(transaction_id & WTP_TID_VALUE_MASK).to_be_bytes());
        payload.push(0x12);
        payload.push(WSP_PDU_GET);
        write_uintvar(uri.len(), &mut payload);
        payload.extend_from_slice(uri.as_bytes());
        payload
    }

    fn build_wtp_wsp_resume_payload(transaction_id: u16, session_id: usize) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.extend_from_slice(&(transaction_id & WTP_TID_VALUE_MASK).to_be_bytes());
        payload.push(0x12);
        payload.push(WSP_PDU_RESUME);
        write_uintvar(session_id, &mut payload);
        write_uintvar(0, &mut payload);
        payload
    }

    fn build_wtp_wsp_connect_payload(transaction_id: u16, capabilities: &[u8]) -> Vec<u8> {
        let mut wsp = Vec::new();
        wsp.push(WSP_PDU_CONNECT);
        wsp.push(0x10);
        write_uintvar(capabilities.len(), &mut wsp);
        write_uintvar(0, &mut wsp);
        wsp.extend_from_slice(capabilities);

        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.extend_from_slice(&(transaction_id & WTP_TID_VALUE_MASK).to_be_bytes());
        payload.push(0x12);
        payload.extend_from_slice(&wsp);
        payload
    }

    fn prepend_short_wtp_tpi(mut payload: Vec<u8>) -> Vec<u8> {
        assert!(payload.len() >= 4, "test helper expects a WTP Invoke fixed header");
        payload[0] |= WTP_CON_FLAG;
        payload.splice(4..4, [0x01, 0x00]);
        payload
    }

    #[test]
    fn wap_udp_request_classifier_accepts_only_mvp_safe_requests() {
        assert_eq!(parse_wap_udp_request(b"", &policy()), Ok(WapUdpRequestKind::Empty));
        assert_eq!(
            parse_wap_udp_request(b"GET / HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml?s=2 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: Some(2),
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml?block=3 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: Some(3),
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET status.xhtml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET ./status.xhtml HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.html HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml?refresh=1 HTTP/1.0\r\n\r\n", &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Wml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /admin HTTP/1.0\r\n\r\n", &policy()),
            Err(WapIpError::UnsupportedWapPath {
                path: "/admin".to_string()
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x01, 0x40, 0x00], &policy()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 3 })
        );
        assert_eq!(
            parse_wap_udp_request(b"POST /", &policy()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 6 })
        );
    }

    #[test]
    fn wap_udp_request_classifier_accepts_terminal_browser_headers() {
        let mut request = b"GET /status.xhtml HTTP/1.1\r\nHost: 10.0.0.1:9200\r\nUser-Agent: MXP600 WAP Browser\r\nAccept: ".to_vec();
        request.resize(392, b'a');
        request.extend_from_slice(b"\r\n");
        assert_eq!(request.len(), 394);

        assert_eq!(
            parse_wap_udp_request(&request, &policy()),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_accepts_mxp600_wtp_wsp_connect() {
        assert_eq!(
            parse_wap_udp_request(&mxp600_wtp_wsp_connect_payload(), &policy()),
            Ok(WapUdpRequestKind::WtpWspConnect {
                transaction_id: 0x13cc,
                retransmission: true
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_skips_wtp_tpis_before_wsp() {
        let connect = prepend_short_wtp_tpi(build_wtp_wsp_connect_payload(0x1234, &[]));
        assert_eq!(
            parse_wap_udp_request(&connect, &policy()),
            Ok(WapUdpRequestKind::WtpWspConnect {
                transaction_id: 0x1234,
                retransmission: false
            })
        );

        let get = prepend_short_wtp_tpi(build_wtp_wsp_get_payload(0x1235, "status.xhtml"));
        assert_eq!(
            parse_wap_udp_request(&get, &policy()),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1235,
                retransmission: false,
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_accepts_wtp_wsp_get_status_path() {
        assert_eq!(
            parse_wap_udp_request(
                &build_wtp_wsp_get_payload(0x1234, "http://10.0.0.1:9200/status.xhtml?refresh=1"),
                &policy()
            ),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false,
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(
                &build_wtp_wsp_get_payload(0x1234, "http://10.0.0.1:9200/status.xhtml?s=1"),
                &policy()
            ),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false,
                sector: Some(1),
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(&build_wtp_wsp_get_payload(0x1234, "status.xhtml"), &policy()),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false,
                sector: None,
                format: WapStatusFormat::Xhtml
            })
        );
        assert_eq!(
            parse_wap_udp_request(&build_wtp_wsp_get_payload(0x1234, "/status.wml?s=2"), &policy()),
            Ok(WapUdpRequestKind::WtpWspStatus {
                transaction_id: 0x1234,
                retransmission: false,
                sector: Some(2),
                format: WapStatusFormat::Wml
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_accepts_wtp_wsp_resume() {
        assert_eq!(
            parse_wap_udp_request(&build_wtp_wsp_resume_payload(0x13f5, 1), &policy()),
            Ok(WapUdpRequestKind::WtpWspResume {
                transaction_id: 0x13f5,
                session_id: 1,
                retransmission: false
            })
        );
    }

    #[test]
    fn wap_udp_request_classifier_treats_wtp_ack_as_no_response() {
        assert_eq!(
            parse_wap_udp_request(&[0x18, 0x13, 0xcc], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ACK,
                abort: None,
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x1c, 0x13, 0xcc], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ACK,
                abort: None,
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x20, 0x13, 0xcc, 0x01], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo { abort_type: 0, reason: 1 }),
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x21, 0x13, 0xcc, 0xe0], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo {
                    abort_type: 1,
                    reason: 0xe0,
                }),
            })
        );
        assert_eq!(
            parse_wap_udp_request(&[0x27, 0x13, 0xcc, 0xe0], &policy()),
            Ok(WapUdpRequestKind::WtpControlNoResponse {
                transaction_id: 0x13cc,
                pdu_type: WTP_PDU_ABORT,
                abort: Some(WtpAbortInfo {
                    abort_type: 7,
                    reason: 0xe0,
                }),
            })
        );
        assert_eq!(
            wtp_abort_reason_name(WtpAbortInfo { abort_type: 0, reason: 1 }),
            "provider/PROTOERR"
        );
        assert_eq!(
            wtp_abort_reason_name(WtpAbortInfo {
                abort_type: 1,
                reason: 0xe0,
            }),
            "user/WSP_PROTOERR"
        );
    }

    #[test]
    fn wap_udp_request_policy_is_default_deny_and_path_scoped() {
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml HTTP/1.0\r\n\r\n", &WapIpServicePolicy::default()),
            Err(WapIpError::StatusServiceDisabled)
        );

        let status_wml_only = WapIpServicePolicy {
            status_enabled: true,
            accept_status_wml_path: true,
            ..WapIpServicePolicy::default()
        };
        assert_eq!(parse_wap_udp_request(b"", &status_wml_only), Err(WapIpError::EmptyProbeDisabled));
        assert_eq!(
            parse_wap_udp_request(b"GET / HTTP/1.0\r\n\r\n", &status_wml_only),
            Err(WapIpError::UnsupportedWapPath { path: "/".to_string() })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml#s HTTP/1.0\r\n\r\n", &status_wml_only),
            Ok(WapUdpRequestKind::Status {
                sector: None,
                format: WapStatusFormat::Wml
            })
        );
        assert_eq!(
            parse_wap_udp_request(b"GET /status.xhtml HTTP/1.0\r\n\r\n", &status_wml_only),
            Err(WapIpError::UnsupportedWapPath {
                path: "/status.xhtml".to_string()
            })
        );
    }

    #[test]
    fn wap_udp_request_policy_bounds_probe_size() {
        let tiny = WapIpServicePolicy {
            status_enabled: true,
            accept_status_wml_path: true,
            max_request_payload_bytes: 8,
            ..WapIpServicePolicy::default()
        };

        assert_eq!(
            parse_wap_udp_request(b"GET /status.wml HTTP/1.0\r\n\r\n", &tiny),
            Err(WapIpError::UdpPayloadTooLarge { len: 28, max: 8 })
        );
    }

    #[test]
    fn wap_policy_can_allowlist_status_issis() {
        let open = WapIpServicePolicy::experimental_status();
        assert!(open.allows_issi(2_260_618));
        assert!(open.allows_issi(2_260_082));

        let restricted = WapIpServicePolicy::experimental_status_for_issis(vec![2_260_618]);
        assert!(restricted.allows_issi(2_260_618));
        assert!(!restricted.allows_issi(2_260_082));
    }

    #[test]
    fn wap_status_response_swaps_ipv4_udp_endpoints() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WAP response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 226]);
        assert_eq!(response_ip.identification, 0x2223);
        assert_eq!(response_ip.ttl, 32);
        assert!(
            response.len() <= IPV4_UDP_HEADER_BYTES + DEFAULT_WAP_STATUS_MAX_BYTES,
            "fallback WAP status response N-PDU should remain one-slot safe: {} bytes",
            response.len()
        );
        assert_eq!(response_udp.source_port, endpoint.port);
        assert_eq!(response_udp.destination_port, 49152);
        let page = std::str::from_utf8(response_udp.payload).expect("WAP status page should be UTF-8");
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("<body>"));
        assert!(!page.contains("text=\"#0f0\""));
        assert!(!page.contains("<wml"));
        assert!(!page.contains("<card"));
        assert!(page.contains("Nexus-BS: OK"));
        assert!(page.contains("Version: 0.1.69"));
        assert!(page.contains("Uptime "));
        assert!(!page.contains("Voice"));
        assert!(
            page.matches("<br />").count() > 3,
            "WSP status body should use the one-slot text budget, not the old tiny page: {page:?}"
        );
        assert!(!page.contains("<br/>"));
        assert!(page.len() <= DEFAULT_WAP_STATUS_MAX_BYTES);
        assert!(page.len() > 128, "default WAP status body should exceed the legacy 128-byte cap");
    }

    #[test]
    fn wap_status_response_rejects_tcp_for_udp_wap_profile() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_tcp_npdu(
            [10, 0, 0, 226],
            endpoint.address,
            49152,
            endpoint.port,
            0x0102_0304,
            0,
            0x002,
            2048,
            b"",
            0x3333,
            64,
        )
        .expect("TCP SYN N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::UnsupportedIpProtocol {
                protocol: IPV4_PROTOCOL_TCP
            })
        );
    }

    #[test]
    fn wap_status_response_answers_mxp600_wtp_wsp_connect() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            &mxp600_wtp_wsp_connect_payload(),
            0x2222,
            64,
        )
        .expect("WSP Connect request N-PDU should build");

        let response =
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP Connect response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(response_ip.source, endpoint.address);
        assert_eq!(response_ip.destination, [10, 0, 0, 2]);
        assert_eq!(response_udp.source_port, endpoint.port);
        assert_eq!(response_udp.destination_port, 49152);
        assert_eq!(&response_udp.payload[..3], &[WTP_RESULT_LAST_PACKET_OF_MESSAGE, 0x93, 0xcc]);
        assert_eq!(response_udp.payload[3], WSP_PDU_CONNECT_REPLY);
        assert_eq!(response_udp.payload[4], 0x01);
        assert_eq!(
            &response_udp.payload[5..],
            &[
                0x08,
                0x00,
                0x03,
                WSP_CAP_CLIENT_SDU_SIZE,
                0x84,
                0x21,
                0x03,
                WSP_CAP_SERVER_SDU_SIZE,
                0x84,
                0x21
            ]
        );
    }

    #[test]
    fn wap_status_response_negotiates_wsp_connect_reply_capabilities() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let mut capabilities = Vec::new();
        push_wsp_uintvar_capability(&mut capabilities, WSP_CAP_CLIENT_SDU_SIZE, 2000);
        push_wsp_uintvar_capability(&mut capabilities, WSP_CAP_SERVER_SDU_SIZE, 1600);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_PROTOCOL_OPTIONS, &[0xf8]);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_METHOD_MOR, &[3]);
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_EXTENDED_METHODS, b"\x50STATUS\0\x70POSTX\0");
        push_wsp_octets_capability(&mut capabilities, WSP_CAP_HEADER_CODE_PAGES, b"\x78x-up-1\0");
        let request_payload = build_wtp_wsp_connect_payload(0x1234, &capabilities);
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP Connect request N-PDU should build");

        let response =
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP Connect response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(&response_udp.payload[..3], &[WTP_RESULT_LAST_PACKET_OF_MESSAGE, 0x92, 0x34]);
        assert_eq!(response_udp.payload[3], WSP_PDU_CONNECT_REPLY);
        assert_eq!(response_udp.payload[4], 0x01);
        let (capabilities_len, cap_len_octets) =
            read_uintvar(&response_udp.payload[5..]).expect("ConnectReply capabilities length should parse");
        let headers_len_start = 5 + cap_len_octets;
        let (headers_len, headers_len_octets) =
            read_uintvar(&response_udp.payload[headers_len_start..]).expect("ConnectReply headers length should parse");
        let capabilities_start = headers_len_start + headers_len_octets;
        let capabilities_end = capabilities_start + capabilities_len;
        assert_eq!(headers_len, 0);
        assert_eq!(capabilities_end, response_udp.payload.len());
        let negotiated = parse_wsp_capabilities(&response_udp.payload[capabilities_start..capabilities_end])
            .expect("ConnectReply capabilities should parse");

        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_CLIENT_SDU_SIZE)
                .and_then(|cap| read_uintvar(&cap.parameters).map(|(value, _)| value)),
            Some(WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES)
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_SERVER_SDU_SIZE)
                .and_then(|cap| read_uintvar(&cap.parameters).map(|(value, _)| value)),
            Some(WSP_CONNECT_REPLY_SERVER_SDU_SIZE_BYTES)
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_PROTOCOL_OPTIONS)
                .map(|cap| cap.parameters.as_slice()),
            None,
            "ConnectReply must not synthesize protocol options"
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_METHOD_MOR)
                .map(|cap| cap.parameters.as_slice()),
            None,
            "ConnectReply must not synthesize method MOR"
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_EXTENDED_METHODS)
                .map(|cap| cap.parameters.as_slice()),
            None,
            "ConnectReply must not synthesize extended methods"
        );
        assert_eq!(
            negotiated
                .iter()
                .find(|cap| cap.id == WSP_CAP_HEADER_CODE_PAGES)
                .map(|cap| cap.parameters.as_slice()),
            None,
            "ConnectReply must not synthesize extension header code pages"
        );
    }

    #[test]
    fn wap_status_response_suppresses_wtp_ack_control_pdu() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            &[0x18, 0x13, 0xcc],
            0x2222,
            64,
        )
        .expect("WTP ACK request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu_optional_with_npdu_budget(&request, endpoint, &policy(), &snapshot(), Some(576)),
            Ok(None)
        );
        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::NoResponseRequired)
        );
    }

    #[test]
    fn wap_status_response_handles_connect_resume_then_get_sequence() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };

        let connect_payload = build_wtp_wsp_connect_payload(0x1234, &[0x03, WSP_CAP_CLIENT_SDU_SIZE, 0x8a, 0x78]);
        let connect_request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &connect_payload, 0x2222, 64)
            .expect("WSP Connect request N-PDU should build");
        let connect_response =
            build_wap_status_response_npdu(&connect_request, endpoint, &policy(), &snapshot()).expect("WSP Connect response should build");
        let connect_response_ip = parse_ipv4_packet(&connect_response).expect("ConnectReply IPv4 should parse");
        let connect_response_udp = parse_udp_datagram(connect_response_ip.payload).expect("ConnectReply UDP should parse");
        assert_eq!(
            &connect_response_udp.payload[..4],
            [WTP_RESULT_LAST_PACKET_OF_MESSAGE, 0x92, 0x34, WSP_PDU_CONNECT_REPLY],
            "non-fragmented WSP ConnectReply must be a WTP Result last packet"
        );

        let ack_request = build_ipv4_udp_npdu(
            [10, 0, 0, 2],
            endpoint.address,
            49152,
            endpoint.port,
            &[WTP_PDU_ACK << 3, 0x12, 0x34],
            0x2223,
            64,
        )
        .expect("WTP ACK request N-PDU should build");
        assert_eq!(
            build_wap_status_response_npdu_optional_with_npdu_budget(&ack_request, endpoint, &policy(), &snapshot(), Some(576)),
            Ok(None),
            "WTP ACK after ConnectReply is terminal control traffic and must not consume the page response"
        );

        let resume_payload = build_wtp_wsp_resume_payload(0x13f5, 1);
        let resume_request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 8502, endpoint.port, &resume_payload, 0x2224, 64)
            .expect("WSP Resume request N-PDU should build");
        let resume_response =
            build_wap_status_response_npdu(&resume_request, endpoint, &policy(), &snapshot()).expect("WSP Resume response should build");
        let resume_response_ip = parse_ipv4_packet(&resume_response).expect("Resume response IPv4 should parse");
        let resume_response_udp = parse_udp_datagram(resume_response_ip.payload).expect("Resume response UDP should parse");
        assert_eq!(
            resume_response_udp.payload,
            [WTP_RESULT_LAST_PACKET_OF_MESSAGE, 0x93, 0xf5, WSP_PDU_REPLY, WSP_STATUS_OK, 0,],
            "WSP Resume should receive a WTP Result + empty WSP Reply OK"
        );

        let get_payload = build_wtp_wsp_get_payload(0x1235, "/status.xhtml");
        let get_request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &get_payload, 0x2225, 64)
            .expect("WSP GET request N-PDU should build");
        let get_response =
            build_wap_status_response_npdu(&get_request, endpoint, &policy(), &snapshot()).expect("WSP GET response should build");
        let get_response_ip = parse_ipv4_packet(&get_response).expect("GET response IPv4 should parse");
        let get_response_udp = parse_udp_datagram(get_response_ip.payload).expect("GET response UDP should parse");
        assert_eq!(
            &get_response_udp.payload[..7],
            [
                WTP_RESULT_LAST_PACKET_OF_MESSAGE,
                0x92,
                0x35,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ],
            "GET /status.xhtml after WSP Connect should receive an XHTML WSP Reply"
        );
    }

    #[test]
    fn wap_status_response_suppresses_wtp_abort_control_pdu() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        for abort_payload in [&[0x20, 0x13, 0xcc, 0x01][..], &[0x21, 0x13, 0xcc, 0xe0][..]] {
            let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, abort_payload, 0x2222, 64)
                .expect("WTP Abort request N-PDU should build");

            assert_eq!(
                build_wap_status_response_npdu_optional_with_npdu_budget(&request, endpoint, &policy(), &snapshot(), Some(576)),
                Ok(None)
            );
            assert_eq!(
                build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
                Err(WapIpError::NoResponseRequired)
            );
        }
    }

    #[test]
    fn wap_status_response_answers_wtp_wsp_get_with_xhtml_reply() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml");
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP GET request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP GET response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_LAST_PACKET_OF_MESSAGE,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ]
        );
        assert_eq!(response_udp.payload[5], 1, "WSP Reply HeadersLen should contain only ContentType");
        assert_eq!(
            response_udp.payload[6], WSP_CT_APP_VND_WAP_XHTML_XML,
            "WSP Reply ContentType should be application/vnd.wap.xhtml+xml as a short-integer"
        );
        assert!(
            response_udp.payload[3..].len() <= WSP_CONNECT_REPLY_CLIENT_SDU_SIZE_BYTES,
            "WSP Reply SDU should not exceed negotiated Client-SDU-Size: {} bytes",
            response_udp.payload[3..].len()
        );
        assert!(
            response_udp.payload.len() <= DEFAULT_WAP_WSP_STATUS_MAX_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES,
            "WSP response payload should fit the negotiated one-slot WAP budget: {} bytes",
            response_udp.payload.len()
        );
        assert!(
            response.len() <= IPV4_UDP_HEADER_BYTES + DEFAULT_WAP_WSP_STATUS_MAX_BYTES + 3 + WSP_REPLY_FIXED_HEADER_BYTES,
            "WSP response N-PDU should stay small enough for AL delivery: {} bytes",
            response.len()
        );
        let page = std::str::from_utf8(&response_udp.payload[7..]).expect("WSP XHTML body should be UTF-8");
        assert!(page.contains("<body>"));
        assert!(!page.contains("text=\"#0f0\""));
        assert!(page.contains("Welcome to Nexus-BS!"), "page={page:?}");
        assert!(page.contains("v0.1.69"), "page={page:?}");
        assert!(page.contains("href=\"/status.wml?s=1\""));
        assert!(
            page.len() <= WSP_OPENWAVE_XHTML_BROWSER_INDEX_MAX_BYTES,
            "WSP browser index should stay compact: {} bytes",
            page.len()
        );
        assert!(!page.contains("Voice"));
        assert!(page.contains("<br/>"));
        assert!(!page.contains("<wml"));
    }

    #[test]
    fn wap_status_response_answers_wtp_wsp_get_sector_with_xhtml_links() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml?s=1");
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP sector GET request N-PDU should build");

        let response =
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP sector response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
        let page = std::str::from_utf8(&response_udp.payload[7..]).expect("WSP XHTML body should be UTF-8");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_LAST_PACKET_OF_MESSAGE,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ]
        );
        assert!(
            response.len() <= 576,
            "sector response N-PDU must stay within negotiated MTU: {}",
            response.len()
        );
        assert!(
            page.len() <= WSP_OPENWAVE_XHTML_BROWSER_SECTOR_MAX_BYTES,
            "WSP browser sector should stay compact: {} bytes",
            page.len()
        );
        assert!(!page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("General OK"), "page={page:?}");
        assert!(page.contains("2/"));
        assert!(page.contains("href=\"?s=0\""));
        assert!(page.contains("href=\"/\""));
        assert!(!page.contains("<script"));
        assert!(!page.contains("<wml"));
    }

    #[test]
    fn wap_status_response_answers_wtp_wsp_get_wml_sector_with_wml_reply() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request_payload = build_wtp_wsp_get_payload(0x1234, "/status.wml?s=1");
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP WML sector GET request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP WML response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
        let page = std::str::from_utf8(&response_udp.payload[7..]).expect("WSP WML body should be UTF-8");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_LAST_PACKET_OF_MESSAGE,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_TEXT_VND_WAP_WML,
            ]
        );
        assert!(
            response.len() <= 576,
            "WML sector response N-PDU must stay within negotiated MTU: {}",
            response.len()
        );
        assert!(
            page.len() <= WSP_OPENWAVE_WML_BROWSER_PAGE_MAX_BYTES,
            "WSP WML sector should stay compact: {} bytes",
            page.len()
        );
        assert!(page.contains("<wml>"), "page={page:?}");
        assert!(page.contains("General OK"), "page={page:?}");
        assert!(page.contains("href=\"?s=0\""));
        assert!(page.contains("href=\"/\""));
        assert!(!page.contains("http://www.w3.org/1999/xhtml"));
    }

    #[test]
    fn wap_status_response_keeps_wtp_result_rid_clear_for_retransmitted_get() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let mut request_payload = build_wtp_wsp_get_payload(0x1234, "/status.xhtml");
        request_payload[0] |= WTP_RID_FLAG;
        let request = build_ipv4_udp_npdu([10, 0, 0, 2], endpoint.address, 49152, endpoint.port, &request_payload, 0x2222, 64)
            .expect("WSP GET request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()).expect("WSP GET response should build");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");

        assert_eq!(
            &response_udp.payload[..7],
            [
                WTP_RESULT_LAST_PACKET_OF_MESSAGE,
                0x92,
                0x34,
                WSP_PDU_REPLY,
                WSP_STATUS_OK,
                0x01,
                WSP_CT_APP_VND_WAP_XHTML_XML,
            ]
        );
    }

    #[test]
    fn wap_status_response_accepts_empty_udp_probe_for_mvp() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"", 0x2222, 64)
            .expect("request N-PDU should build");

        let response = build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot())
            .expect("empty WAP UDP probe should build status response");
        let response_ip = parse_ipv4_packet(&response).expect("response IPv4 should parse");
        let response_udp = parse_udp_datagram(response_ip.payload).expect("response UDP should parse");
        let page = std::str::from_utf8(response_udp.payload).unwrap();
        assert!(page.contains("http://www.w3.org/1999/xhtml"));
        assert!(page.contains("<body>"));
        assert!(!page.contains("text=\"#0f0\""));
        assert!(page.contains("Nexus-BS: OK"));
        assert!(page.contains("Version: 0.1.69"));
        assert!(page.contains("Uptime 0d0h2m5s"));
        assert!(!page.contains("Voice"));
        assert!(
            page.matches("<br />").count() > 3,
            "empty-probe status body should use the one-slot text budget, not the old tiny page: {page:?}"
        );
        assert!(page.len() > 128, "empty-probe status body should exceed the legacy 128-byte cap");
        assert!(!page.contains("<br/>"));
    }

    #[test]
    fn wap_status_response_rejects_when_policy_disables_status() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, endpoint.port, b"GET /", 0x2222, 64)
            .expect("request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &WapIpServicePolicy::default(), &snapshot()),
            Err(WapIpError::StatusServiceDisabled)
        );
    }

    #[test]
    fn wap_status_response_rejects_unsupported_udp_payloads() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let request = build_ipv4_udp_npdu(
            [10, 0, 0, 226],
            endpoint.address,
            49152,
            endpoint.port,
            &[0x01, 0x40, 0x00],
            0x2222,
            64,
        )
        .expect("request N-PDU should build");

        assert_eq!(
            build_wap_status_response_npdu(&request, endpoint, &policy(), &snapshot()),
            Err(WapIpError::UnsupportedWapUdpPayload { len: 3 })
        );
    }

    #[test]
    fn wap_status_response_rejects_non_wap_destination() {
        let endpoint = WapIpEndpoint {
            address: [10, 0, 0, 1],
            port: 9200,
            response_ttl: 32,
        };
        let wrong_address =
            build_ipv4_udp_npdu([10, 0, 0, 226], [10, 0, 0, 2], 49152, endpoint.port, b"GET /", 1, 64).expect("request N-PDU should build");
        assert_eq!(
            build_wap_status_response_npdu(&wrong_address, endpoint, &policy(), &snapshot()),
            Err(WapIpError::WrongDestination {
                expected: endpoint.address,
                actual: [10, 0, 0, 2]
            })
        );

        let wrong_port =
            build_ipv4_udp_npdu([10, 0, 0, 226], endpoint.address, 49152, 9201, b"GET /", 1, 64).expect("request N-PDU should build");
        assert_eq!(
            build_wap_status_response_npdu(&wrong_port, endpoint, &policy(), &snapshot()),
            Err(WapIpError::WrongUdpPort {
                expected: endpoint.port,
                actual: 9201
            })
        );
    }
}
