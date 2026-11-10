//! A click at one point of the page.
//!
//! A caller names a point in CSS pixels of the page, and the engine refuses a
//! point that the page viewport does not hold. The panel draws that viewport, so
//! a point outside it names the chrome of the browser or one pixel that belongs
//! to another panel, and it is never the page's.
//!
//! The click itself is not a new engine capability: it is one trusted press and
//! one trusted release for each click, through the input path that a person's
//! pointer already uses.

use crate::BrowserControlFailure;
use horizon_browser_protocol::{BrowserButton, BrowserInput, BrowserModifiers};

/// The size of the page viewport, in CSS pixels, as the engine last drew it.
pub(crate) type PageViewport = (u32, u32);

/// Refuse a point that the page viewport does not hold.
///
/// A negative point and a point that is not a finite number are refused without
/// a viewport, because no page holds them. A viewport that the engine has not
/// measured yet takes every other point: the page decides where a click lands,
/// and a panel that has drawn no frame has nothing to compare against.
pub(crate) fn check_point(page: Option<PageViewport>, x: f64, y: f64) -> Result<(), BrowserControlFailure> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err(BrowserControlFailure::new(
            "invalid_input",
            "a click point must be a finite, non-negative number of CSS pixels",
        ));
    }
    let Some((width, height)) = page else {
        return Ok(());
    };
    // A click exactly on the right or bottom edge is outside the viewport.
    if x >= f64::from(width) || y >= f64::from(height) {
        return Err(BrowserControlFailure::new(
            "invalid_input",
            format!("the click point ({x}, {y}) is outside the page viewport of {width} by {height} CSS pixels"),
        ));
    }
    Ok(())
}

/// One pressed and one released left button at `(x, y)`, in the order that the
/// page expects them.
pub(crate) fn click_inputs(x: f64, y: f64, click_count: u32) -> [BrowserInput; 2] {
    [
        BrowserInput::MousePress {
            x,
            y,
            button: BrowserButton::Left,
            click_count,
            buttons: 1,
            modifiers: BrowserModifiers::none(),
        },
        BrowserInput::MouseRelease {
            x,
            y,
            button: BrowserButton::Left,
            click_count,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_inside_the_viewport_is_accepted() {
        assert!(check_point(Some((800, 600)), 0.0, 0.0).is_ok());
        assert!(check_point(Some((800, 600)), 799.5, 599.5).is_ok());
    }

    #[test]
    fn a_point_outside_the_viewport_is_refused_with_the_point_and_the_size() {
        let error = check_point(Some((800, 600)), 800.0, 10.0).expect_err("the right edge is outside");
        assert_eq!(error.code, "invalid_input");
        assert!(error.message.contains("800"), "{}", error.message);
        assert!(error.message.contains("600"), "{}", error.message);
        assert!(check_point(Some((800, 600)), 10.0, 600.0).is_err());
        assert!(check_point(Some((800, 600)), -1.0, 10.0).is_err());
        assert!(check_point(Some((800, 600)), 10.0, f64::NAN).is_err());
    }

    #[test]
    fn a_viewport_that_was_never_measured_takes_the_point() {
        assert!(check_point(None, 10_000.0, 10_000.0).is_ok());
    }

    #[test]
    fn a_point_that_no_page_holds_is_refused_without_a_viewport() {
        assert!(check_point(None, -1.0, 10.0).is_err(), "a negative point is nowhere");
        assert!(
            check_point(None, 10.0, f64::INFINITY).is_err(),
            "a point must be finite"
        );
    }

    #[test]
    fn a_click_is_one_press_and_one_release_at_the_point() {
        let [press, release] = click_inputs(12.0, 18.0, 2);
        assert_eq!(
            press,
            BrowserInput::MousePress {
                x: 12.0,
                y: 18.0,
                button: BrowserButton::Left,
                click_count: 2,
                buttons: 1,
                modifiers: BrowserModifiers::none(),
            }
        );
        assert_eq!(
            release,
            BrowserInput::MouseRelease {
                x: 12.0,
                y: 18.0,
                button: BrowserButton::Left,
                click_count: 2,
                buttons: 0,
                modifiers: BrowserModifiers::none(),
            }
        );
    }
}
