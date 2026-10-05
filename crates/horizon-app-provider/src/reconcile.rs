use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use horizon_app_testing::catalog::Device;
use horizon_app_testing::contract::Platform;
use serde_json::Value;
use uuid::Uuid;

use crate::api::{BrowserStack, Decoded, UploadedApp};
use crate::{Error, Result};

const PAGE: usize = 100;
const MAX_PAGES: usize = 64;

struct Budget {
    requests: usize,
    rows: usize,
    bytes: usize,
    deadline: Instant,
}

impl Budget {
    fn new() -> Self {
        Self {
            requests: 64,
            rows: 6400,
            bytes: 8 * 1024 * 1024,
            deadline: Instant::now() + Duration::from_secs(30),
        }
    }

    fn request(&mut self, path: &str, get: &mut impl FnMut(&str, Duration, u64) -> Result<Decoded>) -> Result<Value> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if self.requests == 0 || self.bytes == 0 || remaining.is_zero() {
            return Err(Error::ReconcileIncomplete);
        }
        self.requests -= 1;
        let limit = u64::try_from(self.bytes.min(1024 * 1024)).map_err(|_| Error::ReconcileIncomplete)?;
        let response = get(path, remaining.min(Duration::from_secs(10)), limit)?;
        self.bytes = self
            .bytes
            .checked_sub(response.bytes)
            .ok_or(Error::ReconcileIncomplete)?;
        if Instant::now() >= self.deadline {
            return Err(Error::ReconcileIncomplete);
        }
        Ok(response.value)
    }
}

/// Private provider observation. Never expose provider session IDs or metadata to agent clients.
pub struct Session {
    id: String,
    metadata: Value,
}

impl Session {
    pub fn record_id<T>(&self, record: impl FnOnce(&str) -> T) -> T {
        record(&self.id)
    }

    /// # Errors
    /// Requires positive native-app, physical catalog target and uploaded-app evidence.
    pub fn verify(&self, target: &Device, app: &UploadedApp, operation: Uuid) -> Result<()> {
        let row = &self.metadata;
        let platform = match target.platform {
            Platform::Ios => "ios",
            Platform::Android => "android",
        };
        if row.get("browser") != Some(&Value::Null)
            || text(row, "browser_version")? != "app"
            || !text(row, "os")?.eq_ignore_ascii_case(platform)
            || !text(row, "device")?.eq_ignore_ascii_case(&target.model)
            || text(row, "os_version")? != target.os_version
            || text(row, "build_name")? != build_name(operation)
            || text(row, "name")? != session_name(operation)
            || !app.use_for_driver(|token| row.pointer("/app_details/app_url").and_then(Value::as_str) == Some(token))
        {
            return Err(Error::DeviceUnverified);
        }
        Ok(())
    }

    /// # Errors
    /// Unknown provider statuses remain unresolved; they never authorize capacity reuse.
    pub fn active(&self) -> Result<bool> {
        active(&self.metadata)
    }
}

impl BrowserStack {
    /// # Errors
    /// Discovers only the exact journaled upload intent. Absence is not permission to replay an uncertain upload.
    pub fn find_upload(&self, operation: Uuid) -> Result<Option<UploadedApp>> {
        let name = build_name(operation);
        let rows = self.pages(&mut Budget::new(), &format!("/app-automate/recent_apps/{name}"))?;
        owned_upload(rows, &name)
    }

    /// # Errors
    /// Resolves the exact journaled allocation name; never uses a device name or app token as ownership.
    pub fn find_session(&self, operation: Uuid) -> Result<Option<Session>> {
        let name = build_name(operation);
        let mut budget = Budget::new();
        let builds = self.pages(&mut budget, "/app-automate/builds.json")?;
        let mut matched = None;
        let mut seen_builds = BTreeSet::new();
        let mut seen_sessions = BTreeSet::new();
        for build in builds {
            let row = build.get("automation_build").ok_or(Error::ProviderRejected)?;
            if text(row, "name")? != name {
                continue;
            }
            let id = provider_id(row)?;
            if !seen_builds.insert(id.to_owned()) {
                continue;
            }
            for session in self.sessions(&mut budget, id)? {
                let row = session.get("automation_session").ok_or(Error::ProviderRejected)?;
                if text(row, "build_name")? != name || text(row, "name")? != session_name(operation) {
                    return Err(Error::ReconcileIncomplete);
                }
                let id = provider_id(row)?;
                if seen_sessions.insert(id.to_owned()) {
                    if matched.is_some() {
                        return Err(Error::ReconcileIncomplete);
                    }
                    matched = Some(self.session_bounded(&mut budget, id)?);
                }
            }
        }
        Ok(matched)
    }

    /// # Errors
    /// Fresh account-visible running IDs for overlap accounting with private local reservations.
    /// The quota's running/queued counts remain authoritative when team sessions are not visible.
    pub fn active_session_ids(&self) -> Result<BTreeSet<String>> {
        let mut budget = Budget::new();
        let builds = self.pages(&mut budget, "/app-automate/builds.json?status=running")?;
        let mut ids = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for build in builds {
            let row = build.get("automation_build").ok_or(Error::ProviderRejected)?;
            let id = provider_id(row)?;
            if !seen.insert(id.to_owned()) {
                continue;
            }
            for session in self.sessions(&mut budget, id)? {
                let row = session.get("automation_session").ok_or(Error::ProviderRejected)?;
                let id = provider_id(row)?;
                // Result-marked sessions may already have ended. Only positive running status
                // counts as overlap; missing local IDs remain conservatively reserved.
                active(row)?;
                if text(row, "status")? == "running" {
                    ids.insert(id.to_owned());
                }
            }
        }
        Ok(ids)
    }

    /// # Errors
    /// Host journal ownership must be checked before using this private provider ID.
    pub fn session(&self, id: &str) -> Result<Session> {
        self.session_bounded(&mut Budget::new(), id)
    }

    fn session_bounded(&self, budget: &mut Budget, id: &str) -> Result<Session> {
        check_id(id)?;
        let response = budget.request(
            &format!("/app-automate/sessions/{id}.json"),
            &mut |path, timeout, limit| self.get_measured(path, timeout, limit),
        )?;
        let row = response.get("automation_session").ok_or(Error::ProviderRejected)?;
        if provider_id(row)? != id {
            return Err(Error::ProviderRejected);
        }
        Ok(Session {
            id: id.to_owned(),
            metadata: row.clone(),
        })
    }

    /// # Errors
    /// Deletes an exact journal-owned session and requires an acknowledged `WebDriver` quit.
    /// A lost delete reply remains uncertain even when a later attempt may safely repeat that exact deletion.
    pub fn release_session(&self, session: &Session) -> Result<()> {
        if !self.session(&session.id)?.active()? {
            return Ok(());
        }
        self.driver()?
            .request(
                "DELETE",
                &format!("/session/{}", session.id),
                None,
                Duration::from_secs(30),
            )
            .map_err(|_| Error::ProviderFailed)?;
        Ok(())
    }

    fn sessions(&self, budget: &mut Budget, build: &str) -> Result<Vec<Value>> {
        check_id(build)?;
        self.pages(budget, &format!("/app-automate/builds/{build}/sessions.json"))
    }

    fn pages(&self, budget: &mut Budget, path: &str) -> Result<Vec<Value>> {
        pages(budget, path, |path, timeout, limit| {
            self.get_measured(path, timeout, limit)
        })
    }
}

fn pages(
    budget: &mut Budget,
    path: &str,
    mut get: impl FnMut(&str, Duration, u64) -> Result<Decoded>,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    let separator = if path.contains('?') { '&' } else { '?' };
    for page in 0..MAX_PAGES {
        let response = budget.request(
            &format!("{path}{separator}limit={PAGE}&offset={}", page * PAGE),
            &mut get,
        )?;
        let rows = response.as_array().ok_or(Error::ProviderRejected)?;
        if rows.len() > PAGE {
            return Err(Error::ProviderRejected);
        }
        budget.rows = budget.rows.checked_sub(rows.len()).ok_or(Error::ReconcileIncomplete)?;
        result.extend(rows.iter().cloned());
        if rows.len() < PAGE {
            return Ok(result);
        }
    }
    Err(Error::ReconcileIncomplete)
}

fn owned_upload(rows: Vec<Value>, name: &str) -> Result<Option<UploadedApp>> {
    let mut matched = None;
    let mut ids = BTreeSet::new();
    for row in rows {
        if text(&row, "custom_id")? != name {
            return Err(Error::ReconcileIncomplete);
        }
        let app = UploadedApp::from_response(&row)?;
        if ids.insert(app.use_for_driver(str::to_owned)) {
            if matched.is_some() {
                return Err(Error::ReconcileIncomplete);
            }
            matched = Some(app);
        }
    }
    Ok(matched)
}

fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str> {
    row.get(key).and_then(Value::as_str).ok_or(Error::ProviderRejected)
}

fn check_id(id: &str) -> Result<()> {
    if !(16..=128).contains(&id.len())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(Error::ProviderRejected);
    }
    Ok(())
}

fn provider_id(row: &Value) -> Result<&str> {
    let id = text(row, "hashed_id")?;
    check_id(id)?;
    Ok(id)
}

fn active(row: &Value) -> Result<bool> {
    match text(row, "status")? {
        // passed/failed can be user-marked results while WebDriver still executes.
        "running" | "queued" | "passed" | "failed" | "error" => Ok(true),
        "done" | "timeout" => Ok(false),
        _ => Err(Error::ReconcileIncomplete),
    }
}

fn build_name(operation: Uuid) -> String {
    format!("horizon-native-{operation}")
}

fn session_name(operation: Uuid) -> String {
    format!("native-{operation}")
}

#[cfg(test)]
mod tests;
