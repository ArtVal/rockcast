# Google Cast / CASTV2

## Stack

```text
discovery/       find devices (mDNS + subnet)
  lan.rs         NIC scoring, VPN/tunnel filter
  mdns.rs        _googlecast._tcp browse
  subnet.rs      /24 TCP:8009 probe + eureka_info
  eureka.rs      Parse setup/eureka_info JSON
client/          play / stop / volume / heartbeat session
  session.rs     CONNECT, LAUNCH, LOAD, heartbeat thread
  media.rs       LOAD candidates, media status parsing
  error.rs       ClientError
channel.rs       TLS + 4-byte BE length + CastMessage frames
proto.rs         hand-rolled protobuf (see also proto/cast_channel.proto)
```

No Chrome / no official Cast SDK. Devices speak CASTV2 on **TCP 8009** with TLS (certificate verification intentionally disabled — typical for Cast).

## Discovery

`discover(timeout)` runs in parallel:

1. **mDNS** — `_googlecast._tcp` on a scored LAN interface (skip VPN/Hyper-V/etc.; Russian “Беспроводная…” names recognized)
2. **Subnet scan** — for the chosen NIC’s `/24`, probe `x.x.x.1–254:8009`, then `GET http://ip:8008/setup/eureka_info`

Results merge by device id. Subnet scan is what usually finds JBL / bars when VPN breaks multicast.

Probe CLI: `cargo run --example cast_probe`.

## Session lifecycle (`CastService::play`)

```text
cancel prior op → op_lock
take_or_connect (stop old heartbeat; reuse channel if same device id)
CONNECT (receiver-0, connection namespace)
GET_STATUS / LAUNCH CC1AD845 (Default Media Receiver) if needed
CONNECT (transport_id)
LOAD live stream (contentId=url, streamType=LIVE)
start heartbeat thread (PING every ~5s, pump incoming)
store LiveSession { channel, transport_id, session_id, media_session_id }
```

`stop()`: cancel → op_lock → stop heartbeat → media/receiver STOP + CLOSE.

## Framing (`CastChannel`)

- TCP + rustls, read timeout normally **250ms** (heartbeat-friendly)
- During `receive_find`: **500ms** reads, overall deadline, `cancel` flag
- Incoming non-matching messages go to `inbox` (re-scanned each loop)
- Heartbeat PING from device → immediate PONG
- Device auth challenge/response on connect (failure is logged and ignored when possible)

## Protobuf (`proto.rs`)

Implements the subset needed for CastMessage / auth. Do not regenerate from `proto/cast_channel.proto` without updating encode/decode by hand — the `.proto` file is **reference documentation**, not a build input.

## Content types

`Station::content_type()` selects the PC relay transport/transcoder. Cast LOAD uses the content type returned by `StreamRelay` (commonly `audio/wav` after PCM decoding), with the existing client fallback candidates. The speaker fetches only the PC LAN URL; it does not fetch or authenticate to RockServer or the original station.

## PC relay mode (`StreamRelay`)

PC relay is mandatory for every Cast output; the former Via PC checkbox and persisted mode are removed:

```text
Station → RockServer /api/v1/stations/{id}/stream
  → authenticated RockCast feeder → decode/ring
  → http://LAN_IP:port/stream → JBL / Cast LOAD
```

- Listener binds `0.0.0.0:ephemeral`; advertise IP is the LAN interface nearest the Cast device (VPN/virtual NICs skipped).
- Accepted TCP sockets are forced **blocking** (Windows inherits non-blocking from the listener — that caused WSAEWOULDBLOCK / silent Cast IDLE).
- One shared authenticated RockServer feeder + ring. Before LOAD, the worker waits for PCM format when needed and for the initial audio buffer. Spectrum/title data come from the relay, without a second station download.
- RockCast controls one selected Cast receiver per playback session; discovering multiple receivers does not mean they can play simultaneously. The relay accepts at most two HTTP clients total (the receiver stream and optional spectrum tap). This limit does not provide multi-receiver playback; additional connections receive HTTP 503.
- Native credentials are resolved before opening the stream and again on feeder reconnects; they are never included in the LAN URL or sent to the speaker.
- ICY titles come from the feeder; UI polls `StreamRelay::latest_title()`.
- Windows Firewall / firewalld may prompt once for inbound LAN access — allow it or Cast cannot pull the relay.
- The relay listens on all IPv4 interfaces and has no client authentication. Only use it on a trusted LAN.

## Failure modes

| Log / error | Meaning |
|-------------|---------|
| `Cast rejected LOAD: LOAD_FAILED` | Receiver could not play URL (codec, geo, TLS, dead link) |
| `timed out waiting for Cast response` | No usable MEDIA_STATUS / error within deadline |
| `Cast operation cancelled` | Newer play/stop aborted this handshake |
| Discovery empty | VPN/NIC filter, wrong LAN, firewall |

## Concurrency notes

- Only one Cast play/stop handshake at a time (`op_lock`)
- Heartbeat must not run during LOAD wait on the same channel without coordination — `take_or_connect` stops heartbeat before reuse
- UI must not wrap `CastService` in an outer `Mutex` held across `play()` (that deadlocks Stop/local when LOAD hangs)
