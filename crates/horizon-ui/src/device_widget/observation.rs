//! Report transport and presentation separately; static pixels are not a failed stream.
use super::{DevicePanelState, DeviceUiState, Status};
use horizon_core::browser::manifest::{
    self,
    device::{
        Connection, ConnectionFailure, ConnectionFailureCode, Diagnostics, ImageEvidence, ObservedSshHostKey,
        PanelState, Presentation, Reconnect, SshHostKeyFailure, SshRoute,
    },
};
use horizon_vnc::{HostKeyFailure, HostKeyRefusal};
use std::time::Instant;

impl DeviceUiState {
    pub(crate) fn observation(
        &mut self,
        panel_id: String,
        device: &DevicePanelState,
        visible: bool,
        actor: &str,
    ) -> PanelState {
        if let Some(session) = &self.session {
            let observation = session.observation();
            self.image.received_sequence = observation.received_frame_sequence;
            if let Some(status) = observation.status {
                self.status = status;
            }
            let details = session.take_server_details();
            if let Some(metadata) = details.native_session {
                self.set_native_metadata(metadata);
            }
            if let Some(name) = details.name {
                self.server.name = Some(name);
            }
            if let Some(size) = details.desktop_size {
                self.server.desktop_size = Some(size);
            }
        }
        let observed_at = manifest::now_millis();
        let (connection, reconnect, connection_error, failure) = match &self.status {
            Status::Stopped => (Connection::Stopped, None, None, None),
            Status::Connecting => (Connection::Connecting, None, None, None),
            Status::Connected => (Connection::Connected, None, None, None),
            Status::Reconnecting {
                attempt,
                retry_at,
                message,
            } => (
                Connection::Reconnecting,
                Some(Reconnect {
                    attempt: *attempt,
                    next_attempt_in_millis: u64::try_from(
                        retry_at.saturating_duration_since(Instant::now()).as_millis(),
                    )
                    .unwrap_or(u64::MAX),
                }),
                Some(message.clone()),
                None,
            ),
            Status::Disconnected { message, host_key } => (
                Connection::Disconnected,
                None,
                Some(message.clone()),
                host_key.as_ref().map(connection_failure),
            ),
        };
        let (stream, paused) = self.session.as_ref().map_or_else(
            || (super::session::StreamEvidence::default(), true),
            super::Session::stream_evidence,
        );
        let displayed =
            visible && self.image.received && self.image.previous_displayed && connection == Connection::Connected;
        let presentation = match connection {
            Connection::Stopped => Presentation::Stopped,
            // A lost link keeps the last image on screen, and nothing of the
            // current attempt is presented yet.
            Connection::Connecting | Connection::Reconnecting => Presentation::Connecting,
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
            identity: device.identity.clone(),
            ssh: device.ssh_tunnel.as_ref().map(|connection| SshRoute {
                host: connection.host.clone(),
                user: connection.user.clone(),
                port: connection.port,
            }),
            server: self.server.clone(),
            visible,
            owned_by_caller: self.owner.as_deref() == Some(actor),
            image: ImageEvidence {
                image_received: self.image.received,
                image_displayed: displayed,
                frame_sequence: self.image.sequence,
                received_frame_sequence: self.image.received_sequence,
            },
            diagnostics: Some(Diagnostics {
                observed_at_millis: observed_at,
                connection_generation: self.connection_generation,
                presentation,
                sampling_paused: paused,
                decoded_frame_sequence: stream.sequence,
                last_decoded_age_millis: age(stream.last_frame),
                last_uploaded_age_millis: age(self.image.last_uploaded),
                last_displayed_age_millis: age(self.image.last_displayed),
                host: self.host.observation(),
            }),
            connection,
            reconnect,
            connection_error,
            connection_failure: failure,
        }
    }
}

/// The typed cause of a refusal, with the evidence a person decides with.
/// Horizon has no operation that trusts a host.
fn connection_failure(host_key: &HostKeyFailure) -> ConnectionFailure {
    let code = match host_key.refusal {
        HostKeyRefusal::NotTrusted => ConnectionFailureCode::HostNotTrusted,
        HostKeyRefusal::Changed => ConnectionFailureCode::HostKeyChanged,
    };
    ConnectionFailure {
        code,
        ssh_host_key: Some(SshHostKeyFailure {
            host: host_key.host.clone(),
            port: host_key.port,
            observed: host_key.observed.as_ref().map(|observed| ObservedSshHostKey {
                key_type: observed.key_type.clone(),
                sha256: observed.sha256.clone(),
            }),
        }),
        action: host_key.advice(),
    }
}

fn age(at: Option<Instant>) -> Option<u64> {
    at.map(|at| u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX))
}
