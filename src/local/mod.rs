//! Local internet-radio playback to PC speakers (cpal + symphonia).

mod cpal_util;
mod device;
mod error;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use cpal::traits::{DeviceTrait, StreamTrait};
use parking_lot::Mutex;

use crate::{
    audio::{
        decode::{
            pcm::{PcmResampler, SpscAudioRing, SteadyPlayout, upmix_interleaved_into},
            run_live_decode_f32,
        },
        spectrum::BANDS,
        spectrum::SpectrumTap,
    },
    playback_diag,
};

pub use device::{LocalDeviceInfo, list_local_devices};
pub use error::LocalError;

use cpal_util::{
    LocalCleanup, SendStream, bits_f32, cleanup_sender, f32_bits, pick_cpal_device,
    pick_output_config,
};

const RING_MAX: usize = 48000 * 2 * 4; // ~4 sec stereo @ 48k
/// Give up if the station accepts TCP but never sends HTTP headers / audio.
const OPEN_TIMEOUT: Duration = Duration::from_secs(12);

pub struct LocalPlayer {
    /// Stop flag for the *current* session only. Each `play` gets a fresh Arc so
    /// cancelling an old hung decode cannot be undone by the next `play`.
    session_stop: Mutex<Arc<AtomicBool>>,
    /// Serialize play setup so two concurrent `play` calls cannot race on state.
    play_lock: Mutex<()>,
    state: Mutex<PlayerState>,
    volume: Arc<AtomicU32>,
    levels: Arc<Mutex<[f32; BANDS]>>,
}

struct PlayerState {
    decode_join: Option<thread::JoinHandle<()>>,
    playout_join: Option<thread::JoinHandle<()>>,
    stream: Option<SendStream>,
    pcm_tx: Option<mpsc::SyncSender<Vec<f32>>>,
}

impl Default for LocalPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalPlayer {
    pub fn new() -> Self {
        Self {
            session_stop: Mutex::new(Arc::new(AtomicBool::new(true))),
            play_lock: Mutex::new(()),
            state: Mutex::new(PlayerState {
                decode_join: None,
                playout_join: None,
                stream: None,
                pcm_tx: None,
            }),
            volume: Arc::new(AtomicU32::new(f32_bits(0.5))),
            levels: Arc::new(Mutex::new([0.08; BANDS])),
        }
    }

    pub fn levels(&self) -> [f32; BANDS] {
        *self.levels.lock()
    }

    pub fn set_volume(&self, level: f32) {
        self.volume
            .store(f32_bits(level.clamp(0.0, 1.0)), Ordering::SeqCst);
    }

    pub fn stop(&self) {
        log::info!("LocalPlayer::stop");
        self.session_stop.lock().store(true, Ordering::SeqCst);
        let (stream, decode_join, playout_join) = {
            let mut state = self.state.lock();
            state.pcm_tx.take();
            (
                state.stream.take(),
                state.decode_join.take(),
                state.playout_join.take(),
            )
        };
        log::debug!(
            "LocalPlayer::stop: had_stream={} had_decode={} had_playout={}",
            stream.is_some(),
            decode_join.is_some(),
            playout_join.is_some()
        );
        if stream.is_some() || decode_join.is_some() || playout_join.is_some() {
            let _ = cleanup_sender().send(LocalCleanup {
                stream,
                decode_join,
                playout_join,
            });
        }
        *self.levels.lock() = [0.08; BANDS];
    }

    pub fn play(
        &self,
        device: &LocalDeviceInfo,
        url: &str,
        volume: f32,
        spectrum_enabled: bool,
        title_tx: Option<mpsc::Sender<String>>,
        on_status: impl Fn(&str),
    ) -> Result<(), LocalError> {
        log::info!(
            "LocalPlayer::play begin device='{}' cpal={:?} vol={volume:.2} url={url}",
            device.name,
            device.cpal_name
        );
        self.stop();
        log::debug!("LocalPlayer::play: waiting play_lock");
        let _play_guard = self.play_lock.lock();
        log::debug!("LocalPlayer::play: play_lock acquired");

        self.set_volume(volume);
        on_status(&format!("Local: «{}»...", device.name));

        let stop = Arc::new(AtomicBool::new(false));
        *self.session_stop.lock() = Arc::clone(&stop);
        *self.levels.lock() = [0.08; BANDS];

        let ring = Arc::new(SpscAudioRing::with_capacity(RING_MAX));
        let src_rate = Arc::new(AtomicU32::new(0));
        let src_ch = Arc::new(AtomicU32::new(2));
        let err_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let (pcm_tx, pcm_rx) = mpsc::sync_channel(128);

        let url = url.to_string();
        let stop_dec = Arc::clone(&stop);
        let src_rate_dec = Arc::clone(&src_rate);
        let src_ch_dec = Arc::clone(&src_ch);
        let stop_push = Arc::clone(&stop_dec);
        let err_c = Arc::clone(&err_slot);

        {
            let mut state = self.state.lock();
            state.pcm_tx = Some(pcm_tx.clone());
            state.decode_join = Some(thread::spawn(move || {
                let _worker = crate::profile::worker("local_decode");
                log::info!("LocalPlayer decode thread started url={url}");
                let mut pcm_buf = Vec::with_capacity(16 * 1024);
                if let Err(e) = run_live_decode_f32(
                    &url,
                    &stop_dec,
                    title_tx,
                    None,
                    src_rate_dec,
                    src_ch_dec,
                    move |pcm| {
                        if stop_push.load(Ordering::SeqCst) {
                            return;
                        }
                        playback_diag::decode_pcm(pcm.len());
                        pcm_buf.clear();
                        pcm_buf.extend_from_slice(pcm);
                        if pcm_tx
                            .send(std::mem::replace(
                                &mut pcm_buf,
                                Vec::with_capacity(16 * 1024),
                            ))
                            .is_ok()
                        {
                            playback_diag::local_pcm_sent();
                        }
                    },
                ) {
                    if !stop_dec.load(Ordering::SeqCst) {
                        log::warn!("LocalPlayer decode ended with error: {e}");
                        *err_c.lock() = Some(e);
                    } else {
                        log::info!("LocalPlayer decode stopped ({e})");
                    }
                } else {
                    log::info!("LocalPlayer decode thread exited cleanly");
                }
            }));
        }

        // Wait for probe without holding the state mutex — stop() can interrupt.
        let deadline = Instant::now() + OPEN_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(e) = err_slot.lock().clone() {
                log::error!("LocalPlayer::play: decode error while waiting probe: {e}");
                self.stop();
                return Err(LocalError::Stream(e));
            }
            if src_rate.load(Ordering::SeqCst) > 0 {
                break;
            }
            if stop.load(Ordering::SeqCst) {
                log::info!("LocalPlayer::play: cancelled while waiting probe");
                return Err(LocalError::Stream("stopped".into()));
            }
            thread::sleep(Duration::from_millis(20));
        }
        let rate = src_rate.load(Ordering::SeqCst);
        if rate == 0 {
            log::error!("LocalPlayer::play: probe timeout ({OPEN_TIMEOUT:?})");
            self.stop();
            return Err(LocalError::Stream("failed to open audio stream".into()));
        }
        let channels = src_ch.load(Ordering::SeqCst).max(1) as usize;
        log::info!("LocalPlayer::play: probe ok rate={rate} ch={channels}");

        let cpal_device = pick_cpal_device(device)?;
        let config = pick_output_config(&cpal_device, rate, channels)?;
        let play_rate = config.sample_rate.0;
        let play_ch = config.channels as usize;
        ring.clear();
        log::info!(
            "LocalPlayer::play: cpal out_rate={play_rate} out_ch={play_ch} (src {rate}/{channels})"
        );

        let ring_pl = Arc::clone(&ring);
        let stop_pl = Arc::clone(&stop);
        let src_rate_pl = Arc::clone(&src_rate);
        let src_ch_pl = Arc::clone(&src_ch);
        let levels_pl = Arc::clone(&self.levels);
        {
            let mut state = self.state.lock();
            state.playout_join = Some(thread::spawn(move || {
                let _worker = crate::profile::worker("local_playout");
                let mut resampler = PcmResampler::new(2);
                let mut spectrum = spectrum_enabled.then(|| SpectrumTap::new(levels_pl));
                let mut spectrum_pending = Vec::with_capacity(play_rate as usize * play_ch);
                let spectrum_frame = (play_rate as usize * play_ch * 20 / 1000).max(play_ch);
                let mut interleaved = Vec::with_capacity(8192);
                let mut steady = SteadyPlayout::new(play_rate, play_ch, 20);
                let max_pending = play_rate as usize * play_ch * 2;
                loop {
                    if stop_pl.load(Ordering::SeqCst) {
                        break;
                    }
                    if steady.pending_len() < max_pending {
                        match pcm_rx.recv_timeout(steady.sleep_hint()) {
                            Ok(pcm) => {
                                playback_diag::local_pcm_recv();
                                let sr = src_rate_pl.load(Ordering::Acquire);
                                let sc = src_ch_pl.load(Ordering::Acquire).max(1) as u16;
                                resampler.set_format(sr, sc, play_rate);
                                let resample = crate::profile::scoped("local_resample");
                                resampler.push(&pcm, |out| {
                                    upmix_interleaved_into(
                                        out,
                                        sc as usize,
                                        play_ch,
                                        &mut interleaved,
                                    );
                                    if let Some(tap) = spectrum.as_mut() {
                                        spectrum_pending.extend_from_slice(&interleaved);
                                        while spectrum_pending.len() >= spectrum_frame {
                                            tap.push_f32(
                                                &spectrum_pending[..spectrum_frame],
                                                play_ch,
                                                play_rate,
                                            );
                                            spectrum_pending.copy_within(spectrum_frame.., 0);
                                            spectrum_pending
                                                .truncate(spectrum_pending.len() - spectrum_frame);
                                        }
                                    }
                                    steady.ingest(&interleaved);
                                });
                                drop(resample);
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        // Leave the bounded PCM channel full until wall-clock
                        // playout catches up; this propagates back-pressure to
                        // bursty decoders instead of decoding at maximum speed.
                        thread::sleep(steady.sleep_hint());
                    }
                    let due = steady.drain_due(2);
                    if !due.is_empty() {
                        if stop_pl.load(Ordering::SeqCst) {
                            return;
                        }
                        let pushed = ring_pl.push_slice(&due);
                        if pushed < due.len() {
                            playback_diag::event(
                                "local_ring_full",
                                &format!("dropped_samples={}", due.len() - pushed),
                            );
                        }
                        playback_diag::playout_pending(steady.pending_len());
                        playback_diag::playout_tick(pushed);
                        playback_diag::local_ring_fill(ring_pl.len());
                    }
                }
            }));
        }

        // Pre-buffer ~1 s at device rate so brief HTTP gaps do not underrun cpal.
        let min_buffer = play_rate as usize * play_ch;
        let prebuf_deadline = Instant::now() + Duration::from_secs(4);
        while ring.len() < min_buffer {
            if stop.load(Ordering::SeqCst) {
                log::info!("LocalPlayer::play: cancelled during pre-buffer");
                self.stop();
                return Err(LocalError::Stream("stopped".into()));
            }
            if let Some(e) = err_slot.lock().clone() {
                log::error!("LocalPlayer::play: decode error during pre-buffer: {e}");
                self.stop();
                return Err(LocalError::Stream(e));
            }
            if Instant::now() >= prebuf_deadline {
                log::warn!(
                    "LocalPlayer::play: pre-buffer timeout (have {} want {min_buffer})",
                    ring.len()
                );
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        let ring_cb = Arc::clone(&ring);
        let vol = Arc::clone(&self.volume);
        let stop_cb = Arc::clone(&stop);
        let err_cb = Arc::clone(&err_slot);
        let diag_ring = Arc::new(AtomicU32::new(0));
        let diag_ring_cb = Arc::clone(&diag_ring);
        let mut hold = vec![0.0f32; play_ch.max(1)];

        let stream = cpal_device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let _callback = crate::profile::scoped("cpal_callback");
                    if stop_cb.load(Ordering::SeqCst) {
                        data.fill(0.0);
                        return;
                    }
                    let gain = bits_f32(vol.load(Ordering::Relaxed));
                    let mut scratch = [0.0f32; 8];
                    for frame in data.chunks_mut(play_ch) {
                        let n = ring_cb.pop_slice(&mut scratch[..play_ch]);
                        if n >= play_ch {
                            for (out, &sample) in frame.iter_mut().zip(scratch[..play_ch].iter()) {
                                *out = sample * gain;
                            }
                            hold.copy_from_slice(&scratch[..play_ch]);
                        } else {
                            playback_diag::local_underrun(play_ch - n);
                            for (i, out) in frame.iter_mut().enumerate().take(play_ch) {
                                *out = hold.get(i).copied().unwrap_or(0.0) * gain;
                            }
                        }
                    }
                    let tick = diag_ring_cb.fetch_add(1, Ordering::Relaxed);
                    if tick & 31 == 0 {
                        playback_diag::local_ring_fill(ring_cb.len());
                    }
                },
                move |e| {
                    log::error!("LocalPlayer cpal stream error: {e}");
                    *err_cb.lock() = Some(e.to_string());
                },
                None,
            )
            .map_err(|e| LocalError::Audio(e.to_string()))?;

        stream
            .play()
            .map_err(|e| LocalError::Audio(e.to_string()))?;

        if stop.load(Ordering::SeqCst) {
            log::info!("LocalPlayer::play: cancelled after stream.play");
            drop(stream);
            return Err(LocalError::Stream("stopped".into()));
        }
        self.state.lock().stream = Some(SendStream(stream));

        on_status(&format!("Playing locally: «{}»", device.name));
        thread::sleep(Duration::from_millis(100));
        if stop.load(Ordering::SeqCst) {
            log::info!("LocalPlayer::play: cancelled during settle");
            self.stop();
            return Err(LocalError::Stream("stopped".into()));
        }
        if let Some(e) = err_slot.lock().clone() {
            log::error!("LocalPlayer::play: error after start: {e}");
            self.stop();
            return Err(LocalError::Stream(e));
        }
        log::info!("LocalPlayer::play Ok on '{}'", device.name);
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(LocalError::Stream("stopped".into()));
            }
            if let Some(e) = err_slot.lock().clone() {
                log::error!("LocalPlayer::play: stream ended after start: {e}");
                self.stop();
                return Err(LocalError::Stream(e));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for LocalPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}
