# Playback

## Choosing an output

`output::scan_all(timeout, lang)` builds the device list:

1. All local cpal outputs (`list_local_devices`) — default marked with `★`
2. Cast receivers from `CastService::scan` / `discover`

UI stores `selected_device: Option<usize>` into that vec. `OutputDevice::is_local()` selects the play path.

## Generation token

Every `play`, `stop`, and `shutdown_playback` does roughly:

```text
generation = play_generation.fetch_add(1) + 1
```

Workers capture `generation` at spawn time. Before applying side effects or sending `PlayOk`/`Error`, they compare to the current atomic. Stale workers **return without** calling `local.stop()`.

## Local path (`LocalPlayer`)

### API

- `play(device, url, volume, title_tx, on_status) -> Result<(), LocalError>`
- `stop()` — sets current session stop flag; drops cpal stream + joins decode **off** the caller thread
- `set_volume` / `levels` — lock-free-ish atomics / mutex for UI EQ

### Pipeline

```text
HTTP GET (Icy-MetaData: 1)
  → open_stream_response (side thread + OPEN_TIMEOUT)
  → StopAwareBody (chunk reader, polls stop every READ_POLL)
  → IcyStripReader (strip icy-metaint, parse StreamTitle → title_tx)
  → symphonia probe/decode → interleaved f32
  → ring buffer (back-pressure if full)
  → parallel mono FFT → levels[BANDS]
cpal output callback:
  → linear resample ring → device rate/channels × volume
```

### Session stop

Each `play` installs a **new** `Arc<AtomicBool>` into `session_stop`. Prior sessions keep the old Arc (already `true`) so a hung decode cannot be accidentally cleared by the next play.

`play_lock` ensures only one `play` setup runs at a time; `stop()` is called before waiting on the lock so the previous session is cancelled first.

### Failure modes

| Symptom | Typical cause |
|---------|----------------|
| `failed to open audio stream` / probe timeout | Dead URL, TLS hang, no audio track |
| `stopped` | Cancelled by newer play/stop |
| `device … not found` | cpal device list changed |
| UI plays then silence | Ring underrun / bad stream — check log |

## Cast path (`CastService`)

### API

- `play(device, url, content_type, title, on_status)`
- `stop()`
- `set_volume_current(level 0..=1)`

Internally:

1. `cancel = true` then acquire `op_lock` (aborts previous LOAD wait)
2. `cancel = false`
3. Reuse or open `CastChannel` (`take_or_connect`)
4. CONNECT → ensure Default Media Receiver → LOAD (try station content-type then `audio/mpeg`)
5. Start heartbeat; store `LiveSession` in `current`

`receive_find` uses a **wall-clock** overall timeout (LOAD ~15s) and checks `cancel` every ~500ms read.

### After PlayOk (UI)

- Cast: schedule delayed `IcyWatcher` + optional `SpectrumAnalyzer` tap on the **original** station URL (not the LAN relay URL)
- Local: titles/levels already from `LocalPlayer` — no extra tap

## Cast relay (Via PC)

If `cast_relay` is on and the device is Cast:

1. `StreamRelay::start(station_url, cast_host, …)` → `http://<lan-ip>:<port>/stream`
2. `CastService::play` LOADs that LAN URL
3. Stop / supersede / error calls `relay.stop()`

Direct Cast (relay off) LOADs `station.url` as before.

## Stop and error handling

| Event | UI state | Engines |
|-------|----------|---------|
| `StopOk` | `playing=false` | already stopped in worker |
| `Error` (matching generation) | clear playing, status=message | `local.stop()` again; Cast session may already be empty |
| Stale `Error`/`PlayOk` | ignored | logged |

## Invariants checklist

- [ ] UI thread does not join hung HTTP/decode/cpal drop
- [ ] Superseded play worker does not stop the newer player
- [ ] Cast `play` does not hold an app-level mutex for the whole handshake
- [ ] Local HTTP open and Cast LOAD are time-bounded and cancellable
- [ ] Exit calls `shutdown_playback` then `process::exit(0)`
