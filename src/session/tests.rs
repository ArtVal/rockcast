use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::Mutex,
    thread,
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::rockserver::RuntimeConfig;
use super::client::{AccountClient, SESSION_MUTEX, access_token_needs_refresh};
use super::storage::{CredentialStore, decode_credentials};
#[cfg(windows)]
use super::storage::{dpapi, read_credentials};
use super::types::{
    NativeCredentials, PairingCompletionRequest, PairingPoll, PairingPollControl, PairingRequest,
    SessionError, pairing_poll_control,
};

struct Memory(Mutex<Option<NativeCredentials>>);
impl CredentialStore for Memory {
    fn load(&self) -> Result<Option<NativeCredentials>, SessionError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, c: &NativeCredentials) -> Result<(), SessionError> {
        *self.0.lock().unwrap() = Some(c.clone());
        Ok(())
    }
    fn clear(&self) -> Result<(), SessionError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}
const FRESH_ACCESS_EXPIRES_AT: &str = "2030-01-01T12:00:00Z";

fn stored_credentials(access_token: &str) -> NativeCredentials {
    NativeCredentials::new(
        "device".into(),
        "b".repeat(43),
        access_token.into(),
        FRESH_ACCESS_EXPIRES_AT.into(),
    )
    .unwrap()
}

#[test]
fn access_token_needs_refresh_only_when_unknown_or_near_expiry() {
    assert!(!access_token_needs_refresh(FRESH_ACCESS_EXPIRES_AT));
    assert!(!access_token_needs_refresh("2030-01-01T12:00:00.123456Z"));
    assert!(access_token_needs_refresh(""));
    assert!(access_token_needs_refresh(
        &OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
    ));
}

#[test]
fn legacy_stored_credentials_without_expiry_still_load() {
    let legacy = br#"{"device_id":"device","device_secret":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","access_token":"aaaaaaaaaaaaaaaa"}"#;
    let credentials = decode_credentials(legacy).unwrap().expect("legacy session");
    assert_eq!(credentials.access_expires_at(), "");
    assert!(access_token_needs_refresh(credentials.access_expires_at()));
}

#[test]
fn secrets_do_not_format_or_serialize() {
    let credentials = stored_credentials("a".repeat(16).as_str());
    let store = Memory(Mutex::new(Some(credentials)));
    assert_eq!(store.load().unwrap().unwrap().access_token().len(), 16);
}

#[test]
fn unavailable_secure_store_fails_closed() {
    #[cfg(not(windows))]
    assert_eq!(
        OsCredentialStore.load().unwrap_err(),
        SessionError::SecureStorageUnavailable
    );
}

#[test]
fn legacy_refresh_blob_is_an_absent_session_not_a_storage_failure() {
    let legacy = br#"{"access_token":"aaaaaaaaaaaaaaaa","refresh_token":"bbbbbbbbbbbbbbbb"}"#;
    assert!(decode_credentials(legacy).unwrap().is_none());
}

#[cfg(windows)]
#[test]
fn legacy_dpapi_blob_is_an_absent_session_not_a_storage_failure() {
    let path = std::env::temp_dir().join(format!(
        "rockcast-legacy-session-{}.dpapi",
        uuid::Uuid::new_v4()
    ));
    let legacy = br#"{"access_token":"aaaaaaaaaaaaaaaa","refresh_token":"bbbbbbbbbbbbbbbb"}"#;
    fs::write(&path, dpapi(true, legacy).unwrap()).unwrap();
    assert!(read_credentials(&path).unwrap().is_none());
    let _ = fs::remove_file(path);
}

#[test]
fn pairing_poll_stops_only_for_cancel_or_deadline() {
    let now = std::time::Instant::now();
    assert_eq!(
        pairing_poll_control(false, now, now + Duration::from_secs(1)),
        PairingPollControl::Continue
    );
    assert_eq!(
        pairing_poll_control(true, now, now + Duration::from_secs(1)),
        PairingPollControl::Cancelled
    );
    assert_eq!(
        pairing_poll_control(false, now + Duration::from_secs(1), now),
        PairingPollControl::TimedOut
    );
}

fn server(status: u16, body: &'static str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while let Ok(read) = stream.read(&mut chunk) {
            if read == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..read]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request[..end]);
                let length = header
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let response = format!(
            "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
        String::from_utf8_lossy(&request).into_owned()
    });
    (format!("http://{address}"), thread)
}

fn client(url: String, store: Memory) -> AccountClient<Memory> {
    AccountClient::new(RuntimeConfig::for_test(url, None), store)
}

#[test]
fn offline_pairing_returns_safe_error_without_credentials() {
    let client = client("http://127.0.0.1:1".into(), Memory(Mutex::new(None)));
    assert!(matches!(
        client.create_pairing("RockCast — offline"),
        Err(SessionError::Unavailable)
    ));
}

#[test]
fn create_pairing_uses_g1_contract_without_authorization() {
    let body = r#"{"pairing_request_id":"request","desktop_token":"aaaaaaaaaaaaaaaa","approval_secret":"bbbbbbbbbbbbbbbb","short_code":"AB12CD34","verification_phrase":"AMBER-DAWN","device_display_name":"RockCast — test","device_type":"windows","expires_at":"2026-08-28T12:00:00Z","status":"pending"}"#;
    let (url, server) = server(201, body);
    let pairing = client(url, Memory(Mutex::new(None)))
        .create_pairing("RockCast — test")
        .unwrap();
    assert_eq!(pairing.short_code, "AB12CD34");
    assert_eq!(pairing.device_display_name, "RockCast — test");
    assert_eq!(pairing.status, "pending");
    let request = server.join().unwrap();
    let request_lower = request.to_ascii_lowercase();
    assert!(request_lower.starts_with("post /api/v1/pairing-requests"));
    assert!(request.contains(r#""device_display_name":"RockCast — test""#));
    assert!(request.contains(r#""device_type":"windows""#));
    assert!(!request_lower.contains("\"device_name\""));
    assert!(!request_lower.contains("\"platform\""));
    assert!(!request_lower.contains("authorization:"));
}

#[test]
fn pairing_link_keeps_the_approval_secret_out_of_the_query() {
    let pairing = PairingRequest {
        pairing_request_id: "request".into(),
        desktop_token: "desktop-token-1234".into(),
        approval_secret: "approval-secret-1234".into(),
        short_code: "AB12CD34".into(),
        verification_phrase: "AMBER-DAWN".into(),
        device_display_name: "RockCast — test".into(),
        device_type: "windows".into(),
        expires_at: "2026-08-28T12:00:00Z".into(),
        status: "pending".into(),
    };
    assert_eq!(
        pairing.deep_link("https://rockplatform.win/"),
        "https://rockplatform.win/?code=AB12CD34#secret=approval-secret-1234"
    );
}

#[test]
fn completed_pairing_saves_only_to_secure_store_seam() {
    let body = r#"{"user_id":"must-not-be-used","device_id":"device","session_id":"session","access_token":"aaaaaaaaaaaaaaaa","access_expires_at":"2030-01-01T12:00:00Z","device_secret":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","account_display_name":"Alex's Rock account","device_display_name":"RockCast — test","device_type":"windows"}"#;
    let (url, server) = server(200, body);
    let store = Memory(Mutex::new(None));
    let pairing = PairingRequest {
        pairing_request_id: "request".into(),
        desktop_token: "desktop-token-1234".into(),
        approval_secret: "never-persist".into(),
        short_code: "AB12CD34".into(),
        verification_phrase: "AMBER-DAWN".into(),
        device_display_name: "RockCast — test".into(),
        device_type: "windows".into(),
        expires_at: "2026-08-28T12:00:00Z".into(),
        status: "pending".into(),
    };
    let account_client = client(url, store);
    let (profile, credentials) = account_client.complete_pairing_result(&pairing).unwrap();
    account_client
        .save_pairing_credentials(&credentials)
        .unwrap();
    assert_eq!(profile.device_id, "device");
    assert_eq!(profile.account_display_name, "Alex's Rock account");
    assert_eq!(profile.device_display_name, "RockCast — test");
    let request = server.join().unwrap();
    assert!(request.contains("/complete"));
    assert!(request.contains(r#"{"desktop_token":"desktop-token-1234"}"#));
    assert!(!request.contains("user_id"));
}

#[test]
fn completion_payload_rejects_removed_user_id() {
    assert!(
        serde_json::from_str::<PairingCompletionRequest<'_>>(
            r#"{"desktop_token":"desktop-token-1234","user_id":"forbidden"}"#
        )
        .is_err()
    );
}

#[test]
fn pairing_poll_waits_for_browser_approval() {
    let (url, server) = server(202, "{}");
    let pairing = PairingRequest {
        pairing_request_id: "request".into(),
        desktop_token: "desktop-token-1234".into(),
        approval_secret: "never-persist".into(),
        short_code: "AB12CD34".into(),
        verification_phrase: "AMBER-DAWN".into(),
        device_display_name: "RockCast — test".into(),
        device_type: "windows".into(),
        expires_at: "2026-08-28T12:00:00Z".into(),
        status: "pending".into(),
    };
    assert!(matches!(
        client(url, Memory(Mutex::new(None))).complete_pairing_result(&pairing),
        Err(PairingPoll::Pending)
    ));
    assert!(server.join().unwrap().contains("/complete"));
}

#[test]
fn pairing_poll_maps_terminal_server_states() {
    for (status, expected) in [
        (409, PairingPoll::DeviceLimit),
        (410, PairingPoll::Expired),
        (401, PairingPoll::Rejected),
    ] {
        let (url, server) = server(status, "{}");
        let pairing = PairingRequest {
            pairing_request_id: "request".into(),
            desktop_token: "desktop-token-1234".into(),
            approval_secret: "never-persist".into(),
            short_code: "AB12CD34".into(),
            verification_phrase: "AMBER-DAWN".into(),
            device_display_name: "RockCast — test".into(),
            device_type: "windows".into(),
            expires_at: "2026-08-28T12:00:00Z".into(),
            status: "pending".into(),
        };
        assert!(matches!(
            client(url, Memory(Mutex::new(None))).complete_pairing_result(&pairing),
            Err(actual) if actual == expected
        ));
        assert!(server.join().unwrap().contains("/complete"));
    }
}

#[test]
fn device_session_reuses_the_durable_credential() {
    let (url, server) = server(
        200,
        r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
    );
    let store = Memory(Mutex::new(Some(stored_credentials(
        "a".repeat(16).as_str(),
    ))));
    let client = client(url, store);
    let _guard = SESSION_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    client.issue_device_session_without_lock().unwrap();
    let credentials = client.store.load().unwrap().unwrap();
    assert_eq!(credentials.access_token(), "cccccccccccccccc");
    assert_eq!(credentials.device_secret(), "b".repeat(43));
    let request = server.join().unwrap();
    assert!(request.contains("/api/v1/auth/device-session"));
    assert!(request.contains(r#""device_id":"device""#));
    assert!(request.contains(r#""device_secret""#));
}

#[test]
fn voice_access_token_reuses_fresh_session_without_renewal() {
    let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
    let client = client("http://127.0.0.1:9".into(), store);
    assert_eq!(
        client.voice_access_token().unwrap().as_deref(),
        Some("fresh-access-token")
    );
}

#[test]
fn device_control_reuses_the_stored_pairing_identity() {
    let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
    let client = client("http://127.0.0.1:9".into(), store);
    assert_eq!(
        client
            .device_control_access_token(false)
            .unwrap()
            .as_deref(),
        Some("fresh-access-token")
    );
}

#[test]
fn device_control_can_force_the_existing_session_renewal() {
    let (url, server) = server(
        200,
        r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
    );
    let store = Memory(Mutex::new(Some(stored_credentials("fresh-access-token"))));
    let client = client(url, store);
    assert_eq!(
        client.device_control_access_token(true).unwrap().as_deref(),
        Some("cccccccccccccccc")
    );
    assert!(
        server
            .join()
            .unwrap()
            .contains("/api/v1/auth/device-session")
    );
}

#[test]
fn voice_access_token_renews_when_expiry_is_near() {
    let (url, server) = server(
        200,
        r#"{"access_token":"cccccccccccccccc","access_expires_at":"2030-01-01T12:00:00Z"}"#,
    );
    let near_expiry = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let store = Memory(Mutex::new(Some(
        NativeCredentials::new("device".into(), "b".repeat(43), "a".repeat(16), near_expiry)
            .unwrap(),
    )));
    let client = client(url, store);
    assert_eq!(
        client.voice_access_token().unwrap().as_deref(),
        Some("cccccccccccccccc")
    );
    assert!(
        server
            .join()
            .unwrap()
            .contains("/api/v1/auth/device-session")
    );
}

#[test]
fn unavailable_device_session_keeps_local_credentials() {
    let (url, server) = server(500, "{}");
    let store = Memory(Mutex::new(Some(stored_credentials(
        "a".repeat(16).as_str(),
    ))));
    let client = client(url, store);
    let _guard = SESSION_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        client.issue_device_session_without_lock().unwrap_err(),
        SessionError::Unavailable
    );
    assert!(client.store.load().unwrap().is_some());
    assert!(
        server
            .join()
            .unwrap()
            .contains("/api/v1/auth/device-session")
    );
}

#[test]
fn invalid_device_credential_clears_local_credentials() {
    let (url, server) = server(401, r#"{"code":"device_credential_invalid"}"#);
    let store = Memory(Mutex::new(Some(stored_credentials(
        "a".repeat(16).as_str(),
    ))));
    let client = client(url, store);
    let _guard = SESSION_MUTEX
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        client.issue_device_session_without_lock().unwrap_err(),
        SessionError::Unauthorized
    );
    assert!(client.store.load().unwrap().is_none());
    assert!(
        server
            .join()
            .unwrap()
            .contains("/api/v1/auth/device-session")
    );
}
