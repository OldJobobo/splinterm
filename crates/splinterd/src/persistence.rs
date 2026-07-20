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
use splinterm_core::{LairDocument, MAX_LAIR_DOCUMENT_BYTES};

const STATE_DIRECTORY: &str = "splinterm";
const PRIMARY_FILE: &str = "lair.json";
const BACKUP_FILE: &str = "lair.json.previous";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

/// Owner-only durable metadata storage with an atomic previous-generation backup.
#[derive(Clone)]
pub struct MetadataStore {
    directory: PathBuf,
    primary: PathBuf,
    backup: PathBuf,
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
            directory,
        }
    }

    pub fn load(&self) -> Result<Option<LairDocument>> {
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
            Ok(document) => Ok(document),
            Err(error) => {
                self.quarantine(&self.backup)
                    .context("failed to quarantine invalid backup metadata")?;
                Err(error)
            }
        }
    }

    pub fn save(&self, document: &LairDocument) -> Result<()> {
        self.prepare_directory()?;
        let encoded = document
            .encode()
            .context("failed to encode Lair metadata")?;
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

    fn read_if_present(path: &Path) -> Result<Option<LairDocument>> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        validate_file(path, &metadata)?;
        if metadata.len() > u64::try_from(MAX_LAIR_DOCUMENT_BYTES).unwrap() {
            bail!("metadata file exceeds its size limit");
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap());
        File::open(path)?
            .take(
                u64::try_from(MAX_LAIR_DOCUMENT_BYTES)
                    .unwrap()
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)?;
        let document = LairDocument::decode(&bytes).context("invalid Lair metadata")?;
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
        for suffix in 0..100_u8 {
            let candidate = self
                .directory
                .join(format!("lair.invalid-{timestamp}-{suffix}"));
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
        self.directory
            .join(format!(".lair.json.tmp-{}-{serial}", std::process::id()))
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

    use splinterm_core::{Lair, LayoutNode, SplintState};

    use super::*;

    fn test_base(name: &str) -> PathBuf {
        let serial = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("splinterd-{name}-{}-{serial}", std::process::id()))
    }

    fn document(revision: u64) -> LairDocument {
        let mut lair = Lair::new();
        let dojo = lair.create_dojo("main", PathBuf::from("/tmp")).unwrap();
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        let splint_id = splint.id;
        assert!(lair.set_splint_state(splint_id, SplintState::Exited(0)));
        if revision == 2 {
            let dojo_id = lair.dojos().next().unwrap().id;
            lair.rename_dojo_at(lair.revision(), dojo_id, "renamed")
                .unwrap();
        }
        assert_eq!(lair.revision().get(), revision);
        LairDocument::from_lair(&lair).unwrap()
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
                .into_lair()
                .unwrap()
                .revision()
                .get(),
            2
        );
        assert_eq!(
            MetadataStore::read_if_present(&store.backup)
                .unwrap()
                .unwrap()
                .into_lair()
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
    fn invalid_primary_is_quarantined_and_backup_survives() {
        let base = test_base("quarantine");
        let store = MetadataStore::from_base(&base);
        store.save(&document(1)).unwrap();
        store.save(&document(2)).unwrap();
        fs::write(&store.primary, b"{truncated").unwrap();
        let loaded = store.load().unwrap().unwrap().into_lair().unwrap();
        assert_eq!(loaded.revision().get(), 1);
        assert!(fs::read_dir(&store.directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("lair.invalid-")
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
                .starts_with("lair.invalid-")
        }));
        fs::remove_dir_all(base).unwrap();
        fs::remove_dir_all(target).unwrap();
    }
}
