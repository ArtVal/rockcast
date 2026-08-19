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
        │ bounded runtime      │     │ LocalPlayer         │
        │ cast.play / stop     │     │  decode thread      │
        │ local.play           │     │  cpal callback      │
        └───────────┬──────────┘     │  HTTP body reader   │
                    │                └─────────────────────┘
        ┌───────────▼──────────┐
        │ CastService          │
        │  op_lock (serialize) │
        │  heartbeat thread    │
        │  TLS read/write      │
        └──────────────────────┘
```

| Thread / context | Responsibilities | Must not |
|------------------|------------------|----------|
| UI (`eframe`) | Draw, handle clicks, adapt controller events | Block on HTTP, Cast handshake, `cpal` stream drop |
| `PlaybackController` | Own generation/state and submit Cast/local/relay operations | Depend on egui |
| `BackgroundRuntime` | Execute a bounded number of blocking app jobs, propagate shutdown | Create a thread per UI action |
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
| `StreamObservers` | owns ICY/spectrum | Starts/stops taps outside the view layer |
| `ui_tx` / `ui_rx` | `mpsc` | Only UI polls `ui_rx` |
| Settings | file + dirty flag | Debounced persist |

## Control flow: Play

```text
User Play / double-click station
  → app.play()
      bump play_generation → G
      spawn worker(G)
         if device Cast:
            local.stop()
            cast.play(...)          # may wait ≤15s for LOAD; cancellable
         if device Local:
            cast.stop()             # cancels in-flight Cast LOAD
            local.play(...)         # probe ≤12s, then cpal
         if generation still G:
            UiMsg::PlayOk | Error
  → UI: PlayOk sets playing=true; may schedule Cast stream tap
```

## Control flow: Stop / exit

```text
Stop → bump generation, spawn: local.stop(); cast.stop(); UiMsg::StopOk
Exit → shutdown_playback (local.stop + timed cast.stop) → process::exit(0)
```

`process::exit` is intentional: hung HTTP reader threads must not keep the process alive after the window closes.

## Cancellation rules (critical)

1. **Never** let a stale play worker call `local.stop()` / tear down a newer session — check `play_generation` and return.
2. Cast in-flight `receive_find` watches `CastService.cancel`; new `play`/`stop` sets it before taking `op_lock`.
3. Local sessions use a **fresh** `Arc<AtomicBool>` per `play`; old sessions keep their own flag set to `true` and are not revived by the next play.

## Volume

- UI stores `0..=100`.
- Local: linear `ui/100`.
- Cast: `ui/100 * 0.5` (`VOLUME_CAST_SCALE`) so UI 100% is a comfortable speaker level.

## Spectrum / now playing

| Mode | Titles | Spectrum |
|------|--------|----------|
| Local | ICY inside `LocalPlayer` decode (`title_tx`) | FFT in decode thread → `LocalPlayer::levels` |
| Cast | `observers::IcyWatcher` HTTP tap after PlayOk | `observers::SpectrumAnalyzer` separate HTTP tap |

Both taps are stopped on station change / stop / error.
