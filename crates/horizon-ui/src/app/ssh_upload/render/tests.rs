use super::single_file_display_name;

#[test]
fn upload_pill_name_is_flattened_before_its_scalar_budget_is_applied() {
    assert_eq!(
        single_file_display_name("first line\nsecond line with more detail"),
        "first line second l…"
    );
}
