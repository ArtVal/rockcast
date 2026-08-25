# RockCast

Desktop internet radio player for Windows and Linux. Play rock / metal streams on **PC speakers** or on **Google Cast** devices (Chromecast, Cast-enabled speakers such as JBL) using a native CASTV2 client — no Chrome browser required.

![RockCast](window_shot.png)

## Features

- **Local playback** — decode and play HTTP(S) radio streams on your PC (cpal + symphonia)
- **Google Cast** — discover receivers and stream live radio via CASTV2 (TLS + protobuf)
- **Via PC relay** — optional: PC fetches the station (e.g. through VPN) and serves it to the speaker on LAN
- **VPN-friendly discovery** — mDNS plus a LAN `/24` TCP scan with Cast `eureka_info` (works when Amnezia / WireGuard breaks multicast)
- **Station catalog** — pinned schema-v1 JSON snapshot plus optional enrichment from [Radio Browser](https://www.radio-browser.info/)
- **Now playing** — ICY / Shoutcast `StreamTitle` when the station provides metadata
- **Spectrum visualizer** — optional FFT bars (uses the same local decode path or a stream tap for Cast)
- **Bilingual UI** — Russian and English
- **Persistent settings** — volume, last station, device, language, spectrum toggle
- **Optional RockServer mode** — server-side semantic station search and a bounded SpeechKit voice command; autonomous mode remains the default

## Requirements

- Windows 10 / 11, or Linux (X11 or Wayland; audio via ALSA / PipeWire)
- [Rust](https://rustup.rs/) toolchain (edition 2024 / recent stable)
- Same Wi‑Fi / LAN as your Cast device for casting
- Optional: Amnezia or other VPN — Cast discovery still works via subnet scan if LAN unicast is allowed

### Linux packages (Fedora / Kinoite distrobox)

Build the GUI and cpal against system libraries:

```bash
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config \
  alsa-lib-devel libxkbcommon-devel wayland-devel \
  libX11-devel libXcursor-devel libXrandr-devel libXi-devel mesa-libGL-devel
```

On Fedora Kinoite keep the toolchain in rustup (`~/.cargo`) inside toolbox/distrobox — do not layer `rust`/`cargo` with `rpm-ostree`.

Debian/Ubuntu equivalents: `build-essential pkg-config libasound2-dev libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev`.

## Quick start

Windows:

```bat
run.bat
```

Linux:

```bash
./run.sh
```

Or manually:

```bash
cargo run --release
```

Debug build:

```bash
cargo run
```

Logging (default level `info`):

```bash
# Windows
set RUST_LOG=info
cargo run --release

# Linux / macOS
RUST_LOG=info cargo run --release
```

Useful levels: `rockcast=debug`, `rockcast::cast::discovery=debug`.

## Using the app

Detailed Russian-language instructions, including voice commands: **[docs/user-manual.html](docs/user-manual.html)**. Open the file in a browser.

1. **Wait for stations** — the local catalog loads immediately; Radio Browser enrichment may follow in the background.
2. **Find devices** — click **Find** to scan PC audio outputs and Cast receivers on the LAN.
3. **Select output** — choose **This PC** (speakers) or a Cast device (e.g. a JBL speaker).
4. **Via PC** (Cast only) — enable if the station needs VPN on the PC; RockCast relays audio to the speaker over Wi‑Fi.
5. **Select a station** — click a row in the list.
6. **Play / Stop** — start or stop playback.
7. **Volume** — slider; Cast volume is scaled so the UI “100%” maps to a comfortable speaker level.
8. **Spectrum** — enable for the equalizer-style visualizer (extra network use when casting).
9. **Language** — switch Russian / English from the UI; the choice is saved.

### Cast notes

- Discovery runs **mDNS** (`_googlecast._tcp`) on the real LAN NIC and, in parallel, a **unicast scan** of `x.x.x.1–254` for TCP **8009**, then reads `http://<ip>:8008/setup/eureka_info`.
- Split-tunnel VPN exclusions often fix unicast but **not** multicast; the subnet scan is what finds devices like JBL when mDNS returns nothing.
- Playback on Cast uses CASTV2: connect → launch Default Media Receiver → `LOAD` the live stream URL.
- With **Via PC**, `LOAD` points at `http://<PC-LAN-IP>:<port>/stream` while the PC downloads the station (through VPN if needed). Allow inbound LAN (Windows Firewall / firewalld) if prompted.

### Local playback notes

- Uses the selected output device (or the system default): WASAPI on Windows, ALSA (PipeWire/Pulse) on Linux.
- Supports common stream codecs handled by symphonia (MP3, AAC, Ogg/Vorbis, FLAC, etc., depending on the station).
- Track titles come from ICY metadata when available.

## Station catalog

### Bundled schema-v1 release

RockCast ships the approved offline snapshot in
`assets/catalog/stations.v1.json`: catalogVersion **2026.08.2**,
SHA-256 **3fa20dca94fc059bd433a47b9fba9bb6d5e5e1aa2957a5ffb58b2a7b20b1d74d**.
Its embedded manifest, version, and canonical (UTF-8/LF) checksum are verified before parsing.
The selected playback URL is each station's exactly-one primary stream; additional streams remain
available in metadata. No catalog download occurs at build or startup.

### Overrides

Use the same schema-v1 JSON document for custom overrides. Overrides are explicitly user-owned,
un-pinned full-catalog authority: before replacing one, keep a local backup; remove or rename it to
return immediately to the checksum-verified bundled baseline. Search precedence remains:

1. `ROCKCAST_STATIONS` environment variable (full path; JSON or legacy TXT)
2. `stations.v1.json`, then `stations.txt`, next to the executable
3. `stations.v1.json`, then `stations.txt`, in the current working directory
4. `stations.v1.json`, then `stations.txt`, in app data

If no override exists, RockCast creates an editable `stations.v1.json` app-data copy from the
vendored snapshot. JSON overrides accept forward-compatible unknown optional fields but require
schemaVersion 1, unique stable IDs, valid HTTP(S) streams, and exactly one primary stream.

### Legacy TXT transition

```text
# name | url | tags | bitrate | codec | country
SomaFM — Metal Detector | https://ice6.somafm.com/metal-128-mp3 | metal,heavy metal | 128 | mp3 | USA
```

- Lines starting with `#` are comments.
- At least `name` and `url` are required (`http://` or `https://`).
- Playlist URLs (`.m3u`, `.pls`, …) from Radio Browser are skipped.

Existing `stations.txt` overrides retain their current behavior for one release cycle. This is a
documented legacy exception owned by the RockCast maintainers to protect offline user overrides;
it has a removal date of **2026-10-31** and must not be extended silently. App-data locations are Windows
`%LOCALAPPDATA%\RockCast` and Linux `$XDG_CONFIG_HOME/rockcast` (or `~/.config/rockcast`).

### Radio Browser

After the local list appears, RockCast may query Radio Browser for additional metal/rock stations and merge them (deduped by URL, capped for UI size).

## Settings

Stored at:

```text
Windows: %LOCALAPPDATA%\RockCast\settings.json
Linux:   ~/.config/rockcast/settings.json
```

Typical fields: `volume`, `station_url`, `last_played_station`, `device_id`, `eq_enabled`, `cast_relay`, `language`, and the optional RockServer URL/token settings.

## RockServer and voice control

RockCast starts in autonomous mode and continues to use its local catalog plus Radio Browser exactly as before. Enable **RockServer (search and voice)** in the window only when a local or LAN RockServer is running; the default URL is `http://127.0.0.1:3000` and is saved locally. Enter the `ROCKSERVER_API_BEARER_TOKEN` in the masked **Токен** field. RockCast sends it as `Authorization: Bearer <token>` for both HTTP search and the voice WebSocket handshake; it never appends the credential to the server URL or writes it to the application log. The query box uses `/v1/search`. If the server is unavailable or returns an error, RockCast falls back to the autonomous catalog.

The **Voice** button records PCM16 mono from the default Windows microphone until release or the 60-second limit, sends it to `/api/v1/voice/stream`, and plays the station selected by the server when the command requests playback. In RockServer settings, choose **Buffered (after recording)** for the compatible SpeechKit v1 request, or **Streaming (while recording)** for SpeechKit v3 partial recognition. In streaming mode RockCast opens the WebSocket before microphone capture and sends PCM chunks as they arrive; buffered mode retains the original upload-after-release behavior. The choice is saved locally and sent with each new voice session; RockServer must be configured with the local Yandex SpeechKit credentials. RockCast never stores or sends SpeechKit credentials. Input-device selection/testing and cancellation after upload begins are not implemented yet.

### Voice commands

RockServer recognizes station requests in Russian and English. RockCast additionally recognizes these short control commands locally after receiving the transcript:

| Russian | English | Action |
| --- | --- | --- |
| `Стоп`, `Останови`, `Выключи` | `Stop`, `Pause`, `Turn off` | Stop playback. |
| `Дальше`, `Следующая`, `Вперёд` | `Next`, `Next station`, `Skip` | Select and play the next station in the current list. |
| `Назад`, `Предыдущая` | `Previous`, `Previous station`, `Back` | Select and play the previous station in the current list. |
| `Включи музыку` | `Play music`, `Play some music` | Play the last station that started successfully. The station is remembered across restarts. |

At the beginning or end of a list, **Previous** or **Next** does not wrap around. If no station has played successfully yet, **Play music** leaves playback unchanged.

## Project layout

```text
rockcast/
├── Cargo.toml
├── assets/catalog/       # checksum-pinned schema-v1 baseline release
├── run.bat               # Windows release launcher
├── run.sh                # Linux release launcher
├── docs/                 # architecture & agent-oriented code docs
├── proto/                # Cast channel protobuf reference
├── examples/
│   └── cast_probe.rs     # CLI discovery probe
└── src/
    ├── main.rs           # GUI entry
    ├── lib.rs
    ├── app/              # egui UI (theme, actions, ui panels)
    ├── playback/         # PlaybackController + phase/volume
    ├── local/            # PC playback (cpal + decode)
    ├── observers/        # ICY + spectrum taps for Cast
    ├── stations/         # catalog + Radio Browser
    ├── relay/            # LAN HTTP relay PC → Cast
    ├── voice/            # RockServer voice search
    ├── i18n.rs           # EN / RU strings
    ├── output.rs         # local + Cast device list
    ├── settings.rs
    └── cast/
        ├── discovery/    # mDNS + subnet scan
        ├── client/       # CASTV2 play / stop / volume
        ├── channel/      # TLS framing + auth
        │   ├── recv.rs   # read loop / inbox
        │   └── tls.rs    # connect
        └── proto.rs      # hand-rolled CastMessage codec
```

Code architecture, module map, playback/Cast flows, and notes for AI coding agents: **[docs/README.md](docs/README.md)**.

## Development

### Build

```bash
cargo build --release
```

Binary: `target/release/rockcast.exe` (Windows) or `target/release/rockcast` (Linux).

### Tests

```bash
cargo test --lib
```

Discovery tests include VPN interface name filters and a live LAN probe (skipped automatically if `192.168.31.109:8009` is not open on your network — adjust or ignore as needed).

### Cast discovery probe

```bash
cargo run --example cast_probe
```

Prints every Cast receiver found via mDNS and/or subnet scan (about 8 seconds).

### Architecture (short)

| Path | Role |
|------|------|
| UI thread (egui) | Renders UI; starts background work for scan / play / stop |
| `LocalPlayer` | HTTP stream → decode → ring buffer → cpal output + optional FFT |
| `CastService` | CASTV2 session, heartbeat, volume, `LOAD` / `STOP` |
| `StreamRelay` | Optional LAN HTTP proxy: PC fetches station, Cast LOADs local URL |
| Discovery | LAN NIC whitelist (skip Amnezia/Hyper-V/…) + mDNS + `/24` TCP probe |

## Troubleshooting

| Symptom | What to try |
|---------|-------------|
| No Cast devices | Click **Find** again; ensure PC and speaker are on the same LAN; allow LAN in VPN; run `cargo run --example cast_probe` |
| Cast found, play fails | Confirm the station URL plays on PC first; check firewall for outbound HTTPS to the stream and TCP 8009 to the device |
| Station needs VPN, silent on JBL | Enable **Via PC**; allow inbound LAN (Windows Firewall / firewalld); PC and JBL on same Wi‑Fi |
| Empty station list | Check `stations.txt` path / `ROCKCAST_STATIONS`; inspect logs for Radio Browser errors |
| No track title | Many stations do not send ICY metadata |
| Wrong PC audio device | Pick another entry under **Device** after **Find** |
| Spectrum silent on Cast | Enable spectrum; Cast mode uses a separate stream tap after the receiver starts |

## License

RockCast is licensed under the [GNU General Public License, version 3 or later (GPL-3.0-or-later)](https://www.gnu.org/licenses/gpl-3.0.html).

## Acknowledgments

- [Radio Browser](https://www.radio-browser.info/) — community station directory
- [SomaFM](https://somafm.com/), Rock Antenne, and other listed stations — streams referenced in the default catalog
- Google Cast / CASTV2 protocol community documentation
