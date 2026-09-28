//! Running a recorded companion operation on its target cloud's card.
use super::{
    Cancellation, Context, Event, HorizonApp, Message, Operation, OperationId, Phase, Settings, Stage, Store, channel,
    cloud_runtime, hint, lifecycle,
};

impl HorizonApp {
    pub(super) fn execute_on_card(
        &mut self,
        source: &str,
        target: &str,
        (alias, stop): (&str, bool),
        id: OperationId,
        context: Context,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == target))
            .ok_or("The companion cloud is not open in this Horizon; nothing was started")?;
        let card = group.issue;
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("Horizon has no cloud state directory")?;
        let settings = group
            .remote
            .as_ref()
            .map(|launch| Settings::for_cloud(&root.join("settings.json"), &launch.placement))
            .ok_or("The companion cloud has no deployment")?
            .map_err(|error| error.to_string())?;
        let owner = self
            .cloud_prototype
            .production
            .companions
            .entries
            .get(source)
            .map(|entry| entry.owner.clone())
            .ok_or("The source cloud closed; nothing was started")?;
        let runtime = self.cloud_prototype.production.runtimes.entry(card).or_default();
        if runtime.busy() {
            return Err("The companion cloud's card is running another operation; send the request again once it finishes, and it continues this operation".into());
        }
        // A connected card keeps its connection for an Ensure Ready; a Stop ends it
        // first, as the card's own Stop does.
        let tx = match runtime.sender.clone() {
            Some(sender) if !stop && runtime.receiver.is_some() && runtime.stage == Some(Stage::Ready) => sender,
            _ => {
                if let Some(cancel) = runtime.cancel.take() {
                    cancel.cancel();
                }
                runtime.idle_reports = None;
                runtime.desktop = None;
                runtime.confirmation = super::super::super::Confirmation::None;
                runtime.rebuild = None;
                if runtime.progress.is_deletion() {
                    runtime.progress.reset();
                }
                runtime.error = None;
                runtime.stage = Some(if stop { Stage::Stopping } else { Stage::Provision });
                let (tx, rx) = channel();
                runtime.receiver = Some(rx);
                runtime.sender = Some(tx.clone());
                runtime.cancel = Some(Cancellation::default());
                tx
            }
        };
        let connected = runtime.stage == Some(Stage::Ready) && !stop;
        let cancel = runtime.cancel.clone().unwrap_or_default();
        runtime.push_log(format!(
            "An agent asked to {} this cloud as companion `{alias}`",
            if stop { "stop" } else { "start" }
        ));
        let agent = &mut self.cloud_prototype.production.companions.agent;
        agent.executing.insert(target.to_owned());
        let sender = agent.sender();
        let (source, target, alias) = (source.to_owned(), target.to_owned(), alias.to_owned());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let emit = |event: Event| {
                if matches!(event, Event::Output(_))
                    || (!connected && matches!(event, Event::Stage(..) | Event::Progress(_) | Event::Snapshot(_)))
                {
                    let _ = tx.send(event);
                    ctx.request_repaint();
                }
            };
            let request = lifecycle::Request {
                root: &root,
                owner: &owner,
                context: &context,
                alias: &alias,
            };
            let result = lifecycle::execute(&request, id, &settings, &cancel, &emit);
            let state_root = cloud_runtime::state::cloud_directory(&root, &target).ok();
            let saved = state_root.and_then(|path| Store::lock(&path).and_then(|store| store.load()).ok().flatten());
            for event in finish(result, saved, connected) {
                let _ = tx.send(event);
            }
            let _ = sender.send(Message::Executed { source, target });
            ctx.request_repaint();
        });
        Ok(())
    }
}

/// The events that leave the card where the operation left its cloud.
pub(super) fn finish(
    result: cloud_runtime::Result<Operation>,
    saved: Option<cloud_runtime::state::Deployment>,
    connected: bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    match result {
        // A card that stayed connected already shows a ready cloud.
        Ok(operation) if operation.phase == Phase::Ready && connected => {}
        // The card connects the ready cloud's sessions as its own Resume does.
        Ok(operation) if operation.phase == Phase::Ready => events.push(Event::Resumed),
        Ok(operation) if operation.phase == Phase::Stopped => match saved {
            Some(state) => events.push(Event::Stopped(Box::new(state))),
            None => events.push(Event::failed("Stopped, but the cloud's record is unavailable".into())),
        },
        Ok(operation) => {
            events.extend(saved.map(|state| Event::Snapshot(Box::new(state))));
            events.push(Event::failed(hint(operation.phase).into()));
        }
        Err(error) => {
            events.extend(saved.map(|state| Event::Snapshot(Box::new(state))));
            events.push(Event::failed(error.to_string()));
        }
    }
    events
}
