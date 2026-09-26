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
    assert_eq!(manifest["manifest_revision"], json!(4));
    assert_eq!(manifest["roles"], json!(["player", "voice_endpoint"]));
    let items = manifest["capabilities"]["items"].as_array().unwrap();
    assert!(!items.iter().any(|item| item["name"] == "media.chromecast"));
    assert!(!items.iter().any(|item| item["name"] == "media.relay"));
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
fn player_state_publishes_optional_track_title() {
    let mut state = PlayerState::idle(63);
    state.playback_status = "playing";
    state.station_id = Some("station-a".into());
    state.track_title = Some("Artist - Track".into());
    let playback = serde_json::to_value(state.runtime_state()).unwrap()["playback"].clone();
    assert_eq!(playback["track_title"], "Artist - Track");
    state.track_title = None;
    let playback = serde_json::to_value(state.runtime_state()).unwrap()["playback"].clone();
    assert!(playback.get("track_title").is_none());
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
    let error = r#"{"protocol_version":1,"type":"protocol.error","payload":{"error":{"code":"invalid_message"}}}"#;
    assert_eq!(
        protocol_error_code(error).as_deref(),
        Some("invalid_message")
    );
    assert!(!is_auth_error(error));
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
                json!({"name":"station.play_stream","source":"rockserver_catalog","station_id":"station-rock-002","station":{"name":"Future Icon","icon_url":"https://icons.example.test/future.png"},"stream_uri":"https://stream.test/future.aac"})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::PlayStream {
            station_id: "station-rock-002".into(),
            station: Some(StationPresentation {
                name: "Future Icon".into(),
                icon_url: Some("https://icons.example.test/future.png".into()),
            }),
            stream_uri: "https://stream.test/future.aac".into(),
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
    // The server-resolved delivery carries the catalog id and display data next to the
    // stream URI; the ID remains state identity while the name repairs the local UI.
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"station.play_stream","source":"rockserver_catalog","station_id":"station-rock-001","station":{"name":"Server Rock","icon_url":null},"stream_uri":"https://stream.test/live.aac"})
            ),
            Some(device_id),
        )
        .unwrap()
        .command,
        PlayerCommand::PlayStream {
            station_id: "station-rock-001".into(),
            station: Some(StationPresentation {
                name: "Server Rock".into(),
                icon_url: None,
            }),
            stream_uri: "https://stream.test/live.aac".into(),
        }
    );
    assert_eq!(
        command_from_frame(
            &command_frame(
                command_id,
                device_id,
                json!({"name":"station.play_stream","source":"rockserver_catalog","stream_uri":"https://stream.test/live.aac"})
            ),
            Some(device_id),
        )
        .unwrap_err()
        .code,
        "invalid_payload"
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
    assert!(!command_is_advertised(&PlayerCommand::ChromecastDiscover));
    assert!(!command_is_advertised(&PlayerCommand::RelaySetMode {
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

fn player_state(status: &'static str, station_id: Option<&str>, volume: u8) -> PlayerState {
    PlayerState {
        playback_status: status,
        station_id: station_id.map(str::to_owned),
        track_title: None,
        volume,
        output_mode: "local",
        receiver_id: None,
    }
}

fn inner_with_state(state: Mutex<Option<PublishedState>>) -> ClientInner {
    ClientInner {
        config: RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        auth: Arc::new(FakeAuth),
        transport: Arc::new(FakeTransport),
        state,
        authenticated_device_id: Mutex::new(None),
        commands: Mutex::new(CommandBook::new()),
        wake_ui: Arc::new(|| {}),
        stopped: AtomicBool::new(false),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
    }
}

#[test]
fn playback_statuses_follow_the_phase_and_keep_station_context() {
    assert_eq!(playback_status(PlaybackPhase::Idle, None), "idle");
    // A station chosen earlier keeps its context through terminal Idle (§4.5);
    // the mapping has no input that could fabricate `paused`.
    assert_eq!(
        playback_status(PlaybackPhase::Idle, Some("somafm-metal-detector")),
        "stopped"
    );
    assert_eq!(
        playback_status(
            PlaybackPhase::Opening {
                generation: 1,
                local: true,
            },
            Some("somafm-metal-detector"),
        ),
        "buffering"
    );
    assert_eq!(
        playback_status(
            PlaybackPhase::Playing {
                generation: 1,
                local: false,
            },
            Some("somafm-metal-detector"),
        ),
        "playing"
    );
    assert_eq!(
        playback_status(PlaybackPhase::Stopping { generation: 2 }, Some("station-a")),
        "stopped"
    );
    assert_eq!(
        playback_status(PlaybackPhase::Failed { generation: 3 }, Some("station-a")),
        "error"
    );
}

#[test]
fn truthful_publisher_lifecycle_is_deduplicated_monotonic_and_resyncable() {
    let slot = Mutex::new(Some(PublishedState {
        revision: 40,
        observed_at: String::new(),
        state: PlayerState::idle(62),
    }));
    // Startup: the first observed snapshot always advances the persisted
    // revision instead of replaying the previous process's value.
    assert_eq!(
        advance_published_state(&slot, player_state("idle", None, 62)),
        Some(41)
    );
    // The registration-time snapshot is the complete idle fact: no station
    // chosen yet, factual volume, no fabricated playback value.
    {
        let current = slot.lock();
        let runtime =
            serde_json::to_value(current.as_ref().unwrap().state.runtime_state()).unwrap();
        assert_eq!(
            runtime["playback"],
            json!({"status": "idle", "station_id": null})
        );
        assert_eq!(runtime["volume"]["level"], json!(62));
    }
    // An accepted station A begins loading: buffering with the exact ID (§4.3).
    assert_eq!(
        advance_published_state(&slot, player_state("buffering", Some("station-a"), 62)),
        Some(42)
    );
    // A duplicate fact republishes nothing and bumps no revision (§4.3).
    assert_eq!(
        advance_published_state(&slot, player_state("buffering", Some("station-a"), 62)),
        None
    );
    // Confirmed playback, then a local volume change echoed at the new level.
    assert_eq!(
        advance_published_state(&slot, player_state("playing", Some("station-a"), 62)),
        Some(43)
    );
    assert_eq!(
        advance_published_state(&slot, player_state("playing", Some("station-a"), 71)),
        Some(44)
    );
    // A failed start keeps the station context instead of nulling it (§4.5).
    assert_eq!(
        advance_published_state(&slot, player_state("error", Some("station-a"), 71)),
        Some(45)
    );
    // Stop keeps the last station until a new choice is made (§4.5).
    assert_eq!(
        advance_published_state(&slot, player_state("stopped", Some("station-a"), 71)),
        Some(46)
    );
    // Choosing station B starts its own lifecycle with B's exact ID.
    assert_eq!(
        advance_published_state(&slot, player_state("buffering", Some("station-b"), 71)),
        Some(47)
    );

    let inner = inner_with_state(slot);
    let mut socket = FakeSocket {
        inbound: VecDeque::from([message("protocol.welcome"), message("device.registered")]),
        sent: vec![],
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    wait_for(&mut socket, deadline, "protocol.welcome", &inner).unwrap();
    wait_for(&mut socket, deadline, "device.registered", &inner).unwrap();
    // Registration always publishes the complete current snapshot (§4.3).
    let sent_revision = send_full(&mut socket, &inner, 0).unwrap();
    assert_eq!(sent_revision, 47);
    let snapshot = &socket.sent.last().unwrap()["payload"]["snapshot"];
    assert_eq!(snapshot["state_revision"], json!(47));
    assert!(
        snapshot["observed_at"]
            .as_str()
            .is_some_and(|at| !at.is_empty())
    );
    assert_eq!(
        snapshot["state"]["playback"],
        json!({"status": "buffering", "station_id": "station-b"})
    );
    assert_eq!(
        snapshot["state"]["volume"],
        json!({"level": 71, "muted": false})
    );
    assert_eq!(snapshot["state"]["output"], json!({"mode": "local"}));
    // Without a newer fact the live connection sends nothing more.
    assert!(!state_is_newer_than(&inner.state, sent_revision));
    // A reconnect or server resync resends the same complete snapshot.
    assert_eq!(send_full(&mut socket, &inner, 0).unwrap(), 47);
    assert_eq!(
        socket
            .sent
            .iter()
            .filter(|frame| frame["type"] == "device.state_full")
            .count(),
        2
    );
    // A newer fact on the live slot is delivered as the next full snapshot.
    assert_eq!(
        advance_published_state(&inner.state, player_state("playing", Some("station-b"), 71)),
        Some(48)
    );
    assert!(state_is_newer_than(&inner.state, sent_revision));
    assert_eq!(send_full(&mut socket, &inner, sent_revision).unwrap(), 48);
}

#[test]
fn restarted_client_resumes_the_persisted_revision_without_rollback() {
    let client = DeviceControlClient::with_parts(
        RuntimeConfig::for_test("http://127.0.0.1".into(), None),
        41,
        Arc::new(FakeAuth),
        Arc::new(FakeTransport),
    );
    // AppSettings seeded 41, so the first snapshot of the new process is 42
    // and equal facts still do not bump it further.
    assert_eq!(client.publish(PlayerState::idle(50)), Some(42));
    assert_eq!(client.publish(PlayerState::idle(50)), None);
    assert_eq!(
        client.publish(player_state("playing", Some("station-a"), 50)),
        Some(43)
    );
    client.shutdown();
}
