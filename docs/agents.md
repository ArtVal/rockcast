# Guide for coding agents

Read this first when editing RockCast. Then open [architecture.md](architecture.md) and the module you will change.

## What this project is

Desktop **internet radio** app (Windows + Linux): egui UI, local cpal playback (WASAPI / ALSA), native Google Cast client. Not a web app. Not using Chrome Cast extension APIs.

## Where to change what

| Task | Primary files | Avoid |
|------|---------------|--------|
| UI layout / controls | `src/app.rs` | Putting network I/O on the UI thread |
| Playback state / orchestration | `src/playback.rs` | Moving Cast/local/relay control back into `app.rs` |
| Background jobs / shutdown | `src/runtime.rs` | Unbounded app-level `thread::spawn` |
| ICY / spectrum lifecycle | `src/observers.rs` | Owning observer threads from the UI |
| i18n strings | `src/i18n.rs` | Hardcoding user-visible RU/EN in other modules |
| PC audio bugs | `src/local.rs` | Holding UI while dropping `cpal::Stream` |
| Cast play/stop/volume | `src/cast/client.rs` | Outer `Mutex` around whole `play()` |
| Cast TLS/framing | `src/cast/channel.rs` | Blocking reads without cancel/timeout |
| Cast discovery / VPN | `src/cast/discovery.rs` | Assuming mDNS alone is enough |
| Protobuf wire format | `src/cast/proto.rs` | Treating `proto/*.proto` as codegen input |
| Station list | `src/stations.rs`, `stations.txt` | Blocking UI on Radio Browser |
| Settings / log path | `src/settings.rs`, `src/main.rs` | |
| Cast LAN relay (VPN→JBL) | `src/relay.rs`, `src/app.rs` | Advertising a VPN interface IP to Cast |
| Spectrum / ICY for Cast | `src/spectrum.rs`, `src/icy.rs` | |

## Hard invariants (do not break)

1. **UI never blocks** on HTTP, Cast LOAD, decode join, or cpal stream drop.
2. **`PlaybackController` generation:** stale workers must not `local.stop()` or apply UI success/title for an old generation.
3. **`CastService` is `Arc<CastService>`** with internal `op_lock` + `cancel` — do not reintroduce `Arc<Mutex<CastService>>` held across `play()`.
4. **Local session stop flags are per-play `Arc<AtomicBool>`** — never reset a shared flag to `false` to “reuse” an old hung decode.
5. **Error messages in `thiserror` / `Err(String)` are English.** UI copy stays in `i18n.rs`.
6. **Release exit:** `shutdown_playback` then `std::process::exit(0)` so orphaned HTTP threads cannot hang the process.
7. **Cast relay advertise IP** must be a real LAN NIC near the speaker — never a VPN/tunnel address.
8. **App-level blocking jobs use `BackgroundRuntime`;** do not add fire-and-forget threads to `app.rs`.

## Known footguns (from production bugs)

| Bug pattern | Fix already in tree |
|-------------|---------------------|
| Dead Cast station → LOAD hangs → all later Play/Stop stuck | Cancel + LOAD timeout + no outer mutex |
| Stale local play worker calls `local.stop()` → kills new station | Generation check without stop |
| Shared local `stop` AtomicBool cleared by next play → zombie decode lives | Per-session Arc |
| Dropping cpal/`join` on UI thread → freeze | Background drop in `LocalPlayer::stop` |
| reqwest `timeout(None)` + dead host → forever | Timed open + `StopAwareBody` |
| Cast Via PC silent while EQ moves | Spectrum used a 2nd station download; relay write was non-blocking (10035). Fixed: blocking + shared feeder; EQ taps relay URL |

## Logging

- File: `%LOCALAPPDATA%\RockCast\rockcast.log` on Windows, `~/.config/rockcast/rockcast.log` on Linux (truncated each run)
- Default filter: `rockcast=debug,info`
- Useful prefixes in logs: `play request`, `play worker[N]`, `LocalPlayer::`, `CastService::`, `PlayOk`, `Error applied`, `shutdown_playback`

When debugging user reports, ask for that log file first.

## How to run

```bat
run.bat
```

```bash
./run.sh
cargo run --release
cargo test --lib
cargo run --example cast_probe
```

## Suggested reading order for a new agent task

1. This file  
2. [modules.md](modules.md) — jump to the right file  
3. [playback.md](playback.md) or [cast.md](cast.md) if touching audio/Cast  
4. Open the concrete `.rs` file; prefer small, targeted diffs  
5. `cargo check` / `cargo test --lib` before finishing  

## Out of scope unless asked

- Official Google Cast SDK
- Regenerating protobuf from `.proto` via `prost`/`protobuf-codegen`
- Rewriting UI into another framework
