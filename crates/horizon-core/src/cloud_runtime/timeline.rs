//! Where the last deployment or reconnection spent its time, kept with the cloud.
use super::{Event, Stage};
use serde::{Deserialize, Serialize};
use std::{
    sync::Mutex,
    time::{Duration, SystemTime},
};

// Progress activities that mark phase boundaries. Their emitters use these constants,
// so a relabelled activity cannot silently stop splitting the timeline.
pub(crate) const AWAITING_ENDPOINT: &str = "Waiting for the provider to publish an SSH endpoint";
pub(crate) const AWAITING_SERVICES: &str = "Waiting for SSH and worker services";
pub(crate) const UPLOADING_SOURCE: &str = "Uploading source";
pub(crate) const IMPORTING_OBJECTS: &str = "Git object import";
pub(crate) const IMPORTING_DEPENDENCIES: &str = "Source dependency import";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepare,
    Build,
    Push,
    Provision,
    ProviderStart,
    WorkerStart,
    Readiness,
    SourceUpload,
    SourceImport,
    Sessions,
}

impl Phase {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prepare => "Repository checks",
            Self::Build => "Image build",
            Self::Push => "Image push",
            Self::Provision => "Provider request",
            Self::ProviderStart => "Image download",
            Self::WorkerStart => "Worker boot",
            Self::Readiness => "Readiness checks",
            Self::SourceUpload => "Source upload",
            Self::SourceImport => "Source import",
            Self::Sessions => "Sessions",
        }
    }

    /// What the phase covers, for a hover explanation.
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::Prepare => {
                "Checking the committed tree, packing its history and hashing Git LFS files on this computer"
            }
            Self::Build => "Building the worker image on this computer",
            Self::Push => "Pushing the image and checking the registry and the image contract",
            Self::Provision => "Checking provider capacity and requesting storage and the worker",
            Self::ProviderStart => {
                "From the worker request until its container started: provider scheduling and the image download. \
                 Includes worker boot when the image does not report its start time."
            }
            Self::WorkerStart => "Container boot and waiting for the provider to publish the SSH endpoint",
            Self::Readiness => "Checking SSH and the worker's services until they were ready",
            Self::SourceUpload => "Uploading the committed source and Git LFS files to the worker",
            Self::SourceImport => "Importing Git objects and Git LFS files on the worker",
            Self::Sessions => "Configuring credentials and restoring agent sessions",
        }
    }

    const fn of(stage: Stage) -> Option<Self> {
        match stage {
            Stage::Validate => Some(Self::Prepare),
            Stage::Build | Stage::Replace => Some(Self::Build),
            Stage::Push => Some(Self::Push),
            Stage::Provision => Some(Self::Provision),
            Stage::Readiness => Some(Self::ProviderStart),
            Stage::Worktrees => Some(Self::SourceUpload),
            Stage::Sessions => Some(Self::Sessions),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Span {
    pub phase: Phase,
    pub millis: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Timeline {
    /// The worker already existed, so this attempt reconnected or resumed it.
    #[serde(default)]
    pub reconnected: bool,
    pub spans: Vec<Span>,
}

impl Timeline {
    #[must_use]
    pub fn total(&self) -> Duration {
        Duration::from_millis(self.spans.iter().map(|span| span.millis).sum())
    }

    /// Time per phase, largest first; repeated phases such as two imports are combined.
    #[must_use]
    pub fn phases(&self) -> Vec<(Phase, Duration)> {
        let mut phases: Vec<(Phase, Duration)> = Vec::new();
        for span in &self.spans {
            let duration = Duration::from_millis(span.millis);
            match phases.iter_mut().find(|(phase, _)| *phase == span.phase) {
                Some((_, total)) => *total += duration,
                None => phases.push((span.phase, duration)),
            }
        }
        phases.sort_by_key(|(_, spent)| std::cmp::Reverse(*spent));
        phases
    }
}

#[derive(Default)]
pub(crate) struct Recorder(Mutex<Marks>);

#[derive(Default)]
struct Marks {
    entered: Vec<(Phase, SystemTime)>,
    /// First probe with a published SSH endpoint.
    endpoint: Option<SystemTime>,
}

impl Recorder {
    pub fn observe(&self, event: &Event) {
        self.observe_at(event, SystemTime::now());
    }

    fn observe_at(&self, event: &Event, at: SystemTime) {
        let Ok(mut marks) = self.0.lock() else {
            return;
        };
        let phase = match event {
            Event::Stage(stage, _) => Phase::of(*stage),
            Event::Progress(progress) => match progress.detail.as_str() {
                AWAITING_SERVICES => {
                    marks.endpoint.get_or_insert(at);
                    None
                }
                UPLOADING_SOURCE => Some(Phase::SourceUpload),
                IMPORTING_OBJECTS | IMPORTING_DEPENDENCIES => Some(Phase::SourceImport),
                _ => None,
            },
            _ => None,
        };
        if let Some(phase) = phase
            && marks.entered.last().is_none_or(|(last, _)| *last != phase)
        {
            marks.entered.push((phase, at));
        }
    }

    /// Closes the attempt at `end`. When the worker reported its container start, the
    /// provider phase ends there, so the image download is separated from boot.
    pub fn finish(&self, reconnected: bool, container_started: Option<SystemTime>, end: SystemTime) -> Timeline {
        let Ok(marks) = self.0.lock() else {
            return Timeline::default();
        };
        let mut spans = Vec::new();
        for (index, (phase, start)) in marks.entered.iter().enumerate() {
            let stop = marks.entered.get(index + 1).map_or(end, |(_, next)| *next);
            if *phase == Phase::ProviderStart {
                let clamp = |at: SystemTime| at.clamp(*start, stop.max(*start));
                let booted = container_started.map_or_else(|| clamp(marks.endpoint.unwrap_or(stop)), clamp);
                let reachable = clamp(marks.endpoint.unwrap_or(stop)).max(booted);
                push(&mut spans, Phase::ProviderStart, *start, booted);
                if container_started.is_some() {
                    push(&mut spans, Phase::WorkerStart, booted, reachable);
                }
                push(&mut spans, Phase::Readiness, reachable, stop);
            } else {
                push(&mut spans, *phase, *start, stop);
            }
        }
        Timeline { reconnected, spans }
    }
}

fn push(spans: &mut Vec<Span>, phase: Phase, start: SystemTime, stop: SystemTime) {
    let millis = stop.duration_since(start).unwrap_or_default().as_millis();
    spans.push(Span {
        phase,
        millis: u64::try_from(millis).unwrap_or(u64::MAX),
    });
}

#[cfg(test)]
mod tests;
