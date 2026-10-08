use super::{Arc, Duration, Error, Json, NativeMcp, RequestContext, RoleServer, Value};
use crate::runner::{self, Control, Progress};
use rmcp::model::ProgressNotificationParam;

struct Cancel(Arc<Control>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(super) async fn execute(
    server: &NativeMcp,
    lifetime: Duration,
    context: RequestContext<RoleServer>,
) -> std::result::Result<Json<Value>, String> {
    let control = Arc::new(Control::new(lifetime).map_err(|error| error.to_string())?);
    let _cancel = Cancel(Arc::clone(&control));
    let actor = Arc::clone(&server.actor);
    let views = Arc::clone(&server.views);
    let archive = Arc::new(server.reports.create().map_err(|error| error.to_string())?);
    let retained_archive = Arc::clone(&archive);
    let retained = Arc::clone(&control);
    let (send, mut receive) = tokio::sync::mpsc::channel::<Progress>(32);
    let mut task = tokio::task::spawn_blocking(move || {
        let observer = views.observer();
        let result = runner::run(
            &actor,
            &retained,
            |session, kind, bytes| retained_archive.capture(session, kind, bytes),
            |mut progress| {
                observer.observe(&mut progress)?;
                send.blocking_send(progress).map_err(|_| Error::Cancelled)
            },
        );
        drop(observer);
        retained_archive.finish_result(result)
    });
    let token = context.meta.get_progress_token();
    let mut sequence = 0_u32;
    let mut cancelled = false;
    let mut open = true;
    let report = loop {
        tokio::select! {
            ()=context.ct.cancelled(),if !cancelled => { cancelled=true;control.cancel(); }
            progress=receive.recv(),if open => {
                if let Some(progress)=progress {
                    sequence=sequence.saturating_add(1);
                    if !cancelled && let Some(token)=&token {
                        let message=serde_json::to_string(&progress).map_err(|_|Error::Unavailable.to_string())?;
                        let notification=ProgressNotificationParam::new(token.clone(),f64::from(sequence)).with_message(message);
                        if !matches!(tokio::time::timeout(Duration::from_secs(2),context.peer.notify_progress(notification)).await,Ok(Ok(()))) {
                            cancelled=true;control.cancel();
                        }
                    }
                } else { open=false; }
            }
            result=&mut task => break result.map_err(|_|Error::Unavailable.to_string())?.map_err(|error|error.to_string())?,
        }
    };
    Ok(Json(report))
}
