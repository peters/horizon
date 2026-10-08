//! Backend-neutral semantic page-control values and DOM grounding helpers.
//!
//! The public values are transport-independent. The private state and scripts
//! are shared by the CDP and `WebDriver` driver loops so both backends assign the
//! same short-lived element references and enforce the same payload limits.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use horizon_browser_protocol::{
    AgentActionResult, BrowserActionOutcome, BrowserAttachedFile, BrowserBounds, BrowserControlFailure,
    BrowserControlValue, BrowserFileInput, BrowserNode, BrowserSnapshot, BrowserTarget, NavigationOutcome,
    NavigationState, SelectorState, WaitOutcome,
};

const MAX_CONTROL_RESULT_BYTES: usize = 1024 * 1024;
const MAX_NODE_STRING_BYTES: usize = 2 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TeachCapture {
    Click { x: f64, y: f64 },
    Focused,
}

impl TeachCapture {
    #[must_use]
    pub(crate) fn point(self) -> Option<(f64, f64)> {
        match self {
            Self::Click { x, y } => Some((x, y)),
            Self::Focused => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SemanticState {
    generation: u64,
    revision: u64,
    references: HashMap<String, ResolvedTarget>,
    teach_text_gesture: bool,
}

impl Default for SemanticState {
    fn default() -> Self {
        Self {
            generation: 1,
            revision: 0,
            references: HashMap::new(),
            teach_text_gesture: false,
        }
    }
}

impl SemanticState {
    /// Current page generation; it advances whenever a navigation invalidates
    /// earlier references.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.revision = 0;
        self.references.clear();
        self.teach_text_gesture = false;
    }

    pub(crate) fn invalidate_cdp_frame(&mut self, session: &str, context: Option<u64>) {
        self.references.retain(|_, target| {
            !matches!(
                &target.frame,
                Some(FrameTarget::Cdp { session: stored, context: id })
                    if stored == session && context.is_none_or(|context| context == *id)
            )
        });
    }

    pub(crate) fn invalidate_bidi_frame(&mut self, context: &str) {
        self.references.retain(|_, target| {
            !matches!(
                &target.frame, Some(FrameTarget::Bidi { context: stored, .. }) if stored == context
            )
        });
    }

    pub(crate) fn bidi_frame_is_current(&self, frame: &FrameTarget) -> bool {
        self.references.values().any(|target| matches!(
            (&target.frame, frame),
            (Some(FrameTarget::Bidi { context: stored_context, realm: stored_realm }), FrameTarget::Bidi { context, realm })
                if stored_context == context && stored_realm == realm
        ))
    }

    /// `Some(Click)` for a left press, `Some(Focused)` for the start of a
    /// text-edit gesture, `None` when Teach should not evaluate.
    pub(crate) fn teach_capture_point(&mut self, input: &crate::BrowserInput) -> Option<TeachCapture> {
        match input {
            crate::BrowserInput::MousePress {
                x,
                y,
                button: crate::BrowserButton::Left,
                ..
            } => {
                self.teach_text_gesture = false;
                Some(TeachCapture::Click { x: *x, y: *y })
            }
            crate::BrowserInput::KeyDown {
                key: crate::BrowserKey::Tab | crate::BrowserKey::Escape,
                ..
            } => {
                self.teach_text_gesture = false;
                None
            }
            crate::BrowserInput::InsertText { text } if !text.is_empty() => self.start_text_capture(),
            crate::BrowserInput::KeyDown { text: Some(text), .. } if !text.is_empty() => self.start_text_capture(),
            _ => None,
        }
    }

    fn start_text_capture(&mut self) -> Option<TeachCapture> {
        if self.teach_text_gesture {
            None
        } else {
            self.teach_text_gesture = true;
            Some(TeachCapture::Focused)
        }
    }

    /// Parse a scan without registering references: the page generation and
    /// the nodes (with empty references) for judging a condition, leaving the
    /// reference map of the last registered snapshot or query untouched.
    pub(crate) fn peek_nodes(&self, value: &Value) -> Result<PeekedScan, BrowserControlFailure> {
        let response: NodeScanResponse = serde_json::from_value(value.clone())
            .map_err(|error| BrowserControlFailure::new("invalid_result", format!("invalid page snapshot: {error}")))?;
        if let Some(error) = response.error {
            return Err(error);
        }
        let summary = response.summary;
        let nodes = response
            .nodes
            .into_iter()
            .map(|scanned| BrowserNode {
                reference: String::new(),
                role: truncate_utf8(scanned.role, MAX_NODE_STRING_BYTES),
                name: truncate_utf8(scanned.name, MAX_NODE_STRING_BYTES),
                text: truncate_utf8(scanned.text, MAX_NODE_STRING_BYTES),
                visible: scanned.visible,
                enabled: scanned.enabled,
                bounds: scanned.bounds.filter(valid_bounds),
                file_input: scanned.file_input,
            })
            .collect();
        Ok(PeekedScan {
            generation: self.generation,
            nodes,
            summary,
        })
    }

    pub(crate) fn register_nodes(
        &mut self,
        value: Value,
    ) -> Result<(u64, u64, Vec<BrowserNode>), BrowserControlFailure> {
        let response: NodeScanResponse = serde_json::from_value(value)
            .map_err(|error| BrowserControlFailure::new("invalid_result", format!("invalid page snapshot: {error}")))?;
        if let Some(error) = response.error {
            return Err(error);
        }
        self.revision = self.revision.wrapping_add(1).max(1);
        self.references.clear();
        let mut nodes = Vec::with_capacity(response.nodes.len());
        for (index, scanned) in response.nodes.into_iter().enumerate() {
            let reference = format!("g{}s{}e{}", self.generation, self.revision, index + 1);
            self.references.insert(
                reference.clone(),
                ResolvedTarget {
                    selector: scanned.selector,
                    frame: scanned.frame,
                },
            );
            nodes.push(BrowserNode {
                reference,
                role: truncate_utf8(scanned.role, MAX_NODE_STRING_BYTES),
                name: truncate_utf8(scanned.name, MAX_NODE_STRING_BYTES),
                text: truncate_utf8(scanned.text, MAX_NODE_STRING_BYTES),
                visible: scanned.visible,
                enabled: scanned.enabled,
                bounds: scanned.bounds.filter(valid_bounds),
                file_input: scanned.file_input,
            });
        }
        Ok((self.generation, self.revision, nodes))
    }

    pub(crate) fn resolve_target(&self, target: &BrowserTarget) -> Result<ResolvedTarget, BrowserControlFailure> {
        match target {
            BrowserTarget::Selector { selector } => Ok(ResolvedTarget {
                selector: selector.clone(),
                frame: None,
            }),
            BrowserTarget::Ref { reference } => self.references.get(reference).cloned().ok_or_else(|| {
                BrowserControlFailure::new("stale_reference", "element reference is stale; take a new snapshot")
            }),
        }
    }

    pub(crate) fn resolve(&self, target: &BrowserTarget) -> Result<String, BrowserControlFailure> {
        let target = self.resolve_target(target)?;
        if target.frame.is_some() {
            return Err(BrowserControlFailure::new(
                "unsupported_frame_action",
                "this action does not support a child-frame reference",
            ));
        }
        Ok(target.selector)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedTarget {
    pub(crate) selector: String,
    pub(crate) frame: Option<FrameTarget>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub(crate) enum FrameTarget {
    Cdp { session: String, context: u64 },
    Bidi { context: String, realm: String },
}

pub(crate) const MAX_SEMANTIC_FRAMES: usize = 64;

pub(crate) fn scan_node_limit_reached(scan: &Value, max_nodes: u32) -> bool {
    scan["nodes"]
        .as_array()
        .is_some_and(|nodes| nodes.len() >= max_nodes as usize)
}

/// Frame routes are host-owned; ignore any route a page tried to return.
pub(crate) fn clear_scan_frames(scan: &mut Value) -> Result<(), BrowserControlFailure> {
    check_script_error(scan)?;
    let nodes = scan
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| BrowserControlFailure::new("invalid_result", "page scan returned no nodes"))?;
    for node in nodes {
        node.as_object_mut()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "page scan returned an invalid node"))?
            .remove("frame");
    }
    Ok(())
}

pub(crate) fn append_frame_scan(
    scan: &mut Value,
    mut child: Value,
    frame: &FrameTarget,
    max_nodes: u32,
) -> Result<(), BrowserControlFailure> {
    check_script_error(&child)?;
    let nodes = child
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| BrowserControlFailure::new("invalid_result", "frame scan returned no nodes"))?;
    for node in nodes.iter_mut() {
        let node = node
            .as_object_mut()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "frame scan returned an invalid node"))?;
        node.insert(
            "frame".to_owned(),
            serde_json::to_value(frame)
                .map_err(|_| BrowserControlFailure::new("invalid_result", "could not encode frame reference"))?,
        );
        // Frame-local rectangles cannot be used as top-level hit coordinates.
        node.insert("bounds".to_owned(), Value::Null);
        node.remove("fileInput");
    }
    let output = scan
        .get_mut("nodes")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| BrowserControlFailure::new("invalid_result", "page scan returned no nodes"))?;
    let remaining = (max_nodes as usize).saturating_sub(output.len());
    output.extend(nodes.drain(..).take(remaining));
    bounded_control_value(scan.clone())?;
    Ok(())
}

#[derive(Deserialize)]
struct NodeScanResponse {
    #[serde(default)]
    nodes: Vec<ScannedNode>,
    #[serde(default)]
    summary: Option<ScanSummary>,
    #[serde(default)]
    error: Option<BrowserControlFailure>,
}

/// A scan parsed without registering references: the page generation, the
/// returned nodes (with empty references), the selector-wide match summary
/// when the scan had a selector.
#[derive(Debug, Default)]
pub(crate) struct PeekedScan {
    pub(crate) generation: u64,
    pub(crate) nodes: Vec<BrowserNode>,
    pub(crate) summary: Option<ScanSummary>,
}

/// Match and visibility counts over every element a selector scan matched,
/// beyond the capped nodes it returned.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ScanSummary {
    pub(crate) matched: usize,
    pub(crate) visible: usize,
}

#[derive(Deserialize)]
struct ScannedNode {
    selector: String,
    #[serde(default)]
    frame: Option<FrameTarget>,
    #[serde(default)]
    role: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    visible: bool,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    bounds: Option<BrowserBounds>,
    #[serde(default, rename = "fileInput")]
    file_input: Option<crate::BrowserFileInput>,
}

const fn default_true() -> bool {
    true
}

fn valid_bounds(bounds: &BrowserBounds) -> bool {
    bounds.x.is_finite()
        && bounds.y.is_finite()
        && bounds.width.is_finite()
        && bounds.height.is_finite()
        && bounds.width >= 0.0
        && bounds.height >= 0.0
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value.truncate(end);
    value
}

pub(crate) fn bounded_control_value(value: Value) -> Result<Value, BrowserControlFailure> {
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| BrowserControlFailure::new("invalid_result", format!("could not encode result: {error}")))?;
    if encoded.len() > MAX_CONTROL_RESULT_BYTES {
        Err(BrowserControlFailure::new(
            "result_too_large",
            format!("browser result exceeded {MAX_CONTROL_RESULT_BYTES} bytes"),
        ))
    } else {
        Ok(value)
    }
}

/// A snapshot or query scan: it stops at `max_nodes` results.
pub(crate) fn scan_expression(selector: Option<&str>, max_nodes: u32) -> String {
    let selector = selector.map_or_else(|| "null".to_string(), json_string);
    let semantic_only = selector == "null";
    format!("({NODE_SCAN_FUNCTION})({selector}, {max_nodes}, {semantic_only}, false)")
}

/// A wait observation: the scan returns at most `max_nodes` results but
/// keeps counting matches and visible matches over the whole match list, so
/// the condition can be judged beyond the returned nodes. Only waits pay for
/// the full pass; queries keep the early stop.
pub(crate) fn wait_scan_expression(selector: &str, max_nodes: u32) -> String {
    format!(
        "({NODE_SCAN_FUNCTION})({}, {max_nodes}, false, true)",
        json_string(selector)
    )
}

pub(crate) fn target_rect_expression(selector: &str, clear: bool) -> String {
    format!("({TARGET_RECT_FUNCTION})({}, {clear})", json_string(selector))
}

pub(crate) fn frame_fill_expression(selector: &str) -> String {
    target_rect_expression(selector, true)
}

pub(crate) fn scroll_expression(selector: Option<&str>, delta_x: f64, delta_y: f64) -> String {
    let selector = selector.map_or_else(|| "null".to_string(), json_string);
    format!("({SCROLL_FUNCTION})({selector}, {delta_x}, {delta_y})")
}

pub(crate) fn parse_target_rect(value: &Value) -> Result<(f64, f64), BrowserControlFailure> {
    check_script_error(value)?;
    let x = value.get("x").and_then(Value::as_f64);
    let y = value.get("y").and_then(Value::as_f64);
    match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok((x, y)),
        _ => Err(BrowserControlFailure::new(
            "invalid_result",
            "browser returned invalid element coordinates",
        )),
    }
}

pub(crate) fn check_script_error(value: &Value) -> Result<(), BrowserControlFailure> {
    let Some(error) = value.get("error") else {
        return Ok(());
    };
    serde_json::from_value(error.clone())
        .map_err(|_| BrowserControlFailure::new("javascript_error", "page script returned an invalid error"))
        .and_then(Err)
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

const NODE_SCAN_FUNCTION: &str = r"function(selector, maxNodes, semanticOnly, countMatches) {
    const roleFor = (element) => {
        const explicit = element.getAttribute('role');
        if (explicit) return explicit.split(/\s+/)[0];
        const tag = element.tagName.toLowerCase();
        if (tag === 'a' && element.hasAttribute('href')) return 'link';
        if (tag === 'button') return 'button';
        if (tag === 'textarea') return 'textbox';
        if (tag === 'select') return 'combobox';
        if (tag === 'img') return 'img';
        if (tag === 'iframe') return 'iframe';
        if (/^h[1-6]$/.test(tag)) return 'heading';
        if (tag === 'li') return 'listitem';
        if (tag === 'input') {
            const type = (element.getAttribute('type') || 'text').toLowerCase();
            if (type === 'checkbox') return 'checkbox';
            if (type === 'radio') return 'radio';
            if (type === 'button' || type === 'submit' || type === 'reset') return 'button';
            return 'textbox';
        }
        if (element.isContentEditable) return 'textbox';
        return '';
    };
    const compact = (value, limit = 512) => String(value || '').replace(/\s+/g, ' ').trim().slice(0, limit);
    const nameFor = (element) => {
        const direct = element.getAttribute('aria-label');
        if (direct) return compact(direct);
        const labelledBy = element.getAttribute('aria-labelledby');
        if (labelledBy) {
            const labelled = labelledBy.split(/\s+/).map((id) => document.getElementById(id)?.textContent || '').join(' ');
            if (compact(labelled)) return compact(labelled);
        }
        const tag = element.tagName.toLowerCase();
        const type = tag === 'input' ? (element.getAttribute('type') || 'text').toLowerCase() : '';
        const buttonValue = tag === 'input' && (type === 'button' || type === 'submit' || type === 'reset')
            ? element.value : '';
        return compact(element.getAttribute('alt') || element.getAttribute('title') ||
            element.getAttribute('placeholder') || buttonValue || element.textContent);
    };
    const cssPath = (element) => {
        if (element.id) {
            const escaped = CSS.escape(element.id);
            if (document.querySelectorAll(`#${escaped}`).length === 1) return `#${escaped}`;
        }
        const parts = [];
        let current = element;
        while (current && current.nodeType === Node.ELEMENT_NODE && current !== document.documentElement) {
            const tag = current.tagName.toLowerCase();
            let index = 1;
            let sibling = current.previousElementSibling;
            while (sibling) {
                if (sibling.tagName === current.tagName) index += 1;
                sibling = sibling.previousElementSibling;
            }
            parts.unshift(`${tag}:nth-of-type(${index})`);
            current = current.parentElement;
        }
        parts.unshift('html');
        return parts.join(' > ').slice(0, 2048);
    };
    let candidates;
    try {
        candidates = document.querySelectorAll(selector === null ? '*' : selector);
    } catch (error) {
        return {
            nodes: [],
            error: { code: 'invalid_selector', message: compact(error?.message || error) }
        };
    }
    const nodes = [];
    // A wait asks for match and visibility counts over every match so its
    // condition can be judged beyond the returned cap; snapshots and queries
    // stop at the cap and never lay out the rest of the page.
    let matched = 0;
    let visibleMatches = 0;
    for (const element of candidates) {
        if (nodes.length >= maxNodes && !countMatches) break;
        const style = getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        const visible = style.display !== 'none' && style.visibility !== 'hidden' &&
            Number(style.opacity || 1) !== 0 && rect.width > 0 && rect.height > 0;
        if (countMatches) {
            matched += 1;
            if (visible) visibleMatches += 1;
            if (nodes.length >= maxNodes) continue;
        }
        const role = roleFor(element);
        const name = nameFor(element);
        const tag = element.tagName.toLowerCase();
        const leafText = element.children.length === 0 || /^h[1-6]$/.test(tag) || tag === 'p' || tag === 'li';
        const text = leafText ? compact(element.textContent) : '';
        const interactive = Boolean(role) || element.tabIndex >= 0 || element.hasAttribute('onclick');
        if (semanticOnly && (!visible || (!interactive && !text))) continue;
        const fileInput = tag === 'input' && element.type === 'file' ? {
            accept: (element.getAttribute('accept') || '').slice(0, 8192),
            accept_truncated: (element.getAttribute('accept') || '').length > 8192,
            multiple: element.hasAttribute('multiple'),
            files: element.files ? element.files.length : 0,
        } : undefined;
        nodes.push({
            selector: cssPath(element), role, name, text, visible,
            enabled: !element.matches(':disabled') && element.getAttribute('aria-disabled') !== 'true',
            bounds: visible ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height } : null,
            fileInput,
        });
    }
    return countMatches
        ? { nodes, summary: { matched, visible: visibleMatches } }
        : { nodes };
}";

const TARGET_RECT_FUNCTION: &str = r"function(selector, clear) {
    let element;
    try { element = document.querySelector(selector); }
    catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
    if (!element) return { error: { code: 'no_such_element', message: 'no element matched the target' } };
    element.scrollIntoView({ block: 'center', inline: 'center', behavior: 'auto' });
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || rect.width <= 0 || rect.height <= 0)
        return { error: { code: 'element_not_visible', message: 'target element is not visible' } };
    if (element.matches(':disabled') || element.getAttribute('aria-disabled') === 'true')
        return { error: { code: 'element_disabled', message: 'target element is disabled' } };
    if (clear) {
        const isReadOnly = () => element.readOnly || element.getAttribute('aria-readonly') === 'true';
        const notEditable = { error: { code: 'element_not_editable', message: 'target element is not editable' } };
        const notFocused = { error: { code: 'element_not_focused', message: 'target element did not retain focus' } };
        const contentEditable = () => element.isContentEditable
            && !['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON'].includes(element.tagName);
        const textInput = () => element.tagName === 'INPUT'
            && ['text', 'search', 'tel', 'url', 'email', 'password', 'number'].includes((element.getAttribute('type') || 'text').toLowerCase());
        const isEditable = () => !isReadOnly() && (contentEditable() || element.tagName === 'TEXTAREA' || textInput());
        if (!isEditable()) return notEditable;
        element.focus();
        if (!isEditable()) return notEditable;
        if (document.activeElement !== element) return notFocused;
        if (contentEditable()) {
            element.textContent = '';
            element.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
        } else if ('value' in element) {
            const prototype = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
            const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
            if (setter) setter.call(element, ''); else element.value = '';
            element.dispatchEvent(new Event('input', { bubbles: true }));
        } else {
            return notEditable;
        }
        if (!isEditable()) return notEditable;
        if (document.activeElement !== element) return notFocused;
    }
    return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
}";

const SCROLL_FUNCTION: &str = r"function(selector, deltaX, deltaY) {
    let target = window;
    if (selector !== null) {
        try { target = document.querySelector(selector); }
        catch (error) { return { error: { code: 'invalid_selector', message: String(error?.message || error).slice(0, 512) } }; }
        if (!target) return { error: { code: 'no_such_element', message: 'no element matched the target' } };
        target.scrollIntoView({ block: 'center', inline: 'center', behavior: 'auto' });
    }
    target.scrollBy({ left: deltaX, top: deltaY, behavior: 'auto' });
    const root = document.scrollingElement || document.documentElement;
    return { scrollX: target === window ? window.scrollX : target.scrollLeft,
             scrollY: target === window ? window.scrollY : target.scrollTop,
             contentWidth: target === window ? root.scrollWidth : target.scrollWidth,
             contentHeight: target === window ? root.scrollHeight : target.scrollHeight };
}";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_expire_when_the_page_generation_changes() {
        let mut state = SemanticState::default();
        let value = serde_json::json!({
            "nodes": [{
                "selector": "#submit", "role": "button", "name": "Submit", "text": "",
                "visible": true, "enabled": true,
                "bounds": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 }
            }]
        });
        let (_, _, nodes) = state.register_nodes(value).unwrap_or_default();
        let reference = nodes.first().map(|node| node.reference.clone()).unwrap_or_default();

        assert_eq!(
            state.resolve(&BrowserTarget::Ref {
                reference: reference.clone()
            }),
            Ok("#submit".to_string())
        );
        state.invalidate();
        assert_eq!(
            state
                .resolve(&BrowserTarget::Ref { reference })
                .err()
                .map(|error| error.code),
            Some("stale_reference".to_string())
        );
    }

    #[test]
    fn scan_script_quotes_untrusted_selectors_as_data() {
        let expression = scan_expression(Some("button'); throw new Error('owned"), 10);

        assert!(expression.contains("\"button'); throw new Error('owned\""));
        assert!(expression.contains(", 10, false, false)"));
        let wait = wait_scan_expression("button'); throw new Error('owned", 10);
        assert!(wait.contains("\"button'); throw new Error('owned\""));
        assert!(wait.contains(", 10, false, true)"));
    }

    #[test]
    fn scan_script_never_uses_editable_values_as_semantic_names() {
        let expression = scan_expression(None, 10);

        assert!(expression.contains("const buttonValue"));
        assert!(expression.contains("element.getAttribute('placeholder') || buttonValue"));
        assert!(!expression.contains("element.getAttribute('placeholder') || element.value"));
    }

    #[test]
    fn semantic_snapshot_keeps_iframes_discoverable_in_the_original_panel() {
        let expression = scan_expression(None, 10);

        assert!(expression.contains("if (tag === 'iframe') return 'iframe'"));
    }

    #[test]
    fn page_owned_identity_is_not_an_authoritative_scan_value() {
        let state = SemanticState::default();
        let peeked = state
            .peek_nodes(&serde_json::json!({
                "nodes": [], "documentIdentity": "copied-page-token"
            }))
            .unwrap_or_default();
        assert!(peeked.nodes.is_empty());
        assert!(!scan_expression(None, 10).contains("documentIdentity"));
        assert!(!wait_scan_expression("button", 10).contains("Symbol"));
    }

    #[test]
    fn only_wait_scans_count_matches_past_the_cap() {
        assert!(scan_expression(Some("button"), 10).contains("(\"button\", 10, false, false)"));
        assert!(scan_expression(None, 10).contains("(null, 10, true, false)"));
        assert!(wait_scan_expression("button", 20).contains("(\"button\", 20, false, true)"));
        assert!(NODE_SCAN_FUNCTION.contains("if (nodes.length >= maxNodes && !countMatches) break;"));
    }

    #[test]
    fn teach_inactive_does_not_retain_or_request_fingerprints() {
        let slot = crate::FrameSlot::new();
        assert!(!slot.teach_recording());
        assert!(slot.take_teach_observation().is_none());
        slot.set_teach_recording(true);
        assert!(slot.teach_recording());
        slot.set_teach_recording(false);
        assert!(!slot.teach_recording());
        assert!(!scan_expression(None, 8).contains("elementFromPoint"));
        assert!(!wait_scan_expression("#status", 8).contains("activeElement"));
    }

    #[test]
    fn teach_text_gesture_captures_once_until_the_next_click() {
        let mut state = SemanticState::default();
        let press = crate::BrowserInput::MousePress {
            x: 12.0,
            y: 40.0,
            button: crate::BrowserButton::Left,
            click_count: 1,
            buttons: 1,
            modifiers: crate::BrowserModifiers::none(),
        };
        assert_eq!(
            state.teach_capture_point(&press),
            Some(TeachCapture::Click { x: 12.0, y: 40.0 })
        );
        let insert = crate::BrowserInput::InsertText {
            text: "month".to_string(),
        };
        assert_eq!(state.teach_capture_point(&insert), Some(TeachCapture::Focused));
        assert_eq!(state.teach_capture_point(&insert), None);
        assert_eq!(
            state.teach_capture_point(&press),
            Some(TeachCapture::Click { x: 12.0, y: 40.0 })
        );
        assert_eq!(state.teach_capture_point(&insert), Some(TeachCapture::Focused));
        let tab = crate::BrowserInput::KeyDown {
            physical_key: None,
            key: crate::BrowserKey::Tab,
            text: None,
            modifiers: crate::BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        };
        assert_eq!(state.teach_capture_point(&tab), None);
        assert_eq!(state.teach_capture_point(&insert), Some(TeachCapture::Focused));
    }

    #[test]
    fn page_errors_become_typed_failures() {
        let value = serde_json::json!({ "error": { "code": "no_such_element", "message": "missing" } });

        assert_eq!(
            check_script_error(&value),
            Err(BrowserControlFailure::new("no_such_element", "missing"))
        );
    }
    #[test]
    fn child_routes_are_host_owned_bounded_and_expire_independently() {
        let frame = FrameTarget::Cdp {
            session: "page".into(),
            context: 7,
        };
        let mut scan = serde_json::json!({"nodes":[{"selector":"#top","fileInput":{"multiple":true,"accept":"","files":0},"frame":{"backend":"bidi","context":"foreign","realm":"foreign"}}]});
        clear_scan_frames(&mut scan).unwrap();
        assert!(scan["nodes"][0].get("frame").is_none());
        let child = serde_json::json!({"nodes":[{"selector":"#same","fileInput":{"multiple":true,"accept":"","files":0},"bounds":{"x":1,"y":2,"width":3,"height":4}},{"selector":"#extra"}]});
        append_frame_scan(&mut scan, child, &frame, 2).unwrap();
        assert_eq!(scan["nodes"].as_array().unwrap().len(), 2);
        assert!(scan["nodes"][1]["bounds"].is_null());
        let mut state = SemanticState::default();
        let (_, _, nodes) = state.register_nodes(scan).unwrap();
        assert!(nodes[0].file_input.is_some());
        assert!(nodes[1].file_input.is_none());
        let top = BrowserTarget::Ref {
            reference: nodes[0].reference.clone(),
        };
        let child = BrowserTarget::Ref {
            reference: nodes[1].reference.clone(),
        };
        assert_eq!(state.resolve(&child).unwrap_err().code, "unsupported_frame_action");
        assert!(matches!(
            state.resolve_target(&child).unwrap().frame,
            Some(FrameTarget::Cdp { context: 7, .. })
        ));
        let generation = state.generation();
        state.invalidate_cdp_frame("page", Some(7));
        assert_eq!(state.resolve_target(&child).unwrap_err().code, "stale_reference");
        assert_eq!(state.resolve(&top).unwrap(), "#top");
        assert_eq!(state.generation(), generation);
    }

    #[test]
    fn malformed_child_nodes_fail_without_panicking() {
        let mut scan = serde_json::json!({"nodes":[]});
        let frame = FrameTarget::Cdp {
            session: "page".into(),
            context: 7,
        };
        let child = serde_json::json!({"nodes":[null]});
        assert_eq!(
            append_frame_scan(&mut scan, child, &frame, 1).unwrap_err().code,
            "invalid_result"
        );
    }
    #[test]
    fn bidi_navigation_invalidates_only_that_child_document() {
        let frame = FrameTarget::Bidi {
            context: "child".into(),
            realm: "realm".into(),
        };
        let mut scan = serde_json::json!({"nodes":[{"selector":"#top"}]});
        append_frame_scan(
            &mut scan,
            serde_json::json!({"nodes":[{"selector":"#field"}]}),
            &frame,
            2,
        )
        .unwrap();
        let mut state = SemanticState::default();
        let (_, _, nodes) = state.register_nodes(scan).unwrap();
        assert!(state.bidi_frame_is_current(&frame));
        state.invalidate_bidi_frame("foreign");
        assert!(state.bidi_frame_is_current(&frame));
        state.invalidate_bidi_frame("child");
        assert!(!state.bidi_frame_is_current(&frame));
        assert_eq!(
            state
                .resolve(&BrowserTarget::Ref {
                    reference: nodes[0].reference.clone()
                })
                .unwrap(),
            "#top"
        );
        assert_eq!(
            state
                .resolve_target(&BrowserTarget::Ref {
                    reference: nodes[1].reference.clone()
                })
                .unwrap_err()
                .code,
            "stale_reference"
        );
    }
}
