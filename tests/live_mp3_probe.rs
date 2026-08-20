//! Manual live MP3 decode probe.
//!
//! Run with: `cargo test live_mp3 -- --ignored --nocapture`.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use rockcast::audio::decode::run_live_decode_f32;

const ROCK_ANTENNE_MP3_URL: &str = "https://stream.rockantenne.de/heavy-metal/stream/mp3";

#[test]
#[ignore = "uses a live MP3 stream"]
fn live_mp3_decode_is_sane() {
    let stop = Arc::new(AtomicBool::new(false));
    let rate = Arc::new(AtomicU32::new(0));
    let channels = Arc::new(AtomicU32::new(0));
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let decode = thread::spawn({
        let stop = Arc::clone(&stop);
        let rate = Arc::clone(&rate);
        let channels = Arc::clone(&channels);
        let samples = Arc::clone(&samples);
        move || {
            run_live_decode_f32(
                ROCK_ANTENNE_MP3_URL,
                &stop,
                None,
                None,
                rate,
                channels,
                move |pcm| {
                    samples.lock().unwrap().extend_from_slice(pcm);
                },
            )
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && samples.lock().unwrap().len() < 44_100 {
        thread::sleep(Duration::from_millis(25));
    }
    stop.store(true, Ordering::SeqCst);
    let _ = decode.join();

    let pcm = samples.lock().unwrap();
    let (mut sum_sq, mut peak, mut invalid) = (0.0f64, 0.0f32, 0usize);
    for &sample in pcm.iter() {
        if !sample.is_finite() {
            invalid += 1;
            continue;
        }
        sum_sq += f64::from(sample) * f64::from(sample);
        peak = peak.max(sample.abs());
    }
    let rms = (sum_sq / pcm.len().max(1) as f64).sqrt();
    eprintln!(
        "MP3: samples={} rate={} ch={} rms={rms:.4} peak={peak:.4} invalid={invalid}",
        pcm.len(),
        rate.load(Ordering::SeqCst),
        channels.load(Ordering::SeqCst)
    );
    assert!(pcm.len() > 22_050, "too few MP3 samples: {}", pcm.len());
    assert!(
        rate.load(Ordering::SeqCst) >= 22_050,
        "MP3 decoder did not report rate"
    );
    assert!(
        channels.load(Ordering::SeqCst) >= 1,
        "MP3 decoder did not report channels"
    );
    assert_eq!(invalid, 0, "MP3 decoder emitted non-finite PCM");
    assert!(peak < 1.5, "MP3 peak too high: {peak}");
    assert!(
        (0.001..0.5).contains(&rms),
        "MP3 RMS out of music range: {rms}"
    );
}
