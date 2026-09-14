use serde::{Deserialize, Serialize};

use crate::RoutineError;
use crate::origin::Origin;

const MAX_CANDIDATES: usize = 8;
const MAX_CANDIDATE_BYTES: usize = 4 * 1024;
const MAX_DIGEST_BYTES: usize = 128;
const MAX_FRAME_CHAIN: usize = 8;

/// Ranked, backend-neutral identity for one interacted element.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetFingerprint {
    pub candidates: Vec<RankedCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<u32>,
    pub frame: FrameContext,
    pub digest: String,
}

/// One ranked identity plus uniqueness evidence for that candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedCandidate {
    pub identity: TargetCandidate,
    pub match_count: u32,
    pub unique: bool,
}

/// One identity strategy. Multi-component kinds have explicit fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetCandidate {
    RoleName {
        role: String,
        name: String,
        #[serde(default)]
        reviewed: bool,
    },
    LabelControl {
        label: String,
        control: String,
        #[serde(default)]
        reviewed: bool,
    },
    TestId {
        attribute: String,
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
    UniqueId {
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
    VisibleText {
        text: String,
        context: String,
        #[serde(default)]
        reviewed: bool,
    },
    CssFallback {
        value: String,
        #[serde(default)]
        reviewed: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UniquenessEvidence {
    pub match_count: u32,
    pub unique: bool,
}

/// Browsing context that owned the target. Nested frames keep their own origin.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameContext {
    pub top_level: bool,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<FrameLink>,
}

/// Durable iframe identity. Not a session-scoped browser frame id.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameLink {
    pub origin: Origin,
    pub name: String,
}

impl TargetCandidate {
    fn reviewed(&self) -> bool {
        match self {
            Self::RoleName { reviewed, .. }
            | Self::LabelControl { reviewed, .. }
            | Self::TestId { reviewed, .. }
            | Self::UniqueId { reviewed, .. }
            | Self::VisibleText { reviewed, .. }
            | Self::CssFallback { reviewed, .. } => *reviewed,
        }
    }

    fn is_css_fallback(&self) -> bool {
        matches!(self, Self::CssFallback { .. })
    }

    fn validate_fields(&self) -> Result<(), RoutineError> {
        let fields: &[&str] = match self {
            Self::RoleName { role, name, .. } => &[role, name],
            Self::LabelControl { label, control, .. } => &[label, control],
            Self::TestId { attribute, value, .. } => &[attribute, value],
            Self::UniqueId { value, .. } | Self::CssFallback { value, .. } => &[value],
            Self::VisibleText { text, context, .. } => &[text, context],
        };
        for field in fields {
            if field.is_empty() || field.len() > MAX_CANDIDATE_BYTES || field.chars().any(char::is_control) {
                return Err(RoutineError::InvalidFingerprint);
            }
        }
        Ok(())
    }
}

impl TargetFingerprint {
    pub(crate) fn validate(&self) -> Result<(), RoutineError> {
        self.validate_structure()?;
        if !self.has_durable_candidate() {
            return Err(RoutineError::UndurableTarget);
        }
        Ok(())
    }

    pub(crate) fn validate_structure(&self) -> Result<(), RoutineError> {
        if self.candidates.is_empty() || self.candidates.len() > MAX_CANDIDATES {
            return Err(RoutineError::InvalidFingerprint);
        }
        if self.digest.is_empty() || self.digest.len() > MAX_DIGEST_BYTES || self.digest.chars().any(char::is_control) {
            return Err(RoutineError::InvalidFingerprint);
        }
        if let Some(selected) = self.selected
            && usize::try_from(selected).map_or(true, |index| index >= self.candidates.len())
        {
            return Err(RoutineError::InvalidFingerprint);
        }
        if self.frame.chain.len() > MAX_FRAME_CHAIN
            || self.frame.chain.iter().any(|frame| {
                frame.name.is_empty() || frame.name.len() > MAX_DIGEST_BYTES || frame.name.chars().any(char::is_control)
            })
        {
            return Err(RoutineError::InvalidFingerprint);
        }
        if self.frame.top_level != self.frame.chain.is_empty() {
            return Err(RoutineError::InvalidFingerprint);
        }
        for candidate in &self.candidates {
            if candidate.match_count == 0 || candidate.unique != (candidate.match_count == 1) {
                return Err(RoutineError::InvalidFingerprint);
            }
            candidate.identity.validate_fields()?;
        }
        Ok(())
    }

    #[must_use]
    pub fn has_durable_candidate(&self) -> bool {
        self.candidates
            .iter()
            .any(|candidate| !candidate.identity.is_css_fallback() || candidate.identity.reviewed())
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameContext, FrameLink, RankedCandidate, TargetCandidate, TargetFingerprint};
    use crate::RoutineError;
    use crate::origin::Origin;

    fn fingerprint() -> TargetFingerprint {
        TargetFingerprint {
            candidates: vec![RankedCandidate {
                identity: TargetCandidate::RoleName {
                    role: "button".to_string(),
                    name: "Generate report".to_string(),
                    reviewed: false,
                },
                match_count: 1,
                unique: true,
            }],
            selected: Some(0),
            frame: FrameContext {
                top_level: true,
                origin: Origin::parse("https://reports.example").expect("origin"),
                chain: Vec::new(),
            },
            digest: "dig-1".to_string(),
        }
    }

    #[test]
    fn duplicate_text_is_recorded_as_non_unique() {
        let mut target = fingerprint();
        target.candidates[0].match_count = 2;
        target.candidates[0].unique = false;
        assert_eq!(target.validate(), Ok(()));
    }

    #[test]
    fn css_fallback_alone_is_not_durable_unless_reviewed() {
        let mut target = fingerprint();
        target.candidates = vec![RankedCandidate {
            identity: TargetCandidate::CssFallback {
                value: "div > span:nth-child(3)".to_string(),
                reviewed: false,
            },
            match_count: 1,
            unique: true,
        }];
        assert_eq!(target.validate(), Err(RoutineError::UndurableTarget));
        let TargetCandidate::CssFallback { reviewed, .. } = &mut target.candidates[0].identity else {
            panic!("css");
        };
        *reviewed = true;
        assert_eq!(target.validate(), Ok(()));
    }

    #[test]
    fn nested_frame_keeps_its_own_origin() {
        let mut target = fingerprint();
        target.frame = FrameContext {
            top_level: false,
            origin: Origin::parse("https://idp.example").expect("frame origin"),
            chain: vec![FrameLink {
                origin: Origin::parse("https://idp.example").expect("frame origin"),
                name: "login".to_string(),
            }],
        };
        assert_eq!(target.validate(), Ok(()));
        let encoded = serde_json::to_value(&target).expect("encode");
        assert_eq!(encoded["frame"]["origin"], "https://idp.example");
        assert_eq!(encoded["frame"]["top_level"], false);
        assert_eq!(encoded["candidates"][0]["identity"]["kind"], "role_name");
        assert_eq!(encoded["candidates"][0]["identity"]["role"], "button");
        assert_eq!(encoded["candidates"][0]["identity"]["name"], "Generate report");
        assert_eq!(encoded["frame"]["chain"][0]["name"], "login");
    }

    #[test]
    fn unknown_fingerprint_fields_are_rejected() {
        let err = serde_json::from_str::<TargetFingerprint>(
            r#"{"candidates":[],"uniqueness":{"match_count":1,"unique":true},"frame":{"top_level":true,"origin":"https://example.test"},"digest":"x","extra":1}"#,
        );
        assert!(err.is_err());
    }
}
