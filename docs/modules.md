# Module map

## Layout

```text
src/
  main.rs          Binary: rustls provider, file logging, eframe
  lib.rs           Crate root — re-exports modules
  app.rs           egui app, UiMsg, play generation, wiring
  output.rs        OutputDevice = Local | Cast; scan_all()
  local.rs         LocalPlayer — HTTP decode + cpal
  stations.rs      stations.txt + Radio Browser enrich
  settings.rs      %LOCALAPPDATA%\RockCast\settings.json (+ log_path)
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
main → app → output → local
                 └→ cast::{discovery, client}
       app → stations, settings, i18n, icy, spectrum, relay
       cast::client → channel → proto
       local / relay → (reqwest, …)
       spectrum / icy → (reqwest, symphonia)   # Cast taps only
```

Do not introduce `app` imports into `cast` or `local`. Keep protocol code free of egui.

## Key types

| Type | Module | Role |
|------|--------|------|
| `RockCastApp` | `app` | UI state machine |
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
| `VOLUME_CAST_SCALE` | `app` | `0.5` — Cast volume scale |
| `OPEN_TIMEOUT` | `local` | HTTP open / probe wait (~12s) |
| `RING_MAX` | `local` | PCM ring capacity |
| `BANDS` / `FFT_SIZE` / `HOP` | `spectrum` | Visualizer FFT |
| Default Media Receiver | `cast::channel` | App id `CC1AD845` |
| Cast ports | discovery | TCP **8009** (CASTV2), HTTP **8008** (`eureka_info`) |

## External crates (playback / Cast)

| Crate | Use |
|-------|-----|
| `eframe` / `egui` | GUI |
| `cpal` | WASAPI output |
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
| `%LOCALAPPDATA%\RockCast\settings.json` | User prefs |
| `%LOCALAPPDATA%\RockCast\rockcast.log` | Session log |

## Tests and examples

- `cargo test --lib` — unit tests (discovery filters, proto roundtrip, …)
- `cargo run --example cast_probe` — live LAN discovery (~8s)
