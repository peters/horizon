//! Backend-neutral semantic page-control values and DOM grounding helpers.
//!
//! The public values are transport-independent. The private state and scripts
//! are shared by the CDP and `WebDriver` driver loops so both backends assign the
//! same short-lived element references and enforce the same payload limits.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

pub use horizon_browser_protocol::{
    AgentActionResult, BrowserActionOutcome, BrowserBounds, BrowserControlFailure, BrowserControlValue, BrowserNode,
    BrowserSnapshot, BrowserTarget,
};

const MAX_CONTROL_RESULT_BYTES: usize = 1024 * 1024;
const MAX_NODE_STRING_BYTES: usize = 2 * 1024;

#[derive(Debug)]
pub(crate) struct SemanticState {
    generation: u64,
    revision: u64,
    references: HashMap<String, String>,
}

impl Default for SemanticState {
    fn default() -> Self {
        Self {
            generation: 1,
            revision: 0,
            references: HashMap::new(),
        }
    }
}

impl SemanticState {
    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.revision = 0;
        self.references.clear();
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
            self.references.insert(reference.clone(), scanned.selector);
            nodes.push(BrowserNode {
                reference,
                role: truncate_utf8(scanned.role, MAX_NODE_STRING_BYTES),
                name: truncate_utf8(scanned.name, MAX_NODE_STRING_BYTES),
                text: truncate_utf8(scanned.text, MAX_NODE_STRING_BYTES),
                visible: scanned.visible,
                enabled: scanned.enabled,
                bounds: scanned.bounds.filter(valid_bounds),
            });
        }
        Ok((self.generation, self.revision, nodes))
    }

    pub(crate) fn resolve(&self, target: &BrowserTarget) -> Result<String, BrowserControlFailure> {
        match target {
            BrowserTarget::Selector { selector } => Ok(selector.clone()),
            BrowserTarget::Ref { reference } => self.references.get(reference).cloned().ok_or_else(|| {
                BrowserControlFailure::new(
                    "stale_reference",
                    format!("element reference {reference} is stale; take a new snapshot"),
                )
            }),
        }
    }
}

#[derive(Deserialize)]
struct NodeScanResponse {
    #[serde(default)]
    nodes: Vec<ScannedNode>,
    #[serde(default)]
    error: Option<BrowserControlFailure>,
}

#[derive(Deserialize)]
struct ScannedNode {
    selector: String,
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

pub(crate) fn scan_expression(selector: Option<&str>, max_nodes: u32) -> String {
    let selector = selector.map_or_else(|| "null".to_string(), json_string);
    let semantic_only = selector == "null";
    format!("({NODE_SCAN_FUNCTION})({selector}, {max_nodes}, {semantic_only})")
}

pub(crate) fn target_rect_expression(selector: &str, clear: bool) -> String {
    format!("({TARGET_RECT_FUNCTION})({}, {clear})", json_string(selector))
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

const NODE_SCAN_FUNCTION: &str = r"function(selector, maxNodes, semanticOnly) {
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
        return { nodes: [], error: { code: 'invalid_selector', message: compact(error?.message || error) } };
    }
    const nodes = [];
    for (const element of candidates) {
        if (nodes.length >= maxNodes) break;
        const style = getComputedStyle(element);
        const rect = element.getBoundingClientRect();
        const visible = style.display !== 'none' && style.visibility !== 'hidden' &&
            Number(style.opacity || 1) !== 0 && rect.width > 0 && rect.height > 0;
        const role = roleFor(element);
        const name = nameFor(element);
        const tag = element.tagName.toLowerCase();
        const leafText = element.children.length === 0 || /^h[1-6]$/.test(tag) || tag === 'p' || tag === 'li';
        const text = leafText ? compact(element.textContent) : '';
        const interactive = Boolean(role) || element.tabIndex >= 0 || element.hasAttribute('onclick');
        if (semanticOnly && (!visible || (!interactive && !text))) continue;
        nodes.push({
            selector: cssPath(element), role, name, text, visible,
            enabled: !element.matches(':disabled') && element.getAttribute('aria-disabled') !== 'true',
            bounds: visible ? { x: rect.x, y: rect.y, width: rect.width, height: rect.height } : null,
        });
    }
    return { nodes };
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
        element.focus();
        if (element.isContentEditable) {
            element.textContent = '';
            element.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'deleteContentBackward' }));
        } else if ('value' in element) {
            const prototype = element.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
            const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
            if (setter) setter.call(element, ''); else element.value = '';
            element.dispatchEvent(new Event('input', { bubbles: true }));
        } else {
            return { error: { code: 'element_not_editable', message: 'target element is not editable' } };
        }
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
        assert!(expression.ends_with(", 10, false)"));
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
    fn page_errors_become_typed_failures() {
        let value = serde_json::json!({ "error": { "code": "no_such_element", "message": "missing" } });

        assert_eq!(
            check_script_error(&value),
            Err(BrowserControlFailure::new("no_such_element", "missing"))
        );
    }
}
