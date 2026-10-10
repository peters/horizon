//! The one next action a failure offers: the fix Horizon knows for its cause, else the
//! retry that the header offers too.
use super::super::lifecycle::Action;
use super::status::{Primary, Status};
use horizon_core::cloud_runtime::diagnosis::Remedy;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    /// Retries as the header's own button does.
    Retry(Primary),
    /// Opens Cloud settings on the cloud's image repository, where a login fixes a
    /// registry refusal that a retry would only repeat.
    ContainerRegistry,
}

impl Next {
    pub(super) fn of(status: &Status) -> Option<Self> {
        let failure = status.failure.as_ref()?;
        if failure.remedy() == Some(Remedy::RegistryLogin) {
            return Some(Self::ContainerRegistry);
        }
        status
            .primary
            .filter(|primary| primary.retries().is_some())
            .map(Self::Retry)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Retry(primary) => primary.label(),
            Self::ContainerRegistry => "Open Container registry",
        }
    }

    pub(super) fn action(self) -> Option<Action> {
        match self {
            Self::Retry(primary) => primary.retries(),
            Self::ContainerRegistry => Some(Action::ContainerRegistry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{LogLine, Runtime, Stage};
    use super::super::status::{Occupancy, of};
    use super::*;
    use crate::test_egui::DiscardTextures;
    use std::time::{Instant, SystemTime};

    fn failed(stage: Stage, lines: &[&str], error: &str) -> Runtime {
        let mut runtime = Runtime {
            stage: Some(stage),
            error: Some(error.into()),
            ..Runtime::default()
        };
        runtime.progress.stage(stage, Instant::now());
        runtime.progress.finish(Instant::now());
        runtime
            .logs
            .extend(lines.iter().map(|line| LogLine::new((*line).into(), Some(stage), None)));
        runtime
    }

    fn texts(runtime: &Runtime) -> Vec<String> {
        let status = of(runtime, Occupancy::default(), SystemTime::now());
        egui::Context::default()
            .run_ui(egui::RawInput::default(), |ui| {
                super::super::steps::vertical(ui, runtime, &status);
            })
            .discard_textures()
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_refused_push_offers_the_container_registry_instead_of_a_retry() {
        let runtime = failed(
            Stage::Push,
            &[
                "docker push registry.example/worker:9f3c2a1",
                "error from registry: unauthenticated: User cannot be authenticated with the token provided.",
            ],
            "Uploading image failed; inspect deployment output",
        );
        let status = of(&runtime, Occupancy::default(), SystemTime::now());
        let next = Next::of(&status).unwrap();
        assert_eq!(next, Next::ContainerRegistry);
        assert_eq!(next.action(), Some(Action::ContainerRegistry));
        assert_eq!(status.primary, Some(Primary::Retry), "the header still retries");
        let shown = texts(&runtime);
        assert!(shown.iter().any(|text| text == "Open Container registry"), "{shown:?}");
        assert!(
            !shown.iter().any(|text| text == "Retry deploy"),
            "one next action: {shown:?}"
        );
    }

    #[test]
    fn a_failure_without_a_known_fix_is_retried() {
        let runtime = failed(
            Stage::Build,
            &["write /var/lib/docker: no space left on device"],
            "Building image failed; inspect deployment output",
        );
        let status = of(&runtime, Occupancy::default(), SystemTime::now());
        let next = Next::of(&status).unwrap();
        assert_eq!(next, Next::Retry(Primary::Retry));
        assert_eq!(next.action(), Some(Action::Deploy));
        assert_eq!(next.label(), "Retry deploy");
        assert!(texts(&runtime).iter().any(|text| text == "Retry deploy"));
        let ready = of(&Runtime::default(), Occupancy::default(), SystemTime::now());
        assert_eq!(Next::of(&ready), None, "no failure, no next action");
    }
}
