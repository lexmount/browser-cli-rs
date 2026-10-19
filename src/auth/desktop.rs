//! Desktop account selection and read-only credential access. No secret output or fallback.
use super::Credentials;
use crate::{Error, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const SERVICE: &str = "com.lexmount.tab-manager.auth";
pub const ACCOUNT: &str = "active";
const MAX_CREDENTIAL_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Missing,
    Unavailable,
    Invalid,
    Expired,
    Mismatch,
    Unsupported,
}
impl Failure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Missing => "desktop_credentials_missing",
            Self::Unavailable => "desktop_credentials_unavailable",
            Self::Invalid => "desktop_credentials_invalid",
            Self::Expired => "desktop_credentials_expired",
            Self::Mismatch => "desktop_identity_mismatch",
            Self::Unsupported => "desktop_credentials_unsupported",
        }
    }
    pub(crate) fn error(self) -> Error {
        Error::Authentication(self.code().into())
    }
}

pub(crate) trait Reader {
    fn read(&self) -> std::result::Result<Vec<u8>, Failure>;
}
pub(crate) struct SystemReader;
impl Reader for SystemReader {
    fn read(&self) -> std::result::Result<Vec<u8>, Failure> {
        #[cfg(target_os = "macos")]
        {
            macos::read()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Failure::Unsupported)
        }
    }
}
#[cfg(target_os = "macos")]
mod macos;

pub(crate) struct Settings {
    pub explicit_file: Option<PathBuf>,
    pub legacy_file: PathBuf,
    pub marker_file: PathBuf,
    pub forced: bool,
    pub expected: Option<(String, String)>,
}

impl Settings {
    pub(crate) fn from_env(path: Option<&Path>) -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| Error::Config("home directory is unavailable".into()))?;
        let forced = match std::env::var_os("LEXMOUNT_AUTH_SOURCE") {
            None => false,
            Some(value) if value == "desktop" => true,
            Some(_) => return Err(Error::Config("unsupported authentication source".into())),
        };
        let expected = match (
            std::env::var("LEXMOUNT_DESKTOP_EXPECTED_PROJECT_ID").ok(),
            std::env::var("LEXMOUNT_DESKTOP_EXPECTED_API_BASE_URL").ok(),
        ) {
            (None, None) => None,
            (Some(project), Some(base)) if forced && !project.is_empty() => Some((project, base)),
            _ => {
                return Err(Error::Config(
                    "desktop identity requires a complete forced selection".into(),
                ));
            }
        };
        let explicit_file = path
            .map(Path::to_owned)
            .or_else(|| std::env::var_os("LEXMOUNT_BROWSER_CREDENTIALS_FILE").map(PathBuf::from));
        if forced && explicit_file.is_some() {
            return Err(Error::Config(
                "desktop authentication conflicts with an explicit credential file".into(),
            ));
        }
        Ok(Self {
            explicit_file,
            legacy_file: home.join(".config/lexmount/browser-cli/credentials.json"),
            marker_file: home.join(".config/lexmount/browser-cli/desktop-auth.json"),
            forced,
            expected,
        })
    }

    pub(crate) fn desktop_selected(&self) -> Result<bool> {
        if self.forced {
            return Ok(true);
        }
        if self.explicit_file.is_some() {
            return Ok(false);
        }
        #[derive(Deserialize)]
        struct Marker {
            version: u32,
            enabled: bool,
        }
        let metadata = match fs::symlink_metadata(&self.marker_file) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => {
                return Err(Error::Authentication(
                    "desktop_selection_unavailable".into(),
                ));
            }
        };
        if !metadata.is_file() || metadata.len() > 4096 {
            return Err(Error::Authentication("desktop_selection_invalid".into()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
                return Err(Error::Authentication("desktop_selection_invalid".into()));
            }
        }
        let bytes = fs::read(&self.marker_file)
            .map_err(|_| Error::Authentication("desktop_selection_unavailable".into()))?;
        let marker: Marker = serde_json::from_slice(&bytes)
            .map_err(|_| Error::Authentication("desktop_selection_invalid".into()))?;
        if marker.version != 1 {
            return Err(Error::Authentication("desktop_selection_invalid".into()));
        }
        Ok(marker.enabled)
    }
    pub(crate) fn file(&self) -> &Path {
        self.explicit_file.as_deref().unwrap_or(&self.legacy_file)
    }
}

pub(crate) fn read(
    settings: &Settings,
    reader: &impl Reader,
    now: SystemTime,
) -> std::result::Result<Credentials, Failure> {
    let bytes = reader.read()?;
    if bytes.is_empty() || bytes.len() > MAX_CREDENTIAL_BYTES {
        return Err(Failure::Invalid);
    }
    let credential: Credentials = serde_json::from_slice(&bytes).map_err(|_| Failure::Invalid)?;
    if credential.kind != "api_key"
        || credential.api_key.trim().is_empty()
        || credential.project_id.trim().is_empty()
        || credential.project_id.chars().any(char::is_control)
        || official_origin(&credential.api_base_url).is_none()
    {
        return Err(Failure::Invalid);
    }
    if let Some(expiry) = &credential.expires_at {
        let expiry = parse_expiry(expiry).ok_or(Failure::Invalid)?;
        if expiry <= now {
            return Err(Failure::Expired);
        }
    }
    if let Some((project, base)) = &settings.expected {
        if project != &credential.project_id
            || official_origin(base) != official_origin(&credential.api_base_url)
        {
            return Err(Failure::Mismatch);
        }
    }
    Ok(credential)
}

pub(crate) fn official_origin(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("api.lexmount.cn" | "api.lexmount.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

pub(crate) fn client_credentials(
    settings: &Settings,
    reader: &impl Reader,
    api_key: Option<String>,
    project_id: Option<String>,
    base_url: Option<String>,
    now: SystemTime,
) -> Result<(String, String, String)> {
    let explicit = api_key.is_some() || project_id.is_some() || base_url.is_some();
    if settings.forced && explicit {
        return Err(Error::Config(
            "desktop authentication conflicts with explicit identity overrides".into(),
        ));
    }
    // An explicit complete caller identity never probes another account's
    // Keychain, even when its default desktop account has been signed out.
    if let (Some(key), Some(project)) = (&api_key, &project_id) {
        if key.trim().is_empty() || project.trim().is_empty() {
            return Err(Error::Config("explicit credentials are incomplete".into()));
        }
        return Ok((
            key.clone(),
            project.clone(),
            base_url.unwrap_or_else(|| "https://api.lexmount.cn".into()),
        ));
    }
    let credentials = if settings.desktop_selected()? {
        if explicit {
            return Err(Error::Config(
                "desktop authentication conflicts with incomplete identity overrides".into(),
            ));
        }
        Some(read(settings, reader, now).map_err(Failure::error)?)
    } else {
        super::load_file(settings.file())?
            .filter(|c| c.kind == "api_key" && !c.api_key.is_empty() && !c.project_id.is_empty())
    };
    let api_key = api_key
        .or_else(|| credentials.as_ref().map(|c| c.api_key.clone()))
        .ok_or_else(|| {
            Error::Config("LEXMOUNT_API_KEY is not set; run `browser-cli auth login`".into())
        })?;
    let project = project_id
        .or_else(|| credentials.as_ref().map(|c| c.project_id.clone()))
        .ok_or_else(|| {
            Error::Config("LEXMOUNT_PROJECT_ID is not set; run `browser-cli auth login`".into())
        })?;
    let base = base_url
        .or_else(|| credentials.map(|c| c.api_base_url))
        .unwrap_or_else(|| "https://api.lexmount.cn".into());
    Ok((api_key, project, base))
}

// Strict RFC3339 expiration; unsupported/malformed dates fail closed. No local
// timezone dependence, subprocess or new platform-wide date dependency.
fn parse_expiry(value: &str) -> Option<SystemTime> {
    if !value.is_ascii() || value.len() < 20 || value.len() > 128 {
        return None;
    }
    let b = value.as_bytes();
    if b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let number = |start: usize, end: usize| -> Option<u64> {
        let s = &value[start..end];
        if !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let hour = number(11, 13)?;
    let minute = number(14, 16)?;
    let second = number(17, 19)?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > months[(month - 1) as usize] {
        return None;
    }
    let mut index = 19;
    let mut nanos = 0;
    if b.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while b.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return None;
        }
        for (position, digit) in b[start..index].iter().take(9).enumerate() {
            nanos += u32::from(digit - b'0') * 10_u32.pow(8 - position as u32);
        }
    }
    let offset = match b.get(index) {
        Some(b'Z' | b'z') if index + 1 == b.len() => 0_i64,
        Some(sign @ (b'+' | b'-')) if index + 6 == b.len() && b[index + 3] == b':' => {
            let h = number(index + 1, index + 3)?;
            let m = number(index + 4, index + 6)?;
            if h > 23 || m > 59 {
                return None;
            }
            (h * 3600 + m * 60) as i64 * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let leaps_before = |y: u64| (y - 1) / 4 - (y - 1) / 100 + (y - 1) / 400;
    let days = (year - 1970) * 365 + leaps_before(year) - leaps_before(1970)
        + months[..(month - 1) as usize].iter().sum::<u64>()
        + day
        - 1;
    let seconds = (days * 86400 + hour * 3600 + minute * 60 + second) as i64 - offset;
    if seconds < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::new(seconds as u64, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    struct MockReader {
        value: std::result::Result<Vec<u8>, Failure>,
        calls: Cell<usize>,
    }
    impl Reader for MockReader {
        fn read(&self) -> std::result::Result<Vec<u8>, Failure> {
            self.calls.set(self.calls.get() + 1);
            self.value.clone()
        }
    }
    fn mock(value: std::result::Result<Vec<u8>, Failure>) -> MockReader {
        MockReader {
            value,
            calls: Cell::new(0),
        }
    }
    fn credential(project: &str, key: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"kind":"api_key","project_id":project,
            "api_base_url":"https://api.lexmount.com","api_key":key,"scopes":["browser:sessions"]}))
        .unwrap()
    }
    fn settings() -> (tempfile::TempDir, Settings) {
        let directory = tempfile::tempdir().unwrap();
        let legacy_file = directory.path().join("credentials.json");
        let marker_file = directory.path().join("desktop-auth.json");
        fs::write(&legacy_file, credential("old-project", "old-secret")).unwrap();
        fs::write(&marker_file, br#"{"version":1,"enabled":true}"#).unwrap();
        (
            directory,
            Settings {
                explicit_file: None,
                legacy_file,
                marker_file,
                forced: false,
                expected: None,
            },
        )
    }
    fn now() -> SystemTime {
        parse_expiry("2026-10-19T00:00:00Z").unwrap()
    }

    #[test]
    fn enabled_desktop_is_the_default_for_both_load_and_client_without_secret_status() {
        let (_directory, settings) = settings();
        let reader = mock(Ok(credential("desktop-project", "desktop-secret")));
        let result = super::super::load_with_settings(&settings, &reader, now())
            .unwrap()
            .unwrap();
        assert_eq!(result.project_id, "desktop-project");
        let result = client_credentials(&settings, &reader, None, None, None, now()).unwrap();
        assert_eq!(result.0, "desktop-secret");
        assert_eq!(result.1, "desktop-project");
        let status = super::super::desktop_status(&settings, &reader, now());
        assert!(status.valid);
        assert_eq!(status.source, "desktop_keychain");
        assert!(status.path.is_empty());
        let output = serde_json::to_string(&status).unwrap();
        assert!(!output.contains("desktop-secret"));
        assert!(!output.contains("old-secret"));
    }

    #[test]
    fn signed_out_or_denied_desktop_never_loads_or_deletes_the_old_file() {
        let (_directory, settings) = settings();
        for failure in [Failure::Missing, Failure::Unavailable, Failure::Unsupported] {
            let reader = mock(Err(failure));
            let error = super::super::load_with_settings(&settings, &reader, now())
                .unwrap_err()
                .to_string();
            assert!(error.contains(failure.code()));
            assert!(!error.contains("old-secret"));
            assert!(client_credentials(&settings, &reader, None, None, None, now()).is_err());
            let status = super::super::desktop_status(&settings, &reader, now());
            assert!(!status.valid);
            assert_eq!(status.source, "desktop_keychain");
            assert_eq!(status.error_code, Some(failure.code()));
        }
        assert!(
            super::super::ensure_file_settings(&settings).is_err(),
            "Default login is managed in the app, not an unrelated legacy file."
        );
        assert!(super::super::logout_with_settings(&settings).is_err());
        assert!(settings.legacy_file.exists());
        assert!(settings.marker_file.exists());
    }

    #[test]
    fn explicit_development_file_and_complete_sdk_identity_do_not_read_keychain() {
        let (directory, mut settings) = settings();
        let reader = mock(Err(Failure::Unavailable));
        let explicit = directory.path().join("development.json");
        fs::write(&explicit, credential("dev-project", "dev-secret")).unwrap();
        settings.explicit_file = Some(explicit.clone());
        assert_eq!(
            super::super::load_with_settings(&settings, &reader, now())
                .unwrap()
                .unwrap()
                .project_id,
            "dev-project"
        );
        assert_eq!(
            client_credentials(&settings, &reader, None, None, None, now())
                .unwrap()
                .0,
            "dev-secret"
        );
        assert!(super::super::logout_with_settings(&settings).unwrap());
        assert!(!explicit.exists());
        assert!(settings.legacy_file.exists());
        settings.explicit_file = None;
        let result = client_credentials(
            &settings,
            &reader,
            Some("sdk-secret".into()),
            Some("sdk-project".into()),
            Some("http://127.0.0.1:1234".into()),
            now(),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                "sdk-secret".to_owned(),
                "sdk-project".to_owned(),
                "http://127.0.0.1:1234".to_owned()
            )
        );
        assert_eq!(reader.calls.get(), 0);
    }

    #[test]
    fn incomplete_identity_or_host_override_cannot_mix_with_desktop_account() {
        let (_directory, settings) = settings();
        let reader = mock(Ok(credential("desktop-project", "desktop-secret")));
        for (key, project, base) in [
            (Some("other-key".into()), None, None),
            (None, Some("other-project".into()), None),
            (None, None, Some("https://elsewhere.example".into())),
        ] {
            assert!(client_credentials(&settings, &reader, key, project, base, now()).is_err());
        }
        assert_eq!(
            reader.calls.get(),
            0,
            "Reject mixed identity before requesting another account's secret."
        );
    }

    #[test]
    fn forced_desktop_requires_matching_complete_identity_and_rejects_overrides() {
        let (_directory, mut settings) = settings();
        settings.forced = true;
        fs::remove_file(&settings.marker_file).unwrap();
        settings.expected = Some(("desktop-project".into(), "https://api.lexmount.com/".into()));
        let reader = mock(Ok(credential("desktop-project", "desktop-secret")));
        assert!(read(&settings, &reader, now()).is_ok());
        assert!(
            client_credentials(
                &settings,
                &reader,
                Some("x".into()),
                Some("y".into()),
                None,
                now()
            )
            .is_err()
        );
        for (project, base) in [
            ("other-project", "https://api.lexmount.com"),
            ("desktop-project", "https://api.lexmount.cn"),
            ("desktop-project", "https://api.lexmount.com/not-origin"),
        ] {
            settings.expected = Some((project.into(), base.into()));
            assert_eq!(
                read(&settings, &reader, now()).err(),
                Some(Failure::Mismatch)
            );
        }
    }

    #[test]
    fn invalid_or_expired_keychain_values_fail_closed_with_fixed_errors() {
        let (_directory, settings) = settings();
        for (field, value, failure) in [
            ("expires_at", "2026-10-18T23:59:59Z", Failure::Expired),
            ("expires_at", "2026-10-19T00:00:00Z", Failure::Expired),
            ("expires_at", "bad secret date", Failure::Invalid),
            ("kind", "not-api-key", Failure::Invalid),
            ("project_id", "", Failure::Invalid),
            (
                "api_base_url",
                "https://api.lexmount.com@untrusted.example",
                Failure::Invalid,
            ),
            (
                "api_base_url",
                "https://api.lexmount.com/?token=secret",
                Failure::Invalid,
            ),
        ] {
            let mut value_json: serde_json::Value =
                serde_json::from_slice(&credential("project", "secret-value")).unwrap();
            value_json[field] = json!(value);
            let reader = mock(Ok(serde_json::to_vec(&value_json).unwrap()));
            assert_eq!(read(&settings, &reader, now()).err(), Some(failure));
            let output =
                serde_json::to_string(&super::super::desktop_status(&settings, &reader, now()))
                    .unwrap();
            assert!(!output.contains("secret"));
        }
        for bytes in [
            b"{secret malformed".to_vec(),
            vec![0; MAX_CREDENTIAL_BYTES + 1],
        ] {
            assert_eq!(
                read(&settings, &mock(Ok(bytes)), now()).err(),
                Some(Failure::Invalid)
            );
        }
    }

    #[test]
    fn marker_absent_or_disabled_preserves_file_mode_but_invalid_marker_cannot_downgrade() {
        let (_directory, mut settings) = settings();
        let reader = mock(Err(Failure::Missing));
        for marker in [None, Some(br#"{"version":1,"enabled":false}"#.as_slice())] {
            if let Some(bytes) = marker {
                fs::write(&settings.marker_file, bytes).unwrap();
            } else {
                fs::remove_file(&settings.marker_file).unwrap();
            }
            assert_eq!(
                super::super::load_with_settings(&settings, &reader, now())
                    .unwrap()
                    .unwrap()
                    .project_id,
                "old-project"
            );
        }
        for bytes in [
            br#"{"version":2,"enabled":true}"#.as_slice(),
            br#"{"version":1}"#,
            b"not-json",
        ] {
            fs::write(&settings.marker_file, bytes).unwrap();
            assert!(super::super::load_with_settings(&settings, &reader, now()).is_err());
        }
        settings.explicit_file = Some(settings.legacy_file.clone());
        assert!(super::super::load_with_settings(&settings, &reader, now()).is_ok());
        assert_eq!(reader.calls.get(), 0);
    }

    #[test]
    fn expiration_parser_handles_offsets_fractional_boundaries_and_calendar_validation() {
        assert_eq!(parse_expiry("1970-01-01T00:00:00Z"), Some(UNIX_EPOCH));
        assert_eq!(parse_expiry("2026-10-19T08:00:00+08:00"), Some(now()));
        assert_eq!(parse_expiry("2026-10-18T20:00:00-04:00"), Some(now()));
        assert_eq!(
            parse_expiry("2026-10-19T00:00:00.123456789Z"),
            Some(now() + Duration::new(0, 123_456_789))
        );
        assert_eq!(
            parse_expiry("2024-02-29T00:00:00Z").unwrap() + Duration::from_secs(86400),
            parse_expiry("2024-03-01T00:00:00Z").unwrap()
        );
        for value in [
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-10-19T24:00:00Z",
            "2026-10-19T00:00:00.Z",
            "2026-10-19T00:00:00+24:00",
            "2026-10-19T00:00:00Zextra",
        ] {
            assert!(
                parse_expiry(value).is_none(),
                "Malformed expiration must not grant a non-expiring account."
            );
        }
    }
}
