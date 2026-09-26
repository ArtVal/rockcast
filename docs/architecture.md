# Architecture

## Crates and entry points

```text
rockcast (lib)          ← all domain + UI logic
  └── rockcast (bin)    ← src/main.rs: logging, eframe::run_native
examples/cast_probe     ← CLI Cast discovery only
```

Windows release builds set `#![windows_subsystem = "windows"]` (no console). Logging goes to `%LOCALAPPDATA%\RockCast\rockcast.log` on Windows and `~/.config/rockcast/rockcast.log` on Linux; debug also mirrors to stderr.

## Threading model

```text
┌──────────────────────────── UI thread (egui) ────────────────────────────┐
│  RockCastApp::update                                                     │
│    poll events · draw · submit commands to PlaybackController/runtime     │
└───────────────────┬───────────────────────────┬──────────────────────────┘
                    │ mpsc UiMsg                │ Arc clones
        ┌───────────▼──────────┐     ┌──────────▼──────────┐
        │ playback runtime     │     │ LocalPlayer         │
        │ cast/local/stop      │     │  decode thread      │
        │ 3 bounded workers    │     │  cpal callback      │
        └───────────┬──────────┘     │  HTTP body reader   │
                    │                └─────────────────────┘
        ┌───────────▼──────────┐
        │ CastService          │
        │  op_lock (serialize) │
        │  heartbeat thread    │
        │  TLS read/write      │
        └──────────────────────┘

        rockcast-io-* (4 bounded workers): catalog, icons, account, voice, discovery
        rockcast-settings (latest-value slot): atomic settings persistence
```

| Thread / context | Responsibilities | Must not |
|------------------|------------------|----------|
| UI (`eframe`) | Draw, handle clicks, adapt controller events | Block on HTTP, Cast handshake, `cpal` stream drop |
| `PlaybackController` | Own generation/state and submit Cast/local/relay operations | Depend on egui |
| Playback runtime | Execute only playback transitions and volume | Run catalog, icon, or account work |
| I/O runtime | Execute catalog, icon, account, voice, and discovery jobs | Run playback transitions |
| `station_icons` jobs | Fetch/decode bounded station icons and write the local cache | Perform HTTP, decode images, or touch egui from the UI thread |
| Play worker | Call `CastService::play` or `LocalPlayer::play`, send `PlaybackEvent` | Call `local.stop()` after being superseded |
| Stop / shutdown worker | `local.stop()`, `cast.stop()` | Hold UI |
| Local decode | HTTP + symphonia → ring + FFT levels | Touch egui |
| cpal callback | Read ring, resample, apply volume | Lock UI or do I/O |
| Cast heartbeat | PING/PONG on live session | Steal exclusive op without `op_lock` |
| Volume worker | Drain `vol_tx`, apply local or cast volume | |

## Ownership and shared state

| Object | Type | Notes |
|--------|------|-------|
| `PlaybackController` | owns Cast/local/relay | No playback services are owned by egui state |
| `play_generation` | `Arc<AtomicU64>` inside controller | Bumped on every play/stop/shutdown; workers ignore stale gens |
| operation cancel | per-generation `Arc<AtomicBool>` | Changes only from active to cancelled |
| transition lock | `Arc<Mutex<()>>` | Linearizes cross-output playback side effects |
| `StreamObservers` | owns ICY/spectrum | Starts/stops taps outside the view layer |
| `ui_tx` / `ui_rx` | `mpsc` | Only UI polls `ui_rx` |
| Settings | file + dirty flag | Debounced persist |

## Control flow: Play

```text
User Play / double-click station
  → app.play()
      bump play_generation → G
      cancel previous token; create token(G); spawn playback worker(G)
         acquire transition lock; re-check token(G)
         if device Cast:
            local.stop()
            cast.play(...)          # may wait ≤15s for LOAD; cancellable
         if device Local:
            cast.stop()             # clears an established Cast session
            local.play(...)         # probe ≤12s, then cpal
         if generation still G:
            UiMsg::PlayOk | Error
  → UI: PlayOk sets playing=true; may schedule Cast stream tap
```

## Control flow: Stop / exit

```text
Stop → bump generation, cancel active token, spawn: local.stop(); cast.stop(); UiMsg::StopOk
Exit → cancel token + non-blocking local/relay shutdown → process::exit(0)
```

`process::exit` is intentional: hung HTTP reader threads must not keep the process alive after the window closes.

## Cancellation rules (critical)

1. A stale play worker must not tear down or install a session. Check generation and the operation token after acquiring the transition lock.
2. Cast `receive_find`, LocalPlayer, and relay pre-buffer waits observe the same per-generation cancellation token.
3. Cancellation tokens are never reset. A new Play creates a fresh `Arc<AtomicBool>`.

## Volume

- UI stores `0..=100`.
- Local: linear `ui/100`.
- Cast: `ui/100 * 0.5` (`VOLUME_CAST_SCALE`) so UI 100% is a comfortable speaker level.

## Spectrum / now playing

| Mode | Titles | Spectrum |
|------|--------|----------|
| Local | ICY inside `LocalPlayer` decode (`title_tx`) | FFT in decode thread → `LocalPlayer::levels` |
| Cast | `observers::IcyWatcher` HTTP tap after PlayOk | `observers::SpectrumAnalyzer` separate HTTP tap |

Both taps are stopped on station change / stop / error. The app forwards observed ICY/relay titles into its optional device-control `track_title` runtime field and publishes the changed state. Station ID remains the playback identity; a title is never inferred from the station name.

## Station icon flow (MVP)

```text
Station metadata
  → explicit favicon_url, or homepage_url + /favicon.ico
  → bounded BackgroundRuntime job
  → HTTP(S) response limit + image dimension limit
  → versioned app-data cache
  → UiMsg::StationIcon (decoded RGBA)
  → egui TextureHandle in station list
```

The UI keeps request identities for the session, so a missing or failed icon
does not trigger a new download on every redraw. The future RockServer-hosted
icon endpoint will replace the direct source without changing the UI contract.
