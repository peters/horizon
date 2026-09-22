use super::super::progress::{Progress, Unit};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct BuildSteps(BTreeMap<u32, bool>);

impl BuildSteps {
    pub fn observe(&mut self, line: &str) -> Option<Progress> {
        let (id, message) = line.strip_prefix('#')?.split_once(' ')?;
        let id = id.parse().ok()?;
        let complete = self.0.entry(id).or_default();
        *complete |= message.starts_with("DONE") || message.starts_with("CACHED");
        let completed = self.0.values().filter(|done| **done).count() as u64;
        Some(Progress {
            detail: format!("Building image · {completed}/{} reported steps complete", self.0.len()),
            completed,
            unit: Unit::Steps,
            ..Progress::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_discovery_and_cached_steps_do_not_claim_a_final_total() {
        let mut steps = BuildSteps::default();
        steps.observe("#1 [internal] load Dockerfile");
        steps.observe("#1 DONE 0.1s");
        steps.observe("#2 CACHED");
        let progress = steps.observe("#3 [stage 1/2] RUN compile").unwrap();
        assert_eq!(progress.completed, 2);
        assert_eq!(progress.total, None);
        assert!(progress.detail.contains("2/3"));
    }
}
