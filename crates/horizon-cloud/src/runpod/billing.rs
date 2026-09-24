//! Per-worker billing history. `RunPod` bills compute and the worker's own disk
//! in whole buckets that trail the running worker. Network volumes are billed
//! per account and cannot be attributed to one worker.
use super::RunPod;
use crate::{Cancellation, CloudError, valid_id};
use serde::Deserialize;
use std::time::{Duration, SystemTime};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const INVALID_WINDOW: CloudError = CloudError::Invalid("Invalid billing window");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketSize {
    Hour,
    Day,
}

impl BucketSize {
    #[must_use]
    pub const fn duration(self) -> Duration {
        match self {
            Self::Hour => Duration::from_hours(1),
            Self::Day => Duration::from_hours(24),
        }
    }

    const fn query(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }
}

/// One worker's charges during one bucket, in US dollars.
#[derive(Clone, Debug, PartialEq)]
pub struct BillingBucket {
    /// RFC 3339 start of the bucket, verbatim from the provider.
    pub time: String,
    pub size: BucketSize,
    /// Finite and non-negative.
    pub amount: f64,
    pub time_billed_ms: u64,
}

impl RunPod {
    /// Charges of worker `pod_id` in `size` buckets between `start` and `end`,
    /// both truncated to whole seconds. Valid buckets of other workers are dropped.
    /// # Errors
    /// Rejects invalid IDs and windows, provider failures, and buckets without an
    /// RFC 3339 start or with a negative or non-finite amount.
    pub fn billing(
        &self,
        pod_id: &str,
        size: BucketSize,
        start: SystemTime,
        end: SystemTime,
        cancel: &Cancellation,
    ) -> Result<Vec<BillingBucket>, CloudError> {
        if !valid_id(pod_id) {
            return Err(CloudError::Invalid("Invalid worker ID"));
        }
        if end < start {
            return Err(INVALID_WINDOW);
        }
        let path = format!(
            "/billing/pods?bucketSize={}&startTime={}&endTime={}&grouping=podId&podId={pod_id}",
            size.query(),
            timestamp(start)?,
            timestamp(end)?,
        );
        let buckets: Vec<Bucket> = serde_json::from_value(self.request("GET", &path, None, cancel)?)
            .map_err(|_| CloudError::InvalidResponse)?;
        // Every bucket must be valid, including another worker's, before those are dropped.
        let mut own = Vec::with_capacity(buckets.len());
        for bucket in buckets {
            let mine = bucket.pod_id.as_deref().is_none_or(|id| id == pod_id);
            let bucket = bucket.validate(size)?;
            if mine {
                own.push(bucket);
            }
        }
        Ok(own)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    amount: f64,
    time: String,
    #[serde(default)]
    time_billed_ms: Option<u64>,
    #[serde(default)]
    pod_id: Option<String>,
}

impl Bucket {
    fn validate(self, size: BucketSize) -> Result<BillingBucket, CloudError> {
        if !self.amount.is_finite() || self.amount < 0.0 || OffsetDateTime::parse(&self.time, &Rfc3339).is_err() {
            return Err(CloudError::InvalidResponse);
        }
        Ok(BillingBucket {
            time: self.time,
            size,
            amount: self.amount,
            time_billed_ms: self.time_billed_ms.unwrap_or_default(),
        })
    }
}

/// RFC 3339 in UTC with whole seconds, which needs no escaping in a query.
fn timestamp(at: SystemTime) -> Result<String, CloudError> {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| INVALID_WINDOW)?
        .as_secs();
    OffsetDateTime::from_unix_timestamp(i64::try_from(seconds).map_err(|_| INVALID_WINDOW)?)
        .ok()
        .and_then(|at| at.format(&Rfc3339).ok())
        .ok_or(INVALID_WINDOW)
}
