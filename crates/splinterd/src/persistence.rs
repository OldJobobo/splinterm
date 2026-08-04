use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm_core::{MAX_TOPOLOGY_DOCUMENT_BYTES, TopologyDocument};

const STATE_DIRECTORY: &str = "splinterm";
const PRIMARY_FILE: &str = "topology.json";
const BACKUP_FILE: &str = "topology.json.previous";
const LEGACY_PRIMARY_FILE: &str = "lair.json";
const LEGACY_BACKUP_FILE: &str = "lair.json.previous";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

/// Owner-only durable metadata storage with an atomic previous-generation backup.
#[derive(Clone)]
pub struct MetadataStore {
    directory: PathBuf,
    primary: PathBuf,
    backup: PathBuf,
    legacy_primary: PathBuf,
    legacy_backup: PathBuf,
}

impl MetadataStore {
    pub fn discover() -> Result<Self> {
        Self::from_environment(env::var_os("XDG_STATE_HOME"), env::var_os("HOME"))
    }

    fn from_environment(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Result<Self> {
        let base = if let Some(path) = xdg_state_home.filter(|path| !path.is_empty()) {
            PathBuf::from(path)
        } else {
            PathBuf::from(home.context("XDG_STATE_HOME and HOME are unset")?).join(".local/state")
        };
        if !base.is_absolute() {
            bail!("state directory base must be absolute");
        }
        Ok(Self::from_base(&base))
    }

    pub fn from_base(base: &Path) -> Self {
        let directory = base.join(STATE_DIRECTORY);
        Self {
            primary: directory.join(PRIMARY_FILE),
            backup: directory.join(BACKUP_FILE),
            legacy_primary: directory.join(LEGACY_PRIMARY_FILE),
            legacy_backup: directory.join(LEGACY_BACKUP_FILE),
            directory,
        }
    }

    pub fn load(&self) -> Result<Option<TopologyDocument>> {
        self.prepare_directory()?;
        match Self::read_if_present(&self.primary) {
            Ok(Some(document)) => return Ok(Some(document)),
            Ok(None) => {}
            Err(error) => {
                self.quarantine(&self.primary)
                    .context("failed to quarantine invalid primary metadata")?;
                tracing::warn!(%error, "quarantined invalid primary metadata");
            }
        }
        match Self::read_if_present(&self.backup) {
            Ok(Some(document)) => return Ok(Some(document)),
            Ok(None) => {}
            Err(error) => {
                self.quarantine(&self.backup)
                    .context("failed to quarantine invalid backup metadata")?;
                return Err(error);
            }
        }

        // Schema-v2 installations used lair.json. Decode through the core's
        // strict legacy DTO, commit canonical schema-v3 state, and retain the
        // legacy file so a failed migration can never destroy the only copy.
        for legacy in [&self.legacy_primary, &self.legacy_backup] {
            match Self::read_if_present(legacy) {
                Ok(Some(document)) => {
                    self.save(&document)
                        .context("failed to commit migrated topology metadata")?;
                    return Ok(Some(document));
                }
                Ok(None) => {}
                Err(error) => {
                    self.quarantine(legacy)
                        .context("failed to quarantine invalid legacy metadata")?;
                    tracing::warn!(%error, path = %legacy.display(), "quarantined invalid legacy metadata");
                }
            }
        }
        Ok(None)
    }

    pub fn save(&self, document: &TopologyDocument) -> Result<()> {
        self.prepare_directory()?;
        let encoded = document
            .encode()
            .context("failed to encode Topology metadata")?;
        let temp = self.temporary_path();
        let result = self.write_temp_and_commit(&temp, &encoded);
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn prepare_directory(&self) -> Result<()> {
        fs::create_dir_all(&self.directory).with_context(|| {
            format!(
                "failed to create state directory {}",
                self.directory.display()
            )
        })?;
        let metadata = fs::symlink_metadata(&self.directory)?;
        if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
            bail!("state directory has unsafe owner or type");
        }
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    fn read_if_present(path: &Path) -> Result<Option<TopologyDocument>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        validate_file(path, &metadata)?;
        if metadata.len() > u64::try_from(MAX_TOPOLOGY_DOCUMENT_BYTES).unwrap() {
            bail!("metadata file exceeds its size limit");
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap());
        File::open(path)?
            .take(
                u64::try_from(MAX_TOPOLOGY_DOCUMENT_BYTES)
                    .unwrap()
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)?;
        let document = TopologyDocument::decode(&bytes).context("invalid Topology metadata")?;
        Ok(Some(document))
    }

    fn write_temp_and_commit(&self, temp: &Path, encoded: &[u8]) -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(temp)
            .with_context(|| format!("failed to create {}", temp.display()))?;
        file.write_all(encoded)?;
        file.sync_all()?;
        drop(file);

        if let Some(metadata) = metadata_if_present(&self.primary)? {
            match validate_file(&self.primary, &metadata) {
                Ok(()) => fs::rename(&self.primary, &self.backup)?,
                Err(error) => {
                    tracing::warn!(%error, "preserving backup and quarantining invalid primary");
                    self.quarantine(&self.primary)?;
                }
            }
        }
        fs::rename(temp, &self.primary)?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    fn quarantine(&self, path: &Path) -> Result<()> {
        if metadata_if_present(path)?.is_none() {
            return Ok(());
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let prefix = if path
            .file_name()
            .is_some_and(|name| name == LEGACY_PRIMARY_FILE || name == LEGACY_BACKUP_FILE)
        {
            "lair"
        } else {
            "topology"
        };
        for suffix in 0..100_u8 {
            let candidate = self
                .directory
                .join(format!("{prefix}.invalid-{timestamp}-{suffix}"));
            if !candidate.exists() {
                fs::rename(path, candidate)?;
                File::open(&self.directory)?.sync_all()?;
                return Ok(());
            }
        }
        bail!("unable to allocate quarantine filename")
    }

    fn temporary_path(&self) -> PathBuf {
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        self.directory.join(format!(
            ".topology.json.tmp-{}-{serial}",
            std::process::id()
        ))
    }
}

fn metadata_if_present(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_file(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("unsafe metadata file {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use splinterm_core::{LayoutNode, SplintState, Topology};

    use super::*;

    fn test_base(name: &str) -> PathBuf {
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("splinterd-{name}-{}-{serial}", std::process::id()))
    }

    fn document(revision: u64) -> TopologyDocument {
        let mut lair = Topology::new();
        let dojo = lair.create_lair("main", PathBuf::from("/tmp")).unwrap();
        let LayoutNode::Leaf(splint) = &dojo.dojos[0].root else {
            unreachable!()
        };
        let splint_id = splint.id;
        assert!(lair.set_splint_state(splint_id, SplintState::Exited(0)));
        if revision == 2 {
            let lair_id = lair.lairs().next().unwrap().id;
            lair.rename_lair_at(lair.revision(), lair_id, "renamed")
                .unwrap();
        }
        assert_eq!(lair.revision().get(), revision);
        TopologyDocument::from_topology(&lair).unwrap()
    }

    #[test]
    fn atomic_save_load_and_backup_preserve_generations() {
        let base = test_base("generations");
        let store = MetadataStore::from_base(&base);
        store.save(&document(1)).unwrap();
        store.save(&document(2)).unwrap();
        assert_eq!(
            store
                .load()
                .unwrap()
                .unwrap()
                .into_topology()
                .unwrap()
                .revision()
                .get(),
            2
        );
        assert_eq!(
            MetadataStore::read_if_present(&store.backup)
                .unwrap()
                .unwrap()
                .into_topology()
                .unwrap()
                .revision()
                .get(),
            1
        );
        assert_eq!(
            fs::metadata(&store.directory).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(fs::metadata(&store.primary).unwrap().mode() & 0o777, 0o600);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn legacy_v2_lair_file_migrates_without_deleting_the_source() {
        let base = test_base("legacy-v2");
        let store = MetadataStore::from_base(&base);
        store.prepare_directory().unwrap();
        let legacy = serde_json::json!({
            "schema_version": 2,
            "revision": 7,
            "dojos": [{
                "id": "018f4d8c-2a18-4b31-8c2f-9e7c5de77101",
                "name": "main",
                "windows": [{
                    "id": "018f4d8c-2a18-4b31-8c2f-9e7c5de77102",
                    "title": "editor",
                    "default_focus": "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
                    "root": {"Leaf": {
                        "id": "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
                        "title": "shell",
                        "cwd": "/tmp",
                        "command": [],
                        "state": {"Exited": 0}
                    }}
                }]
            }]
        });
        fs::write(&store.legacy_primary, serde_json::to_vec(&legacy).unwrap()).unwrap();
        fs::set_permissions(&store.legacy_primary, fs::Permissions::from_mode(0o600)).unwrap();

        let topology = store.load().unwrap().unwrap().into_topology().unwrap();
        let lair = topology.lairs().next().unwrap();
        assert_eq!(topology.revision().get(), 7);
        assert_eq!(lair.id.to_string(), "018f4d8c-2a18-4b31-8c2f-9e7c5de77101");
        assert_eq!(
            lair.dojos[0].id.to_string(),
            "018f4d8c-2a18-4b31-8c2f-9e7c5de77102"
        );
        assert_eq!(lair.dojos[0].name, "editor");
        assert!(store.primary.exists());
        assert!(store.legacy_primary.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn invalid_primary_is_quarantined_and_backup_survives() {
        let base = test_base("quarantine");
        let store = MetadataStore::from_base(&base);
        store.save(&document(1)).unwrap();
        store.save(&document(2)).unwrap();
        fs::write(&store.primary, b"{truncated").unwrap();
        let loaded = store.load().unwrap().unwrap().into_topology().unwrap();
        assert_eq!(loaded.revision().get(), 1);
        assert!(fs::read_dir(&store.directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("topology.invalid-")
        }));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn refuses_symlinked_state_directory_and_metadata_file() {
        let base = test_base("symlinks");
        let target = test_base("symlink-target");
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&target).unwrap();
        symlink(&target, base.join(STATE_DIRECTORY)).unwrap();
        assert!(MetadataStore::from_base(&base).save(&document(1)).is_err());
        fs::remove_file(base.join(STATE_DIRECTORY)).unwrap();

        let store = MetadataStore::from_base(&base);
        store.prepare_directory().unwrap();
        let outside = target.join("outside");
        fs::write(&outside, b"untouched").unwrap();
        symlink(&outside, &store.primary).unwrap();
        assert!(store.load().unwrap().is_none());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        assert!(fs::read_dir(&store.directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("topology.invalid-")
        }));
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
}
