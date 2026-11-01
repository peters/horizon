use super::super::dismissed;

#[test]
fn a_click_outside_the_form_keeps_it_and_cancel_escape_and_save_still_close_it() {
    assert!(!dismissed(false, false, false), "nothing was asked");
    assert!(
        !dismissed(false, true, true),
        "a click on the backdrop keeps the keys and tokens the person entered"
    );
    assert!(dismissed(true, false, false), "Cancel closes it");
    assert!(
        dismissed(false, true, false),
        "Escape and the dialog's own close still close it"
    );
    assert!(
        dismissed(true, true, true),
        "Cancel closes it even when the click also landed on the backdrop"
    );
}
