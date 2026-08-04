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
use splinterm_core::{DojoId, Lair, LairId, LayoutNode, SplintState};
use unicode_width::UnicodeWidthChar;

const RECENT_FILE: &str = "recent-dojos.json";
const LEGACY_RECENT_FILE: &str = "recent-windows.json";
const RECENT_VERSION: u8 = 2;
const LEGACY_RECENT_VERSION: u8 = 1;
const MAX_RECENT_DOJOS: usize = 64;
const MAX_RECENT_BYTES: usize = 16 * 1024;
const MAX_PICKER_TITLE_SCALARS: usize = 256;
const MAX_PICKER_TITLE_CELLS: usize = 160;
const MAX_PICKER_CWD_SCALARS: usize = 512;
const MAX_PICKER_CWD_CELLS: usize = 240;
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

const fn is_bidi_formatting(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

fn picker_label(value: &str, maximum_scalars: usize, maximum_cells: usize) -> String {
    let mut characters = Vec::new();
    let mut cells = 0_usize;
    let mut truncated = false;
    for character in value.chars() {
        if is_bidi_formatting(character) {
            continue;
        }
        let sanitized = if character.is_control() {
            if characters.last() == Some(&' ') {
                continue;
            }
            ' '
        } else {
            character
        };
        let width = UnicodeWidthChar::width(sanitized).unwrap_or(0).min(2);
        if characters.len() == maximum_scalars || cells.saturating_add(width) > maximum_cells {
            truncated = true;
            break;
        }
        characters.push(sanitized);
        cells = cells.saturating_add(width);
    }
    if truncated && maximum_scalars > 0 && maximum_cells > 0 {
        while characters.len() >= maximum_scalars || cells.saturating_add(1) > maximum_cells {
            let Some(character) = characters.pop() else {
                break;
            };
            cells = cells.saturating_sub(UnicodeWidthChar::width(character).unwrap_or(0).min(2));
        }
        characters.push('…');
    }
    characters.into_iter().collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionEntry {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub lair_name: String,
    pub dojo_name: String,
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
    pub fn display_title(&self) -> String {
        let label = if self.lair_name == self.dojo_name {
            self.lair_name.clone()
        } else {
            format!("{} / {}", self.lair_name, self.dojo_name)
        };
        picker_label(&label, MAX_PICKER_TITLE_SCALARS, MAX_PICKER_TITLE_CELLS)
    }

    #[must_use]
    pub fn working_directory(&self) -> String {
        picker_label(
            &self.cwd.to_string_lossy(),
            MAX_PICKER_CWD_SCALARS,
            MAX_PICKER_CWD_CELLS,
        )
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
pub fn collect_sessions(lairs: &[Lair], recent: &[DojoId]) -> Vec<SessionEntry> {
    let ranks: HashMap<_, _> = recent
        .iter()
        .copied()
        .enumerate()
        .map(|(rank, id)| (id, rank))
        .collect();
    let mut entries = Vec::new();
    for lair in lairs {
        for dojo in &lair.dojos {
            let (running_panes, exited_panes) = pane_states(&dojo.root);
            let cwd = dojo
                .root
                .find_splint(dojo.default_focus)
                .or_else(|| dojo.root.find_splint(dojo.root.first_splint_id()))
                .map_or_else(PathBuf::new, |splint| splint.cwd.clone());
            entries.push(SessionEntry {
                lair_id: lair.id,
                dojo_id: dojo.id,
                lair_name: lair.name.clone(),
                dojo_name: dojo.name.clone(),
                cwd,
                pane_count: dojo.root.splint_count(),
                running_panes,
                exited_panes,
            });
        }
    }
    entries.sort_by(|left, right| {
        let left_rank = ranks.get(&left.dojo_id).copied().unwrap_or(usize::MAX);
        let right_rank = ranks.get(&right.dojo_id).copied().unwrap_or(usize::MAX);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.lair_name.cmp(&right.lair_name))
            .then_with(|| left.dojo_name.cmp(&right.dojo_name))
            .then_with(|| left.dojo_id.to_string().cmp(&right.dojo_id.to_string()))
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
    dojos: Vec<DojoId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRecentDocument {
    version: u8,
    windows: Vec<DojoId>,
}

#[derive(Clone, Debug)]
pub struct RecentDojos {
    path: PathBuf,
}

impl RecentDojos {
    /// Resolves the owner-local recent-Dojo state path.
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
    pub fn load(&self) -> Vec<DojoId> {
        self.load_checked().unwrap_or_default()
    }

    fn load_checked(&self) -> Result<Vec<DojoId>> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.load_legacy_checked();
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > u64::try_from(MAX_RECENT_BYTES).unwrap()
        {
            bail!("recent-Dojo state has unsafe owner, type, or size");
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(&self.path)?
            .take(u64::try_from(MAX_RECENT_BYTES + 1).unwrap())
            .read_to_end(&mut bytes)?;
        let document: RecentDocument =
            serde_json::from_slice(&bytes).context("invalid recent-Dojo state")?;
        if document.version != RECENT_VERSION || document.dojos.len() > MAX_RECENT_DOJOS {
            bail!("unsupported or oversized recent-Dojo state");
        }
        Ok(document.dojos)
    }

    fn load_legacy_checked(&self) -> Result<Vec<DojoId>> {
        let legacy = self.path.with_file_name(LEGACY_RECENT_FILE);
        let metadata = match fs::symlink_metadata(&legacy) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != 0o600
            || metadata.len() > u64::try_from(MAX_RECENT_BYTES).unwrap()
        {
            bail!("legacy recent-Dojo state has unsafe owner, type, or size");
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        File::open(&legacy)?
            .take(u64::try_from(MAX_RECENT_BYTES + 1).unwrap())
            .read_to_end(&mut bytes)?;
        let document: LegacyRecentDocument =
            serde_json::from_slice(&bytes).context("invalid legacy recent-Dojo state")?;
        if document.version != LEGACY_RECENT_VERSION || document.windows.len() > MAX_RECENT_DOJOS {
            bail!("unsupported or oversized legacy recent-Dojo state");
        }
        self.save(&document.windows)?;
        Ok(document.windows)
    }

    /// Moves one logical Dojo to the front of the bounded MRU list.
    ///
    /// # Errors
    ///
    /// Returns an error when the owner-only state directory or atomic file
    /// update cannot be prepared and committed.
    pub fn record(&self, dojo_id: DojoId) -> Result<()> {
        let mut dojos = self.load();
        dojos.retain(|candidate| *candidate != dojo_id);
        dojos.insert(0, dojo_id);
        dojos.truncate(MAX_RECENT_DOJOS);
        self.save(&dojos)
    }

    fn save(&self, dojos: &[DojoId]) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("recent-Dojo state has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            bail!("recent-Dojo state directory has unsafe owner or type");
        }
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let encoded = serde_json::to_vec(&RecentDocument {
            version: RECENT_VERSION,
            dojos: dojos.to_vec(),
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
    use splinterm_core::{Axis, Dojo, Splint, SplitRatio};

    fn running_dojo(name: &str, cwd: &str) -> Lair {
        let mut dojo = Lair::new(name, PathBuf::from(cwd));
        let default_focus = dojo.dojos[0].default_focus;
        let splint = dojo.dojos[0].root.find_splint_mut(default_focus).unwrap();
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
    fn sessions_are_dojo_level_and_recent_first() {
        let first = running_dojo("work", "/work");
        let mut second = running_dojo("notes", "/notes");
        second
            .dojos
            .push(Dojo::with_shell("logs", PathBuf::from("/logs")));
        let recent = second.dojos[0].id;
        let entries = collect_sessions(&[first, second], &[recent]);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].dojo_id, recent);
        assert_eq!(entries[0].display_title(), "notes / terminal");
        assert_eq!(entries[0].working_directory(), "/notes");
    }

    #[test]
    fn mixed_running_and_exited_dojos_are_not_offered_for_reopen() {
        let mut dojo = running_dojo("mixed", "/work");
        let running = dojo.dojos[0].root.clone();
        let mut exited = Splint::shell(PathBuf::from("/work"));
        exited.state = SplintState::Exited(0);
        dojo.dojos[0].root = LayoutNode::Branch {
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
    fn exited_only_dojos_are_not_reopenable() {
        let mut dojo = Lair::new("old", PathBuf::from("/old"));
        let default_focus = dojo.dojos[0].default_focus;
        let splint = dojo.dojos[0].root.find_splint_mut(default_focus).unwrap();
        splint.state = SplintState::Exited(0);
        let entries = collect_sessions(&[dojo], &[]);
        assert!(latest_reopenable(&entries).is_none());
    }

    #[test]
    fn picker_labels_replace_controls_remove_bidi_and_bound_metadata() {
        let mut entry = collect_sessions(&[running_dojo("work\n\rspoof", "/tmp")], &[]).remove(0);
        entry.dojo_name = format!(
            "{}{}",
            "terminal\u{1b}[31m\u{202e}\u{2066}".repeat(100),
            "界".repeat(200)
        );
        entry.cwd = PathBuf::from(format!("/tmp/{}\u{200f}", "界".repeat(300)));
        let title = entry.display_title();
        let cwd = entry.working_directory();
        assert!(!title.chars().any(char::is_control));
        assert!(!title.chars().any(is_bidi_formatting));
        assert!(!cwd.chars().any(is_bidi_formatting));
        assert!(title.chars().count() <= MAX_PICKER_TITLE_SCALARS);
        assert!(cwd.chars().count() <= MAX_PICKER_CWD_SCALARS);
        assert!(
            title
                .chars()
                .map(|character| character.width().unwrap_or(0).min(2))
                .sum::<usize>()
                <= MAX_PICKER_TITLE_CELLS
        );
        assert!(
            cwd.chars()
                .map(|character| character.width().unwrap_or(0).min(2))
                .sum::<usize>()
                <= MAX_PICKER_CWD_CELLS
        );
        assert!(title.ends_with('…'));
        assert!(cwd.ends_with('…'));
        assert!(entry.display_title().contains("work spoof"));
    }

    #[test]
    fn picker_label_preserves_combining_marks_with_scalar_and_cell_bounds() {
        let label = picker_label(
            &format!("{}界", "e\u{301}".repeat(MAX_PICKER_TITLE_SCALARS)),
            MAX_PICKER_TITLE_SCALARS,
            MAX_PICKER_TITLE_CELLS,
        );
        assert!(label.ends_with('…'));
        assert!(label.chars().count() <= MAX_PICKER_TITLE_SCALARS);
        assert!(
            label
                .chars()
                .map(|character| character.width().unwrap_or(0).min(2))
                .sum::<usize>()
                <= MAX_PICKER_TITLE_CELLS
        );
    }

    #[test]
    fn legacy_recent_windows_migrate_to_recent_dojos_without_deleting_source() {
        let path = temp_recent_path();
        let legacy = path.with_file_name(LEGACY_RECENT_FILE);
        let store = RecentDojos::from_path(path.clone());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let first = DojoId::new();
        fs::write(
            &legacy,
            serde_json::to_vec(&serde_json::json!({
                "version": LEGACY_RECENT_VERSION,
                "windows": [first]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.load(), vec![first]);
        assert!(path.exists());
        assert!(legacy.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn recent_store_is_bounded_and_ignores_malformed_state() {
        let path = temp_recent_path();
        let store = RecentDojos::from_path(path.clone());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json").unwrap();
        assert!(store.load().is_empty());
        let first = DojoId::new();
        store.record(first).unwrap();
        assert_eq!(store.load(), vec![first]);
        let second = DojoId::new();
        store.record(second).unwrap();
        assert_eq!(store.load(), vec![second, first]);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
