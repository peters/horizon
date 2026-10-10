//! Protected credential records, one per issued client ID, under the cloud root.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const DIRECTORY: &str = "chatgpt";
const HOST_ID_FILE: &str = "host_id";
const ACTIVE_FILE: &str = "active";

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

/// What the flow stores for one issued client ID and its verified account.
#[derive(Debug)]
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

impl Record {
    pub fn plan_usage(&self) -> bool {
        self.scopes.iter().any(|scope| scope == "chatgpt.tokens.use.direct")
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    email: Option<String>,
    issuer: String,
    subject: String,
    client_id: String,
    ext_agent_host_id: String,
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    earliest_refresh_at: Option<i64>,
    scopes: Vec<String>,
    #[serde(default)]
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
            signed_in: record.access_token.is_some() && record.refresh_token.is_some(),
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

fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
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
    let (_, temp_path) = temp.keep().map_err(|error| error.error)?;
    fs::rename(&temp_path, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()?.trim().to_string().into()
}

/// The stable host identifier, created on first use and reused for every sign-in.
/// # Errors
/// The host ID file could not be read or created.
pub(super) fn host_id(root: &Path) -> Result<String> {
    let path = directory(root).join(HOST_ID_FILE);
    if let Some(id) = read_text(&path)
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
    let Ok(bytes) = fs::read(path) else {
        return Ok(None);
    };
    let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
    if stored.version != 1 {
        return Err(Error::Malformed);
    }
    Ok(Some(Record {
        email: stored.email,
        issuer: stored.issuer,
        subject: stored.subject,
        client_id: stored.client_id,
        ext_agent_host_id: stored.ext_agent_host_id,
        id_token: Zeroizing::new(stored.id_token.unwrap_or_default()),
        access_token: stored.access_token.map(Zeroizing::new),
        refresh_token: stored.refresh_token.map(Zeroizing::new),
        token_type: stored.token_type,
        expires_in: stored.expires_in,
        earliest_refresh_at: stored.earliest_refresh_at,
        scopes: stored.scopes,
        usage_confirmed: stored.usage_confirmed,
        saved_at_unix: stored.saved_at_unix,
    }))
}

fn encode(record: &Record) -> Result<Vec<u8>> {
    let stored = Stored {
        version: 1,
        email: record.email.clone(),
        issuer: record.issuer.clone(),
        subject: record.subject.clone(),
        client_id: record.client_id.clone(),
        ext_agent_host_id: record.ext_agent_host_id.clone(),
        id_token: (!record.id_token.is_empty()).then(|| record.id_token.to_string()),
        access_token: record.access_token.as_ref().map(|token| token.to_string()),
        refresh_token: record.refresh_token.as_ref().map(|token| token.to_string()),
        token_type: record.token_type.clone(),
        expires_in: record.expires_in,
        earliest_refresh_at: record.earliest_refresh_at,
        scopes: record.scopes.clone(),
        usage_confirmed: record.usage_confirmed,
        saved_at_unix: record.saved_at_unix,
    };
    serde_json::to_vec_pretty(&stored).map_err(|_| Error::Malformed)
}

/// Saves one registration, replacing any file with the same client ID.
/// # Errors
/// The record could not be written.
pub(super) fn save(root: &Path, record: &Record) -> Result<()> {
    private_file(&file(root, &record.client_id), &encode(record)?)?;
    Ok(())
}

fn active_client_id(root: &Path) -> Option<String> {
    read_text(&directory(root).join(ACTIVE_FILE))
}

pub(super) fn set_active(root: &Path, client_id: &str) -> Result<()> {
    private_file(&directory(root).join(ACTIVE_FILE), client_id.as_bytes())
}

/// Every saved registration, newest first.
/// # Errors
/// A connection file was malformed.
pub(super) fn connections(root: &Path) -> Result<Vec<Connection>> {
    Ok(read_records(root)?.into_iter().map(Connection::from).collect())
}

/// The registration this host signs in as by default: the active one, else the most
/// recently saved one.
/// # Errors
/// A connection file was malformed.
pub(super) fn default_registration(root: &Path) -> Result<Option<Record>> {
    let mut found = read_records(root)?;
    found.sort_by_key(|record| std::cmp::Reverse(record.saved_at_unix));
    let active = active_client_id(root);
    let index = active
        .as_ref()
        .and_then(|id| found.iter().position(|record| &record.client_id == id));
    Ok(index.map(|index| found.remove(index)).or_else(|| found.pop()))
}

fn read_records(root: &Path) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let Some(entries) = fs::read_dir(directory(root)).ok() else {
        return Ok(records);
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.to_ascii_lowercase().ends_with(".json") {
            continue;
        }
        if let Some(record) = read_record(&entry.path())? {
            records.push(record);
        }
    }
    Ok(records)
}

/// Marks the first-sign-in plan-usage confirmation as shown.
/// # Errors
/// The registration could not be read or written.
pub(super) fn confirm_usage(root: &Path, client_id: &str) -> Result<()> {
    let mut record = read_record(&file(root, client_id))?.ok_or(Error::Missing)?;
    record.usage_confirmed = true;
    save(root, &record)
}

/// Clears the tokens and retained ID token of one registration, keeping the account
/// and client mapping for a later sign-in.
/// # Errors
/// The registration could not be read or written.
pub(super) fn clear_tokens(root: &Path, client_id: &str) -> Result<()> {
    let mut record = read_record(&file(root, client_id))?.ok_or(Error::Missing)?;
    record.access_token = None;
    record.refresh_token = None;
    record.token_type = None;
    record.expires_in = None;
    record.earliest_refresh_at = None;
    record.id_token = Zeroizing::new(String::new());
    record.saved_at_unix = now_unix();
    save(root, &record)?;
    if active_client_id(root).as_deref() == Some(client_id) {
        // The pointer file holds no secret; replacing it with an empty write drops it.
        let path = directory(root).join(ACTIVE_FILE);
        let _ = fs::remove_file(&path);
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
    let mut record = read_record(&file(root, client_id))?.ok_or(Error::Missing)?;
    record.access_token = Some(Zeroizing::new(access_token.to_owned()));
    record.refresh_token = Some(Zeroizing::new(refresh_token.to_owned()));
    record.token_type = Some("Bearer".into());
    record.expires_in = Some(expires_in);
    record.earliest_refresh_at = earliest_refresh_at;
    record.scopes = scopes;
    record.saved_at_unix = now_unix();
    save(root, &record)
}

fn now_unix() -> i64 {
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
        assert!(active_client_id(root.path()).is_none());
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
}
