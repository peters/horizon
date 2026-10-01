use super::*;

#[test]
fn printable_keeps_text_newlines_and_tabs_only() {
    assert_eq!(printable("run /compact\nthen\tstop"), "run /compact\nthen\tstop");
    assert_eq!(printable("a\r\nb"), "a\nb");
    assert_eq!(printable("rm\x1b[2Jx\x03y\x07z\r"), "rm[2Jxyz");
}

#[test]
fn unavailable_is_the_same_answer_for_every_hidden_panel() {
    let Outcome::Failed { code, .. } = unavailable() else {
        panic!("expected a failure");
    };
    assert_eq!(code, "panel_unavailable");
}
