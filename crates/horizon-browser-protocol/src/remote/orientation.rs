//! Normalized device orientation, independent of the provider adapter.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOrientation {
    Portrait,
    Landscape,
}

impl RemoteOrientation {
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
