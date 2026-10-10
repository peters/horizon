//! Protected credential records, one per issued client ID, under the cloud root.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const DIRECTORY: &str = "chatgpt";
const HOST_ID_FILE: &str = "host_id";
const ACTIVE_FILE: &str = "active";
/// A record holds one token set; anything larger is not one of ours.
const MAX_RECORD: u64 = 64 * 1024;

/// A saved registration the settings card can show; it never carries tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub client_id: String,
    pub email: Option<String>,
    pub subject: String,
    pub scopes: Vec<String>,
    /// Whether the granted scopes include `chatgpt.tokens.use.direct`.
    pub plan_usage: bool,
    /// Whether the first-sign-in plan-usage confirmation has been shown.
    pub usage_confirmed: bool,
    /// Whether a usable access and refresh token pair is stored.
    pub signed_in: bool,
    pub saved_at_unix: i64,
}

impl Connection {
    /// Whether a renewable sign-in also grants access to the account's plan.
    #[must_use]
    pub const fn can_use_plan(&self) -> bool {
        self.signed_in && self.plan_usage
    }
}

/// What the flow stores for one issued client ID and its verified account.
pub struct Record {
    pub email: Option<String>,
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub ext_agent_host_id: String,
    pub id_token: Zeroizing<String>,
    pub access_token: Option<Zeroizing<String>>,
    pub refresh_token: Option<Zeroizing<String>>,
    pub token_type: Option<String>,
    pub expires_in: Option<u64>,
    pub earliest_refresh_at: Option<i64>,
    pub scopes: Vec<String>,
    pub usage_confirmed: bool,
    pub saved_at_unix: i64,
}

impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("email", &self.email)
            .field("issuer", &self.issuer)
            .field("subject", &self.subject)
            .field("client_id", &self.client_id)
            .field("ext_agent_host_id", &self.ext_agent_host_id)
            .field("id_token", &"<redacted>")
            .field("access_token", &self.access_token.as_ref().map(|_| "<redacted>"))
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field("earliest_refresh_at", &self.earliest_refresh_at)
            .field("scopes", &self.scopes)
            .field("usage_confirmed", &self.usage_confirmed)
            .field("saved_at_unix", &self.saved_at_unix)
            .finish()
    }
}

impl Record {
    pub fn plan_usage(&self) -> bool {
        self.scopes.iter().any(|scope| scope == "chatgpt.tokens.use.direct")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    email: Option<String>,
    issuer: String,
    subject: String,
    client_id: String,
    ext_agent_host_id: String,
    #[serde(default, deserialize_with = "protected_token")]
    id_token: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "protected_token")]
    access_token: Option<Zeroizing<String>>,
    #[serde(default, deserialize_with = "protected_token")]
    refresh_token: Option<Zeroizing<String>>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    earliest_refresh_at: Option<i64>,
    scopes: Vec<String>,
    #[serde(default)]
    usage_confirmed: bool,
    saved_at_unix: i64,
}

pub(super) fn protected_token<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Zeroizing<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(|token| token.map(Zeroizing::new))
}

pub(super) fn protected_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

/// Serialize operations which can replace a rotating session's token set.
pub(super) fn session_lock(root: &Path) -> Result<fs::File> {
    let directory = directory(root);
    private_directory(&directory)?;
    let path = directory.join("session.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(Error::Invalid("the session lock must be a regular file, not a link"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid("the session lock must be a regular file"));
    }
    file.try_lock()
        .map_err(|_| Error::Invalid("another sign-in operation is in progress"))?;
    Ok(file)
}

/// The serialized form, borrowing the record's tokens so the only encoding happens
/// directly into wiped memory.
#[derive(Serialize)]
struct StoredFields<'a> {
    version: u32,
    email: Option<&'a str>,
    issuer: &'a str,
    subject: &'a str,
    client_id: &'a str,
    ext_agent_host_id: &'a str,
    id_token: Option<&'a str>,
    access_token: Option<&'a str>,
    refresh_token: Option<&'a str>,
    token_type: Option<&'a str>,
    expires_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    earliest_refresh_at: Option<i64>,
    scopes: &'a [String],
    usage_confirmed: bool,
    saved_at_unix: i64,
}

impl From<Record> for Connection {
    fn from(record: Record) -> Self {
        let plan_usage = record.plan_usage();
        Self {
            client_id: record.client_id,
            email: record.email,
            subject: record.subject,
            scopes: record.scopes,
            plan_usage,
            usage_confirmed: record.usage_confirmed,
            signed_in: record.access_token.as_ref().is_some_and(|token| !token.is_empty())
                && record.refresh_token.as_ref().is_some_and(|token| !token.is_empty()),
            saved_at_unix: record.saved_at_unix,
        }
    }
}

fn directory(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

fn file(root: &Path, client_id: &str) -> PathBuf {
    directory(root).join(format!("{client_id}.json"))
}

/// A client ID is used in file names, and it comes from the sign-in service; only a
/// conservative set of characters is safe in a file name.
pub(super) fn valid_client_id(client_id: &str) -> bool {
    (1..=200).contains(&client_id.len())
        && !matches!(
            client_id.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
        && client_id
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && client_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn directory_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(Error::Invalid(
            "the credential directory must be a directory, not a link",
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn private_directory(path: &Path) -> Result<()> {
    directory_exists(path)?;
    fs::create_dir_all(path)?;
    directory_exists(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `bytes` to `path` atomically with owner-only permissions.
fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(directory) = path.parent() else {
        return Err(Error::Invalid("the credential path has no parent directory"));
    };
    private_directory(directory)?;
    let mut temp = tempfile::Builder::new().prefix(".chatgpt-").tempfile_in(directory)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o600))?;
    }
    temp.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
}

/// The bytes of a private record file, or `None` when it does not exist. A file that
/// is a link, that others could read, or that is not a small regular file is refused.
fn read_private(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let parent = path
        .parent()
        .ok_or(Error::Invalid("the credential path has no parent directory"))?;
    if !directory_exists(parent)? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::Invalid("a credential file must be a regular file, not a link"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        if metadata.permissions().mode() & 0o077 != 0 || metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err(Error::Invalid(
                "a credential file must be private and owned by this user",
            ));
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_RECORD {
        return Err(Error::Invalid("a credential file is not a small private file"));
    }
    // The fixed bound also covers a file that grows after the metadata check.
    let capacity = usize::try_from(MAX_RECORD + 1).map_err(|_| Error::Malformed)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(capacity));
    file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
    if bytes.len() >= capacity {
        return Err(Error::Invalid("the credential file is too large"));
    }
    Ok(Some(bytes))
}

fn read_text(path: &Path) -> Result<Option<String>> {
    read_private(path)?
        .map(|bytes| {
            std::str::from_utf8(&bytes)
                .map(|text| text.trim().to_owned())
                .map_err(|_| Error::Malformed)
        })
        .transpose()
}

/// Serializes into a wiped buffer of exactly its size: the output is counted first, so
/// no reallocation frees an unwiped copy of a credential in it.
fn serialized(write: impl Fn(&mut dyn std::io::Write) -> serde_json::Result<()>) -> Result<Zeroizing<Vec<u8>>> {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    write(&mut count).map_err(|_| Error::Malformed)?;
    if u64::try_from(count.0).map_err(|_| Error::Malformed)? > MAX_RECORD {
        return Err(Error::Invalid("the credential record is too large"));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(count.0));
    write(&mut *bytes).map_err(|_| Error::Malformed)?;
    Ok(bytes)
}

/// The stable host identifier, created on first use and reused for every sign-in.
/// # Errors
/// The host ID file could not be read or created.
pub(super) fn host_id(root: &Path) -> Result<String> {
    let _lock = session_lock(root)?;
    let path = directory(root).join(HOST_ID_FILE);
    if let Some(id) = read_text(&path)?
        && (id.starts_with("urn:uuid:")
            || id.starts_with("urn:ietf:params:oauth:jwk-thumbprint:")
            || id.starts_with("did:key:"))
    {
        return Ok(id);
    }
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4().hyphenated());
    private_file(&path, id.as_bytes())?;
    Ok(id)
}

fn read_record(path: &Path) -> Result<Option<Record>> {
    let Some(bytes) = read_private(path)? else {
        return Ok(None);
    };
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
    if stored.version != 1
        || !valid_client_id(&stored.client_id)
        || path.file_stem().and_then(|name| name.to_str()) != Some(stored.client_id.as_str())
    {
        return Err(Error::Malformed);
    }
    Ok(Some(Record {
        email: stored.email,
        issuer: stored.issuer,
        subject: stored.subject,
        client_id: stored.client_id,
        ext_agent_host_id: stored.ext_agent_host_id,
        id_token: stored.id_token.unwrap_or_default(),
        access_token: stored.access_token,
        refresh_token: stored.refresh_token,
        token_type: stored.token_type,
        expires_in: stored.expires_in,
        earliest_refresh_at: stored.earliest_refresh_at,
        scopes: stored.scopes,
        usage_confirmed: stored.usage_confirmed,
        saved_at_unix: stored.saved_at_unix,
    }))
}

fn encode(record: &Record) -> Result<Zeroizing<Vec<u8>>> {
    let fields = StoredFields {
        version: 1,
        email: record.email.as_deref(),
        issuer: &record.issuer,
        subject: &record.subject,
        client_id: &record.client_id,
        ext_agent_host_id: &record.ext_agent_host_id,
        id_token: (!record.id_token.is_empty()).then(|| &record.id_token[..]),
        access_token: record.access_token.as_ref().map(|token| &token[..]),
        refresh_token: record.refresh_token.as_ref().map(|token| &token[..]),
        token_type: record.token_type.as_deref(),
        expires_in: record.expires_in,
        earliest_refresh_at: record.earliest_refresh_at,
        scopes: &record.scopes,
        usage_confirmed: record.usage_confirmed,
        saved_at_unix: record.saved_at_unix,
    };
    serialized(|writer| serde_json::to_writer_pretty(writer, &fields))
}

/// Saves one registration, replacing any file with the same client ID.
/// # Errors
/// The client ID is not safe in a file name, or the record could not be written.
pub(super) fn save(root: &Path, record: &Record) -> Result<()> {
    if !valid_client_id(&record.client_id) {
        return Err(Error::Provider(
            "ChatGPT returned an unusable client ID. Try again.".into(),
        ));
    }
    private_file(&file(root, &record.client_id), &encode(record)?)?;
    Ok(())
}

/// Reads the requested account mapping even when the active account changed.
pub(super) fn registration(root: &Path, client_id: &str) -> Result<Option<Record>> {
    if !valid_client_id(client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    read_record(&file(root, client_id))
}

fn active_client_id(root: &Path) -> Result<Option<String>> {
    let active = read_text(&directory(root).join(ACTIVE_FILE))?;
    if active.as_deref().is_some_and(|id| !valid_client_id(id)) {
        return Err(Error::Malformed);
    }
    Ok(active)
}

pub(super) fn set_active(root: &Path, client_id: &str) -> Result<()> {
    if !valid_client_id(client_id) {
        return Err(Error::Provider(
            "ChatGPT returned an unusable client ID. Try again.".into(),
        ));
    }
    private_file(&directory(root).join(ACTIVE_FILE), client_id.as_bytes())
}

/// Every saved registration, newest first.
/// # Errors
/// A connection file was malformed.
pub(super) fn connections(root: &Path) -> Result<Vec<Connection>> {
    let mut found = read_records(root)?;
    found.sort_by_key(|record| std::cmp::Reverse(record.saved_at_unix));
    Ok(found.into_iter().map(Connection::from).collect())
}

/// The registration this host signs in as by default: the active one, else the most
/// recently saved one.
/// # Errors
/// A connection file was malformed.
pub(super) fn default_registration(root: &Path) -> Result<Option<Record>> {
    let mut found = read_records(root)?;
    found.sort_by_key(|record| std::cmp::Reverse(record.saved_at_unix));
    let active = active_client_id(root)?;
    let index = active
        .as_ref()
        .and_then(|id| found.iter().position(|record| &record.client_id == id));
    Ok(index
        .map(|index| found.remove(index))
        .or_else(|| found.into_iter().next()))
}

fn read_records(root: &Path) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let directory = directory(root);
    if !directory_exists(&directory)? {
        return Ok(records);
    }
    let entries = fs::read_dir(directory)?;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.to_ascii_lowercase().ends_with(".json") {
            continue;
        }
        if let Some(record) = read_record(&entry.path())? {
            if name != format!("{}.json", record.client_id) {
                return Err(Error::Malformed);
            }
            records.push(record);
        }
    }
    Ok(records)
}

/// Marks the first-sign-in plan-usage confirmation as shown.
/// # Errors
/// The registration could not be read or written.
pub(super) fn confirm_usage(root: &Path, client_id: &str) -> Result<()> {
    let _lock = session_lock(root)?;
    if !valid_client_id(client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    let mut record = registration(root, client_id)?.ok_or(Error::Missing)?;
    record.usage_confirmed = true;
    save(root, &record)
}

/// Clears the tokens and retained ID token of one registration, keeping the account
/// and client mapping for a later sign-in.
/// # Errors
/// The registration could not be read or written.
pub(super) fn clear_tokens(root: &Path, client_id: &str) -> Result<()> {
    if !valid_client_id(client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    let mut record = registration(root, client_id)?.ok_or(Error::Missing)?;
    record.access_token = None;
    record.refresh_token = None;
    record.token_type = None;
    record.expires_in = None;
    record.earliest_refresh_at = None;
    record.id_token = Zeroizing::new(String::new());
    record.saved_at_unix = now_unix();
    save(root, &record)?;
    if active_client_id(root)?.as_deref() == Some(client_id) {
        // The pointer file holds no secret; replacing it with an empty write drops it.
        let path = directory(root).join(ACTIVE_FILE);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Replaces the stored tokens of one registration with a refreshed set.
/// # Errors
/// The registration could not be read or written.
pub(super) fn replace_tokens(
    root: &Path,
    client_id: &str,
    access_token: &str,
    refresh_token: &str,
    expires_in: u64,
    earliest_refresh_at: Option<i64>,
    scopes: Vec<String>,
) -> Result<()> {
    if !valid_client_id(client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    let mut record = registration(root, client_id)?.ok_or(Error::Missing)?;
    record.access_token = Some(Zeroizing::new(access_token.to_owned()));
    record.refresh_token = Some(Zeroizing::new(refresh_token.to_owned()));
    record.token_type = Some("Bearer".into());
    record.expires_in = Some(expires_in);
    record.earliest_refresh_at = earliest_refresh_at;
    record.scopes = scopes;
    record.saved_at_unix = now_unix();
    save(root, &record)
}

pub(super) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| i64::try_from(age.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_record(client_id: &str, subject: &str) -> Record {
        Record {
            email: Some("peters@example.com".into()),
            issuer: "https://auth.openai.com".into(),
            subject: subject.into(),
            client_id: client_id.into(),
            ext_agent_host_id: "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
            id_token: Zeroizing::new("id-token".into()),
            access_token: Some(Zeroizing::new("access".into())),
            refresh_token: Some(Zeroizing::new("refresh".into())),
            token_type: Some("Bearer".into()),
            expires_in: Some(3600),
            earliest_refresh_at: None,
            scopes: vec!["openid".into(), "chatgpt.tokens.use.direct".into()],
            usage_confirmed: false,
            saved_at_unix: now_unix(),
        }
    }

    #[test]
    fn a_registration_keeps_its_account_when_another_account_becomes_active() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &test_record("client-a", "account-a")).unwrap();
        save(root.path(), &test_record("client-b", "account-b")).unwrap();
        set_active(root.path(), "client-b").unwrap();
        assert_eq!(
            registration(root.path(), "client-a").unwrap().unwrap().subject,
            "account-a"
        );
        assert_eq!(default_registration(root.path()).unwrap().unwrap().subject, "account-b");
    }

    #[test]
    fn host_id_is_created_once_and_reused() {
        let root = tempfile::tempdir().unwrap();
        let first = host_id(root.path()).unwrap();
        let second = host_id(root.path()).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("urn:uuid:"));
        assert_eq!(first.len(), "urn:uuid:".len() + 36);
    }

    #[test]
    fn a_saved_record_round_trips_as_the_default_registration() {
        let root = tempfile::tempdir().unwrap();
        let record = test_record("oaiapp_one", "user-1");
        save(root.path(), &record).unwrap();
        let loaded = default_registration(root.path()).unwrap().unwrap();
        assert_eq!(loaded.client_id, "oaiapp_one");
        assert_eq!(loaded.subject, "user-1");
        assert_eq!(loaded.email.as_deref(), Some("peters@example.com"));
        assert!(loaded.plan_usage());
        assert!(loaded.access_token.is_some());
    }

    #[test]
    fn signed_in_requires_a_renewable_token_pair() {
        let root = tempfile::tempdir().unwrap();
        let mut record = test_record("oaiapp_one", "user-1");
        record.refresh_token = None;
        save(root.path(), &record).unwrap();
        let loaded = default_registration(root.path()).unwrap().unwrap();
        assert!(
            !Connection::from(loaded).signed_in,
            "an access token alone is not a usable connection"
        );
    }

    #[test]
    fn an_unsafe_client_id_is_refused_before_any_file_io() {
        let root = tempfile::tempdir().unwrap();
        for client_id in [
            "../../escape",
            "oaiapp/one",
            "",
            "-leading",
            ".dot",
            "C:escape",
            "one:stream",
            "CON",
            "nul",
            "LPT1",
            "oaiapp\\one",
        ] {
            let mut record = test_record("oaiapp_one", "user-1");
            record.client_id = client_id.into();
            assert!(
                save(root.path(), &record).is_err(),
                "{client_id:?} must not reach the filesystem"
            );
        }
        assert!(
            !directory(root.path()).is_dir(),
            "no credential directory is created for a refused client ID"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_credential_directory_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), directory(root.path())).unwrap();
        assert!(save(root.path(), &test_record("oaiapp_one", "user-1")).is_err());
        assert_eq!(
            target.path().read_dir().unwrap().count(),
            0,
            "nothing may land in the link target"
        );
    }

    #[test]
    fn the_active_pointer_wins_over_newer_records() {
        let root = tempfile::tempdir().unwrap();
        let first = test_record("oaiapp_one", "user-1");
        save(root.path(), &first).unwrap();
        let mut second = test_record("oaiapp_two", "user-2");
        second.saved_at_unix = now_unix() + 60;
        save(root.path(), &second).unwrap();
        set_active(root.path(), "oaiapp_one").unwrap();
        let loaded = default_registration(root.path()).unwrap().unwrap();
        assert_eq!(loaded.client_id, "oaiapp_one");
    }

    #[test]
    fn connections_are_newest_first() {
        let root = tempfile::tempdir().unwrap();
        let mut first = test_record("oaiapp_one", "user-1");
        first.saved_at_unix = 100;
        save(root.path(), &first).unwrap();
        let mut second = test_record("oaiapp_two", "user-2");
        second.saved_at_unix = 200;
        save(root.path(), &second).unwrap();
        let names: Vec<_> = connections(root.path())
            .unwrap()
            .into_iter()
            .map(|connection| connection.client_id)
            .collect();
        assert_eq!(names, vec!["oaiapp_two", "oaiapp_one"]);
    }

    #[test]
    fn sign_out_keeps_the_registration_but_clears_its_tokens() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &test_record("oaiapp_one", "user-1")).unwrap();
        set_active(root.path(), "oaiapp_one").unwrap();
        clear_tokens(root.path(), "oaiapp_one").unwrap();
        let loaded = default_registration(root.path()).unwrap().unwrap();
        assert_eq!(loaded.client_id, "oaiapp_one");
        assert!(loaded.access_token.is_none());
        assert!(loaded.refresh_token.is_none());
        assert!(loaded.id_token.is_empty());
        assert!(active_client_id(root.path()).unwrap().is_none());
    }

    #[test]
    fn confirm_usage_sets_its_flag() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &test_record("oaiapp_one", "user-1")).unwrap();
        confirm_usage(root.path(), "oaiapp_one").unwrap();
        let connection = default_registration(root.path())
            .unwrap()
            .map(Connection::from)
            .unwrap();
        assert!(connection.usage_confirmed);
    }

    #[test]
    fn records_are_written_owner_only() {
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &test_record("oaiapp_one", "user-1")).unwrap();
        let file = directory(root.path()).join("oaiapp_one.json");
        let metadata = fs::metadata(&file).unwrap();
        let permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(permissions.mode() & 0o777, 0o600);
        }
        #[cfg(not(unix))]
        {
            let _ = permissions;
        }
    }
    #[test]
    fn diagnostic_output_redacts_every_token() {
        let record = test_record("oaiapp_one", "user-1");
        let debug = format!("{record:?}");
        for secret in [
            &*record.id_token,
            &**record.access_token.as_ref().unwrap(),
            &**record.refresh_token.as_ref().unwrap(),
        ] {
            assert!(
                !debug.contains(&format!("\"{secret}\"")),
                "tokens must not appear in diagnostics"
            );
        }
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn without_an_active_pointer_the_newest_registration_is_selected() {
        let root = tempfile::tempdir().unwrap();
        for (client_id, saved_at) in [("oaiapp_old", 100), ("oaiapp_new", 300), ("oaiapp_middle", 200)] {
            let mut record = test_record(client_id, "user-1");
            record.saved_at_unix = saved_at;
            save(root.path(), &record).unwrap();
        }
        assert_eq!(
            default_registration(root.path()).unwrap().unwrap().client_id,
            "oaiapp_new"
        );
    }

    #[test]
    fn oversized_and_non_regular_records_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let path = file(root.path(), "oaiapp_one");
        private_file(&path, &vec![b' '; usize::try_from(MAX_RECORD + 1).unwrap()]).unwrap();
        assert!(matches!(read_record(&path), Err(Error::Invalid(_))));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(read_record(&path), Err(Error::Invalid(_))));
        assert!(read_record(&file(root.path(), "oaiapp_missing")).unwrap().is_none());
    }

    #[test]
    fn a_record_cannot_claim_a_different_client_than_its_file() {
        let root = tempfile::tempdir().unwrap();
        let record = test_record("oaiapp_one", "user-1");
        private_file(&file(root.path(), "oaiapp_other"), &encode(&record).unwrap()).unwrap();
        assert!(matches!(connections(root.path()), Err(Error::Malformed)));
    }

    #[test]
    fn session_mutations_are_serialized() {
        let root = tempfile::tempdir().unwrap();
        let lock = session_lock(root.path()).unwrap();
        assert!(session_lock(root.path()).is_err());
        drop(lock);
        assert!(session_lock(root.path()).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_and_public_credential_files_are_rejected() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let root = tempfile::tempdir().unwrap();
        save(root.path(), &test_record("oaiapp_one", "user-1")).unwrap();
        let path = file(root.path(), "oaiapp_one");
        let link = file(root.path(), "oaiapp_link");
        symlink(&path, &link).unwrap();
        assert!(matches!(read_record(&link), Err(Error::Invalid(_))));
        fs::remove_file(&link).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(read_record(&path), Err(Error::Invalid(_))));
    }

    #[test]
    #[cfg(unix)]
    fn reading_a_symlinked_credential_directory_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        save(target.path(), &test_record("oaiapp_one", "user-1")).unwrap();
        std::os::unix::fs::symlink(directory(target.path()), directory(root.path())).unwrap();
        assert!(matches!(connections(root.path()), Err(Error::Invalid(_))));
    }
}
