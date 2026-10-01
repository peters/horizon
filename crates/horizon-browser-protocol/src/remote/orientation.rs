//! Normalized device orientation, independent of the provider adapter.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOrientation {
    Portrait,
    Landscape,
}

impl RemoteOrientation {
    pub const DEFAULT_TIMEOUT_MILLIS: u64 = 15_000;
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }

    #[must_use]
    pub const fn webdriver_value(self) -> &'static str {
        match self {
            Self::Portrait => "PORTRAIT",
            Self::Landscape => "LANDSCAPE",
        }
    }

    #[must_use]
    pub fn from_driver(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("portrait") {
            Some(Self::Portrait)
        } else if value.eq_ignore_ascii_case("landscape") {
            Some(Self::Landscape)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrientationSupport {
    Supported,
    Unsupported,
    #[default]
    Unverified,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct RemoteOrientationState {
    pub support: OrientationSupport,
    pub applied: Option<RemoteOrientation>,
}

/// Runtime presentation shared by desktop and cloud hosts.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct RemoteOrientationView {
    #[serde(default)]
    pub action_id: Option<String>,
    #[serde(default)]
    pub completed: Vec<RemoteOrientationCompletion>,
    pub state: RemoteOrientationState,
    pub pending: Option<RemoteOrientation>,
    pub error: Option<String>,
}

/// Bounded terminal acknowledgements survive latest-only cloud polling.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct RemoteOrientationCompletion {
    pub action_id: String,
    pub error: Option<String>,
}
impl RemoteOrientationView {
    pub fn record_completion(&mut self, completion: RemoteOrientationCompletion) {
        self.completed.retain(|entry| entry.action_id != completion.action_id);
        self.completed.push(completion);
        if self.completed.len() > 64 {
            self.completed.remove(0);
        }
    }
}
