use std::collections::BTreeMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::panel::PanelId;

/// A single piece of shared context published to a workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContextItem {
    pub key: String,
    pub value: String,
    /// The panel that published this item (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_panel_id: Option<u64>,
    #[serde(with = "system_time_serde")]
    pub published_at: SystemTime,
    #[serde(default)]
    pub pinned: bool,
}

/// Shared context for a workspace.
///
/// Agents can publish key-value context items via OSC escape sequences. The
/// board processes these each frame, routing them to the correct workspace.
/// Only pinned items survive persistence across restarts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceContext {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    items: BTreeMap<String, ContextItem>,
}

impl WorkspaceContext {
    /// Publish or update a context item.
    pub fn publish(&mut self, key: String, value: String, source: Option<PanelId>) {
        let preserve_pin = self.items.get(&key).is_some_and(|existing| existing.pinned);
        self.items.insert(
            key.clone(),
            ContextItem {
                key,
                value,
                source_panel_id: source.map(|id| id.0),
                published_at: SystemTime::now(),
                pinned: preserve_pin,
            },
        );
    }

    /// Remove a context item by key. Returns `true` if it existed.
    pub fn remove(&mut self, key: &str) -> bool {
        self.items.remove(key).is_some()
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ContextItem> {
        self.items.get(key)
    }

    pub fn items(&self) -> impl Iterator<Item = &ContextItem> {
        self.items.values()
    }

    pub fn pinned_items(&self) -> impl Iterator<Item = &ContextItem> {
        self.items.values().filter(|item| item.pinned)
    }

    /// Mark an item as pinned so it survives persistence.
    pub fn pin(&mut self, key: &str) {
        if let Some(item) = self.items.get_mut(key) {
            item.pinned = true;
        }
    }

    /// Unpin an item.
    pub fn unpin(&mut self, key: &str) {
        if let Some(item) = self.items.get_mut(key) {
            item.pinned = false;
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Drop all non-pinned items. Called before persistence so only durable
    /// context survives restarts.
    pub fn retain_pinned_only(&mut self) {
        self.items.retain(|_, item| item.pinned);
    }

    /// Number of context items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// A context event emitted by an agent via OSC and pending processing by the
/// board.
#[derive(Clone, Debug)]
pub struct ContextEvent {
    pub key: String,
    pub value: String,
}

/// Minimal `SystemTime` serde using seconds since UNIX epoch.
mod system_time_serde {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(time: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        let secs = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs();
        secs.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_get() {
        let mut ctx = WorkspaceContext::default();
        ctx.publish("file".into(), "main.rs".into(), Some(PanelId(1)));

        let item = ctx.get("file").expect("item should exist");
        assert_eq!(item.value, "main.rs");
        assert_eq!(item.source_panel_id, Some(1));
        assert!(!item.pinned);
    }

    #[test]
    fn publish_overwrites_value_preserves_pin() {
        let mut ctx = WorkspaceContext::default();
        ctx.publish("file".into(), "old.rs".into(), None);
        ctx.pin("file");

        ctx.publish("file".into(), "new.rs".into(), None);
        let item = ctx.get("file").expect("item should exist");
        assert_eq!(item.value, "new.rs");
        assert!(item.pinned);
    }

    #[test]
    fn remove_returns_false_for_missing() {
        let mut ctx = WorkspaceContext::default();
        assert!(!ctx.remove("missing"));
    }

    #[test]
    fn retain_pinned_only_drops_unpinned() {
        let mut ctx = WorkspaceContext::default();
        ctx.publish("a".into(), "1".into(), None);
        ctx.publish("b".into(), "2".into(), None);
        ctx.pin("b");
        ctx.retain_pinned_only();

        assert!(ctx.get("a").is_none());
        assert!(ctx.get("b").is_some());
    }

    #[test]
    fn serde_round_trip() {
        let mut ctx = WorkspaceContext::default();
        ctx.publish("key".into(), "val".into(), Some(PanelId(5)));
        ctx.pin("key");

        let yaml = serde_yaml::to_string(&ctx).expect("serialize");
        let restored: WorkspaceContext = serde_yaml::from_str(&yaml).expect("deserialize");
        let item = restored.get("key").expect("item");
        assert_eq!(item.value, "val");
        assert!(item.pinned);
    }

    #[test]
    fn is_empty_and_len() {
        let mut ctx = WorkspaceContext::default();
        assert!(ctx.is_empty());
        assert_eq!(ctx.len(), 0);

        ctx.publish("x".into(), "y".into(), None);
        assert!(!ctx.is_empty());
        assert_eq!(ctx.len(), 1);
    }
}
