<!-- WAP port specification for FlowStation. Clean-room behavioral contract derived from
     the nexus-bs WAP wire behavior + ETSI/OMA specs + captured MXP600 vectors.
     nexus WAP source is PolyForm-Noncommercial: this doc captures protocol BEHAVIOR/wire
     format only (facts), NOT nexus code. Reimplement fresh; do not transcribe nexus source. -->

# FlowStation WAP Port Specification — Byte-Exact Reimplementation Contract

Derived from nexus-bs WAP/SNDCP. Every layout/constant cites nexus `file:line`. Verified against source: `pdp.rs:201-233` ACCEPT encoder, `pdp.rs:427-449,510-528` timer/MTU ranges, `pdp.rs:594-606` ACCEPT test, FlowStation `sndcp_bs.rs` (full read), and IP checksums (`0x536d`, `0x16c4`) recomputed and confirmed.

---

## 0. Clean-room ground rule

FlowStation reimplements WAP/SNDCP from **this specification + ETSI EN 300 392-2 clause 28 (SNDCP) / EN 300 392-7 + OMA WAP-WSP/WTP/WAE + RFC 768/791/793/1071/1994 + the captured terminal vectors in §7**, which are the ground-truth on-air bytes. nexus's WAP code is PolyForm-Noncommercial; we extract only the **protocol behavior and wire format (facts, not copyrightable expression)**. Do **not** transcribe nexus source — write fresh code that emits byte-identical on-air output and passes the §7 conformance fixtures. The field layouts, constants, and vectors below are sufficient to build without ever reopening nexus.

---

## 1. Full request/response sequence (PDP activation → rendered page)

A complete MXP600 (Motorola/Dimetra, Openwave UP.Browser 6.3.0.1) packet-data session. "BS" = SwMI/FlowStation. All SN-PDUs travel under a 3-bit MLE protocol discriminator `0b100` (SNDCP) on the air; below the SN-PDU bit content is what each layer must produce.

| # | Direction | PDU / event | What the BS must do |
|---|-----------|-------------|---------------------|
| 1 | MS → BS | **SN-ACTIVATE PDP CONTEXT DEMAND** (SN type 0). Carries SNDCP ver=1, NSAPI, ATID (0=static IPv4, 1=dynamic), packet-data-MS-type, PCOMP=0, **PCO with CHAP Response** (Motorola). | Decode header; classify ATID. |
| 2 | BS → MS | **SN-ACTIVATE PDP CONTEXT ACCEPT** (SN type 0). NSAPI echoed; TIA=1 (static, echo MS IPv4) or TIA=2 (dynamic, assign pool IP); timers/MTU; **PCO CHAP-Success** if DEMAND had CHAP, else O-bit=0. | Sent on **acknowledged basic link** (`TlaTlDataReqBl`) to `ind.received_tetra_address`. Context state → STANDBY. |
| 3 | MS → BS | **SN-DATA TRANSMIT REQUEST** (SN type 6). Optional phase-modulation resource request (1 or 4 slots). | Build **SN-DATA TRANSMIT RESPONSE** (type 7, Accept). Attach **CmceChanAllocReq**: TS2 single-slot, Replace, Both. Context → READY. |
| 4 | BS → MS | **SN-DATA TRANSMIT RESPONSE** + PDCH channel allocation (timeslots `[F,T,F,F]`). | MS moves to assigned PDCH on TS2. |
| 5 | MS → BS | **SN-UNITDATA** (type 4) *or* **SN-DATA** (type 1) carrying an **IPv4/UDP** N-PDU to `BS_IP:9200`. Payload = WTP Invoke+WSP Connect, or WSP GET, or plain-text `GET /…`. | Decode SN body; extract N-PDU; parse IPv4→UDP; classify WAP request. |
| 6 | (within 5) | **WSP CONNECT** (WTP Invoke) | Reply **WSP CONNECT-REPLY** in a WTP Result. |
| 7 | (within 5) | **WSP GET /status.xhtml** (WTP Invoke) | Render page; reply **WSP Reply** (200 OK + content-type) in a WTP Result. |
| 8 | BS → MS | **SN-UNITDATA** (type 4 — *always*, even if request was type 1) wrapping IPv4/UDP, src/dst swapped, IP id = req+1, UDP cksum 0. | Built only **after** WAP response succeeds; then READY-state gate runs. |
| 9 | MS → BS | **WTP ACK** or **WTP ABORT** | **No response** (drop). |
| 10 | MS → BS | **SN-DEACTIVATE PDP CONTEXT DEMAND** (type 2) | Reply **SN-DEACTIVATE PDP CONTEXT ACCEPT** (type 1); remove context. |
| — | MS → BS | **SN-END OF DATA** (type 8) | Attach return-to-common-control alloc (timeslots `[F,F,F,F]`, QuitAndGo). |

**Critical ordering (`wap_session.rs:348-370`):** for step 5/8 the full WAP response is **built first**; only then does `prepare_swmi_unitdata_transfer` enforce READY. A non-READY context still runs the whole pipeline and fails last (build errors mask the READY check).

---

## 2. Byte-exact PDU / header layouts

Bit order is **MSB-first / big-endian throughout** the bitstream (`bitbuffer.rs:316-422` write, `630-668` read; round-trip `0xAB,0xCD ⇒ [0xAB,0xCD]` at `bitbuffer.rs:689-696`). `write_bits(v,n)` emits the low `n` bits of `v`, MSB first; the first field written occupies the **high** bits of byte 0. No inter-field byte alignment — PDUs are sub-byte. All IP/UDP multi-byte integers are network byte order (big-endian).

### 2.1 SN-ACTIVATE PDP CONTEXT ACCEPT (downlink) — `pdp.rs:201-233`

| Field | Bits | Value / encoding | Notes |
|---|---|---|---|
| SN PDU type | 4 | `0` | ACCEPT reuses type 0; disambiguated by direction (`pdp.rs:215`) |
| NSAPI | 4 | echo, 1..14 | `pdp.rs:216` |
| PDU priority max | 3 | 0..7 | `pdp.rs:217` |
| Ready timer | 4 | code 1..14 | `pdp.rs:218,427-433` |
| Standby timer | 4 | code 1..15 | `pdp.rs:219,435-441` |
| Response wait timer | 4 | code 0..14 | `pdp.rs:220,443-449` |
| TIA | 3 | 0=NoAddr, 1=IPv4 static, 2=IPv4 dynamic | `pdp.rs:221,493-508` |
| IPv4 address | 32 | **iff TIA∈{1,2}**, octet[0] first, each octet 8 bits MSB-first | `pdp.rs:222-227,375-379` |
| PCOMP negotiation | 8 | `0` only | `pdp.rs:228` |
| MTU code | 3 | 1=296,2=576,3=1006,**4=1500**,5=2002 | `pdp.rs:229,510-528` |
| O-bit | 1 | `0` (no optional elements) | `pdp.rs:230,381-383` |

Dynamic/static IPv4 ACCEPT = **70 bits** (`pdp.rs:648`); NoAddress = 38 bits.

### 2.2 SN-ACTIVATE PDP CONTEXT DEMAND (uplink) — `pdp.rs:129-160`

| Field | Bits | Value | Notes |
|---|---|---|---|
| SN PDU type | 4 | `0` | |
| SNDCP version | 4 | `1` only valid | `pdp.rs:399-405` |
| NSAPI | 4 | 1..14 | |
| ATID | 3 | 0=IPv4 static(+32b IPv4), 1=dynamic, 2=IPv6, 3/4=MIPv4, 5=secondary(+4b primary NSAPI), 6/7 reserved | `pdp.rs:147,451-472` |
| (conditional) | 0/32/4 | IPv4 (ATID 0) / primary NSAPI (ATID 5) | |
| Packet-data MS type | 4 | 0=A,1=B,2=C,3=D, 4..15 reserved | `pdp.rs:155,474-491` |
| PCOMP negotiation | 8 | `0` only | `pdp.rs:156` |
| O-bit | 1 | `0` | |

Dynamic = **28 bits** (`pdp.rs:615`); static = 60; secondary = 32.

### 2.3 SN-UNITDATA (type 4) / SN-DATA (type 5/1) — `unitdata.rs:149-176`

| Field | Bits | Value |
|---|---|---|
| SN PDU type | 4 | 4=UNITDATA (`unitdata.rs:11`); 1=DATA inbound, **downlink always 4** |
| NSAPI | 4 | 1..14 |
| PCOMP | 4 | `0` (`unitdata.rs:12`) |
| DCOMP | 4 | `0` |
| N-PDU | N | raw IP datagram, copied bit-for-bit MSB-first, **no length prefix, no padding** (`unitdata.rs:169-173`) |

Header exactly **16 bits**; total = 16 + N-PDU bits. **No O-bit.** IP version = first nibble of N-PDU (4⇒IPv4, 6⇒IPv6). N-PDU must be non-empty.

### 2.4 IPv4 header (20 bytes, IHL=5, no options) — `ip.rs:206-219`

| Off | Len | Field | Build value |
|---|---|---|---|
| 0 | 1 | Version\|IHL | `0x45` |
| 1 | 1 | DSCP/ECN | `0x00` |
| 2 | 2 | Total Length | BE = 20 + transport_len |
| 4 | 2 | Identification | BE, caller-supplied (= **req_id + 1 wrapping** for WAP responses, `wap_ip.rs:384`) |
| 6 | 2 | Flags\|FragOffset | `0x0000` |
| 8 | 1 | TTL | caller (`endpoint.response_ttl`, =32) |
| 9 | 1 | Protocol | `17` UDP / `6` TCP |
| 10 | 2 | Header Checksum | RFC1071 over 20-byte header (field zeroed first) |
| 12 | 4 | Source | verbatim |
| 16 | 4 | Destination | verbatim |

Parser honors IHL (`packet[0]&0x0f`)×4 and trims to Total Length; does **not** verify checksum.

### 2.5 UDP header (8 bytes) — `ip.rs:221-227`

| Off | Len | Field | Build value |
|---|---|---|---|
| 0 | 2 | Source Port | BE (=9200 for response) |
| 2 | 2 | Dest Port | BE (= request src port) |
| 4 | 2 | Length | BE = 8 + payload_len |
| 6 | 2 | Checksum | **`0x0000` always — never computed** (`ip.rs:224-226`) |
| 8 | N | Payload | verbatim |

### 2.6 TCP header (20 bytes, DataOffset=5) — `ip.rs:269-282`

Off 0/2 ports BE; 4 seq u32; 8 ack u32; 12 = `(5<<4)|((flags>>8)&1)` = `0x50`; 13 = `flags&0xff`; 14 window u16; 16 checksum (**computed** w/ pseudo-header `src4 ++ dst4 ++ {0x00,0x06} ++ be16(seg_len) ++ seg`); 18 urgent `0x0000`. Flags: FIN=0x001 SYN=0x002 RST=0x004 PSH=0x008 ACK=0x010 (`ip.rs:14-18`).

### 2.7 WTP Invoke (request) — `wap_ip.rs:515-541`

| Octet | Field | Encoding |
|---|---|---|
| 0 | flags/type | bit7=CON(`0x80`); bits6..3=PDU type (`>>3 & 0x0f`, Invoke=1); bit0=RID(`0x01`) |
| 1-2 | TID | u16 BE; value = raw `& 0x7fff` |
| 3 | Invoke hdr | bits7..6=Version(must 0); bits3..2=reserved(0); bits1..0=TCL(must **2**). Canonical byte = `0x12` |
| 4.. | WSP | if CON set, skip TPIs first (see below) |

**TPI skip** (`wap_ip.rs:543-565`): per TPI, `cont = h&0x80`, `long = h&0x04`; long ⇒ next = off+2+payload[off+1]; short ⇒ next = off+1+(h&0x03); stop when cont clear.

### 2.8 WTP Result (response) — `wap_ip.rs:620-627`

| Octet | Field | Value |
|---|---|---|
| 0 | type/trailer | **`0x12`** = `(2<<3)|0x02` (Result, last-packet). **RID always clear**, even if request had RID |
| 1-2 | TID | `(req_tid & 0x7fff) \| 0x8000` BE |
| 3.. | WSP PDU | verbatim |

### 2.9 WSP Connect (request) — `wap_ip.rs:567-586`

Octet 0=`0x01`; octet 1=Version (MXP600 sends `0x10`); then **CapabilitiesLength uintvar**, **HeadersLength uintvar**, then `Capabilities[len]` then `Headers[len]`. Each capability: uintvar Length (of rest), 1 octet capability id, (Length−1) param octets.

### 2.10 WSP Connect-Reply — `wap_ip.rs:629-638`

`[0x02, 0x01, uintvar(caps_len), 0x00, <caps>]`. Octet 1 server session id **hardcoded `0x01`**; HeadersLength **`0x00`**. Only caps 0x80 (Client-SDU) and 0x81 (Server-SDU) echoed, each clamped to `min(req, 545)`, encoded `[0x03, id, 0x84, 0x21]` (0x8421 = 545). **All other caps dropped.**

### 2.11 WSP Reply (GET) / Resume-OK — `wap_ip.rs:732-740 / 640-642`

GET: `[0x04, 0x20, 0x01, content_type, body…]` (Reply, OK, HeadersLen=1, content-type short-int). Resume-OK: exactly `[0x04, 0x20, 0x00]`.

**uintvar codec** (`wap_ip.rs:742-769`): big-endian base-128, ≤5 octets; non-final octets have bit7 set; `value = (value<<7)|(octet&0x7f)`; minimal-length except `0 ⇒ 0x00`. Capability uintvar must consume the entire parameter slice or the cap is ignored.

---

## 3. Master constants table

| Name | Value | Meaning | Source |
|---|---|---|---|
| SN type ACTIVATE PDP | 0 | DEMAND & ACCEPT share | pdp.rs:9 |
| SN type DEACTIVATE ACCEPT | 1 | (note: accept < demand) | pdp.rs:10 |
| SN type DEACTIVATE DEMAND | 2 | | pdp.rs:11 |
| SN type ACTIVATE REJECT | 3 | | pdp.rs:12 |
| SN type UNITDATA | 4 | | unitdata.rs:11 |
| SN type DATA | 5 | | transfer.rs:11 |
| SN type DATA TRANSMIT REQ | 6 | | transfer.rs:12 |
| SN type DATA TRANSMIT RESP | 7 | | transfer.rs:13 |
| SN type END OF DATA | 8 | | transfer.rs:14 |
| SN type RECONNECT | 9 | | transfer.rs:15 |
| SN type PAGE REQ/RESP | 10 | | transfer.rs:16 |
| SN type NOT SUPPORTED | 11 | | transfer.rs:17 |
| SN type DATA PRIORITY | 12 | | transfer.rs:18 |
| SN type MODIFY | 13 | | transfer.rs:19 |
| SNDCP_VERSION_1 | 1 | only accepted | pdp.rs:14,400 |
| PCOMP_NEGOTIATION_NONE | 0 | only accepted (PDP) | pdp.rs:15,412 |
| SNDCP_NO_COMPRESSION | 0 | PCOMP/DCOMP in UNITDATA | unitdata.rs:12 |
| IPV4_VERSION / IPV6_VERSION | 4 / 6 | IP version nibble | unitdata.rs:14-15 |
| NSAPI valid | 1..14 | 0,15 reserved | sn.rs:133-141 |
| ATID codes | 0,1,2,3,4,5; 6,7 rsvd | demand address type | pdp.rs:451-472 |
| TIA codes | 0,1,2; 3..7 rsvd | accept type identifier | pdp.rs:493-508 |
| Packet-data MS type | 0=A,1=B,2=C,3=D; 4..15 rsvd | | pdp.rs:474-491 |
| MTU codes | 1=296,2=576,3=1006,4=1500,5=2002; **0/6/7 rsvd** | enum starts at 1 | pdp.rs:510-528 |
| Ready timer range | 1..14 | 0,15 rsvd | pdp.rs:427-433 |
| Standby timer range | 1..15 | 0 rsvd | pdp.rs:435-441 |
| Response-wait range | 0..14 | 15 rsvd | pdp.rs:443-449 |
| PDU priority max | 0..7 | | pdp.rs:419-425 |
| Activation reject causes | 0,1,2,3,4,7,8,9,10,15,16,19,27,28,34; else Other | 8-bit | pdp.rs:538-578 |
| Deactivation type | 0=AllNsapis,1=single(+4b NSAPI); 2..255 rsvd | | pdp.rs:531-536 |
| Resource-req timeslot code | count−1 (0..3 ⇒ 1..4 slots) | | transfer.rs:403-412 |
| Resource-req mean-tput sentinel | `0b110`=6 unspecified PM | symmetric only | transfer.rs:332-339 |
| Resource-req reserved trailer | `0b11` | must equal on decode | transfer.rs:345 |
| **IPV4_PROTOCOL_TCP / UDP** | 6 / 17 | | ip.rs:8-9 |
| IPV4_MIN_HEADER_LEN / TCP / UDP | 20 / 20 / 8 | | ip.rs:10-12 |
| IPv4 byte0 / byte1 / flags-frag | 0x45 / 0x00 / 0x0000 | build constants | ip.rs:207,208,211 |
| UDP checksum (build) | 0x0000 | always | ip.rs:226 |
| TCP byte12 base / urgent | 0x50 / 0x0000 | DataOffset=5 | ip.rs:274,278 |
| WTP_CON_FLAG / RID_FLAG | 0x80 / 0x01 | | wap_ip.rs:16-17 |
| WTP PDU INVOKE/RESULT/ACK/ABORT | 1/2/3/4 | | wap_ip.rs:18-21 |
| WTP_RESULT_LAST_PACKET | 0x12 | =(2<<3)\|0x02 | wap_ip.rs:23 |
| WTP_TID_RESPONSE_FLAG / VALUE_MASK | 0x8000 / 0x7fff | | wap_ip.rs:24-25 |
| WSP PDU CONNECT/CONNECT_REPLY/REPLY/RESUME/GET | 0x01/0x02/0x04/0x09/0x40 | GET also 0x50..0x5f | wap_ip.rs:26-30,478 |
| WSP_STATUS_OK | 0x20 | | wap_ip.rs:31 |
| WSP_CT WML / XHTML | 0x88 (0x80\|0x08) / 0xC5 (0x80\|0x45) | content-type short-int | wap_ip.rs:33-36 |
| WSP cap ids | 0x80,0x81,0x82,0x83,0x85,0x86 | | wap_ip.rs:37-42 |
| WSP server session id | 0x01 | hardcoded | wap_ip.rs:633 |
| WSP_CONNECT_REPLY_CLIENT/SERVER_SDU | 545 / 545 | the "545 cap" | wap_ip.rs:45-46 |
| WSP_REPLY_FIXED_HEADER_BYTES | 4 | | wap_ip.rs:47 |
| DEFAULT_WAP_WSP_STATUS_MAX_BYTES | 541 | =545−4 | wap_ip.rs:14 |
| IPV4_UDP_HEADER_BYTES | 28 | | wap_ip.rs:15 |
| DEFAULT_WAP_UDP_REQUEST_MAX_BYTES | 1024 | | wap_ip.rs:13 |
| WSP_OPENWAVE_WML_BROWSER_PAGE_MAX | 144 | | wap_ip.rs:48 |
| WSP_OPENWAVE_XHTML_INDEX_MAX | 104 | | wap_ip.rs:49 |
| WSP_OPENWAVE_XHTML_SECTOR_MAX | 144 | | wap_ip.rs:50 |
| DEFAULT_WAP_STATUS_MAX_BYTES | 548 | raw-UDP page cap | wap_status.rs:6 |
| MleProtocolDiscriminator::Sndcp | 4 (`0b100`, 3-bit) | MM=1,CMCE=2,MLE=5,TME=6; 0,3,7 rsvd | mle_protocol_discriminator.rs:15 |
| ChanAllocType | Replace=0,Additional=1,QuitAndGo=2,ReplaceWithCarrierSig=3 | 2-bit | alloc_type.rs:10-16 |
| UlDlAssignment | Augmented=0,Dl=1,Ul=2,Both=3 | 2-bit | ul_dl_assignment.rs:10-15 |
| SNDCP_BASIC_LINK_ID | 0 | | pdch.rs:18 |
| SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT | [F,T,F,F] (TS2) | | pdch.rs:32 |
| WAP_IP_MVP_NONFRAG_MAC_CAPACITY_BITS | 124 | | ltpd_pipeline.rs:26 |
| **Config WAP defaults** | addr [10,0,0,1], port 9200, ttl 32, pool 10.0.0.2..254, max_req 1024 | | sec_cell.rs:31-37 |
| SDS text PID default | 0xDC (220) | Motorola/Openwave home-screen | sec_cell.rs:30 |
| **FlowStation-only** (CHAP path) | MLE_DISCRIMINATOR_SNDCP=0b100; POOL_IPV4=0xC0A801B4 (192.168.1.180); PCO_TYPE34_ID=1; PPP_PROTO_CHAP=0xC223; PPP_CONFIG_PROTOCOL_PPP=0; CHAP_CODE_SUCCESS=3; PCO_CHAP_SUCCESS_BITS=60 | no nexus equivalent | sndcp_bs.rs:14,28,34-40 |

> **Timer-code → seconds note:** nexus stores raw 4-bit codes and validates **range only**; it never maps to seconds. FlowStation's comments "ready 8=10s, standby 5=10min, response-wait 8=10s" (`sndcp_bs.rs:20-22`) are FlowStation annotations, **not** verifiable from nexus and **not** part of the wire contract — only the code values and ranges are.

---

## 4. The WML/XHTML-MP page

Bodies are byte-for-byte ASCII (operator text escaped). **No trailing newline.** All facts `wap_status.rs`.

### 4.1 Shared literals
- DOCTYPE (`L19-20`): `<!DOCTYPE html PUBLIC "-//WAPFORUM//DTD XHTML Mobile 1.0//EN" "http://www.wapforum.org/DTD/xhtml-mobile10.dtd">`
- `TINY_XHTML_PREFIX` (`L21`): `<html xmlns="http://www.w3.org/1999/xhtml"><body>`; SUFFIX `</body></html>`
- **Two `<br>` spellings, do not unify:** rich/raw-UDP pages use `<br />` (spaced, `TINY_XHTML_BR L23`); WSP/browser + WML pages use `<br/>` (no space).
- XML decl: `<?xml version="1.0" encoding="UTF-8"?>`

### 4.2 Render ladder (`render_wml2_status`, `L50-72`)
Mode order **Full → Compact → Text → Tiny**; **first that fits `max_bytes` wins**. Title empty ⇒ `Err(EmptyTitle)` (only hard precondition). If none fit, re-render Tiny and return `RenderedTooLarge{len: tiny.len(), max}` (Tiny's length, not the smallest).

- **Full** (`L491-494`): xml-decl `\n` DOCTYPE `\n` `<html…><head><title>{title}</title><meta http-equiv="Cache-Control" content="no-cache" /><meta http-equiv="refresh" content="8;url={refresh}" /><style…>…</style></head><body>` + hero/box markup. subtitle `WAP 2.0 / WML2 live core`; refresh **8s**; counts `MS {} G {} P {} SDS {}` (spaces, `<span class="k">` wrappers); `Ver` not compacted.
- **Compact** (`L495-498`): refresh **10s**, no Cache-Control, smaller CSS; counts `MS:{} G:{} P:{} SDS:{}` (colons).
- **Text** (`L504-573`): prefix has **no LF, no DOCTYPE, no CSS**; lines joined `<br />` greedily; refresh 10s.
- **Tiny** (`L589-638`): `PREFIX + body + SUFFIX`; counts dropped if won't fit; optional `<br />Last {…}`. No refresh.

### 4.3 Escape-then-truncate (`escape_xhtml_text_limited`, `L710-744`) — CRITICAL
Operates on **escaped byte length**. For each char compute fragment; if `escaped.len()+fragment.len() > max` set truncated, **break (never split an entity)**; else append. After loop, if truncated **and** `escaped.len()+1 ≤ max`, push `'~'` (0x7E). Map: `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`, `"`→`&quot;`, `'`→`&apos;`, `\n|\r|\t`→single space, other control→`?`, else UTF-8 bytes.
Vectors: `escape("&&&&",10)="&amp;&amp;"`; `escape("&&&&",11)="&amp;&amp;~"`; `escape("<tag>",8)="&lt;tag~"`; `escape("abc",0)=""`.

### 4.4 Size ladder & per-field caps
Escaped caps (`L10-17`): title 24, state 20, version 32, last-activity 32, health 32, detail-line 32, health-line 28, detail-max-lines 3.
Per-mode overrides (literals): sector title 24/16/8 (Normal/Compact/Tiny `L301-305`); sector line 44/28/18 (`L306-310`); sector max-lines 5/4/3 (`L311-315`); dashboard health-summary cap 32/20/18/18 (Full/Compact/Text/Tiny `L646-661`); browser sector heading cap 14; browser inline title/state cap 12.

### 4.5 Openwave caps (`.min()` applied AFTER MTU budget, `wap_ip.rs:339-355`)
- **Raw-UDP** (plain GET, no WTP/WSP): budget `min(max_npdu−28, 548)` → full DOCTYPE+CSS XHTML-MP.
- **WSP** (browser path): budget `min(max_npdu−28−3−4, 541)`, then **hard-clamp**: WML page **144**, XHTML index **104**, XHTML sector **144**. WSP pages are **bare** `<html><body>…</body></html>` (no xmlns/DOCTYPE/CSS) or `<wml><card><p>…`, nav single-letter `N`/`P`/`H`, `<br/>`.
- **Budget chain:** 576-octet one-slot N-PDU − 28 (IPv4+UDP) = 548 (raw); WSP SDU 545; 545−4 = 541.

### 4.6 Quirks
- **rendered_registered_ms** (`L773-776`): if any radio_lines, MS count = radio-line count, else `registered_ms`.
- **Three uptime formatters**: `compact_uptime` (2-field zero-padded, e.g. 93784→`1d02h`), `compact_tiny_uptime` (`{d}d{h}h{m}m{s}s` no pad, clamp 99d, 93784→`1d2h3m4s`), `compact_browser_uptime` (drops `m` when 0).
- `compact_tiny_state` (`L778-787`): substring `CRITICAL`→`Err`, `DEGRADED`→`Warn`, else `OK`.
- `compact_browser_radio_line`: on Radios pages reduce `MS 2260618 -38dB G1 SA` to ISSI `2260618`; all radio ISSIs joined into one space-separated line.
- Sector pagination: Summary (always, 4 lines), Health (if any), Radios (chunked by 5), Calls (chunked by 5), Activity (if present). Block label `{sector+1}/{count}`; sector index clamped to last page.
- Content-type byte: WML `0x88`, XHTML `0xC5` (short-integer, never textual MIME).

---

## 5. Downlink encapsulation + channel

### 5.1 Downlink SN-UNITDATA (always type 4) — `unitdata.rs:149-176`, `mle_adapter.rs:262-285`
Header `type=4 | nsapi(echo) | pcomp=0 | dcomp=0` (16 bits), then the IPv4/UDP N-PDU bit-appended. Built even when the request was SN-DATA (type 1). `layer2service = Unacknowledged` for WAP unitdata; `packet_data_flag = true` (forced, `mle_adapter.rs:267`); `fcs_flag = false` default.

**Response IPv4/UDP fields** (`wap_ip.rs:379-385`, `ip.rs:187-229`): src=`endpoint.address` (10.0.0.1), dst=`request.source` (MS IP); src_port=`endpoint.port` (9200), dst_port=`request.src_port`; **IP id = req_id + 1 wrapping**; ttl=`endpoint.response_ttl` (32); UDP cksum `0x0000`; IPv4 cksum computed.

### 5.2 TL-SDU toward TLA — `mle_adapter.rs:296-301`
`BitBuffer::new(3 + sdu_len)`; `write_bits(Sndcp=4, 3)`; then copy SN-UNITDATA bits. → `[3-bit MLE discriminator 0b100][SN-PDU bits]`. Everything shifts right by 3 bits (not byte-aligned).

### 5.3 CmceChanAllocReq — `chan_alloc_req.rs:10-24`
Attached for **SN-DATA TRANSMIT RESPONSE** (not the UNITDATA status path), built by `attach_mvp_pdch_allocation_for_data_transmit_response` (`ltpd_pipeline.rs:106-181`):

| Field | New/refresh PDCH | End-of-data |
|---|---|---|
| usage | `None` | `None` |
| carrier | `None` | `None` |
| timeslots | `[F,T,F,F]` (TS2 only) | `[F,F,F,F]` |
| alloc_type | `Replace`(0) | `QuitAndGo`(2) |
| ul_dl_assigned | `Both`(3) | `Both`(3) |

`assigned_scch_pdch_timeslots_for_resource_request` **ignores the MS's resource request** and always returns `[F,T,F,F]` (`pdch.rs:883-885`); `normalize_pdch_timeslots_to_single` collapses any multi-slot policy to one slot (prefers first set slot at idx>0). TS1 (MCCH) never assigned. `active_circuit_mode_service && !parallel_voice_data_permitted` ⇒ `CircuitModeConflict`.

---

## 6. Enablement

### 6.1 Config keys (`sec_cell.rs` / `parsing.rs`)
`[cell_info.wap_ip]`: `enabled` (default false), `address` (def `10.0.0.1`), `port` (def 9200, reject 0), `response_ttl` (def 32, reject 0), `dynamic_pool_prefix` (def `10.0.0`), `dynamic_pool_first_host`/`last_host` (def 2/254), `allow_static_ipv4`/`accept_empty_probe`/`accept_root_path`/`accept_status_path`/`accept_status_wml_path` (def true), `max_request_payload_bytes` (def 1024, must be 1..=1024), `assume_pdch_ready_after_data_transmit` (def false).
**Validation** (`sec_cell.rs:463-480`): `sndcp_service=true` requires `wap_ip.enabled=true`; `wap_ip.enabled=true` conflicts with `sndcp_service=false` or `advanced_link=false`. Neighbor `bs_service_details.sndcp_service=true` always rejected (`:569-584`).
**The single gating predicate:** `wap_ip_sndcp_profile_enabled := cfg.cell.sndcp_service && wap_ip.is_some_and(|w| w.enabled)` (`umac_bs.rs:1356-1359`, `mle_bs.rs:104-107`, `sndcp_bs.rs:189-197`). It drives all three things below — they must never disagree.

### 6.2 D-MLE-SYSINFO BS-service-details (12-bit IE) — `bs_service_details.rs:70-83`, built `umac_bs.rs:1292-1313`
14-bit location_area + 16-bit subscriber_class + 12-bit IE (`d_mle_sysinfo.rs:38-43`, total 42 bits). IE bit indices (MSB first):

| IE bit | Field | When WAP on |
|---|---|---|
| 0 | registration | cfg |
| 1 | deregistration | cfg |
| 2 | priority_cell | cfg |
| 3 | no_minimum_mode | cfg |
| 4 | migration | cfg |
| 5 | system_wide_services | state |
| 6 | voice_service | cfg |
| 7 | circuit_mode_data_service | cfg |
| 8 | RESERVED | **0** |
| **9** | **sndcp_service** | **1** |
| 10 | aie_service | **0** (always) |
| **11** | **advanced_link** | **1** (= cfg && profile) |

The reserved bit 8 sits **between** circuit_mode_data (7) and sndcp_service (9) — omitting it misaligns bits 9/11.

### 6.3 MAC SYSINFO extended-services (section 1) — `sysinfo_ext_services.rs:96-124`, `umac_bs.rs:1212-1234`
SYSINFO emits two variants alternately: sysinfo1 `option_field=2` (access-code params), sysinfo2 `option_field=3` (ExtServicesBroadcast). The IE (20 bits, AIE disabled): `auth_required(1)=0 | class1(1)=0 | class3(1)=0 | security(5)=0 | sdstl_addressing_method(2)=0b10(=2) | gck(1)=0 | section(2)=0b00 | section_data(7)`. **`section_data = 0b1000000 (0x40) iff WAP profile enabled, else 0`** (bit6/MSB = data-priority/WAP-IP advertisement).

### 6.4 MLE 3-bit discriminator cursor contract — THE HEADLINE FIX
- **Strip (uplink):** MLE does `sdu.read_bits(3)` which **advances the cursor to bit 3** (does NOT slice/remove bytes), then forwards the same buffer with `pos==3` (`mle_bs.rs:219`). SNDCP rebases with `from_bitbuffer_pos` when `pos!=0` (`sndcp_bs.rs:1051-1060`, `bitbuffer.rs:127-140`: copies from `pos/8`, `start=pos%8`). The SN-PDU is then decoded starting at the bit **immediately after** the 3-bit discriminator.
- **Prepend (downlink):** allocate `(3 + payload_bits)`, write 3 discriminator bits MSB-first, copy the SN-PDU, `seek(0)` (`mle_bs.rs:578-582`).
- **DO NOT** `raw[3..]` (byte slice) — that drops 3 **bytes** (24 bits). Drop 3 **bits**. After a 3-bit prefix the 4-bit `sn_pdu_type` straddles byte0 bit3..7 + byte1 bit0, so bit-level extraction is mandatory (or left-shift the whole remainder by 3 bits).
- **Note FlowStation's current stub** (`sndcp_bs.rs:209-211`) works in a **binary-string** domain (`dump_bin_unformatted()` then `raw.get(3..)`), which correctly drops 3 *bits* (each char = 1 bit) — so the stub is right *for its string representation*, but any BitBuffer-native reimplementation must drop 3 bits, not 3 bytes.

---

## 7. Captured test vectors (conformance fixtures to freeze)

> nexus stores struct + length asserts (the precise spec); IP/WSP layers store byte vectors. Hex below is ground-truth and was recomputed where derivable (checksums confirmed: `0x536d`, `0x16c4`).

### 7.1 SN-PDP (`pdp.rs`)
- **DEMAND dynamic IPv4** (`L584-617`, **28 bits**): ver=1,nsapi=2,ATID=1,mstype=0,pcomp=0 → `0000 0001 0010 001 0000 00000000 0` ⇒ bytes `0x01 0x22 0x00 0x00` (first 28 bits).
- **DEMAND static 10.0.0.18** (`L621-628`, 60 bits): ATID=0, IPv4 `00001010 00000000 00000000 00010010`.
- **DEMAND secondary** (`L630-634`, 32 bits): nsapi=3, ATID=5, primary=2.
- **ACCEPT dynamic IPv4** (`L594-606`, **70 bits**): nsapi=2,pri=4,ready=8,standby=4,respwait=7,TIA=2,IPv4=`10.0.0.226`,pcomp=0,mtu=Octets576(code 2). Bitstring `0000 0010 100 1000 0100 0111 010 00001010000000000000000011100010 00000000 010 0`; **byte view** (pad to 72): `0x0A 0x44 0x74 0x28 0x00 0x00 0x38 0x80 0x40`. **This is the ground-truth ACCEPT ordering FlowStation must match.**
- **REJECT** (`L653-657`, 17 bits): type=3,nsapi=2,cause=34 → `00110010 00100010 0`.
- **DEACTIVATE** (`L667-687`): AllNsapis DEMAND(2) 13 bits `0010 00000000 0`; single NSAPI=2 ACCEPT(1) 17 bits.

### 7.2 SN-UNITDATA (`unitdata.rs`)
- **Round-trip** (`L217-231`): `encode(nsapi=2,pcomp=0,dcomp=0, n_pdu=[0x45,0x00,0x00,0x14])` → bytes `0x42 0x45 0x00 0x00 0x14` (header `0x42`=type4|nsapi2). kind=Ipv4. 48 bits.
- **IPv6** (`L234-243`): nsapi=3, n_pdu=`[0x60,…]` → first byte `0x43`, kind=Ipv6.
- **Reserved NSAPI** (`L276-286`): first byte `0x4F` → `Err UnsupportedNsapi(15)`.
- **Compression** (`L288-298`): `0x41 0x10 0x45…` → `UnsupportedCompression{pcomp:1,dcomp:0}`.

### 7.3 IPv4/UDP/TCP (`ip.rs`) — full packets
- **Vector A** (`L340-359`, UDP, 78-byte XHTML): src 10.0.0.1, dst 10.0.0.226, ports 9200/9200, id 0x1234, ttl 64. IP hdr `45 00 00 6a 12 34 00 00 40 11 53 6d 0a 00 00 01 0a 00 00 e2`; UDP `23 f0 23 f0 00 56 00 00`.
- **Vector B** (`L399-412`, UDP "wap", **31 bytes**): src 192.0.2.1, dst 192.0.2.2, sport 49152, dport 9200, id 7, ttl 32. **Full packet:** `4500001f 00070000 201116c4 c0000201 c0000202 c00023f0 000b0000 776170`.
- **Vector C** (`L361-397`, TCP, 58 bytes): src 10.0.0.1, dst 10.0.0.226, sport 9200, dport 49152, seq 0x11223344, ack 0x55667788, flags 0x019 (FIN+PSH+ACK), window 0x1000, payload `"HTTP/1.0 200 OK\r\n\r\n"`. IP hdr cksum `0x53a7`; TCP cksum `0xcbbe`. **Full:** `4500003b12340000400653a70a0000010a0000e2 23f0c000112233445566778850191000cbbe0000 485454502f312e3020323030204f4b0d0a0d0a`.
- Re-running `ipv4_header_checksum` over a built header yields **0**; `ipv4_tcp_checksum` over a built segment yields **0**.
- Errors: `[0u8;19]`→`Ipv4TooShort`; 9-bit buffer→`NpduNotOctetAligned{9}`; UDP `[0u8;7]`→`UdpTooShort`; TCP byte12=`4<<4`→`TcpHeaderTooShort`.

### 7.4 WTP/WSP (`wap_ip.rs`)
- **MXP600 Connect capture** (`L866-880`, **394 bytes**): header `0b 13cc 12 01 10 1d 8264 …`. byte0 `0x0b` (CON=0, Invoke, RID=1); TID `0x13cc`; Invoke `0x12`; WSP Connect; ver `0x10`; CapLen uintvar 29; HeadersLen uintvar `82 64`=**356** (2-octet uintvar — must handle). Caps: 0x80 SDU=327680, 0x81 SDU=327680, 0x82 0xf0, 0x83 0x03, 0x84 0x01, 0x86 `"\x10x-up-1\0"`. UA `MOT-MXP600\MR2026.1 UP.Browser/6.3.0.1 (GUI) MMP/2.0`. Classify ⇒ `WtpWspConnect{tid:0x13cc, retransmission:true}`.
- **ConnectReply** (`L1311-1355`): `[0x12, 0x93, 0xcc, 0x02, 0x01, 0x08, 0x00, 0x03,0x80,0x84,0x21, 0x03,0x81,0x84,0x21]` (TID 0x13cc|0x8000=0x93cc; both SDUs clamped 327680→545=0x8421). Caps 0x82/0x83/0x85/0x86 absent.
- **WSP GET /status.xhtml** (`L1580-1591`, tid 0x1234): response prefix `[0x12, 0x92, 0x34, 0x04, 0x20, 0x01, 0xc5]` + XHTML body ≤104 bytes (index), contains `Welcome to Nexus-BS!`, `href="/status.wml?s=1"`.
- **WSP GET ?s=1** (`L1629-1674`): same prefix, body ≤144, `Health OK`, `href="?s=0"`, `href="/"`.
- **WSP GET /status.wml?s=1** (`L1677-1719`): prefix `[…0x01,0x88]` (WML), body ≤144.
- **WSP Resume** (`L1106-1115`, tid 0x13f5): response `[0x12, 0x93, 0xf5, 0x04, 0x20, 0x00]`.
- **WTP ACK** `[0x18,0x13,0xcc]` → `WtpControlNoResponse{pdu:3}`, **no datagram**. **WTP ABORT** `[0x20,0x13,0xcc,0x01]` → abort_type 0, reason 1, no datagram.
- **Endpoint swap** (`L1236-1278`): req id 0x2222 → resp id 0x2223; ports/IPs swapped; ttl 32.
- `[0x01,0x40,0x00]` (3 bytes, type nibble 0) → falls through to text path → `UnsupportedWapUdpPayload{3}`.

### 7.5 Status pages (`wap_status.rs` sample snapshot, `L841-864`)
title=`Nexus-BS`, version=`v0.1.69_dev-test`, state=`ON AIR`, registered_ms=3, radio_lines=2 (so rendered MS=**2**), uptime 93784s, last=`SDS 2260082>2260618`, health=`OK`. Full@2048 contains `<span class="k">MS</span> 2`, `Up</span> 1d02h`, `Ver</span> v0.1.69_dev-test`, `Last: SDS 2260082&gt;2260618`, `Health:OK`. Tiny@128: `Nexus-BS: OK`, `Version: 0.1.69`, `Uptime 1d2h3m4s`, exactly 2 `<br />`. WSP XHTML index payload[7..] ≤104; content-type byte payload[6]==0xC5.

### 7.6 FlowStation CHAP fixtures (`sndcp_bs.rs:252-298`) — freeze these
- **Captured Motorola DEMAND PCO** (`L256-258`): `0c22318010500180aac20e0caf974bc75e02f44494d455452415f50 c2231a0205001a10db3b2df8c57cce0db8712b16aa9cb5a361646d696` → `find_chap_response_id(...) == Some(5)`.
- **`chap_success_optional_section(5)`** (`L283-298`, **81 bits**): `o-bit(1)=1 | type-2 P-bits(3)=000 | M-bit(1)=1 | PCO id(4)=0001 | length(11)=60=00000111100 | cfg-proto(4)=0000 | proto-id(16)=1100001000100011(C223) | len-of-contents(8)=00000100 | CHAP code(8)=00000011(Success) | identifier(8)=00000101(=5) | CHAP length(16)=0000000000000100 | closing M-bit(1)=0`.

---

## 8. FlowStation current-stub correctness audit

FlowStation `sndcp_bs.rs send_pdp_accept` (`L56-136`) vs nexus `pdp.rs encode_activate_pdp_context_accept` (`L201-233`). **The SN-PDU field order and widths are IDENTICAL** (type4/nsapi4/pri3/ready4/standby4/respwait4/tia3/ipv4(32 iff TIA∈{1,2})/pcomp8/mtu3/o-bit1). Divergences are field *values* (SwMI policy) plus FlowStation's CHAP extension:

| # | Item | FlowStation | nexus | Verdict |
|---|---|---|---|---|
| 1 | Ready timer | code **8** (`sndcp_bs.rs:20`) | test uses **8** (`pdp.rs:598`) | **Equivalent.** Identical; both in range 1..14. nexus value is a test example, not a required spec value. |
| 2 | Standby timer | code **5** (`:21`) | test uses **4** (`pdp.rs:599`) | **Both correct (equivalent on the wire-contract).** Differ, but both ∈ 1..15 — a nexus decoder accepts FlowStation's. Pick is SwMI policy; no conformance issue. |
| 3 | Response-wait timer | code **8** (`:22`) | test uses **7** (`pdp.rs:600`) | **Both correct.** Both ∈ 0..14; policy choice. |
| 4 | MTU | **1500 = code 4** (`:25`) | test uses **576 = code 2** (`pdp.rs:604`) | **Both correct.** Both valid (`pdp.rs:515,513`). FlowStation advertises a larger MTU; policy. **Caveat:** the page-size budgets (548/541/144/104) assume a **576-octet** one-slot N-PDU; if FlowStation actually negotiates 1500 it must keep its page budgets consistent — but the ACCEPT byte is valid either way. |
| 5 | TIA static / dynamic | **1 / 2** (`:23-24`) | **1 / 2** (`pdp.rs:505-507`) | **Equivalent (exact match).** |
| 6 | PDU priority max | **4** (`:19`) | test uses 4 (`pdp.rs:597`) | **Equivalent.** Both ∈ 0..7. |
| 7 | PCOMP negotiation | **0** (`:83`) | **0** | **Equivalent.** |
| 8 | NSAPI | echoed from DEMAND | echoed | **Equivalent.** |
| 9 | Dynamic-IP pool | hardcoded **192.168.1.180** (`POOL_IPV4`, `:28,69`) | config-driven pool **10.0.0.2..254** (`sec_cell.rs:34-36`); test assigns 10.0.0.2 | **nexus is more correct.** Both produce a valid 32-bit IPv4 in the ACCEPT (wire-valid), but FlowStation's `192.168.1.180` is a single hardcoded address with no pool/lease management → collisions with >1 simultaneous MS. **Action:** replace with a config-driven pool. Not a framing bug; an addressing-policy defect. |
| 10 | Static IP | echoes MS request bits 15..47 (`:64-66`) | echoes requested static IPv4 | **Equivalent.** |
| 11 | O-bit / optional elements | writes O-bit=0 **unless CHAP present**, then writes a PCO section (`:90-97`) | **always** O-bit=0; rejects any optional element on decode (`pdp.rs:381-397`) | **See #12 — this is the key divergence.** |
| 12 | **CHAP-Success PCO** | **present** (`find_chap_response_id` + `chap_success_optional_section`, `:144-192`) | **absent** — nexus has no PCO/CHAP support at all | **FlowStation is correct *for Motorola Dimetra terminals*.** See below. |

### 8.1 The CHAP difference and what it means for Motorola Dimetra terminals
Motorola/Dimetra MS (incl. MXP600) run **PPP CHAP (RFC 1994) inside PDP-context activation**: the DEMAND carries the MS's CHAP Response (username + MD5 hash) in a **PCO type-3 element** (config protocol 0=PPP, protocol-id `0xC223`). **If the ACCEPT does not return a CHAP-Success in its PCO, the MS aborts with "data server not responding"** (`sndcp_bs.rs:30-40`). The PCO is **bit-packed, not byte-aligned**: `0xC223` can land at any bit offset, so FlowStation scans the whole DEMAND bit-string for the 16-bit `C223` pattern, then reads CHAP code at `marker+24` and identifier at `marker+32` (`:174-192`); code 2=Response (echo its id), code 1=Challenge (fallback). The Success echoes that identifier with code=3, length=4, content-length 60 bits (`:144-162`).

- **nexus would never satisfy a real Motorola terminal** — its codec always writes O-bit=0 and rejects optional elements. nexus's WAP works only with terminals that don't require CHAP.
- **FlowStation's CHAP path is required for Dimetra interop and is the more terminal-correct implementation.** It must be **kept** in the clean-room port, and it must be exercised against §7.6 fixtures.
- **Conclusion:** for the mandatory SN-PDU framing, FlowStation and nexus are byte-compatible (a nexus decoder accepts FlowStation's ACCEPT timer/MTU choices). The *only* substantive on-air difference is the **trailing PCO optional section**, which FlowStation adds (correctly, for Motorola) and nexus omits. The CHAP `chap_id` validation (rejecting a coincidental `C223` pattern inside a hash by checking the code byte) is a correctness safeguard FlowStation already has.

---

## 9. Port checklist (ordered; nexus behavior → FlowStation file/symbol)

> Status legend: **REUSE** = existing FlowStation code is correct as-is; **EXTEND** = keep but add; **REWRITE** = new code; **VERIFY** = confirm against fixtures.

1. **BitBuffer MSB-first engine** → reuse `tetra_core::BitBuffer` (`write_bits`/`read_bits`/`peek_bits`). **VERIFY** round-trip `0xAB,0xCD ⇒ [0xAB,0xCD]`. **REUSE.**
2. **MLE 3-bit discriminator strip/prepend** → `mle_bs.rs` routing + SNDCP rebase. **CRITICAL:** ensure cursor-advance (drop 3 *bits*, rebase `from_bitbuffer_pos`), never `raw[3..]` byte-slice in a BitBuffer-native path. Current stub's string-domain `raw.get(3..)` is correct only because chars=bits. **VERIFY/REWRITE** when moving off the string domain.
3. **SN PDU type codes + NSAPI/PCOMP/timer/MTU validation tables** (§3) → new `pdp.rs`/`unitdata.rs`/`transfer.rs` codecs in FlowStation `sndcp/`. **REWRITE** (FlowStation currently only string-slices the DEMAND header; needs full encode/decode for round-trip parity).
4. **SN-ACTIVATE PDP CONTEXT ACCEPT encoder** → `sndcp_bs.rs send_pdp_accept`. Framing already matches nexus (§8). **REUSE** framing; **EXTEND** to config-driven timers/MTU/IP-pool (replace hardcoded `POOL_IPV4`, item #9).
5. **CHAP PCO Success path** → `find_chap_response_id` + `chap_success_optional_section`. **REUSE** verbatim behavior; **VERIFY** against §7.6 (`Some(5)`, 81-bit layout). This is FlowStation-unique and must survive the port.
6. **DEMAND/REJECT/DEACTIVATE/UNITDATA decoders+encoders** → new in FlowStation `sndcp/`. **REWRITE** (gain round-trip parity nexus has; FlowStation only handles type 0 today, `:238`).
7. **IPv4/UDP/TCP N-PDU build+parse + RFC1071 checksum** → new `sndcp/ip.rs`. Pure `Vec<u8>` functions (not BitBuffer-native); UDP cksum=0, IPv4/TCP cksum computed. **REWRITE.** **VERIFY** §7.3 full-packet hex + checksum=0 re-runs.
8. **WTP class-2 SAP** (Invoke→Result, ACK/ABORT→no-response, TID `&0x7fff|0x8000`, RID-always-clear, TPI skip) → new `sndcp/wap_ip.rs` WTP layer. **REWRITE.** **VERIFY** §7.4 prefixes (`0x12 …`).
9. **WSP SAP** (Connect/ConnectReply session-id `0x01`, only 0x80/0x81 caps clamped to 545, Resume-OK `[04 20 00]`, Reply `[04 20 01 ct …]`, uintvar codec) → `sndcp/wap_ip.rs` WSP layer. **REWRITE.** **VERIFY** §7.4 ConnectReply bytes.
10. **Request classifier** (status_enabled gate → size gate → empty-probe → **WTP-binary first** → text-GET fallback; `<3 bytes` short-circuit) → `wap_ip.rs parse_wap_udp_request`. **REWRITE**, preserve ordering.
11. **WML2/XHTML-MP page renderer + escape-then-truncate + size ladder + Openwave caps** → new `sndcp/wap_status.rs`. **REWRITE.** **VERIFY** §7.5 (rendered MS=2 quirk, `Health:OK` no-space, two `<br>` spellings, `~` truncation, 104/144/541/548 budgets).
12. **WAP gateway validation order** (ReservedNsapi→IssiNotAllowed→MissingContext→MissingPduPriorityMax→UnsupportedPdpType→ContextAddressNotIpv4→NpduNotOctetAligned→parse→Fragmented→SourceAddressMismatch→render→post-render MTU) and **build-then-gate READY** ordering → `wap_gateway.rs`/`wap_session.rs`. **REWRITE**, preserve precedence.
13. **Downlink SN-UNITDATA encapsulation** (always type 4; src/dst swap; id+1; ttl 32; UDP cksum 0) + **TL-SDU 3-bit prefix** → `mle_adapter.rs` equivalent. **REWRITE.**
14. **CmceChanAllocReq attach** (TS2 `[F,T,F,F]` Replace/Both; end-of-data `[F,F,F,F]` QuitAndGo; ignore MS resource request; normalize-to-single) → `ltpd_pipeline.rs`/`pdch.rs`. **REWRITE.** **VERIFY** §ltpd tests (timeslots `[false,true,false,false]`).
15. **Enablement gating** — single predicate `sndcp_service && wap_ip.enabled` drives (a) D-MLE-SYSINFO bits 9+11, (b) MAC SYSINFO section_data `0x40`, (c) SNDCP runtime accept-vs-drop. Config validation forbidding disagreement. → `umac_bs.rs`/`mle_bs.rs`/config. **REWRITE/EXTEND.** **VERIFY** reserved IE-bit 8 = 0 between circuit_mode_data and sndcp_service.
16. **D-MLE-SYNC neighbor_cell_broadcast = 2 hardcoded** (Motorola time/date display) and **AIE/security forced off** → `umac_bs.rs`. **REWRITE** (do not config-drive `neighbor_cell_broadcast`).
17. **SAP routing reuse** → FlowStation's existing TLA/LTPD/SN SAPs and MLE routing are **REUSED**; the discriminator match (4→SNDCP, 1→MM, 2→CMCE; 0/3/5/6/7 drop) is **VERIFY**.
18. **Freeze conformance fixtures** — port §7.1-7.6 as FlowStation tests (the ACCEPT 70-bit byte view `0A 44 74 28 00 00 38 80 40`, the three IP full-packet hex, the WTP/WSP prefixes, the CHAP 81-bit section). **REWRITE** as FlowStation `#[cfg(test)]`.

**Files reused vs rewritten:** the BitBuffer engine, SAP types (TLA/LTPD/SN), and MLE routing skeleton are **reused**; `sndcp_bs.rs send_pdp_accept` framing + the entire CHAP path are **reused/extended**; everything else (IP/UDP/TCP, WTP, WSP, page renderer, gateway, full SN-PDU codecs, PDCH alloc, SYSINFO enablement) is **rewritten clean-room** from this spec.

Relevant paths — nexus (reference, do not transcribe): `/Users/razvanzeces/Desktop/nexus-bs-main/crates/tetra-entities/src/sndcp/{pdp.rs,unitdata.rs,transfer.rs,ip.rs,wap_ip.rs,wap_status.rs,wap_gateway.rs,wap_session.rs,ltpd_pipeline.rs,mle_adapter.rs,pdch.rs}`. FlowStation (target): `/Users/razvanzeces/Desktop/flowstation-main/crates/tetra-entities/src/sndcp/sndcp_bs.rs` (existing stub + CHAP, to extend), `/Users/razvanzeces/Desktop/flowstation-main/crates/tetra-entities/src/sndcp/mod.rs`.
