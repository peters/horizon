//! The two forms of a card body.
//!
//! A component draws its narrow form while the body is narrower than
//! [`WIDE_FORM_MIN_WIDTH`] and its wide form from there on. The form follows the
//! real width of the body, so it changes when the person resizes the card,
//! whatever the document asked for.

use super::metrics::WIDE_FORM_MIN_WIDTH;

/// The form that a body draws its components in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    /// The card of today: one column of components.
    Narrow,
    /// A board of waves and workers, a callout row and columns of a log.
    Wide,
}

impl Form {
    /// The form of a body that is `width` points wide.
    pub(crate) fn of(width: f32) -> Self {
        if width >= WIDE_FORM_MIN_WIDTH {
            Self::Wide
        } else {
            Self::Narrow
        }
    }

    /// Whether this is the wide form.
    pub(crate) const fn is_wide(self) -> bool {
        matches!(self, Self::Wide)
    }
}

/// How the buttons of a component sit in the room that the component gives
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answers {
    /// One row: the ordinary buttons from the left, a destructive one at the
    /// right edge of the room.
    Row,
    /// One button per line, and every line starts at the left edge of the room.
    Stack,
}

#[cfg(test)]
mod tests {
    use super::Form;

    #[test]
    fn the_form_changes_at_the_threshold_of_the_wide_card() {
        assert_eq!(Form::of(999.0), Form::Narrow);
        assert_eq!(Form::of(1000.0), Form::Wide);
        assert_eq!(Form::of(2400.0), Form::Wide);
        assert!(!Form::Narrow.is_wide());
    }
}
