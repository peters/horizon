use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use horizon_browser_protocol::{BrowserBounds, BrowserNode};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;

use crate::recipe::Target;
use crate::{Error, Result};

const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_NODES: usize = 2500;
const REF_LIFETIME: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    #[serde(flatten)]
    pub semantic: BrowserNode,
    pub identifier: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub generation: u64,
    pub revision: u64,
    pub url: String,
    pub title: String,
    pub nodes: Vec<Node>,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Identity {
    pub attributes: Vec<(String, String)>,
    pub bounds: Option<BrowserBounds>,
}

struct Entry {
    node: Node,
    xpath: String,
    element_id: Option<String>,
    actionable: bool,
    identity: Identity,
}

pub struct References {
    namespace: String,
    generation: u64,
    acquired: Option<Instant>,
    entries: BTreeMap<String, Entry>,
}

impl Default for References {
    fn default() -> Self {
        Self {
            namespace: uuid::Uuid::new_v4().simple().to_string(),
            generation: 0,
            acquired: None,
            entries: BTreeMap::new(),
        }
    }
}

impl References {
    #[cfg(test)]
    pub(crate) fn expire_in(&mut self, remaining: Duration) {
        self.acquired = Instant::now()
            .checked_sub(REF_LIFETIME)
            .and_then(|when| when.checked_add(remaining));
    }

    pub fn invalidate(&mut self) {
        self.entries.clear();
        self.acquired = None;
    }

    /// # Errors
    /// Invalidates previous refs even when a new source cannot be decoded.
    pub fn snapshot(&mut self, xml: &str, app: &str) -> Result<Snapshot> {
        self.invalidate();
        self.generation = self.generation.checked_add(1).ok_or(Error::SourceInvalid)?;
        let decoded = decode(xml)?;
        let mut nodes = Vec::with_capacity(decoded.len());
        for (index, (mut node, xpath, actionable, identity)) in decoded.into_iter().enumerate() {
            let reference = format!("d.{}.{}.{}", self.namespace, self.generation, index);
            node.semantic.reference.clone_from(&reference);
            nodes.push(node.clone());
            self.entries.insert(
                reference,
                Entry {
                    node,
                    xpath,
                    element_id: None,
                    actionable,
                    identity,
                },
            );
        }
        self.acquired = Some(Instant::now());
        Ok(Snapshot {
            generation: self.generation,
            revision: self.generation,
            url: format!("app://{app}"),
            title: app.into(),
            nodes,
        })
    }

    pub(crate) fn binding(&self, target: &Target) -> Result<(String, Option<String>, Identity)> {
        let entry = self.lookup(target)?;
        if !entry.actionable {
            return Err(Error::TargetMissing);
        }
        if self
            .entries
            .values()
            .filter(|other| other.actionable && other.identity == entry.identity)
            .count()
            > 1
        {
            return Err(Error::TargetAmbiguous);
        }
        Ok((entry.xpath.clone(), entry.element_id.clone(), entry.identity.clone()))
    }

    pub(crate) fn bind(&mut self, target: &Target, element: &str) -> Result<()> {
        // Recheck lifetime and uniqueness after all driver identity observations.
        let (xpath, retained, _) = self.binding(target)?;
        if retained.is_some_and(|known| known != element) {
            return Err(Error::ReferenceExpired);
        }
        let entry = self
            .entries
            .values_mut()
            .find(|entry| entry.xpath == xpath)
            .ok_or(Error::ReferenceExpired)?;
        entry.element_id = Some(element.to_owned());
        Ok(())
    }

    /// # Errors
    /// Inspection only: `XPath` locations do not authorize native mutation or prove element identity.
    /// Rejects expired/foreign refs and non-unique identifier or label matches.
    pub fn xpath(&self, target: &Target) -> Result<String> {
        Ok(self.lookup(target)?.xpath.clone())
    }

    fn lookup(&self, target: &Target) -> Result<&Entry> {
        if self.acquired.is_none_or(|when| when.elapsed() > REF_LIFETIME) {
            return Err(Error::ReferenceExpired);
        }
        let entries: Vec<_> = match target {
            Target::Ref(reference) => vec![self.entries.get(reference).ok_or(Error::ReferenceExpired)?],
            Target::Identifier(identifier) => self
                .entries
                .values()
                .filter(|e| e.node.identifier == *identifier)
                .collect(),
            Target::Label(label) => self
                .entries
                .values()
                .filter(|e| e.node.semantic.name == *label)
                .collect(),
            Target::Coordinates(_) => return Err(Error::RecipeInvalid),
        };
        match entries.as_slice() {
            [] => Err(Error::TargetMissing),
            [entry] => Ok(entry),
            _ => Err(Error::TargetAmbiguous),
        }
    }
}

fn decode(xml: &str) -> Result<Vec<(Node, String, bool, Identity)>> {
    if xml.is_empty() || xml.len() > MAX_SOURCE_BYTES {
        return Err(Error::SourceInvalid);
    }
    let mut reader = Reader::from_str(xml);
    let mut levels = vec![(String::new(), 0_usize)];
    let mut decoded = Vec::new();
    let mut roots = 0;
    let mut version = quick_xml::XmlVersion::Implicit1_0;
    loop {
        let event = reader.read_event().map_err(|_| Error::SourceInvalid)?;
        match &event {
            Event::Start(start) | Event::Empty(start) => {
                let empty = matches!(event, Event::Empty(_));
                let Some((parent, next)) = levels.last_mut() else {
                    return Err(Error::SourceInvalid);
                };
                *next += 1;
                let xpath = format!("{parent}/*[{next}]");
                if levels.len() == 1 {
                    roots += 1;
                }
                if roots > 1 || levels.len() > 128 || decoded.len() >= MAX_NODES {
                    return Err(Error::SourceInvalid);
                }
                let actionable = !matches!(start.name().as_ref(), "AppiumAUT" | "hierarchy");
                let (node, identity) = node(start, version)?;
                decoded.push((node, xpath.clone(), actionable, identity));
                if !empty {
                    levels.push((xpath, 0));
                }
            }
            Event::End(_) => {
                if levels.len() <= 1 {
                    return Err(Error::SourceInvalid);
                }
                levels.pop();
            }
            Event::DocType(_) | Event::GeneralRef(_) => return Err(Error::SourceInvalid),
            Event::Decl(declaration) => {
                if roots != 0 {
                    return Err(Error::SourceInvalid);
                }
                version = match declaration.version().map_err(|_| Error::SourceInvalid)?.as_ref() {
                    "1.0" => quick_xml::XmlVersion::Explicit1_0,
                    _ => return Err(Error::SourceInvalid),
                };
            }
            Event::Text(text) if !text.as_ref().bytes().all(|b| b.is_ascii_whitespace()) => {
                return Err(Error::SourceInvalid);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if roots != 1 || levels.len() != 1 {
        return Err(Error::SourceInvalid);
    }
    Ok(decoded)
}

fn node(event: &BytesStart<'_>, version: quick_xml::XmlVersion) -> Result<(Node, Identity)> {
    let tag = event.name().as_ref().to_owned();
    let mut attributes = BTreeMap::new();
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|_| Error::SourceInvalid)?;
        let key = attribute.key.as_ref().to_owned();
        let value = attribute
            .normalized_value(version)
            .map_err(|_| Error::SourceInvalid)?
            .into_owned();
        if key.len() > 128 || value.len() > 16 * 1024 {
            return Err(Error::SourceInvalid);
        }
        attributes.insert(key, value);
    }
    let get = |key| attributes.get(key).map_or("", String::as_str);
    let class = if tag.starts_with("XCUIElementType") || get("class").is_empty() {
        tag.as_str()
    } else {
        get("class")
    };
    let secure = tag.contains("SecureTextField") || class.contains("SecureTextField") || get("password") == "true";
    let text = if secure { "[redacted]" } else { get("text") };
    let editable = class.contains("TextField") || class.contains("EditText") || class.contains("AutoCompleteTextView");
    let value = if secure {
        "[redacted]"
    } else if !attributes.contains_key("value") && editable && !tag.starts_with("XCUIElementType") {
        get("text")
    } else {
        get("value")
    };
    let identifier = if secure {
        ""
    } else if get("resource-id").is_empty() {
        get("name")
    } else {
        get("resource-id")
    };
    let name = if secure {
        "[redacted]"
    } else {
        [get("label"), get("content-desc"), text, identifier]
            .into_iter()
            .find(|v| !v.is_empty())
            .unwrap_or("")
    };
    let role = if editable {
        "textbox"
    } else if class.contains("Button") || get("clickable") == "true" {
        "button"
    } else if class.contains("Text") {
        "text"
    } else if class.contains("Image") {
        "image"
    } else {
        "group"
    };
    let ios = tag.starts_with("XCUIElementType");
    let mut identifying = vec![(if ios { "type" } else { "class" }.to_owned(), class.to_owned())];
    if !ios {
        identifying.push(("password".to_owned(), if secure { "true" } else { "false" }.to_owned()));
    }
    let keys: &[&str] = if ios {
        &["name", "label"]
    } else if secure {
        &["resource-id", "content-desc"]
    } else {
        &["resource-id", "content-desc", "text"]
    };
    for key in keys {
        if let Some(value) = attributes.get(*key) {
            identifying.push(((*key).to_owned(), value.clone()));
        }
    }
    let identity = Identity {
        attributes: identifying,
        bounds: bounds(&attributes),
    };
    Ok((
        Node {
            semantic: BrowserNode {
                reference: String::new(),
                role: role.into(),
                name: name.into(),
                text: text.into(),
                visible: get("visible") != "false" && get("displayed") != "false",
                enabled: get("enabled") != "false",
                bounds: bounds(&attributes),
                file_input: None,
            },
            identifier: identifier.into(),
            value: value.into(),
        },
        identity,
    ))
}

fn bounds(attributes: &BTreeMap<String, String>) -> Option<BrowserBounds> {
    let number = |key| attributes.get(key)?.parse::<f64>().ok().filter(|v| v.is_finite());
    if let Some(android) = attributes.get("bounds") {
        let numbers: Vec<_> = android
            .split(['[', ']', ','])
            .filter(|v| !v.is_empty())
            .map(str::parse::<f64>)
            .collect::<std::result::Result<_, _>>()
            .ok()?;
        let [left, top, right, bottom] = numbers.as_slice() else {
            return None;
        };
        if numbers.iter().any(|n| !n.is_finite()) || right < left || bottom < top {
            return None;
        }
        let width = right - left;
        let height = bottom - top;
        if !width.is_finite() || !height.is_finite() {
            return None;
        }
        Some(BrowserBounds {
            x: *left,
            y: *top,
            width,
            height,
        })
    } else {
        let width = number("width")?;
        let height = number("height")?;
        if width < 0.0 || height < 0.0 {
            return None;
        }
        Some(BrowserBounds {
            x: number("x")?,
            y: number("y")?,
            width,
            height,
        })
    }
}
