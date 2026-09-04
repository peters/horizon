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
/// This path never reads or writes the clipboard. Direct insertion refuses
/// ambiguous, protected, stale, hidden, read-only, or selected-text targets
/// so failure cannot partially replace existing text. On Linux, if no AT-SPI
/// editable field exists, keys are typed into the focused window. A unique
/// focused object that is a password field, or that exposes a visible
/// selection, is still refused when accessibility identifies it. Chromium,
/// Electron, and Microsoft Teams typically have no AT-SPI tree; desktop
/// dictation types into those windows anyway. Synthesis aborts if the OS
/// input-focus window changes during the accessibility preflight.
///
/// # Errors
///
/// Returns [`InjectError::Unsupported`] when no Linux AT-SPI session is
/// available, [`InjectError::Target`] when the focused object is unsafe or
/// unsupported, and [`InjectError::Failed`] for accessibility transport
/// failures.
pub fn insert_text_into_focused_accessible(text: &str) -> Result<(), InjectError> {
    insert_text_into_focused_accessible_for_window(text, None)
}

/// Insert into the focused accessibility target, requiring `expected_window`
/// to still be the X11 input-focus window when the Linux path starts.
///
/// # Errors
///
/// Same as [`insert_text_into_focused_accessible`].
pub fn insert_text_into_focused_accessible_for_window(
    text: &str,
    expected_window: Option<u32>,
) -> Result<(), InjectError> {
    platform::insert_text(text, expected_window)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::borrow::Cow;
    use std::collections::{HashSet, VecDeque};
    use std::future::Future;
    use std::sync::{Mutex, TryLockError};
    use std::time::Duration;

    use atspi::proxy::accessible::{AccessibleProxy, ObjectRefExt as _};
    use atspi::proxy::device_event_controller::{DeviceEventControllerProxy, KeySynthType};
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

        fn has_text_interfaces(self) -> bool {
            self.interfaces.contains(Interface::Text | Interface::EditableText)
        }

        fn is_focused_text_target(self) -> bool {
            self.states.contains(State::Focused) && self.has_text_interfaces()
        }

        fn is_focused(self) -> bool {
            self.states.contains(State::Focused)
        }

        fn validate_synthesis_target(self) -> Result<(), InjectError> {
            if self.role == Role::PasswordText {
                return Err(InjectError::Target("dictation into password fields is disabled"));
            }
            if self.states.intersects(State::Defunct | State::Stale) {
                return Err(InjectError::Target("focused accessibility target is stale"));
            }
            if !self.states.contains(State::Focused) {
                return Err(InjectError::Target("focused accessibility target changed"));
            }
            if !self.states.contains(State::Enabled | State::Sensitive) {
                return Err(InjectError::Target("focused field is not available for input"));
            }
            if !self.states.contains(State::Visible | State::Showing) {
                return Err(InjectError::Target("focused field is not visible"));
            }
            // Chromium Frame/DocumentWeb often report ReadOnly without being
            // an editable field. A read-only Entry/text object must still
            // refuse KEY_STRING even when it exposes only Interface::Text.
            if self.states.contains(State::ReadOnly) && !matches!(self.role, Role::Frame | Role::DocumentWeb) {
                return Err(InjectError::Target("focused field is not editable"));
            }
            Ok(())
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
            if !self.has_text_interfaces() {
                return Err(InjectError::Target(
                    "focused field does not support direct text insertion",
                ));
            }
            if self.states.contains(State::ReadOnly) {
                return Err(InjectError::Target("focused field is not editable"));
            }
            if !self.states.contains(State::Editable) {
                // Firefox documents expose EditableText without Editable.
                // Keep them classified, then fall back to KEY_STRING.
                return Err(InjectError::Target(
                    "focused application does not expose an editable text field",
                ));
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

    pub(super) fn insert_text(text: &str, expected_window: Option<u32>) -> Result<(), InjectError> {
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
        runtime.block_on(insert_text_async(text, expected_window))
    }

    async fn insert_text_async(text: &str, expected_window: Option<u32>) -> Result<(), InjectError> {
        let text = sanitize_desktop_transcript(text);
        let observed = crate::os_focus::current_input_focus_window();
        let focus_window = match expected_window {
            Some(expected) => {
                focus_window_still_matches(Some(expected), observed)?;
                Some(expected)
            }
            None => observed,
        };
        let deadline = tokio::time::Instant::now() + PREFLIGHT_TIMEOUT;
        let connection = bounded_preflight(deadline, async {
            AccessibilityConnection::new()
                .await
                .map_err(|_| InjectError::Unsupported)
        })
        .await?;
        let focus_pids = focus_window
            .and_then(crate::os_focus::window_candidate_pids)
            .unwrap_or_default();
        match insert_into_editable_field(&connection, deadline, &text, &focus_pids).await {
            Ok(()) => Ok(()),
            Err(error) if can_synthesize_keys(&error) => {
                synthesize_into_focused_target(&connection, deadline, &text, focus_window, &focus_pids).await
            }
            Err(error) => Err(error),
        }
    }

    async fn insert_into_editable_field(
        connection: &AccessibilityConnection,
        deadline: tokio::time::Instant,
        text: &str,
        focus_pids: &[u32],
    ) -> Result<(), InjectError> {
        let target = bounded_preflight(deadline, find_focused_target(connection, focus_pids)).await?;
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

    async fn find_focused_target(
        connection: &AccessibilityConnection,
        focus_pids: &[u32],
    ) -> Result<ObjectRefOwned, InjectError> {
        find_unique_focus(
            connection,
            FocusSearch::EditableText,
            unique_candidate_index,
            focus_pids,
        )
        .await
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FocusSearch {
        EditableText,
        FocusedOnly,
    }

    fn collection_match_rule(search: FocusSearch) -> ObjectMatchRule {
        let mut rule = ObjectMatchRule::builder().states([State::Focused], MatchType::All);
        if search == FocusSearch::EditableText {
            rule = rule.interfaces([Interface::Text, Interface::EditableText], MatchType::All);
        }
        let mut rule = rule.build();
        // atspi-rs defaults unused match types to Invalid. GTK's Collection
        // treats Invalid as "match nothing", so a Focused-only rule returns
        // empty and KEY_STRING replaces a classified selection.
        rule.attr_mt = MatchType::All;
        rule.roles_mt = MatchType::All;
        if search != FocusSearch::EditableText {
            rule.ifaces_mt = MatchType::All;
        }
        rule
    }

    async fn collection_candidates(root: &AccessibleProxy<'_>, search: FocusSearch) -> Option<Vec<ObjectRefOwned>> {
        let proxies = root.proxies().await.ok()?;
        let collection = proxies.collection().await.ok()?;
        collection
            .get_matches(collection_match_rule(search), SortOrder::Canonical, 2, true)
            .await
            .ok()
    }

    async fn traversal_candidates(
        root: ObjectRefOwned,
        connection: &atspi::zbus::Connection,
        search: FocusSearch,
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
            let facts = TargetFacts::new(interfaces, states, Role::Invalid);
            let matched = match search {
                FocusSearch::EditableText => facts.is_focused_text_target(),
                FocusSearch::FocusedOnly => facts.is_focused(),
            };
            if matched {
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

    async fn resolve_focus_candidates<T, Fut>(
        collection: Option<Vec<T>>,
        traversal: impl FnOnce() -> Fut,
    ) -> Option<Vec<T>>
    where
        Fut: Future<Output = Option<Vec<T>>>,
    {
        match collection {
            Some(candidates) => Some(candidates),
            None => traversal().await,
        }
    }

    fn include_atspi_app(app_pid: Option<u32>, focus_pids: &[u32]) -> bool {
        match app_pid {
            Some(app) if !focus_pids.is_empty() => focus_pids.contains(&app),
            _ => true,
        }
    }

    async fn atspi_application_pid(
        dbus: &atspi::zbus::fdo::DBusProxy<'_>,
        application: &ObjectRefOwned,
    ) -> Option<u32> {
        let name = application.name()?;
        let bus_name = atspi::zbus::names::BusName::try_from(name.as_str()).ok()?;
        dbus.get_connection_unix_process_id(bus_name).await.ok()
    }

    fn unique_index(candidate_count: usize, empty: &'static str, multiple: &'static str) -> Result<usize, InjectError> {
        if candidate_count == 0 {
            return Err(InjectError::Target(empty));
        }
        if candidate_count > 1 {
            return Err(InjectError::Target(multiple));
        }
        Ok(0)
    }

    fn unique_candidate_index(candidate_count: usize) -> Result<usize, InjectError> {
        unique_index(
            candidate_count,
            "focused application does not expose an editable text field",
            "multiple focused editable fields were reported",
        )
    }

    fn unique_focused_index(candidate_count: usize) -> Result<usize, InjectError> {
        unique_index(
            candidate_count,
            "focused application does not expose a focused accessibility object",
            "multiple focused fields were reported",
        )
    }

    fn sanitize_desktop_transcript(text: &str) -> Cow<'_, str> {
        if text.chars().any(char::is_control) {
            Cow::Owned(text.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect())
        } else {
            Cow::Borrowed(text)
        }
    }

    fn can_synthesize_keys(error: &InjectError) -> bool {
        matches!(
            error,
            InjectError::Target("focused application does not expose an editable text field")
        )
    }

    fn is_unclassified_focus(error: &InjectError) -> bool {
        matches!(
            error,
            InjectError::Target("focused application does not expose a focused accessibility object")
        )
    }

    fn focus_window_still_matches(captured: Option<u32>, current: Option<u32>) -> Result<(), InjectError> {
        match (captured, current) {
            (Some(expected), Some(now)) if expected == now => Ok(()),
            _ => Err(InjectError::Target(
                "focused window changed before transcript insertion",
            )),
        }
    }

    fn ensure_focus_window_unchanged(captured: Option<u32>) -> Result<(), InjectError> {
        focus_window_still_matches(captured, crate::os_focus::current_input_focus_window())
    }

    async fn find_unique_focus(
        connection: &AccessibilityConnection,
        search: FocusSearch,
        unique_index: fn(usize) -> Result<usize, InjectError>,
        focus_pids: &[u32],
    ) -> Result<ObjectRefOwned, InjectError> {
        let registry = connection
            .root_accessible_on_registry()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect the accessibility registry"))?;
        let applications = registry
            .get_children()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect accessible applications"))?;
        let dbus = atspi::zbus::fdo::DBusProxy::new(connection.connection()).await.ok();
        let mut candidates = Vec::new();
        for application in applications {
            if application.is_null() {
                continue;
            }
            let app_pid = match dbus.as_ref() {
                Some(proxy) => atspi_application_pid(proxy, &application).await,
                None => None,
            };
            if !include_atspi_app(app_pid, focus_pids) {
                continue;
            }
            let Ok(root) = application.as_accessible_proxy(connection.connection()).await else {
                continue;
            };
            // Collection GetMatches is authoritative, including no hits.
            // Treating Some([]) as "unknown" walks GNOME Shell (~4s to the
            // 2048-object cap) and the 5s preflight deadline expires before
            // Chromium/Teams KEY_STRING.
            let Some(discovered) = resolve_focus_candidates(collection_candidates(&root, search).await, || {
                traversal_candidates(application, connection.connection(), search)
            })
            .await
            else {
                continue;
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
        let index = unique_index(candidates.len())?;
        Ok(candidates.swap_remove(index))
    }

    async fn find_focused_object(
        connection: &AccessibilityConnection,
        focus_pids: &[u32],
    ) -> Result<ObjectRefOwned, InjectError> {
        find_unique_focus(connection, FocusSearch::FocusedOnly, unique_focused_index, focus_pids).await
    }

    async fn synthesize_into_focused_target(
        connection: &AccessibilityConnection,
        deadline: tokio::time::Instant,
        text: &str,
        focus_window: Option<u32>,
        focus_pids: &[u32],
    ) -> Result<(), InjectError> {
        ensure_synthesis_target_still_safe(connection, deadline, focus_pids).await?;
        synthesize_key_string(connection, deadline, text, focus_window, focus_pids).await
    }

    async fn synthesize_key_string(
        connection: &AccessibilityConnection,
        deadline: tokio::time::Instant,
        text: &str,
        focus_window: Option<u32>,
        focus_pids: &[u32],
    ) -> Result<(), InjectError> {
        let proxy = bounded_preflight(deadline, async {
            DeviceEventControllerProxy::new(connection.connection())
                .await
                .map_err(|_| InjectError::Failed("failed to reach the accessibility key controller"))
        })
        .await?;
        ensure_focus_window_unchanged(focus_window)?;
        // Tab/click can move focus to a password or selection in the same
        // window while the device proxy is created. Re-classify immediately
        // before the mutating call; unclassified Chromium/Teams still type.
        ensure_synthesis_target_still_safe(connection, deadline, focus_pids).await?;
        ensure_focus_window_unchanged(focus_window)?;
        proxy
            .generate_keyboard_event(0, text, KeySynthType::String)
            .await
            .map_err(|_| InjectError::Failed("focused application rejected synthesized key input"))?;
        Ok(())
    }

    async fn ensure_synthesis_target_still_safe(
        connection: &AccessibilityConnection,
        deadline: tokio::time::Instant,
        focus_pids: &[u32],
    ) -> Result<(), InjectError> {
        match bounded_preflight(deadline, find_focused_object(connection, focus_pids)).await {
            Ok(target) => validate_classified_synthesis_target(connection, deadline, target).await,
            // No AT-SPI focused object (Chromium/Teams). Type into OS focus.
            Err(error) if is_unclassified_focus(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn validate_classified_synthesis_target(
        connection: &AccessibilityConnection,
        deadline: tokio::time::Instant,
        target: ObjectRefOwned,
    ) -> Result<(), InjectError> {
        let accessible = bounded_preflight(deadline, async {
            target
                .as_accessible_proxy(connection.connection())
                .await
                .map_err(|_| InjectError::Failed("focused accessibility target disappeared"))
        })
        .await?;
        let facts = bounded_preflight(deadline, read_target_facts(&accessible)).await?;
        facts.validate_synthesis_target()?;
        if facts.interfaces.contains(Interface::Text) {
            let selected = async {
                let proxies = accessible
                    .proxies()
                    .await
                    .map_err(|_| InjectError::Failed("failed to inspect focused field interfaces"))?;
                let text_proxy = proxies
                    .text()
                    .await
                    .map_err(|_| InjectError::Target("focused field does not expose readable text state"))?;
                let selection_count = read_visible_selection_count(&text_proxy).await?;
                Ok(selection_count != 0)
            };
            if let Ok(true) = bounded_preflight(deadline, selected).await {
                return Err(InjectError::Target(
                    "selected text cannot be replaced safely by direct dictation",
                ));
            }
        }
        Ok(())
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

    fn visible_selection_count(reported: i32, first_selection: Option<(i32, i32)>) -> i32 {
        if reported != 0 {
            reported
        } else {
            i32::from(first_selection.is_some_and(|(start, end)| start != end))
        }
    }

    async fn read_visible_selection_count(text: &TextProxy<'_>) -> Result<i32, InjectError> {
        let reported = text
            .get_n_selections()
            .await
            .map_err(|_| InjectError::Failed("failed to inspect focused field selection"))?;
        if reported != 0 {
            return Ok(reported);
        }
        // GTK can report a caret-range selection via GetSelection(0) while
        // GetNSelections is 0. Treat a non-empty range as selected so dictation
        // cannot replace highlighted text.
        Ok(visible_selection_count(reported, text.get_selection(0).await.ok()))
    }

    async fn read_text_snapshot(text: &TextProxy<'_>) -> Result<TextSnapshot, InjectError> {
        let selection_count = read_visible_selection_count(text).await?;
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
        use std::future::Future;
        use std::sync::PoisonError;

        use atspi::{Interface, InterfaceSet, MatchType, Role, State, StateSet};

        use super::{
            FocusSearch, INSERT_LOCK, InjectError, TargetFacts, TextSnapshot, can_synthesize_keys,
            collection_match_rule, focus_window_still_matches, include_atspi_app, is_unclassified_focus,
            resolve_focus_candidates, sanitize_desktop_transcript, unique_candidate_index, unique_focused_index,
            visible_selection_count,
        };

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

        fn block_on_test<T>(future: impl Future<Output = T>) -> T {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap_or_else(|error| panic!("{error}"))
                .block_on(future)
        }

        #[test]
        fn empty_collection_result_does_not_walk_the_tree() {
            let mut walked = false;
            let empty = block_on_test(resolve_focus_candidates(Some(Vec::<i32>::new()), || {
                walked = true;
                async { Some(vec![1, 2]) }
            }));
            assert_eq!(empty, Some(vec![]));
            assert!(!walked);

            let hit = block_on_test(resolve_focus_candidates(Some(vec![7]), || {
                walked = true;
                async { Some(vec![1]) }
            }));
            assert_eq!(hit, Some(vec![7]));
            assert!(!walked);

            let from_walk = block_on_test(resolve_focus_candidates(None, || {
                walked = true;
                async { Some(vec![9]) }
            }));
            assert_eq!(from_walk, Some(vec![9]));
            assert!(walked);

            walked = false;
            let missing = block_on_test(resolve_focus_candidates::<i32, _>(None, || {
                walked = true;
                async { None }
            }));
            assert_eq!(missing, None);
            assert!(walked);
        }

        #[test]
        fn collection_rules_do_not_leave_unused_match_types_invalid() {
            let editable = collection_match_rule(FocusSearch::EditableText);
            assert_eq!(editable.states_mt, MatchType::All);
            assert_eq!(editable.ifaces_mt, MatchType::All);
            assert_eq!(editable.roles_mt, MatchType::All);
            assert_eq!(editable.attr_mt, MatchType::All);
            let focused = collection_match_rule(FocusSearch::FocusedOnly);
            assert_eq!(focused.states_mt, MatchType::All);
            assert_eq!(focused.ifaces_mt, MatchType::All);
            assert_eq!(focused.roles_mt, MatchType::All);
            assert_eq!(focused.attr_mt, MatchType::All);
        }

        #[test]
        fn leftover_focused_apps_do_not_claim_the_os_focus_target() {
            const TEAMS: u32 = 54_087;
            const FIREFOX: u32 = 14_474;
            const GNOME_SHELL: u32 = 6_749;
            const FRAME: u32 = 6_766;
            assert!(include_atspi_app(Some(TEAMS), &[TEAMS]));
            assert!(include_atspi_app(Some(TEAMS), &[FRAME, TEAMS]));
            assert!(!include_atspi_app(Some(FIREFOX), &[TEAMS]));
            assert!(!include_atspi_app(Some(GNOME_SHELL), &[TEAMS]));
            assert!(include_atspi_app(None, &[TEAMS]));
            assert!(include_atspi_app(Some(FIREFOX), &[]));
            assert!(include_atspi_app(None, &[]));
        }

        #[test]
        fn focused_text_targets_include_read_only_and_non_editable_text() {
            assert!(safe_target().is_focused_text_target());
            let mut document = safe_target();
            document.role = Role::DocumentWeb;
            document.states.remove(State::Editable);
            assert!(document.is_focused_text_target());
            let mut read_only = safe_target();
            read_only.states.insert(State::ReadOnly);
            assert!(read_only.is_focused_text_target());
            let mut unfocused = safe_target();
            unfocused.states.remove(State::Focused);
            assert!(!unfocused.is_focused_text_target());
            let missing_interface = TargetFacts {
                interfaces: InterfaceSet::new(Interface::Text),
                ..safe_target()
            };
            assert!(!missing_interface.is_focused_text_target());
        }

        #[test]
        fn control_characters_are_replaced_before_desktop_insertion() {
            assert_eq!(sanitize_desktop_transcript("hello"), "hello");
            assert_eq!(sanitize_desktop_transcript("hello\nworld\r"), "hello world ");
        }

        #[test]
        fn key_synthesis_is_only_used_when_no_editable_field_exists() {
            assert!(can_synthesize_keys(&InjectError::Target(
                "focused application does not expose an editable text field"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "focused field does not support direct text insertion"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "focused field does not expose readable text state"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "dictation into password fields is disabled"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "selected text cannot be replaced safely by direct dictation"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "multiple focused editable fields were reported"
            )));
            assert!(!can_synthesize_keys(&InjectError::Target(
                "focused field is not editable"
            )));
        }

        #[test]
        fn key_synthesis_refuses_password_fields_not_unclassified_focus() {
            let mut terminal = safe_target();
            terminal.role = Role::Terminal;
            terminal.interfaces = InterfaceSet::new(Interface::Text);
            assert_eq!(terminal.validate_synthesis_target(), Ok(()));

            let password = TargetFacts {
                role: Role::PasswordText,
                ..terminal
            };
            assert_eq!(
                password.validate_synthesis_target(),
                Err(InjectError::Target("dictation into password fields is disabled"))
            );

            let frame = TargetFacts {
                role: Role::Frame,
                ..terminal
            };
            assert_eq!(frame.validate_synthesis_target(), Ok(()));

            let document = TargetFacts {
                role: Role::DocumentWeb,
                ..terminal
            };
            assert_eq!(document.validate_synthesis_target(), Ok(()));

            let mut read_only_frame = frame;
            read_only_frame.states.insert(State::ReadOnly);
            assert_eq!(read_only_frame.validate_synthesis_target(), Ok(()));

            let mut read_only_document = document;
            read_only_document.states.insert(State::ReadOnly);
            assert_eq!(read_only_document.validate_synthesis_target(), Ok(()));

            let mut read_only_text_only_entry = terminal;
            read_only_text_only_entry.role = Role::Entry;
            read_only_text_only_entry.states.insert(State::ReadOnly);
            assert_eq!(
                read_only_text_only_entry.validate_synthesis_target(),
                Err(InjectError::Target("focused field is not editable"))
            );

            let mut read_only_text = safe_target();
            read_only_text.states.insert(State::ReadOnly);
            assert_eq!(
                read_only_text.validate_synthesis_target(),
                Err(InjectError::Target("focused field is not editable"))
            );

            let mut firefox_document = safe_target();
            firefox_document.role = Role::DocumentWeb;
            firefox_document.states.remove(State::Editable);
            assert_eq!(firefox_document.validate_synthesis_target(), Ok(()));

            let mut hidden = frame;
            hidden.states.remove(State::Showing);
            assert_eq!(
                hidden.validate_synthesis_target(),
                Err(InjectError::Target("focused field is not visible"))
            );
            let mut disabled = frame;
            disabled.states.remove(State::Enabled);
            assert_eq!(
                disabled.validate_synthesis_target(),
                Err(InjectError::Target("focused field is not available for input"))
            );
        }

        #[test]
        fn synthesis_aborts_when_the_os_focus_window_changes() {
            assert_eq!(focus_window_still_matches(Some(0x3c0_0004), Some(0x3c0_0004)), Ok(()));
            assert_eq!(
                focus_window_still_matches(None, Some(0x3c0_0004)),
                Err(InjectError::Target(
                    "focused window changed before transcript insertion"
                ))
            );
            assert_eq!(
                focus_window_still_matches(Some(0x3c0_0004), Some(0x3c0_0005)),
                Err(InjectError::Target(
                    "focused window changed before transcript insertion"
                ))
            );
            assert_eq!(
                focus_window_still_matches(Some(0x3c0_0004), None),
                Err(InjectError::Target(
                    "focused window changed before transcript insertion"
                ))
            );
        }

        #[test]
        fn unclassified_focus_is_typed_into_the_focused_window() {
            assert!(is_unclassified_focus(&InjectError::Target(
                "focused application does not expose a focused accessibility object"
            )));
            assert!(!is_unclassified_focus(&InjectError::Target(
                "multiple focused fields were reported"
            )));
            assert!(!is_unclassified_focus(&InjectError::Target(
                "dictation into password fields is disabled"
            )));
            assert_eq!(unique_focused_index(1), Ok(0));
            assert_eq!(
                unique_focused_index(0),
                Err(InjectError::Target(
                    "focused application does not expose a focused accessibility object"
                ))
            );
            assert_eq!(
                unique_focused_index(2),
                Err(InjectError::Target("multiple focused fields were reported"))
            );
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
            assert_eq!(
                not_editable.validate(),
                Err(InjectError::Target(
                    "focused application does not expose an editable text field"
                ))
            );
            assert!(can_synthesize_keys(&not_editable.validate().unwrap_err()));
            assert_eq!(
                read_only.validate(),
                Err(InjectError::Target("focused field is not editable"))
            );
            assert!(!can_synthesize_keys(&read_only.validate().unwrap_err()));
        }

        #[test]
        fn empty_text_is_a_no_op_without_an_accessibility_session() {
            assert_eq!(super::insert_text("", None), Ok(()));
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
            assert_eq!(visible_selection_count(1, None), 1);
            assert_eq!(visible_selection_count(0, Some((0, 7))), 1);
            assert_eq!(visible_selection_count(0, Some((7, 0))), 1);
            assert_eq!(visible_selection_count(0, Some((0, 0))), 0);
            assert_eq!(visible_selection_count(0, None), 0);
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
                super::insert_text("hello", None),
                Err(InjectError::Target("another desktop insertion is still pending"))
            );
        }

        #[test]
        #[ignore = "live AT-SPI editable-field smoke"]
        fn live_accessibility_insert() {
            super::insert_text("direct blå final marker zephyr", None).expect("accessibility insertion");
        }

        #[test]
        #[ignore = "live AT-SPI unsafe-target smoke"]
        fn live_accessibility_rejects_unsafe_target() {
            assert!(matches!(
                super::insert_text("must not appear", None),
                Err(InjectError::Target(_))
            ));
        }

        #[test]
        #[ignore = "live unavailable-AT-SPI smoke"]
        fn live_unavailable_accessibility_bus_is_unsupported() {
            assert_eq!(
                super::insert_text("must not appear", None),
                Err(InjectError::Unsupported)
            );
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

    pub(super) fn insert_text(_text: &str, _expected_window: Option<u32>) -> Result<(), InjectError> {
        Err(InjectError::Unsupported)
    }
}
