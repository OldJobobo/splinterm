use std::{
    collections::HashMap,
    env,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use splinterm_core::{Dojo, DojoId, LayoutNode, SplintState, WindowId};

const RECENT_FILE: &str = "recent-windows.json";
const RECENT_VERSION: u8 = 1;
const MAX_RECENT_WINDOWS: usize = 64;
const MAX_RECENT_BYTES: usize = 16 * 1024;
const MAX_PICKER_LABEL_CHARS: usize = 256;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

fn picker_label(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_PICKER_LABEL_CHARS)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEntry {
    pub dojo_id: DojoId,
    pub window_id: WindowId,
    pub dojo_name: String,
    pub window_title: String,
    pub cwd: PathBuf,
    pub pane_count: usize,
    pub running_panes: usize,
    pub exited_panes: usize,
}

impl SessionEntry {
    #[must_use]
    pub const fn reopenable(&self) -> bool {
        self.pane_count > 0 && self.running_panes == self.pane_count
    }

    #[must_use]
    pub fn primary_label(&self) -> String {
        let label = if self.dojo_name == self.window_title {
            self.dojo_name.clone()
        } else {
            format!("{} / {}", self.dojo_name, self.window_title)
        };
        picker_label(&label)
    }

    #[must_use]
    pub fn secondary_label(&self) -> String {
        let pane_label = if self.pane_count == 1 {
            "pane"
        } else {
            "panes"
        };
        picker_label(&format!(
            "{} · {} {pane_label} · {} running",
            self.cwd.display(),
            self.pane_count,
            self.running_panes
        ))
    }
}

fn pane_states(node: &LayoutNode) -> (usize, usize) {
    match node {
        LayoutNode::Leaf(splint) => match splint.state {
            SplintState::Running => (1, 0),
            SplintState::Exited(_) => (0, 1),
            SplintState::Starting => (0, 0),
        },
        LayoutNode::Branch { first, second, .. } => {
            let (first_running, first_exited) = pane_states(first);
            let (second_running, second_exited) = pane_states(second);
            (first_running + second_running, first_exited + second_exited)
        }
    }
}

#[must_use]
pub fn collect_sessions(dojos: &[Dojo], recent: &[WindowId]) -> Vec<SessionEntry> {
    let ranks: HashMap<_, _> = recent
        .iter()
        .copied()
        .enumerate()
        .map(|(rank, id)| (id, rank))
        .collect();
    let mut entries = Vec::new();
    for dojo in dojos {
        for window in &dojo.windows {
            let (running_panes, exited_panes) = pane_states(&window.root);
            let cwd = window
                .root
                .find_splint(window.default_focus)
                .or_else(|| window.root.find_splint(window.root.first_splint_id()))
                .map_or_else(PathBuf::new, |splint| splint.cwd.clone());
            entries.push(SessionEntry {
                dojo_id: dojo.id,
                window_id: window.id,
                dojo_name: dojo.name.clone(),
                window_title: window.title.clone(),
                cwd,
                pane_count: window.root.splint_count(),
                running_panes,
                exited_panes,
            });
        }
    }
    entries.sort_by(|left, right| {
        let left_rank = ranks.get(&left.window_id).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.window_id).copied().unwrap_or(usize::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.dojo_name.cmp(&right.dojo_name))
            .then_with(|| left.window_title.cmp(&right.window_title))
            .then_with(|| left.window_id.to_string().cmp(&right.window_id.to_string()))
    });
    entries
}

#[must_use]
pub fn latest_reopenable(entries: &[SessionEntry]) -> Option<&SessionEntry> {
    entries.iter().find(|entry| entry.reopenable())
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecentDocument {
    version: u8,
    windows: Vec<WindowId>,
}

#[derive(Clone, Debug)]
pub struct RecentWindows {
    path: PathBuf,
}

impl RecentWindows {
    /// Resolves the owner-local recent-window state path.
    ///
    /// # Errors
    ///
    /// Returns an error when neither state-home source is available or when the
    /// configured state base is not absolute.
    pub fn discover() -> Result<Self> {
        let base = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                PathBuf::from(env::var_os("HOME").context("XDG_STATE_HOME and HOME are unset")?)
                    .join(".local/state")
            }
        };
        if !base.is_absolute() {
            bail!("state directory base must be absolute");
        }
        Ok(Self::from_path(base.join("splinterm").join(RECENT_FILE)))
    }

    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn load(&self) -> Vec<WindowId> {
        self.load_checked().unwrap_or_default()
    }

    fn load_checked(&self) -> Result<Vec<WindowId>> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > u64::try_from(MAX_RECENT_BYTES).unwrap()
        {
            bail!("recent-window state has unsafe owner, type, or size");
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(&self.path)?
            .take(u64::try_from(MAX_RECENT_BYTES + 1).unwrap())
            .read_to_end(&mut bytes)?;
        let document: RecentDocument =
            serde_json::from_slice(&bytes).context("invalid recent-window state")?;
        if document.version != RECENT_VERSION || document.windows.len() > MAX_RECENT_WINDOWS {
            bail!("unsupported or oversized recent-window state");
        }
        Ok(document.windows)
    }

    /// Moves one logical window to the front of the bounded MRU list.
    ///
    /// # Errors
    ///
    /// Returns an error when the owner-only state directory or atomic file
    /// update cannot be prepared and committed.
    pub fn record(&self, window_id: WindowId) -> Result<()> {
        let mut windows = self.load();
        windows.retain(|candidate| *candidate != window_id);
        windows.insert(0, window_id);
        windows.truncate(MAX_RECENT_WINDOWS);
        self.save(&windows)
    }

    fn save(&self, windows: &[WindowId]) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("recent-window state has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            bail!("recent-window state directory has unsafe owner or type");
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let encoded = serde_json::to_vec(&RecentDocument {
            version: RECENT_VERSION,
            windows: windows.to_vec(),
        })?;
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(
            ".{RECENT_FILE}.tmp-{}-{serial}",
            std::process::id()
        ));
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &self.path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_core::{Axis, Splint, SplitRatio, Window};

    fn running_dojo(name: &str, cwd: &str) -> Dojo {
        let mut dojo = Dojo::new(name, PathBuf::from(cwd));
        let default_focus = dojo.windows[0].default_focus;
        let splint = dojo.windows[0].root.find_splint_mut(default_focus).unwrap();
        splint.state = SplintState::Running;
        splint.last_incarnation = Some(1);
        dojo
    }

    fn temp_recent_path() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        std::env::temp_dir()
            .join(format!(
                "splinterm-recent-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .join(RECENT_FILE)
    }

    #[test]
    fn sessions_are_window_level_and_recent_first() {
        let first = running_dojo("work", "/work");
        let mut second = running_dojo("notes", "/notes");
        second
            .windows
            .push(Window::with_shell(PathBuf::from("/logs")));
        let recent = second.windows[0].id;
        let entries = collect_sessions(&[first, second], &[recent]);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].window_id, recent);
        assert_eq!(entries[0].primary_label(), "notes / terminal");
        assert!(entries[0].secondary_label().contains("1 pane"));
    }

    #[test]
    fn mixed_running_and_exited_windows_are_not_offered_for_reopen() {
        let mut dojo = running_dojo("mixed", "/work");
        let running = dojo.windows[0].root.clone();
        let mut exited = Splint::shell(PathBuf::from("/work"));
        exited.state = SplintState::Exited(0);
        dojo.windows[0].root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(running),
            second: Box::new(LayoutNode::Leaf(exited)),
        };
        let entries = collect_sessions(&[dojo], &[]);
        assert_eq!((entries[0].running_panes, entries[0].exited_panes), (1, 1));
        assert!(!entries[0].reopenable());
        assert!(latest_reopenable(&entries).is_none());
    }

    #[test]
    fn exited_only_windows_are_not_reopenable() {
        let mut dojo = Dojo::new("old", PathBuf::from("/old"));
        let default_focus = dojo.windows[0].default_focus;
        let splint = dojo.windows[0].root.find_splint_mut(default_focus).unwrap();
        splint.state = SplintState::Exited(0);
        let entries = collect_sessions(&[dojo], &[]);
        assert!(latest_reopenable(&entries).is_none());
    }

    #[test]
    fn picker_labels_replace_controls_and_bound_untrusted_metadata() {
        let mut entry = collect_sessions(&[running_dojo("work\nspoof", "/tmp")], &[]).remove(0);
        entry.window_title = "terminal\u{1b}[31m".repeat(100);
        let primary = entry.primary_label();
        assert!(!primary.chars().any(char::is_control));
        assert!(primary.chars().count() <= MAX_PICKER_LABEL_CHARS);
    }

    #[test]
    fn recent_store_is_bounded_and_ignores_malformed_state() {
        let path = temp_recent_path();
        let store = RecentWindows::from_path(path.clone());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json").unwrap();
        assert!(store.load().is_empty());
        let first = WindowId::new();
        store.record(first).unwrap();
        assert_eq!(store.load(), vec![first]);
        let second = WindowId::new();
        store.record(second).unwrap();
        assert_eq!(store.load(), vec![second, first]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
