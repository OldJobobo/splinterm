//! The old daemon must get back to exactly one thread once its Tokio runtime
//! and its systemd D-Bus connection are gone, or upgrade handoff can never
//! take the process guard (Plan 0061 M3.0).
//!
//! This target uses `harness = false`: libtest runs every test on a spawned
//! thread, so no test inside it can ever observe one thread. Each stage runs
//! in a fresh child copy of this binary that starts with only its main
//! thread. Harness arguments such as `--test-threads=1` are accepted and
//! ignored; `--list` prints the two stages the way libtest lists tests.
//!
//! The `runtime` stage always runs. The `session-bus` stage runs under
//! `dbus-run-session` with a private bus. When no private bus can be started
//! it is skipped with a message, unless `SPLINTERD_REQUIRE_SESSION_BUS=1`
//! makes that a failure.

use std::{
    env, fs,
    process::{Command, ExitCode},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use splinterd::{
    dbus_worker::{BLOCKING_THREAD_NAME, DbusWorker, DbusWorkerError, WORKER_THREAD_NAME},
    handoff_coordinator::{HandoffCoordinatorError, HandoffProcessGuard},
};
use zbus::{fdo::DBusProxy, proxy::CacheProperties};

const STAGE: &str = "SPLINTERD_RUNTIME_STOP_STAGE";
const REQUIRE_SESSION_BUS: &str = "SPLINTERD_REQUIRE_SESSION_BUS";
const METHOD_TIMEOUT: Duration = Duration::from_millis(500);
/// Bound for joined threads to leave `/proc/self/task`.
const SETTLE: Duration = Duration::from_secs(2);
/// How long a negative control keeps retrying before it must report failure.
const BUSY: Duration = Duration::from_millis(100);
/// Shutdown grace for the stuck-request check.
const STUCK_GRACE: Duration = Duration::from_millis(200);
const STAGES: [&str; 2] = ["runtime", "session-bus"];

fn main() -> ExitCode {
    if env::args().skip(1).any(|argument| argument == "--list") {
        for stage in STAGES {
            println!("handoff_runtime_stop::{stage}: test");
        }
        return ExitCode::SUCCESS;
    }
    match env::var(STAGE).ok().as_deref() {
        Some("runtime") => runtime_stage(),
        Some("session-bus") => session_bus_stage(),
        Some(stage) => panic!("unknown stage {stage:?}"),
        None => run_stages(),
    }
    ExitCode::SUCCESS
}

fn run_stages() {
    let executable = env::current_exe().unwrap();

    let mut runtime = Command::new(&executable);
    // Point the failed-connection check at a bus that cannot exist.
    runtime.env(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/splinterd-handoff-runtime-stop/bus",
    );
    run_stage("runtime", runtime);

    match private_session_bus() {
        Ok(()) => {
            let mut session_bus = Command::new("dbus-run-session");
            session_bus.arg("--").arg(&executable);
            run_stage("session-bus", session_bus);
        }
        Err(reason) if env::var_os(REQUIRE_SESSION_BUS).is_some_and(|value| value == "1") => {
            panic!("session-bus stage is required but cannot run: {reason}");
        }
        Err(reason) => eprintln!("handoff_runtime_stop: skipping session-bus stage: {reason}"),
    }
    println!("handoff_runtime_stop: ok");
}

fn run_stage(stage: &str, mut command: Command) {
    let output = command.env(STAGE, stage).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{stage} stage failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    println!("handoff_runtime_stop: {stage} stage passed");
}

/// Checks that `dbus-run-session` exists and can start a private bus.
fn private_session_bus() -> Result<(), String> {
    match Command::new("dbus-run-session")
        .args(["--", "true"])
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => Err(format!(
            "dbus-run-session could not start a private bus ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("dbus-run-session is not installed".to_owned())
        }
        Err(error) => Err(format!("cannot run dbus-run-session: {error}")),
    }
}

fn thread_count() -> usize {
    fs::read_dir("/proc/self/task").unwrap().count()
}

fn assert_starts_single_threaded() {
    assert_eq!(
        thread_count(),
        1,
        "stage must start with only its main thread"
    );
}

/// A daemon-shaped runtime: two workers and a used blocking pool.
fn daemon_runtime() -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (worker, blocking) = runtime.block_on(async {
        let worker = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            thread::current().id()
        });
        let blocking = tokio::task::spawn_blocking(|| thread::current().id());
        (worker.await.unwrap(), blocking.await.unwrap())
    });
    assert_ne!(worker, thread::current().id());
    assert_ne!(blocking, thread::current().id());
    runtime
}

/// The guard must be refused, and keep being refused until the deadline.
fn assert_guard_refused(context: &str) -> usize {
    match HandoffProcessGuard::acquire_until(Instant::now() + BUSY) {
        Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed }) => {
            assert!(observed >= 2, "{context}: observed {observed} threads");
            observed
        }
        Err(error) => panic!("{context}: unexpected guard error: {error}"),
        Ok(_) => panic!("{context}: guard acquired with other threads alive"),
    }
}

fn assert_guard_acquired(context: &str) {
    match HandoffProcessGuard::acquire_until(Instant::now() + SETTLE) {
        Ok(guard) => drop(guard),
        Err(error) => panic!(
            "{context}: guard refused ({error}); other threads: {:?}",
            other_thread_names()
        ),
    }
}

/// Names of every live thread except the main thread.
fn other_thread_names() -> Vec<String> {
    let main = std::process::id().to_string();
    fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|task| task.file_name() != main.as_str())
        .map(|task| {
            fs::read_to_string(task.path().join("comm"))
                .map_or_else(|_| "?".to_owned(), |name| name.trim().to_owned())
        })
        .collect()
}

/// Negative control: one deliberately leaked thread must block the guard.
fn assert_leaked_thread_blocks_guard() {
    let (started, ready) = mpsc::channel();
    // Detached and never joined; it parks until the process exits.
    drop(thread::spawn(move || {
        started.send(()).unwrap();
        loop {
            thread::park();
        }
    }));
    ready.recv().unwrap();
    assert_eq!(assert_guard_refused("leaked thread"), 2);
}

fn runtime_stage() {
    assert_starts_single_threaded();

    // A failed systemd connection must not leave its worker thread behind.
    assert!(matches!(
        DbusWorker::connect_session(METHOD_TIMEOUT),
        Err(DbusWorkerError::Connect(_))
    ));
    assert_guard_acquired("after a failed D-Bus connection");

    let runtime = daemon_runtime();
    assert!(thread_count() >= 4, "workers and blocking pool are running");
    assert_guard_refused("while the runtime is alive");
    drop(runtime);
    assert_guard_acquired("after dropping the runtime");

    assert_leaked_thread_blocks_guard();
}

fn session_bus_stage() {
    assert_starts_single_threaded();
    assert!(
        env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some(),
        "dbus-run-session provides a private bus address"
    );

    let runtime = daemon_runtime();
    // The daemon connects from its async startup and places workloads from
    // `spawn_blocking`; exercise both contexts.
    let bus = runtime
        .block_on(async { DbusWorker::connect_session(METHOD_TIMEOUT) })
        .map(Arc::new)
        .expect("connect to the private session bus");
    let worker = Arc::clone(&bus);
    let bus_id = runtime
        .block_on(runtime.spawn_blocking(move || {
            worker.call(|connection| async move {
                let proxy = DBusProxy::builder(&connection)
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await?;
                Ok(proxy.get_id().await?.to_string())
            })
        }))
        .unwrap()
        .expect("worker serves the request")
        .expect("GetId succeeds");
    assert_ne!(bus_id, "");
    let missing = bus
        .call(|connection| async move {
            connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "SplintermNoSuchMethod",
                    &(),
                )
                .await
                .map(drop)
        })
        .expect("worker serves the request");
    assert!(matches!(missing, Err(zbus::Error::MethodError(..))));

    drop(runtime);
    assert_guard_refused("while the D-Bus worker is alive");
    // The daemon runtime is gone; only the D-Bus worker's own threads remain.
    let others = other_thread_names();
    assert!(
        others.iter().any(|name| name == WORKER_THREAD_NAME)
            && others
                .iter()
                .all(|name| name == WORKER_THREAD_NAME || name == BLOCKING_THREAD_NAME),
        "threads besides main: {others:?}"
    );

    // A request that never completes must not hold shutdown: it is aborted
    // after the grace period and its caller sees the worker closed.
    let (entered, stuck_running) = mpsc::channel();
    let stuck_worker = Arc::clone(&bus);
    let stuck = thread::spawn(move || {
        stuck_worker.call(move |_| async move {
            entered.send(()).unwrap();
            std::future::pending::<zbus::Result<()>>().await
        })
    });
    stuck_running.recv().unwrap();
    let started = Instant::now();
    // A second caller that arrives mid-shutdown must wait for the same join
    // rather than return while the worker is still running.
    let concurrent_worker = Arc::clone(&bus);
    let concurrent = thread::spawn(move || {
        thread::sleep(STUCK_GRACE / 4);
        let result = concurrent_worker.shutdown();
        (result, started.elapsed())
    });
    bus.shutdown_within(STUCK_GRACE).unwrap();
    let elapsed = started.elapsed();
    let (concurrent_result, concurrent_elapsed) = concurrent.join().unwrap();
    concurrent_result.unwrap();
    assert!(
        concurrent_elapsed >= STUCK_GRACE,
        "a concurrent shutdown returned before the join: {concurrent_elapsed:?}"
    );
    assert!(
        elapsed >= STUCK_GRACE,
        "shutdown returned before the grace period: {elapsed:?}"
    );
    // Drain plus a prompt abort and close: well under three grace periods.
    assert!(
        elapsed < STUCK_GRACE * 3,
        "shutdown exceeded its bound: {elapsed:?}"
    );
    assert!(matches!(
        stuck.join().unwrap(),
        Err(DbusWorkerError::Closed)
    ));
    assert!(matches!(
        bus.call(|_| async { Ok(()) }),
        Err(DbusWorkerError::Closed)
    ));
    bus.shutdown().unwrap();
    assert_guard_acquired("after D-Bus worker shutdown");
    drop(bus);

    assert_leaked_thread_blocks_guard();
}
