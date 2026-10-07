//! Bounded native operation receipts exclude targets, entered text and provider identifiers.
use crate::{Error, Result};
use serde::Serialize;
use std::{collections::VecDeque, sync::Mutex, time::Instant};
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct Entry {
    pub sequence: u64,
    pub operation: Uuid,
    pub session: Option<Uuid>,
    pub action: &'static str,
    pub status: Status,
    pub duration_millis: Option<u64>,
    pub error_code: Option<String>,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Accepted,
    Completed,
    Failed,
}
#[derive(Serialize)]
pub struct Page {
    pub stream: Uuid,
    pub entries: Vec<Entry>,
    pub next_sequence: u64,
    pub latest_sequence: u64,
    pub truncated: bool,
}
#[derive(Default)]
struct State {
    sequence: u64,
    entries: VecDeque<Entry>,
}
pub struct Audit {
    stream: Uuid,
    state: Mutex<State>,
}
impl Default for Audit {
    fn default() -> Self {
        Self {
            stream: Uuid::new_v4(),
            state: Mutex::new(State::default()),
        }
    }
}
impl Audit {
    fn record(&self, mut entry: Entry) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| Error::AuditUnavailable)?;
        state.sequence = state.sequence.checked_add(1).ok_or(Error::AuditUnavailable)?;
        entry.sequence = state.sequence;
        if state.entries.len() == 256 {
            state.entries.pop_front();
        }
        state.entries.push_back(entry);
        Ok(())
    }
    pub(crate) fn execute<T>(
        &self,
        session: Option<Uuid>,
        action: &'static str,
        call: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let mut entry = Entry {
            sequence: 0,
            operation: Uuid::new_v4(),
            session,
            action,
            status: Status::Accepted,
            duration_millis: None,
            error_code: None,
        };
        self.record(entry.clone())?;
        let start = Instant::now();
        let result = call();
        entry.status = if result.is_ok() {
            Status::Completed
        } else {
            Status::Failed
        };
        entry.duration_millis = Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));
        entry.error_code = result
            .as_ref()
            .err()
            .map(|error| error.to_string().split(':').next().unwrap_or("app_failed").to_owned());
        self.record(entry)?;
        result
    }
    /// # Errors
    /// Page size is 1..256. Compare the returned stream UUID before resuming a saved cursor.
    pub fn page(&self, after_sequence: u64, limit: usize) -> Result<Page> {
        let state = self.state.lock().map_err(|_| Error::AuditUnavailable)?;
        if after_sequence > state.sequence || !(1..=256).contains(&limit) {
            return Err(horizon_app_testing::Error::RecipeInvalid.into());
        }
        let entries = state
            .entries
            .iter()
            .filter(|entry| entry.sequence > after_sequence)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok(Page {
            stream: self.stream,
            next_sequence: entries.last().map_or(after_sequence, |entry| entry.sequence),
            latest_sequence: state.sequence,
            truncated: state
                .entries
                .front()
                .is_some_and(|entry| after_sequence.saturating_add(1) < entry.sequence),
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_receipts_are_bounded_and_correlate_without_input_content() {
        let audit = Audit::default();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..40 {
                        audit.execute(None, "type", || Ok(())).unwrap();
                    }
                });
            }
        });
        let page = audit.page(0, 256).unwrap();
        assert_eq!(page.entries.len(), 256);
        assert_eq!(page.latest_sequence, 320);
        assert!(page.truncated);
        assert!(page.entries.windows(2).all(|pair| pair[0].sequence < pair[1].sequence));
        let failed = audit.execute::<()>(None, "type", || Err(Error::SessionUnknown));
        assert!(failed.is_err());
        let final_page = audit.page(320, 2).unwrap();
        assert_eq!(final_page.entries[0].operation, final_page.entries[1].operation);
        assert_eq!(final_page.entries[1].error_code.as_deref(), Some("app_session_unknown"));
        let encoded = serde_json::to_string(&final_page).unwrap();
        assert!(!encoded.contains("target"));
        assert!(!encoded.contains("text"));
        assert!(audit.page(323, 1).is_err());
    }
}
