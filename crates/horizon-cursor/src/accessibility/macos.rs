//! macOS Accessibility insertion with an observed, single-use target.

use std::sync::{Mutex, PoisonError};

use axuielement::async_api::AXNotificationStream;
use axuielement::ax_attribute::{attributes, subroles};
use axuielement::ax_notification::{
    AX_APPLICATION_DEACTIVATED_NOTIFICATION, AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION,
    AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION,
};
use axuielement::{AXError, AXRange, AXUIElement, is_process_trusted, is_process_trusted_with_prompt, system_wide};
use objc2_app_kit::NSRunningApplication;

use super::InjectError;

const AX_TIMEOUT_SECONDS: f32 = 0.5;
const HORIZON_BUNDLE_IDENTIFIER: &str = "com.github.peters.horizon";
const HORIZON_EXECUTABLE_NAME: &str = "horizon";
const FOCUS_NOTIFICATIONS: &[&str] = &[
    AX_APPLICATION_DEACTIVATED_NOTIFICATION,
    AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION,
    AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION,
];

static PREPARED_TARGET: Mutex<Option<PreparedTarget>> = Mutex::new(None);

struct PreparedTarget {
    application_pid: i32,
    element: AXUIElement,
    focus_events: AXNotificationStream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetFacts {
    enabled: Option<bool>,
    editable: Option<bool>,
    secure: bool,
    selected_text_settable: bool,
    selection: Option<AXRange>,
    focused: Option<bool>,
}

impl TargetFacts {
    fn validate(self) -> Result<(), InjectError> {
        if self.secure {
            return Err(InjectError::Target("dictation into password fields is disabled"));
        }
        if self.enabled == Some(false) {
            return Err(InjectError::Target("focused field is not enabled"));
        }
        if self.editable == Some(false) || !self.selected_text_settable {
            return Err(InjectError::Target("focused field is not editable"));
        }
        let selection = self
            .selection
            .ok_or(InjectError::Target("focused field does not expose a text caret"))?;
        if selection.location < 0 {
            return Err(InjectError::Target("focused field reported an invalid caret"));
        }
        if selection.length != 0 {
            return Err(InjectError::Target(
                "selected text cannot be replaced safely by direct dictation",
            ));
        }
        if self.focused != Some(true) {
            return Err(InjectError::Target("focused accessibility target changed"));
        }
        Ok(())
    }
}

pub(super) fn capture_target() -> Result<(), InjectError> {
    {
        let slot = PREPARED_TARGET.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err(InjectError::Target("a previous desktop insertion is still pending"));
        }
    }

    let prepared = prepare_target()?;
    let mut slot = PREPARED_TARGET.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.is_some() {
        return Err(InjectError::Target("another desktop target was captured first"));
    }
    *slot = Some(prepared);
    Ok(())
}

pub(super) fn release_target() {
    PREPARED_TARGET.lock().unwrap_or_else(PoisonError::into_inner).take();
}

pub(super) fn permission_granted() -> bool {
    is_process_trusted()
}

pub(super) fn request_permission() -> bool {
    is_process_trusted_with_prompt()
}

pub(super) fn insert_text(text: &str, _expected_window: Option<u32>) -> Result<(), InjectError> {
    let mut slot = PREPARED_TARGET.lock().unwrap_or_else(PoisonError::into_inner);
    let prepared = slot
        .as_ref()
        .ok_or(InjectError::Target("no captured desktop field is available"))?;
    let result = if text.is_empty() {
        Ok(())
    } else {
        validate_prepared_target(prepared).and_then(|()| {
            prepared
                .element
                .set_string_attribute(attributes::AX_SELECTED_TEXT_ATTRIBUTE, text)
                .map_err(|_| insertion_error())
        })
    };
    slot.take();
    result
}

fn prepare_target() -> Result<PreparedTarget, InjectError> {
    require_permission()?;
    let system = system_wide().ok_or(InjectError::Unsupported)?;
    system
        .set_timeout(AX_TIMEOUT_SECONDS)
        .map_err(|_| transport_error("failed to configure macOS Accessibility"))?;
    let application = system
        .focused_application()
        .map_err(|_| transport_error("failed to inspect the focused application"))?
        .ok_or(InjectError::Target("no focused application is available"))?;
    let application_pid = application
        .pid()
        .map_err(|_| transport_error("failed to identify the focused application"))?;
    reject_horizon_application(application_pid)?;
    application
        .set_timeout(AX_TIMEOUT_SECONDS)
        .map_err(|_| transport_error("failed to configure the focused application"))?;

    // Subscribe before reading the field. Any away-and-back focus sequence
    // after this point remains buffered and invalidates the single-use target.
    let focus_events = AXNotificationStream::subscribe_many(&application, FOCUS_NOTIFICATIONS, 8)
        .map_err(|_| InjectError::Target("focused application cannot be observed safely"))?;
    ensure_focused_application(&system, application_pid)?;
    let element = system
        .focused_ui_element()
        .map_err(|_| transport_error("failed to inspect the focused field"))?
        .ok_or(InjectError::Target("no focused text field is available"))?;
    element
        .set_timeout(AX_TIMEOUT_SECONDS)
        .map_err(|_| transport_error("failed to configure the focused field"))?;
    ensure_target_pid(&element, application_pid)?;
    read_target_facts(&element)?.validate()?;
    ensure_no_focus_events(&focus_events)?;

    Ok(PreparedTarget {
        application_pid,
        element,
        focus_events,
    })
}

fn reject_horizon_application(application_pid: i32) -> Result<(), InjectError> {
    let application = NSRunningApplication::runningApplicationWithProcessIdentifier(application_pid)
        .ok_or_else(|| transport_error("failed to identify the focused application identity"))?;
    let bundle_identifier = application.bundleIdentifier().map(|identifier| identifier.to_string());
    let executable_name = application
        .executableURL()
        .and_then(|url| url.lastPathComponent())
        .map(|name| name.to_string());

    if application_identity_is_horizon(bundle_identifier.as_deref(), executable_name.as_deref()) {
        Err(InjectError::Target("focus is still inside Horizon"))
    } else {
        Ok(())
    }
}

fn application_identity_is_horizon(bundle_identifier: Option<&str>, executable_name: Option<&str>) -> bool {
    bundle_identifier == Some(HORIZON_BUNDLE_IDENTIFIER)
        || executable_name.is_some_and(|name| name.eq_ignore_ascii_case(HORIZON_EXECUTABLE_NAME))
}

fn validate_prepared_target(target: &PreparedTarget) -> Result<(), InjectError> {
    require_permission()?;
    ensure_no_focus_events(&target.focus_events)?;
    let system = system_wide().ok_or(InjectError::Unsupported)?;
    system
        .set_timeout(AX_TIMEOUT_SECONDS)
        .map_err(|_| transport_error("failed to configure macOS Accessibility"))?;
    ensure_focused_application(&system, target.application_pid)?;
    ensure_target_pid(&target.element, target.application_pid)?;
    // Focus is deliberately read last by `read_target_facts`, immediately
    // before the final observer check and the one mutating AX call.
    read_target_facts(&target.element)?.validate()?;
    ensure_no_focus_events(&target.focus_events)
}

fn read_target_facts(element: &AXUIElement) -> Result<TargetFacts, InjectError> {
    let enabled = optional_bool(element, attributes::AX_ENABLED_ATTRIBUTE)?;
    let editable = optional_bool(element, attributes::AX_IS_EDITABLE_ATTRIBUTE)?;
    let secure = optional_string(element, attributes::AX_SUBROLE_ATTRIBUTE)?
        .is_some_and(|subrole| subrole == subroles::AX_SECURE_TEXT_FIELD_SUBROLE);
    let selected_text_settable = element
        .is_attribute_settable(attributes::AX_SELECTED_TEXT_ATTRIBUTE)
        .map_err(|_| transport_error("failed to inspect focused field editability"))?;
    let selection = element
        .range_attribute(attributes::AX_SELECTED_TEXT_RANGE_ATTRIBUTE)
        .map_err(|_| transport_error("failed to inspect the focused field caret"))?;
    // Keep this last: it is the final synchronous focus observation before
    // the caller checks the notification stream.
    let focused = optional_bool(element, attributes::AX_FOCUSED_ATTRIBUTE)?;
    Ok(TargetFacts {
        enabled,
        editable,
        secure,
        selected_text_settable,
        selection,
        focused,
    })
}

fn optional_bool(element: &AXUIElement, attribute: &str) -> Result<Option<bool>, InjectError> {
    match element.bool_attribute(attribute) {
        Ok(value) => Ok(value),
        Err(AXError::AttributeUnsupported(_) | AXError::NoValue) => Ok(None),
        Err(_) => Err(transport_error("failed to inspect the focused field")),
    }
}

fn optional_string(element: &AXUIElement, attribute: &str) -> Result<Option<String>, InjectError> {
    match element.string_attribute(attribute) {
        Ok(value) => Ok(value),
        Err(AXError::AttributeUnsupported(_) | AXError::NoValue) => Ok(None),
        Err(_) => Err(transport_error("failed to inspect the focused field")),
    }
}

fn ensure_focused_application(system: &AXUIElement, expected_pid: i32) -> Result<(), InjectError> {
    let current = system
        .element_attribute(attributes::AX_FOCUSED_APPLICATION_ATTRIBUTE)
        .map_err(|_| transport_error("failed to recheck the focused application"))?
        .ok_or(InjectError::Target("focused application changed"))?;
    let pid = current
        .pid()
        .map_err(|_| transport_error("failed to recheck the focused application"))?;
    if pid == expected_pid {
        Ok(())
    } else {
        Err(InjectError::Target("focused application changed"))
    }
}

fn ensure_target_pid(element: &AXUIElement, expected_pid: i32) -> Result<(), InjectError> {
    let pid = element
        .pid()
        .map_err(|_| transport_error("focused field disappeared"))?;
    if pid == expected_pid {
        Ok(())
    } else {
        Err(InjectError::Target("focused accessibility target changed"))
    }
}

fn ensure_no_focus_events(events: &AXNotificationStream) -> Result<(), InjectError> {
    if events.is_closed() || events.buffered_count() != 0 {
        Err(InjectError::Target("focused accessibility target changed"))
    } else {
        Ok(())
    }
}

fn require_permission() -> Result<(), InjectError> {
    if is_process_trusted() {
        Ok(())
    } else {
        Err(InjectError::Target("macOS Accessibility permission is required"))
    }
}

const fn transport_error(message: &'static str) -> InjectError {
    InjectError::Failed(message)
}

fn insertion_error() -> InjectError {
    if is_process_trusted() {
        InjectError::Failed("focused field rejected accessibility insertion")
    } else {
        InjectError::Target("macOS Accessibility permission was revoked")
    }
}

#[cfg(test)]
mod tests {
    use super::{AXRange, HORIZON_BUNDLE_IDENTIFIER, InjectError, TargetFacts, application_identity_is_horizon};

    fn safe_target() -> TargetFacts {
        TargetFacts {
            enabled: Some(true),
            editable: Some(true),
            secure: false,
            selected_text_settable: true,
            selection: Some(AXRange { location: 4, length: 0 }),
            focused: Some(true),
        }
    }

    #[test]
    fn safe_caret_target_is_accepted() {
        assert_eq!(safe_target().validate(), Ok(()));
    }

    #[test]
    fn secure_read_only_selected_and_stale_targets_are_rejected() {
        let unsafe_targets = [
            TargetFacts {
                secure: true,
                ..safe_target()
            },
            TargetFacts {
                enabled: Some(false),
                ..safe_target()
            },
            TargetFacts {
                editable: Some(false),
                ..safe_target()
            },
            TargetFacts {
                selected_text_settable: false,
                ..safe_target()
            },
            TargetFacts {
                selection: Some(AXRange { location: 4, length: 2 }),
                ..safe_target()
            },
            TargetFacts {
                focused: Some(false),
                ..safe_target()
            },
        ];
        for target in unsafe_targets {
            assert!(matches!(target.validate(), Err(InjectError::Target(_))));
        }
    }

    #[test]
    fn missing_enabled_and_editable_hints_are_allowed_only_with_settable_selected_text() {
        assert_eq!(
            TargetFacts {
                enabled: None,
                editable: None,
                ..safe_target()
            }
            .validate(),
            Ok(())
        );
        assert!(matches!(
            TargetFacts {
                editable: None,
                selected_text_settable: false,
                ..safe_target()
            }
            .validate(),
            Err(InjectError::Target(_))
        ));
    }

    #[test]
    fn every_horizon_instance_is_rejected_by_stable_application_identity() {
        assert!(application_identity_is_horizon(
            Some(HORIZON_BUNDLE_IDENTIFIER),
            Some("renamed")
        ));
        assert!(application_identity_is_horizon(None, Some("horizon")));
        assert!(application_identity_is_horizon(None, Some("Horizon")));
        assert!(!application_identity_is_horizon(
            Some("com.apple.TextEdit"),
            Some("TextEdit")
        ));
    }
}
