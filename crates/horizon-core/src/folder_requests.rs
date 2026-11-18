//! The channel of `horizon <folder>` to a Horizon that already runs of this home.
//!
//! A second start cannot reach the first window in memory, so it writes one
//! request into the home and a running window reads it. The record names one
//! folder and one verb, `show`, and nothing else: no command, no preset, no
//! argument and no text of a request reaches a command line. The window applies
//! the folder through the function that a start with that folder uses, so a
//! request opens the workspace of the folder or makes it with one panel, as
//! [`docs/review/trust-boundaries/folder-requests.md`] states.
//!
//! The folder is `<home>/runtime/folder-requests/`, mode `0700`. A request is
//! `<id>.request`, an answer is `<id>.answer` and a claim is `<id>.claim`, each
//! mode `0600` and written through a temporary file and a rename. Every file of
//! that folder is untrusted: this module checks the owner and the mode of each
//! file before it reads one, reads at most [`MAX_RECORD_BYTES`] of it, and
//! removes the file when its name, its type, its owner, its mode, its bytes or
//! its fields break a rule of this module.
//!
//! One window takes a request. The claim is an exclusive create of the claim
//! file, which only one process can win; a rename cannot be exclusive when two
//! windows race for the same name. A claim, an answer and a request that no
//! caller waits for any more are removed when they are older than
//! [`REQUEST_MAX_AGE`].
//!
//! Every function here reads or writes a file, so a window runs it on a worker
//! thread and never on the UI thread. The caller runs it before a window opens.
//!
//! [`docs/review/trust-boundaries/folder-requests.md`]: ../../../docs/review/trust-boundaries/folder-requests.md

use std::io::{ErrorKind, Read as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The version of a request record and of an answer record.
pub const RECORD_VERSION: u32 = 1;

/// The one verb of a request: show the folder in the window that runs.
pub const VERB_SHOW: &str = "show";

/// The age after which a request, a claim or an answer is removed, which is the
/// age after which a window applies a request no more.
pub const REQUEST_MAX_AGE: Duration = Duration::from_secs(10);

/// The most bytes that this module reads of one record.
pub const MAX_RECORD_BYTES: u64 = 4_096;

/// The most bytes of the folder of a request, and the most bytes of a reason.
pub const MAX_FOLDER_BYTES: usize = 4_096;

/// The most requests that one window handles in one tick. The others of that
/// folder are removed without an apply.
pub const MAX_REQUESTS_PER_TICK: usize = 4;

/// How long a caller waits for the answer of a window.
pub const CALLER_WAIT: Duration = Duration::from_secs(5);

/// The wait between two reads of the answer of a caller.
const CALLER_POLL: Duration = Duration::from_millis(20);

/// The reason of an answer that names a folder that is not a directory now.
pub const FOLDER_GONE: &str = "folder_gone";

/// The reason of an answer that names a window that cannot show a folder.
pub const WINDOW_BUSY: &str = "window_busy";

/// The reason of an answer of a window that ends with its window, so the start
/// that named no folder opens its own window.
pub const KEEP_RUNNING_OFF: &str = "keep_running_off";

/// Why the channel cannot continue.
#[derive(Debug, thiserror::Error)]
pub enum FolderRequestError {
    /// The request folder, or one file of it, could not be read or written.
    #[error("{path} is not usable as a folder request: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The reason of the system.
        source: std::io::Error,
    },
    /// The request folder of this home belongs to another user, or another
    /// process widened it, so this start must not use it.
    #[error("{path} holds the folder requests of another user")]
    NotPrivate {
        /// The folder that failed.
        path: PathBuf,
    },
}

/// The answer of a window to the caller that asked for a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// The window showed the workspace that the folder already had.
    Shown {
        /// The name of that workspace.
        workspace: String,
    },
    /// The window made a workspace for the folder, with one panel in it.
    Made {
        /// The name of the new workspace.
        workspace: String,
    },
    /// The window showed itself and raised itself, for a start that named no folder.
    Window,
    /// The window took the request and showed nothing, for `reason`.
    Refused {
        /// One word of this module, never text of a request.
        reason: String,
    },
}

/// One request that a window claimed, and the folder that it names.
#[derive(Clone, Debug)]
pub struct FolderRequest {
    id: String,
    /// The folder that the request names, and `None` for a start that only asks
    /// the window to show itself.
    folder: Option<PathBuf>,
}

impl FolderRequest {
    /// The id of the request, which names its three files.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The folder that the request names, resolved as the board names a folder.
    #[must_use]
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }
}

/// The folder of the requests of the home at `home_root`.
#[must_use]
pub fn directory(home_root: &Path) -> PathBuf {
    home_root.join("runtime").join("folder-requests")
}

/// Asks a window of the home at `home_root` to show `folder`, and waits
/// [`CALLER_WAIT`] for its answer.
///
/// The caller writes one record, reads the answer that a window writes, and
/// removes its own files. The answer is `None` when no window answers in time,
/// which is no error: the window may be busy, minimized or gone.
///
/// # Errors
///
/// Returns why the request folder or one of its files cannot be used.
pub fn ask(home_root: &Path, folder: &Path) -> Result<Option<Answer>, FolderRequestError> {
    ask_with(home_root, folder, CALLER_WAIT)
}

/// [`ask`] with an explicit wait, so a test needs no five seconds.
///
/// # Errors
///
/// Returns why the request folder or one of its files cannot be used.
pub fn ask_with(home_root: &Path, folder: &Path, wait: Duration) -> Result<Option<Answer>, FolderRequestError> {
    ask_record(home_root, Some(folder), wait)
}

/// Asks a window of the home at `home_root` to show itself and raise itself, and
/// waits [`CALLER_WAIT`] for its answer. The answer is `None` when no window
/// answers in time.
///
/// # Errors
///
/// Returns why the request folder or one of its files cannot be used.
pub fn ask_window(home_root: &Path) -> Result<Option<Answer>, FolderRequestError> {
    ask_window_with(home_root, CALLER_WAIT)
}

/// [`ask_window`] with an explicit wait, so a test needs no five seconds.
///
/// # Errors
///
/// Returns why the request folder or one of its files cannot be used.
pub fn ask_window_with(home_root: &Path, wait: Duration) -> Result<Option<Answer>, FolderRequestError> {
    ask_record(home_root, None, wait)
}

/// [`ask_with`] and [`ask_window`]: one record, with or without a folder.
fn ask_record(home_root: &Path, folder: Option<&Path>, wait: Duration) -> Result<Option<Answer>, FolderRequestError> {
    let directory = private_request_directory(home_root)?;
    let id = Uuid::new_v4().to_string();
    let record = RequestRecord {
        version: RECORD_VERSION,
        verb: VERB_SHOW.to_string(),
        folder: folder.map_or_else(String::new, |folder| folder.to_string_lossy().into_owned()),
        requested_at_millis: now_millis(),
    };
    let encoded = serde_json::to_vec(&record).map_err(|error| FolderRequestError::Io {
        path: directory.clone(),
        source: std::io::Error::other(error),
    })?;
    write_private(&request_path(&directory, &id), &encoded)?;
    let deadline = Instant::now() + wait;
    loop {
        if let Some(answer) = read_answer(&directory, &id) {
            remove_files(&directory, &id);
            return Ok(Some(answer));
        }
        if Instant::now() >= deadline {
            remove_files(&directory, &id);
            return Ok(None);
        }
        std::thread::sleep(CALLER_POLL);
    }
}

/// Whether the folder of the home at `home_root` holds a request that a window
/// may apply now. Reads no record and removes nothing.
#[must_use]
pub fn has_pending(home_root: &Path, now_millis: i64) -> bool {
    let directory = directory(home_root);
    if !readable(&directory) {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        is_request_name(&path) && modified_millis(&path).is_some_and(|at| is_fresh(at, now_millis))
    })
}

/// Removes what no window and no caller needs any more: a request, a claim and
/// an answer that are older than [`REQUEST_MAX_AGE`].
///
/// A window calls it when it cannot take a request, so a request of a caller
/// that is gone never holds the request folder.
///
/// # Errors
///
/// Returns why a file of the folder cannot be removed.
pub fn prune(home_root: &Path, now_millis: i64) -> Result<(), FolderRequestError> {
    let directory = directory(home_root);
    if !readable(&directory) {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(());
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_channel_name(&path) {
            continue;
        }
        if modified_millis(&path).is_some_and(|at| !is_fresh(at, now_millis)) {
            remove(&path)?;
        }
    }
    Ok(())
}

/// Claims the requests that this window takes now, up to
/// [`MAX_REQUESTS_PER_TICK`], and removes the rest of that folder.
///
/// A request that a second window claimed is not answered here, and a request
/// whose folder is not a directory now is answered `refused` with
/// [`FOLDER_GONE`]. This reads the folder of the home, so a window calls it on a
/// worker thread.
///
/// # Errors
///
/// Returns why the request folder or one of its files cannot be used.
pub fn claim(home_root: &Path, now_millis: i64) -> Result<Vec<FolderRequest>, FolderRequestError> {
    prune(home_root, now_millis)?;
    let directory = directory(home_root);
    if !readable(&directory) {
        return Ok(Vec::new());
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_request_name(path))
        .collect();
    // The order of a folder is the order of the system, and two windows sort the
    // same names the same way.
    paths.sort();
    let mut claimed = Vec::new();
    // The folder of a request must be resolved before a window applies it, and
    // the frame that applies it reads no folder: this worker does both.
    for path in paths {
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned) else {
            remove(&path)?;
            continue;
        };
        let Some(request) = read_request(&path, &id, now_millis) else {
            remove(&path)?;
            continue;
        };
        if claimed.len() >= MAX_REQUESTS_PER_TICK {
            remove(&path)?;
            continue;
        }
        if request.folder().is_some_and(|folder| !folder.is_dir()) {
            write_refusal(&directory, &request, FOLDER_GONE)?;
            remove(&path)?;
            continue;
        }
        if !claim_exclusively(&directory, &request.id)? {
            continue;
        }
        remove(&path)?;
        claimed.push(request);
    }
    Ok(claimed)
}

/// Writes the answer of this window to `request` and removes its claim.
///
/// # Errors
///
/// Returns why the answer file cannot be written or the claim cannot be removed.
pub fn answer(home_root: &Path, request: &FolderRequest, answer: &Answer) -> Result<(), FolderRequestError> {
    let directory = directory(home_root);
    write_answer(&directory, request.id(), answer)?;
    remove(&claim_path(&directory, request.id()))
}

/// The time of this computer in milliseconds since the epoch, as a record holds
/// it.
#[must_use]
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// One request record. Every field is checked before a window uses it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestRecord {
    version: u32,
    verb: String,
    folder: String,
    requested_at_millis: i64,
}

/// One answer record of a window.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnswerRecord {
    version: u32,
    outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

fn is_fresh(at: i64, now_millis: i64) -> bool {
    (now_millis - at).abs() <= i64::try_from(REQUEST_MAX_AGE.as_millis()).unwrap_or(i64::MAX)
}

fn request_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.request"))
}

fn claim_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.claim"))
}

fn answer_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.answer"))
}

fn remove_files(directory: &Path, id: &str) {
    for path in [
        request_path(directory, id),
        claim_path(directory, id),
        answer_path(directory, id),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether `path` names a request of this channel: `<uuid>.request`.
fn is_request_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".request"))
        .is_some_and(is_id)
}

/// Whether `path` names any file of this channel.
fn is_channel_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    [".request", ".claim", ".answer"]
        .iter()
        .any(|suffix| name.strip_suffix(suffix).is_some_and(is_id))
}

/// The form of a request id: the hexadecimal digits and dashes of a UUID.
fn is_id(text: &str) -> bool {
    text.len() == 36
        && text
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
}

/// The request folder of the home, made private when it is not there yet.
///
/// A folder that another user made, or that another process widened, is not used.
fn private_request_directory(home_root: &Path) -> Result<PathBuf, FolderRequestError> {
    let directory = directory(home_root);
    let io = |source| FolderRequestError::Io {
        path: directory.clone(),
        source,
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(io)?;
    let metadata = std::fs::symlink_metadata(&directory).map_err(io)?;
    if !metadata.is_dir() || !private_metadata(&metadata) || metadata.mode() & 0o700 != 0o700 {
        return Err(FolderRequestError::NotPrivate { path: directory });
    }
    // A umask can remove an owner bit of a folder that this call made, and a
    // folder of an older run can carry a wider mode.
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    Ok(directory)
}

/// Whether the request folder is one that this window may read: a directory of
/// this user that no group or other user reaches.
fn readable(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory)
        .is_ok_and(|metadata| metadata.is_dir() && private_metadata(&metadata) && metadata.mode() & 0o700 == 0o700)
}

/// Whether this process owns `metadata` and no group or other user reaches it.
fn private_metadata(metadata: &std::fs::Metadata) -> bool {
    // No group and no other user: the six low bits of a mode are zero.
    metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode().trailing_zeros() >= 6
}

/// Reads one request of `path`, or `None` when the file breaks a rule of this
/// module.
fn read_request(path: &Path, id: &str, now_millis: i64) -> Option<FolderRequest> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || !private_metadata(&metadata) {
        return None;
    }
    let record: RequestRecord = serde_json::from_slice(&read_bounded(path)?).ok()?;
    if record.version != RECORD_VERSION || record.verb != VERB_SHOW {
        return None;
    }
    let folder = match record.folder.as_str() {
        "" => None,
        folder => {
            if folder.len() > MAX_FOLDER_BYTES
                || folder.chars().any(char::is_control)
                || !Path::new(folder).is_absolute()
            {
                return None;
            }
            Some(crate::folder_identity(Path::new(folder)))
        }
    };
    if !is_fresh(record.requested_at_millis, now_millis) {
        return None;
    }
    Some(FolderRequest {
        id: id.to_owned(),
        folder,
    })
}

/// Reads at most [`MAX_RECORD_BYTES`] of `path`, or `None` when it holds more.
fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    Some(bytes)
}

/// Takes the claim of `id`, or `false` when a second window has it.
fn claim_exclusively(directory: &Path, id: &str) -> Result<bool, FolderRequestError> {
    let path = claim_path(directory, id);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
    {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(source) => Err(FolderRequestError::Io { path, source }),
    }
}

/// Writes an answer record through a temporary file and a rename.
fn write_answer(directory: &Path, id: &str, answer: &Answer) -> Result<(), FolderRequestError> {
    let (outcome, workspace, reason) = match answer {
        Answer::Shown { workspace } => ("shown", Some(workspace.clone()), None),
        Answer::Made { workspace } => ("made", Some(workspace.clone()), None),
        Answer::Window => ("window", None, None),
        Answer::Refused { reason } => ("refused", None, Some(reason.clone())),
    };
    let record = AnswerRecord {
        version: RECORD_VERSION,
        outcome: outcome.to_string(),
        workspace,
        reason,
    };
    let encoded = serde_json::to_vec(&record).map_err(|error| FolderRequestError::Io {
        path: directory.to_path_buf(),
        source: std::io::Error::other(error),
    })?;
    write_private(&answer_path(directory, id), &encoded)
}

/// Answers `FOLDER_GONE` to the caller of a request that a window cannot apply.
fn write_refusal(directory: &Path, request: &FolderRequest, reason: &str) -> Result<(), FolderRequestError> {
    if !claim_exclusively(directory, &request.id)? {
        return Ok(());
    }
    write_answer(
        directory,
        &request.id,
        &Answer::Refused {
            reason: reason.to_string(),
        },
    )?;
    remove(&claim_path(directory, &request.id))
}

/// One private file of this channel, replaced as a whole.
fn write_private(path: &Path, encoded: &[u8]) -> Result<(), FolderRequestError> {
    crate::AtomicFile::new(path.to_path_buf())
        .write(encoded)
        .map_err(|source| FolderRequestError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn remove(path: &Path) -> Result<(), FolderRequestError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FolderRequestError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The time that `path` last changed, in milliseconds since the epoch.
fn modified_millis(path: &Path) -> Option<i64> {
    let modified = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// The answer of `id`, when a window wrote one that this module can read.
fn read_answer(directory: &Path, id: &str) -> Option<Answer> {
    // A window writes its answer, and no group or other user reaches it.
    let path = answer_path(directory, id);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || !private_metadata(&metadata) {
        return None;
    }
    let record: AnswerRecord = serde_json::from_slice(&read_bounded(&path)?).ok()?;
    if record.version != RECORD_VERSION {
        return None;
    }
    let reason = record.reason.filter(|reason| !reason.is_empty());
    match (record.outcome.as_str(), record.workspace) {
        ("shown", Some(workspace)) => Some(Answer::Shown { workspace }),
        ("made", Some(workspace)) => Some(Answer::Made { workspace }),
        ("window", _) => Some(Answer::Window),
        ("refused", _) => Some(Answer::Refused {
            reason: reason.unwrap_or_else(|| FOLDER_GONE.to_string()),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier};

    use super::{
        Answer, FOLDER_GONE, FolderRequest, MAX_RECORD_BYTES, RECORD_VERSION, RequestRecord, VERB_SHOW, answer,
        ask_with, claim, directory, has_pending, now_millis, prune, request_path,
    };

    fn home() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("home");
        std::fs::create_dir(&root).expect("the home");
        (temp, root)
    }

    fn folder_of(temp: &tempfile::TempDir) -> PathBuf {
        let folder = temp.path().join("repo");
        std::fs::create_dir_all(&folder).expect("the folder");
        folder
    }

    fn id(number: u32) -> String {
        format!("00000000-0000-4000-8000-{number:012}")
    }

    /// Writes one request file of `text` with `mode`, as another process could.
    fn write_at(root: &Path, file_id: &str, text: &str, mode: u32) -> PathBuf {
        let directory = directory(root);
        std::fs::create_dir_all(&directory).expect("the folder of requests");
        std::fs::set_permissions(&directory, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .expect("a private folder");
        let path = request_path(&directory, file_id);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&path)
            .expect("the request");
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(mode)).expect("the mode");
        std::io::Write::write_all(&mut file, text.as_bytes()).expect("the record");
        path
    }

    fn record(folder: &Path, verb: &str, at_millis: i64) -> String {
        serde_json::to_string(&RequestRecord {
            version: RECORD_VERSION,
            verb: verb.to_string(),
            folder: folder.display().to_string(),
            requested_at_millis: at_millis,
        })
        .expect("a record")
    }

    fn entries(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory(root))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// A window claims the request of a caller and answers it, and the caller
    /// reads one answer of the three that a window writes.
    #[test]
    fn a_caller_reads_the_answer_of_the_window_that_claims_its_request() {
        for (answered, expected) in [
            (
                Answer::Shown {
                    workspace: "repo".to_string(),
                },
                Answer::Shown {
                    workspace: "repo".to_string(),
                },
            ),
            (
                Answer::Made {
                    workspace: "repo".to_string(),
                },
                Answer::Made {
                    workspace: "repo".to_string(),
                },
            ),
        ] {
            let (temp, root) = home();
            let folder = folder_of(&temp);
            let asking = folder.clone();
            let asking_root = root.clone();
            let caller = std::thread::spawn(move || super::ask_with(&asking_root, &asking, super::CALLER_WAIT));

            // The window of the test: it claims what the caller wrote and
            // answers it, as the frame of a running Horizon does.
            let answered = answered.clone();
            let window_root = root.clone();
            let window = std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + super::CALLER_WAIT;
                while std::time::Instant::now() < deadline {
                    let claimed: Vec<FolderRequest> = claim(&window_root, now_millis()).expect("a claim");
                    if let Some(request) = claimed.first() {
                        answer_in(&window_root, request, &answered);
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                panic!("no request reached the window");
            });
            let read = caller.join().expect("the caller").expect("a caller of the channel");
            assert_eq!(read, Some(expected.clone()));
            window.join().expect("the window");
            assert!(
                entries(&root).is_empty(),
                "the caller removes its files: {:?}",
                entries(&root)
            );
        }
    }

    fn answer_in(root: &Path, request: &FolderRequest, written: &Answer) {
        answer(root, request, written).expect("the answer");
    }

    /// A caller that no window answers waits its bound, removes its files and
    /// reads no answer.
    #[test]
    fn a_caller_without_a_window_ends_without_an_answer_and_leaves_no_file() {
        let (temp, root) = home();
        let folder = folder_of(&temp);

        let answer = ask_with(&root, &folder, std::time::Duration::from_millis(120)).expect("the call");

        assert_eq!(answer, None, "no window answered");
        assert!(entries(&root).is_empty(), "{:?}", entries(&root));
    }

    /// Every file that breaks a rule of the channel is removed, and a window
    /// applies none of them.
    #[test]
    fn a_record_that_breaks_a_rule_is_removed_and_applied_to_nothing() {
        let (temp, root) = home();
        let folder = folder_of(&temp);
        let now = now_millis();
        let valid = record(&folder, VERB_SHOW, now);

        for (case, text, mode) in [
            ("malformed", "{ not json".to_string(), 0o600),
            ("another verb", record(&folder, "run", now), 0o600),
            ("a relative folder", record(Path::new("repo"), VERB_SHOW, now), 0o600),
            ("an old request", record(&folder, VERB_SHOW, now - 11_000), 0o600),
            (
                "a request of the future",
                record(&folder, VERB_SHOW, now + 60_000),
                0o600,
            ),
            (
                "more bytes than the record bound",
                format!(
                    "{valid}{}",
                    " ".repeat(usize::try_from(MAX_RECORD_BYTES).unwrap_or(4096))
                ),
                0o600,
            ),
            ("a wider mode", valid.clone(), 0o644),
        ] {
            write_at(&root, &id(1), &text, mode);
            let claimed = claim(&root, now).expect("a claim");
            assert!(claimed.is_empty(), "{case}: a window applies no such request");
            assert!(
                entries(&root).is_empty(),
                "{case}: the file stays: {:?}",
                entries(&root)
            );
        }

        // A symbolic link is removed, and its target stays.
        let target = temp.path().join("elsewhere.json");
        std::fs::write(&target, &valid).expect("the target");
        std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o600)).expect("the mode");
        std::fs::create_dir_all(directory(&root)).expect("the folder of requests");
        std::os::unix::fs::symlink(&target, request_path(&directory(&root), &id(2))).expect("the link");
        assert!(claim(&root, now).expect("a claim").is_empty());
        assert!(!request_path(&directory(&root), &id(2)).exists(), "the link is removed");
        assert!(target.exists(), "the file that it named stays");

        // A folder that is a file, or that is gone, is answered and removed.
        let file = temp.path().join("a-file");
        std::fs::write(&file, b"not a folder").expect("the file");
        write_at(&root, &id(3), &record(&file, VERB_SHOW, now), 0o600);
        assert!(claim(&root, now).expect("a claim").is_empty(), "a file is no folder");
        let answer = std::fs::read_to_string(directory(&root).join(format!("{}.answer", id(3)))).expect("the answer");
        assert!(answer.contains(FOLDER_GONE), "{answer}");
        assert_eq!(entries(&root), vec![format!("{}.answer", id(3))], "the request is gone");
    }

    /// A start that names no folder asks the window to show itself: the record
    /// carries no folder, a window claims it, and the caller reads the answer of
    /// a window that showed itself.
    #[test]
    fn a_request_without_a_folder_asks_the_window_to_show_itself() {
        let (_temp, root) = home();
        let asking_root = root.clone();
        let caller = std::thread::spawn(move || super::ask_window(&asking_root));

        let window_root = root.clone();
        let window = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + super::CALLER_WAIT;
            while std::time::Instant::now() < deadline {
                let claimed: Vec<FolderRequest> = claim(&window_root, now_millis()).expect("a claim");
                if let Some(request) = claimed.first() {
                    assert_eq!(request.folder(), None, "the request names no folder");
                    return answer_in(&window_root, request, &Answer::Window);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            panic!("the window of the test claimed no request");
        });

        let answer = caller.join().expect("the caller").expect("a caller of the channel");
        window.join().expect("the window of the test");

        assert_eq!(answer, Some(Answer::Window), "the window showed itself");
    }

    /// A record of the form of this module is claimed, and it names the folder
    /// that its caller asked for.
    #[test]
    fn a_record_of_the_form_is_claimed_and_names_its_folder() {
        let (temp, root) = home();
        let folder = folder_of(&temp);
        let now = now_millis();
        write_at(&root, &id(4), &record(&folder, VERB_SHOW, now), 0o600);

        let claimed = claim(&root, now).expect("a claim");

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id(), id(4));
        assert_eq!(claimed[0].folder(), Some(crate::folder_identity(&folder).as_path()));
        assert!(
            entries(&root).contains(&format!("{}.claim", id(4))),
            "the claim stays for the answer"
        );
        assert!(
            !entries(&root).contains(&format!("{}.request", id(4))),
            "no second window claims it"
        );
    }

    /// Two windows that claim at the same time leave one winner for each
    /// request, 200 times.
    #[test]
    fn one_request_has_one_winner_among_two_claimers() {
        let (temp, root) = home();
        let folder = folder_of(&temp);
        let now = now_millis();
        std::fs::create_dir_all(directory(&root)).expect("the folder of requests");

        for round in 0..200 {
            let file_id = id(round);
            write_at(&root, &file_id, &record(&folder, VERB_SHOW, now), 0o600);
            let barrier = Arc::new(Barrier::new(2));
            let claims: Vec<usize> = [(), ()]
                .into_iter()
                .map(|()| {
                    let (root, barrier) = (root.clone(), Arc::clone(&barrier));
                    std::thread::spawn(move || {
                        barrier.wait();
                        claim(&root, now).expect("a claim").len()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|thread| thread.join().expect("a claimer"))
                .collect();

            assert_eq!(claims.iter().sum::<usize>(), 1, "round {round}: {claims:?}");
            std::fs::remove_file(directory(&root).join(format!("{file_id}.claim"))).expect("the claim of the winner");
        }
    }

    /// A request that waits for a window is visible to the pump of a window, an
    /// old one is not, and a prune removes it.
    #[test]
    fn a_window_sees_a_young_request_and_prunes_an_old_one() {
        let (temp, root) = home();
        let folder = folder_of(&temp);
        let now = now_millis();

        assert!(!has_pending(&root, now), "no folder of requests is no request");
        write_at(&root, &id(5), &record(&folder, VERB_SHOW, now), 0o600);
        assert!(has_pending(&root, now), "a young request wakes a window");

        let later = now + 11_000;
        assert!(!has_pending(&root, later), "an old request wakes nothing");
        prune(&root, later).expect("a prune");
        assert!(entries(&root).is_empty(), "{:?}", entries(&root));
    }
}
