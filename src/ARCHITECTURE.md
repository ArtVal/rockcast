# RockCast source map

## Root responsibilities

| Area | Owns |
|---|---|
| `app/` | egui state, rendering, and adaptation of background events |
| `playback/`, `runtime.rs` | playback lifecycle, bounded background work, cancellation |
| `local/`, `cast/`, `relay/`, `audio/` | output adapters and stream transport/decoding |
| `stations/`, `personal_data.rs`, `settings.rs` | catalog and durable local user data |
| `session.rs`, `rockserver.rs`, `voice/` | paired identity and RockServer HTTP/WSS clients; Windows credentials use DPAPI (`session.dpapi`), Linux uses Secret Service in `session/secret_service.rs` |
| `device_control.rs` | DC-012 registration plus DC-013/DC-014 bounded command lifecycle; `protocol.rs` owns v1 JSON, `output.rs` owns opaque TTL Chromecast handles, `transport.rs` owns tungstenite, and `tests.rs` holds wire/lifecycle regression coverage |

## Dependency direction

`main → app → playback → {local, cast, relay}`. `app` may use settings, catalog,
session, and device-control; protocol and output adapters must not depend on egui.
`device_control` may depend on `session` and `rockserver`, but never on app or playback. It hands parsed, locally capability-checked commands through a bounded process-local queue; `app` remains the sole `PlaybackController` owner and reports the observed terminal outcome back to the client.

## Lifecycle and reading order

Start with `lib.rs`, `main.rs`, then `app/mod.rs` and `playback/mod.rs`; follow the selected
output adapter. For paired features read `session.rs` before `voice/` or `device_control.rs`.
The device-control loop is `DeviceControlClient::new → publish/take_command/complete_command → shutdown`; it reuses the existing paired credentials, executes no playback on its worker thread, and emits a terminal result only after the UI-owned controller reports an actual outcome. DC-014 discovery runs as a bounded UI background job, returns opaque receiver handles valid for 60 seconds only, and never treats a receiver as a registered device identity.

## Tests

Focused domain tests live beside their module; large device-control regression coverage is in
`device_control/tests.rs`. Broader integration tests remain under `tests/`.
