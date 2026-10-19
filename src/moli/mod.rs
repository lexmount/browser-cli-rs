//! Same-user, CLI-owned local Moli instances. No cloud credentials are loaded.
use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod close;
mod process;
mod registry;
use registry::{Record, Registry};

pub struct Moli {
    registry: Registry,
}

impl Moli {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            registry: Registry::from_env()?,
        })
    }

    /// Launch an owned process; successful return does not transfer ownership to a viewer.
    pub fn create(&self, timeout: Duration) -> Result<Value> {
        if timeout.is_zero() {
            return Err(Error::Config(
                "Moli startup timeout must be positive".into(),
            ));
        }
        let binary = find_binary()?;
        let id = format!("moli_{:032x}", rand::random::<u128>());
        let log = self.registry.log(&id)?;
        let (mut child, identity) = process::spawn(&binary, &log)?;
        let started = Instant::now();
        let result = (|| {
            let mut origin = None;
            loop {
                if child.try_wait()?.is_some() {
                    return Err(Error::Cdp("Moli exited before becoming ready".into()));
                }
                if started.elapsed() >= timeout {
                    return Err(Error::Timeout("Moli did not become ready".into()));
                }
                if origin.is_none() {
                    let bytes = read_startup(&log)?;
                    origin = startup_origin(&String::from_utf8_lossy(&bytes));
                }
                if let Some(origin) = &origin {
                    if let Ok(version) =
                        fetch_json(origin, "/json/version", Duration::from_millis(400))
                    {
                        if let Some(ws) =
                            version.get("webSocketDebuggerUrl").and_then(Value::as_str)
                        {
                            validate_endpoints(origin, ws)?;
                            if !process::matches(&identity) {
                                return Err(Error::Conflict(
                                    "Moli process identity changed during startup".into(),
                                ));
                            }
                            let record = Record {
                                version: 1,
                                session_id: id.clone(),
                                process: identity.clone(),
                                origin: origin.clone(),
                                browser_ws: ws.to_owned(),
                                created_at: SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64,
                                closed: false,
                            };
                            self.registry.write(&record)?;
                            return Ok(row(&record));
                        }
                    }
                }
                thread::sleep(Duration::from_millis(40));
            }
        })();
        if result.is_err() {
            let _ = process::stop(&identity);
            let _ = child.try_wait();
        }
        result
    }

    pub fn get(&self, id: &str) -> Result<Value> {
        Ok(row(&self.registry.read(id)?))
    }

    pub fn list(
        &self,
        status: Option<&str>,
        page: Option<u64>,
        page_size: Option<u64>,
    ) -> Result<Value> {
        let status = status.unwrap_or("active");
        if !matches!(status, "active" | "closed") {
            return Err(Error::Config("Moli status must be active or closed".into()));
        }
        let page = page.unwrap_or(1);
        let page_size = page_size.unwrap_or(100);
        if page == 0 || page_size == 0 {
            return Err(Error::Config(
                "Moli page and page size must be positive".into(),
            ));
        }
        let rows: Vec<Value> = self.registry.records()?.iter().map(row).collect();
        let active = rows.iter().filter(|r| r["status"] == "active").count();
        let closed = rows.len() - active;
        let matching: Vec<Value> = rows.into_iter().filter(|r| r["status"] == status).collect();
        let total = matching.len() as u64;
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        let sessions: Vec<_> = matching
            .into_iter()
            .skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(usize::try_from(page_size).unwrap_or(usize::MAX))
            .collect();
        Ok(
            json!({"provider":"moli","sessions":sessions,"pagination":{"currentPage":page,"pageSize":page_size,"totalCount":total,"totalPages":total.div_ceil(page_size).max(1),"activeCount":active,"closedCount":closed}}),
        )
    }

    pub fn targets(&self, id: &str) -> Result<Value> {
        let record = self.active(id)?;
        let targets = fetch_json(&record.origin, "/json/list", Duration::from_secs(3))?;
        let rows = targets.as_array().ok_or_else(invalid_response)?;
        for target in rows {
            if target.get("type").and_then(Value::as_str) != Some("page") {
                continue;
            }
            let id = target
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| {
                    !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
                })
                .ok_or_else(invalid_response)?;
            let ws = target
                .get("webSocketDebuggerUrl")
                .and_then(Value::as_str)
                .ok_or_else(invalid_response)?;
            validate_socket(&record.origin, ws, &format!("/devtools/page/{id}"))?;
        }
        if !process::matches(&record.process) {
            return Err(Error::Conflict("Moli process identity changed".into()));
        }
        Ok(targets)
    }

    pub fn browser_ws(&self, id: &str) -> Result<String> {
        Ok(self.active(id)?.browser_ws)
    }

    pub fn event_connection_ref(&self, id: &str) -> Result<String> {
        self.active(id)?;
        connection_ref(id)
    }

    pub fn close(&self, id: &str) -> Result<Value> {
        let mut record = self.registry.read(id)?;
        let already_inactive = close::close(&record)?;
        record.closed = true;
        self.registry.write(&record)?;
        Ok(
            json!({"provider":"moli","session_id":id,"closed":true,"already_inactive":already_inactive}),
        )
    }

    fn active(&self, id: &str) -> Result<Record> {
        let record = self.registry.read(id)?;
        if record.closed || !process::matches(&record.process) {
            return Err(Error::NotFound("Moli instance is not active".into()));
        }
        Ok(record)
    }
}

/// Stable only for this owned instance, independent from cloud identity and port reuse.
pub fn connection_ref(id: &str) -> Result<String> {
    registry::validate_id(id)?;
    let mut hash = Sha256::new();
    hash.update(b"lexmount-local-moli-v1\0");
    hash.update(id.as_bytes());
    Ok(format!("moli:{:x}", hash.finalize()))
}

fn row(record: &Record) -> Value {
    let active = !record.closed && process::matches(&record.process);
    json!({"provider":"moli","session_id":record.session_id,"status":if active {"active"} else {"closed"},"context_display_name":format!("Moli {}", &record.session_id[5..13]),"connection_ref":connection_ref(&record.session_id).expect("validated registry ID"),"created_at":record.created_at,"http_endpoint":if active {Some(record.origin.as_str())} else {None},"ws":if active {Some(record.browser_ws.as_str())} else {None}})
}

pub(super) fn validate_endpoints(origin: &str, ws: &str) -> Result<()> {
    let parsed = url::Url::parse(ws).map_err(|_| invalid_response())?;
    if !parsed.path().starts_with("/devtools/browser/")
        || parsed.path().len() <= "/devtools/browser/".len()
    {
        return Err(invalid_response());
    }
    validate_socket(origin, ws, parsed.path())
}

fn validate_socket(origin: &str, ws: &str, expected_path: &str) -> Result<()> {
    let origin = url::Url::parse(origin).map_err(|_| invalid_response())?;
    let ws = url::Url::parse(ws).map_err(|_| invalid_response())?;
    if origin.scheme() != "http"
        || origin.host_str() != Some("127.0.0.1")
        || origin.port().is_none()
        || origin.path() != "/"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || ws.scheme() != "ws"
        || ws.host_str() != origin.host_str()
        || ws.port() != origin.port()
        || ws.path() != expected_path
        || !ws.username().is_empty()
        || ws.password().is_some()
        || ws.query().is_some()
        || ws.fragment().is_some()
    {
        return Err(invalid_response());
    }
    Ok(())
}

fn invalid_response() -> Error {
    Error::Cdp("Moli returned an invalid local CDP endpoint or response".into())
}

fn fetch_json(origin: &str, path: &str, timeout: Duration) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(format!("{origin}{path}"))
        .send()?
        .error_for_status()?;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(invalid_response());
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid_response())
}

fn startup_origin(log: &str) -> Option<String> {
    // tracing may emit SGR escapes even when stderr is a private file. In
    // particular the field name and '=' can be separated by color/reset codes.
    let plain = without_ansi_csi(log);
    for line in plain
        .lines()
        .filter(|line| line.contains("protocol server listening"))
    {
        let Some(after) = line.split("addr=127.0.0.1:").nth(1) else {
            continue;
        };
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(port) = digits.parse::<u16>() {
            if port > 0 {
                return Some(format!("http://127.0.0.1:{port}"));
            }
        }
    }
    None
}

fn without_ansi_csi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' && characters.peek() == Some(&'[') {
            characters.next();
            for parameter in characters.by_ref() {
                if ('@'..='~').contains(&parameter) {
                    break;
                }
            }
        } else {
            result.push(character);
        }
    }
    result
}

fn read_startup(log: &fs::File) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; 64 * 1024];
        let length = log.read_at(&mut bytes, 0)?;
        bytes.truncate(length);
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = log;
        Err(Error::Config(
            "Moli owned process startup is not supported on this platform".into(),
        ))
    }
}

fn find_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("LEXMOUNT_MOLI_BINARY") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(Error::Config(
                "LEXMOUNT_MOLI_BINARY must be an absolute executable path".into(),
            ));
        }
        return executable(&path)
            .ok_or_else(|| Error::Config("Configured Moli executable is unavailable".into()));
    }
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("moli"));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/share/lexmount/moli/moli"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .map(|p| p.join("moli")),
        );
    }
    candidates.iter().find_map(|path| executable(path)).ok_or_else(|| Error::Config("Moli executable is unavailable; install the bundled Moli or set LEXMOUNT_MOLI_BINARY for development".into()))
}

fn executable(path: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_announcement_requires_own_loopback_bound_port() {
        assert_eq!(
            startup_origin("INFO protocol server listening addr=127.0.0.1:43210\n"),
            Some("http://127.0.0.1:43210".into())
        );
        assert_eq!(
            startup_origin(
                "\u{1b}[2m2026-10-19T12:00:00Z\u{1b}[0m \u{1b}[32mINFO\u{1b}[0m protocol server listening \u{1b}[3maddr\u{1b}[0m\u{1b}[2m=\u{1b}[0m127.0.0.1:43210\n"
            ),
            Some("http://127.0.0.1:43210".into())
        );
        for line in [
            "other addr=127.0.0.1:80",
            "protocol server listening addr=0.0.0.0:80",
            "protocol server listening addr=127.0.0.1:0",
            "protocol server listening addr=127.0.0.1:65536",
        ] {
            assert!(startup_origin(line).is_none());
        }
    }
    #[test]
    fn endpoints_cannot_redirect_or_escape_the_owned_loopback_origin() {
        assert!(
            validate_endpoints(
                "http://127.0.0.1:43210",
                "ws://127.0.0.1:43210/devtools/browser/test"
            )
            .is_ok()
        );
        for ws in [
            "ws://127.0.0.1:43211/devtools/browser/test",
            "ws://localhost:43210/devtools/browser/test",
            "ws://127.0.0.1:43210/devtools/browser/test?secret=1",
            "ws://user@127.0.0.1:43210/devtools/browser/test",
            "ws://example.test:43210/devtools/browser/test",
        ] {
            assert!(validate_endpoints("http://127.0.0.1:43210", ws).is_err());
        }
        let id = format!("moli_{:032x}", 3);
        assert_eq!(connection_ref(&id).unwrap(), connection_ref(&id).unwrap());
        assert_ne!(
            connection_ref(&id).unwrap(),
            connection_ref(&format!("moli_{:032x}", 4)).unwrap()
        );
        assert!(connection_ref("session_cloud").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stale_birth_is_closed_and_never_signalled_and_pagination_is_real() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Registry::open(temp.path().join("moli")).unwrap();
        let identity = process::identity(std::process::id()).unwrap();
        for number in 0..3 {
            let mut process = identity.clone();
            if number == 2 {
                process.start_identity.push_str(":stale");
            }
            registry
                .write(&Record {
                    version: 1,
                    session_id: format!("moli_{number:032x}"),
                    process,
                    origin: "http://127.0.0.1:43210".into(),
                    browser_ws: "ws://127.0.0.1:43210/devtools/browser/test".into(),
                    created_at: number,
                    closed: false,
                })
                .unwrap();
        }
        let moli = Moli { registry };
        let first = moli.list(None, Some(1), Some(1)).unwrap();
        assert_eq!(first["pagination"]["totalPages"], 2);
        assert_eq!(first["pagination"]["activeCount"], 2);
        assert_eq!(first["pagination"]["closedCount"], 1);
        let second = moli.list(None, Some(2), Some(1)).unwrap();
        assert_ne!(
            first["sessions"][0]["session_id"],
            second["sessions"][0]["session_id"]
        );
        let stale = format!("moli_{:032x}", 2);
        assert!(moli.browser_ws(&stale).is_err());
        assert_eq!(moli.get(&stale).unwrap()["ws"], Value::Null);
        assert_eq!(moli.close(&stale).unwrap()["already_inactive"], true);
        assert!(process::matches(&identity));
        assert!(moli.list(Some("unknown"), None, None).is_err());
        assert!(moli.list(None, Some(0), None).is_err());
    }
}
