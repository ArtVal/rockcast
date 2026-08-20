//! Live regression probe: transcoded relay must preserve ICY titles for the UI.

use std::{
    sync::{Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

use rockcast::relay::StreamRelay;

static ENV_LOCK: Mutex<()> = Mutex::new(());
const ROCK_ANTENNE_AAC_URL: &str = "https://stream.rockantenne.de/heavy-metal/stream/aacp";

#[test]
#[ignore = "uses live Rock Antenne AAC stream and a local relay listener"]
fn live_via_pc_aac_forwards_icy_title_to_relay() {
    let _ = env_logger::builder().is_test(true).try_init();
    let _guard = ENV_LOCK.lock().expect("env lock");
    unsafe {
        std::env::set_var("ROCKCAST_RELAY_ADVERTISE_IP", "127.0.0.1");
    }

    let relay = StreamRelay::new();
    let cancel = AtomicBool::new(false);
    relay
        .start(ROCK_ANTENNE_AAC_URL, "127.0.0.1", "audio/aac", &cancel)
        .expect("start AAC relay");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut title = None;
    while Instant::now() < deadline {
        if let Some(value) = relay.latest_title()
            && !value.is_empty()
        {
            title = Some(value);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    relay.stop();
    unsafe {
        std::env::remove_var("ROCKCAST_RELAY_ADVERTISE_IP");
    }

    assert!(
        title.is_some(),
        "AAC relay did not forward an ICY stream title"
    );
    eprintln!("relay ICY title: {}", title.unwrap());
}
