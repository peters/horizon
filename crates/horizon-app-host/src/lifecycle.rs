//! Host failures retain bounded typed causes instead of private input or error text.
pub use horizon_app_process::diagnostic::{Cause as Fault, Lock, Operation, Reason};

impl crate::Error {
    pub(crate) const fn host_lock(resource: Lock) -> Self {
        Self::LifecycleUnavailable(Fault::LockPoisoned { resource })
    }
    pub(crate) const fn host(operation: Operation, reason: Reason) -> Self {
        Self::LifecycleUnavailable(Fault::State { operation, reason })
    }

    pub(crate) fn host_io(operation: Operation, error: &std::io::Error) -> Self {
        Self::LifecycleUnavailable(Fault::Io {
            operation,
            kind: error.kind(),
        })
    }

    pub(crate) fn host_task(operation: Operation, error: &tokio::task::JoinError) -> Self {
        Self::host(
            operation,
            if error.is_cancelled() {
                Reason::TaskCancelled
            } else {
                Reason::TaskPanicked
            },
        )
    }

    pub(crate) fn host_json(operation: Operation, error: &serde_json::Error) -> Self {
        if let Some(kind) = error.io_error_kind() {
            Self::LifecycleUnavailable(Fault::Io { operation, kind })
        } else {
            Self::host(operation, Reason::SerializationFailed)
        }
    }

    pub(crate) fn host_mcp(error: &rmcp::service::ServerInitializeError) -> Self {
        use rmcp::service::ServerInitializeError;
        let reason = match error {
            ServerInitializeError::TransportError { error, .. } => {
                let mut source = Some(error.error.as_ref() as &(dyn std::error::Error + 'static));
                for _ in 0..16 {
                    let Some(current) = source else { break };
                    if let Some(error) = current.downcast_ref::<std::io::Error>() {
                        return Self::host_io(Operation::Mcp, error);
                    }
                    source = current.source();
                }
                Reason::TransportFailed
            }
            ServerInitializeError::ConnectionClosed(_) => Reason::ChannelClosed,
            ServerInitializeError::Cancelled => Reason::TaskCancelled,
            ServerInitializeError::ExpectedInitializeRequest(_)
            | ServerInitializeError::UnexpectedInitializeResponse(_) => Reason::HandshakeInvalid,
            ServerInitializeError::InitializeFailed(_) => Reason::HandshakeRefused,
            _ => Reason::TransportFailed,
        };
        Self::host(Operation::Mcp, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn io_retains_kind_without_private_source_text() {
        let error = std::io::Error::new(ErrorKind::PermissionDenied, "secret=/private/client.json?token=private");
        let fault = crate::Error::host_io(Operation::Client, &error);
        assert_eq!(
            fault,
            crate::Error::LifecycleUnavailable(Fault::Io {
                operation: Operation::Client,
                kind: ErrorKind::PermissionDenied
            })
        );
        assert_eq!(fault.to_string(), "app_host_unavailable: Client: I/O PermissionDenied");
        assert!(!format!("{fault:?}").contains("private"));
    }

    #[test]
    fn json_output_keeps_write_error_kind() {
        struct Failed;
        impl std::io::Write for Failed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(ErrorKind::BrokenPipe, "private output path"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let error = serde_json::to_writer(Failed, &serde_json::json!({"synthetic":"report"})).unwrap_err();
        assert_eq!(
            crate::Error::host_json(Operation::Output, &error),
            crate::Error::LifecycleUnavailable(Fault::Io {
                operation: Operation::Output,
                kind: ErrorKind::BrokenPipe
            })
        );
    }

    #[test]
    fn mcp_initialization_keeps_io_kind_and_redacts_foreign_error_payloads() {
        use rmcp::{service::ServerInitializeError, transport::DynamicTransportError};
        let error = ServerInitializeError::TransportError {
            error: DynamicTransportError::from_parts(
                "private transport",
                std::any::TypeId::of::<()>(),
                Box::new(std::io::Error::new(ErrorKind::ConnectionReset, "private token")),
            ),
            context: "private context".into(),
        };
        let fault = crate::Error::host_mcp(&error);
        assert_eq!(
            fault,
            crate::Error::LifecycleUnavailable(Fault::Io {
                operation: Operation::Mcp,
                kind: ErrorKind::ConnectionReset
            })
        );
        assert!(!fault.to_string().contains("private"));
        assert_eq!(
            crate::Error::host_mcp(&ServerInitializeError::ConnectionClosed("private reason".into())),
            crate::Error::host(Operation::Mcp, Reason::ChannelClosed)
        );
        assert_eq!(
            crate::Error::host_mcp(&ServerInitializeError::Cancelled),
            crate::Error::host(Operation::Mcp, Reason::TaskCancelled)
        );
    }

    #[test]
    fn cyclic_foreign_transport_sources_cannot_stall_diagnostics() {
        #[derive(Debug)]
        struct Cyclic;
        impl std::fmt::Display for Cyclic {
            fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                output.write_str("private transport text")
            }
        }
        impl std::error::Error for Cyclic {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(self)
            }
        }
        let error = rmcp::service::ServerInitializeError::TransportError {
            error: rmcp::transport::DynamicTransportError::from_parts(
                "synthetic",
                std::any::TypeId::of::<()>(),
                Box::new(Cyclic),
            ),
            context: "private context".into(),
        };
        assert_eq!(
            crate::Error::host_mcp(&error),
            crate::Error::host(Operation::Mcp, Reason::TransportFailed)
        );
    }

    #[tokio::test]
    async fn task_panics_and_cancellation_remain_distinct_without_payloads() {
        let panic = tokio::spawn(async { panic!("private panic payload") })
            .await
            .unwrap_err();
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let cancelled = task.await.unwrap_err();
        assert_eq!(
            crate::Error::host_task(Operation::Run, &panic),
            crate::Error::host(Operation::Run, Reason::TaskPanicked)
        );
        assert_eq!(
            crate::Error::host_task(Operation::Run, &cancelled),
            crate::Error::host(Operation::Run, Reason::TaskCancelled)
        );
        assert!(
            !crate::Error::host_task(Operation::Run, &panic)
                .to_string()
                .contains("private")
        );
    }

    #[test]
    fn production_host_failures_cannot_return_an_untyped_unavailable_error() {
        fn scan(path: &std::path::Path) {
            for entry in std::fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "tests") {
                        scan(&path);
                    }
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs")
                    || path.file_name().is_some_and(|name| name == "tests.rs")
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let production = source
                    .lines()
                    .take_while(|line| !(line.trim_start().starts_with("#[cfg(") && line.contains("test")));
                for line in production {
                    let compatibility_pattern = path.ends_with("runner/mod.rs") && line.trim() == "Error::Unavailable";
                    assert!(
                        compatibility_pattern
                            || !(line.contains("Error::Unavailable") || line.contains("Self::Unavailable")),
                        "untyped production host failure in {}: {line}",
                        path.display()
                    );
                }
            }
        }
        scan(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
    }
}
