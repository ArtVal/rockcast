# RockCast tasks

## RC-4a — accept the server-delivered `station.play_stream` (fixed, 2026-09-17)

- Symptom: remote play commands from RockMobile were rejected with
  `invalid_payload` ("Command cannot be applied to local playback state").
- Root cause: the RS-3 delivery carries `{ station_id, stream_uri }`, but the
  parser dropped `station_id` and the planner matched the URI against the
  local station list; stations from the phone's server catalog are absent
  there, so every such play failed the lookup.
- Fix: parse and require `station_id` on the catalog-source delivery; map it
  to a `PlayDelivered` plan that plays the resolved stream directly (local
  id matches contribute metadata only, unknown ids are appended with their
  exact catalog id per live-control §4.3).
- Checks: `cargo fmt --check`; `cargo clippy --all-targets --all-features --
  -D warnings`; `cargo test` — 130 unit + 2 integration passed, 0 failed;
  `git diff --check`. Release binary rebuilt; live retest pending.

## RC-4 — publish truthful playback and volume state for RockMobile (2026-09-17)

- Goal: publish the owned player's complete revisioned state — exact catalog
  `station_id`, truthful playback status and volume — after
  registration/resync and after every real local/remote fact change, so a
  succeeded command result is never mistaken for playback confirmation.
- Scope: map `PlaybackPhase` one-to-one onto the wire statuses
  (`Opening`→buffering, `Playing`→playing, `Stopping`/post-playback
  `Idle`→stopped, `Failed`→error, pristine `Idle`→idle; `paused` never
  published); bind the exact station ID of each start lifecycle in `play()`
  so it survives error/stopped (§4.5); extract the deduplicating
  revision-advancing publisher step as a testable pure function. No manifest,
  transport, endpoint or capability change; pause/mute remain rejected.
- Result: fake-socket tests prove the lifecycle idle → buffering(A) →
  playing(A) → volume echo → error(A) with kept station → stopped(A) with kept
  station → buffering(B), one revision per changed fact and none for
  duplicates, the registration/reconnect/resync full snapshot at the current
  revision, and restart resumption at the persisted revision + 1.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features
  -- -D warnings`, `cargo test` (129 unit + 2 integration passed; live-network
  tests remain ignored), `git diff --check`. `cargo fmt` also reflowed
  `src/voice/mod.rs`, which was committed unformatted for the current
  toolchain.
- Status: local implementation complete per live-control Phase 1 (RC-4).
  Physical USB-phone acceptance (Phase 4) is not performed and remains open,
  as does the RockMobile state-driven UI (Phase 3).

## RC-3 — restore live Windows control acceptance (2026-09-17)

- Root cause: an already-persisted revision-2 manifest differed from the current
  RockCast declaration, so RockServer correctly returned `registration_rejected`.
- Fix: publish revision 4 and lifecycle diagnostics; omit unrouteable relay/Cast
  capabilities until RS-7. Focused `cargo test device_control --lib` passed (17/17)
  and the release binary was rebuilt.
- Live acceptance: a USB-connected RockMobile selected the now-online target and its
  one standard `playback.stop` ended with confirmed player state.

## DC-016 — idle command wake-up and live E2E acceptance (2026-09-07)

- Fixed the missing UI wake-up after the control worker enqueues a server-delivered command. The
  UI remains the sole `PlaybackController` owner; the worker only requests an egui repaint, so an
  idle player consumes the bounded queue without moving playback work to a background thread.
- Live result: a paired physical RockMobile sent `Stop` through deployed RockServer to RockCast.
  The terminal lifecycle was persisted as `succeeded` in under one second. The preceding timeout
  was reproduced as an accepted but unpolled UI queue and is now covered by the command wake test.
- Checks: `cargo fmt --check` and `cargo test device_control --lib` (15 passed).
- Status: deployed-control-plane playback command E2E accepted. Chromecast hardware smoke remains
  separate and unperformed.

## DC-014 — Chromecast and relay adapters (local implementation, 2026-09-06)

- [x] Advertise only `media.chromecast` (`discover`, `connect`, `disconnect`, 60-second TTL) and
      `media.relay` (`start`, `stop`, `set_mode`, only `via_pc`) that map to the existing CastV2
      and PC-to-Cast relay paths.
- [x] Keep discovery receivers as opaque process-local UUID handles. Results contain only the
      canonical receiver schema; no receiver becomes a device-control target or durable identity.
- [x] Require a fresh handle and a selected local catalog station before Cast side effects. A
      transition succeeds only after `PlaybackController` reports the observed Cast/local/relay
      outcome; interrupted/failed operations return one terminal failure.
- [x] Publish factual mutually-exclusive `output` state (`local`, `chromecast`, `relay`) and a
      receiver ID only when RockCast knows its ephemeral handle. Server reconnects only retry an
      undelivered terminal result, never replay hardware work.
- [x] Cover bounded parsing, opaque cache/TTL, discovery result shape, duplicate lifecycle and
      output capability manifest without a live LAN receiver.
- [ ] Real Chromecast smoke remains unperformed: no configured safe receiver was used. Rockmobile
      target UI and DC-016 remain outside RockCast.

## DC-012 — registered player transport (local implementation, 2026-09-04)

- Reused the existing paired native credential and device-session renewal; no new identity, pairing,
  token, or credential store was introduced.
- Added the bounded control-plane connection lifecycle, truthful player manifest/state publication,
  registration/reconnect snapshot resync, and safe malformed/unknown-frame handling.
- Deferred exactly to DC-013: parsing or executing `device.command`, command lifecycle results, and
  state confirmation after remote actions. Chromecast/relay/display/voice/HA stay absent from the
  manifest.
- Remaining acceptance blocker: run a real RockCast-to-deployed-RockServer device-control E2E smoke
  after the target endpoint is available; local playback fallback must be observed there too.

## Voice route selection — 2026-09-02

- Goal: keep anonymous voice usable while sending paired RockCast voice sessions with a current
  native-session token.
- Result: all voice sessions use `/api/v1/voice/stream`; a persisted credential is renewed through
  `/api/v1/auth/device-session` and sent as Bearer when available. Renewal failures preserve the
  binding and use that same voice path anonymously.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo test` passed; external stream probes remain ignored.
- Status: **implemented locally.**

## RM-011 — 2026-08-30 — durable device-secret client sessions

- Goal: remove refresh-token rotation from RockCast so a transient renewal failure cannot disconnect
  a paired PC.
- Scope: replace the persisted refresh token with `device_id` and `device_secret`, request a short
  access token from `/api/v1/auth/device-session`, and revoke the device for explicit disconnect.
- Result: only an explicit `device_credential_invalid` response clears the local DPAPI credential;
  unavailable or malformed renewal responses preserve it.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and
  `cargo test` passed.
- Status: local client implementation complete; matching RockServer contract remains required for E2E.

## RM-011 Wave 9 — A4 secure pairing handoff (complete, 2026-08-29)

- [x] Generate all existing QR/open/copy handoffs through the shared fragment-based link helper.
- [x] Keep the approval secret out of query URLs, UI text and diagnostics; retain it in memory
      only until normal pairing completion/cancellation.
- [x] Add a deterministic exact-shape test and pass Rust formatting, strict Clippy and tests.
- [ ] Production App Link association remains external to RockCast and was not deployed or claimed.

## RM-011 Wave 4 — C4–C8 account UX (complete, 2026-08-29)

- [x] Provide localized browser-approval steps, parsed-expiry countdown, secure-link open/copy
      actions, and a no-share copy warning; cancel/close stops only the local polling job.
- [x] Render a 256–320 logical-pixel QR with M correction, four quiet-zone modules, and integer
      module scale without logging or displaying the link payload.
- [x] Transition pairing success atomically to its own screen with primary device-centre and
      secondary done actions.
- [x] Keep the current PC first in the account centre; use local logout for it and confirmed
      remote disconnect only for other devices, with localized dates and distinct empty/unavailable
      device-list states.
- [x] Localize all account UI through `i18n::Strings` and `Lang`; account paths emit no URL,
      query, QR payload, short code, phrase, proof, token, or account/device identifier diagnostics.

## RM-011-G4 — clear PC connection UX (complete, 2026-08-28)

- [x] Call the action “Connect this PC to an account”; pair with a user-editable,
      server-validated default `RockCast — <PC name>`.
- [x] Use the G1 create/complete DTOs and the G2 request-specific browser link; show QR,
      fallback link action, short code, verification phrase, device name, status, expiry and cancel.
- [x] Show the approved account/device display names without rendering UUIDs, `user_id`, proofs or tokens.
- [x] Keep anonymous playback independent from account availability; timeout, cancellation,
      secure-storage failure and offline errors leave local radio available.
- [x] Connect the published native device list/revoke endpoints to Account & devices.
- [ ] Browser-side rename and richer device management remain the G3 surface; no new server API
      was invented here.
- [ ] Physical staging phone/passkey smoke test remains part of G7.

## RM-011-E — account and secure session UX (complete)

- [x] Create/complete native desktop pairing through the RM-011-C `/v1` contract.
- [x] Show a QR/deep link, short code and verification phrase; keep pairing proofs memory-only.
- [x] Store native credentials with Windows DPAPI, fail closed if protected storage fails,
      and support refresh, local cleanup/logout, device list and revoke.
- [x] Cover client requests with local mock HTTP tests; no live RockServer is used.
- [x] Finish pairing automatically using only the request ID and one-time desktop proof; the
      approved owner is derived by RockServer.

## MVP-001-C — official RockServer defaults

- [x] Use the production HTTPS RockServer base URL in official releases.
- [x] Remove RockServer URL/token requirements from ordinary user settings and UI.
- [x] Call public `/api/v1/search` and `/api/v1/voice/stream` without Bearer authorization.
- [x] Preserve HTTPS-to-WSS TLS and avoid legacy protected `/api/v1` aliases.
- [x] Isolate endpoint/token/voice-mode overrides to debug/test runtime without
      displaying or logging their values.
- [x] Preserve local catalog, Radio Browser fallback, and playback when the
      public API is unavailable.
- [ ] Reconcile published RockServer OpenAPI security metadata with the
      endpoint-level public allowlist in the RockServer repository, if still stale.

## Station icons

- [x] Add direct client-side favicon/logo loading for the MVP.
- [x] Keep HTTP, decoding, and cache work off the UI thread.
- [x] Add bounded response/image limits, HTTP(S)-only validation, safe cache
      keys, URL-based cache invalidation, and deterministic offline tests.
- [x] Preserve optional homepage/favicon metadata from RockServer search and
      voice responses.
- [ ] Populate the shared catalog with reviewed station favicon/logo metadata.
- [ ] Replace the direct-client source with the RockServer-hosted icon contract
      once server support lands.
