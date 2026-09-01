use std::{collections::HashSet, time::Instant};

use x11rb::{
    NONE,
    connection::Connection as _,
    protocol::{
        Event,
        xproto::{Atom, AtomEnum, ConnectionExt as _, GetPropertyReply, Property, Window},
    },
};

use super::{ClipboardSession, ClipboardSnapshot, MAX_SNAPSHOT_BYTES, MAX_TARGETS, StoredTarget, StoredValue};
use crate::InjectError;

impl ClipboardSession {
    pub(super) fn read_snapshot(&self, owner: Window, timestamp: u32) -> Result<ClipboardSnapshot, InjectError> {
        if owner == NONE {
            return Ok(ClipboardSnapshot::default());
        }
        let deadline = Instant::now() + super::SNAPSHOT_TIMEOUT;
        let targets_reply = self.request_selection_target(self.atoms.TARGETS, timestamp, deadline)?;
        if targets_reply.property_type != AtomEnum::ATOM.into() {
            return Err(InjectError::Clipboard("unsupported clipboard target list"));
        }
        let StoredValue::Bytes32(targets) = &targets_reply.value else {
            return Err(InjectError::Clipboard("invalid clipboard targets"));
        };
        if targets.len() > MAX_TARGETS {
            return Err(InjectError::Clipboard("too many clipboard formats to preserve safely"));
        }
        if targets.contains(&self.atoms.X_KDE_PASSWORDMANAGERHINT) {
            return Err(InjectError::Clipboard(
                "sensitive clipboard content cannot be preserved safely",
            ));
        }

        let mut seen = HashSet::with_capacity(targets.len());
        let mut snapshot = ClipboardSnapshot::default();
        for &target in targets {
            if self.is_protocol_target(target) || !seen.insert(target) {
                continue;
            }
            let stored = self.request_selection_target(target, timestamp, deadline)?;
            if stored.value.byte_len() > self.max_property_bytes {
                return Err(InjectError::Clipboard("clipboard item is too large to preserve safely"));
            }
            snapshot.total_bytes = snapshot
                .total_bytes
                .checked_add(stored.value.byte_len())
                .ok_or(InjectError::Clipboard("clipboard is too large to preserve safely"))?;
            if snapshot.total_bytes > MAX_SNAPSHOT_BYTES {
                return Err(InjectError::Clipboard("clipboard is too large to preserve safely"));
            }
            snapshot.targets.push(stored);
        }

        if self.selection_owner(self.atoms.CLIPBOARD)? != owner {
            return Err(InjectError::Clipboard("clipboard changed while preparing speech paste"));
        }
        Ok(snapshot)
    }

    fn request_selection_target(
        &self,
        target: Atom,
        timestamp: u32,
        deadline: Instant,
    ) -> Result<StoredTarget, InjectError> {
        self.conn
            .delete_property(self.window, self.atoms.HORIZON_CLIPBOARD_DATA)
            .map_err(|_| InjectError::Clipboard("failed to prepare clipboard read"))?
            .check()
            .map_err(|_| InjectError::Clipboard("failed to prepare clipboard read"))?;
        self.conn
            .convert_selection(
                self.window,
                self.atoms.CLIPBOARD,
                target,
                self.atoms.HORIZON_CLIPBOARD_DATA,
                timestamp,
            )
            .map_err(|_| InjectError::Clipboard("failed to request existing clipboard data"))?;
        self.conn
            .flush()
            .map_err(|_| InjectError::Clipboard("failed to request existing clipboard data"))?;

        loop {
            let event = self.wait_for_event(deadline)?;
            let Event::SelectionNotify(notify) = event else {
                continue;
            };
            if notify.requestor != self.window || notify.selection != self.atoms.CLIPBOARD || notify.target != target {
                continue;
            }
            if notify.property == NONE {
                return Err(InjectError::Clipboard(
                    "existing clipboard format could not be preserved",
                ));
            }
            let reply = self.read_property(true)?;
            if reply.type_ == self.atoms.INCR {
                return self.read_incremental_target(target, deadline);
            }
            return StoredTarget::from_reply(target, &reply);
        }
    }

    fn read_incremental_target(&self, target: Atom, deadline: Instant) -> Result<StoredTarget, InjectError> {
        let mut property_type = None;
        let mut format = None;
        let mut value = StoredValue::Bytes8(Vec::new());
        loop {
            let event = self.wait_for_event(deadline)?;
            let Event::PropertyNotify(notify) = event else {
                continue;
            };
            if notify.window != self.window
                || notify.atom != self.atoms.HORIZON_CLIPBOARD_DATA
                || notify.state != Property::NEW_VALUE
            {
                continue;
            }
            let reply = self.read_property(true)?;
            if reply.value_len == 0 {
                return Ok(StoredTarget {
                    target,
                    property_type: property_type.ok_or(InjectError::Clipboard(
                        "existing clipboard sent an empty incremental format",
                    ))?,
                    value,
                });
            }
            if let (Some(existing_type), Some(existing_format)) = (property_type, format)
                && (existing_type != reply.type_ || existing_format != reply.format)
            {
                return Err(InjectError::Clipboard(
                    "existing clipboard changed format while being read",
                ));
            }
            property_type = Some(reply.type_);
            format = Some(reply.format);
            value.extend_reply(&reply)?;
            if value.byte_len() > MAX_SNAPSHOT_BYTES {
                return Err(InjectError::Clipboard("clipboard is too large to preserve safely"));
            }
        }
    }

    fn read_property(&self, delete: bool) -> Result<GetPropertyReply, InjectError> {
        let reply = self
            .conn
            .get_property(
                delete,
                self.window,
                self.atoms.HORIZON_CLIPBOARD_DATA,
                AtomEnum::ANY,
                0,
                u32::try_from(MAX_SNAPSHOT_BYTES / 4).unwrap_or(u32::MAX),
            )
            .map_err(|_| InjectError::Clipboard("failed to read existing clipboard data"))?
            .reply()
            .map_err(|_| InjectError::Clipboard("failed to read existing clipboard data"))?;
        if reply.bytes_after != 0 {
            return Err(InjectError::Clipboard("clipboard item is too large to read safely"));
        }
        Ok(reply)
    }
}
