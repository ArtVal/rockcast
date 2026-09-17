# RockCast status

## RC-4a — server-delivered `station.play_stream` acceptance (fixed, 2026-09-17)

Live testing showed RockCast rejecting remote play commands with
`invalid_payload`: after RS-3 the server delivers the phone's
`station.play_station` as `station.play_stream { station_id, stream_uri }`,
but RockCast ignored the `station_id` field and instead required the stream
URI to match some station already in its local list. Any station picked from
the phone's larger server catalog therefore failed the lookup and the command
was answered with a terminal failure — RockCast "did not accept play".

`station.play_stream` now parses the catalog `station_id` (a catalog-source
delivery without it is rejected as `invalid_payload`, because truthful state
cannot be published without the id) and maps to a new `PlayDelivered` plan:
the delivered stream plays directly, a local entry with the same id only
reuses its metadata while the server-resolved URI stays authoritative, and a
station unknown to the local catalog is appended as a minimal entry with its
exact catalog id (never an id recovered from the URL, live-control §4.3).
Verified by `cargo test` (130 unit + 2 integration, 0 failed) covering the
wire parse with/without `station_id`, the plan mapping, and playback without
local-catalog membership; `cargo fmt --check`, strict Clippy and
`git diff --check` pass. A fresh release binary was rebuilt for live testing.

## RC-4 — truthful runtime-state publisher (implemented locally, 2026-09-17)

RockCast now publishes its owned player's actual facts for the RockServer
directory projection (RS-8) instead of a playing-only approximation. The
`PlaybackPhase` state machine is mapped one-to-one onto the wire status:
`Opening` → `buffering`, `Playing` → `playing`, `Stopping` → `stopped`,
`Failed` → `error`, and a pristine `Idle` (no station chosen this session) →
`idle`. An `Idle` that follows a chosen station keeps `stopped`, so the stop
fact survives until a new choice. `paused` is never produced: pause stays
unimplemented and unadvertised, and `PlayerCommand::Pause`/`SetMute` are still
rejected as `capability_not_supported`.

`station_id` is now bound in `play()` to the exact catalog ID of the station
chosen for that start lifecycle (local pick, voice, or server-resolved
`station.play_station`/`station.play_stream` mapped through the local catalog);
it is never recovered from a stream URL. It survives `buffering`, `playing`,
`error` and `stopped` transitions per live-control §4.5 and is `null` only
before the first choice in a session. Volume echoes both local slider changes
and remote `volume.set_volume`/`change_volume` at the new percent level.

Publication triggers are registration/reconnect/resync (full snapshot after
every registration, unchanged) plus every real fact change: phase transitions
(including the start of buffering), station switches, and volume changes —
remote commands execute and publish inside one UI frame, local UI changes
publish on the following frame; the egui loop is verified to tick after each of
these events. Equal states stay deduplicated and do not advance the revision;
every changed state advances the strictly monotonic revision persisted in
`AppSettings.device_control_state_revision`, and a restarted process resumes at
`persisted + 1` without rollback. The manifest still advertises only
play/stop/next/previous for `media.playback`; no new transport, endpoint, or
capability was added, and no URL, header, or credential enters state or logs.

Deterministic coverage (fake socket/transport, no network, no audio): phase→
status mapping with station context, the full lifecycle
idle → buffering(A) → playing(A) → volume echo → error(A) → stopped(A) →
buffering(B) with per-fact revision increments and duplicate suppression,
registration snapshot shape (status, exact station_id, volume level, muted,
output), reconnect/resync resending the complete snapshot at the current
revision, and restart revision resumption. Checks: `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`
(129 unit + 2 integration passed; live-network tests remain ignored), and
`git diff --check`. Physical phone acceptance (Phase 4) and RockMobile's
state-driven UI (Phase 3) remain open.

## RC-3 — live RockMobile control accepted (2026-09-17)

The offline entry was caused by `registration_rejected`, not token renewal: the
native session and `protocol.welcome` both succeeded, but the server already stored a
different manifest at revision 2. RockCast now advances the changed declaration to
manifest revision 4, registers successfully, and the paired Android directory reports
the target online. Diagnostic logs record only lifecycle stage/error codes, never
credentials. One USB-phone `playback.stop` reached the player and RockMobile confirmed
the resulting state. `media.chromecast` and `media.relay` are deliberately withheld
from the manifest until their server router work (RS-7) is implemented; their local
adapters remain unchanged.

## Single Windows instance (2026-09-08)

RockCast now claims a named mutex before logging or application initialization. A second launch
therefore exits cleanly without truncating the running instance's log. It waits briefly for an
instance that is still creating its native window, restores that window if minimized, and asks
Windows to place it in the foreground. Both Russian and English window titles come from the same
i18n constants used by the UI, so changing a title cannot silently break activation.

The rebuilt debug executable was exercised in five consecutive two-process Windows runs. Each time
the first window was minimized, the second process exited with code 0, exactly the original PID
remained, and its window was both restored and reported by Windows as the foreground window. The
activation temporarily attaches to the current foreground input queue because Windows can reject a
plain `SetForegroundWindow` call from a background process; an initial smoke run reproduced that
OS restriction before this hardening.

## Play/Stop concurrency repair (2026-09-08)

The GUI hang had two concrete causes. `StreamObservers::stop` synchronously waited up to two
seconds for each of the ICY and spectrum readers while running on egui, which could stop Windows
message processing for about four seconds. Separately, `match rx.lock().recv()` kept the background
runtime's receiver mutex through execution of the selected match arm, silently serializing the
worker pool whenever a job blocked.

A subsequent physical GUI run found a third instance of the same Rust temporary lifetime trap.
The device-control loop retained `state.lock()` through an `if let` body and called `send_full`,
which attempted to acquire `state` again. On the first Play state publication, the connection
worker self-deadlocked and egui then blocked in `DeviceControlClient::publish`. The snapshot is now
extracted in its own scope before any send. The settings writer was hardened against the same
pattern so disk I/O cannot retain its pending-slot mutex.

Observer stop now only cancels and detaches the old reader. The receiver guard is scoped to
`recv`, and a deterministic test proves a second worker completes while the first remains blocked.
Playback has a dedicated bounded runtime, an immutable cancellation token per generation, and a
transition lock that linearizes teardown/start while remaining cancellable. Catalog, icons,
account, voice, pairing, and discovery use a separate bounded I/O runtime. Relay pre-buffer waits
observe cancellation. Settings persistence uses a latest-value slot and one-item wake queue rather
than filesystem sync on egui. Remote commands have a 64-item admission limit, process at most 16
per frame, and never hold their ledger mutex during a WebSocket send; device-control shutdown no
longer joins its network worker from window close.

Checks: `cargo fmt`, `cargo check --all-targets`, strict Clippy including
`clippy::significant_drop_in_scrutinee`, the new concurrency/observer/state/queue regressions, and
the full non-live suite passed (120 unit tests with one environment-specific DPAPI test filtered,
plus 2 integration tests; live network tests remain ignored). The unfiltered run's only remaining
failure is the pre-existing
`legacy_dpapi_blob_is_an_absent_session_not_a_storage_failure`, because this execution identity has
no interactive Windows user DPAPI key.

The rebuilt debug executable was then exercised through its real Windows UI Automation tree.
Local Play/Stop completed with 0 failed window-message pings (100 samples during Play, 60 during
Stop; worst 22/7 ms). Eight rapid Play→Stop cycles during HTTP probe also had 0 failures (worst
17 ms). A physical `Главная спальня` Chromecast completed relay, CastV2 LOAD to `PLAYING`, and STOP
with 0 failed pings (180/80 samples; worst 21/15 ms). WM_CLOSE exited in 84 ms. A new physical
RockMobile command was not injected during this run; its server path remains covered by the prior
DC-016 paired-mobile acceptance below.

## DC-016 — idle command wake-up and live E2E acceptance (2026-09-07)

When RockCast was idle, its device-control worker could enqueue `device.command` while egui had
no scheduled repaint, leaving the UI-owned executor asleep until the server deadline. Enqueueing
now requests a thread-safe egui repaint; the worker still performs no playback work and terminal
results remain authoritative UI outcomes. A physical paired RockMobile `Stop` traversed deployed
RockServer and the refreshed RockCast, with staging recording `succeeded` in under one second.

Checks: `cargo fmt --check` and `cargo test device_control --lib` (15 passed). This accepts
playback-command E2E only; real Chromecast hardware smoke remains unperformed.

## DC-014 — Chromecast and relay adapters (local implementation, 2026-09-06)

RockCast now advertises its real CastV2 actions (`discover`, `connect`, `disconnect`) with a
60-second receiver-cache TTL and its existing PC-to-Cast relay operations (`start`, `stop`,
`set_mode` with only `via_pc`). Discovery maps internal network receiver data to opaque UUID
handles local to the running player; the command result has the canonical bounded receiver list,
and neither a handle nor a receiver is registered, paired, persisted, or exposed as a control
target. Expired/missing handles and arbitrary hostnames are rejected before a hardware action.

The UI remains the exclusive PlaybackController owner. Cast connect, local fallback disconnect and
relay transitions complete only on an actual playback event; failures/interruption return one
terminal failure and publish the factual fallback output state. State contains exactly one output
mode (`local`, `chromecast`, `relay`) and includes `receiver_id` only for a known live handle.
WSS reconnect may retry an unsent result but cannot recreate a completed hardware operation.

Checks: `cargo fmt --check`, `cargo check --all-targets`, strict all-target/all-feature Clippy,
focused device-control tests and `cargo test` were run. The full suite had 116 passing tests plus
the pre-existing environment-specific DPAPI failure (`legacy_dpapi_blob_is_an_absent_session_not_a_storage_failure`): this sandbox lacks the interactive Windows user DPAPI key. No test was
weakened. No live Chromecast/network smoke was run, and Rockmobile/DC-015/DC-016 remain external.

## DC-013 — server-routed playback and volume commands (local implementation, 2026-09-04)

RockCast now strictly parses bounded `device.command` frames only after device registration and
requires their explicit target to match the server-authenticated device ID. It accepts the
truthfully advertised playback (`play`, `stop`, `next`, `previous`), station and volume commands,
deduplicates queued/in-flight/completed command IDs in a bounded process-local ledger, and returns
one terminal result only after the UI-owned `PlaybackController` has emitted its outcome. Completed
results whose send failed stay undelivered and are retried on a later authenticated connection;
disconnect never becomes a synthetic success.

`station.play_station` and RockServer-resolved `station.play_stream` map only to the current
validated local catalog (exact station ID or exact catalog stream URI). `direct_stream` is rejected;
no URL, identity, scope, output target, Chromecast or relay input is accepted from a command.
Pause and mute are parsed but rejected as `capability_not_supported`, because the existing
PlaybackController/manifest does not support them. State is published from the existing local facts
only after command handling; no remote optimistic playback, station, volume or mute value is made.

Checks passed: `cargo fmt --check`, strict all-target/all-feature Clippy, focused device-control
and playback-adapter tests, and `cargo test` (114 passed) in the interactive Windows profile. The
sandbox identity has no user DPAPI key, so the DPAPI test must run in that interactive profile; its
green result confirms the production credential path without weakening it. Live RockServer/Rockmobile
E2E was not run because it requires paired credentials and a deployed command router. The published
v1 error enum has no specific playback-failure/cancelled code, so actual playback
error/interruption is represented as a failed `command_timeout` result; this contract limitation is
isolated here rather than changing RockServer/OpenAPI. DC-014 remains the handoff for Chromecast and
relay commands.

## Structural refactor (2026-09-04)

The DC-012 device-control implementation is now split by lifecycle, v1 wire protocol,
tungstenite transport, and regression tests while preserving its existing public module path.
This is behavior-preserving only; deployed-control-plane E2E acceptance remains unverified.

## DC-012 — RockServer registered player (local implementation, 2026-09-04)

RockCast now reuses its existing DPAPI-protected `device_id` and durable device secret to renew a
native access token through `POST /api/v1/auth/device-session`, then maintains one bounded WSS
device-control v1 loop at `/api/v1/devices/connect`. It registers only as a `player`, publishes
only local playback/station and volume facts, sends a full snapshot after every registration or
resync, heartbeats every 20 seconds, and reconnects with bounded deterministic jitter. Local radio
playback remains independent when the server, token renewal, or WSS is unavailable; a revoked
credential is handled by the existing session renewal path.

The v1 assumptions are `hello → welcome → register → registered → state_full`, 65,536-byte frames,
61,440-byte payloads, and server-derived identity (no identity or secret is sent in protocol
messages). The manifest intentionally excludes Chromecast, relay, display, voice, Home Assistant,
and mute. DC-013 is implemented locally as documented above; live deployed-control-plane E2E
remains unverified, so this is not marked as full acceptance complete.

## Authenticated voice route (implemented locally, 2026-09-02)

Voice uses one `wss://.../api/v1/voice/stream` endpoint. If the PC has a durable paired session,
it first renews the short-lived access token through `POST /api/v1/auth/device-session` and sends
that token with the WebSocket handshake. A failed renewal uses the same endpoint anonymously
without clearing the durable binding; only the existing revoked-credential handling clears it.
The namespace and token-renewal regressions are deterministic.

Verified: `cargo fmt --check`, strict Clippy, and `cargo test` (97 unit tests plus non-live
integration coverage) passed. The matching RockServer API change is local; no deployment or live
voice request was made.

## RM-011 device-secret native sessions (implemented locally, 2026-08-30)

RockCast now treats pairing as a durable device binding. The DPAPI-protected credential contains a
`device_id`, a persistent `device_secret`, and a replaceable access token. On an expired access
token it calls `POST /api/v1/auth/device-session`; network and server failures keep the binding, while
only `401 device_credential_invalid` clears it. The legacy refresh-token endpoint and rotating
refresh-token recovery path are no longer used by this client. This requires the corresponding
RockServer API change before end-to-end use. Verified: `cargo fmt --check`, strict Clippy, and
`cargo test` (94 unit tests plus non-live integration coverage) passed.

## RM-011-09 — Wave 9 A4 secure pairing handoff (complete locally, 2026-08-29)

RockCast now constructs its QR, copy and open-link payload through the existing `PairingRequest`
helper as `?code=<code>#secret=<proof>`. The one-time approval secret remains only in process
memory, is never shown as text or logged, and is no longer in the URL query. A deterministic unit
test locks the exact fragment shape.

Local checks passed: `cargo fmt --check`, strict `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo test` (93 unit tests; live-network tests intentionally ignored), and final
`git diff --check`. No server/API/OpenAPI change, push, deploy, staging mutation or real pairing
flow occurred.

## RM-011 Wave 4 — C4–C8 account UX (complete, 2026-08-29)

The Account & devices dialog now renders one localized state at a time in both
Russian and English. The browser-approval screen explains the next steps,
shows an expiry countdown, and provides a primary secure-link action plus a
copy warning. Its QR is rendered with error correction M, a four-module quiet
zone, and integer-size modules in a 256–320 logical-pixel target; the link and
QR payload are never logged or shown as text.

Successful pairing moves atomically to a dedicated success screen, whose
primary action opens devices and whose secondary action closes the dialog.
The connected centre keeps the current PC first, gives it only local logout,
and offers confirmed disconnect only for other devices. It distinguishes an
empty list from an unavailable list, formats dates for the selected language,
and never renders identifiers, sessions, proofs, or tokens. Closing or
cancelling the waiting screen stops the local polling job; no server cancel
endpoint was added.

Local verification: `cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test`, and `git diff --check`.

## RM-011-G4 — clear PC connection UX (complete, 2026-08-28)

The Account & devices dialog now connects the current PC to an existing Rock account instead of
presenting RockCast as a separate registration. It starts the published G1 pairing request with
`device_display_name` and `device_type`, offers the default `RockCast — <PC name>` for editing,
and renders the G2 request-specific QR/deep-link fallback, short code, verification phrase,
expiry, status and cancel action.

Completion sends only the desktop proof and accepts the server-derived account/device display
context. The UI does not render UUIDs, `user_id`, device proofs or native tokens. It shows
`This PC is connected to account <account_display_name>` and the current device name, and uses
the published native device list/revoke endpoints. Polling stops on server errors, cancellation
or a bounded timeout; anonymous/offline playback remains independent. Browser rename and any
additional device-center operations remain the G3 browser dependency, and physical passkey/phone
acceptance remains G7.

Local verification for this change: `cargo fmt --check`, `cargo check --all-targets`, and
`cargo test --all-targets` (88 passed; live network probes remain ignored). No RockServer,
deployment, commit or push was made.

## RM-011-E — account and secure session UX (complete, 2026-08-26)

RockCast now has an optional Account & devices dialog. It creates a desktop pairing request via
the deployed `/api/v1/pairing-requests` contract, renders the one-time browser deep link as a QR code,
and displays the short code and verification phrase. The desktop proof and approval secret remain
only in process memory. Native access/refresh credentials are stored only in a Windows DPAPI
protected blob (`session.dpapi`); a DPAPI failure leaves RockCast anonymous/offline rather than
falling back to plaintext settings. The dialog supports silent refresh before profile/device reads,
remote logout followed by local cleanup, and owner-scoped device revoke. No token is shown or
written to logs.

RockCast polls completion automatically after browser/passkey approval. Its exact request body is
only `{ "desktop_token": "…" }`; it never asks for or sends a user ID, and the returned profile
is accepted only from the server. Local mock HTTP tests cover create/poll/complete, rejection of
the former extra `user_id`, refresh replay cleanup and offline logout cleanup; full `cargo test`
passed (85 unit tests and 2 local relay integration tests; 10 live-network tests remain ignored).
The Windows DPAPI calls compile on this host
but have not been exercised against a real Windows user profile in CI.

`cargo fmt --check` and `git diff --check` pass. Strict all-target Clippy reaches one pre-existing,
unrelated `clippy::too_many_arguments` diagnostic in `src/local/mod.rs::play` (8 arguments); no
new RM-011-E diagnostics remain.

## Station icons MVP

Implemented for the pre-RockServer-icon phase (2026-08-26).

- RockCast fetches a valid station `favicon_url` directly from the station's
  HTTP(S) server. If that field is absent, it may fetch the conventional
  `/favicon.ico` from the configured official `homepage_url`; it does not
  scrape homepage HTML.
- Fetching, bounded response reads, image decoding, and disk cache I/O run in
  the existing `BackgroundRuntime`, never on the egui thread.
- ICO, JPEG, and PNG payloads are accepted, bounded to 512 KiB on the wire and
  decoded to a maximum 64px thumbnail. Invalid, oversized, unsupported, or
  failed payloads keep the existing text-only station row.
- Successful thumbnails are cached in the platform app-data directory under
  `station-icons`. The cache filename is a safe hex-encoded station key and
  the stored source URL invalidates stale metadata. Requests are attempted at
  most once per station/source identity per app session.
- RockServer and voice station DTO adapters preserve optional `homepage` and
  `favicon` fields for this client-side MVP. No RockServer endpoint or database
  migration is part of this change.

The embedded catalog currently has no homepage/favicon metadata for its
stations, so those rows intentionally remain text-only until catalog or
RockServer metadata supplies a permitted source URL.

## MVP-001-C — zero-configuration official RockServer client

Implemented and locally verified on 2026-08-26.

- Official releases use `https://alex.vault57.ru` without user configuration.
- Public search uses `POST /api/v1/search` without Bearer authorization. Voice
  preserves TLS by mapping HTTPS to WSS and uses `/api/v1/voice/stream`, also
  without Bearer authorization.
- RockServer URL/token controls and persisted RockServer settings were removed.
  Legacy JSON fields are ignored and scrubbed during settings migration.
- Endpoint, optional Bearer token, and streaming-mode overrides exist only for
  debug/test runtime through `ROCKCAST_DEV_ROCKSERVER_*`; release builds ignore
  them, and their values are neither displayed nor logged.
- The embedded catalog is delivered before the public request. A failed or
  empty public response continues through the existing local catalog + Radio
  Browser path, so local selection and playback do not depend on RockServer.

The client follows the deployed RockServer runtime contract from MVP-001-B.
Legacy `/api/v1` aliases remain intentionally unused because they are
Bearer-protected. No RockServer or OpenAPI repository was changed here. If the
published OpenAPI still applies global Bearer security to these allowlisted
`/v1` operations, that documentation/runtime mismatch remains an external
contract-documentation issue, not a reason for the client to send a token.
