# RockCast documentation

Code-oriented docs for humans and coding agents. Product overview and user instructions stay in the root [`README.md`](../README.md).

| Doc | Purpose |
|-----|---------|
| [architecture.md](architecture.md) | Threads, ownership, end-to-end data flow |
| [modules.md](modules.md) | File/module map, key types, dependencies |
| [playback.md](playback.md) | Local + Cast play/stop, cancellation, volume |
| [cast.md](cast.md) | Discovery, CASTV2 stack, protocol notes |
| [agents.md](agents.md) | **Start here if you are an LLM/agent** — invariants, gotchas, where to edit |

## Quick facts

- **Language:** Rust (edition 2024), GUI via `eframe` / `egui`
- **Platform:** Windows first (WASAPI via `cpal`, `windows_subsystem = "windows"` in release)
- **Outputs:** PC speakers (`LocalPlayer`) or Google Cast (`CastService`)
- **Library crate:** `src/lib.rs` — binary entry is `src/main.rs`
- **Log file:** `%LOCALAPPDATA%\RockCast\rockcast.log` (truncated each launch)
- **Settings:** `%LOCALAPPDATA%\RockCast\settings.json`

## Mental model (one paragraph)

The UI thread (`RockCastApp`) never blocks on network or decode. Play/stop/scan run on background threads and report via `mpsc` (`UiMsg`). A monotonic `play_generation` invalidates stale workers. Local audio is HTTP → ICY strip → symphonia → ring → cpal (+ FFT). Cast is TLS CASTV2: discover → connect → LAUNCH Default Media Receiver → `LOAD` stream URL (optionally a LAN URL from `StreamRelay` when **Via PC** is on). Titles/spectrum for Cast use a separate HTTP tap (`IcyWatcher` / `SpectrumAnalyzer`) on the original station URL.
