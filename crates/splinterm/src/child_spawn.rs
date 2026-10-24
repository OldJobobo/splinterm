//! The graphical client's gate on spawning child processes (Plan 0061 M3.14).
//!
//! A Window that relaunches itself across a daemon handoff clears
//! close-on-exec on exactly two descriptors, its sealed resume record and its
//! continuation socket, just before it replaces its process image. A child
//! another thread forks in that moment would inherit both. So every place a
//! running Window starts a child goes through [`spawn`], and a participating
//! Window [`pause`]s the gate from the moment it accepts its prepare frame:
//! spawns are refused, not queued, while it is paused, as typed input is
//! (M3.12), and the relaunch waits for spawns already under way to finish
//! ([`SpawnPause::wait_idle`]) before it touches a descriptor flag. A
//! cancelled attempt drops the pause and spawning resumes.
//!
//! The gate covers the client's own spawns. A library thread that forks on
//! its own is outside it; the relaunched image's post-exec claim closes any
//! descriptor it finds inheritable that it did not expect, as a backstop.

use std::{
    io,
    sync::{Condvar, Mutex, PoisonError},
    time::{Duration, Instant},
};

#[derive(Debug, Default)]
struct GateState {
    /// Pauses in force; spawning is refused while any is.
    pauses: usize,
    /// Spawns admitted and not yet returned.
    active: usize,
}

static STATE: Mutex<GateState> = Mutex::new(GateState {
    pauses: 0,
    active: 0,
});
static IDLE: Condvar = Condvar::new();

/// Serializes the tests that pause the gate or assert that it is open. The
/// gate is process-wide and the test harness runs tests in parallel threads
/// of one process, so one test's pause would otherwise refuse another's
/// spawn.
#[cfg(test)]
pub(crate) static TEST_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn lock() -> std::sync::MutexGuard<'static, GateState> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `start`, which spawns one child (or runs it to completion), unless a
/// relaunch has paused spawning.
///
/// # Errors
/// `start`'s own error, or [`io::ErrorKind::ResourceBusy`] while paused.
pub fn spawn<T>(start: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    struct Admitted;
    impl Drop for Admitted {
        fn drop(&mut self) {
            let mut state = lock();
            state.active -= 1;
            if state.active == 0 {
                IDLE.notify_all();
            }
        }
    }
    {
        let mut state = lock();
        if state.pauses > 0 {
            return Err(io::Error::new(
                io::ErrorKind::ResourceBusy,
                "child processes are paused while this Window is relaunched for an upgrade",
            ));
        }
        state.active += 1;
    }
    let _admitted = Admitted;
    start()
}

/// Refuses every later spawn until the returned pause is dropped.
#[must_use]
pub fn pause() -> SpawnPause {
    lock().pauses += 1;
    SpawnPause { _private: () }
}

/// One pause of the spawn gate; see [`pause`].
#[derive(Debug)]
pub struct SpawnPause {
    _private: (),
}

impl SpawnPause {
    /// Waits up to `timeout` for every spawn admitted before the pause to
    /// return; returns whether none is still under way.
    #[must_use]
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock();
        while state.active > 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = IDLE
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

impl Drop for SpawnPause {
    fn drop(&mut self) {
        lock().pauses -= 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// One test covers the whole gate: it is process-wide state.
    #[test]
    fn a_pause_refuses_new_spawns_and_waits_for_running_ones() {
        let _gate = TEST_GATE.blocking_lock();
        assert_eq!(spawn(|| Ok(7)).unwrap(), 7);

        let (entered, wait_entered) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let running = std::thread::spawn(move || {
            spawn(|| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            })
        });
        wait_entered.recv().unwrap();
        let pause = pause();
        let refused = spawn(|| Ok(())).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::ResourceBusy);
        assert!(!pause.wait_idle(Duration::from_millis(20)));
        release.send(()).unwrap();
        assert!(pause.wait_idle(Duration::from_secs(5)));
        running.join().unwrap().unwrap();
        drop(pause);
        assert!(spawn(|| Ok(())).is_ok());
    }
}
