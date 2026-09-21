use std::path::PathBuf;

use super::{Scripted, json, set_files_through};

#[test]
fn set_files_uses_the_resolved_element_without_querying_the_selector_again() {
    let transport = Scripted::new(vec![Ok(json!({"value": null}))]);
    set_files_through(
        &transport,
        "/session/s1",
        "file 1",
        &[PathBuf::from("/uploads/a.pdf"), PathBuf::from("/uploads/b.png")],
        false,
    )
    .expect("attach");
    assert_eq!(
        transport.sent(),
        vec![(
            "POST".to_string(),
            "/session/s1/element/file%201/value".to_string(),
            json!({"text": "/uploads/a.pdf\n/uploads/b.png"})
        )],
        "the original element reference is used even if the page replaces its selector"
    );
}

#[test]
fn a_stale_element_ends_the_attachment_without_finding_a_replacement() {
    let transport = Scripted::new(vec![Err("stale element reference".into())]);
    let error = set_files_through(
        &transport,
        "/session/s1",
        "old-file",
        &[PathBuf::from("/uploads/a.pdf")],
        false,
    )
    .expect_err("stale element");
    assert!(error.contains("stale element reference"), "{error}");
    assert_eq!(transport.sent().len(), 1);
}

#[test]
fn file_paths_with_line_breaks_never_reach_the_driver() {
    let transport = Scripted::new(vec![]);
    assert!(
        set_files_through(
            &transport,
            "/session/s1",
            "file",
            &[PathBuf::from("/uploads/a\n.pdf")],
            false
        )
        .is_err()
    );
    assert!(transport.sent().is_empty());
}

#[test]
fn safari_restores_wildcard_accept_on_the_same_element_after_success_or_failure() {
    for accept in ["*/*,.pdf", "*/json", "im*age/png", "image/p*ng", "image/*"] {
        for succeeds in [true, false] {
            let transport = Scripted::new(vec![
                Ok(json!({"value": accept})),
                if succeeds {
                    Ok(json!({"value": null}))
                } else {
                    Err("selection failed".into())
                },
                Ok(json!({"value": null})),
            ]);
            let result = set_files_through(
                &transport,
                "/session/s1",
                "file 1",
                &[PathBuf::from("/uploads/unknown.ext")],
                true,
            );
            assert_eq!(result.is_ok(), succeeds);
            let sent = transport.sent();
            assert_eq!(sent.len(), 3);
            assert_eq!(
                sent[0].2["args"],
                json!([{"element-6066-11e4-a52e-4f735466cecf": "file 1"}])
            );
            assert_eq!(sent[1].1, "/session/s1/element/file%201/value");
            assert_eq!(sent[2].1, "/session/s1/execute/sync");
            assert_eq!(
                sent[2].2["args"],
                json!([{"element-6066-11e4-a52e-4f735466cecf": "file 1"}, accept])
            );
        }
    }
}
