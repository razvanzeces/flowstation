# WAP packet data (SNDCP) — setup guide

FlowStation can serve **WAP** to a packet-data-capable TETRA radio (e.g. a
Motorola MXP600 with the Openwave browser). Once configured, a radio can:

- activate a **PDP context** (packet-data session), authenticated with CHAP,
- be granted a dedicated **PDCH** (packet data channel), and
- **browse a live base-station status page** over WAP — registered radios,
  uptime, SDS activity and station health.

Under the hood this is the TETRA SNDCP service (ETSI EN 300 392-2 clause 28)
plus an IPv4/UDP stack and a WTP/WSP/WAE gateway on the base station.

---

## The feature is OFF until you configure it

There is **no WAP service unless you add the `[cell_info.wap_ip]` section** to
your `config.toml` with `enabled = true`. If the section is absent (or
`enabled = false`), the base station does not advertise or answer packet data
at all — that is by design, not a bug.

## Requirements

There is **one switch**: `[cell_info.wap_ip].enabled = true`. When set, the cell
runs the gateway and advertises `sndcp_service` + `advanced_link` in SYSINFO for
you — you do **not** need to set those flags by hand. Disabling (or removing)
the section is always safe and never affects startup.

| Setting | Where | Value |
|---|---|---|
| `enabled` | `[cell_info.wap_ip]` | `true` to turn WAP on |

The plain `sndcp_service` / `advanced_link` flags under `[cell_info]` are
advisory; the WAP switch drives what is actually advertised and run.

## config.toml

Add this to your `[cell_info]` block (uncomment and adjust):

```toml
[cell_info]
# ... your existing cell settings ...

[cell_info.wap_ip]
enabled                   = true        # the only switch needed to turn WAP on
address                   = "10.0.0.1"  # BS gateway IPv4 the radio targets — MUST be quoted
port                      = 9200        # UDP port the radio sends the WAP request to
response_ttl              = 32          # IPv4 TTL on the downlink reply
max_request_payload_bytes = 1024        # cap on an inbound request, in bytes (1–1024)
```

### Field reference

| Key | Type | Default | Notes |
|---|---|---|---|
| `enabled` | bool | `false` | Master switch for the gateway. |
| `address` | string | `"10.0.0.1"` | Gateway IPv4 the radio's browser points at. **Quote it** — a bare `10.0.0.1` is not valid TOML. |
| `port` | integer | `9200` | UDP port. `0` is rejected (falls back to the default). |
| `response_ttl` | integer | `32` | TTL on the reply datagram. `0` is rejected. |
| `max_request_payload_bytes` | integer | `1024` | Inbound request cap; clamped to `1`–`1024`. |

`[cell_info.wap_ip]` is a sub-table of `[cell_info]`, so it must appear **after**
the plain `[cell_info]` keys and before the next top-level `[section]`.

## Radio-side setup

Point the radio's WAP/Openwave browser homepage at the gateway, e.g.:

```
http://10.0.0.1:9200/
```

Use the same `address` and `port` you set above. The status page is served at
`/` (and `/status.wml`); the browser follows the links from there.

## Verifying it works

1. Restart the service and confirm it boots (no config error on the dashboard).
2. On the dashboard, `sndcp_service` shows as advertised in the cell's SYSINFO.
3. From the radio, open the browser homepage — you should get the FlowStation
   status page. The figures (registered radios, uptime, SDS count, health)
   update from live station state.

## Troubleshooting

**"config.toml does not parse — fix it on the Config page first"** (dashboard)
A TOML syntax error. The most common cause is an **unquoted IP address** —
write `address = "10.0.0.1"`, not `address = 10.0.0.1`. Also check the
`[cell_info.wap_ip]` header is placed after the plain `[cell_info]` keys.

**WAP isn't working but the station starts fine**
Check `[cell_info.wap_ip].enabled = true` is actually set and the section is
placed after the plain `[cell_info]` keys. Enabling it is all that is needed;
`sndcp_service`/`advanced_link` are advertised automatically. (Disabling WAP
never prevents startup — if the station won't boot, it is a different config
error; read the message on the dashboard Config page.)

**The radio can't reach the page**
Confirm the browser homepage host/port exactly match `address`/`port`, and that
the radio has an active packet-data (PDP) context.

**The page loads only sometimes, or stops when a call starts**
Packet data uses timeslot 4 of the main carrier while a radio is browsing, and
voice always has priority: a call that needs that slot (the third simultaneous
call on the carrier) ends the data session, and a radio cannot start one while
the slot carries a call. The radio retries by itself once the slot is free.

## How it uses the air interface

- A radio asks for packet data on the control channel and is moved to the packet
  data channel on **timeslot 4**. The other traffic slots stay free for voice,
  which is allocated from timeslot 2 upwards.
- While it is there, calls and text messages addressed to that radio are sent
  to it on the packet data channel, so it does not miss them. Group calls are
  announced on the main control channel only; the radio picks them up when it
  returns, at most about 10 seconds after its last page request.
- A radio that moves to another cell of a multi-cell station mid-session carries
  on there without activating a new context.

See also: [install-and-setup.md](install-and-setup.md).

---

*WAP packet-data (SNDCP) gateway and status page by **David Harcus, MM7FDM**.*
