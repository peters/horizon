use std::collections::HashSet;

use horizon_core::{AgentSessionCatalog, RuntimeState};

pub(in crate::app) struct StartupBootstrap {
    pub(in crate::app) runtime_state: RuntimeState,
    pub(in crate::app) session_catalog: AgentSessionCatalog,
    pub(in crate::app) runtime_state_changed: bool,
    /// The live Claude conversations that the bootstrap thread read, so the
    /// answer of a person reuses them instead of reading the folder again.
    pub(in crate::app) busy_claude_session_ids: HashSet<String>,
}

pub(in crate::app) struct StartupBootstrapValidationFailure {
    pub(in crate::app) runtime_state: RuntimeState,
    pub(in crate::app) message: String,
    pub(in crate::app) unavailable_exact_session_ids: HashSet<String>,
    pub(in crate::app) all_exact_session_ids: bool,
    pub(in crate::app) runtime_state_changed: bool,
    /// The live Claude conversations that the bootstrap thread read.
    pub(in crate::app) busy_claude_session_ids: HashSet<String>,
}

pub(in crate::app) enum StartupBootstrapOutcome {
    Ready(Box<StartupBootstrap>),
    ExactValidationFailed(Box<StartupBootstrapValidationFailure>),
}

pub(in crate::app) enum StartupBootstrapFailure {
    ExactValidationFailed {
        message: String,
        unavailable_exact_session_ids: HashSet<String>,
        all_exact_session_ids: bool,
    },
    WorkerDisconnected,
    RecoverySaveFailed {
        message: String,
    },
}
