# RockCast status

## Voice search pagination, table geometry fit, and scroll controls (2026-09-30)

- **Voice search pagination & infinite scroll**: `handle_voice_result` now automatically initiates a catalog
  search for `clean_query` via `search_stations` while immediately starting playback of the top candidate.
  The station table receives full paginated results with infinite scroll enabled, displaying `Все (20 из 448)`
  instead of freezing at `Все (10)` with disabled pagination.
- **Playing station preservation**: In `handle_stations_loaded`, if the currently playing station is absent from
  the new search page, it is prepended to the top of the loaded stations list (`preserve_playing_station`),
  maintaining active playback indicators and selection. Selection restoration prioritizes `playback_station_id`.
- **Table width overflow fix**: Column widths are strictly clamped to the viewport content bounds
  (`(viewport_rect().width() - 56.0)`). Filter chips are split into two clean rows (Row 1: views and genres;
  Row 2: facets, reset, counts), completely eliminating horizontal table overflow.
- **Scroll controls**: Floating «Наверх» and «К играющей» buttons are rendered in `egui::Area` (`Order::Foreground`)
  preventing click drop-through to underlying station rows. `resolve_playing_station_index` prioritizes
  `playback_station_id`, and `icon_locate.png` provides a crisp reticle asset.
- Checks: `cargo check --all-targets` (0 warnings), `cargo test --lib` (193 tests passed).

## Favourite display names recovered from history (2026-09-28, follow-up)

Favourites applied by server sync arrive without display names (the RM-012-A contract carries no
favourite metadata), which made their table rows show raw station ids. A backfill now recovers
the name from the newest history entry of the same station — both when the profile opens and
after every sync response is applied (history records do carry `lastKnownName` metadata).
Favourite resolution failures also log start/outcome, and when both Radio Browser and RockServer
are unreachable the status says so explicitly instead of a bare «не найдена».

Note: the running app binary must be rebuilt and restarted to pick up the muted-row play button
and this backfill; a stale running `rockcast.exe` also blocks rebuilding the default target.

Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test` (178 unit tests) — green. New tests cover open-time and sync-apply-time backfill.

## Muted favourite rows are playable (2026-09-28, follow-up)

The play button on a muted favourites row now resolves the station on demand and starts playback.
Resolution runs on the background runtime: Radio Browser first — searching by the last known name
and matching RockCast's stable `radio-browser-<hash(url)>` id (an exact stream-identity match),
with a unique case-insensitive name match accepted when the stream was rehosted — then the
RockServer search as the second source (same matching). While resolving, the row button is dimmed
and the status shows «Ищу станцию …». On success the station is inserted at the top of the loaded
list, selected and played; when it resolved under a new id (rehosted stream), the favourite is
repointed (remove + re-add, which syncs as delete+upsert). Ambiguous name matches never play a
wrong station: the row reports «не найдена» instead.

Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test` (176 unit + 2 integration tests) — green. New unit tests cover query encoding and
the exact-id/unique-name/ambiguous matching matrix without network access.

## Favourites tab shows the whole profile list (2026-09-28, follow-up)

The station-table Favourites filter no longer intersects the profile with the currently loaded
station list. It now renders every favourite record: stations present in the loaded list as
normal playable rows, and favourites whose station is not loaded (Radio Browser/RockServer ids
from other sessions, phone-synced records) as muted rows with the last known name, a monogram,
and a «станция недоступна в текущем списке» hint; a hover tooltip explains that searching for the
station makes it playable again. The star on a muted row removes the favourite
(`PersonalDataStore::remove_favourite` by station id, wired into the sync debounce), since
`toggle_favourite` requires a loaded `Station`. Zebra striping follows the visible row position.
The History filter and All mode are unchanged.

Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo test` (172 unit + 2 integration tests) — green.

## Quarantine retired for personal records (2026-09-28, follow-up to RM-012-B)

The RM-007-A lifecycle quarantine is disabled per user request: opening the profile no longer
moves favourites/history whose `station_id` does not resolve in the current catalog into
`unresolvedReferences`. Such records now simply stay in their collections — they are listed with
their last known name and play nothing until the station is discoverable again. This also removes
the RM-012-B hazard where a slim offline catalog (Radio Browser/RockServer unreachable) made the
next sync cycle mistake quarantined records for user deletions and tombstone them server-wide.

- One-time restore migration: existing `unresolvedReferences` entries (favourite and history
  kinds) move back into the live collections on the next profile open, keeping their original
  station ids, `reference_id` as `record_id`, and `first_seen_at` as the timestamp. The usual
  pre-migration backup and journal (`restore-and-remap`) are written first.
- Legacy-id remapping and duplicate-favourite collapsing at open are kept; only the
  quarantine/`unresolved_for` path was deleted. `clear_history` no longer touches the (now
  permanently empty) quarantine list.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` (171 unit + 2 integration tests) — green. New tests cover the survive-untouched
  and restore-on-open paths.

## RM-012-B — client favourites/history sync with RockServer (2026-09-28)

RockCast now converges favourites and playback history with RockMobile through the deployed
RM-012-A `POST /api/v1/sync` contract (OpenAPI 0.6.0, rockserver 2d8e27f). The local RM-007-A
profile stays the full offline fallback: without an account session or network, every
favourite/history feature works exactly as before, and sync only runs in the background.

- **Sync engine** (`src/personal_sync/`, no UI logic): per-device state
  (`personal-sync-state.v1.json`) keeps the last `server_revision` cursor plus the exact record
  versions the server acknowledged; diffing the live profile against that base yields the next
  push (upserts + tombstone deletes). Oversized pushes are split into server-legal requests
  (≤300 upserts and ≤300 deletes per collection) with the cursor threaded through chunks.
- **Application rules**: each incoming record applies last-writer-wins on `updated_at`
  (strictly newer replaces, ties keep the local record); `deleted_at` deletes locally by
  `record_id`; losing push echoes arrive as ordinary records and lose honestly. The cursor is
  persisted only after the response is durably applied to the profile, so a crash mid-cycle
  merely re-pulls the same delta (verified idempotence test).
- **Tombstone safety**: a local deletion is pushed with `updated_at` strictly newer than the
  newest version this device ever saw (including other devices' future-dated clocks), so deletes
  cannot be resurrected by an older replay; a recreated/lost profile file or a freshly paired
  device resets the per-device state to a full-snapshot pull instead of tombstoning the account.
- **Lifecycle reuse**: coalescing (5 min), caps (500/500), and 90-day retention stay in
  `PersonalDataStore`; the sync layer never duplicates them. History entries gained an
  `updatedAt` sync field (backfilled from `lastPlayedAt` for pre-RM-012-B files) and history
  metadata now passes unknown keys through verbatim for cross-client round trips.
- **Transport**: same native device-session Bearer as device-control, one 401-driven session
  renewal per request, 429/5xx mapped to exponential backoff (1→15 min) on the scheduler,
  422 surfaced by `details.field` only (no payloads), 15 s request timeout.
- **Triggers**: app start (first periodic tick once a session is active), debounced local edits
  (10 s after a favourite toggle, history record, or clear), and a periodic pull every 5 minutes.
  Everything runs on the existing `BackgroundRuntime`; the UI thread only applies results.
- **Diagnostics**: the account panel shows the sync phase («выполнена …» / «ошибка» / «ожидает»)
  with local time; METRICS lines gained a `sync=` field (off/idle/ok/error). Logs carry counters
  and phases only — no tokens, identifiers, or record payloads.
- **Checks:** `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test` (170 unit + 2 integration tests) — green. Sync tests run on a scripted fake
  channel: LWW incl. tombstones and echoes, cursors, chunk splitting, 401→renew→retry,
  429/422/503 classes, first-sync full push+snapshot, and crash-idempotence.
- **Pending:** the two-device manual acceptance (favourite added on one device appears on the
  other; deletion arrives as a tombstone) runs after the client build is deployed.

## Remote playback track publication (2026-09-26)

RockCast now publishes an optional bounded `track_title` with its revisioned device-control playback state. The title comes from observed ICY/relay metadata, is cleared on station start/stop/error, and is omitted when absent. The current Windows release binary was installed and restarted; a paired RockMobile changed stations twice and displayed distinct current titles from the deployed RockServer directory. `cargo test`, strict Clippy and release build passed. `cargo fmt --check` still reports pre-existing formatting outside these edits.

## RC-7 — UI polish pass: polished table, deck redesign, asset icons (implemented locally, 2026-09-25)

The approved mockup (`design/mockups/variant-a.html`) was implemented on top of the RC-6 overhaul
after hands-on review. All four review complaints are closed: clipped edges, unclear play
affordance, mixed font sizes, and the raw account window.

- **Single type scale** (`theme.rs`): 10.5 / 11.5 / 12.5 / 13.5 / 18 px only; the app title shrank
  22 → 18; page margins are 16 px in the central panel and the player deck.
- **Station table**: rows 36 → 44 px; the round 30 px play/pause button on every row is always
  visible (outlined tile, accent fill on hover/playing) and sits 16 px off the panel edge; the
  playing row shows an accent tint, left bar and an animated 4-bar equalizer after the name; the
  horizontal scrollbar is gone because row width follows the scroll area's inner width (the
  floating scrollbar reserve is accounted for in the header alignment too).
- **Player deck redesign**: a single 64 px row on one vertical center line — 48 px station art, a
  transport button pinned to the exact panel axis (start/stop toggle), a spectrum at full row
  height centered in the right half; station info is two painter-drawn lines whose heights are
  measured from the font, so the block is exactly vertically centered; the destination string
  moved out (the status bar already reports it).
- **Deck footer**: volume moved into the status bar — speaker mute icon, a thin custom 3 px slider
  and a percent readout placed left of the RockServer label; the status label is left-aligned with
  a reserved right-side width so it can never push the group off the panel.
- **Search row**: the embedded action button is flush with the input's right edge; an empty field
  shows «Голос» (mic asset; click or Enter starts voice input), typed text switches it to an
  accent «Найти» (text only, no mic); the clear control is an icon-asset button.
- **Account window**: non-resizable card anchored under the Аккаунт button with «Этот компьютер» /
  «Другие устройства» sections, green «Подключено» chips, icon tiles per device, danger-styled
  logout/disconnect and footer buttons in one row.
- **Icons are assets only** (AGENTS.md rule): `icon_mic` regenerated in near-white so ACCENT
  tinting works (the old orange fill multiplied into an unreadable blob), new `icon_pc`,
  `icon_phone`, `icon_clear`; vector-drawn and emoji glyphs (tofu «✓»/«✕»/«⚠») removed.
- **Metadata race fix** (`actions/poll.rs`): a stream often delivers its first ICY title before
  PlayOk (the playout buffer fills later); PlayOk no longer overwrites `self.track` unconditionally
  and shows the hint only when no real title has arrived. Stream titles are logged
  (`stream title: …`) for diagnostics.
- **Checks:** cargo fmt, cargo clippy `-D warnings` (0 warnings), cargo test (139 unit +
  2 integration tests) — green.

## RC-6 — Rock-styled UI/UX overhaul and mobile voice search parity (implemented locally, 2026-09-25)

Desktop UI and voice search UX were overhauled to match the RockMobile flow and the approved interactive prototype (`rockcast_ux_prototype.html`), with all visual glitches, column collisions, and missing glyphs resolved:

- **Unified bottom player bar (`draw_player_deck`):** Now Playing metadata, station monogram with individual rock palette color, pulsing live dot, 60 FPS spectrum analyzer with toggle, 1-click mute/unmute button, volume slider with monospace percent readout, circular Play button in ACCENT, Stop button, and bottom status sub-bar are consolidated into a single cohesive panel. Strict 3-region horizontal bounding eliminates widget overlaps between the spectrum analyzer and volume slider.
- **Embedded PNG UI icons & font-glyph safety (`AppIcons`):**
  - Integrated 64x64 RGBA antialiased PNG assets (`assets/icon_search.png`, `assets/icon_mic.png`, `assets/icon_speaker.png`, `assets/icon_speaker_mute.png`) loaded via `src/app/icons.rs` without external dependencies.
  - Eliminated missing emoji replacement squares (`□`): search, microphone, speaker, and speaker mute render crisply via textured quads.
  - Removed emojis from buttons (`Каталог`, `История`, `Stop`) and replaced non-breaking hyphens (U+2011) with ASCII hyphens (`Wi-Fi`).
- **Table layout & column collision fix:**
  - Fixed column span calculation: `available` width strictly excludes the dedicated rightmost play button column (`play_col_w = 48.0`) and left offset (`col_name_x = 68.0`), providing guaranteed 24px clearance between the country badge and row Play button.
  - Center-aligned country badge and header directly within the country column (`COUNTRY_COL_W = 54.0`).
- **Compact device controls:** Inline relay checkbox placed directly on the device row next to the acoustic search button, removing the wasteful separate frame.
- **Voice search mobile parity & auto-stop:**
  - Microphone capture automatically commits and sends audio to RockServer upon a 1.0s speech pause without requiring the user to press Stop manually.
  - Startup grace period (350ms) ignores prompt beep playback and mouse clicks. Active radio playback is stopped immediately upon mic activation to eliminate speaker bleed.
  - Dynamic ambient noise calibration computes adaptive speech threshold clamped within conversational bounds (280..520 RMS).
  - Search input container embeds a 1-click stateful `[Голос]` / `[Стоп]` / `[Распознаю…]` button with embedded mic icon, an instant `✕` clear button, and Enter key binding for empty text.
- **Station list table polish:**
  - Individual rock brand palette for station monograms (`station_color`).
  - 1-click favourite star toggle on each station row.
  - 1-click circular play/pause button on every row.
  - Left accent border and glowing amber highlight on selected station row.
  - Instant client-side genre chip filtering.
- **Checks:** `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` (0 warnings), and `cargo test` (139 unit + 2 integration tests passed, 0 failed).

## RC-5 — rockplatform.win domain and server-owned station icons (implemented locally, 2026-09-24)

RockServer production moved to `https://rockplatform.win`, so
`PRODUCTION_BASE_URL`, the voice/session test literals and the account
pairing deep-link expectations now use the new domain. The server search DTO
publishes a nullable same-origin `favicon_url` path
(`/api/v1/stations/{id}/icon`), which the pre-server MVP icon loader would
have rejected as a non-absolute URL.

`station_icons::source_url` now resolves such relative paths against the
configured RockServer base URL (borrowed from the runtime config in both the
job scheduler and the station table), so RockServer stations fetch their icon
from RockServer only — never from the station itself. Protocol-relative
(`//host/...`) and non-rooted sources are rejected; an absolute favicon URL
(an offline-catalog station) still wins; the homepage `/favicon.ico` fallback
without scraping remains only for stations without a server icon URL. The
existing bounded download/decode/file cache is unchanged, and WebP decodes
through the same `image` limits. Checks: `cargo fmt --check`, strict
all-target/all-feature Clippy, and `cargo test` (136 unit tests passed; live
tests remain ignored by their gates). Not yet exercised against the live
server from the physical target.

## RC-4b — name and future icon in a server-delivered station (implemented locally, 2026-09-17)

The fallback entry introduced by RC-4a used the catalog ID as its local name.
It was correct as identity but rendered an opaque UUID in the `Now playing`
panel when RockCast did not already have that station in its own cache.

The server-to-target `station.play_stream` contract now has a bounded optional
`station` presentation object. `station_id` remains the exact state identity;
`station.name` is display-only and is used for both a new fallback entry and an
existing local entry. `station.icon_url` is retained as a nullable future field:
the current server catalog has no stored icon, so deployed server output will
honestly contain `icon_url: null`. If it is later present, RockCast keeps the
URL in the existing station-icon path and queues the existing bounded image
fetch; no catalog dump, new fetch API, or stream-URL-derived icon is added.

For a non-breaking rollout, install this RockCast build before the server that
emits `station`: this build accepts an older delivery without the object and
falls back to the ID, whereas an older strict parser rejects the new field.
The display object is not published as device state and does not change the
exact `station_id` sent to RockServer. Checks: `cargo fmt --check`, strict
all-target/all-feature Clippy, and `cargo test` (131 unit + 2 integration
passed; live-network tests ignored by their explicit gates). Physical
RockMobile acceptance with the new server delivery remains pending. **Rollout
note:** RockServer `509ea0c` was deployed on 2026-09-17 by explicit
server-only request before this local commit is installed on the Windows target.
Do not issue a new catalog selection from RockMobile until this RC-4b build is
launched; the old strict parser rejects the new `station` field.

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
