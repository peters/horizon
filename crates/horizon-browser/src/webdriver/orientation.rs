//! Remote orientation negotiation and page acknowledgement through classic transport.
use super::{http::HttpError, transport::ClassicTransport};
use crate::BrowserControlFailure;
use crate::remote::{OrientationSupport, RemoteOrientation, RemoteOrientationState};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

const SAMPLE_SCRIPT: &str = "return {width:innerWidth,height:innerHeight,visual_width:window.visualViewport?.width ?? innerWidth,visual_height:window.visualViewport?.height ?? innerHeight,orientation:screen.orientation?.type ?? (typeof window.orientation === 'number' ? (Math.abs(window.orientation)%180 === 90 ? 'landscape' : 'portrait') : null)};";

pub(super) fn unsupported(error: &HttpError) -> bool {
    matches!(error, HttpError::WebDriver { error, .. } if matches!(error.as_str(), "unknown command" | "unsupported operation" | "unknown method"))
}

/// Allocation discovers endpoint support without measuring its temporary page.
pub(super) fn probe_support(transport: &dyn ClassicTransport, session: &str) -> RemoteOrientationState {
    let support = match transport.get_with_read_timeout(&format!("{session}/orientation"), Duration::from_secs(3)) {
        Ok(value)
            if value["value"]
                .as_str()
                .and_then(RemoteOrientation::from_driver)
                .is_some() =>
        {
            OrientationSupport::Supported
        }
        Err(error) if unsupported(&error) => OrientationSupport::Unsupported,
        _ => OrientationSupport::Unverified,
    };
    RemoteOrientationState { support, applied: None }
}

pub(super) fn probe(transport: &dyn ClassicTransport, session: &str) -> RemoteOrientationState {
    let deadline = Instant::now() + Duration::from_secs(3);
    match transport.get_with_read_timeout(&format!("{session}/orientation"), Duration::from_secs(3)) {
        Ok(value) => {
            let device = value["value"].as_str().and_then(RemoteOrientation::from_driver);
            let applied = device.filter(|orientation| {
                measure_page(transport, session, deadline).is_ok_and(|page| page.confirms(*orientation))
            });
            RemoteOrientationState {
                support: if device.is_some() {
                    OrientationSupport::Supported
                } else {
                    OrientationSupport::Unverified
                },
                applied,
            }
        }
        Err(error) => RemoteOrientationState {
            support: if unsupported(&error) {
                OrientationSupport::Unsupported
            } else {
                OrientationSupport::Unverified
            },
            applied: None,
        },
    }
}

pub(super) fn remaining(deadline: Instant) -> Result<Duration, BrowserControlFailure> {
    deadline.checked_duration_since(Instant::now()).filter(|duration| !duration.is_zero()).ok_or_else(|| BrowserControlFailure::new("orientation_timeout", "orientation was not acknowledged within the deadline; the device may already have rotated, inspect before retrying"))
}

#[derive(Debug, Deserialize)]
pub(super) struct PageMeasurement {
    pub(super) width: u32,
    pub(super) height: u32,
    visual_width: f64,
    visual_height: f64,
    orientation: Option<String>,
}
impl PageMeasurement {
    pub(super) fn confirms(&self, requested: RemoteOrientation) -> bool {
        let page = self
            .orientation
            .as_deref()
            .and_then(|value| value.split('-').next().and_then(RemoteOrientation::from_driver));
        let landscape = requested == RemoteOrientation::Landscape;
        self.width > 0
            && self.height > 0
            && self.width != self.height
            && self.visual_width.is_finite()
            && self.visual_height.is_finite()
            && self.visual_width > 0.0
            && self.visual_height > 0.0
            && (self.visual_width - self.visual_height).abs() > f64::EPSILON
            && (self.width > self.height) == landscape
            && (self.visual_width > self.visual_height) == landscape
            && (self.orientation.is_none() || page == Some(requested))
    }
}

pub(super) fn observe(
    transport: &dyn ClassicTransport,
    session: &str,
    requested: RemoteOrientation,
    deadline: Instant,
) -> Result<Option<PageMeasurement>, BrowserControlFailure> {
    let device = transport
        .get_with_read_timeout(
            &format!("{session}/orientation"),
            remaining(deadline)?.min(Duration::from_secs(3)),
        )
        .map_err(|error| protocol_failure(&error))?;
    let applied = device["value"]
        .as_str()
        .and_then(RemoteOrientation::from_driver)
        .ok_or_else(|| {
            BrowserControlFailure::new(
                "orientation_unverified",
                "the endpoint did not return a valid device orientation",
            )
        })?;
    if applied != requested {
        return Ok(None);
    }
    let measured = measure_page(transport, session, deadline)?;
    Ok(measured.confirms(requested).then_some(measured))
}

fn measure_page(
    transport: &dyn ClassicTransport,
    session: &str,
    deadline: Instant,
) -> Result<PageMeasurement, BrowserControlFailure> {
    let page = transport
        .post_with_read_timeout(
            &format!("{session}/execute/sync"),
            &json!({"script": SAMPLE_SCRIPT,"args":[]}),
            remaining(deadline)?.min(Duration::from_secs(3)),
        )
        .map_err(|error| protocol_failure(&error))?;
    let measured: PageMeasurement = serde_json::from_value(page["value"].clone()).map_err(|_| {
        BrowserControlFailure::new(
            "orientation_unverified",
            "the page did not return trustworthy orientation geometry",
        )
    })?;
    remaining(deadline)?;
    if !measured.confirms(RemoteOrientation::Portrait) && !measured.confirms(RemoteOrientation::Landscape) {
        return Err(BrowserControlFailure::new(
            "orientation_unverified",
            "the page did not return consistent orientation geometry",
        ));
    }
    Ok(measured)
}

pub(super) fn set(
    transport: &dyn ClassicTransport,
    session: &str,
    orientation: RemoteOrientation,
    deadline: Instant,
) -> Result<(), BrowserControlFailure> {
    let result = transport.post_with_read_timeout(
        &format!("{session}/orientation"),
        &json!({"orientation":orientation.webdriver_value()}),
        remaining(deadline)?,
    );
    remaining(deadline)?;
    result.map(|_| ()).map_err(|error| protocol_failure(&error))
}
fn protocol_failure(error: &HttpError) -> BrowserControlFailure {
    if unsupported(error) {
        BrowserControlFailure::new(
            "orientation_unsupported",
            "this endpoint does not support device orientation",
        )
    } else {
        BrowserControlFailure::new(
            "orientation_unverified",
            "the endpoint did not confirm orientation; the device may already have rotated, inspect before retrying",
        )
    }
}

#[cfg(test)]
mod tests;
