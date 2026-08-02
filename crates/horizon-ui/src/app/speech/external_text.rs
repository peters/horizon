//! UI-thread ownership and safe insertion for macOS Accessibility targets.
//!
//! AX objects are deliberately kept behind opaque [`ExternalTargetId`] values.
//! The speech engine and its workers never receive an AX object, field value,
//! or window title.

#![cfg_attr(not(all(feature = "speech", target_os = "macos")), allow(dead_code))]

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use super::ExternalTargetId;

/// Result of inspecting the frontmost application at global-hotkey key-down.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusedTarget {
    /// A Horizon window is frontmost; use the existing terminal routing.
    Horizon,
    /// An exact external application/window/element tuple was retained.
    External(ExternalTargetId),
}

/// Privacy-preserving refusal reasons for external dictation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalTargetError {
    AccessibilityRequired,
    NoFocusedApplication,
    NoFocusedWindow,
    NoFocusedElement,
    Disabled,
    ReadOnly,
    SecureField,
    FocusChanged,
    TargetUnavailable,
    InsertionFailed,
    #[cfg(not(all(feature = "speech", target_os = "macos")))]
    UnsupportedPlatform,
}

impl fmt::Display for ExternalTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AccessibilityRequired => "macOS Accessibility permission is required",
            Self::NoFocusedApplication => "no focused application was available",
            Self::NoFocusedWindow => "no focused window was available",
            Self::NoFocusedElement => "no focused text field was available",
            Self::Disabled => "the focused field is disabled",
            Self::ReadOnly => "the focused field is read-only",
            Self::SecureField => "secure and password fields cannot receive dictation",
            Self::FocusChanged => "the focused field changed during dictation",
            Self::TargetUnavailable => "the focused field is no longer available",
            Self::InsertionFailed => "macOS refused selected-text insertion",
            #[cfg(not(all(feature = "speech", target_os = "macos")))]
            Self::UnsupportedPlatform => "external dictation is available on macOS only",
        })
    }
}

impl std::error::Error for ExternalTargetError {}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FocusSnapshot<Application, Window, Element> {
    application: Application,
    window: Window,
    element: Element,
    horizon_frontmost: bool,
}

type FocusResult<Application, Window, Element> =
    Result<FocusSnapshot<Application, Window, Element>, ExternalTargetError>;

trait ExternalTextAdapter {
    type Application: Clone + Eq;
    type Window: Clone + Eq;
    type Element: Clone + Eq;

    fn accessibility_trusted(&self) -> bool;
    fn focus_snapshot(&self) -> FocusResult<Self::Application, Self::Window, Self::Element>;
    fn validate_element(&self, element: &Self::Element) -> Result<(), ExternalTargetError>;
    fn insert_selected_text(&self, element: &Self::Element, text: &str) -> Result<(), ExternalTargetError>;
}

type AdapterSnapshot<A> = FocusSnapshot<
    <A as ExternalTextAdapter>::Application,
    <A as ExternalTextAdapter>::Window,
    <A as ExternalTextAdapter>::Element,
>;

const TARGET_REVALIDATION_INTERVAL: Duration = Duration::from_millis(100);

struct RetainedTarget<A: ExternalTextAdapter> {
    snapshot: AdapterSnapshot<A>,
    last_revalidated_at: Instant,
}

struct TargetRegistry<A: ExternalTextAdapter> {
    adapter: A,
    next_id: u64,
    targets: HashMap<ExternalTargetId, RetainedTarget<A>>,
}

impl<A: ExternalTextAdapter> TargetRegistry<A> {
    fn new(adapter: A) -> Self {
        Self {
            adapter,
            next_id: 1,
            targets: HashMap::new(),
        }
    }

    fn capture(&mut self) -> Result<FocusedTarget, ExternalTargetError> {
        self.capture_at(Instant::now())
    }

    fn capture_at(&mut self, now: Instant) -> Result<FocusedTarget, ExternalTargetError> {
        if !self.adapter.accessibility_trusted() {
            return Err(ExternalTargetError::AccessibilityRequired);
        }
        let snapshot = self.adapter.focus_snapshot()?;
        if snapshot.horizon_frontmost {
            return Ok(FocusedTarget::Horizon);
        }
        self.adapter.validate_element(&snapshot.element)?;

        let id = ExternalTargetId::from_raw(self.next_id);
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        self.targets.insert(
            id,
            RetainedTarget {
                snapshot,
                last_revalidated_at: now,
            },
        );
        Ok(FocusedTarget::External(id))
    }

    fn validate(&self, id: ExternalTargetId) -> Result<(), ExternalTargetError> {
        if !self.adapter.accessibility_trusted() {
            return Err(ExternalTargetError::AccessibilityRequired);
        }
        let retained = self.targets.get(&id).ok_or(ExternalTargetError::TargetUnavailable)?;
        let current = self.adapter.focus_snapshot()?;
        if current.horizon_frontmost
            || current.application != retained.snapshot.application
            || current.window != retained.snapshot.window
            || current.element != retained.snapshot.element
        {
            return Err(ExternalTargetError::FocusChanged);
        }
        self.adapter.validate_element(&retained.snapshot.element)
    }

    fn validate_if_due(
        &mut self,
        id: ExternalTargetId,
        now: Instant,
        interval: Duration,
    ) -> Result<bool, ExternalTargetError> {
        let last_revalidated_at = self
            .targets
            .get(&id)
            .ok_or(ExternalTargetError::TargetUnavailable)?
            .last_revalidated_at;
        if now.saturating_duration_since(last_revalidated_at) < interval {
            return Ok(false);
        }
        self.validate(id)?;
        if let Some(retained) = self.targets.get_mut(&id) {
            retained.last_revalidated_at = now;
        }
        Ok(true)
    }

    fn insert(&self, id: ExternalTargetId, text: &str) -> Result<(), ExternalTargetError> {
        self.validate(id)?;
        let retained = self.targets.get(&id).ok_or(ExternalTargetError::TargetUnavailable)?;
        self.adapter.insert_selected_text(&retained.snapshot.element, text)
    }

    fn release(&mut self, id: ExternalTargetId) {
        self.targets.remove(&id);
    }

    fn clear(&mut self) {
        self.targets.clear();
    }
}

/// Main-thread registry for external Accessibility targets.
pub struct ExternalTextTargets {
    registry: TargetRegistry<PlatformAdapter>,
    #[cfg(test)]
    injected_capture: Option<Result<FocusedTarget, ExternalTargetError>>,
}

impl ExternalTextTargets {
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: TargetRegistry::new(PlatformAdapter::new()),
            #[cfg(test)]
            injected_capture: None,
        }
    }

    /// Capture and validate the exact frontmost field without prompting.
    pub fn capture(&mut self) -> Result<FocusedTarget, ExternalTargetError> {
        #[cfg(test)]
        if let Some(capture) = self.injected_capture.take() {
            return capture;
        }
        self.registry.capture()
    }

    #[cfg(test)]
    pub(in crate::app) fn inject_capture(&mut self, capture: Result<FocusedTarget, ExternalTargetError>) {
        self.injected_capture = Some(capture);
    }

    /// Revalidate application, window, element, permission, and editability at
    /// most once per active-speech poll interval. Insertion always performs a
    /// separate, fresh validation regardless of this throttle.
    pub fn revalidate_if_due(&mut self, id: ExternalTargetId) -> Result<(), ExternalTargetError> {
        self.registry
            .validate_if_due(id, Instant::now(), TARGET_REVALIDATION_INTERVAL)
            .map(|_| ())
    }

    /// Insert through `AXSelectedText`. `text_with_space` must already include
    /// the caller's desired trailing space; this method never adds Enter.
    pub fn insert_selected_text(&self, id: ExternalTargetId, text_with_space: &str) -> Result<(), ExternalTargetError> {
        self.registry.insert(id, text_with_space)
    }

    pub fn release(&mut self, id: ExternalTargetId) {
        self.registry.release(id);
    }

    pub fn clear(&mut self) {
        self.registry.clear();
    }
}

impl Default for ExternalTextTargets {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(feature = "speech", target_os = "macos"))]
mod platform {
    use accessibility::{AXAttribute, AXUIElement, AXUIElementAttributes};
    use core_foundation::{
        base::{CFType, TCFType},
        boolean::CFBoolean,
        string::CFString,
    };
    use macos_accessibility_client::accessibility::application_is_trusted;

    use super::{ExternalTargetError, ExternalTextAdapter, FocusSnapshot};

    const FOCUSED_APPLICATION: &str = "AXFocusedApplication";
    const FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";
    const IS_EDITABLE: &str = "AXIsEditable";
    const SELECTED_TEXT: &str = "AXSelectedText";
    const SECURE_TEXT_FIELD: &str = "AXSecureTextField";

    pub(super) struct AxAdapter;

    impl AxAdapter {
        pub(super) const fn new() -> Self {
            Self
        }

        fn attribute(name: &'static str) -> AXAttribute<CFType> {
            AXAttribute::new(&CFString::from_static_string(name))
        }

        fn element_attribute(
            element: &AXUIElement,
            name: &'static str,
            missing: ExternalTargetError,
        ) -> Result<AXUIElement, ExternalTargetError> {
            let value = element
                .attribute(&Self::attribute(name))
                .map_err(|_| Self::ax_error(missing))?;
            value.downcast_into::<AXUIElement>().ok_or(missing)
        }

        fn ax_error(fallback: ExternalTargetError) -> ExternalTargetError {
            if application_is_trusted() {
                fallback
            } else {
                ExternalTargetError::AccessibilityRequired
            }
        }
    }

    impl ExternalTextAdapter for AxAdapter {
        type Application = AXUIElement;
        type Window = AXUIElement;
        type Element = AXUIElement;

        fn accessibility_trusted(&self) -> bool {
            application_is_trusted()
        }

        fn focus_snapshot(&self) -> Result<FocusSnapshot<AXUIElement, AXUIElement, AXUIElement>, ExternalTargetError> {
            if !application_is_trusted() {
                return Err(ExternalTargetError::AccessibilityRequired);
            }
            let system = AXUIElement::system_wide();
            let application =
                Self::element_attribute(&system, FOCUSED_APPLICATION, ExternalTargetError::NoFocusedApplication)?;
            let horizon_application = AXUIElement::application(std::process::id().cast_signed());
            if application == horizon_application {
                return Ok(FocusSnapshot {
                    application: application.clone(),
                    window: application.clone(),
                    element: application,
                    horizon_frontmost: true,
                });
            }
            let window = application
                .focused_window()
                .map_err(|_| Self::ax_error(ExternalTargetError::NoFocusedWindow))?;
            let element =
                Self::element_attribute(&application, FOCUSED_UI_ELEMENT, ExternalTargetError::NoFocusedElement)?;
            Ok(FocusSnapshot {
                horizon_frontmost: false,
                application,
                window,
                element,
            })
        }

        fn validate_element(&self, element: &AXUIElement) -> Result<(), ExternalTargetError> {
            if !application_is_trusted() {
                return Err(ExternalTargetError::AccessibilityRequired);
            }
            let enabled = element
                .enabled()
                .map(bool::from)
                .map_err(|_| Self::ax_error(ExternalTargetError::TargetUnavailable))?;
            if !enabled {
                return Err(ExternalTargetError::Disabled);
            }
            if element
                .subrole()
                .is_ok_and(|subrole| subrole.to_string() == SECURE_TEXT_FIELD)
            {
                return Err(ExternalTargetError::SecureField);
            }

            // AXIsEditable is not exposed by every application. When present,
            // false is authoritative; otherwise AXSelectedText settability is
            // the fail-closed editability contract below.
            match element.attribute(&Self::attribute(IS_EDITABLE)) {
                Ok(value) => {
                    if value
                        .downcast_into::<CFBoolean>()
                        .is_some_and(|editable| !bool::from(editable))
                    {
                        return Err(ExternalTargetError::ReadOnly);
                    }
                }
                Err(_) if !application_is_trusted() => {
                    return Err(ExternalTargetError::AccessibilityRequired);
                }
                Err(_) => {}
            }

            let selected_text = Self::attribute(SELECTED_TEXT);
            let settable = element
                .is_settable(&selected_text)
                .map_err(|_| Self::ax_error(ExternalTargetError::TargetUnavailable))?;
            if !settable {
                return Err(ExternalTargetError::ReadOnly);
            }
            Ok(())
        }

        fn insert_selected_text(&self, element: &AXUIElement, text: &str) -> Result<(), ExternalTargetError> {
            element
                .set_attribute(&Self::attribute(SELECTED_TEXT), CFString::new(text).into_CFType())
                .map_err(|_| Self::ax_error(ExternalTargetError::InsertionFailed))
        }
    }
}

#[cfg(all(feature = "speech", target_os = "macos"))]
use platform::AxAdapter as PlatformAdapter;

#[cfg(not(all(feature = "speech", target_os = "macos")))]
struct PlatformAdapter;

#[cfg(not(all(feature = "speech", target_os = "macos")))]
impl PlatformAdapter {
    const fn new() -> Self {
        Self
    }
}

#[cfg(not(all(feature = "speech", target_os = "macos")))]
impl ExternalTextAdapter for PlatformAdapter {
    type Application = ();
    type Window = ();
    type Element = ();

    fn accessibility_trusted(&self) -> bool {
        false
    }

    fn focus_snapshot(&self) -> Result<FocusSnapshot<(), (), ()>, ExternalTargetError> {
        Err(ExternalTargetError::UnsupportedPlatform)
    }

    fn validate_element(&self, _element: &()) -> Result<(), ExternalTargetError> {
        Err(ExternalTargetError::UnsupportedPlatform)
    }

    fn insert_selected_text(&self, _element: &(), _text: &str) -> Result<(), ExternalTargetError> {
        Err(ExternalTargetError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct FakeFocus {
        application: u8,
        window: u8,
        element: u8,
        horizon: bool,
    }

    #[derive(Clone)]
    struct FakeAdapter(Rc<RefCell<FakeState>>);

    struct FakeState {
        trusted: bool,
        focus: Option<FakeFocus>,
        refusal: Option<ExternalTargetError>,
        insertion_error: Option<ExternalTargetError>,
        inserted: Vec<(u8, String)>,
        focus_snapshot_calls: usize,
        element_validation_calls: usize,
    }

    impl FakeAdapter {
        fn new() -> (Self, Rc<RefCell<FakeState>>) {
            let state = Rc::new(RefCell::new(FakeState {
                trusted: true,
                focus: Some(FakeFocus {
                    application: 1,
                    window: 2,
                    element: 3,
                    horizon: false,
                }),
                refusal: None,
                insertion_error: None,
                inserted: Vec::new(),
                focus_snapshot_calls: 0,
                element_validation_calls: 0,
            }));
            (Self(Rc::clone(&state)), state)
        }
    }

    impl ExternalTextAdapter for FakeAdapter {
        type Application = u8;
        type Window = u8;
        type Element = u8;

        fn accessibility_trusted(&self) -> bool {
            self.0.borrow().trusted
        }

        fn focus_snapshot(&self) -> Result<FocusSnapshot<u8, u8, u8>, ExternalTargetError> {
            let mut state = self.0.borrow_mut();
            state.focus_snapshot_calls += 1;
            let focus = state.focus.as_ref().ok_or(ExternalTargetError::NoFocusedElement)?;
            Ok(FocusSnapshot {
                application: focus.application,
                window: focus.window,
                element: focus.element,
                horizon_frontmost: focus.horizon,
            })
        }

        fn validate_element(&self, _element: &u8) -> Result<(), ExternalTargetError> {
            let mut state = self.0.borrow_mut();
            state.element_validation_calls += 1;
            state.refusal.map_or(Ok(()), Err)
        }

        fn insert_selected_text(&self, element: &u8, text: &str) -> Result<(), ExternalTargetError> {
            let mut state = self.0.borrow_mut();
            if let Some(error) = state.insertion_error {
                return Err(error);
            }
            state.inserted.push((*element, text.to_string()));
            Ok(())
        }
    }

    fn captured_id(registry: &mut TargetRegistry<FakeAdapter>) -> ExternalTargetId {
        match registry.capture() {
            Ok(FocusedTarget::External(id)) => id,
            other => panic!("expected external target, got {other:?}"),
        }
    }

    #[test]
    fn exact_target_revalidates_and_inserts_selected_text() {
        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let id = captured_id(&mut registry);
        registry.insert(id, "hello ").expect("same target inserts");
        assert_eq!(state.borrow().inserted, vec![(3, "hello ".to_string())]);
    }

    #[test]
    fn active_revalidation_is_throttled_but_insertion_is_always_fresh() {
        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let captured_at = Instant::now();
        let id = match registry.capture_at(captured_at) {
            Ok(FocusedTarget::External(id)) => id,
            other => panic!("expected external target, got {other:?}"),
        };
        assert_eq!(state.borrow().focus_snapshot_calls, 1);
        assert_eq!(state.borrow().element_validation_calls, 1);

        assert_eq!(
            registry.validate_if_due(
                id,
                captured_at + Duration::from_millis(99),
                TARGET_REVALIDATION_INTERVAL,
            ),
            Ok(false)
        );
        assert_eq!(state.borrow().focus_snapshot_calls, 1);
        assert_eq!(state.borrow().element_validation_calls, 1);

        assert_eq!(
            registry.validate_if_due(
                id,
                captured_at + TARGET_REVALIDATION_INTERVAL,
                TARGET_REVALIDATION_INTERVAL,
            ),
            Ok(true)
        );
        assert_eq!(state.borrow().focus_snapshot_calls, 2);
        assert_eq!(state.borrow().element_validation_calls, 2);
        assert_eq!(
            registry.validate_if_due(
                id,
                captured_at + TARGET_REVALIDATION_INTERVAL,
                TARGET_REVALIDATION_INTERVAL,
            ),
            Ok(false)
        );

        registry.insert(id, "fresh ").expect("insertion revalidates");
        assert_eq!(state.borrow().focus_snapshot_calls, 3);
        assert_eq!(state.borrow().element_validation_calls, 3);
        assert_eq!(state.borrow().inserted, vec![(3, "fresh ".to_string())]);
    }

    #[test]
    fn any_application_window_or_element_change_refuses_without_fallback() {
        for change in [
            FakeFocus {
                application: 9,
                window: 2,
                element: 3,
                horizon: false,
            },
            FakeFocus {
                application: 1,
                window: 9,
                element: 3,
                horizon: false,
            },
            FakeFocus {
                application: 1,
                window: 2,
                element: 9,
                horizon: false,
            },
            FakeFocus {
                application: 1,
                window: 2,
                element: 3,
                horizon: true,
            },
        ] {
            let (adapter, state) = FakeAdapter::new();
            let mut registry = TargetRegistry::new(adapter);
            let id = captured_id(&mut registry);
            state.borrow_mut().focus = Some(change);
            assert_eq!(registry.insert(id, "discard "), Err(ExternalTargetError::FocusChanged));
            assert!(state.borrow().inserted.is_empty());
        }
    }

    #[test]
    fn unsafe_fields_and_revoked_permission_fail_closed() {
        for refusal in [
            ExternalTargetError::Disabled,
            ExternalTargetError::ReadOnly,
            ExternalTargetError::SecureField,
        ] {
            let (adapter, state) = FakeAdapter::new();
            state.borrow_mut().refusal = Some(refusal);
            let mut registry = TargetRegistry::new(adapter);
            assert_eq!(registry.capture(), Err(refusal));
        }

        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let id = captured_id(&mut registry);
        state.borrow_mut().trusted = false;
        assert_eq!(registry.validate(id), Err(ExternalTargetError::AccessibilityRequired));
    }

    #[test]
    fn closed_changed_or_insertion_failed_targets_never_fall_back() {
        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let id = captured_id(&mut registry);
        state.borrow_mut().focus = None;
        assert_eq!(
            registry.insert(id, "discard "),
            Err(ExternalTargetError::NoFocusedElement)
        );
        assert!(state.borrow().inserted.is_empty());

        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let id = captured_id(&mut registry);
        state.borrow_mut().refusal = Some(ExternalTargetError::ReadOnly);
        assert_eq!(registry.insert(id, "discard "), Err(ExternalTargetError::ReadOnly));
        assert!(state.borrow().inserted.is_empty());

        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let id = captured_id(&mut registry);
        state.borrow_mut().insertion_error = Some(ExternalTargetError::InsertionFailed);
        assert_eq!(
            registry.insert(id, "discard "),
            Err(ExternalTargetError::InsertionFailed)
        );
        assert!(state.borrow().inserted.is_empty());
    }

    #[test]
    fn horizon_focus_does_not_allocate_an_external_target() {
        let (adapter, state) = FakeAdapter::new();
        state.borrow_mut().focus.as_mut().expect("focus").horizon = true;
        let mut registry = TargetRegistry::new(adapter);
        assert_eq!(registry.capture(), Ok(FocusedTarget::Horizon));
        assert!(registry.targets.is_empty());
    }

    #[test]
    fn release_and_clear_drop_ax_ownership() {
        let (adapter, state) = FakeAdapter::new();
        let mut registry = TargetRegistry::new(adapter);
        let first = captured_id(&mut registry);
        state.borrow_mut().focus.as_mut().expect("focus").element = 4;
        let second = captured_id(&mut registry);
        registry.release(first);
        assert!(registry.targets.contains_key(&second));
        registry.release(second);
        assert!(registry.targets.is_empty());
        let third = captured_id(&mut registry);
        assert!(registry.targets.contains_key(&third));
        registry.clear();
        assert!(registry.targets.is_empty());
    }
}
