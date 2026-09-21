use std::path::PathBuf;

use super::{Scripted, json, set_files_through};

#[test]
fn set_files_finds_the_input_and_sends_newline_separated_paths() {
    let transport = Scripted::new(vec![
        Ok(json!({"value": {"element-6066-11e4-a52e-4f735466cecf": "file 1"}})),
        Ok(json!({"value": null})),
    ]);
    set_files_through(
        &transport,
        "/session/s1",
        "input[type=file]",
        &[PathBuf::from("/uploads/a.pdf"), PathBuf::from("/uploads/b.png")],
    )
    .expect("attach");
    assert_eq!(
        transport.sent(),
        vec![
            (
                "POST".to_string(),
                "/session/s1/element".to_string(),
                json!({"using": "css selector", "value": "input[type=file]"})
            ),
            (
                "POST".to_string(),
                "/session/s1/element/file%201/value".to_string(),
                json!({"text": "/uploads/a.pdf\n/uploads/b.png"})
            ),
        ],
        "the input is never cleared and the paths travel as one Send Keys text"
    );
}

#[test]
fn a_failing_command_ends_the_attachment_sequence() {
    let transport = Scripted::new(vec![Err("no such element".into())]);
    let error = set_files_through(
        &transport,
        "/session/s1",
        "#missing",
        &[PathBuf::from("/uploads/a.pdf")],
    )
    .expect_err("not found");
    assert!(error.contains("no such element"), "{error}");
    assert_eq!(transport.sent().len(), 1, "nothing follows a failed Find Element");

    let transport = Scripted::new(vec![Ok(json!({"value": {"ELEMENT": "a/b"}}))]);
    let error = set_files_through(&transport, "/session/s1", "#doc", &[PathBuf::from("/uploads/a.pdf")])
        .expect_err("unroutable");
    assert!(error.contains("cannot form a route"), "{error}");
    assert_eq!(transport.sent().len(), 1);

    let transport = Scripted::new(vec![]);
    let error = set_files_through(&transport, "/session/s1", "#doc", &[PathBuf::from("/uploads/a\n.pdf")])
        .expect_err("line break");
    assert!(error.contains("line break"), "{error}");
    assert!(
        transport.sent().is_empty(),
        "a path that would split into two uploads never reaches the driver"
    );
}
