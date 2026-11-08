use std::path::Path;

use base64::Engine as _;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};

use super::{MAX_INLINE_BYTES, attach};

const NAME: &str = "report.pdf";

fn answer(folder: &Path, rows: &serde_json::Value) -> CallToolResponse {
    CallToolResult::structured(serde_json::json!({
        "panel_id": "panel-1",
        "action_id": "action-1",
        "directory": folder,
        "reporting": true,
        "downloads": rows,
        "dropped": 0,
        "next_step": "read the path",
    }))
    .into()
}

fn completed(folder: &Path, name: &str) -> serde_json::Value {
    serde_json::json!({
        "file_name": name,
        "state": "completed",
        "received_bytes": 5,
        "total_bytes": 5,
        "path": folder.join(name),
    })
}

fn blobs(response: &CallToolResponse) -> Vec<String> {
    let CallToolResponse::Complete(result) = response else {
        return Vec::new();
    };
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Resource(resource) => match &resource.resource {
                rmcp::model::ResourceContents::BlobResourceContents { blob, .. } => Some(blob.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn texts(response: &CallToolResponse) -> Vec<String> {
    let CallToolResponse::Complete(result) = response else {
        return Vec::new();
    };
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

/// The block that states why the bytes did not cross. The structured result also
/// carries its own JSON as the first text block, so the refusal is the last one.
fn refusal(response: &CallToolResponse) -> String {
    texts(response).pop().unwrap_or_default()
}

fn structured(response: &CallToolResponse) -> serde_json::Value {
    let CallToolResponse::Complete(result) = response else {
        panic!("complete result");
    };
    result.structured_content.clone().unwrap_or_default()
}

fn folder_with(name: &str, bytes: &[u8]) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    std::fs::write(folder.path().join(name), bytes).unwrap_or_else(|error| panic!("{error}"));
    folder
}

#[test]
fn the_bytes_of_a_completed_download_cross_as_one_blob() {
    let folder = folder_with(NAME, b"hello");
    let response = attach(
        answer(folder.path(), &serde_json::json!([completed(folder.path(), NAME)])),
        NAME,
    );
    let expected = base64::engine::general_purpose::STANDARD.encode(b"hello");
    assert_eq!(blobs(&response), vec![expected]);
    assert_eq!(structured(&response)["fetched_file_name"], NAME);
    assert_eq!(structured(&response)["fetched_bytes"], 5);
}

#[test]
fn a_download_that_the_answer_does_not_report_is_never_read() {
    let folder = folder_with(NAME, b"hello");
    let response = attach(answer(folder.path(), &serde_json::json!([])), NAME);
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("no completed download named"));
}

#[test]
fn a_download_that_is_still_in_progress_has_no_bytes_to_send() {
    let folder = folder_with(NAME, b"hello");
    let rows = serde_json::json!([{"file_name": NAME, "state": "in_progress", "path": folder.path().join(NAME)}]);
    let response = attach(answer(folder.path(), &rows), NAME);
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("no completed download named"));
}

#[test]
fn a_path_outside_the_panel_folder_is_refused() {
    let folder = folder_with(NAME, b"hello");
    let outside = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    std::fs::write(outside.path().join(NAME), b"secret").unwrap_or_else(|error| panic!("{error}"));
    let rows = serde_json::json!([completed(outside.path(), NAME)]);
    let response = attach(answer(folder.path(), &rows), NAME);
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("outside the panel's download folder"));
}

#[test]
fn a_name_that_is_not_one_path_component_is_refused() {
    let folder = folder_with(NAME, b"hello");
    let rows = serde_json::json!([{
        "file_name": "../report.pdf",
        "state": "completed",
        "path": folder.path().join(NAME),
    }]);
    let response = attach(answer(folder.path(), &rows), "../report.pdf");
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("outside the panel's download folder"));
}

#[test]
fn a_symlink_that_leads_out_of_the_panel_folder_is_refused() {
    let folder = folder_with(NAME, b"hello");
    let outside = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let target = outside.path().join("secret");
    std::fs::write(&target, b"secret").unwrap_or_else(|error| panic!("{error}"));
    let link = folder.path().join("link.pdf");
    std::os::unix::fs::symlink(&target, &link).unwrap_or_else(|error| panic!("{error}"));
    let rows = serde_json::json!([{
        "file_name": "link.pdf",
        "state": "completed",
        "path": link,
    }]);
    let response = attach(answer(folder.path(), &rows), "link.pdf");
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("outside the panel's download folder"));
}

#[test]
fn a_file_larger_than_the_bound_stays_where_it_was_saved() {
    let folder = folder_with(
        NAME,
        &vec![b'a'; usize::try_from(MAX_INLINE_BYTES).unwrap_or(usize::MAX) + 1],
    );
    let rows = serde_json::json!([completed(folder.path(), NAME)]);
    let response = attach(answer(folder.path(), &rows), NAME);
    assert!(blobs(&response).is_empty());
    assert!(refusal(&response).contains("larger than"));
    assert_eq!(structured(&response)["inline_withheld"], true);
}

#[test]
fn an_error_answer_keeps_its_error() {
    let folder = folder_with(NAME, b"hello");
    let CallToolResponse::Complete(mut result) =
        answer(folder.path(), &serde_json::json!([completed(folder.path(), NAME)]))
    else {
        panic!("complete result");
    };
    result.is_error = Some(true);
    let response = attach(CallToolResponse::Complete(result), NAME);
    assert!(blobs(&response).is_empty());
}
