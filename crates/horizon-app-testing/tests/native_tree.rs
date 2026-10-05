use horizon_app_testing::{Error, recipe::Target, tree::References};
const IOS_SOURCE: &str = r#"<?xml version="1.0"?><AppiumAUT><XCUIElementTypeApplication name="app" enabled="true" visible="true"><XCUIElementTypeButton name="menu.open" label="Menu" enabled="true" visible="true" x="0" y="10" width="40" height="20"/><XCUIElementTypeSecureTextField name="password" value="synthetic-password" enabled="true" visible="true"/></XCUIElementTypeApplication></AppiumAUT>"#;
const ANDROID_SOURCE: &str = r#"<hierarchy><node class="android.widget.Button" resource-id="menu.open" content-desc="Menu &amp; navigation" bounds="[0,10][40,30]" enabled="true" displayed="true"/><node class="android.widget.EditText" text="synthetic-password" password="true" clickable="true" enabled="true"/></hierarchy>"#;

#[test]
fn both_native_sources_normalize_bounds_and_redact_secure_values() -> Result<(), Error> {
    for source in [IOS_SOURCE, ANDROID_SOURCE] {
        let mut refs = References::default();
        let snapshot = refs.snapshot(source, "no.youpark.app")?;
        let menu = snapshot
            .nodes
            .iter()
            .find(|n| n.identifier == "menu.open")
            .ok_or(Error::TargetMissing)?;
        assert_eq!(menu.semantic.role, "button");
        let bounds = menu.semantic.bounds.ok_or(Error::SourceInvalid)?;
        assert_eq!(
            (bounds.x, bounds.y, bounds.width, bounds.height),
            (0.0, 10.0, 40.0, 20.0)
        );
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-password"));
        assert!(snapshot.nodes.iter().any(|node| node.semantic.role == "textbox"));
        assert!(refs.xpath(&Target::Ref(menu.semantic.reference.clone())).is_ok());
    }
    Ok(())
}

#[test]
fn references_cannot_cross_sessions_or_survive_a_new_or_failed_snapshot() -> Result<(), Error> {
    let mut first = References::default();
    let mut second = References::default();
    let snapshot = first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    let target = Target::Ref(snapshot.nodes[0].semantic.reference.clone());
    second.snapshot(IOS_SOURCE, "no.youpark.app")?;
    assert_eq!(second.xpath(&target).err(), Some(Error::ReferenceExpired));
    first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    assert_eq!(first.xpath(&target).err(), Some(Error::ReferenceExpired));
    let fresh = first.snapshot(IOS_SOURCE, "no.youpark.app")?;
    let current = Target::Ref(fresh.nodes[0].semantic.reference.clone());
    assert_eq!(
        first.snapshot("<unclosed>", "no.youpark.app").err(),
        Some(Error::SourceInvalid)
    );
    assert_eq!(first.xpath(&current).err(), Some(Error::ReferenceExpired));
    Ok(())
}

#[test]
fn malformed_sources_and_entity_declarations_never_resolve_targets() {
    for source in [
        "",
        "<a></b>",
        "<a/><b/>",
        "<!DOCTYPE a [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><a name='&x;'/>",
        "<a duplicate='1' duplicate='2'/>",
    ] {
        assert_eq!(
            References::default().snapshot(source, "app").err(),
            Some(Error::SourceInvalid)
        );
    }
}

#[test]
fn duplicate_identifiers_are_ambiguous_instead_of_selecting_a_random_node() -> Result<(), Error> {
    let mut refs = References::default();
    refs.snapshot("<a><b name='duplicate'/><b name='duplicate'/></a>", "app")?;
    assert_eq!(
        refs.xpath(&Target::Identifier("duplicate".into())).err(),
        Some(Error::TargetAmbiguous)
    );
    Ok(())
}

#[test]
fn secure_fields_redact_every_content_channel() -> Result<(), Error> {
    for source in [
        "<XCUIElementTypeSecureTextField name='synthetic-secret' label='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>",
        "<node class='android.widget.EditText' password='true' resource-id='synthetic-secret' content-desc='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>",
    ] {
        let snapshot = References::default().snapshot(source, "app")?;
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-secret"));
        assert!(snapshot.nodes[0].identifier.is_empty());
    }
    Ok(())
}

#[test]
fn android_editable_values_and_autocomplete_roles_preserve_secure_redaction() {
    let snapshot = References::default().snapshot("<hierarchy><node class='android.widget.EditText' text='populated' clickable='true'/><node class='android.widget.AutoCompleteTextView' text='suggestion' clickable='true'/><node class='android.widget.MultiAutoCompleteTextView' text='hidden-password' password='true' clickable='true'/></hierarchy>", "app").unwrap();
    for node in &snapshot.nodes[1..] {
        assert_eq!(node.semantic.role, "textbox");
    }
    assert_eq!(snapshot.nodes[1].value, "populated");
    assert_eq!(snapshot.nodes[2].value, "suggestion");
    assert_eq!(snapshot.nodes[3].value, "[redacted]");
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("hidden-password"));
}

#[test]
fn source_limits_and_broken_deep_trees_invalidate_prior_references() {
    for invalid in [
        format!("<a>{}</a>", "<b/>".repeat(2500)),
        format!("{}{}", "<a>".repeat(129), "</a>".repeat(129)),
        "x".repeat(8 * 1024 * 1024 + 1),
    ] {
        let mut refs = References::default();
        let snapshot = refs.snapshot(IOS_SOURCE, "app").unwrap();
        let target = Target::Ref(snapshot.nodes[0].semantic.reference.clone());
        assert_eq!(refs.snapshot(&invalid, "app").err(), Some(Error::SourceInvalid));
        assert_eq!(refs.xpath(&target).err(), Some(Error::ReferenceExpired));
    }
}

#[test]
fn overridden_secure_native_tags_never_expose_any_content_channel() {
    let snapshot = References::default().snapshot("<XCUIElementTypeSecureTextField class='plain' name='synthetic-secret' label='synthetic-secret' text='synthetic-secret' value='synthetic-secret'/>", "app").unwrap();
    assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-secret"));
    assert!(snapshot.nodes[0].identifier.is_empty());
    assert_eq!(snapshot.nodes[0].value, "[redacted]");
}

#[test]
fn finite_coordinates_with_overflowing_dimensions_do_not_publish_invalid_bounds() {
    for bounds in ["[-1e308,0][1e308,1]", "[0,-1e308][1,1e308]"] {
        let source = format!("<node class='android.widget.Button' bounds='{bounds}'/>");
        let snapshot = References::default().snapshot(&source, "app").unwrap();
        assert!(snapshot.nodes[0].semantic.bounds.is_none());
        serde_json::to_string(&snapshot).unwrap();
    }
}
