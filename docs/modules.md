# Module map

## Layout

Modular layout mirrors `relay/`: each domain gets a directory with `mod.rs` plus focused submodules (no single “god file”).

```text
src/
  main.rs              Binary: rustls provider, file logging, eframe
  lib.rs               Crate root — re-exports modules
  app/                 egui view state and UI event adaptation
    mod.rs             RockCastApp, eframe::App, Drop
    theme.rs           Colors, panel chrome, column widths
    messages.rs        UiMsg (background → UI), same_output_device()
    actions/           Play/stop/scan/settings/voice handlers
      settings.rs      Persist prefs, language, EQ settings
      playback.rs      play/stop/shutdown, observer wiring
      poll.rs          PlaybackEvent + UiMsg dispatch
      poll/            Account/catalog/device/playback/voice event handlers
      catalog.rs       Station/device refresh, bootstrap
      icons.rs         Schedule direct station icon jobs
      voice.rs         RockServer mic capture
      eq.rs            Spectrum bar animation
    ui/                egui panel draw impls
      account.rs       Account and paired-device card
      devices.rs       Output device list
      stations.rs      Station catalog table
      stations/        Filters, search bar, rows, missing favourites
      controls.rs      Transport + volume + spectrum toggles
      eq.rs            EQ / spectrum visualizer
  playback/            PlaybackController, state machine, orchestration
    mod.rs             Controller API (play/stop/volume/shutdown)
    phase.rs           PlaybackPhase + PlaybackEvent
    volume.rs          Local/Cast volume scaling
  runtime.rs           Bounded blocking-job runtime and shared shutdown token
  observers/           ICY/spectrum lifecycle and stream tap
    mod.rs             StreamObservers
    icy.rs             IcyWatcher — StreamTitle for Cast mode
    spectrum.rs        SpectrumAnalyzer — FFT tap for Cast
  net.rs               Shared HTTP stream policy and ICY header helpers
  output.rs            OutputDevice = Local | Cast; scan_all()
  local/               LocalPlayer — HTTP decode + cpal
    mod.rs             Public LocalPlayer API
    device.rs          DeviceTrait, LocalDeviceInfo
    cpal_util.rs       cpal stream helpers
    error.rs           LocalError
  stations/            Station models, sorting and codec inference
    mod.rs             Station type definition
    catalog.rs         order_stations and infer_codec helpers
  station_icons.rs     RockServer WebP icon fetch, decode, and disk cache
  settings.rs          app dir settings.json + log_path (LOCALAPPDATA or ~/.config/rockcast)
  i18n.rs              Lang::Ru | En string tables
  relay/               StreamRelay — LAN HTTP proxy (RockServer→PC→Cast)
    error.rs           RelayError
    fanout.rs          Multi-client broadcast
    feeder.rs          Upstream fetch + transcode paths
    net.rs             Advertise LAN IPv4 near Cast device
    server.rs          HTTP listener + routing
    transport.rs       PCM tap / WAV / passthrough selection
    wav.rs             WAV header helpers
  audio/               Shared decode + format detection + spectrum math
    format.rs          Content-type / codec sniffing
    spectrum.rs        FFT analyzer (used by local decode path)
    decode/            symphonia decoders
      live/            Unified live HTTP decode (local + relay)
        open.rs        ICY stream open + format peek
        local.rs       f32 decode for LocalPlayer
        relay.rs       16-bit PCM transcode for Cast relay
        symphonia.rs   MP3/OGG fallback via symphonia
      adts.rs          AAC/ADTS framing helpers
      codecs.rs        Codec selection and stream helpers
      icy.rs           ICY metadata parsing and stream opening
      opus.rs          Opus stream decode
      pcm.rs           PCM helpers
      probe.rs         Stream format probing
  cast/
    mod.rs             Re-exports CastService, CastDeviceInfo
    discovery/         mDNS + /24 TCP:8009 + eureka_info
      mod.rs           discover(), DiscoveredDevice
      lan.rs           NIC scoring / VPN filter
      mdns.rs          _googlecast._tcp browse
      subnet.rs        /24 TCP probe + eureka_info
      eureka.rs        Parse setup/eureka_info JSON
    client/            High-level play / stop / volume / heartbeat
      mod.rs           CastService, CastDeviceInfo
      session.rs       Connect, LAUNCH, LOAD, heartbeat
      media.rs         Media status parsing, codec candidates
      error.rs         ClientError
    channel/           TLS socket, length-prefixed CastMessage I/O
      error.rs         ChannelError
      consts.rs        Namespace / app id constants
      tls.rs           TLS connect (no cert verify)
      auth.rs          Post-connect device auth
      recv.rs          receive_find, inbox, heartbeat pump
    proto.rs           Hand-rolled protobuf encode/decode
  voice/               RockServer voice search transport
    mod.rs             WebSocket session glue
    dto.rs             JSON message types
    record.rs          Microphone capture
    rank.rs            Station reranking
    resample.rs        Mono PCM16 sample-rate conversion
  device_control.rs    Device-control lifecycle, auth, and command orchestration
  device_control/      RockServer remote commands (private crate module)
    protocol.rs        Versioned wire DTOs and capability validation
    output.rs          Opaque, expiring Cast receiver handles
    transport.rs       WebSocket connection and bounded command delivery
    tests.rs           Wire and lifecycle regression tests
  personal_data/       Local favourites and playback history
  personal_sync/       RM-012-B server sync contract and merge engine
  session.rs           Account-session module entry point
  session/             Paired identity, token renewal, OS credential storage
  profile.rs           User profile helpers
  voice_prompts.rs     Embedded beep / prompt playback
  rockserver.rs        Public RockServer runtime config + HTTP search client
  playback_diag/       Playback diagnostics (`ROCKCAST_PROFILE=1`)
    mod.rs             maybe_log summary, enabled gate
    local.rs           cpal ring / underrun counters
    relay.rs           Fanout buffer / drop metrics
    http.rs            HTTP read gaps, decode throughput
    playout.rs         Playout-thread pending buffer
  profile.rs           User profile helpers
  telemetry.rs         UI telemetry snapshots
```

## Dependency direction (simplified)

```text
main → app → playback → output → local
                 └→ cast::{discovery, client}
       app → stations, settings, i18n, observers, voice
       playback → runtime, relay
       observers → icy, spectrum
       cast::client → channel → proto
       local / relay / observers → audio::decode, net
       local / relay → (reqwest, cpal, …)
       spectrum / icy → (reqwest, symphonia)   # Cast taps only
```

Do not introduce `app` imports into `cast` or `local`. Keep protocol code free of egui.

## Key types

| Type | Module | Role |
|------|--------|------|
| `RockCastApp` | `app` | egui view state and event adapter |
| `UiMsg` | `app::messages` | Background → UI messages |
| `PlaybackController` | `playback` | Playback state machine and orchestration |
| `PlaybackPhase` | `playback::phase` | Idle / Opening / Playing / Stopping / Failed (drives device-control statuses) |
| `BackgroundRuntime` | `runtime` | Bounded app-level blocking jobs |
| `StreamObservers` | `observers` | ICY/spectrum lifecycle |
| `OutputDevice` | `output` | `Local(LocalDeviceInfo)` \| `Cast(CastDeviceInfo)` |
| `LocalPlayer` | `local` | PC playback engine |
| `LocalDeviceInfo` | `local::device` | cpal device id/name |
| `Station` | `stations` | name, url, tags, bitrate, codec… |
| `CastService` | `cast::client` | Session lifecycle |
| `CastDeviceInfo` | `cast::client` | Wrapper around `DiscoveredDevice` |
| `DiscoveredDevice` | `cast::discovery` | host, port, name, model, id |
| `CastChannel` | `cast::channel` | One TLS connection + inbox |
| `CastMessage` | `cast::proto` | CASTV2 message |
| `AppSettings` | `settings` | Persisted prefs |
| `StreamRelay` | `relay` | LAN HTTP relay for Cast |
| `IcyWatcher` | `observers::icy` | Async title watcher |
| `SpectrumAnalyzer` | `observers::spectrum` | Async FFT levels |

## Constants worth knowing

| Symbol | Where | Meaning |
|--------|-------|---------|
| `VOLUME_CAST_SCALE` | `playback::volume` | `0.5` — Cast volume scale |
| `OPEN_TIMEOUT` | `local` | HTTP open / probe wait (~12s) |
| `RING_MAX` | `local` | Local PCM ring capacity |
| `RING_MAX` | `relay::fanout` | Relay fanout ring capacity (distinct from the local PCM ring) |
| `BANDS` / `FFT_SIZE` / `HOP` | `audio::spectrum` | Visualizer FFT |
| Default Media Receiver | `cast::channel` | App id `CC1AD845` |
| Cast ports | discovery | TCP **8009** (CASTV2), HTTP **8008** (`eureka_info`) |

## External crates (playback / Cast)

| Crate | Use |
|-------|-----|
| `eframe` / `egui` | GUI |
| `cpal` | Audio output (WASAPI on Windows, ALSA on Linux) |
| `symphonia` | Stream decode |
| `reqwest` (blocking) | RockServer API/stream, station icons, and stream analysis |
| `rustls` | Cast TLS and HTTPS clients |
| `mdns-sd` | Cast mDNS |
| `rustfft` | Spectrum |
| `parking_lot` | Mutexes in hot paths |
| `thiserror` | Error types (messages in English) |

## Config / data files

| Path | Purpose |
|------|---------|
| `proto/cast_channel.proto` | Reference schema (runtime uses hand-rolled codec) |
| `%LOCALAPPDATA%\RockCast\settings.json` | User prefs (Windows) |
| `~/.config/rockcast/settings.json` | User prefs (Linux) |
| `%LOCALAPPDATA%\RockCast\rockcast.log` | Session log (Windows) |
| `~/.config/rockcast/rockcast.log` | Session log (Linux) |

## Tests and examples

- `cargo test --lib` — unit tests (discovery filters, proto roundtrip, relay transport, …)
- `cargo run --example cast_probe` — live LAN discovery (~8s)

## Back-compat re-exports

`lib.rs` re-exports `observers::spectrum::{BANDS, SpectrumAnalyzer}` as `rockcast::spectrum` for older tests/examples.
