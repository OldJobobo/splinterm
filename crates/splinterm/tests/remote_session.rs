use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use splinterm::{automation::protocol_error, remote::RemoteCatalog, remote_session::RemoteSession};
use splinterm_core::SplintId;
use splinterm_protocol::{AccessScope, ControlMode, ErrorCode, Request, Response};

const FAKE_SSH: &str = include_str!("fixtures/fake_ssh.py");

fn test_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "splinterm-remote-session-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn fake_ssh(directory: &Path, mode: &str) -> PathBuf {
    let path = directory.join("ssh");
    let source = FAKE_SSH.replace(
        "mode = os.environ.get('SPLINTERM_FAKE_SSH_MODE', 'read-only')",
        &format!("mode = {mode:?}"),
    );
    fs::write(&path, source).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[tokio::test]
async fn one_fake_ssh_process_serves_multiple_automation_connections() {
    let directory = test_directory("multiplex");
    let ssh = fake_ssh(&directory, "read-only");
    let record = directory.join("argv.json");
    let count = directory.join("count");
    let catalog = RemoteCatalog::parse(
        "version = 1\n[remotes.test]\nhost = \"example.invalid\"\nuser = \"operator\"\nport = 2222\n",
        None,
    )
    .unwrap();
    let session = RemoteSession::connect_with_program(
        catalog.get("test").unwrap(),
        OsStr::new(ssh.as_os_str()),
    )
    .await
    .unwrap();
    let (first, second) = tokio::join!(session.connect_automation(), session.connect_automation());
    let mut first = first.unwrap();
    let mut second = second.unwrap();
    assert!(matches!(
        first.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    assert!(matches!(
        second.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(first);
    drop(second);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(fs::read_to_string(&count).unwrap(), "1");
    let arguments: Vec<String> = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(
        arguments.last().unwrap(),
        "/usr/bin/splinterm relay --graphical-stdio"
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "StrictHostKeyChecking=yes")
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "RemoteCommand=none")
    );
    assert!(
        arguments
            .iter()
            .any(|argument| argument == "example.invalid")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn stalled_relay_negotiation_times_out_and_reaps_the_child() {
    let directory = test_directory("timeout");
    let stalled = directory.join("ssh-stalled");
    fs::write(
        &stalled,
        "#!/usr/bin/python3\nimport os,time\nopen(os.path.join(os.path.dirname(os.path.realpath(__file__)), 'pid'), 'w').write(str(os.getpid()))\ntime.sleep(30)\n",
    )
    .unwrap();
    fs::set_permissions(&stalled, fs::Permissions::from_mode(0o700)).unwrap();
    let catalog = RemoteCatalog::parse(
        "version = 1\n[remotes.test]\nhost = \"example.invalid\"\n",
        None,
    )
    .unwrap();
    let started = std::time::Instant::now();
    let error = RemoteSession::connect_with_program_and_timeout(
        catalog.get("test").unwrap(),
        stalled.as_os_str(),
        std::time::Duration::from_millis(200),
    )
    .await
    .unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(
        error
            .downcast_ref::<splinterm::remote_session::RemoteFailure>()
            .unwrap()
            .kind(),
        splinterm::remote_session::RemoteFailureKind::TransportFailed
    );
    let pid = fs::read_to_string(directory.join("pid")).unwrap();
    assert!(!std::path::Path::new("/proc").join(pid).exists());
    fs::remove_dir_all(directory).unwrap();
}

fn profile() -> RemoteCatalog {
    RemoteCatalog::parse(
        "version = 1\n[remotes.test]\nhost = \"example.invalid\"\n",
        None,
    )
    .unwrap()
}

async fn session(directory: &Path, mode: &str) -> RemoteSession {
    let ssh = fake_ssh(directory, mode);
    let catalog = profile();
    RemoteSession::connect_with_program(catalog.get("test").unwrap(), ssh.as_os_str())
        .await
        .unwrap()
}

#[tokio::test]
async fn channel_admission_timeout_names_the_stalled_stage() {
    let directory = test_directory("channel-admission-timeout");
    let ssh = fake_ssh(&directory, "stall-channel-open");
    let catalog = RemoteCatalog::parse(
        "version = 1\n[remotes.test]\nhost = \"example.invalid\"\nconnect_timeout_seconds = 1\n",
        None,
    )
    .unwrap();
    let session = RemoteSession::connect_with_program_and_timeout(
        catalog.get("test").unwrap(),
        ssh.as_os_str(),
        std::time::Duration::from_secs(2),
    )
    .await
    .unwrap();

    let error = session.connect_automation().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<splinterm::remote_session::RemoteFailure>()
            .unwrap()
            .kind(),
        splinterm::remote_session::RemoteFailureKind::TransportFailed
    );
    assert!(
        error
            .to_string()
            .contains("logical channel admission timed out: graphical relay channel 1")
    );
    assert_eq!(
        session.terminal_failure().as_deref(),
        Some("graphical relay channel 1 admission was cancelled")
    );

    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let pid = fs::read_to_string(directory.join("pid-1")).unwrap();
    assert!(
        !std::path::Path::new("/proc").join(pid).exists(),
        "admission timeout did not reap the SSH fixture"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn private_daemon_hello_timeout_names_the_stalled_stage() {
    let directory = test_directory("private-hello-timeout");
    let ssh = fake_ssh(&directory, "stall-private-hello");
    let catalog = RemoteCatalog::parse(
        "version = 1\n[remotes.test]\nhost = \"example.invalid\"\nconnect_timeout_seconds = 1\n",
        None,
    )
    .unwrap();
    let session = RemoteSession::connect_with_program_and_timeout(
        catalog.get("test").unwrap(),
        ssh.as_os_str(),
        std::time::Duration::from_secs(2),
    )
    .await
    .unwrap();

    let error = session.connect_automation().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<splinterm::remote_session::RemoteFailure>()
            .unwrap()
            .kind(),
        splinterm::remote_session::RemoteFailureKind::TransportFailed
    );
    assert!(
        error
            .to_string()
            .contains("private daemon Hello timed out on logical channel 1")
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        fs::read_to_string(directory.join("events.jsonl")).unwrap(),
        "close:1\n"
    );

    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let pid = fs::read_to_string(directory.join("pid-1")).unwrap();
    assert!(
        !std::path::Path::new("/proc").join(pid).exists(),
        "private Hello timeout did not reap the SSH fixture"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn immediate_private_hello_eof_names_channel_and_preserves_later_admission() {
    let directory = test_directory("private-hello-eof");
    let session = session(&directory, "close-first-private-hello").await;

    let error = session.connect_automation().await.unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<splinterm::remote_session::RemoteFailure>()
            .unwrap()
            .kind(),
        splinterm::remote_session::RemoteFailureKind::TransportFailed
    );
    let diagnostic = error.to_string();
    assert!(
        diagnostic.contains("private daemon handshake failed on logical channel 1"),
        "unexpected diagnostic: {diagnostic}"
    );
    assert!(
        diagnostic.contains("early eof"),
        "unexpected diagnostic: {diagnostic}"
    );

    let mut later = session.connect_automation().await.unwrap();
    assert!(matches!(
        later.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(later);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn startup_channel_pattern_repeats_on_a_fresh_session_after_complete_teardown() {
    let directory = test_directory("startup-reconnect");
    let ssh = fake_ssh(&directory, "read-only");
    let catalog = profile();

    for expected_processes in 1..=2 {
        let session =
            RemoteSession::connect_with_program(catalog.get("test").unwrap(), ssh.as_os_str())
                .await
                .unwrap();
        let mut identity = session.connect_automation().await.unwrap();
        assert!(matches!(
            identity.request(Request::ListLairs).await.unwrap(),
            Response::Lairs { lairs, .. } if lairs.is_empty()
        ));
        drop(identity);

        let mut observation = session.connect_automation().await.unwrap();
        let mut control = session.connect_automation().await.unwrap();
        let mut topology = session.connect_automation().await.unwrap();
        for connection in [&mut observation, &mut control, &mut topology] {
            assert!(matches!(
                connection.request(Request::Ping).await.unwrap(),
                Response::Pong
            ));
        }
        drop(observation);
        drop(control);
        drop(topology);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(session);
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        assert_eq!(
            fs::read_to_string(directory.join("count")).unwrap(),
            expected_processes.to_string()
        );
        let pid = fs::read_to_string(directory.join(format!("pid-{expected_processes}"))).unwrap();
        assert!(
            !std::path::Path::new("/proc").join(pid).exists(),
            "fresh-session SSH fixture was not reaped"
        );
    }

    let events = fs::read_to_string(directory.join("events.jsonl")).unwrap();
    for channel_id in 1..=4 {
        assert_eq!(
            events
                .lines()
                .filter(|event| *event == format!("close:{channel_id}"))
                .count(),
            2,
            "channel {channel_id} was not closed once per session"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn interactive_requests_preserve_exact_controller_and_terminal_identity() {
    let directory = test_directory("interactive");
    let session = session(&directory, "interactive").await;
    let mut connection = session.connect_automation().await.unwrap();
    let splint_id = SplintId::new();
    let incarnation = 17;
    let controller_id = connection
        .acquire_control(
            splint_id,
            incarnation,
            vec![ControlMode::Input, ControlMode::Resize],
        )
        .await
        .unwrap();
    assert_eq!(controller_id, 91);
    for request in [
        Request::Input {
            controller_id,
            splint_id,
            incarnation,
            bytes: b"exact-input".to_vec(),
        },
        Request::Resize {
            controller_id,
            splint_id,
            incarnation,
            columns: 120,
            rows: 40,
            pixel_width: 960,
            pixel_height: 640,
        },
    ] {
        assert!(matches!(
            connection.request(request).await.unwrap(),
            Response::TerminalActionAcknowledged {
                splint_id: acknowledged,
                incarnation: 17,
                ..
            } if acknowledged == splint_id
        ));
    }
    connection.release_control(controller_id).await.unwrap();
    drop(connection);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let requests = fs::read_to_string(directory.join("requests.jsonl")).unwrap();
    assert!(requests.contains("\"type\":\"acquire_control\""));
    assert!(requests.contains("\"type\":\"input\""));
    assert!(requests.contains("\"type\":\"resize\""));
    assert!(requests.contains("\"type\":\"release_control\""));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn read_only_pane_access_succeeds_without_interactive_scopes() {
    let directory = test_directory("read-only-pane");
    let session = session(&directory, "read-only-pane").await;
    let mut connection = session.connect_automation().await.unwrap();
    let splint_id = SplintId::new();
    assert!(matches!(
        connection
            .request(Request::RequestAccess {
                splint_id,
                incarnation: 5,
                scopes: vec![AccessScope::Observe, AccessScope::Scrollback],
            })
            .await
            .unwrap(),
        Response::AccessGranted { grant, .. }
            if grant.splint_id == splint_id
                && grant.scopes == vec![AccessScope::Observe, AccessScope::Scrollback]
    ));
    let response = connection
        .request(Request::Attach {
            splint_id,
            incarnation: Some(5),
            scrollback_rows: 0,
        })
        .await
        .unwrap();
    let Response::Attached {
        subscription_id: 73,
        provenance,
        snapshot,
    } = response
    else {
        panic!("fake relay did not attach the read-only observer");
    };
    assert_eq!(provenance.splint_id, splint_id);
    assert_eq!(provenance.incarnation, 5);
    assert_eq!(snapshot.splint_id, splint_id);
    assert_eq!(snapshot.incarnation, 5);
    assert!(snapshot.images.is_none());
    snapshot.validate().unwrap();

    let error = connection
        .acquire_control(splint_id, 5, vec![ControlMode::Input, ControlMode::Resize])
        .await
        .unwrap_err();
    assert_eq!(
        protocol_error(&error).unwrap().code,
        ErrorCode::Unauthorized
    );
    assert!(matches!(
        connection.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(connection);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn interactive_policy_denial_is_returned_without_losing_the_session() {
    let directory = test_directory("denied-interactive");
    let session = session(&directory, "denied-interactive").await;
    let mut connection = session.connect_automation().await.unwrap();
    let error = connection
        .acquire_control(
            SplintId::new(),
            9,
            vec![ControlMode::Input, ControlMode::Resize],
        )
        .await
        .unwrap_err();
    assert_eq!(
        protocol_error(&error).unwrap().code,
        ErrorCode::Unauthorized
    );
    assert!(matches!(
        connection.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(connection);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn mismatched_terminal_acknowledgement_cannot_match_the_requested_splint() {
    let directory = test_directory("mismatched-identity");
    let session = session(&directory, "mismatched-identity").await;
    let mut connection = session.connect_automation().await.unwrap();
    let splint_id = SplintId::new();
    let response = connection
        .request(Request::Input {
            controller_id: 91,
            splint_id,
            incarnation: 23,
            bytes: vec![1],
        })
        .await
        .unwrap();
    assert!(matches!(
        response,
        Response::TerminalActionAcknowledged {
            splint_id: acknowledged,
            incarnation: 23,
            ..
        } if acknowledged != splint_id
    ));
    drop(connection);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn one_logical_channel_loss_does_not_retarget_or_close_another_channel() {
    let directory = test_directory("channel-loss");
    let session = session(&directory, "close-first").await;
    let mut first = session.connect_automation().await.unwrap();
    let mut second = session.connect_automation().await.unwrap();
    let splint_id = SplintId::new();
    assert!(matches!(
        first
            .request(Request::SubscribeControl {
                splint_id,
                incarnation: 31,
            })
            .await
            .unwrap(),
        Response::ControlSubscribed {
            subscription_id: 77,
            status,
        } if status.splint_id == splint_id && status.incarnation == 31
    ));
    assert_eq!(
        first
            .acquire_control(splint_id, 31, vec![ControlMode::Input, ControlMode::Resize],)
            .await
            .unwrap(),
        91
    );
    assert!(first.request(Request::Ping).await.is_err());
    assert!(matches!(
        second.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(first);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(matches!(
        second.request(Request::Ping).await.unwrap(),
        Response::Pong
    ));
    drop(second);
    drop(session);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn fake_ssh_script_has_no_shell_interpolated_profile_values() {
    assert!(!FAKE_SSH.contains("eval"));
    assert!(!FAKE_SSH.contains("shell=True"));
    assert!(FAKE_SSH.starts_with("#!/usr/bin/python3\n"));
}
