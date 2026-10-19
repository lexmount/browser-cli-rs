use super::process::ProcessIdentity;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_RECORD_BYTES: u64 = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub version: u8,
    pub session_id: String,
    pub process: ProcessIdentity,
    pub origin: String,
    pub browser_ws: String,
    pub created_at: u64,
    pub closed: bool,
}

pub(super) struct Registry {
    root: PathBuf,
}

impl Registry {
    pub fn from_env() -> Result<Self> {
        let root = match std::env::var_os("LEXMOUNT_MOLI_STATE_DIR") {
            Some(path) => PathBuf::from(path),
            None => dirs::home_dir()
                .ok_or_else(|| Error::Config("Moli home directory is unavailable".into()))?
                .join(".local/state/lexmount/browser-cli/moli"),
        };
        Self::open(root)
    }

    pub fn open(root: PathBuf) -> Result<Self> {
        if !root.is_absolute() {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&root)?;
            check_directory(&root)?;
            Ok(Self { root })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Err(Error::Config(
                "Moli owned instances are currently supported on macOS and Linux".into(),
            ))
        }
    }

    pub fn log(&self, id: &str) -> Result<fs::File> {
        validate_id(id)?;
        private_create(&self.root.join(format!("{id}.startup.log")))
    }

    pub fn read(&self, id: &str) -> Result<Record> {
        validate_id(id)?;
        let path = self.root.join(format!("{id}.json"));
        let file = private_read(&path).map_err(|error| match error {
            Error::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound => {
                Error::NotFound("Moli instance is not registered".into())
            }
            other => other,
        })?;
        let record: Record =
            serde_json::from_reader(file.take(MAX_RECORD_BYTES + 1)).map_err(|_| invalid())?;
        if record.version != 1 || record.session_id != id {
            return Err(invalid());
        }
        super::validate_endpoints(&record.origin, &record.browser_ws)?;
        Ok(record)
    }

    pub fn records(&self) -> Result<Vec<Record>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
                continue;
            };
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            if validate_id(id).is_ok() {
                result.push(self.read(id)?);
            }
        }
        result.sort_by_key(|record| record.created_at);
        Ok(result)
    }

    pub fn write(&self, record: &Record) -> Result<()> {
        validate_id(&record.session_id)?;
        super::validate_endpoints(&record.origin, &record.browser_ws)?;
        let bytes = serde_json::to_vec(record)?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(invalid());
        }
        let destination = self.root.join(format!("{}.json", record.session_id));
        let temporary = self.root.join(format!(
            ".{}.{:032x}.tmp",
            record.session_id,
            rand::random::<u128>()
        ));
        let result = (|| {
            let mut file = private_create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, destination)?;
            fs::File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

pub(super) fn validate_id(id: &str) -> Result<()> {
    let Some(suffix) = id.strip_prefix("moli_") else {
        return Err(invalid());
    };
    if suffix.len() != 32
        || !suffix
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> Error {
    Error::Config("Moli registry is invalid or not private to the current user".into())
}

#[cfg(unix)]
fn check_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(())
}

fn private_create(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    Ok(options.open(path)?)
}

fn private_read(path: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        let file = options.open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file()
            || meta.len() > MAX_RECORD_BYTES
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o600
        {
            return Err(invalid());
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        let _ = options;
        Err(invalid())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn private_atomic_records_reject_unsafe_paths_modes_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("registry");
        let registry = Registry::open(root.clone()).unwrap();
        let record = Record {
            version: 1,
            session_id: format!("moli_{:032x}", 3),
            process: super::super::process::identity(std::process::id()).unwrap(),
            origin: "http://127.0.0.1:12345".into(),
            browser_ws: "ws://127.0.0.1:12345/devtools/browser/fixed".into(),
            created_at: 1,
            closed: false,
        };
        registry.write(&record).unwrap();
        assert_eq!(
            registry.read(&record.session_id).unwrap().process.pid,
            std::process::id()
        );
        assert!(registry.read("../escape").is_err());
        let path = root.join(format!("{}.json", record.session_id));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(registry.read(&record.session_id).is_err());
        fs::remove_file(&path).unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        assert!(registry.read(&record.session_id).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        fs::remove_file(&path).unwrap();
        let target = temp.path().join("target");
        fs::write(&target, "{}").unwrap();
        symlink(target, &path).unwrap();
        assert!(registry.read(&record.session_id).is_err());
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Registry::open(root).is_err());
    }
}
