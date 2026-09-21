//! Report transport and presentation separately; static pixels are not a failed stream.
use super::{DevicePanelState, DeviceUiState, Status};
use horizon_core::browser::manifest::{
    self,
    device::{Connection, Diagnostics, ImageEvidence, PanelState, Presentation},
};
use std::time::Instant;

impl DeviceUiState {
    pub(crate) fn observation(
        &mut self,
        panel_id: String,
        device: &DevicePanelState,
        visible: bool,
        actor: &str,
    ) -> PanelState {
        if let Some(status) = self.session.as_ref().and_then(super::Session::take_status) {
            self.status = status;
        }
        let (connection, connection_error) = match &self.status {
            Status::Stopped => (Connection::Stopped, None),
            Status::Connecting => (Connection::Connecting, None),
            Status::Connected => (Connection::Connected, None),
            Status::Disconnected(error) => (Connection::Disconnected, Some(error.clone())),
        };
        let (stream, paused) = self.session.as_ref().map_or_else(
            || (super::session::StreamEvidence::default(), true),
            super::Session::stream_evidence,
        );
        let displayed =
            visible && self.image.received && self.image.previous_displayed && connection == Connection::Connected;
        let presentation = match connection {
            Connection::Stopped => Presentation::Stopped,
            Connection::Connecting => Presentation::Connecting,
            Connection::Disconnected => Presentation::Disconnected,
            Connection::Connected if !visible => Presentation::Hidden,
            Connection::Connected if !self.previous_rendered => Presentation::NotRendered,
            Connection::Connected if !self.image.received => Presentation::AwaitingFrame,
            Connection::Connected if displayed => Presentation::Displayed,
            Connection::Connected => Presentation::Clipped,
        };
        PanelState {
            panel_id,
            endpoint: device.target.address().to_string(),
            visible,
            owned_by_caller: self.owner.as_deref() == Some(actor),
            image: ImageEvidence {
                image_received: self.image.received,
                image_displayed: displayed,
                frame_sequence: self.image.sequence,
            },
            diagnostics: Some(Diagnostics {
                observed_at_millis: manifest::now_millis(),
                connection_generation: self.connection_generation,
                presentation,
                sampling_paused: paused,
                decoded_frame_sequence: stream.sequence,
                last_decoded_age_millis: age(stream.last_frame),
                last_displayed_age_millis: age(self.image.last_displayed),
            }),
            connection,
            connection_error,
        }
    }
}

fn age(at: Option<Instant>) -> Option<u64> {
    at.map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX))
}
