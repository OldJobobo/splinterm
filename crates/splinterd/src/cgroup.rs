use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use splinterd::ProcessPlacement;
use splinterm_core::{DojoId, SplintId};
use splinterm_pty::LinuxPtyIdentity;
use thiserror::Error;
use zbus::{
    blocking::{Connection, connection},
    zvariant::{OwnedObjectPath, OwnedValue, Str, Value},
};

const AGGREGATE_TASKS_MAX: u64 = 2_048;
const AGGREGATE_MEMORY_HIGH_PERCENT: u32 = 75;
const METHOD_TIMEOUT: Duration = Duration::from_millis(500);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(2);
const VERIFY_INTERVAL: Duration = Duration::from_millis(25);
static NEXT_PROBE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkloadResourcePolicy {
    pub(crate) dojo_tasks_max: u64,
    pub(crate) splint_tasks_max: u64,
    pub(crate) dojo_memory_high_percent: u32,
    pub(crate) splint_memory_high_percent: u32,
}

impl Default for WorkloadResourcePolicy {
    fn default() -> Self {
        Self {
            dojo_tasks_max: 1_024,
            splint_tasks_max: 512,
            dojo_memory_high_percent: 50,
            splint_memory_high_percent: 25,
        }
    }
}

impl WorkloadResourcePolicy {
    fn validate(self) -> Result<Self, WorkloadUnitError> {
        if self.splint_tasks_max == 0
            || self.splint_tasks_max >= self.dojo_tasks_max
            || self.dojo_tasks_max >= AGGREGATE_TASKS_MAX
            || self.splint_memory_high_percent == 0
            || self.splint_memory_high_percent >= self.dojo_memory_high_percent
            || self.dojo_memory_high_percent >= AGGREGATE_MEMORY_HIGH_PERCENT
        {
            return Err(WorkloadUnitError::InvalidPolicy);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkloadUnitNames {
    pub(crate) dojo_slice: String,
    pub(crate) splint_scope: String,
}

impl WorkloadUnitNames {
    fn new(dojo_id: DojoId, splint_id: SplintId, incarnation: u64) -> Self {
        Self {
            dojo_slice: format!("app-splinterm-dojo{}.slice", compact_id(&dojo_id)),
            splint_scope: format!(
                "splinterm-splint{}-i{incarnation}.scope",
                compact_id(&splint_id)
            ),
        }
    }
}

fn compact_id(id: &impl ToString) -> String {
    id.to_string()
        .chars()
        .filter(|character| *character != '-')
        .collect()
}

fn percent_scale(percent: u32) -> u32 {
    let scaled = (u64::from(u32::MAX) * u64::from(percent) + 50) / 100;
    u32::try_from(scaled).expect("bounded percentage scale fits u32")
}

#[derive(Debug, Error)]
pub(crate) enum WorkloadUnitError {
    #[error("workload resource policy is invalid")]
    InvalidPolicy,
    #[error("systemd user-manager connection failed")]
    Connection(#[source] zbus::Error),
    #[error("systemd transient unit request failed")]
    Request(#[source] zbus::Error),
    #[error("systemd transient unit property is invalid")]
    Property(#[source] zbus::zvariant::Error),
    #[error("systemd did not place the PTY child in the expected scope")]
    IdentityMismatch,
    #[error("systemd workload unit did not stop within the deadline")]
    StopTimeout,
    #[error("workload unit state lock is unavailable")]
    State,
}

trait SystemdUnitAdapter: Send + Sync {
    fn probe(&self) -> Result<(), WorkloadUnitError>;

    fn start_dojo_slice(
        &self,
        name: &str,
        policy: WorkloadResourcePolicy,
    ) -> Result<(), WorkloadUnitError>;

    fn start_splint_scope(
        &self,
        names: &WorkloadUnitNames,
        child_pid: u32,
        policy: WorkloadResourcePolicy,
    ) -> Result<(), WorkloadUnitError>;

    fn stop_unit(&self, name: &str) -> Result<(), WorkloadUnitError>;
}

struct SystemdUserManager {
    connection: Connection,
    owner_unit: Option<String>,
}

impl SystemdUserManager {
    fn connect(owner_unit: Option<&str>) -> Result<Self, WorkloadUnitError> {
        let connection = connection::Builder::session()
            .map_err(WorkloadUnitError::Connection)?
            .method_timeout(METHOD_TIMEOUT)
            .build()
            .map_err(WorkloadUnitError::Connection)?;
        Ok(Self {
            connection,
            owner_unit: owner_unit.map(str::to_owned),
        })
    }

    fn manager(&self) -> Result<SystemdManagerProxyBlocking<'_>, WorkloadUnitError> {
        SystemdManagerProxyBlocking::new(&self.connection).map_err(WorkloadUnitError::Request)
    }

    fn start_transient_unit(
        &self,
        name: &str,
        properties: Vec<(&str, OwnedValue)>,
    ) -> Result<(), WorkloadUnitError> {
        self.manager()?
            .start_transient_unit(name, "fail", properties, Vec::new())
            .map(|_| ())
            .map_err(WorkloadUnitError::Request)
    }

    fn unit_path(&self, name: &str) -> Result<OwnedObjectPath, WorkloadUnitError> {
        self.manager()?
            .get_unit(name)
            .map_err(WorkloadUnitError::Request)
    }

    fn pid_unit_path(&self, pid: u32) -> Result<OwnedObjectPath, WorkloadUnitError> {
        self.manager()?
            .get_unit_by_pid(pid)
            .map_err(WorkloadUnitError::Request)
    }
}

impl SystemdUnitAdapter for SystemdUserManager {
    fn probe(&self) -> Result<(), WorkloadUnitError> {
        let probe_id = NEXT_PROBE_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("app-splinterm-probe{}i{probe_id}.slice", std::process::id());
        let mut properties = vec![
            string_property("Description", "Splinterm workload capability probe"),
            u64_property("TasksMax", 1),
            u32_property("MemoryHighScale", percent_scale(1)),
            string_property("CollectMode", "inactive-or-failed"),
        ];
        if let Some(owner_unit) = &self.owner_unit {
            properties.push(vec_string_property("PartOf", vec![owner_unit.clone()])?);
        }
        if let Err(error) = self.start_transient_unit(&name, properties) {
            if !is_unit_exists(&error) {
                let _ = self.stop_unit(&name);
            }
            return Err(error);
        }
        self.stop_unit(&name)
    }

    fn start_dojo_slice(
        &self,
        name: &str,
        policy: WorkloadResourcePolicy,
    ) -> Result<(), WorkloadUnitError> {
        let mut properties = vec![
            string_property("Description", "Splinterm Dojo workload"),
            u64_property("TasksMax", policy.dojo_tasks_max),
            u32_property(
                "MemoryHighScale",
                percent_scale(policy.dojo_memory_high_percent),
            ),
            string_property("CollectMode", "inactive-or-failed"),
        ];
        if let Some(owner_unit) = &self.owner_unit {
            properties.push(vec_string_property("PartOf", vec![owner_unit.clone()])?);
        }
        self.start_transient_unit(name, properties)
    }

    fn start_splint_scope(
        &self,
        names: &WorkloadUnitNames,
        child_pid: u32,
        policy: WorkloadResourcePolicy,
    ) -> Result<(), WorkloadUnitError> {
        let result = self.start_transient_unit(
            &names.splint_scope,
            vec![
                string_property("Description", "Splinterm Splint workload"),
                string_property("Slice", &names.dojo_slice),
                vec_u32_property("PIDs", vec![child_pid])?,
                u64_property("TasksMax", policy.splint_tasks_max),
                u32_property(
                    "MemoryHighScale",
                    percent_scale(policy.splint_memory_high_percent),
                ),
                string_property("CollectMode", "inactive-or-failed"),
            ],
        );
        if let Err(error) = result {
            if !is_unit_exists(&error) {
                let _ = self.stop_unit(&names.splint_scope);
            }
            return Err(error);
        }

        let expected = self.unit_path(&names.splint_scope)?;
        let deadline = Instant::now() + VERIFY_TIMEOUT;
        let mut last_error = None;
        while Instant::now() < deadline {
            match self.pid_unit_path(child_pid) {
                Ok(actual) if actual == expected => return Ok(()),
                Ok(_) => last_error = None,
                Err(error) => last_error = Some(error),
            }
            thread::sleep(VERIFY_INTERVAL);
        }
        let _ = self.stop_unit(&names.splint_scope);
        Err(last_error.unwrap_or(WorkloadUnitError::IdentityMismatch))
    }

    fn stop_unit(&self, name: &str) -> Result<(), WorkloadUnitError> {
        let request_error = match self.manager()?.stop_unit(name, "replace") {
            Ok(_) => None,
            Err(error) if is_no_such_unit(&error) => return Ok(()),
            Err(error) => Some(error),
        };
        let deadline = Instant::now() + VERIFY_TIMEOUT;
        while Instant::now() < deadline {
            match self.unit_path(name) {
                Err(WorkloadUnitError::Request(error)) if is_no_such_unit(&error) => return Ok(()),
                Err(error) => return Err(error),
                Ok(_) => thread::sleep(VERIFY_INTERVAL),
            }
        }
        request_error.map_or(Err(WorkloadUnitError::StopTimeout), |error| {
            Err(WorkloadUnitError::Request(error))
        })
    }
}

fn is_unit_exists(error: &WorkloadUnitError) -> bool {
    matches!(
        error,
        WorkloadUnitError::Request(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.systemd1.UnitExists"
    )
}

fn is_no_such_unit(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.systemd1.NoSuchUnit"
    )
}

fn string_property<'a>(name: &'a str, value: &str) -> (&'a str, OwnedValue) {
    (name, OwnedValue::from(Str::from(value.to_owned())))
}

fn u64_property(name: &str, value: u64) -> (&str, OwnedValue) {
    (name, OwnedValue::from(value))
}

fn u32_property(name: &str, value: u32) -> (&str, OwnedValue) {
    (name, OwnedValue::from(value))
}

fn vec_u32_property(name: &str, value: Vec<u32>) -> Result<(&str, OwnedValue), WorkloadUnitError> {
    Ok((
        name,
        OwnedValue::try_from(Value::from(value)).map_err(WorkloadUnitError::Property)?,
    ))
}

fn vec_string_property(
    name: &str,
    value: Vec<String>,
) -> Result<(&str, OwnedValue), WorkloadUnitError> {
    Ok((
        name,
        OwnedValue::try_from(Value::from(value)).map_err(WorkloadUnitError::Property)?,
    ))
}

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: Vec<(&str, OwnedValue)>,
        auxiliary: Vec<(&str, Vec<(&str, OwnedValue)>)>,
    ) -> zbus::Result<OwnedObjectPath>;

    fn stop_unit(&self, name: &str, mode: &str) -> zbus::Result<OwnedObjectPath>;

    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;

    #[zbus(name = "GetUnitByPID")]
    fn get_unit_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;
}

#[derive(Default)]
struct WorkloadUnitState {
    dojo_references: HashMap<DojoId, usize>,
}

#[derive(Clone)]
pub(crate) struct WorkloadUnitManager {
    adapter: Arc<dyn SystemdUnitAdapter>,
    policy: WorkloadResourcePolicy,
    state: Arc<Mutex<WorkloadUnitState>>,
}

impl WorkloadUnitManager {
    pub(crate) fn connect(
        policy: WorkloadResourcePolicy,
        owner_unit: Option<&str>,
    ) -> Result<Self, WorkloadUnitError> {
        Self::new(Arc::new(SystemdUserManager::connect(owner_unit)?), policy)
    }

    fn new(
        adapter: Arc<dyn SystemdUnitAdapter>,
        policy: WorkloadResourcePolicy,
    ) -> Result<Self, WorkloadUnitError> {
        let policy = policy.validate()?;
        adapter.probe()?;
        Ok(Self {
            adapter,
            policy,
            state: Arc::new(Mutex::new(WorkloadUnitState::default())),
        })
    }

    pub(crate) fn prepare(
        &self,
        dojo_id: DojoId,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<PreparedWorkloadScope, WorkloadUnitError> {
        let names = WorkloadUnitNames::new(dojo_id, splint_id, incarnation);
        let mut state = self.state.lock().map_err(|_| WorkloadUnitError::State)?;
        let known = state.dojo_references.contains_key(&dojo_id);
        let references = state.dojo_references.entry(dojo_id).or_default();
        if *references == 0 {
            if known {
                self.adapter.stop_unit(&names.dojo_slice)?;
            }
            if let Err(error) = self
                .adapter
                .start_dojo_slice(&names.dojo_slice, self.policy)
            {
                if !is_unit_exists(&error) {
                    let _ = self.adapter.stop_unit(&names.dojo_slice);
                }
                return Err(error);
            }
        }
        *references = references
            .checked_add(1)
            .expect("live Splint bound prevents Dojo reference overflow");
        drop(state);

        Ok(PreparedWorkloadScope {
            manager: self.clone(),
            dojo_id,
            names,
            committed: false,
        })
    }

    fn release_dojo(&self, dojo_id: DojoId, dojo_slice: &str) -> Result<(), WorkloadUnitError> {
        let mut state = self.state.lock().map_err(|_| WorkloadUnitError::State)?;
        let Some(references) = state.dojo_references.get_mut(&dojo_id) else {
            return Ok(());
        };
        if *references == 0 {
            return Err(WorkloadUnitError::State);
        }
        *references -= 1;
        if *references == 0 {
            self.adapter.stop_unit(dojo_slice)?;
            state.dojo_references.remove(&dojo_id);
        }
        Ok(())
    }
}

pub(crate) struct PreparedWorkloadScope {
    manager: WorkloadUnitManager,
    dojo_id: DojoId,
    names: WorkloadUnitNames,
    committed: bool,
}

impl PreparedWorkloadScope {
    pub(crate) fn place(mut self, identity: LinuxPtyIdentity) -> io::Result<ActiveWorkloadScope> {
        self.manager
            .adapter
            .start_splint_scope(&self.names, identity.child_pid(), self.manager.policy)
            .map_err(io::Error::other)?;
        self.committed = true;
        Ok(ActiveWorkloadScope {
            manager: self.manager.clone(),
            dojo_id: self.dojo_id,
            names: self.names.clone(),
            released: false,
        })
    }
}

impl Drop for PreparedWorkloadScope {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self
                .manager
                .release_dojo(self.dojo_id, &self.names.dojo_slice);
        }
    }
}

pub(crate) struct ActiveWorkloadScope {
    manager: WorkloadUnitManager,
    dojo_id: DojoId,
    names: WorkloadUnitNames,
    released: bool,
}

impl ActiveWorkloadScope {
    pub(crate) fn release(mut self) -> Result<(), WorkloadUnitError> {
        let scope_result = self.manager.adapter.stop_unit(&self.names.splint_scope);
        let dojo_result = self
            .manager
            .release_dojo(self.dojo_id, &self.names.dojo_slice);
        self.released = true;
        scope_result.and(dojo_result)
    }
}

impl Drop for ActiveWorkloadScope {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.manager.adapter.stop_unit(&self.names.splint_scope);
            let _ = self
                .manager
                .release_dojo(self.dojo_id, &self.names.dojo_slice);
        }
    }
}

impl ProcessPlacement for ActiveWorkloadScope {
    fn release(self) {
        let _ = ActiveWorkloadScope::release(self);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Condvar, mpsc};

    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum AdapterCall {
        StartDojo(String),
        StartSplint(String, String, u32),
        Stop(String),
    }

    #[derive(Default)]
    struct StopGate {
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }

    impl StopGate {
        fn wait_until_entered(&self) {
            let mut state = self.state.lock().unwrap();
            while !state.0 {
                state = self.changed.wait(state).unwrap();
            }
        }

        fn release(&self) {
            let mut state = self.state.lock().unwrap();
            state.1 = true;
            self.changed.notify_all();
        }
    }

    #[derive(Default)]
    struct FakeAdapter {
        calls: Mutex<Vec<AdapterCall>>,
        fail_probe: Mutex<bool>,
        fail_scope: Mutex<bool>,
        stop_gate: Mutex<Option<Arc<StopGate>>>,
    }

    impl FakeAdapter {
        fn calls(&self) -> Vec<AdapterCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SystemdUnitAdapter for FakeAdapter {
        fn probe(&self) -> Result<(), WorkloadUnitError> {
            if *self.fail_probe.lock().unwrap() {
                Err(WorkloadUnitError::IdentityMismatch)
            } else {
                Ok(())
            }
        }

        fn start_dojo_slice(
            &self,
            name: &str,
            _policy: WorkloadResourcePolicy,
        ) -> Result<(), WorkloadUnitError> {
            self.calls
                .lock()
                .unwrap()
                .push(AdapterCall::StartDojo(name.to_owned()));
            Ok(())
        }

        fn start_splint_scope(
            &self,
            names: &WorkloadUnitNames,
            child_pid: u32,
            _policy: WorkloadResourcePolicy,
        ) -> Result<(), WorkloadUnitError> {
            self.calls.lock().unwrap().push(AdapterCall::StartSplint(
                names.dojo_slice.clone(),
                names.splint_scope.clone(),
                child_pid,
            ));
            if *self.fail_scope.lock().unwrap() {
                self.calls
                    .lock()
                    .unwrap()
                    .push(AdapterCall::Stop(names.splint_scope.clone()));
                Err(WorkloadUnitError::IdentityMismatch)
            } else {
                Ok(())
            }
        }

        fn stop_unit(&self, name: &str) -> Result<(), WorkloadUnitError> {
            self.calls
                .lock()
                .unwrap()
                .push(AdapterCall::Stop(name.to_owned()));
            let gate = self.stop_gate.lock().unwrap().clone();
            if is_slice_unit(name)
                && let Some(gate) = gate
            {
                let mut state = gate.state.lock().unwrap();
                state.0 = true;
                gate.changed.notify_all();
                while !state.1 {
                    state = gate.changed.wait(state).unwrap();
                }
            }
            Ok(())
        }
    }

    fn identity(pid: u32) -> LinuxPtyIdentity {
        LinuxPtyIdentity::from_raw(pid, pid, pid).unwrap()
    }

    fn is_slice_unit(name: &str) -> bool {
        name.strip_suffix(".slice").is_some()
    }

    fn assert_systemd_unit_absent(manager: &SystemdUserManager, name: &str) {
        assert!(matches!(
            manager.unit_path(name),
            Err(WorkloadUnitError::Request(error)) if is_no_such_unit(&error)
        ));
    }

    #[test]
    fn manager_creation_requires_a_successful_capability_probe() {
        let adapter = Arc::new(FakeAdapter::default());
        *adapter.fail_probe.lock().unwrap() = true;
        assert!(WorkloadUnitManager::new(adapter, WorkloadResourcePolicy::default()).is_err());
    }

    #[test]
    fn names_are_stable_unit_safe_and_incarnation_specific() {
        let dojo_id = DojoId::new();
        let splint_id = SplintId::new();
        let first = WorkloadUnitNames::new(dojo_id, splint_id, 7);
        let second = WorkloadUnitNames::new(dojo_id, splint_id, 8);

        assert!(first.dojo_slice.starts_with("app-splinterm-dojo"));
        assert!(is_slice_unit(&first.dojo_slice));
        assert!(first.splint_scope.starts_with("splinterm-splint"));
        assert!(first.splint_scope.ends_with("-i7.scope"));
        assert!(!first.dojo_slice.contains(&dojo_id.to_string()));
        assert_ne!(first.splint_scope, second.splint_scope);
        assert!(
            first
                .dojo_slice
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-".contains(character))
        );
    }

    #[test]
    fn resource_policy_requires_nested_ordering() {
        assert!(WorkloadResourcePolicy::default().validate().is_ok());
        assert!(
            WorkloadResourcePolicy {
                splint_tasks_max: 1_024,
                ..WorkloadResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            WorkloadResourcePolicy {
                splint_memory_high_percent: 50,
                ..WorkloadResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
        assert_eq!(percent_scale(25), 1_073_741_824);
        assert_eq!(percent_scale(50), 2_147_483_648);
    }

    #[test]
    fn scopes_share_one_dojo_reference_and_last_release_stops_slice() {
        let adapter = Arc::new(FakeAdapter::default());
        let manager =
            WorkloadUnitManager::new(adapter.clone(), WorkloadResourcePolicy::default()).unwrap();
        let dojo_id = DojoId::new();
        let first_prepared = manager.prepare(dojo_id, SplintId::new(), 1).unwrap();
        let second_prepared = manager.prepare(dojo_id, SplintId::new(), 2).unwrap();
        let first = first_prepared.place(identity(101)).unwrap();
        let second = second_prepared.place(identity(102)).unwrap();

        assert_eq!(
            adapter
                .calls()
                .iter()
                .filter(|call| matches!(call, AdapterCall::StartDojo(_)))
                .count(),
            1
        );
        first.release().unwrap();
        assert_eq!(
            adapter
                .calls()
                .iter()
                .filter(|call| matches!(call, AdapterCall::Stop(name) if is_slice_unit(name)))
                .count(),
            0
        );
        second.release().unwrap();
        assert_eq!(
            adapter
                .calls()
                .iter()
                .filter(|call| matches!(call, AdapterCall::Stop(name) if is_slice_unit(name)))
                .count(),
            1
        );
    }

    #[test]
    fn immediate_reprepare_waits_for_completed_last_slice_stop() {
        let adapter = Arc::new(FakeAdapter::default());
        let manager =
            WorkloadUnitManager::new(adapter.clone(), WorkloadResourcePolicy::default()).unwrap();
        let dojo_id = DojoId::new();
        let active = manager
            .prepare(dojo_id, SplintId::new(), 1)
            .unwrap()
            .place(identity(104))
            .unwrap();
        let gate = Arc::new(StopGate::default());
        *adapter.stop_gate.lock().unwrap() = Some(Arc::clone(&gate));

        let release = std::thread::spawn(move || active.release().unwrap());
        gate.wait_until_entered();
        let (prepared, received) = mpsc::channel();
        let preparing_manager = manager.clone();
        let prepare = std::thread::spawn(move || {
            prepared
                .send(
                    preparing_manager
                        .prepare(dojo_id, SplintId::new(), 2)
                        .is_ok(),
                )
                .unwrap();
        });
        assert!(received.recv_timeout(Duration::from_millis(50)).is_err());

        gate.release();
        release.join().unwrap();
        assert!(received.recv_timeout(Duration::from_secs(1)).unwrap());
        prepare.join().unwrap();
        assert_eq!(
            adapter
                .calls()
                .iter()
                .filter(|call| matches!(call, AdapterCall::StartDojo(_)))
                .count(),
            2
        );
    }

    #[test]
    fn partial_scope_failure_rolls_back_scope_and_dojo() {
        let adapter = Arc::new(FakeAdapter::default());
        *adapter.fail_scope.lock().unwrap() = true;
        let manager =
            WorkloadUnitManager::new(adapter.clone(), WorkloadResourcePolicy::default()).unwrap();
        let prepared = manager.prepare(DojoId::new(), SplintId::new(), 1).unwrap();

        assert!(prepared.place(identity(103)).is_err());
        let calls = adapter.calls();
        assert!(matches!(calls[0], AdapterCall::StartDojo(_)));
        let expected_scope = match &calls[1] {
            AdapterCall::StartSplint(_, scope, 103) => scope,
            call => panic!("unexpected scope start call: {call:?}"),
        };
        assert!(matches!(&calls[2], AdapterCall::Stop(name) if name == expected_scope));
        assert!(matches!(&calls[3], AdapterCall::Stop(name) if is_slice_unit(name)));
    }

    #[test]
    #[ignore = "requires a running systemd user manager and creates disposable transient units"]
    fn systemd_adapter_moves_and_cleans_up_a_disposable_child() {
        let manager = WorkloadUnitManager::connect(
            WorkloadResourcePolicy::default(),
            Some("splinterd.service"),
        )
        .unwrap();
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let prepared = manager.prepare(DojoId::new(), SplintId::new(), 1).unwrap();
        let names = prepared.names.clone();
        let active = prepared.place(identity(pid)).unwrap();

        active.release().unwrap();
        child.wait().unwrap();
        let systemd = SystemdUserManager::connect(None).unwrap();
        assert_systemd_unit_absent(&systemd, &names.splint_scope);
        assert_systemd_unit_absent(&systemd, &names.dojo_slice);
    }

    #[test]
    #[ignore = "requires a running systemd user manager and creates disposable transient units"]
    fn pty_target_executes_only_inside_verified_splint_scope() {
        use std::{io::ErrorKind, time::Instant};

        use splinterm_pty::{LinuxPtyBackend, PtyCommand, PtyError, PtySize};

        let test_binary = std::env::current_exe().unwrap();
        let helper = test_binary
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("splinterm-pty-child");
        assert!(helper.is_file(), "missing PTY helper: {}", helper.display());
        let manager =
            WorkloadUnitManager::connect(WorkloadResourcePolicy::default(), None).unwrap();
        let prepared = manager.prepare(DojoId::new(), SplintId::new(), 1).unwrap();
        let names = prepared.names.clone();
        let command = PtyCommand::new("/bin/sh", "/tmp").args([
            "-c",
            "printf 'PARENT\\n'; cat /proc/self/cgroup; sleep 0.2 & child=$!; \
             printf 'CHILD\\n'; cat /proc/$child/cgroup; wait; printf 'DONE\\n'",
        ]);
        let (mut session, active) = LinuxPtyBackend::new(helper)
            .spawn_with_placement(&command, PtySize::cells(80, 24), move |identity| {
                prepared.place(identity)
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !String::from_utf8_lossy(&output).contains("DONE") {
            match session.read(&mut buffer) {
                Ok(count) => output.extend_from_slice(&buffer[..count]),
                Err(PtyError::Io { source, .. })
                    if matches!(
                        source.kind(),
                        ErrorKind::WouldBlock | ErrorKind::Interrupted
                    ) => {}
                Err(error) => panic!("failed reading PTY: {error}"),
            }
            assert!(Instant::now() < deadline, "timed out reading cgroup path");
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            String::from_utf8_lossy(&output)
                .matches(&names.splint_scope)
                .count()
                >= 2
        );
        assert!(session.wait().unwrap().success());
        active.release().unwrap();
        let systemd = SystemdUserManager::connect(None).unwrap();
        assert_systemd_unit_absent(&systemd, &names.splint_scope);
        assert_systemd_unit_absent(&systemd, &names.dojo_slice);
    }
}
