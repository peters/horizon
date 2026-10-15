//! Companion requests of Upkeep loops. An Upkeep agent runs on its worker, not in a Horizon
//! panel, so its Ensure Ready and Stop come through its cloud's exchange instead of the
//! browser MCP queue, always with its own cloud as the source. They then run as an agent
//! panel's do: only a companion the owner checked on the card starts, and the operation runs
//! on the companion's card. Once no run is active, or the Upkeep cloud stays not ready,
//! Horizon stops each companion that a run of that Upkeep started.
use super::{HorizonApp, Message, Submitted, submit};
use horizon_core::{
    browser::manifest::{
        self,
        provider_usage::{CompanionAction, CompanionRequest, REQUEST_DEADLINE_MILLIS},
    },
    cloud_runtime::{companions::lifecycle::Phase, state::OperationId},
    upkeep::worker::{CompanionAction as Ask, CompanionAsk},
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How many answers of one Upkeep travel to its worker; the exchange command has a size limit.
const ANSWERS: usize = 4;
/// How many characters of an answer's error or message travel.
const TEXT: usize = 120;
/// How long an Upkeep cloud stays not ready before what its runs started stops, so that a
/// short refresh never ends a check in the middle.
const NOT_READY: Duration = Duration::from_mins(5);
/// How often those stops are sent again while the cloud stays not ready, at the pace of a
/// ready cloud's exchanges.
const NOT_READY_RETRY: Duration = Duration::from_secs(30);

/// What one sync of an Upkeep cloud saw.
pub(in crate::app) enum Seen {
    /// An exchange answered, with the requests it read and whether a run is active.
    Answer { asks: Vec<CompanionAsk>, running: bool },
    /// The cloud is ready.
    Ready,
    /// The cloud is not ready.
    NotReady,
}

#[derive(Default)]
pub(super) struct State {
    /// The last answer for each alias of each Upkeep cloud, as its next exchange sends them.
    answers: BTreeMap<String, BTreeMap<String, Value>>,
    /// Requests whose job has not answered yet, by cloud and alias.
    inflight: BTreeSet<(String, String)>,
    /// Companions that a run started, by cloud and alias, with the operation ID of the
    /// Stop that ends them once no run is active.
    started: BTreeMap<(String, String), Option<String>>,
    /// When what the runs of each Upkeep cloud that is not ready started stops next.
    not_ready: BTreeMap<String, Instant>,
}

impl HorizonApp {
    /// Acts on what a sync of Upkeep `cloud` saw: its agent's requests while a run is active,
    /// and a Stop for each companion that a run started once no run is active or the cloud
    /// has not been ready for [`NOT_READY`]. A request outside a run, such as a file the
    /// agent left behind, starts nothing.
    pub(in crate::app) fn run_upkeep_companions(&mut self, cloud: &str, seen: Seen, ctx: &egui::Context) {
        let state = &mut self.cloud_prototype.production.companions.agent.upkeep;
        let (asks, running) = match seen {
            Seen::Answer { asks, running } => {
                state.not_ready.remove(cloud);
                (asks, running)
            }
            Seen::Ready => {
                state.not_ready.remove(cloud);
                return;
            }
            Seen::NotReady => {
                let now = Instant::now();
                let due = state.not_ready.entry(cloud.to_owned()).or_insert(now + NOT_READY);
                if now < *due {
                    return;
                }
                // Not on every frame while a Stop runs.
                *due = now + NOT_READY_RETRY;
                (Vec::new(), false)
            }
        };
        let requests: Vec<(String, Ask, String)> = if running {
            asks.into_iter()
                .map(|ask| (ask.alias, ask.action, ask.operation_id))
                .collect()
        } else {
            state
                .started
                .iter_mut()
                .filter(|((source, _), _)| source == cloud)
                .map(|((_, alias), stop)| {
                    let id = stop.get_or_insert_with(manifest::provider_usage::new_operation_id);
                    (alias.clone(), Ask::Stop, id.clone())
                })
                .collect()
        };
        for (alias, action, id) in requests {
            let state = &self.cloud_prototype.production.companions.agent.upkeep;
            let key = (cloud.to_owned(), alias.clone());
            let answered = state
                .answers
                .get(cloud)
                .and_then(|answers| answers.get(&alias))
                .is_some_and(|answer| answer["operation_id"] == id.as_str() && answer["done"] == true);
            if state.inflight.contains(&key) || answered {
                continue;
            }
            if let Err(error) = self.start_upkeep_companion(cloud, &alias, action, &id, ctx) {
                let state = &mut self.cloud_prototype.production.companions.agent.upkeep;
                let answer = failure(action, &id, &error, state.started.contains_key(&key));
                state.record(cloud, &alias, &answer);
            }
        }
    }

    /// Horizon's answers to the companion requests of Upkeep `cloud`, by alias, for its next
    /// exchange.
    pub(in crate::app) fn upkeep_companion_answers(&self, cloud: &str) -> Map<String, Value> {
        let answers = &self.cloud_prototype.production.companions.agent.upkeep.answers;
        answers
            .get(cloud)
            .map(|answers| {
                answers
                    .iter()
                    .map(|(alias, answer)| (alias.clone(), answer.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn start_upkeep_companion(
        &mut self,
        cloud: &str,
        alias: &str,
        action: Ask,
        id: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let operation_id =
            serde_json::from_value::<OperationId>(json!(id)).map_err(|_| "The operation ID is not a UUID")?;
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("Horizon has no cloud state directory")?;
        let groups = self.cloud_prototype.groups.clone();
        let companions = &mut self.cloud_prototype.production.companions;
        let owner = companions
            .entries
            .get(cloud)
            .filter(|entry| !entry.blocked)
            .map(|entry| entry.owner.clone())
            .ok_or("Horizon has not read this cloud's companions yet; nothing was started")?;
        // As an agent panel's request: the cloud's periodic refresh waits for it.
        companions.agent.hold(cloud);
        let state = &mut companions.agent.upkeep;
        state.inflight.insert((cloud.to_owned(), alias.to_owned()));
        let sender = companions.agent.sender();
        let request = CompanionRequest {
            action: match action {
                Ask::EnsureReady => CompanionAction::EnsureReady,
                Ask::Stop => CompanionAction::Stop,
            },
            cloud: Some(cloud.to_owned()),
            alias: Some(alias.to_owned()),
            operation_id: Some(id.to_owned()),
            tailnet: None,
        };
        let deadline = manifest::now_millis() + REQUEST_DEADLINE_MILLIS;
        let (source, alias, ctx) = (cloud.to_owned(), alias.to_owned(), ctx.clone());
        let ask = (action, id.to_owned());
        std::thread::spawn(move || {
            let outcome = submit(&root, &owner, &groups, (&request, deadline), operation_id).map(Box::new);
            let _ = sender.send(Message::Upkeep {
                source,
                alias,
                ask,
                outcome,
            });
            ctx.request_repaint();
        });
        Ok(())
    }

    /// Keeps the answer to an Upkeep request for the next exchange, continuing its operation
    /// on the companion's card as an agent panel's request would.
    pub(super) fn finish_upkeep_companion(
        &mut self,
        source: &str,
        alias: &str,
        (action, id): &(Ask, String),
        outcome: Result<Box<Submitted>, String>,
        ctx: &egui::Context,
    ) {
        let key = (source.to_owned(), alias.to_owned());
        let (answer, started) = match outcome {
            Ok(submitted) => {
                let (phase, stop) = (
                    submitted.operation.phase,
                    submitted.operation.intent.action == horizon_core::cloud_runtime::companions::intent::Action::Stop,
                );
                let (answer, started) = self.answer_submitted(false, false, source, *submitted, ctx);
                let state = &mut self.cloud_prototype.production.companions.agent.upkeep;
                match (stop, phase) {
                    // Nothing runs for these, so nothing is left to stop.
                    (false, Phase::ConfirmationRequired | Phase::Refused | Phase::RetryRequired) => {}
                    // A Stop sent before ended an earlier start: the next one is a new
                    // operation, or the done Stop would be sent again and stop nothing.
                    (false, _) => {
                        state.started.insert(key.clone(), None);
                    }
                    (true, Phase::Stopped | Phase::Refused) => {
                        state.started.remove(&key);
                    }
                    // A Stop that needs a new request gets a new operation ID.
                    (true, Phase::RetryRequired) => {
                        if let Some(stop) = state.started.get_mut(&key) {
                            *stop = None;
                        }
                    }
                    (true, _) => {}
                }
                (answer, started)
            }
            Err(error) => {
                let started = self
                    .cloud_prototype
                    .production
                    .companions
                    .agent
                    .upkeep
                    .started
                    .contains_key(&key);
                (failure(*action, id, &error, started), false)
            }
        };
        // A started execution keeps the hold until it reports back.
        if !started {
            self.release_companion_source(source);
        }
        let state = &mut self.cloud_prototype.production.companions.agent.upkeep;
        state.inflight.remove(&key);
        state.record(source, alias, &answer);
    }
}

/// The answer to a request that Horizon could not record. A busy companion, and a Stop of a
/// companion that a run started (`started`), are not done, so the next exchange sends them
/// again; any other request is done, with the reason for the agent, so that it never holds
/// back a later one.
fn failure(action: Ask, id: &str, error: &str, started: bool) -> Value {
    let done = (action != Ask::Stop || !started) && !error.starts_with("cloud_companion_busy");
    json!({ "operation_id": id, "action": action_name(action), "done": done, "error": error })
}

fn action_name(action: Ask) -> &'static str {
    match action {
        Ask::EnsureReady => "ensure_ready",
        Ask::Stop => "stop",
    }
}

impl State {
    /// Keeps the fields the agent reads, and its error or else its message, short and without
    /// control characters. Answers of other aliases that are done give way when there are too
    /// many.
    fn record(&mut self, cloud: &str, alias: &str, answer: &Value) {
        let mut kept = Map::new();
        for field in [
            "operation_id",
            "action",
            "target_cloud_id",
            "phase",
            "done",
            "executing",
            "resend",
        ] {
            if let Some(value) = answer.get(field) {
                kept.insert(field.into(), value.clone());
            }
        }
        if let Some((field, text)) = ["error", "message"]
            .into_iter()
            .find_map(|field| Some((field, answer.get(field)?.as_str()?)))
        {
            let text = text.chars().filter(|c| !c.is_control()).take(TEXT).collect();
            kept.insert(field.into(), Value::String(text));
        }
        let answers = self.answers.entry(cloud.to_owned()).or_default();
        answers.insert(alias.to_owned(), Value::Object(kept));
        if answers.len() > ANSWERS {
            answers.retain(|held, answer| held == alias || answer["done"] != true);
        }
        while answers.len() > ANSWERS {
            let Some(other) = answers.keys().find(|held| *held != alias).cloned() else {
                break;
            };
            answers.remove(&other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_panel::{CloudConfig, CloudGroup, CloudGroups, CloudLaunch};

    /// An app whose Upkeep cloud `upkeep` has a companion entry, and the operation ID of a
    /// request of its agent.
    fn upkeep_app() -> (tempfile::TempDir, crate::app::HorizonApp, String) {
        let (temp, mut app) = crate::app::test_support::test_app();
        let config = CloudConfig::parse(
            "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 2\n    memory_gb: 4\n",
        )
        .unwrap();
        let mut group = CloudGroup::new(1, "upkeep".into(), "w".into(), "/synthetic".into(), [0.0, 0.0]);
        group.remote = Some(CloudLaunch {
            publishing_selection: None,
            deployment_started: true,
            id: "upkeep".into(),
            revision: "a".repeat(40),
            profile_name: "dev".into(),
            profile: config.profiles["dev"].clone(),
            placement: horizon_core::cloud_panel::Placement::default(),
        });
        app.cloud_prototype.groups = CloudGroups(vec![group]);
        app.cloud_prototype.root = Some(temp.path().join("cloud"));
        let state = &mut app.cloud_prototype.production.companions;
        state.sync(Some("session"), &app.cloud_prototype.groups);
        (temp, app, manifest::provider_usage::new_operation_id())
    }

    fn ask(action: Ask, operation_id: &str) -> CompanionAsk {
        CompanionAsk {
            alias: "youpark".into(),
            action,
            operation_id: operation_id.into(),
        }
    }

    fn running(asks: Vec<CompanionAsk>) -> Seen {
        Seen::Answer { asks, running: true }
    }

    fn ended(asks: Vec<CompanionAsk>) -> Seen {
        Seen::Answer { asks, running: false }
    }

    #[test]
    fn a_request_outside_a_run_starts_nothing() {
        let (_temp, mut app, id) = upkeep_app();
        let ctx = egui::Context::default();
        app.run_upkeep_companions("upkeep", ended(vec![ask(Ask::EnsureReady, &id)]), &ctx);
        let agent = &app.cloud_prototype.production.companions.agent;
        assert!(!agent.holds("upkeep"));
        assert!(agent.upkeep.inflight.is_empty() && agent.upkeep.answers.is_empty());
    }

    #[test]
    fn a_request_runs_once_until_its_job_answers() {
        let (_temp, mut app, id) = upkeep_app();
        let ctx = egui::Context::default();
        app.run_upkeep_companions("upkeep", running(vec![ask(Ask::EnsureReady, &id)]), &ctx);
        app.run_upkeep_companions("upkeep", running(vec![ask(Ask::EnsureReady, &id)]), &ctx);
        let agent = &app.cloud_prototype.production.companions.agent;
        assert_eq!(
            agent.held.get("upkeep"),
            Some(&1),
            "the second exchange waits for the first job"
        );
        assert!(agent.upkeep.inflight.contains(&("upkeep".into(), "youpark".into())));
    }

    #[test]
    fn a_cloud_without_companions_answers_why() {
        let (_temp, mut app, id) = upkeep_app();
        let ctx = egui::Context::default();
        app.run_upkeep_companions("other", running(vec![ask(Ask::EnsureReady, &id)]), &ctx);
        let answers = app.upkeep_companion_answers("other");
        assert_eq!(answers["youpark"]["done"], true);
        assert_eq!(answers["youpark"]["operation_id"], id.as_str());
        assert!(
            answers["youpark"]["error"]
                .as_str()
                .unwrap()
                .contains("nothing was started")
        );
        assert!(!app.cloud_prototype.production.companions.agent.holds("other"));
    }

    #[test]
    fn a_failed_request_names_its_operation_and_a_failed_stop_is_sent_again() {
        let id = manifest::provider_usage::new_operation_id();
        let refused = failure(Ask::EnsureReady, &id, "Companion is not selected", false);
        assert_eq!(refused["operation_id"], id.as_str());
        assert_eq!(refused["done"], true);
        assert_eq!(
            failure(Ask::EnsureReady, &id, "cloud_companion_busy: wait", false)["done"],
            false
        );
        assert_eq!(
            failure(Ask::Stop, &id, "Horizon has no cloud state directory", true)["done"],
            false
        );
        // A refused Stop of a companion that no run started is done.
        assert_eq!(
            failure(Ask::Stop, &id, "Companion is not selected", false)["done"],
            true
        );
        // A Stop whose start failed is tried again with the next exchange.
        let (temp, mut app, _) = upkeep_app();
        let ctx = egui::Context::default();
        app.cloud_prototype.root = None;
        let key = ("upkeep".to_owned(), "youpark".to_owned());
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.started.insert(key.clone(), Some(id.clone()));
        app.run_upkeep_companions("upkeep", ended(Vec::new()), &ctx);
        let answers = app.upkeep_companion_answers("upkeep");
        assert_eq!(answers["youpark"]["done"], false);
        app.cloud_prototype.root = Some(temp.path().join("cloud"));
        app.run_upkeep_companions("upkeep", ended(Vec::new()), &ctx);
        assert!(app.cloud_prototype.production.companions.agent.holds("upkeep"));
    }

    #[test]
    fn what_a_run_started_is_stopped_once_no_run_is_active() {
        let (_temp, mut app, _) = upkeep_app();
        let ctx = egui::Context::default();
        let key = ("upkeep".to_owned(), "youpark".to_owned());
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.started.insert(key.clone(), None);
        app.run_upkeep_companions("upkeep", running(Vec::new()), &ctx);
        assert!(
            !app.cloud_prototype.production.companions.agent.holds("upkeep"),
            "not during a run"
        );
        app.run_upkeep_companions("upkeep", ended(Vec::new()), &ctx);
        let agent = &app.cloud_prototype.production.companions.agent;
        assert!(agent.holds("upkeep"));
        let stop = agent.upkeep.started[&key].clone().unwrap();
        // A Stop that is done is not sent again.
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.inflight.clear();
        state.record("upkeep", "youpark", &json!({"operation_id": stop, "done": true}));
        app.run_upkeep_companions("upkeep", ended(Vec::new()), &ctx);
        assert!(
            app.cloud_prototype
                .production
                .companions
                .agent
                .upkeep
                .inflight
                .is_empty()
        );
    }

    #[test]
    fn a_new_start_gets_a_new_stop() {
        let (_temp, mut app, id) = upkeep_app();
        let ctx = egui::Context::default();
        let key = ("upkeep".to_owned(), "youpark".to_owned());
        let earlier = manifest::provider_usage::new_operation_id();
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.started.insert(key.clone(), Some(earlier));
        let started = Box::new(super::super::tests::settled_start("upkeep", "youpark"));
        app.finish_upkeep_companion("upkeep", "youpark", &(Ask::EnsureReady, id), Ok(started), &ctx);
        assert_eq!(
            app.cloud_prototype.production.companions.agent.upkeep.started[&key],
            None
        );
    }

    #[test]
    fn a_cloud_that_stays_not_ready_stops_what_a_run_started() {
        let (_temp, mut app, _) = upkeep_app();
        let ctx = egui::Context::default();
        let key = ("upkeep".to_owned(), "youpark".to_owned());
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.started.insert(key, None);
        app.run_upkeep_companions("upkeep", Seen::NotReady, &ctx);
        let agent = &app.cloud_prototype.production.companions.agent;
        assert!(!agent.holds("upkeep"), "a short refresh stops nothing");
        // Ready again: the next time it is not ready, the wait starts again.
        app.run_upkeep_companions("upkeep", Seen::Ready, &ctx);
        assert!(
            app.cloud_prototype
                .production
                .companions
                .agent
                .upkeep
                .not_ready
                .is_empty()
        );
        app.run_upkeep_companions("upkeep", Seen::NotReady, &ctx);
        let state = &mut app.cloud_prototype.production.companions.agent.upkeep;
        state.not_ready.insert("upkeep".into(), Instant::now());
        app.run_upkeep_companions("upkeep", Seen::NotReady, &ctx);
        let agent = &app.cloud_prototype.production.companions.agent;
        assert!(agent.holds("upkeep"));
        assert!(
            agent.upkeep.not_ready["upkeep"] > Instant::now(),
            "sent again later, not every frame"
        );
    }

    #[test]
    fn answers_keep_short_known_fields_and_a_bounded_count() {
        let mut state = State::default();
        let long = "x\u{1}".repeat(TEXT * 2);
        state.record(
            "c1",
            "youpark",
            &json!({"operation_id": "o1", "phase": "ready", "done": true, "message": long, "cloud": "c1", "extra": 1}),
        );
        let answer = &state.answers["c1"]["youpark"];
        assert_eq!(answer["message"].as_str().unwrap(), "x".repeat(TEXT));
        assert!(answer.get("extra").is_none() && answer.get("cloud").is_none());
        state.record("c1", "youpark", &json!({"done": true, "message": "m", "error": "e"}));
        assert_eq!(state.answers["c1"]["youpark"], json!({"done": true, "error": "e"}));
        for n in 0..ANSWERS + 3 {
            state.record("c1", &format!("a{n}"), &json!({"done": n % 2 == 0}));
        }
        assert_eq!(state.answers["c1"].len(), ANSWERS);
        assert!(
            state.answers["c1"].contains_key(&format!("a{}", ANSWERS + 2)),
            "the newest stays"
        );
    }
}
