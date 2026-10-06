//! Adopts the browser host instance that the worker supervisor assigned.
//!
//! On a worker with agent isolation the control service runs as the agent
//! account, so it cannot write the supervisor's root-owned runtime directory.
//! The supervisor chooses the value instead, gives it to this service, and
//! publishes it to agent sessions after the service answers. It is an
//! identity, not a credential.
use std::{ffi::OsString, io};

/// Set by `horizon-worker-supervise` for the control service only.
pub const ASSIGNED_ENV: &str = "HORIZON_WORKER_BROWSER_HOST_INSTANCE";

/// Adopts the assigned value. Without one, the service keeps a generated
/// identity that no agent session receives.
pub fn adopt() -> io::Result<()> {
    adopt_from(
        std::env::var_os(ASSIGNED_ENV),
        horizon_browser_control::manifest::configure_host_instance,
    )
}

fn adopt_from(value: Option<OsString>, configure: impl FnOnce(&str) -> io::Result<()>) -> io::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value
        .into_string()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid browser host instance"))?;
    configure(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assigned_value_is_configured_exactly() {
        let mut configured = None;
        adopt_from(Some("assigned-host".into()), |value| {
            configured = Some(value.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(configured.as_deref(), Some("assigned-host"));
    }

    #[test]
    fn without_an_assignment_nothing_is_configured() {
        adopt_from(None, |_| panic!("no value was assigned")).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_value_that_is_not_text_is_refused_before_configuration() {
        use std::os::unix::ffi::OsStringExt;
        let error = adopt_from(Some(OsString::from_vec(vec![0xff])), |_| panic!("configured")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_refused_value_stops_the_service() {
        let error = adopt_from(Some("other-host".into()), |_| {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, "already in use"))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }
}
