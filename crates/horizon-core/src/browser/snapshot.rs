//! Page snapshotting for agent use (`hb snap`).
//!
//! The snapshot is a small canned JS expression that returns a compact text
//! description of the visible page: URL, title, headings, links, buttons,
//! and form controls. Agents can read pages with plain text instead of
//! vision-only screenshots.

/// The snapshot expression. Must remain a single self-contained IIFE.
pub const SNAPSHOT_JS: &str = r#"(() => {
  const lines = [];
  const push = (s) => { if (lines.length < 200) lines.push(String(s).slice(0, 300)); };
  push('URL: ' + location.href);
  push('TITLE: ' + (document.title || '(untitled)'));
  const visible = (el) => {
    const style = getComputedStyle(el);
    return style.display !== 'none' && style.visibility !== 'hidden' && !el.hasAttribute('hidden');
  };
  const labelFor = (el) => {
    if (el.id) { const l = document.querySelector('label[for="' + CSS.escape(el.id) + '"]'); if (l) return l.textContent.trim(); }
    const wrap = el.closest('label');
    if (wrap) return wrap.textContent.trim().replace(el.value || '', '').trim();
    return el.name || el.getAttribute('aria-label') || el.placeholder || '';
  };
  document.querySelectorAll('h1,h2,h3').forEach((h) => { if (visible(h)) push('# ' + h.tagName.toLowerCase() + ': ' + h.textContent.trim()); });
  document.querySelectorAll('a[href]').forEach((a) => {
    if (!visible(a)) return;
    const text = a.textContent.trim().slice(0, 80);
    push('[link] ' + (text || a.href) + ' -> ' + a.href);
  });
  document.querySelectorAll('button,[role="button"],input[type="submit"],input[type="button"]').forEach((b) => {
    if (!visible(b)) return;
    const text = (b.textContent || b.value || b.getAttribute('aria-label') || 'button').trim().slice(0, 80);
    push('[button] ' + text);
  });
  document.querySelectorAll('input,textarea,select').forEach((i) => {
    if (!visible(i)) return;
    const label = labelFor(i);
    const value = (i.value || '').slice(0, 80);
    push('[field] ' + i.tagName.toLowerCase() + (i.type ? ':' + i.type : '') + (label ? ' "' + label + '"' : '') + (value ? ' = "' + value + '"' : ''));
  });
  const bodyText = (document.body && document.body.innerText || '').trim();
  if (bodyText) {
    const chunks = bodyText.split(/\n+/).slice(0, 40);
    chunks.forEach((c) => push('  ' + c));
  }
  return lines.join('\n');
})()"#;

/// Extract the text snapshot from a `Runtime.evaluate` result value.
#[must_use]
pub fn snapshot_text(result: &serde_json::Value) -> String {
    if let Some(value) = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_str())
    {
        return value.to_string();
    }
    if let Some(exception) = result
        .get("exceptionDetails")
        .and_then(|e| e.get("exception"))
        .and_then(|e| e.get("description"))
        .and_then(|d| d.as_str())
    {
        return format!("snapshot error: {exception}");
    }
    result.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_result() {
        let value = serde_json::json!({"result": {"type": "string", "value": "URL: x\nTITLE: y"}});
        assert_eq!(snapshot_text(&value), "URL: x\nTITLE: y");
    }

    #[test]
    fn parses_exception() {
        let value = serde_json::json!({
            "exceptionDetails": {"exception": {"description": "boom at <anonymous>:1:1"}}
        });
        assert!(snapshot_text(&value).starts_with("snapshot error:"));
    }

    #[test]
    fn js_is_single_iife() {
        assert!(SNAPSHOT_JS.starts_with("(() => {"));
        assert!(SNAPSHOT_JS.trim_end().ends_with("})()"));
    }
}
