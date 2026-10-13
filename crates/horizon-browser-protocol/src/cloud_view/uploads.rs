//! Local files that a person uploads into a worker browser.
//!
//! A host that relays uploads sends [`CloudUploadPoll`] with each poll. The
//! worker then relays the page's file chooser to that host, receives the bytes
//! of each picked or dropped file in [`CloudStageChunk`] requests, and attaches
//! only files that it staged and verified itself. Host paths never reach the
//! worker. A worker that predates uploads omits `CloudViewState::uploads`, and a
//! host that predates them omits `CloudUploadPoll`: the worker then cancels each
//! file chooser at once. Later versions only add fields.
//!
//! The host does not trust the worker or its pages: `accept`, `origin`, errors
//! and notices are their text, and worker limits can only lower host limits.
use serde::{Deserialize, Serialize};

/// The upload protocol version of this build.
pub const CLOUD_UPLOAD_VERSION: u32 = 1;
/// Most bytes of one file name on the worker.
pub const MAX_UPLOAD_NAME_BYTES: usize = 255;
const MAX_BATCH_ID_BYTES: usize = 64;

/// The bounds that a worker enforces and advertises.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudUploadLimits {
    /// Most files in one choice or drop.
    pub max_files: usize,
    /// Most bytes of all files in one choice or drop.
    pub max_batch_bytes: u64,
    /// Most decoded bytes in one [`CloudStageChunk`].
    pub max_chunk_bytes: u64,
}

impl Default for CloudUploadLimits {
    fn default() -> Self {
        Self {
            max_files: crate::MAX_ATTACHMENT_FILES,
            max_batch_bytes: 256 * 1024 * 1024,
            max_chunk_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Sent with each poll by a host that relays file choosers and drops.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CloudUploadPoll {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<CloudChooserAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop: Option<CloudFileDrop>,
}

impl Default for CloudUploadPoll {
    fn default() -> Self {
        Self {
            version: CLOUD_UPLOAD_VERSION,
            answer: None,
            drop: None,
        }
    }
}

/// The person's answer to one worker file chooser request.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudChooserAnswer {
    /// [`CloudChooserRequest::id`] of the request that this answers.
    pub request: u64,
    /// `None` cancels the request.
    #[serde(default)]
    pub files: Option<CloudStagedFiles>,
}

/// Files that earlier [`CloudStageChunk`] requests completed in one batch.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudStagedFiles {
    pub batch: String,
    pub names: Vec<String>,
}

/// Staged files dropped at a point of the page, in page pixels.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CloudFileDrop {
    pub x: f64,
    pub y: f64,
    pub files: CloudStagedFiles,
}

/// One part of one local file, in order. A host sends these on a separate
/// connection so that presentation polls continue during a transfer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudStageChunk {
    /// The worker browser that receives the file.
    pub id: String,
    pub batch: String,
    pub name: String,
    /// Bytes of the whole file.
    pub size: u64,
    /// Bytes of the file that earlier chunks sent.
    pub offset: u64,
    /// Standard base64 of at most [`CloudUploadLimits::max_chunk_bytes`] bytes.
    pub data: String,
    /// Lowercase hexadecimal SHA-256 of the whole file, only on the chunk that completes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// The worker's state of a file after one [`CloudStageChunk`].
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudStageReceipt {
    pub received: u64,
    /// The worker verified the size and the SHA-256 of the whole file.
    pub complete: bool,
}

/// What a worker browser offers for uploads.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudUploadState {
    pub version: u32,
    pub limits: CloudUploadLimits,
    /// The browser relays the page's file chooser.
    pub file_chooser: bool,
    /// The browser accepts dropped files.
    pub drops: bool,
    /// The page waits for the person to pick files or cancel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<CloudChooserRequest>,
    /// The newest failure to attach an answer to the pending request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<CloudChooserError>,
    /// The newest failure that the worker browser reported, such as a failed drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<CloudBrowserNotice>,
}

/// A page file chooser. `accept` and `origin` come from the page.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudChooserRequest {
    pub id: u64,
    pub multiple: bool,
    pub accept: String,
    pub origin: String,
}

/// `sequence` grows with each new error, so a host shows each error once.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudChooserError {
    pub request: u64,
    pub sequence: u64,
    pub message: String,
}

/// `sequence` grows with each new notice, so a host shows each notice once.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudBrowserNotice {
    pub sequence: u64,
    pub message: String,
}

/// A batch ID is 1 to 64 ASCII letters, digits, `-` or `_`.
#[must_use]
pub fn valid_upload_batch(batch: &str) -> bool {
    !batch.is_empty()
        && batch.len() <= MAX_BATCH_ID_BYTES
        && batch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A file name is one path component of at most [`MAX_UPLOAD_NAME_BYTES`]
/// bytes without control characters.
#[must_use]
pub fn valid_upload_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_UPLOAD_NAME_BYTES
        && name != "."
        && name != ".."
        && !name.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_view::{CloudViewRequest, CloudViewResponse};

    #[test]
    fn polls_without_uploads_keep_the_wire_format_of_older_hosts() {
        let old = r#"{"operation":"poll","id":"browser-1","after":7,"commands":[]}"#;
        let CloudViewRequest::Poll { uploads, after, .. } = serde_json::from_str(old).unwrap() else {
            panic!("an older host's poll must still parse as a poll");
        };
        assert_eq!(after, 7);
        assert!(uploads.is_none());
        let encoded = serde_json::to_value(CloudViewRequest::Poll {
            id: "browser-1".into(),
            after: 7,
            commands: Vec::new(),
            uploads: None,
        })
        .unwrap();
        assert_eq!(encoded, serde_json::from_str::<serde_json::Value>(old).unwrap());
    }

    #[test]
    fn responses_of_older_workers_have_no_upload_state() {
        let old = r#"{"browsers":[{"id":"browser-1","title":"","url":"","owner":null,"ready":true,
            "visible":true,"handoff":null,"lost":false,"sequence":3,"png":null,"error":null}],"error":null}"#;
        let response: CloudViewResponse = serde_json::from_str(old).unwrap();
        assert!(response.browsers[0].uploads.is_none());
        assert!(response.staged.is_none());
        let encoded = serde_json::to_value(&response).unwrap();
        assert!(encoded.get("staged").is_none());
        assert!(encoded["browsers"][0].get("uploads").is_none());
    }

    #[test]
    fn upload_requests_and_state_round_trip() {
        let poll = CloudUploadPoll {
            answer: Some(CloudChooserAnswer {
                request: 4,
                files: Some(CloudStagedFiles {
                    batch: "b1".into(),
                    names: vec!["notes 1.txt".into()],
                }),
            }),
            drop: Some(CloudFileDrop {
                x: 10.5,
                y: 20.0,
                files: CloudStagedFiles {
                    batch: "b2".into(),
                    names: vec!["photo.png".into()],
                },
            }),
            ..CloudUploadPoll::default()
        };
        let request = CloudViewRequest::Poll {
            id: "browser-1".into(),
            after: 0,
            commands: Vec::new(),
            uploads: Some(poll.clone()),
        };
        let CloudViewRequest::Poll { uploads, .. } =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap()
        else {
            panic!("poll");
        };
        assert_eq!(uploads, Some(poll));
        let cancel: CloudChooserAnswer = serde_json::from_str(r#"{"request":9}"#).unwrap();
        assert!(cancel.files.is_none());

        let chunk = CloudStageChunk {
            id: "browser-1".into(),
            batch: "b1".into(),
            name: "notes 1.txt".into(),
            size: 3,
            offset: 0,
            data: "YWJj".into(),
            sha256: Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()),
        };
        let encoded = serde_json::to_value(CloudViewRequest::Stage(chunk.clone())).unwrap();
        assert_eq!(encoded["operation"], "stage");
        let CloudViewRequest::Stage(decoded) = serde_json::from_value(encoded).unwrap() else {
            panic!("stage");
        };
        assert_eq!(decoded, chunk);

        let state = CloudUploadState {
            version: CLOUD_UPLOAD_VERSION,
            limits: CloudUploadLimits::default(),
            file_chooser: true,
            drops: true,
            request: Some(CloudChooserRequest {
                id: 4,
                multiple: true,
                accept: ".txt".into(),
                origin: "https://upload.test".into(),
            }),
            error: None,
            notice: None,
        };
        let decoded: CloudUploadState = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn a_newer_host_or_worker_adds_fields_without_breaking_this_version() {
        let poll: CloudUploadPoll = serde_json::from_str(r#"{"version":2,"future":{"kind":"x"}}"#).unwrap();
        assert_eq!(poll.version, 2);
        assert!(poll.answer.is_none() && poll.drop.is_none());
        let state: CloudUploadState = serde_json::from_str(
            r#"{"version":2,"limits":{"max_files":4,"max_batch_bytes":10,"max_chunk_bytes":5,"later":1},
                "file_chooser":true,"drops":false,"later":[]}"#,
        )
        .unwrap();
        assert_eq!(state.limits.max_files, 4);
        assert!(state.file_chooser && !state.drops);
    }

    #[test]
    fn batch_ids_and_names_cannot_leave_their_folder() {
        assert!(valid_upload_batch("0f3a-b_9"));
        for batch in ["", "../b", "b/c", "b.c", &"b".repeat(65)] {
            assert!(!valid_upload_batch(batch), "{batch:?}");
        }
        for name in ["notes.txt", "Rapport ñ 2.pdf", "it's here.png", ".hidden", "名前.txt"] {
            assert!(valid_upload_name(name), "{name:?}");
        }
        let long = "a".repeat(MAX_UPLOAD_NAME_BYTES + 1);
        for name in ["", ".", "..", "a/b", "a\\b", "line\nbreak", "nul\0", long.as_str()] {
            assert!(!valid_upload_name(name), "{name:?}");
        }
        assert!(valid_upload_name(&"a".repeat(MAX_UPLOAD_NAME_BYTES)));
    }
}
