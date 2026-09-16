//! Host-owned native `<select>` popup state.
//!
//! Chromium `Page.startScreencast` and Firefox `WebDriver` screenshots omit the
//! OS/browser popup for a size-1 single-select. CDP/`WebDriver` pointer events
//! also cannot hit that popup. Horizon paints the options itself from this
//! probe and applies the chosen index through page script.

use serde_json::Value;

use crate::BrowserBounds;

const MAX_OPTIONS: usize = 512;
const MAX_LABEL_CHARS: usize = 256;
const MAX_PATH_CHARS: usize = 512;

/// Open size-1 single-select that the host should present.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeSelectPopup {
    pub css_path: String,
    pub name: String,
    pub selected_index: i32,
    pub bounds: BrowserBounds,
    pub options: Vec<NativeSelectOption>,
}

/// One `<option>` in a native popup select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeSelectOption {
    pub index: u32,
    pub value: String,
    pub label: String,
    pub group: Option<String>,
    pub disabled: bool,
    pub selected: bool,
}

/// Result of applying a host overlay choice to the live `<select>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeSelectApply {
    Applied,
    Unchanged,
    Blocked,
    Gone,
}

/// What a pointer or key event should ask the page about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeSelectProbe {
    Point { x: f64, y: f64 },
    Focused { open: bool },
}

impl NativeSelectProbe {
    /// Clicks always probe. Tab only updates focus tracking. Space/F4/Alt+Down
    /// open after a popup select was last focused, so typing Space in a text
    /// field does not pay an evaluate on every keystroke.
    #[must_use]
    pub fn from_input(input: &crate::BrowserInput, last_popup_focused: bool) -> Option<Self> {
        match input {
            crate::BrowserInput::MouseRelease {
                x,
                y,
                button: crate::BrowserButton::Left,
                ..
            } if x.is_finite() && y.is_finite() => Some(Self::Point { x: *x, y: *y }),
            crate::BrowserInput::KeyDown {
                key: crate::BrowserKey::Tab,
                ..
            } => Some(Self::Focused { open: false }),
            crate::BrowserInput::KeyDown {
                key: crate::BrowserKey::Space | crate::BrowserKey::F4,
                ..
            } if last_popup_focused => Some(Self::Focused { open: true }),
            crate::BrowserInput::KeyDown {
                key: crate::BrowserKey::ArrowDown,
                modifiers,
                ..
            } if last_popup_focused && modifiers.alt => Some(Self::Focused { open: true }),
            _ => None,
        }
    }

    #[must_use]
    pub const fn should_open(self) -> bool {
        match self {
            Self::Point { .. } | Self::Focused { open: true } => true,
            Self::Focused { open: false } => false,
        }
    }
}

impl NativeSelectPopup {
    #[must_use]
    pub fn option(&self, index: u32) -> Option<&NativeSelectOption> {
        self.options.iter().find(|option| option.index == index)
    }

    #[must_use]
    pub fn selectable_index(&self, index: u32) -> Option<u32> {
        self.option(index)
            .filter(|option| !option.disabled)
            .map(|option| option.index)
    }

    #[must_use]
    pub fn step_from(&self, current: i32, delta: i32) -> i32 {
        if self.options.is_empty() {
            return -1;
        }
        let start = if current < 0 { 0 } else { current };
        let len = i32::try_from(self.options.len()).unwrap_or(i32::MAX);
        let mut index = start;
        for _ in 0..self.options.len() {
            index += delta;
            if index < 0 {
                index = len - 1;
            } else if index >= len {
                index = 0;
            }
            let Some(option) = self.options.get(usize::try_from(index).unwrap_or(0)) else {
                break;
            };
            if !option.disabled {
                return i32::try_from(option.index).unwrap_or(index);
            }
        }
        start
    }
}

/// Page script that inspects the element at a point, or the focused element.
#[must_use]
pub fn probe_expression(probe: NativeSelectProbe) -> String {
    match probe {
        NativeSelectProbe::Point { x, y } => format!("({PROBE_FUNCTION})({x:.4}, {y:.4}, false)"),
        NativeSelectProbe::Focused { .. } => format!("({PROBE_FUNCTION})(0, 0, true)"),
    }
}

/// Page script that selects `index` on the probed control.
#[must_use]
pub fn apply_expression(css_path: &str, index: u32) -> String {
    format!("({APPLY_FUNCTION})({}, {index})", json_string(css_path))
}

/// Parse a probe evaluation into an open popup, if the target is a native one.
#[must_use]
pub fn parse_probe(value: &Value) -> Option<NativeSelectPopup> {
    if value.get("kind").and_then(Value::as_str) != Some("popup") {
        return None;
    }
    let css_path = bounded_string(value.get("cssPath"), MAX_PATH_CHARS)?;
    if css_path.is_empty() {
        return None;
    }
    let bounds = parse_bounds(value.get("bounds"))?;
    let options = parse_options(value.get("options"))?;
    if options.is_empty() {
        return None;
    }
    let selected_index = value.get("selectedIndex").and_then(Value::as_i64).unwrap_or(-1);
    Some(NativeSelectPopup {
        css_path,
        name: bounded_string(value.get("name"), MAX_LABEL_CHARS).unwrap_or_default(),
        selected_index: i32::try_from(selected_index).unwrap_or(-1),
        bounds,
        options,
    })
}

/// Parse an apply evaluation.
#[must_use]
pub fn parse_apply(value: &Value) -> NativeSelectApply {
    match value.get("kind").and_then(Value::as_str) {
        Some("applied") => NativeSelectApply::Applied,
        Some("unchanged") => NativeSelectApply::Unchanged,
        Some("blocked") => NativeSelectApply::Blocked,
        _ => NativeSelectApply::Gone,
    }
}

fn parse_bounds(value: Option<&Value>) -> Option<BrowserBounds> {
    let value = value?;
    let bounds = BrowserBounds {
        x: value.get("x").and_then(Value::as_f64)?,
        y: value.get("y").and_then(Value::as_f64)?,
        width: value.get("width").and_then(Value::as_f64)?,
        height: value.get("height").and_then(Value::as_f64)?,
    };
    let finite = [bounds.x, bounds.y, bounds.width, bounds.height]
        .into_iter()
        .all(f64::is_finite);
    (finite && bounds.width > 0.0 && bounds.height > 0.0).then_some(bounds)
}

fn parse_options(value: Option<&Value>) -> Option<Vec<NativeSelectOption>> {
    let values = value?.as_array()?;
    let mut options = Vec::new();
    for value in values.iter().take(MAX_OPTIONS) {
        let index = u32::try_from(value.get("index").and_then(Value::as_u64)?).ok()?;
        options.push(NativeSelectOption {
            index,
            value: bounded_string(value.get("value"), MAX_LABEL_CHARS).unwrap_or_default(),
            label: bounded_string(value.get("label"), MAX_LABEL_CHARS).unwrap_or_default(),
            group: bounded_string(value.get("group"), MAX_LABEL_CHARS).filter(|group| !group.is_empty()),
            disabled: value.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            selected: value.get("selected").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    Some(options)
}

fn bounded_string(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let text = value?.as_str()?;
    Some(text.chars().take(max_chars).collect())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

const PROBE_FUNCTION: &str = r"function(x, y, focused) {
    const MAX_OPTIONS = 512;
    const compact = (value, limit) => String(value || '').replace(/\s+/g, ' ').trim().slice(0, limit || 256);
    const cssPath = (element, doc) => {
        if (element.id && /^[A-Za-z][\w-]*$/.test(element.id)
            && doc.querySelectorAll('#' + element.id).length === 1) {
            return '#' + element.id;
        }
        const parts = [];
        for (let el = element; el && el.nodeType === 1 && el !== doc.documentElement; el = el.parentElement) {
            let selector = el.tagName.toLowerCase();
            if (el.id && /^[A-Za-z][\w-]*$/.test(el.id)) {
                parts.unshift('#' + el.id);
                break;
            }
            const parent = el.parentElement;
            if (parent) {
                const same = Array.prototype.filter.call(parent.children, (child) => child.tagName === el.tagName);
                if (same.length > 1) selector += ':nth-of-type(' + (same.indexOf(el) + 1) + ')';
            }
            parts.unshift(selector);
            if (parts.length > 8) break;
        }
        return parts.join('>');
    };
    const isPopupSelect = (el) => {
        if (!el || el.tagName !== 'SELECT' || el.disabled) return false;
        if (el.multiple) return false;
        const size = Number(el.getAttribute('size') || el.size || 1);
        return !(size > 1);
    };
    const topBounds = (el) => {
        const rect = el.getBoundingClientRect();
        let left = rect.x, top = rect.y;
        let win = el.ownerDocument.defaultView;
        while (win && win !== window) {
            const frame = win.frameElement;
            if (!frame) break;
            const frameRect = frame.getBoundingClientRect();
            left += frameRect.left;
            top += frameRect.top;
            win = win.parent;
        }
        return { x: left, y: top, width: rect.width, height: rect.height };
    };
    const collect = (el) => {
        const options = [];
        for (let i = 0; i < el.options.length && options.length < MAX_OPTIONS; i += 1) {
            const node = el.options[i];
            const parent = node.parentElement;
            const groupDisabled = parent && parent.tagName === 'OPTGROUP' && parent.disabled;
            options.push({
                index: node.index,
                value: String(node.value),
                label: compact(node.label || node.textContent),
                group: parent && parent.tagName === 'OPTGROUP' ? compact(parent.label) : '',
                disabled: !!(node.disabled || groupDisabled),
                selected: !!node.selected
            });
        }
        return {
            kind: 'popup',
            cssPath: cssPath(el, el.ownerDocument),
            name: compact(el.getAttribute('aria-label') || el.getAttribute('name') || el.id),
            selectedIndex: el.selectedIndex,
            value: String(el.value),
            bounds: topBounds(el),
            options: options
        };
    };
    let el = focused ? (document.activeElement || null) : document.elementFromPoint(x, y);
    let px = x, py = y;
    while (el) {
        if (el.tagName === 'IFRAME') {
            try {
                const inner = el.contentDocument;
                if (!inner) break;
                if (!focused) {
                    const rect = el.getBoundingClientRect();
                    px -= rect.left;
                    py -= rect.top;
                    el = inner.elementFromPoint(px, py);
                } else {
                    el = inner.activeElement;
                }
                continue;
            } catch (error) { break; }
        }
        if (!focused && el.shadowRoot) {
            const inner = el.shadowRoot.elementFromPoint(px, py);
            if (inner && inner !== el) { el = inner; continue; }
        }
        if (focused && el.shadowRoot && el.shadowRoot.activeElement) {
            el = el.shadowRoot.activeElement;
            continue;
        }
        break;
    }
    while (el && el.tagName !== 'SELECT') el = el.parentElement;
    if (!isPopupSelect(el)) return { kind: 'none' };
    return collect(el);
}";

const APPLY_FUNCTION: &str = r"function(cssPath, index) {
    const find = (root) => {
        try {
            const match = root.querySelector(cssPath);
            if (match && match.tagName === 'SELECT') return match;
        } catch (error) {}
        const nodes = root.querySelectorAll ? root.querySelectorAll('*') : [];
        for (const node of nodes) {
            if (node.shadowRoot) {
                const shadowed = find(node.shadowRoot);
                if (shadowed) return shadowed;
            }
            if (node.tagName !== 'IFRAME') continue;
            try {
                const inner = node.contentDocument;
                if (!inner) continue;
                const found = find(inner);
                if (found) return found;
            } catch (error) {}
        }
        return null;
    };
    const el = find(document);
    if (!el || el.disabled) return { kind: 'gone' };
    if (el.multiple) return { kind: 'gone' };
    const size = Number(el.getAttribute('size') || el.size || 1);
    if (size > 1) return { kind: 'gone' };
    const option = el.options[index];
    if (!option) return { kind: 'blocked' };
    const parent = option.parentElement;
    if (option.disabled || (parent && parent.tagName === 'OPTGROUP' && parent.disabled)) {
        return { kind: 'blocked' };
    }
    const previous = el.selectedIndex;
    el.selectedIndex = index;
    if (el.selectedIndex !== index) return { kind: 'blocked' };
    if (previous === index) return { kind: 'unchanged', value: String(el.value), selectedIndex: el.selectedIndex };
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return { kind: 'applied', value: String(el.value), selectedIndex: el.selectedIndex };
}";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserButton, BrowserInput, BrowserKey, BrowserModifiers};
    use serde_json::json;

    fn popup_json() -> Value {
        json!({
            "kind": "popup",
            "cssPath": "#native-single",
            "name": "native-single",
            "selectedIndex": 1,
            "bounds": { "x": 10.0, "y": 20.0, "width": 80.0, "height": 22.0 },
            "options": [
                { "index": 0, "value": "alpha", "label": "Alpha", "group": "", "disabled": false, "selected": false },
                { "index": 1, "value": "bravo", "label": "Bravo", "group": "", "disabled": false, "selected": true },
                { "index": 2, "value": "nope", "label": "Nope", "group": "Skip", "disabled": true, "selected": false },
                { "index": 3, "value": "charlie", "label": "Charlie", "group": "", "disabled": false, "selected": false }
            ]
        })
    }

    #[test]
    fn probe_parses_grouped_disabled_and_selected_options() {
        let popup = parse_probe(&popup_json()).expect("popup");
        assert_eq!(popup.css_path, "#native-single");
        assert_eq!(popup.selected_index, 1);
        assert_eq!(popup.options.len(), 4);
        assert_eq!(popup.options[2].group.as_deref(), Some("Skip"));
        assert!(popup.options[2].disabled);
        assert_eq!(popup.selectable_index(3), Some(3));
        assert_eq!(popup.selectable_index(2), None);
    }

    #[test]
    fn listbox_and_missing_kind_are_not_host_popups() {
        assert!(parse_probe(&json!({ "kind": "none" })).is_none());
        assert!(parse_probe(&json!({ "kind": "popup", "cssPath": "", "bounds": { "x": 1, "y": 1, "width": 1, "height": 1 }, "options": [] })).is_none());
        assert!(parse_probe(&json!({ "kind": "popup", "cssPath": "#x", "bounds": { "x": "bad" } })).is_none());
    }

    #[test]
    fn apply_kinds_map_to_typed_outcomes() {
        assert_eq!(parse_apply(&json!({ "kind": "applied" })), NativeSelectApply::Applied);
        assert_eq!(
            parse_apply(&json!({ "kind": "unchanged" })),
            NativeSelectApply::Unchanged
        );
        assert_eq!(parse_apply(&json!({ "kind": "blocked" })), NativeSelectApply::Blocked);
        assert_eq!(parse_apply(&json!({ "kind": "gone" })), NativeSelectApply::Gone);
    }

    #[test]
    fn arrow_step_skips_disabled_options_and_wraps() {
        let popup = parse_probe(&popup_json()).expect("popup");
        assert_eq!(popup.step_from(1, 1), 3);
        assert_eq!(popup.step_from(3, 1), 0);
        assert_eq!(popup.step_from(0, -1), 3);
        assert_eq!(popup.step_from(3, -1), 1);
    }

    #[test]
    fn click_and_open_keys_probe_but_typing_space_does_not() {
        let click = BrowserInput::MouseRelease {
            x: 12.0,
            y: 40.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        };
        assert_eq!(
            NativeSelectProbe::from_input(&click, false),
            Some(NativeSelectProbe::Point { x: 12.0, y: 40.0 })
        );
        let space = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Space,
            text: Some(" ".to_string()),
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        };
        assert!(NativeSelectProbe::from_input(&space, false).is_none());
        assert_eq!(
            NativeSelectProbe::from_input(&space, true),
            Some(NativeSelectProbe::Focused { open: true })
        );
        let tab = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Tab,
            text: None,
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        };
        assert_eq!(
            NativeSelectProbe::from_input(&tab, false),
            Some(NativeSelectProbe::Focused { open: false })
        );
        assert!(!NativeSelectProbe::Focused { open: false }.should_open());
        assert!(NativeSelectProbe::Focused { open: true }.should_open());
        let letter = BrowserInput::KeyDown {
            physical_key: None,
            key: BrowserKey::Char('a'),
            text: Some("a".to_string()),
            modifiers: BrowserModifiers::none(),
            repeat: false,
            edit_command: None,
        };
        assert!(NativeSelectProbe::from_input(&letter, true).is_none());
    }

    #[test]
    fn expressions_quote_paths_and_keep_probe_mode() {
        let apply = apply_expression("#native-single", 3);
        assert!(apply.contains("\"#native-single\""));
        assert!(apply.contains(", 3)"));
        assert!(!apply.contains("elementFromPoint"));
        let point = probe_expression(NativeSelectProbe::Point { x: 10.5, y: 20.25 });
        assert!(point.contains("10.5000, 20.2500, false"));
        assert!(point.contains("elementFromPoint"));
        let focused = probe_expression(NativeSelectProbe::Focused { open: false });
        assert!(focused.contains("0, 0, true"));
    }
}
