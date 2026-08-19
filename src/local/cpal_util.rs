//! cpal stream helpers and background cleanup.

use std::{
    sync::{mpsc, OnceLock},
    thread,
};

use cpal::traits::{DeviceTrait, HostTrait};

use super::{device::LocalDeviceInfo, error::LocalError};

/// cpal marks Stream as !Send for portability. Access is serialized via Mutex
/// and the reaper thread (WASAPI / ALSA / PipeWire).
#[allow(dead_code)]
pub(super) struct SendStream(pub(super) cpal::Stream);

// SAFETY: Stream lives only inside LocalPlayer; access is serialized via Mutex.
unsafe impl Send for SendStream {}
unsafe impl Sync for SendStream {}

pub(super) struct LocalCleanup {
    pub stream: Option<SendStream>,
    pub decode_join: Option<thread::JoinHandle<()>>,
    pub playout_join: Option<thread::JoinHandle<()>>,
}

pub(super) fn cleanup_sender() -> &'static mpsc::Sender<LocalCleanup> {
    static TX: OnceLock<mpsc::Sender<LocalCleanup>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<LocalCleanup>();
        thread::Builder::new()
            .name("rockcast-local-reaper".into())
            .spawn(move || {
                while let Ok(cleanup) = rx.recv() {
                    drop(cleanup.stream);
                    if let Some(join) = cleanup.playout_join {
                        let _ = join.join();
                    }
                    if let Some(join) = cleanup.decode_join {
                        let _ = join.join();
                    }
                }
            })
            .expect("spawn local audio cleanup worker");
        tx
    })
}

pub(super) fn f32_bits(v: f32) -> u32 {
    v.to_bits()
}

pub(super) fn bits_f32(b: u32) -> f32 {
    f32::from_bits(b)
}

pub(super) fn pick_cpal_device(info: &LocalDeviceInfo) -> Result<cpal::Device, LocalError> {
    let host = cpal::default_host();
    if let Some(want) = &info.cpal_name {
        let devices = host
            .output_devices()
            .map_err(|e| LocalError::Audio(e.to_string()))?;
        for d in devices {
            if d.name().ok().as_ref() == Some(want) {
                return Ok(d);
            }
        }
        return Err(LocalError::Audio(format!("device «{want}» not found")));
    }
    host.default_output_device()
        .ok_or_else(|| LocalError::Audio("no default output device".into()))
}

pub(super) fn pick_output_config(
    device: &cpal::Device,
    _prefer_rate: u32,
    prefer_ch: usize,
) -> Result<cpal::StreamConfig, LocalError> {
    // Default mix format is the reliable host path (WASAPI Shared, PipeWire via ALSA);
    // resample in the callback if the station rate differs.
    if let Ok(def) = device.default_output_config() {
        return Ok(def.config());
    }

    let supported = device
        .supported_output_configs()
        .map_err(|e| LocalError::Audio(e.to_string()))?;

    let prefer_ch = prefer_ch.clamp(1, 2) as u16;
    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
    for range in supported {
        if range.channels() == prefer_ch {
            best = Some(range);
            break;
        }
        if best.is_none() {
            best = Some(range);
        }
    }
    let range = best.ok_or_else(|| LocalError::Audio("no suitable output format".into()))?;
    Ok(range.with_max_sample_rate().config())
}
