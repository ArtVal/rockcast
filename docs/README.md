# RockCast documentation

Code-oriented docs for humans and coding agents. Product overview and user instructions stay in the root [`README.md`](../README.md).

| Doc | Purpose |
|-----|---------|
| [architecture.md](architecture.md) | Threads, ownership, end-to-end data flow |
| [modules.md](modules.md) | File/module map, key types, dependencies |
| [playback.md](playback.md) | Local + Cast play/stop, cancellation, volume, remote track-state publication |
| [concurrency.md](concurrency.md) | Thread pools, cancellation, Play/Stop freeze postmortem |
| [cast.md](cast.md) | Discovery, CASTV2 stack, protocol notes |
| [agents.md](agents.md) | **Start here if you are an LLM/agent** — invariants, gotchas, where to edit |

## Quick facts

- **Language:** Rust (edition 2024), GUI via `eframe` / `egui`
- **Platform:** Windows (WASAPI) and Linux (ALSA / PipeWire via `cpal`). Release Windows builds set `windows_subsystem = "windows"`.
- **Outputs:** PC speakers (`LocalPlayer`) or Google Cast (`CastService`)
- **Library crate:** `src/lib.rs` — binary entry is `src/main.rs`
- **Log / settings:** `%LOCALAPPDATA%\RockCast\` on Windows; `~/.config/rockcast/` on Linux

## Mental model (one paragraph)

The UI thread (`RockCastApp`) never blocks on network, decode, observer joins, or settings writes. Playback and general I/O use separate bounded executors and report through `mpsc`. A monotonic generation plus a per-operation cancellation token invalidates stale workers. Every selected station opens the authenticated RockServer `/api/v1/stations/{id}/stream` route. Local audio is RockServer HTTP → ICY strip → decode → ring → cpal (+ FFT). Cast always uses RockServer → PC `StreamRelay` → LAN stream URL → one selected receiver; CASTV2 controls discovery, launch and LOAD. The receiver receives no RockServer bearer token and never receives the original station URL. The relay allows two concurrent clients total and has no LAN-client authentication.
