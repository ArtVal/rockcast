//! Regression coverage for the bounded DC-012 registration loop.

use super::*;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

struct FakeSocket {
    inbound: VecDeque<Inbound>,
    sent: Vec<Value>,
}

impl ControlSocket for FakeSocket {
    fn send_text(&mut self, text: String) -> Result<(), ControlError> {
        self.sent.push(serde_json::from_str(&text).unwrap());
        Ok(())
    }

    fn read(&mut self) -> Result<Option<Inbound>, ControlError> {
        Ok(self.inbound.pop_front())
    }

    fn close(&mut self) {}
}

fn message(kind: &str) -> Inbound {
    Inbound::Text(json!({ "protocol_version": 1, "type": kind, "payload": {} }).to_string())
}

#[test]
fn manifest_and_state_advertise_only_the_implemented_output_actions() {
    let manifest = serde_json::to_value(super::protocol::manifest()).unwrap();
    assert_eq!(manifest["roles"], json!(["player", "voice_endpoint"]));
    let items = manifest["capabilities"]["items"].as_array().unwrap();
    assert!(items.iter().any(|item| {
        item == &json!({
            "name": "media.chromecast", "version": 1,
            "actions": ["discover", "connect", "disconnect"],
            "discovery_ttl_seconds": 60,
        })
    }));
    assert!(items.iter().any(|item| {
        item == &json!({
            "name": "media.relay", "version": 1,
            "actions": ["start", "stop", "set_mode"], "modes": ["via_pc"],
        })
    }));
    assert!(items.iter().any(|item| {
        item == &json!({
            "name": "voice.input", "version": 1,
            "formats": ["pcm16_mono_16000"],
        })
    }));
    assert_eq!(
        manifest["surfaces"],
        json!([{
            "surface_id": "voice.main",
            "kind": "voice",
            "display_name": "RockCast voice",
            "views": []
        }])
    );
    assert_eq!(
        serde_json::to_value(PlayerState::idle(63).runtime_state()).unwrap()["volume"],
        json!({"level":63,"muted":false})
    );
    assert_eq!(
        serde_json::to_value(PlayerState::idle(63).runtime_state()).unwrap()["output"],
        json!({"mode":"local"})
    );
}

#[test]
fn protocol_parser_rejects_malformed_and_oversized_frames() {
    assert_eq!(
        inbound_type(r#"{"protocol_version":1,"type":"future.notice","payload":{}}"#).unwrap(),
        Some("future.notice".into())
    );
    assert_eq!(
        inbound_type(r#"{"protocol_version":1,"type":"device.command","payload":[]}"#),
        Err(ControlError::Protocol)
    );
    assert_eq!(
        inbound_type(&"x".repeat(protocol::MAX_FRAME_BYTES + 1)),
        Err(ControlError::Protocol)
    );
}

fn command_frame(command_id: &str, device_id: &str, body: Value) -> String {
    json!({
        "protocol_version": 1,
        "message_id": "00000000-0000-4000-8000-000000000001",
        "type": "device.command",
        "sent_at": "2026-09-04T00:00:00Z",
        "payload": { "command_id": command_id, "target": { "device_id": device_id }, "body": body }
    })
    .to_string()
}

#[test]
fn commands_are_strictly_bounded_and_catalog_only() {
    let command_id = "00000000-0000-4000-8000-000000000002";
    let device_id = "00000000-0000-4000-8000-000000000003";
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"station.play_station","station_id":"somafm-metal-detector"})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::PlayStation {
            station_id: "somafm-metal-detector".into()
        }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"volume.change_volume","delta":-7})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::ChangeVolume { delta: -7 }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(command_id, device_id, json!({"name":"station.play_stream","source":"direct_stream","stream_uri":"https://unsafe.example/stream"})),
            Some(device_id),
        ).unwrap_err().code,
        "unsupported_command"
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                "00000000-0000-4000-8000-000000000004",
                json!({"name":"playback.play"})
            ),
            Some(device_id),
        )
        .unwrap_err()
        .code,
        "invalid_payload"
    );
    for (name, expected) in [
        ("playback.play", PlayerCommand::Play),
        ("playback.pause", PlayerCommand::Pause),
        ("playback.stop", PlayerCommand::Stop),
        ("playback.next", PlayerCommand::Next),
        ("playback.previous", PlayerCommand::Previous),
    ] {
        assert_eq!(
            command_from_frame(
                &command_frame(command_id, device_id, json!({"name": name})),
                Some(device_id)
            )
            .unwrap()
            .command,
            expected
        );
    }
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"volume.set_volume","level":100})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::SetVolume { level: 100 }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"volume.set_mute","muted":true})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::SetMute { muted: true }
    );
    let receiver_id = "00000000-0000-4000-8000-000000000007";
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"chromecast.connect","receiver_id":receiver_id})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::ChromecastConnect {
            receiver_id: receiver_id.into()
        }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"relay.set_mode","mode":"via_pc"})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::RelaySetMode {
            mode: "via_pc".into()
        }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"chromecast.connect","receiver_id":"192.168.1.4"})
            ),
            Some(device_id),
        )
        .unwrap_err()
        .code,
        "invalid_payload"
    );
}

#[test]
fn unsupported_local_capabilities_never_reach_the_ui() {
    assert!(command_is_advertised(&PlayerCommand::Play));
    assert!(command_is_advertised(&PlayerCommand::SetVolume {
        level: 50
    }));
    assert!(!command_is_advertised(&PlayerCommand::Pause));
    assert!(!command_is_advertised(&PlayerCommand::SetMute {
        muted: true
    }));
    assert!(command_is_advertised(&PlayerCommand::ChromecastDiscover));
    assert!(command_is_advertised(&PlayerCommand::RelaySetMode {
        mode: "via_pc".into()
    }));
}

#[test]
fn discovery_result_uses_the_canonical_receivers_output_shape() {
    let mut socket = FakeSocket {
        inbound: VecDeque::new(),
        sent: vec![],
    };
    command_result(
        &mut socket,
        "00000000-0000-4000-8000-000000000008",
        &CommandResult::succeeded_with_receivers(vec![super::output::ChromecastReceiver {
            receiver_id: "00000000-0000-4000-8000-000000000009".into(),
            display_name: "Kitchen".into(),
            discovered_at: "2026-09-06T00:00:00Z".into(),
            expires_at: "2026-09-06T00:01:00Z".into(),
        }]),
    )
    .unwrap();
    let payload = &socket.sent[0]["payload"];
    assert_eq!(payload["status"], "succeeded");
    assert_eq!(payload["error"], Value::Null);
    assert_eq!(payload["output"]["receivers"][0]["display_name"], "Kitchen");
}

#[test]
fn duplicate_command_executes_once_and_sends_one_terminal_result() {
    let command_id = "00000000-0000-4000-8000-000000000005";
    let device_id = "00000000-0000-4000-8000-000000000006";
    let wakes = Arc::new(AtomicUsize::new(0));
    let inner = ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state: Mutex::new(None),
        authenticated_device_id: Mutex::new(Some(device_id.into())),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: {
            let wakes = Arc::clone(&wakes);
            Arc::new(move || {
                wakes.fetch_add(1, Ordering::Relaxed);
            })
        },
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    let frame = command_frame(command_id, device_id, json!({"name":"playback.stop"}));
    let mut socket = FakeSocket {
        inbound: VecDeque::new(),
        sent: vec![],
    };
    receive_command(&mut socket, &inner, &frame).unwrap();
    receive_command(&mut socket, &inner, &frame).unwrap();
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(
        socket
            .sent
            .iter()
            .filter(|frame| frame["type"] == "command.accepted")
            .count(),
        1
    );
    let command = inner.commands.lock().queued.pop_front().unwrap();
    inner
        .commands
        .lock()
        .in_flight
        .insert(command.id.clone(), command);
    inner
        .commands
        .lock()
        .complete(command_id, CommandResult::succeeded());
    send_pending_results(&mut socket, &inner).unwrap();
    send_pending_results(&mut socket, &inner).unwrap();
    assert_eq!(
        socket
            .sent
            .iter()
            .filter(|frame| frame["type"] == "command.result")
            .count(),
        1
    );
}

#[test]
fn full_command_queue_rejects_without_growing() {
    let device_id = "00000000-0000-4000-8000-000000000006";
    let inner = ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state: Mutex::new(None),
        authenticated_device_id: Mutex::new(Some(device_id.into())),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: Arc::new(|| {}),
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    let mut socket = FakeSocket {
        inbound: VecDeque::new(),
        sent: vec![],
    };
    for index in 0..=MAX_PENDING_COMMANDS {
        let command_id = format!("00000000-0000-4000-8000-{index:012x}");
        let frame = command_frame(&command_id, device_id, json!({"name":"playback.stop"}));
        receive_command(&mut socket, &inner, &frame).unwrap();
    }
    assert_eq!(inner.commands.lock().queued.len(), MAX_PENDING_COMMANDS);
    assert_eq!(
        socket.sent.last().unwrap()["payload"]["error"]["code"],
        "command_timeout"
    );
}

#[test]
fn reconnect_backoff_is_bounded_and_deterministic() {
    assert_eq!(backoff(0), Duration::from_secs(1));
    assert_eq!(backoff(5), Duration::from_secs(30));
    assert_eq!(backoff(99), Duration::from_secs(30));
}

#[test]
fn state_snapshot_releases_mutex_before_publish() {
    let state = Mutex::new(Some(PublishedState {
        revision: 2,
        observed_at: timestamp(),
        state: PlayerState::idle(50),
    }));
    assert!(state_is_newer_than(&state, 1));
    assert!(state.try_lock().is_some());
}

#[test]
fn hello_registration_and_fresh_snapshot_are_ordered() {
    let state = PublishedState {
        revision: 7,
        observed_at: timestamp(),
        state: PlayerState::idle(50),
    };
    let inner = ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state: Mutex::new(Some(state)),
        authenticated_device_id: Mutex::new(None),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: Arc::new(|| {}),
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    let mut socket = FakeSocket {
        inbound: VecDeque::from([message("protocol.welcome"), message("device.registered")]),
        sent: vec![],
    };
    assert!(
        send(
            &mut socket,
            "protocol.hello",
            json!({"supported_protocol_versions":[1]}),
        )
        .is_ok()
    );
    wait_for(
        &mut socket,
        Instant::now() + Duration::from_secs(1),
        "protocol.welcome",
        &inner,
    )
    .unwrap();
    send(
        &mut socket,
        "device.register",
        json!({"device_type":"rockcast","app_version":"0","manifest":super::protocol::manifest()}),
    )
    .unwrap();
    wait_for(
        &mut socket,
        Instant::now() + Duration::from_secs(1),
        "device.registered",
        &inner,
    )
    .unwrap();
    send_full(&mut socket, &inner, 0).unwrap();
    assert_eq!(
        socket
            .sent
            .iter()
            .map(|message| message["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["protocol.hello", "device.register", "device.state_full"]
    );
}

#[test]
fn resync_always_publishes_another_full_snapshot() {
    let state = PublishedState {
        revision: 7,
        observed_at: timestamp(),
        state: PlayerState::idle(50),
    };
    let inner = ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state: Mutex::new(Some(state)),
        authenticated_device_id: Mutex::new(None),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: Arc::new(|| {}),
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    let mut socket = FakeSocket {
        inbound: VecDeque::new(),
        sent: vec![],
    };
    let revision = send_full(&mut socket, &inner, 0).unwrap();
    send_full(&mut socket, &inner, revision).unwrap();
    assert_eq!(
        socket
            .sent
            .iter()
            .filter(|message| message["type"] == "device.state_full")
            .count(),
        2
    );
}

#[test]
fn server_disconnect_is_recoverable_and_does_not_execute_playback() {
    let inner = ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state: Mutex::new(None),
        authenticated_device_id: Mutex::new(None),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: Arc::new(|| {}),
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    };
    let mut socket = FakeSocket {
        inbound: VecDeque::from([Inbound::Close]),
        sent: vec![],
    };
    assert_eq!(
        wait_for(
            &mut socket,
            Instant::now() + Duration::from_secs(1),
            "protocol.welcome",
            &inner
        ),
        Err(ControlError::Unavailable)
    );
    assert!(socket.sent.is_empty());
}

#[test]
fn start_keeps_one_live_worker() {
    let (started_tx, started_rx) = mpsc::channel();
    let client = DeviceControlClient::with_parts(
        RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        0,
        Arc::new(CountingAuth {
            calls: AtomicUsize::new(0),
            started: started_tx,
        }),
        Arc::new(FakeTransport),
    );
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    client.start();
    assert!(started_rx.recv_timeout(Duration::from_millis(100)).is_err());
    client.shutdown();
}

struct FakeAuth;

impl DeviceControlAuth for FakeAuth {
    fn access_token(&self, _: bool) -> Result<Option<String>, SessionError> {
        Ok(None)
    }
}

struct FakeTransport;

impl DeviceControlTransport for FakeTransport {
    fn connect(&self, _: &str, _: &str) -> Result<Box<dyn ControlSocket>, ControlError> {
        Err(ControlError::Unavailable)
    }
}

struct CountingAuth {
    calls: AtomicUsize,
    started: mpsc::Sender<()>,
}

impl DeviceControlAuth for CountingAuth {
    fn access_token(&self, _: bool) -> Result<Option<String>, SessionError> {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            let _ = self.started.send(());
        }
        Ok(None)
    }
}
