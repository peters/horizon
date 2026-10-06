//! One bounded matrix run over the same owned actor used by interactive MCP tools.
mod model;
use crate::{Error, Result, actor::Actor};
use horizon_app_testing::{
    catalog::ResolvedDevice,
    contract::Platform,
    recipe::{Action, Recipe},
};
pub use model::*;
use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

pub struct Control {
    cancelled: AtomicBool,
    deadline: Instant,
}
impl Control {
    /// # Errors
    /// The complete run, including builds and uploads, shares this original lifetime.
    pub fn new(lifetime: Duration) -> Result<Self> {
        if lifetime.is_zero() || lifetime > Duration::from_mins(30) {
            return Err(horizon_app_runtime::Error::OperationInvalid.into());
        }
        Ok(Self {
            cancelled: AtomicBool::new(false),
            deadline: Instant::now() + lifetime,
        })
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub(crate) fn remaining(&self) -> Result<Duration> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(horizon_app_runtime::Error::OperationExpired.into());
        }
        Ok(remaining)
    }
}

trait Runtime: Sync {
    fn build(&self, platform: Platform, control: &Control) -> Result<()>;
    fn upload(&self, platform: Platform, deadline: Instant) -> Result<Uuid>;
    fn create(&self, index: usize, app: Uuid, deadline: Instant) -> Result<Uuid>;
    fn act(&self, session: Uuid, action: &Action) -> Result<Option<Uuid>>;
    fn screenshot(&self, session: Uuid) -> Result<Vec<u8>>;
    fn session_link(&self, _session: Uuid, _timeout: Duration) -> Result<String> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
    fn media(&self, _session: Uuid, _kind: horizon_app_provider::media::Kind, _timeout: Duration) -> Result<Vec<u8>> {
        Err(horizon_app_provider::Error::MediaUnavailable.into())
    }
    fn close(&self, session: Uuid) -> Result<()>;
    fn release(&self, app: Uuid) -> Result<()>;
}
impl Runtime for Actor {
    fn build(&self, platform: Platform, control: &Control) -> Result<()> {
        let argv = self
            .contract()
            .apps
            .get(&platform)
            .ok_or(Error::Unavailable)?
            .build
            .clone();
        command(self, argv, control)
    }
    fn upload(&self, platform: Platform, deadline: Instant) -> Result<Uuid> {
        Ok(self.upload_until(platform, deadline)?.id)
    }
    fn create(&self, index: usize, app: Uuid, deadline: Instant) -> Result<Uuid> {
        Ok(self.create_until(index, app, deadline)?.id)
    }
    fn act(&self, session: Uuid, action: &Action) -> Result<Option<Uuid>> {
        if matches!(action, Action::Reset {}) {
            return Ok(Some(self.reset_for_run(session)?.id));
        }
        Actor::act(self, session, action)?;
        Ok(None)
    }
    fn screenshot(&self, session: Uuid) -> Result<Vec<u8>> {
        Actor::screenshot(self, session)
    }
    fn session_link(&self, session: Uuid, timeout: Duration) -> Result<String> {
        Actor::session_link(self, session, timeout)
    }
    fn media(&self, session: Uuid, kind: horizon_app_provider::media::Kind, timeout: Duration) -> Result<Vec<u8>> {
        self.media_with_timeout(session, kind, timeout)
    }
    fn close(&self, session: Uuid) -> Result<()> {
        Actor::close(self, session)
    }
    fn release(&self, app: Uuid) -> Result<()> {
        self.release_upload(app)
    }
}

fn allocate(runtime: &impl Runtime, control: &Control, index: usize, app: Uuid) -> Result<Uuid> {
    loop {
        control.remaining()?;
        match runtime.create(index, app, control.deadline) {
            Err(Error::AdmissionDeferred) => {
                std::thread::sleep(control.remaining()?.min(Duration::from_millis(50)));
            }
            result => return result,
        }
    }
}

fn command(actor: &Actor, argv: Vec<String>, control: &Control) -> Result<()> {
    let remaining = control.remaining()?;
    if remaining < Duration::from_secs(1) {
        return Err(horizon_app_runtime::Error::OperationExpired.into());
    }
    let operation = actor.local.process(
        argv,
        horizon_app_process::Kind::Build,
        Duration::from_secs(1),
        remaining,
    )?;
    let outcome = (|| loop {
        let remaining = control.remaining()?;
        match operation.next(remaining.min(Duration::from_millis(250))) {
            Ok(horizon_app_process::Event::Complete { success: true }) => return Ok(()),
            Ok(horizon_app_process::Event::Complete { success: false } | horizon_app_process::Event::Failed { .. }) => {
                return Err(horizon_app_process::Error::Failed.into());
            }
            Err(Error::Process(horizon_app_process::Error::Timeout)) => (),
            Err(error) => return Err(error),
            _ => (),
        }
    })();
    operation.close()?;
    outcome
}

/// # Errors
/// Validate every recipe before building or allocating. Each device owns a distinct service lane.
pub fn run(
    actor: &Actor,
    control: &Control,
    capture: impl Fn(Uuid, CaptureKind, &[u8]) -> Result<Evidence> + Sync,
    progress: impl Fn(Progress) -> Result<()> + Sync,
) -> Result<Report> {
    let _run = actor.begin_run()?;
    let contract = actor.contract();
    let mut bytes = 0_usize;
    let recipes = contract
        .recipes
        .iter()
        .map(|path| {
            let text = actor.project_text(path)?;
            bytes = bytes.checked_add(text.len()).ok_or(Error::Unavailable)?;
            if bytes > 8 * 1024 * 1024 {
                return Err(Error::Unavailable);
            }
            Ok(Recipe::from_markdown(&text)?)
        })
        .collect::<Result<Vec<_>>>()?;
    if recipes.iter().map(|recipe| recipe.steps.len()).sum::<usize>() > 1024 {
        return Err(Error::Unavailable);
    }
    if recipes
        .iter()
        .map(|recipe| recipe.steps.len())
        .sum::<usize>()
        .saturating_mul(contract.matrix.len())
        > 1024
    {
        return Err(Error::Unavailable);
    }
    let parallel = actor
        .available_parallel(control.remaining()?.min(Duration::from_secs(30)))?
        .min(contract.max_parallel)
        .min(2);
    if parallel == 0 {
        return Err(horizon_app_runtime::Error::CapacityUnavailable.into());
    }
    let _evidence = actor.retain_run_evidence()?;
    if let Some(reset) = &contract.reset {
        command(actor, reset.clone(), control)?;
    }
    Plan {
        targets: actor.targets(),
        recipes: &recipes,
        parallel,
        screenshots: contract.evidence.screenshots,
        video: contract.evidence.video,
        logs_on_failure: contract.evidence.logs_on_failure,
    }
    .execute(actor, control, capture, progress)
}

struct Plan<'a> {
    targets: Vec<ResolvedDevice>,
    recipes: &'a [Recipe],
    parallel: usize,
    screenshots: bool,
    video: bool,
    logs_on_failure: bool,
}
impl Plan<'_> {
    fn execute(
        &self,
        runtime: &impl Runtime,
        control: &Control,
        capture: impl Fn(Uuid, CaptureKind, &[u8]) -> Result<Evidence> + Sync,
        progress: impl Fn(Progress) -> Result<()> + Sync,
    ) -> Result<Report> {
        let targets = &self.targets;
        let parallel = self.parallel;
        let id = Uuid::new_v4();
        let mut builds = Vec::new();
        let mut assets = Assets {
            runtime,
            apps: BTreeMap::new(),
        };
        // Platform order is deliberate: iOS first, then Android; one immutable upload per platform.
        for platform in [Platform::Ios, Platform::Android]
            .into_iter()
            .filter(|platform| targets.iter().any(|target| target.device.platform == *platform))
        {
            progress(Progress {
                run: id,
                matrix_index: None,
                phase: "build",
                recipe: None,
                step: None,
                session: None,
                view: None,
            })?;
            let start = Instant::now();
            let result = control
                .remaining()
                .and_then(|_| runtime.build(platform, control))
                .and_then(|()| {
                    control.remaining()?;
                    runtime.upload(platform, control.deadline)
                });
            let error = result.as_ref().err().map(ToString::to_string);
            if let Ok(app) = result {
                assets.apps.insert(platform, app);
            }
            builds.push(Build {
                platform,
                duration_millis: millis(start),
                error,
            });
        }
        let next = AtomicUsize::new(0);
        let output = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for _ in 0..parallel {
                workers.push(scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::AcqRel);
                        let Some(target) = targets.get(index) else {
                            return Ok::<(), Error>(());
                        };
                        let result = self.device(
                            runtime,
                            control,
                            target,
                            &assets.apps,
                            &Observation {
                                run: id,
                                capture: &capture,
                                progress: &progress,
                            },
                        );
                        output.lock().map_err(|_| Error::Unavailable)?.push(result);
                    }
                }));
            }
            let mut outcome = Ok(());
            for worker in workers {
                if let Err(error) = worker.join().unwrap_or(Err(Error::Unavailable)) {
                    outcome = Err(error);
                }
            }
            outcome
        })?;
        let upload_cleanup_errors = assets
            .apps
            .values()
            .filter_map(|app| runtime.release(*app).err().map(|error| error.to_string()))
            .collect();
        assets.apps.clear();
        let mut devices = output.into_inner().map_err(|_| Error::Unavailable)?;
        devices.sort_by_key(|device| device.matrix_index);
        Ok(Report {
            id,
            parallel,
            builds,
            devices,
            cancelled: control.cancelled.load(Ordering::Acquire),
            upload_cleanup_errors,
        })
    }

    fn device(
        &self,
        runtime: &impl Runtime,
        control: &Control,
        target: &ResolvedDevice,
        apps: &BTreeMap<Platform, Uuid>,
        observation: &Observation<'_>,
    ) -> DeviceResult {
        let run = observation.run;
        let progress = observation.progress;
        let mut report = DeviceResult {
            matrix_index: target.matrix_index,
            target: target.device.clone(),
            session: None,
            allocations: Vec::new(),
            error: None,
            cleanup_confirmed: true,
            steps: Vec::new(),
            media: Vec::new(),
            provider_session_link: None,
            provider_link_error: None,
        };
        let mut attempted = false;
        let created = control.remaining().and_then(|_| {
            let app = apps.get(&target.device.platform).ok_or(Error::ArtifactUnknown)?;
            attempted = true;
            allocate(runtime, control, target.matrix_index, *app)
        });
        match created {
            Ok(session) => {
                report.session = Some(session);
                report.allocations.push(session);
            }
            Err(error) => {
                report.error = Some(error.to_string());
                report.cleanup_confirmed = !attempted;
            }
        }
        let mut owned = OwnedSession {
            runtime,
            session: report.session,
        };
        if let Some(session) = report.session
            && let Err(error) = progress(Progress {
                run,
                matrix_index: Some(target.matrix_index),
                phase: "session_created",
                recipe: None,
                step: None,
                session: Some(session),
                view: None,
            })
        {
            report.error = Some(error.to_string());
            if let Some(session) = owned.session.take() {
                report.cleanup_confirmed = runtime.close(session).is_ok();
            }
            self.media(runtime, control, &mut report, observation);
            return report;
        }
        self.steps(runtime, control, &mut report, &mut owned, observation);
        if let Some(session) = owned.session.take()
            && let Err(error) = runtime.close(session)
        {
            report.cleanup_confirmed = false;
            report.error = Some(error.to_string());
        }
        self.media(runtime, control, &mut report, observation);
        report
    }
    fn steps<R: Runtime>(
        &self,
        runtime: &R,
        control: &Control,
        report: &mut DeviceResult,
        owned: &mut OwnedSession<'_, R>,
        observation: &Observation<'_>,
    ) {
        let run = observation.run;
        let progress = observation.progress;
        let capture = observation.capture;
        let platform = report.target.platform;
        for recipe in self.recipes.iter().filter(|recipe| {
            recipe
                .platforms
                .as_ref()
                .is_none_or(|platforms| platforms.contains(&platform))
        }) {
            for step in &recipe.steps {
                let start = Instant::now();
                let mut screenshot = None;
                let outcome = (|| {
                    control.remaining()?;
                    let session = report.session.ok_or(Error::SessionUnknown)?;
                    progress(Progress {
                        run,
                        matrix_index: Some(report.matrix_index),
                        phase: "step",
                        recipe: Some(recipe.id.clone()),
                        step: Some(step.id.clone()),
                        session: Some(session),
                        view: None,
                    })?;
                    control.remaining()?;
                    // Screenshot steps use the retained capture below, avoiding a discarded second request.
                    let action = if matches!(step.action, Action::Screenshot {}) {
                        Ok(None)
                    } else {
                        runtime.act(session, &step.action)
                    };
                    if matches!(step.action, Action::Reset {}) && action.is_err() {
                        // A replacement may have reached the provider without a returned handle.
                        report.cleanup_confirmed = false;
                    }
                    let session = match action? {
                        Some(replacement) => {
                            // Retain cleanup ownership before any fallible capture or callback.
                            owned.session = Some(replacement);
                            report.session = Some(replacement);
                            report.allocations.push(replacement);
                            replacement
                        }
                        None => session,
                    };
                    if self.screenshots || matches!(step.action, Action::Screenshot {}) {
                        screenshot = Some(capture(
                            session,
                            CaptureKind::Screenshot,
                            &runtime.screenshot(session)?,
                        )?);
                    }
                    Ok::<(), Error>(())
                })();
                report.steps.push(Step {
                    recipe: recipe.id.clone(),
                    step: step.id.clone(),
                    passed: outcome.is_ok(),
                    duration_millis: millis(start),
                    error: outcome.err().map(|error| error.to_string()),
                    screenshot,
                });
            }
        }
    }
    fn media(
        &self,
        runtime: &impl Runtime,
        control: &Control,
        report: &mut DeviceResult,
        observation: &Observation<'_>,
    ) {
        use horizon_app_provider::media::Kind;
        let Some(session) = report.session else {
            return;
        };
        let link = control
            .remaining()
            .and_then(|timeout| runtime.session_link(session, timeout));
        report.provider_link_error = link.as_ref().err().map(ToString::to_string);
        report.provider_session_link = link.ok();
        let mut kinds = Vec::new();
        if self.video {
            kinds.push(Kind::Video);
        }
        if self.logs_on_failure && (report.error.is_some() || report.steps.iter().any(|step| !step.passed)) {
            kinds.extend([Kind::Device, Kind::Crash, Kind::Appium, Kind::Network]);
        }
        for session in report.allocations.clone() {
            for kind in kinds.iter().copied() {
                let result = control.remaining().and_then(|remaining| {
                    let bytes = finalized_media(runtime, control, session, kind, remaining)?;
                    (observation.capture)(session, CaptureKind::Provider(kind), &bytes)
                });
                report.media.push(Media {
                    session,
                    kind,
                    error: result.as_ref().err().map(ToString::to_string),
                    evidence: result.ok(),
                });
            }
        }
    }
}
fn finalized_media(
    runtime: &impl Runtime,
    control: &Control,
    session: Uuid,
    kind: horizon_app_provider::media::Kind,
    remaining: Duration,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + remaining.min(Duration::from_secs(30));
    loop {
        control.remaining()?;
        let budget = deadline.saturating_duration_since(Instant::now());
        if budget.is_zero() {
            return Err(horizon_app_provider::Error::MediaUnavailable.into());
        }
        let result = runtime.media(session, kind, budget.min(Duration::from_secs(10)));
        if !matches!(kind, horizon_app_provider::media::Kind::Video)
            || !matches!(
                result,
                Err(Error::Provider(horizon_app_provider::Error::MediaUnavailable))
            )
        {
            return result;
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(2)),
        );
    }
}

struct OwnedSession<'a, R: Runtime> {
    runtime: &'a R,
    session: Option<Uuid>,
}
impl<R: Runtime> Drop for OwnedSession<'_, R> {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = self.runtime.close(session);
        }
    }
}

type Capture<'a> = dyn Fn(Uuid, CaptureKind, &[u8]) -> Result<Evidence> + Sync + 'a;
struct Observation<'a> {
    run: Uuid,
    capture: &'a Capture<'a>,
    progress: &'a (dyn Fn(Progress) -> Result<()> + Sync),
}
struct Assets<'a, R: Runtime> {
    runtime: &'a R,
    apps: BTreeMap<Platform, Uuid>,
}
impl<R: Runtime> Drop for Assets<'_, R> {
    fn drop(&mut self) {
        for app in self.apps.values() {
            let _ = self.runtime.release(*app);
        }
    }
}

fn millis(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
