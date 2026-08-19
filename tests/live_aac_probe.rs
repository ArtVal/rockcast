//! Live probe for AAC+ decode quality (manual: `cargo test live_aac -- --ignored --nocapture`).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use rockcast::audio::decode::run_live_decode_f32;

fn stats(samples: &[f32]) -> (f64, f64, usize) {
    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f64;
    let mut bad = 0usize;
    for &s in samples {
        if !s.is_finite() {
            bad += 1;
            continue;
        }
        let a = f64::from(s.abs());
        peak = peak.max(a);
        sum_sq += f64::from(s) * f64::from(s);
    }
    let rms = (sum_sq / samples.len().max(1) as f64).sqrt();
    (rms, peak, bad)
}

#[test]
#[ignore = "uses live rockantenne aacp stream"]
fn live_aac_fdk_decode_is_sane() {
    let url = "https://stream.rockantenne.de/heavy-metal/stream/aacp";
    let stop = Arc::new(AtomicBool::new(false));
    let src_rate = Arc::new(AtomicU32::new(0));
    let src_ch = Arc::new(AtomicU32::new(2));
    let ring: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ring_push = Arc::clone(&ring);
    let stop_push = Arc::clone(&stop);

    let src_rate_wait = Arc::clone(&src_rate);
    let src_ch_wait = Arc::clone(&src_ch);
    let decode = std::thread::spawn({
        let url = url.to_string();
        move || {
            run_live_decode_f32(
                &url,
                &stop_push,
                None,
                None,
                src_rate,
                src_ch,
                move |pcm| {
                    ring_push.lock().unwrap().extend_from_slice(pcm);
                },
            )
        }
    });

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let rate = src_rate_wait.load(Ordering::SeqCst);
        let ch = src_ch_wait.load(Ordering::SeqCst).max(1) as usize;
        if rate > 0 && ring.lock().unwrap().len() > rate as usize * ch * 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    let _ = decode.join();

    let all = ring.lock().unwrap().clone();
    assert!(all.len() > 22_050, "too few samples: {}", all.len());
    let (rms, peak, bad) = stats(&all);
    eprintln!(
        "libfdk AAC: samples={} rate={} ch={} rms={rms:.4} peak={peak:.4} bad={bad}",
        all.len(),
        src_rate_wait.load(Ordering::SeqCst),
        src_ch_wait.load(Ordering::SeqCst),
    );
    assert!(bad == 0, "non-finite samples");
    assert!(peak < 1.5, "peak too high — likely garbage decode");
    assert!(rms > 0.001 && rms < 0.5, "rms {rms} out of music-like range");
    let rate = src_rate_wait.load(Ordering::SeqCst);
    assert!(
        rate >= 44_100,
        "HE-AAC/aacp should report PCM output rate (sampleRate), got {rate}"
    );
}

#[test]
#[ignore = "uses live rockantenne aacp stream"]
fn live_aac_resample_output_is_sane() {
    use std::sync::Mutex;

    let url = "https://stream.rockantenne.de/heavy-metal/stream/aacp";
    let stop = Arc::new(AtomicBool::new(false));
    let src_rate = Arc::new(AtomicU32::new(0));
    let src_ch = Arc::new(AtomicU32::new(2));
    let ring: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let ring_push = Arc::clone(&ring);
    let stop_push = Arc::clone(&stop);

    let src_rate_wait = Arc::clone(&src_rate);
    let src_ch_wait = Arc::clone(&src_ch);
    let decode = std::thread::spawn({
        let url = url.to_string();
        move || {
            run_live_decode_f32(
                &url,
                &stop_push,
                None,
                None,
                Arc::clone(&src_rate),
                Arc::clone(&src_ch),
                move |pcm| {
                    ring_push.lock().unwrap().extend_from_slice(pcm);
                },
            )
        }
    });

    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        if src_rate_wait.load(Ordering::SeqCst) > 0 && ring.lock().unwrap().len() > 22050 * 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    let _ = decode.join();

    let rate = src_rate_wait.load(Ordering::SeqCst);
    let channels = src_ch_wait.load(Ordering::SeqCst).max(1) as usize;
    let mut ring = ring.lock().unwrap().clone();
    assert!(rate > 0, "probe failed");
    assert!(ring.len() > rate as usize * channels, "ring too short");

    let out_rate = 48_000u32;
    let out_ch = 2usize;
    let ratio = rate as f64 / out_rate as f64;
    let mut read_pos = 0.0f64;
    let mut out = Vec::new();
    let frames_out = out_rate as usize * 2;
    while out.len() < frames_out * out_ch {
        let need = ((read_pos.floor() as usize) + 2) * channels;
        if ring.len() < need {
            out.extend(std::iter::repeat(0.0f32).take(out_ch));
            continue;
        }
        let i0 = read_pos.floor() as usize;
        let frac = (read_pos - i0 as f64) as f32;
        for c in 0..out_ch {
            let src_c = c.min(channels - 1);
            let s0 = ring[i0 * channels + src_c];
            let s1 = ring[(i0 + 1) * channels + src_c];
            out.push(s0 + (s1 - s0) * frac);
        }
        read_pos += ratio;
        let drop_frames = read_pos.floor() as usize;
        if drop_frames > 0 {
            let drop_samples = drop_frames * channels;
            if drop_samples <= ring.len() {
                ring.drain(..drop_samples);
                read_pos -= drop_frames as f64;
            }
        }
    }

    let (rms, peak, bad) = stats(&out);
    eprintln!(
        "resampled: out_samples={} rate={rate} rms={rms:.4} peak={peak:.4} bad={bad}",
        out.len()
    );
    assert!(bad == 0);
    assert!(peak < 1.5 && rms > 0.001 && rms < 0.5);
}

#[test]
#[ignore = "uses live somafm aac stream"]
fn live_somafm_aac_decode_is_sane() {
    let url = "https://ice4.somafm.com/metal-128-aac";
    let stop = Arc::new(AtomicBool::new(false));
    let src_rate = Arc::new(AtomicU32::new(0));
    let src_ch = Arc::new(AtomicU32::new(2));
    let ring: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ring_push = Arc::clone(&ring);
    let stop_push = Arc::clone(&stop);

    let src_rate_wait = Arc::clone(&src_rate);
    let src_ch_wait = Arc::clone(&src_ch);
    let decode = std::thread::spawn({
        let url = url.to_string();
        move || {
            run_live_decode_f32(
                &url,
                &stop_push,
                None,
                None,
                src_rate,
                src_ch,
                move |pcm| {
                    ring_push.lock().unwrap().extend_from_slice(pcm);
                },
            )
        }
    });

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let rate = src_rate_wait.load(Ordering::SeqCst);
        let ch = src_ch_wait.load(Ordering::SeqCst).max(1) as usize;
        if rate > 0 && ring.lock().unwrap().len() > rate as usize * ch * 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop.store(true, Ordering::SeqCst);
    let decode_err = decode.join().unwrap();
    assert!(decode_err.is_err(), "expected stopped, got {decode_err:?}");

    let all = ring.lock().unwrap().clone();
    assert!(all.len() > 22_050, "too few samples: {}", all.len());
    let (rms, peak, bad) = stats(&all);
    eprintln!(
        "SomaFM AAC: samples={} rate={} ch={} rms={rms:.4} peak={peak:.4} bad={bad}",
        all.len(),
        src_rate_wait.load(Ordering::SeqCst),
        src_ch_wait.load(Ordering::SeqCst),
    );
    assert!(bad == 0, "non-finite samples");
    assert!(peak < 1.5, "peak too high — likely garbage decode");
    assert!(rms > 0.001 && rms < 0.5, "rms {rms} out of music-like range");
}
