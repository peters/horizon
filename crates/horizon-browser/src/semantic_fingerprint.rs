//! Action-time Teach fingerprints. Never consulted on pointer-move or frames.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::BrowserControlFailure;
use crate::semantic::check_script_error;

const MAX_CANDIDATES: usize = 8;
const MAX_FIELD_BYTES: usize = 4 * 1024;
const MAX_DIGEST_BYTES: usize = 128;
const MAX_FRAME_CHAIN: usize = 8;

/// Ranked backend-neutral identity captured at click/fill time.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeachFingerprint {
    pub candidates: Vec<RankedCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    pub frame: FrameContext,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedCandidate {
    pub identity: TargetCandidate,
    pub match_count: u32,
    pub unique: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetCandidate {
    RoleName {
        role: String,
        name: String,
        #[serde(default)]
        reviewed: bool,
    },
    LabelControl {
        label: String,
        control: String,
        #[serde(default)]
        reviewed: bool,
    },
    TestId {
        attribute: String,
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
    UniqueId {
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
    VisibleText {
        text: String,
        context: String,
        #[serde(default)]
        reviewed: bool,
    },
    CssFallback {
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
}

impl TargetCandidate {
    const fn is_unreviewed_css(&self) -> bool {
        matches!(self, Self::CssFallback { reviewed: false, .. })
    }

    fn semantic_identity(&self) -> Self {
        let mut identity = self.clone();
        match &mut identity {
            Self::RoleName { reviewed, .. }
            | Self::LabelControl { reviewed, .. }
            | Self::TestId { reviewed, .. }
            | Self::UniqueId { reviewed, .. }
            | Self::VisibleText { reviewed, .. }
            | Self::CssFallback { reviewed, .. } => *reviewed = false,
        }
        identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameContext {
    pub top_level: bool,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<FrameLink>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameLink {
    pub origin: String,
    pub name: String,
}

/// Page-script observation of one hit-tested element. Uniqueness counts are
/// measured against the current document, not guessed.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ElementObservation {
    pub origin: String,
    #[serde(default = "default_true")]
    pub top_level: bool,
    #[serde(default)]
    pub frame_chain: Vec<FrameLink>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub control: String,
    #[serde(default)]
    pub test_id_attribute: String,
    #[serde(default)]
    pub test_id_value: String,
    #[serde(default)]
    pub element_id: String,
    #[serde(default)]
    pub visible_text: String,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub css_path: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub role_name_matches: u32,
    #[serde(default)]
    pub label_matches: u32,
    #[serde(default)]
    pub test_id_matches: u32,
    #[serde(default)]
    pub id_matches: u32,
    #[serde(default)]
    pub text_matches: u32,
    #[serde(default)]
    pub css_matches: u32,
}

const fn default_true() -> bool {
    true
}

/// Rank a hit-tested element into durable candidates. CSS fallback is never
/// marked reviewed.
///
/// # Errors
/// Missing origin, empty identity, or oversized fields.
pub fn fingerprint_from_observation(
    observation: &ElementObservation,
) -> Result<TeachFingerprint, BrowserControlFailure> {
    validate_origin(&observation.origin)?;
    if observation.frame_chain.len() > MAX_FRAME_CHAIN {
        return Err(fail("invalid_fingerprint", "frame chain exceeds the bound"));
    }
    for link in &observation.frame_chain {
        validate_origin(&link.origin)?;
        if !link.name.is_empty() {
            bounded_field(&link.name)?;
        }
    }
    let mut candidates = Vec::new();
    push_candidate(
        &mut candidates,
        TargetCandidate::RoleName {
            role: observation.role.clone(),
            name: observation.name.clone(),
            reviewed: false,
        },
        observation.role_name_matches,
        !observation.role.is_empty() && !observation.name.is_empty(),
    )?;
    push_candidate(
        &mut candidates,
        TargetCandidate::LabelControl {
            label: observation.label.clone(),
            control: observation.control.clone(),
            reviewed: false,
        },
        observation.label_matches,
        !observation.label.is_empty() && !observation.control.is_empty(),
    )?;
    push_candidate(
        &mut candidates,
        TargetCandidate::TestId {
            attribute: observation.test_id_attribute.clone(),
            value: observation.test_id_value.clone(),
            reviewed: false,
        },
        observation.test_id_matches,
        !observation.test_id_attribute.is_empty() && !observation.test_id_value.is_empty(),
    )?;
    if !is_dynamic_id(&observation.element_id) {
        push_candidate(
            &mut candidates,
            TargetCandidate::UniqueId {
                value: observation.element_id.clone(),
                reviewed: false,
            },
            observation.id_matches,
            !observation.element_id.is_empty(),
        )?;
    }
    push_candidate(
        &mut candidates,
        TargetCandidate::VisibleText {
            text: observation.visible_text.clone(),
            context: observation.context.clone(),
            reviewed: false,
        },
        observation.text_matches,
        !observation.visible_text.is_empty(),
    )?;
    push_candidate(
        &mut candidates,
        TargetCandidate::CssFallback {
            value: observation.css_path.clone(),
            reviewed: false,
        },
        observation.css_matches.max(1),
        !observation.css_path.is_empty(),
    )?;
    if candidates.is_empty() {
        return Err(fail("undurable_target", "element has no durable semantic candidate"));
    }
    candidates.truncate(MAX_CANDIDATES);
    let selected = candidates
        .iter()
        .position(|candidate| candidate.unique && !candidate.identity.is_unreviewed_css())
        .and_then(|index| u32::try_from(index).ok());
    let digest = digest_from_candidates(&candidates);
    Ok(TeachFingerprint {
        candidates,
        selected,
        frame: FrameContext {
            top_level: observation.top_level && observation.frame_chain.is_empty(),
            origin: observation.origin.clone(),
            chain: observation.frame_chain.clone(),
        },
        digest,
    })
}

/// Parse a page-script observation, checking script errors first.
///
/// # Errors
/// Script failure, malformed JSON, or ranking failure.
pub fn fingerprint_from_script_value(value: &Value) -> Result<TeachFingerprint, BrowserControlFailure> {
    check_script_error(value)?;
    let observation: ElementObservation = serde_json::from_value(value.clone())
        .map_err(|_| fail("invalid_fingerprint", "page script returned an invalid fingerprint"))?;
    fingerprint_from_observation(&observation)
}

/// Replay match: exactly one unique compatible candidate on the current page.
///
/// # Errors
/// Zero or many matches, or digest mismatch.
pub fn match_fingerprint(
    fingerprint: &TeachFingerprint,
    observations: &[ElementObservation],
) -> Result<usize, BrowserControlFailure> {
    let Some(selected) = fingerprint
        .selected
        .and_then(|index| fingerprint.candidates.get(index as usize))
    else {
        return Err(fail("needs_reteach", "fingerprint has no unique candidate"));
    };
    if selected.identity.is_unreviewed_css() {
        return Err(fail("needs_reteach", "css fallback requires review before replay"));
    }
    let selected_identity = selected.identity.semantic_identity();
    let mut hits = Vec::new();
    for (index, observation) in observations.iter().enumerate() {
        let Ok(ranked) = fingerprint_from_observation(observation) else {
            continue;
        };
        if ranked.digest != fingerprint.digest || ranked.frame != fingerprint.frame {
            continue;
        }
        if ranked
            .candidates
            .iter()
            .any(|candidate| candidate.unique && candidate.identity.semantic_identity() == selected_identity)
        {
            hits.push(index);
        }
    }
    match hits.as_slice() {
        [index] => Ok(*index),
        [] => Err(fail("needs_reteach", "no unique compatible target on this page")),
        _ => Err(fail("needs_reteach", "target resolved ambiguously")),
    }
}

pub(crate) fn fingerprint_at_point_expression(x: f64, y: f64) -> String {
    format!("({FINGERPRINT_FUNCTION})({x}, {y}, false)")
}

pub(crate) fn fingerprint_focused_expression() -> String {
    format!("({FINGERPRINT_FUNCTION})(0, 0, true)")
}

fn push_candidate(
    candidates: &mut Vec<RankedCandidate>,
    identity: TargetCandidate,
    match_count: u32,
    present: bool,
) -> Result<(), BrowserControlFailure> {
    if !present {
        return Ok(());
    }
    match &identity {
        TargetCandidate::RoleName { role, name, .. } => {
            bounded_field(role)?;
            bounded_field(name)?;
        }
        TargetCandidate::LabelControl { label, control, .. } => {
            bounded_field(label)?;
            bounded_field(control)?;
        }
        TargetCandidate::TestId { attribute, value, .. }
        | TargetCandidate::VisibleText {
            text: attribute,
            context: value,
            ..
        } => {
            bounded_field(attribute)?;
            bounded_field(value)?;
        }
        TargetCandidate::UniqueId { value, .. } | TargetCandidate::CssFallback { value, .. } => {
            bounded_field(value)?;
        }
    }
    let match_count = match_count.max(1);
    candidates.push(RankedCandidate {
        unique: match_count == 1,
        match_count,
        identity,
    });
    Ok(())
}

fn bounded_field(value: &str) -> Result<(), BrowserControlFailure> {
    if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control) {
        return Err(fail(
            "invalid_fingerprint",
            "candidate field is empty, oversized, or has controls",
        ));
    }
    Ok(())
}

fn digest_from_candidates(candidates: &[RankedCandidate]) -> String {
    for candidate in candidates {
        let digest = match &candidate.identity {
            TargetCandidate::RoleName { role, name, .. } => format!("{role}/{name}"),
            TargetCandidate::LabelControl { label, control, .. } => format!("{label}/{control}"),
            TargetCandidate::TestId { value, .. } | TargetCandidate::UniqueId { value, .. } => value.clone(),
            TargetCandidate::VisibleText { text, context, .. } => {
                if context.is_empty() {
                    text.clone()
                } else {
                    format!("{text}/{context}")
                }
            }
            TargetCandidate::CssFallback { .. } => continue,
        };
        return digest.chars().take(MAX_DIGEST_BYTES).collect();
    }
    "el".to_string()
}

fn validate_origin(origin: &str) -> Result<(), BrowserControlFailure> {
    let origin = origin.trim();
    if origin.is_empty() || origin.len() > MAX_FIELD_BYTES || origin.contains('@') {
        return Err(fail("invalid_fingerprint", "frame origin is not an exact origin"));
    }
    let Some((scheme, rest)) = origin.split_once("://") else {
        return Err(fail("invalid_fingerprint", "frame origin is not an exact origin"));
    };
    if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return Err(fail("invalid_fingerprint", "frame origin is not an exact origin"));
    }
    let host = match rest.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) => host,
        _ => rest,
    };
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err(fail("invalid_fingerprint", "frame origin is not an exact origin"));
    }
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    match scheme {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(fail("invalid_fingerprint", "frame origin is not an exact origin")),
    }
}

fn is_dynamic_id(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    let hexish = value.len() >= 24 && value.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-');
    let react = value.starts_with(":r") && value.ends_with(':');
    let uuidish = value.len() == 36 && value.chars().filter(|ch| *ch == '-').count() == 4;
    hexish || react || uuidish
}

fn fail(code: &str, message: &str) -> BrowserControlFailure {
    BrowserControlFailure::new(code, message)
}

const FINGERPRINT_FUNCTION: &str = r#"function(x, y, focused) {
    const compact = (value, limit = 512) => String(value || '').replace(/\s+/g, ' ').trim().slice(0, limit);
    const count = (root, selector) => { try { return root.querySelectorAll(selector).length; } catch (error) { return 0; } };
    const generatedId = (value) => {
        if (!value) return true;
        const hexish = value.length >= 24 && /^[0-9a-fA-F-]+$/.test(value);
        const react = value.startsWith(':r') && value.endsWith(':');
        const uuidish = value.length === 36 && (value.match(/-/g) || []).length === 4;
        return hexish || react || uuidish;
    };
    const collect = (root) => {
        const out = [];
        const walk = (base) => {
            if (!base || !base.querySelectorAll) return;
            for (const node of base.querySelectorAll('*')) {
                out.push(node);
                if (node.shadowRoot) walk(node.shadowRoot);
            }
        };
        walk(root);
        return out;
    };
    const cssPath = (element, root) => {
        if (element.id && count(root, '#' + CSS.escape(element.id)) === 1) return '#' + CSS.escape(element.id);
        const parts = [];
        let current = element;
        while (current && current.nodeType === 1 && current !== root.documentElement) {
            const tag = current.tagName.toLowerCase();
            let index = 1;
            let sibling = current.previousElementSibling;
            while (sibling) { if (sibling.tagName === current.tagName) index += 1; sibling = sibling.previousElementSibling; }
            parts.unshift(tag + ':nth-of-type(' + index + ')');
            current = current.parentElement;
        }
        return parts.join(' > ').slice(0, 2048);
    };
    const labelFor = (element, root) => {
        if (element.id) {
            const label = root.querySelector('label[for="' + CSS.escape(element.id) + '"]');
            if (label) return compact(label.textContent);
        }
        const wrapping = element.closest('label');
        return wrapping ? compact(wrapping.textContent) : '';
    };
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
    const nameFor = (element, root) => {
        const direct = element.getAttribute('aria-label');
        if (direct) return compact(direct);
        const labelledBy = element.getAttribute('aria-labelledby');
        if (labelledBy && root && root.getElementById) {
            const labelled = labelledBy.split(/\s+/).map((id) => root.getElementById(id)?.textContent || '').join(' ');
            if (compact(labelled)) return compact(labelled);
        }
        return compact(element.getAttribute('alt') || element.getAttribute('title') ||
            ((element.tagName === 'INPUT' && /^(button|submit|reset)$/i.test(element.getAttribute('type') || '')) ? element.value : '') ||
            element.textContent);
    };
    const frameName = (frame) => compact(frame.title || frame.name || frame.getAttribute('aria-label'));
    const crossOriginFrame = (frame) => {
        let frameOrigin = '';
        try {
            const src = frame.getAttribute('src');
            if (src) frameOrigin = new URL(src, location.href).origin;
        } catch (error) {}
        return { error: { code: 'cross_origin_frame', message: 'target is inside a cross-origin frame', origin: frameOrigin } };
    };
    let doc = document;
    let chain = [];
    let px = x;
    let py = y;
    let el = focused ? (document.activeElement || document.body) : document.elementFromPoint(x, y);
    while (el) {
        if (el.tagName === 'IFRAME') {
            try {
                const inner = el.contentDocument;
                if (!inner) return crossOriginFrame(el);
                if (!focused) {
                    const rect = el.getBoundingClientRect();
                    px -= rect.left;
                    py -= rect.top;
                }
                chain.push({ origin: inner.location.origin, name: frameName(el) });
                el = focused ? (inner.activeElement || inner.body) : inner.elementFromPoint(px, py);
                doc = inner;
                continue;
            } catch (error) { return crossOriginFrame(el); }
        }
        if (focused) {
            if (el.shadowRoot && el.shadowRoot.activeElement) {
                el = el.shadowRoot.activeElement;
                continue;
            }
            break;
        }
        if (el.shadowRoot) {
            const inner = el.shadowRoot.elementFromPoint(px, py);
            if (inner && inner !== el) {
                el = inner;
                continue;
            }
        }
        break;
    }
    if (!el || el.nodeType !== 1) return { error: { code: 'no_such_element', message: 'no element under the pointer' } };
    const searchRoot = (el.getRootNode && el.getRootNode() instanceof ShadowRoot) ? el.getRootNode() : doc;
    const role = roleFor(el);
    const name = nameFor(el, searchRoot);
    const control = role || el.tagName.toLowerCase();
    const label = labelFor(el, searchRoot);
    const testAttr = ['data-testid', 'data-test-id', 'data-qa'].find((attr) => el.getAttribute(attr));
    const testVal = testAttr ? compact(el.getAttribute(testAttr)) : '';
    const host = el.getRootNode && el.getRootNode() instanceof ShadowRoot ? compact(el.getRootNode().host?.getAttribute('aria-label') || el.getRootNode().host?.id) : '';
    const visible = compact(el.children.length === 0 ? el.textContent : name);
    const id = compact(el.id);
    const durableId = generatedId(id) ? '' : id;
    const path = cssPath(el, doc);
    const origin = chain.length ? chain[chain.length - 1].origin : location.origin;
    const nodes = collect(searchRoot);
    const roleNameMatches = role && name ? nodes.filter((node) => roleFor(node) === role && nameFor(node, searchRoot) === name).length : 0;
    const labelMatches = label ? nodes.filter((node) => labelFor(node, searchRoot) === label).length : 0;
    const textMatches = visible ? nodes.filter((node) => compact(node.children.length === 0 ? node.textContent : nameFor(node, searchRoot)) === visible).length : 0;
    return {
        origin, topLevel: chain.length === 0 && window === window.top, frameChain: chain,
        role, name, label, control,
        testIdAttribute: testAttr || '', testIdValue: testVal, elementId: id,
        visibleText: visible, context: host, cssPath: path,
        digest: compact([role, name, testVal || durableId].filter(Boolean).join('/'), 128),
        roleNameMatches,
        labelMatches,
        testIdMatches: testAttr ? count(searchRoot, '[' + testAttr + '="' + CSS.escape(testVal) + '"]') : 0,
        idMatches: durableId ? count(searchRoot, '#' + CSS.escape(durableId)) : 0,
        textMatches,
        cssMatches: path ? Math.max(1, count(doc, path)) : 0
    };
}"#;

#[cfg(test)]
mod tests {
    use super::{
        ElementObservation, FrameLink, TargetCandidate, fingerprint_at_point_expression,
        fingerprint_focused_expression, fingerprint_from_observation, is_dynamic_id, match_fingerprint,
        validate_origin,
    };
    use crate::semantic::{scan_expression, wait_scan_expression};

    fn labeled_textbox() -> ElementObservation {
        ElementObservation {
            origin: "https://reports.example".to_string(),
            top_level: true,
            role: "textbox".to_string(),
            name: "Month".to_string(),
            label: "Month".to_string(),
            control: "textbox".to_string(),
            digest: "textbox/Month".to_string(),
            role_name_matches: 1,
            label_matches: 1,
            css_path: "form > input:nth-of-type(1)".to_string(),
            css_matches: 1,
            ..ElementObservation::default()
        }
    }

    #[test]
    fn label_and_role_rank_ahead_of_css() {
        let fingerprint = fingerprint_from_observation(&labeled_textbox()).expect("rank");
        assert!(matches!(
            fingerprint.candidates[0].identity,
            TargetCandidate::RoleName { .. }
        ));
        assert!(matches!(
            fingerprint.candidates[1].identity,
            TargetCandidate::LabelControl { .. }
        ));
        assert!(fingerprint.candidates.last().is_some_and(|candidate| {
            matches!(candidate.identity, TargetCandidate::CssFallback { reviewed: false, .. })
        }));
        assert_eq!(fingerprint.selected, Some(0));
        assert!(fingerprint.candidates[0].unique);
    }

    #[test]
    fn duplicate_text_is_recorded_as_non_unique() {
        let mut observation = labeled_textbox();
        observation.visible_text = "Save".to_string();
        observation.context = "row 3".to_string();
        observation.text_matches = 2;
        observation.role_name_matches = 2;
        observation.name = "Save".to_string();
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        let text = fingerprint
            .candidates
            .iter()
            .find(|candidate| matches!(candidate.identity, TargetCandidate::VisibleText { .. }))
            .expect("text");
        assert!(!text.unique);
        assert_eq!(text.match_count, 2);
    }

    #[test]
    fn generated_ids_are_not_durable_unique_ids() {
        assert!(is_dynamic_id("4f8a1c2b9d0e7a6b5c4d3e2f"));
        assert!(is_dynamic_id(":r0:"));
        assert!(is_dynamic_id("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"));
        let mut observation = labeled_textbox();
        observation.element_id = "4f8a1c2b9d0e7a6b5c4d3e2f".to_string();
        observation.id_matches = 1;
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        assert!(
            !fingerprint
                .candidates
                .iter()
                .any(|candidate| matches!(candidate.identity, TargetCandidate::UniqueId { .. }))
        );
    }

    #[test]
    fn unnamed_same_origin_iframe_is_durable() {
        let mut observation = labeled_textbox();
        observation.top_level = false;
        observation.frame_chain = vec![FrameLink {
            origin: "https://reports.example".to_string(),
            name: String::new(),
        }];
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        assert_eq!(fingerprint.frame.chain[0].name, "");
        assert_eq!(fingerprint.frame.origin, "https://reports.example");
    }

    #[test]
    fn nested_frame_keeps_its_own_origin() {
        let mut observation = labeled_textbox();
        observation.top_level = false;
        observation.origin = "https://idp.example".to_string();
        observation.frame_chain = vec![FrameLink {
            origin: "https://idp.example".to_string(),
            name: "login".to_string(),
        }];
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        assert!(!fingerprint.frame.top_level);
        assert_eq!(fingerprint.frame.origin, "https://idp.example");
        assert_eq!(fingerprint.frame.chain[0].name, "login");
    }

    #[test]
    fn shadow_host_is_recorded_as_visible_text_context() {
        let mut observation = labeled_textbox();
        observation.context = "date-picker".to_string();
        observation.visible_text = "15".to_string();
        observation.text_matches = 1;
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        assert!(fingerprint.candidates.iter().any(|candidate| matches!(
            &candidate.identity,
            TargetCandidate::VisibleText { context, .. } if context == "date-picker"
        )));
    }

    #[test]
    fn unique_digest_match_replays_and_duplicates_need_reteach() {
        let fingerprint = fingerprint_from_observation(&labeled_textbox()).expect("rank");
        assert_eq!(match_fingerprint(&fingerprint, &[labeled_textbox()]).expect("hit"), 0);
        let mut twin = labeled_textbox();
        twin.css_path = "form > input:nth-of-type(2)".to_string();
        assert_eq!(
            match_fingerprint(&fingerprint, &[labeled_textbox(), twin])
                .err()
                .map(|error| error.code),
            Some("needs_reteach".to_string())
        );
    }

    #[test]
    fn match_requires_compatible_frame_context() {
        let fingerprint = fingerprint_from_observation(&labeled_textbox()).expect("rank");
        let mut other_frame = labeled_textbox();
        other_frame.top_level = false;
        other_frame.origin = "https://idp.example".to_string();
        other_frame.frame_chain = vec![FrameLink {
            origin: "https://idp.example".to_string(),
            name: "login".to_string(),
        }];
        assert_eq!(
            match_fingerprint(&fingerprint, &[other_frame])
                .err()
                .map(|error| error.code),
            Some("needs_reteach".to_string())
        );
    }

    #[test]
    fn generated_ids_are_omitted_from_digest() {
        let mut observation = labeled_textbox();
        observation.element_id = "4f8a1c2b9d0e7a6b5c4d3e2f".to_string();
        observation.digest = "textbox/Month/4f8a1c2b9d0e7a6b5c4d3e2f".to_string();
        let fingerprint = fingerprint_from_observation(&observation).expect("rank");
        assert!(!fingerprint.digest.contains("4f8a1c2b9d0e7a6b5c4d3e2f"));
        assert_eq!(fingerprint.digest, "textbox/Month");
    }

    #[test]
    fn reviewed_flag_is_not_part_of_replay_identity() {
        let mut fingerprint = fingerprint_from_observation(&labeled_textbox()).expect("rank");
        if let TargetCandidate::RoleName { reviewed, .. } = &mut fingerprint.candidates[0].identity {
            *reviewed = true;
        }
        assert_eq!(match_fingerprint(&fingerprint, &[labeled_textbox()]).expect("hit"), 0);
    }

    fn css_only() -> ElementObservation {
        ElementObservation {
            origin: "https://reports.example".to_string(),
            top_level: true,
            css_path: "form > input:nth-of-type(1)".to_string(),
            css_matches: 1,
            digest: "el".to_string(),
            ..ElementObservation::default()
        }
    }

    #[test]
    fn unreviewed_css_fallback_does_not_replay() {
        let fingerprint = fingerprint_from_observation(&css_only()).expect("rank");
        assert!(fingerprint.selected.is_none());
        assert_eq!(
            match_fingerprint(&fingerprint, &[css_only()])
                .err()
                .map(|error| error.code),
            Some("needs_reteach".to_string())
        );
        let mut reviewed = fingerprint;
        if let TargetCandidate::CssFallback { reviewed, .. } = &mut reviewed.candidates[0].identity {
            *reviewed = true;
        }
        reviewed.selected = Some(0);
        assert_eq!(match_fingerprint(&reviewed, &[css_only()]).expect("hit"), 0);
    }

    #[test]
    fn teach_scripts_are_absent_from_per_frame_scans() {
        let scan = scan_expression(None, 10);
        let wait = wait_scan_expression("button", 10);
        assert!(!scan.contains("elementFromPoint"));
        assert!(!wait.contains("elementFromPoint"));
        assert!(!scan.contains("FINGERPRINT"));
        let at_point = fingerprint_at_point_expression(12.0, 40.0);
        assert!(at_point.contains("elementFromPoint"));
        assert!(at_point.contains("shadowRoot"));
        assert!(at_point.contains("contentDocument"));
        assert!(at_point.contains("data-testid"));
        assert!(at_point.contains("IFRAME"));
        assert!(at_point.contains("el.shadowRoot.elementFromPoint"));
        assert!(at_point.contains("label[for="));
        assert!(at_point.contains("generatedId"));
        assert!(at_point.contains("px -= rect.left"));
        assert!(at_point.contains("collect("));
        assert!(at_point.contains("roleFor(node) === role && nameFor(node, searchRoot) === name"));
        assert!(at_point.contains("aria-labelledby"));
        assert!(!at_point.contains("labelMatches: label ? 1 : 0"));
        assert!(!at_point.contains("textMatches: visible ? 1 : 0"));
        let focused = fingerprint_focused_expression();
        assert!(focused.contains("activeElement"));
        assert!(focused.contains("shadowRoot.activeElement"));
        assert!(focused.contains("inner.activeElement"));
        assert!(at_point.contains("cross_origin_frame"));
        assert!(at_point.contains("return '';"));
        assert!(!at_point.contains("return tag;"));
        assert!(!at_point.contains("MouseMove"));
    }

    #[test]
    fn exact_origin_rejects_path_and_localhost_prefix() {
        assert!(validate_origin("https://reports.example").is_ok());
        assert!(validate_origin("http://127.0.0.1:8080").is_ok());
        assert!(validate_origin("http://localhost").is_ok());
        assert!(validate_origin("http://[::1]").is_ok());
        assert!(validate_origin("https://example.com/path").is_err());
        assert!(validate_origin("http://localhost.evil.example").is_err());
        assert!(validate_origin("https://user:name@example.com").is_err());
        assert!(validate_origin("http://example.test").is_err());
    }
}
