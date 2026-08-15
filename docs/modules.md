# Module map

## Layout

```text
src/
  main.rs          Binary: rustls provider, file logging, eframe
  lib.rs           Crate root — re-exports modules
  app.rs           egui view state and UI event adaptation
  playback.rs      PlaybackController, state machine, generation, Cast/local/relay orchestration
  runtime.rs       Bounded blocking-job runtime and shared shutdown token
  observers.rs     ICY/spectrum lifecycle and delayed stream tap
  net.rs           Shared HTTP stream policy and ICY header helpers
  output.rs        OutputDevice = Local | Cast; scan_all()
  local.rs         LocalPlayer — HTTP decode + cpal
  stations.rs      stations.txt + Radio Browser enrich
  settings.rs      app dir settings.json + log_path (LOCALAPPDATA or ~/.config/rockcast)
  i18n.rs          Lang::Ru | En string tables
  icy.rs           IcyWatcher — StreamTitle for Cast mode
  spectrum.rs      SpectrumAnalyzer — FFT tap for Cast (+ BANDS/FFT consts)
  relay.rs         StreamRelay — LAN HTTP proxy (PC→Cast, VPN-friendly)
  cast/
    mod.rs         Re-exports CastService, CastDeviceInfo
    discovery.rs   mDNS + /24 TCP:8009 + eureka_info
    client.rs      High-level play / stop / volume / heartbeat
    channel.rs     TLS socket, length-prefixed CastMessage I/O
    proto.rs       Hand-rolled protobuf encode/decode
```

## Dependency direction (simplified)

```text
main → app → playback → output → local
                 └→ cast::{discovery, client}
       app → stations, settings, i18n, observers
       playback → runtime, relay
       observers → icy, spectrum
       cast::client → channel → proto
       local / relay → (reqwest, …)
       spectrum / icy → (reqwest, symphonia)   # Cast taps only
```

Do not introduce `app` imports into `cast` or `local`. Keep protocol code free of egui.

## Key types

| Type | Module | Role |
|------|--------|------|
| `RockCastApp` | `app` | egui view state and event adapter |
| `PlaybackController` | `playback` | Playback state machine and orchestration |
| `BackgroundRuntime` | `runtime` | Bounded app-level blocking jobs |
| `StreamObservers` | `observers` | ICY/spectrum lifecycle |
| `UiMsg` | `app` | Background → UI messages |
| `OutputDevice` | `output` | `Local(LocalDeviceInfo)` \| `Cast(CastDeviceInfo)` |
| `LocalPlayer` | `local` | PC playback engine |
| `LocalDeviceInfo` | `local` | cpal device id/name |
| `Station` | `stations` | name, url, tags, bitrate, codec… |
| `CastService` | `cast::client` | Session lifecycle |
| `CastDeviceInfo` | `cast::client` | Wrapper around `DiscoveredDevice` |
| `DiscoveredDevice` | `cast::discovery` | host, port, name, model, id |
| `CastChannel` | `cast::channel` | One TLS connection + inbox |
| `CastMessage` | `cast::proto` | CASTV2 message |
| `AppSettings` | `settings` | Persisted prefs |
| `StreamRelay` | `relay` | LAN HTTP relay for Cast |
| `IcyWatcher` | `icy` | Async title watcher |
| `SpectrumAnalyzer` | `spectrum` | Async FFT levels |

## Constants worth knowing

| Symbol | Where | Meaning |
|--------|-------|---------|
| `VOLUME_CAST_SCALE` | `playback` | `0.5` — Cast volume scale |
| `OPEN_TIMEOUT` | `local` | HTTP open / probe wait (~12s) |
| `RING_MAX` | `local` | PCM ring capacity |
| `BANDS` / `FFT_SIZE` / `HOP` | `spectrum` | Visualizer FFT |
| Default Media Receiver | `cast::channel` | App id `CC1AD845` |
| Cast ports | discovery | TCP **8009** (CASTV2), HTTP **8008** (`eureka_info`) |

## External crates (playback / Cast)

| Crate | Use |
|-------|-----|
| `eframe` / `egui` | GUI |
| `cpal` | Audio output (WASAPI on Windows, ALSA on Linux) |
| `symphonia` | Stream decode |
| `reqwest` (blocking) | HTTP(S) radio + taps |
| `rustls` | Cast TLS + Radio Browser custom HTTPS |
| `mdns-sd` | Cast mDNS |
| `rustfft` | Spectrum |
| `parking_lot` | Mutexes in hot paths |
| `thiserror` | Error types (messages in English) |

## Config / data files

| Path | Purpose |
|------|---------|
| `stations.txt` | Bundded catalog (`name \| url \| tags \| …`) |
| `ROCKCAST_STATIONS` | Optional override path for catalog |
| `proto/cast_channel.proto` | Reference schema (runtime uses hand-rolled codec) |
| `%LOCALAPPDATA%\RockCast\settings.json` | User prefs (Windows) |
| `~/.config/rockcast/settings.json` | User prefs (Linux) |
| `%LOCALAPPDATA%\RockCast\rockcast.log` | Session log (Windows) |
| `~/.config/rockcast/rockcast.log` | Session log (Linux) |

## Tests and examples

- `cargo test --lib` — unit tests (discovery filters, proto roundtrip, …)
- `cargo run --example cast_probe` — live LAN discovery (~8s)
