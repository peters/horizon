//! Direct insertion into focused platform accessibility objects.

use crate::InjectError;

/// Prepare focused-target handling before recording starts.
///
/// macOS validates and retains the exact Accessibility object until insertion.
/// Linux only probes insertion ownership here and defers focused-target and
/// AT-SPI validation until transcript delivery.
///
/// # Errors
///
/// On macOS, returns a privacy-preserving target refusal. Linux returns an
/// error only when another desktop insertion is pending.
pub fn capture_focused_accessible_target() -> Result<(), InjectError> {
    platform::capture_target()
}

/// Discard a previously captured target without inserting text.
pub fn release_focused_accessible_target() {
    platform::release_target();
}

/// Whether macOS currently trusts Horizon for Accessibility access.
///
/// `None` means this platform does not use macOS Accessibility permission.
#[must_use]
pub fn accessibility_permission_granted() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(platform::permission_granted())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Ask macOS to show its standard Accessibility permission prompt.
///
/// Returns the trust state reported immediately after the request. Other
/// platforms return `None` and never prompt.
#[must_use]
pub fn request_accessibility_permission() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(platform::request_permission())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Insert text at the caret of the focused editable accessibility object.
///
/// This path never reads or writes the clipboard. It refuses ambiguous,
/// protected, stale, hidden, read-only, or selected-text targets so failure
/// cannot partially replace existing text.
///
/// # Errors
///
/// Returns [`InjectError::Unsupported`] when no Linux AT-SPI session is
/// available, [`InjectError::Target`] when the focused object is unsafe or
/// unsupported, and [`InjectError::Failed`] for accessibility transport
/// failures.
pub fn insert_text_into_focused_accessible(text: &str) -> Result<(), InjectError> {
    platform::insert_text(text)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::{HashSet, VecDeque};
    use std::future::Future;
    use std::sync::{Mutex, TryLockError};
    use std::time::Duration;

    use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt as _};
    use atspi::proxy::proxy_ext::ProxyExt as _;
    use atspi::proxy::text::TextProxy;
    use atspi::{
        AccessibilityConnection, Interface, InterfaceSet, MatchType, ObjectMatchRule, ObjectRefOwned, Role, SortOrder,
        State, StateSet,
    };

    use super::InjectError;

    const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(5);
    const MAX_TRAVERSED_OBJECTS: usize = 2_048;
    static INSERT_LOCK: Mutex<()> = Mutex::new(());

    pub(super) fn capture_target() -> Result<(), InjectError> {
        match INSERT_LOCK.try_lock() {
            Ok(guard) => {
                drop(guard);
                Ok(())
            }
            Err(TryLockError::Poisoned(error)) => {
                drop(error.into_inner());
                Ok(())
            }
            Err(TryLockError::WouldBlock) => Err(InjectError::Target("another desktop insertion is still pending")),
        }
    }

    pub(super) const fn release_target() {}

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TargetFacts {
        interfaces: InterfaceSet,
        states: StateSet,
        role: Role,
    }

    impl TargetFacts {
        fn new(interfaces: InterfaceSet, states: StateSet, role: Role) -> Self {
            Self {
                interfaces,
                states,
                role,
            }
        }

        fn is_focused_text_target(self) -> bool {
            self.states.contains(State::Focused) && self.interfaces.contains(Interface::Text | Interface::EditableText)
        }

        fn validate(self) -> Result<(), InjectError> {
            if self.role == Role::PasswordText {
                return Err(InjectError::Target("dictation into password fields is disabled"));
            }
            if self.states.intersects(State::Defunct | State::Stale) {
                return Err(InjectError::Target("focused accessibility target is stale"));
            }
            if !self.states.contains(State::Focused) {
                return Err(InjectError::Target("focused accessibility target changed"));
            }
            if !self.interfaces.contains(Interface::Text | Interface::EditableText) {
                return Err(InjectError::Target(
                    "focused field does not support direct text insertion",
                ));
            }
            if !self.states.contains(State::Editable) || self.states.contains(State::ReadOnly) {
                return Err(InjectError::Target("focused field is not editable"));
            }
            if !self.states.contains(State::Enabled | State::Sensitive) {
                return Err(InjectError::Target("focused field is not available for input"));
            }
            if !self.states.contains(State::Visible | State::Showing) {
                return Err(InjectError::Target("focused field is not visible"));
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TextSnapshot {
        caret: i32,
        character_count: i32,
        selection_count: i32,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct InsertionRequest {
        position: i32,
        length: i32,
    }

    impl TextSnapshot {
        fn request(self, text: &str) -> Result<InsertionRequest, InjectError> {
            if self.selection_count != 0 {
                return Err(InjectError::Target(
                    "selected text cannot be replaced safely by direct dictation",
                ));
            }
            if self.character_count < 0 || self.caret < 0 || self.caret > self.character_count {
                return Err(InjectError::Target("focused field reported an invalid caret"));
            }
            // AT-SPI's position is a character offset, but InsertText's length
            // is explicitly the UTF-8 byte count.
            let length = i32::try_from(text.len())
                .map_err(|_| InjectError::Target("speech transcript is too large to insert"))?;
            Ok(InsertionRequest {
                position: self.caret,
                length,
            })
        }
    }

    pub(super) fn insert_text(text: &str) -> Result<(), InjectError> {
        if text.is_empty() {
            return Ok(());
        }
        let _guard = match INSERT_LOCK.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(InjectError::Target("another desktop insertion is still pending"));
            }
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| InjectError::Failed("failed to start accessibility runtime"))?;
        runtime.block_on(insert_text_async(text))
    }

    async fn insert_text_async(text: &str) -> Result<(), InjectError> {
        let deadline = tokio::time::Instant::now() + PREFLIGHT_TIMEOUT;
        let connection = bounded_preflight(deadline, async {
            AccessibilityConnection::new()
                .await
                .map_err(|_| InjectError::Unsupported)
        })
        .await?;
        let target = bounded_preflight(deadline, find_focused_target(&connection)).await?;
        let accessible = bounded_preflight(deadline, async {
            target
                .as_accessible_proxy(connection.connection())
                .await
                .map_err(|_| InjectError::Failed("focused accessibility target disappeared"))
        })
        .await?;
        let initial_target = bounded_preflight(deadline, read_target_facts(&accessible)).await?;
        initial_target.validate()?;

        let proxies = bounded_preflight(deadline, async {
            accessible
                .proxies()
                .await
                .map_err(|_| InjectError::Failed("failed to inspect focused field interfaces"))
        })
        .await?;
        let text_proxy = bounded_preflight(deadline, async {
            proxies
                .text()
                .await
                .map_err(|_| InjectError::Target("focused field does not expose readable text state"))
        })
        .await?;
        let editable_proxy = bounded_preflight(deadline, async {
            proxies
                .editable_text()
                .await
                .map_err(|_| InjectError::Target("focused field does not support direct text insertion"))
        })
        .await?;

        let initial_text = bounded_preflight(deadline, read_text_snapshot(&text_proxy)).await?;
        let request = initial_text.request(text)?;

        let checked_text = bounded_preflight(deadline, read_text_snapshot(&text_proxy)).await?;
        let checked_request = checked_text.request(text)?;
        if checked_text != initial_text || checked_request != request {
            return Err(InjectError::Target("focused field changed before transcript insertion"));
        }

        let final_target = bounded_preflight(deadline, read_target_facts(&accessible)).await?;
        final_target.validate()?;
        if final_target != initial_target {
            return Err(InjectError::Target("focused field changed before transcript insertion"));
        }

        // Once this mutating call is sent, keep INSERT_LOCK until its reply.
        // Timing out locally could report failure while a late insertion still
        // completes, allowing a later transcript to overtake or duplicate it.
        let inserted = editable_proxy
            .insert_text(request.position, text, request.length)
            .await
            .map_err(|_| InjectError::Failed("focused field rejected accessibility insertion"))?;
        if !inserted {
            return Err(InjectError::Target("focused field rejected transcript insertion"));
        }
        Ok(())
    }

    async fn bounded_preflight<T, F>(deadline: tokio::time::Instant, operation: F) -> Result<T, InjectError>
    where
        F: Future<Output = Result<T, InjectError>>,
    {
        tokio::time::timeout_at(deadline, operation)
            .await
            .map_err(|_| InjectError::Failed("accessibility preflight timed out"))?
    }

    async fn find_focused_target(connection: &AccessibilityConnection) -> Result<ObjectRefOwned, InjectError> {
        let registry = connection
            .root_accessible_on_registry()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect the accessibility registry"))?;
        let applications = registry
            .get_children()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect accessible applications"))?;
        let mut candidates = Vec::new();
        for application in applications {
            if application.is_null() {
                continue;
            }
            let Ok(root) = application.as_accessible_proxy(connection.connection()).await else {
                continue;
            };
            let discovered = match collection_candidates(&root).await {
                Some(candidates) if !candidates.is_empty() => candidates,
                Some(_) | None => {
                    let Some(candidates) = traversal_candidates(application, connection.connection()).await else {
                        continue;
                    };
                    candidates
                }
            };
            for candidate in discovered {
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
            if candidates.len() > 1 {
                break;
            }
        }
        let index = unique_candidate_index(candidates.len())?;
        Ok(candidates.swap_remove(index))
    }

    async fn collection_candidates(root: &AccessibleProxy<'_>) -> Option<Vec<ObjectRefOwned>> {
        let proxies = root.proxies().await.ok()?;
        let collection = proxies.collection().await.ok()?;
        let rule = ObjectMatchRule::builder()
            .states([State::Focused], MatchType::All)
            .interfaces([Interface::Text, Interface::EditableText], MatchType::All)
            .build();
        collection.get_matches(rule, SortOrder::Canonical, 2, true).await.ok()
    }

    async fn traversal_candidates(
        root: ObjectRefOwned,
        connection: &atspi::zbus::Connection,
    ) -> Option<Vec<ObjectRefOwned>> {
        let mut queue = VecDeque::from([root]);
        let mut visited = HashSet::new();
        let mut candidates = Vec::new();
        while let Some(object) = queue.pop_front() {
            if object.is_null() || !visited.insert(object.clone()) {
                continue;
            }
            if visited.len() > MAX_TRAVERSED_OBJECTS {
                // The bound is per application. Discard the partial result so
                // it cannot hide a second candidate, then let the registry
                // search continue with other applications.
                return None;
            }
            let Ok(accessible) = object.as_accessible_proxy(connection).await else {
                continue;
            };
            let Ok(interfaces) = accessible.get_interfaces().await else {
                continue;
            };
            let Ok(states) = accessible.get_state().await else {
                continue;
            };
            if TargetFacts::new(interfaces, states, Role::Invalid).is_focused_text_target() {
                candidates.push(object.clone());
                if candidates.len() > 1 {
                    return Some(candidates);
                }
            }
            if states.intersects(State::Defunct | State::Stale) {
                continue;
            }
            if let Ok(children) = accessible.get_children().await {
                queue.extend(children);
            }
        }
        Some(candidates)
    }

    fn unique_candidate_index(candidate_count: usize) -> Result<usize, InjectError> {
        if candidate_count == 0 {
            return Err(InjectError::Target(
                "focused application does not expose an editable text field",
            ));
        }
        if candidate_count > 1 {
            return Err(InjectError::Target("multiple focused editable fields were reported"));
        }
        Ok(0)
    }

    async fn read_target_facts(accessible: &AccessibleProxy<'_>) -> Result<TargetFacts, InjectError> {
        let interfaces = accessible
            .get_interfaces()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field interfaces"))?;
        let role = accessible
            .get_role()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field role"))?;
        // Read state last so the focus observation is the final D-Bus round
        // trip when this helper guards the mutation boundary.
        let states = accessible
            .get_state()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field state"))?;
        Ok(TargetFacts::new(interfaces, states, role))
    }

    async fn read_text_snapshot(text: &TextProxy<'_>) -> Result<TextSnapshot, InjectError> {
        let selection_count = text
            .get_n_selections()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field selection"))?;
        let caret = text
            .caret_offset()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field caret"))?;
        let character_count = text
            .character_count()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field length"))?;
        Ok(TextSnapshot {
            caret,
            character_count,
            selection_count,
        })
    }

    #[cfg(test)]
    mod tests {
        use std::sync::PoisonError;

        use atspi::{Interface, InterfaceSet, Role, State, StateSet};

        use super::{INSERT_LOCK, InjectError, TargetFacts, TextSnapshot, unique_candidate_index};

        fn safe_target() -> TargetFacts {
            TargetFacts {
                interfaces: InterfaceSet::new(Interface::Text | Interface::EditableText),
                states: StateSet::new(
                    State::Focused
                        | State::Editable
                        | State::Enabled
                        | State::Sensitive
                        | State::Visible
                        | State::Showing,
                ),
                role: Role::Entry,
            }
        }

        #[test]
        fn focused_candidate_must_be_unique() {
            assert_eq!(unique_candidate_index(1), Ok(0));
            assert_eq!(
                unique_candidate_index(2),
                Err(InjectError::Target("multiple focused editable fields were reported"))
            );
            assert_eq!(
                unique_candidate_index(0),
                Err(InjectError::Target(
                    "focused application does not expose an editable text field"
                ))
            );
        }

        #[test]
        fn protected_and_unsafe_targets_are_rejected() {
            let password = TargetFacts {
                role: Role::PasswordText,
                ..safe_target()
            };
            let mut stale = safe_target();
            stale.states.insert(State::Stale);
            let mut defunct = safe_target();
            defunct.states.insert(State::Defunct);
            let mut read_only = safe_target();
            read_only.states.insert(State::ReadOnly);
            let mut not_editable = safe_target();
            not_editable.states.remove(State::Editable);
            let mut disabled = safe_target();
            disabled.states.remove(State::Enabled);
            let mut insensitive = safe_target();
            insensitive.states.remove(State::Sensitive);
            let mut hidden = safe_target();
            hidden.states.remove(State::Showing);
            let mut invisible = safe_target();
            invisible.states.remove(State::Visible);
            let mut unfocused = safe_target();
            unfocused.states.remove(State::Focused);
            let missing_interface = TargetFacts {
                interfaces: InterfaceSet::new(Interface::Text),
                ..safe_target()
            };
            for unsafe_target in [
                password,
                stale,
                defunct,
                read_only,
                not_editable,
                disabled,
                insensitive,
                hidden,
                invisible,
                unfocused,
                missing_interface,
            ] {
                assert!(matches!(unsafe_target.validate(), Err(InjectError::Target(_))));
            }
            assert_eq!(safe_target().validate(), Ok(()));
        }

        #[test]
        fn empty_text_is_a_no_op_without_an_accessibility_session() {
            assert_eq!(super::insert_text(""), Ok(()));
        }

        #[test]
        fn selected_text_is_never_partially_replaced() {
            let snapshot = TextSnapshot {
                caret: 4,
                character_count: 8,
                selection_count: 1,
            };
            assert_eq!(
                snapshot.request("hello"),
                Err(InjectError::Target(
                    "selected text cannot be replaced safely by direct dictation"
                ))
            );
        }

        #[test]
        fn unicode_length_uses_utf8_bytes_required_by_at_spi() {
            let snapshot = TextSnapshot {
                caret: 3,
                character_count: 7,
                selection_count: 0,
            };
            let request = snapshot.request("blå 🌊").expect("valid insertion request");
            assert_eq!(request.position, 3);
            assert_eq!(request.length, 9);
        }

        #[test]
        fn invalid_caret_is_rejected_before_mutation() {
            let snapshot = TextSnapshot {
                caret: 9,
                character_count: 8,
                selection_count: 0,
            };
            assert_eq!(
                snapshot.request("hello"),
                Err(InjectError::Target("focused field reported an invalid caret"))
            );
        }

        #[test]
        fn concurrent_insertion_is_rejected_before_starting_a_runtime() {
            let _guard = INSERT_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
            assert_eq!(
                super::insert_text("hello"),
                Err(InjectError::Target("another desktop insertion is still pending"))
            );
        }

        #[test]
        #[ignore = "live AT-SPI editable-field smoke"]
        fn live_accessibility_insert() {
            super::insert_text("direct blå final marker zephyr").expect("accessibility insertion");
        }

        #[test]
        #[ignore = "live AT-SPI unsafe-target smoke"]
        fn live_accessibility_rejects_unsafe_target() {
            assert!(matches!(
                super::insert_text("must not appear"),
                Err(InjectError::Target(_))
            ));
        }

        #[test]
        #[ignore = "live unavailable-AT-SPI smoke"]
        fn live_unavailable_accessibility_bus_is_unsupported() {
            assert_eq!(super::insert_text("must not appear"), Err(InjectError::Unsupported));
        }
    }
}

#[cfg(target_os = "macos")]
#[path = "accessibility/macos.rs"]
mod platform;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::InjectError;

    pub(super) const fn capture_target() -> Result<(), InjectError> {
        Err(InjectError::Unsupported)
    }

    pub(super) const fn release_target() {}

    pub(super) fn insert_text(_text: &str) -> Result<(), InjectError> {
        Err(InjectError::Unsupported)
    }
}
