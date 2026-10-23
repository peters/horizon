use base64::Engine as _;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};

use super::inline;

fn result(path: &std::path::Path) -> CallToolResponse {
    CallToolResult::structured(serde_json::json!({
        "panel_id": "profile-web-aaaaaaaaaaaa",
        "path": path,
        "width": 2,
        "height": 1,
        "clipboard_requested": false,
    }))
    .into()
}

fn images(response: &CallToolResponse) -> Vec<String> {
    let CallToolResponse::Complete(result) = response else {
        return Vec::new();
    };
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image(image) if image.mime_type == "image/png" => Some(image.data.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_png_under_the_root_becomes_an_image_block_after_the_json() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let png = root.path().join("capture-1.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\nbytes").unwrap_or_else(|error| panic!("{error}"));
    let response = inline(result(&png), root.path());
    let expected = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nbytes");
    assert_eq!(images(&response), vec![expected]);
    let CallToolResponse::Complete(done) = response else {
        panic!("complete result");
    };
    assert!(
        matches!(done.content.first(), Some(ContentBlock::Text(_))),
        "the JSON stays first"
    );
}

#[test]
fn a_file_outside_the_root_or_not_a_png_or_an_error_stays_a_path() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let other = tempfile::tempdir().unwrap_or_else(|error| panic!("temp: {error}"));
    let outside = other.path().join("capture-1.png");
    std::fs::write(&outside, b"png").unwrap_or_else(|error| panic!("{error}"));
    assert!(images(&inline(result(&outside), root.path())).is_empty());
    let text = root.path().join("capture-1.txt");
    std::fs::write(&text, b"png").unwrap_or_else(|error| panic!("{error}"));
    assert!(images(&inline(result(&text), root.path())).is_empty());
    let missing = root.path().join("gone.png");
    assert!(images(&inline(result(&missing), root.path())).is_empty());
    let png = root.path().join("capture-2.png");
    std::fs::write(&png, b"png").unwrap_or_else(|error| panic!("{error}"));
    let CallToolResponse::Complete(mut failed) = result(&png) else {
        panic!("complete result");
    };
    failed.is_error = Some(true);
    assert!(images(&inline(failed.into(), root.path())).is_empty());
}
