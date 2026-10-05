use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::contract::{Platform, identifier, printable, yaml_blocks};
use crate::{Error, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub version: u32,
    pub id: String,
    pub platforms: Option<Vec<Platform>>,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Tap {
        target: Target,
    },
    LongPress {
        target: Target,
        duration_millis: u64,
    },
    Type {
        target: Target,
        text: String,
    },
    Clear {
        target: Target,
    },
    Swipe {
        from: Point,
        to: Point,
        duration_millis: u64,
    },
    Scroll {
        direction: Direction,
        distance: u32,
    },
    Wait {
        target: Target,
        state: State,
        timeout_millis: u64,
    },
    Assert {
        target: Target,
        state: State,
    },
    Back {},
    Home {},
    Rotate {
        orientation: Orientation,
    },
    Launch {},
    Terminate {},
    Reset {},
    DeepLink {
        url: String,
    },
    Screenshot {},
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "by", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Identifier(String),
    Label(String),
    Ref(String),
    Coordinates(Point),
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Visible,
    Hidden,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(rename = "device-recipe")]
    recipe: Recipe,
}

impl Recipe {
    /// # Errors
    /// Recipes must have a single executable YAML fence; prose alone cannot be reported as passing.
    pub fn from_markdown(markdown: &str) -> Result<Self> {
        if markdown.len() > 1024 * 1024 {
            return Err(Error::RecipeInvalid);
        }
        let blocks = yaml_blocks(markdown, "device-recipe:").map_err(|_| Error::RecipeInvalid)?;
        let [block] = blocks.as_slice() else {
            return Err(Error::RecipeInvalid);
        };
        let envelope: Envelope = serde_yaml::from_str(block).map_err(|_| Error::RecipeInvalid)?;
        envelope.recipe.validate()?;
        Ok(envelope.recipe)
    }

    /// # Errors
    /// Rejects unbounded waits, duplicate steps, unknown actions and invalid targets.
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || !identifier(&self.id)
            || self.steps.is_empty()
            || self.steps.len() > 1024
            || self.platforms.as_ref().is_some_and(|p| p.is_empty() || p.len() > 2)
        {
            return Err(Error::RecipeInvalid);
        }
        let mut seen = BTreeSet::new();
        for step in &self.steps {
            if !identifier(&step.id) || !seen.insert(&step.id) {
                return Err(Error::RecipeInvalid);
            }
            step.action.validate()?;
        }
        Ok(())
    }
}

impl Action {
    /// # Errors
    /// Validates bounded native actions before the session dispatches them.
    pub fn validate(&self) -> Result<()> {
        let target = match self {
            Self::Tap { target } | Self::Clear { target } | Self::Assert { target, .. } => Some(target),
            Self::LongPress {
                target,
                duration_millis,
            } => {
                if !(100..=10_000).contains(duration_millis) {
                    return Err(Error::RecipeInvalid);
                }
                Some(target)
            }
            Self::Type { target, text } => {
                if text.len() > 16 * 1024 {
                    return Err(Error::RecipeInvalid);
                }
                Some(target)
            }
            Self::Wait {
                target, timeout_millis, ..
            } => {
                if !(1..=60_000).contains(timeout_millis) {
                    return Err(Error::RecipeInvalid);
                }
                Some(target)
            }
            Self::Swipe {
                from,
                to,
                duration_millis,
            } => {
                if !from.valid() || !to.valid() || !(100..=10_000).contains(duration_millis) {
                    return Err(Error::RecipeInvalid);
                }
                None
            }
            Self::DeepLink { url } => {
                if !printable(url, 4096) || !url.contains("://") {
                    return Err(Error::RecipeInvalid);
                }
                None
            }
            Self::Scroll { distance, .. } => {
                if !(1..=16_384).contains(distance) {
                    return Err(Error::RecipeInvalid);
                }
                None
            }
            _ => None,
        };
        if let Some(target) = target {
            target.validate()?;
        }
        Ok(())
    }
}

impl Target {
    fn validate(&self) -> Result<()> {
        let valid = match self {
            Self::Identifier(s) | Self::Label(s) => printable(s, 512),
            Self::Ref(s) => identifier(s),
            Self::Coordinates(p) => p.valid(),
        };
        if valid { Ok(()) } else { Err(Error::RecipeInvalid) }
    }
}

impl Point {
    const fn valid(self) -> bool {
        self.x <= 16_384 && self.y <= 16_384
    }
}
