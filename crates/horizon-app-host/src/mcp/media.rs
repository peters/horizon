use super::{Arc, Json, NativeMcp, Value, handle, model};
use crate::{Error, Result};
use horizon_app_provider::media::Kind;

fn archive(server: &NativeMcp) -> Result<Arc<crate::archive::Archive>> {
    let mut retained = server.media.lock().map_err(|_| Error::Unavailable)?;
    if retained.is_none() {
        *retained = Some(Arc::new(server.reports.create()?));
    }
    retained.as_ref().cloned().ok_or(Error::Unavailable)
}
pub(super) async fn logs(server: &NativeMcp, input: model::Logs) -> std::result::Result<Json<Value>, String> {
    let server = server.clone();
    tokio::task::spawn_blocking(move || {
        let id = handle(&input.session)?;
        let kind = match input.kind {
            model::LogKind::Device => Kind::Device,
            model::LogKind::Crash => Kind::Crash,
            model::LogKind::Appium => Kind::Appium,
            model::LogKind::Network => Kind::Network,
        };
        let archive = archive(&server)?;
        server
            .actor
            .export_media(id, kind, std::time::Duration::from_secs(30), |bytes| {
                serde_json::to_value(archive.media(kind, &bytes)?).map_err(|_| Error::Unavailable)
            })
    })
    .await
    .map_err(|_| Error::Unavailable.to_string())?
    .map(Json)
    .map_err(|error: Error| error.to_string())
}
pub(super) async fn video(server: &NativeMcp, input: model::Video) -> std::result::Result<Json<Value>, String> {
    let server = server.clone();
    tokio::task::spawn_blocking(move || {
        let id=handle(&input.session)?;
        let enabled=server.actor.video_enabled(id)?;
        if matches!(input.operation,model::VideoOperation::Start) && !enabled {
            return Err(horizon_app_provider::Error::MediaUnavailable.into());
        }
        if matches!(input.operation,model::VideoOperation::Start|model::VideoOperation::Status) {
            return Ok(serde_json::json!({"enabled":enabled,"recording_policy":"allocation_to_session_close","pause_supported":false}));
        }
        if matches!(input.operation,model::VideoOperation::Stop) {server.actor.close(id)?;}
        if !enabled {return Err(horizon_app_provider::Error::MediaUnavailable.into());}
        let archive=archive(&server)?;
        server.actor.export_media(id, Kind::Video, std::time::Duration::from_secs(30), |bytes| {
            serde_json::to_value(archive.media(Kind::Video, &bytes)?).map_err(|_|Error::Unavailable)
        })
    }).await.map_err(|_|Error::Unavailable.to_string())?.map(Json).map_err(|error:Error|error.to_string())
}
