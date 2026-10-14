use super::{ApiCredentialKind, Error, Result, Store};
use crate::tailnet::valid_id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Records {
    #[serde(default)]
    pub(super) bindings: BTreeMap<String, Binding>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Binding {
    Active { slot: String, kind: ApiCredentialKind },
    Disabled,
}

impl Records {
    pub(super) fn validate(&self, store: &Store) -> Result<()> {
        if serde_json::to_vec_pretty(self).map_err(|_| Error::Storage)?.len() > 64 * 1024 {
            return Err(Error::Storage);
        }
        let ids: BTreeSet<_> = store.load()?.tailnets.into_iter().map(|network| network.id).collect();
        let mut slots = BTreeSet::new();
        for (id, binding) in &self.bindings {
            if !valid_id(id) {
                return Err(Error::Storage);
            }
            if let Binding::Active { slot, .. } = binding
                && (!valid_id(slot) || ids.contains(slot) || self.bindings.contains_key(slot) || !slots.insert(slot))
            {
                return Err(Error::Storage);
            }
        }
        Ok(())
    }

    pub(super) fn slots(&self) -> BTreeSet<String> {
        self.bindings
            .values()
            .filter_map(|binding| match binding {
                Binding::Active { slot, .. } => Some(slot.clone()),
                Binding::Disabled => None,
            })
            .collect()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub(super) before: Records,
    pub(super) after: Records,
    pub(super) created: Option<String>,
    pub(super) retired: BTreeSet<String>,
}

impl Pending {
    pub(super) fn validate(&self, store: &Store) -> Result<()> {
        self.before.validate(store)?;
        self.after.validate(store)?;
        let ids: BTreeSet<_> = self.before.bindings.keys().chain(self.after.bindings.keys()).collect();
        let changed: Vec<_> = ids
            .into_iter()
            .filter(|id| self.before.bindings.get(*id) != self.after.bindings.get(*id))
            .collect();
        let [id] = changed.as_slice() else {
            return Err(Error::Storage);
        };
        let before = self.before.slots();
        let after = self.after.slots();
        let created: BTreeSet<_> = self.created.iter().cloned().collect();
        let mut retired: BTreeSet<_> = before.difference(&after).cloned().collect();
        // The original local-ID account may exist without any sidecar metadata.
        retired.insert((*id).clone());
        if created != after.difference(&before).cloned().collect() || retired != self.retired {
            return Err(Error::Storage);
        }
        Ok(())
    }
}
