//! Bounded enrollment evidence retained in the owning cloud allocation record.
use horizon_cloud::provider::{self, Kind};
use horizon_cloud_protocol::OperationId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("Invalid tailnet enrollment identity")]
    Identity,
    #[error("Tailnet enrollment evidence exceeded its bound")]
    Bound,
    #[error("Conflicting tailnet enrollment evidence")]
    Conflict,
    #[error("Tailnet capture attempt was not recorded")]
    MissingAttempt,
}

macro_rules! identity {
    ($name:ident, $validate:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// # Errors
            /// Rejects an empty, malformed or oversized identity.
            pub fn new(value: &str) -> Result<Self, Error> {
                if ($validate)(value) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(Error::Identity)
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;

            fn try_from(value: String) -> Result<Self, Error> {
                if ($validate)(&value) {
                    Ok(Self(value))
                } else {
                    Err(Error::Identity)
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

identity!(TailnetId, horizon_cloud::tailnet::valid_id);
identity!(WorkerId, horizon_cloud::valid_id);
identity!(AllocationRequestId, horizon_cloud::valid_id);
// Device IDs are opaque. They must later be encoded as one URL path segment.
identity!(DeviceId, |value: &str| !value.is_empty()
    && value.len() <= 256
    && value.bytes().all(|byte| byte.is_ascii_graphic()));

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnrollmentProvider(Kind);

impl EnrollmentProvider {
    #[must_use]
    pub const fn new(kind: Kind) -> Self {
        Self(kind)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            Kind::RunPod => provider::RUNPOD.id,
            Kind::Hetzner => provider::HETZNER.id,
            Kind::DockerHost => provider::DOCKER_HOST.id,
        }
    }
}

impl TryFrom<String> for EnrollmentProvider {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Error> {
        provider::by_id(&value)
            .map(|value| Self(value.kind))
            .ok_or(Error::Identity)
    }
}

impl From<EnrollmentProvider> for String {
    fn from(value: EnrollmentProvider) -> Self {
        value.as_str().into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAttempt {
    pub attempt_id: OperationId,
    pub tailnet_id: TailnetId,
    pub provider: EnrollmentProvider,
    pub worker_id: WorkerId,
    pub allocation_request_id: AllocationRequestId,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentReceipt {
    pub tailnet_id: TailnetId,
    pub provider: EnrollmentProvider,
    pub worker_id: WorkerId,
    pub allocation_request_id: AllocationRequestId,
    pub device_id: DeviceId,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub node_deleted: bool,
}

/// A retained device is an explicit manual outcome until desktop cleanup is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualCleanupReason {
    DeviceIdentityUnavailable,
    ApiCredentialMissing,
    Offline,
}

impl ManualCleanupReason {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::DeviceIdentityUnavailable => {
                "The worker device ID was not recorded. Check this cloud's device in the saved tailnet and remove it manually; automatic cleanup cannot identify it."
            }
            Self::ApiCredentialMissing => {
                "Tailnet node cleanup is pending: the saved tailnet has no API credential. Remove the recorded device manually or retry after configuring desktop API access."
            }
            Self::Offline => {
                "Tailnet node cleanup is pending because the API is offline. Keep the recorded device ID and retry, or remove it manually."
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "StoredLedger")]
pub struct EnrollmentLedger {
    receipts: Vec<EnrollmentReceipt>,
    attempts: Vec<CaptureAttempt>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredLedger {
    #[serde(default, deserialize_with = "entries")]
    receipts: Vec<EnrollmentReceipt>,
    #[serde(default, deserialize_with = "entries")]
    attempts: Vec<CaptureAttempt>,
}

fn entries<'de, T: Deserialize<'de>, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<T>, D::Error> {
    struct Entries<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Entries<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("bounded tailnet enrollment entries")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(value) = sequence.next_element()? {
                if values.len() == MAX_ENTRIES {
                    return Err(serde::de::Error::custom(Error::Bound));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    decoder.deserialize_seq(Entries(std::marker::PhantomData))
}

impl TryFrom<StoredLedger> for EnrollmentLedger {
    type Error = Error;

    fn try_from(value: StoredLedger) -> Result<Self, Error> {
        let ledger = Self {
            receipts: value.receipts,
            attempts: value.attempts,
        };
        ledger.validate()?;
        Ok(ledger)
    }
}

impl EnrollmentLedger {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty() && self.attempts.is_empty()
    }

    #[must_use]
    pub fn receipts(&self) -> &[EnrollmentReceipt] {
        &self.receipts
    }

    #[must_use]
    pub fn attempts(&self) -> &[CaptureAttempt] {
        &self.attempts
    }

    #[must_use]
    pub fn cleanup_pending(&self) -> bool {
        !self.attempts.is_empty() || self.receipts.iter().any(|entry| !entry.node_deleted)
    }

    /// Retain the exact identity after a confirmed API response.
    /// # Errors
    /// Rejects stale/foreign receipts or oversized confirmations without changing evidence.
    pub fn confirm_removed(&mut self, receipt: &EnrollmentReceipt) -> Result<(), Error> {
        if !self.receipts.contains(receipt) {
            return Err(Error::Conflict);
        }
        let mut next = self.clone();
        for current in &mut next.receipts {
            if current.tailnet_id == receipt.tailnet_id && current.device_id == receipt.device_id {
                current.node_deleted = true;
            }
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// The caller must save the deployment under its ownership lock before enrollment.
    /// # Errors
    /// Refuses reusing an attempt ID for another worker or exceeding the ledger bounds.
    pub fn arm(&mut self, attempt: CaptureAttempt) -> Result<(), Error> {
        if let Some(prior) = self
            .attempts
            .iter()
            .find(|prior| prior.attempt_id == attempt.attempt_id)
        {
            return if prior == &attempt {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let mut next = self.clone();
        next.attempts.push(attempt);
        next.validate()?;
        *self = next;
        Ok(())
    }

    /// Record a trusted observation, retaining previous devices and unresolved attempts.
    /// The caller must save before permitting sessions or readiness.
    /// # Errors
    /// Requires the exact armed attempt and refuses oversized evidence without mutation.
    pub fn record(&mut self, attempt_id: OperationId, device_id: DeviceId) -> Result<(), Error> {
        let mut next = self.clone();
        let index = next
            .attempts
            .iter()
            .position(|attempt| attempt.attempt_id == attempt_id)
            .ok_or(Error::MissingAttempt)?;
        let attempt = next.attempts.remove(index);
        let receipt = EnrollmentReceipt {
            tailnet_id: attempt.tailnet_id,
            provider: attempt.provider,
            worker_id: attempt.worker_id,
            allocation_request_id: attempt.allocation_request_id,
            device_id,
            node_deleted: false,
        };
        if !next.receipts.iter().any(|prior| {
            prior.tailnet_id == receipt.tailnet_id
                && prior.provider == receipt.provider
                && prior.worker_id == receipt.worker_id
                && prior.allocation_request_id == receipt.allocation_request_id
                && prior.device_id == receipt.device_id
        }) {
            next.receipts.push(receipt);
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.receipts.len() + self.attempts.len() > MAX_ENTRIES
            || serde_json::to_vec(self).map_err(|_| Error::Bound)?.len() > MAX_BYTES
        {
            return Err(Error::Bound);
        }
        let mut attempts = BTreeSet::new();
        let mut receipts = BTreeSet::new();
        if self.attempts.iter().any(|attempt| !attempts.insert(attempt.attempt_id))
            || self.receipts.iter().any(|receipt| {
                !receipts.insert((
                    &receipt.tailnet_id,
                    receipt.provider.as_str(),
                    &receipt.worker_id,
                    &receipt.allocation_request_id,
                    &receipt.device_id,
                ))
            })
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::cloud_runtime) mod tests {
    use super::*;

    fn attempt(worker: &str) -> CaptureAttempt {
        CaptureAttempt {
            attempt_id: OperationId::generate(),
            tailnet_id: TailnetId::new("saved-test-network").unwrap(),
            provider: EnrollmentProvider::new(Kind::RunPod),
            worker_id: WorkerId::new(worker).unwrap(),
            allocation_request_id: AllocationRequestId::new("synthetic-allocation").unwrap(),
        }
    }

    pub(in crate::cloud_runtime) fn populated() -> EnrollmentLedger {
        let mut ledger = EnrollmentLedger::default();
        let joined = attempt("old-worker");
        ledger.arm(joined.clone()).unwrap();
        ledger
            .record(joined.attempt_id, DeviceId::new("nSyntheticOnly").unwrap())
            .unwrap();
        ledger.arm(attempt("unknown-worker")).unwrap();
        ledger
    }

    #[test]
    fn receipt_retries_deduplicate_only_the_exact_device_and_preserve_unknown_attempts() {
        let mut ledger = populated();
        let unknown = ledger.attempts().to_vec();
        for (worker, device, count) in [
            ("old-worker", "nSyntheticOnly", 1),
            ("old-worker", "nDifferent", 2),
            ("new-worker", "nSyntheticOnly", 3),
        ] {
            let current = attempt(worker);
            ledger.arm(current.clone()).unwrap();
            ledger.arm(current.clone()).unwrap();
            ledger
                .record(current.attempt_id, DeviceId::new(device).unwrap())
                .unwrap();
            assert_eq!(ledger.receipts().len(), count);
            assert_eq!(ledger.attempts(), unknown);
        }
        let mut local = attempt("docker-worker");
        local.provider = EnrollmentProvider::new(Kind::DockerHost);
        ledger.arm(local.clone()).unwrap();
        ledger
            .record(local.attempt_id, DeviceId::new("nDockerSelected").unwrap())
            .unwrap();
        assert_eq!(ledger.receipts().last().unwrap().provider.as_str(), "docker-host");
        let bytes = serde_json::to_vec(&ledger).unwrap();
        assert_eq!(serde_json::from_slice::<EnrollmentLedger>(&bytes).unwrap(), ledger);
    }

    #[test]
    fn conflicting_attempt_or_unarmed_observation_never_changes_persisted_evidence() {
        let mut ledger = populated();
        let before = ledger.clone();
        let mut conflicting = ledger.attempts()[0].clone();
        conflicting.worker_id = WorkerId::new("other-worker").unwrap();
        assert_eq!(ledger.arm(conflicting), Err(Error::Conflict));
        assert_eq!(
            ledger.record(OperationId::generate(), DeviceId::new("opaque").unwrap()),
            Err(Error::MissingAttempt)
        );
        assert_eq!(ledger, before);
    }

    #[test]
    fn enrollment_identity_fields_are_bounded_without_interpreting_device_ids() {
        for value in ["", "../outside", "has space", &"a".repeat(65)] {
            assert!(TailnetId::new(value).is_err());
        }
        for value in ["", "../outside", "has space", &"a".repeat(101)] {
            assert!(WorkerId::new(value).is_err());
        }
        for value in ["", "has space", "control\n", "nonascii-ø", &"a".repeat(257)] {
            assert!(DeviceId::new(value).is_err());
        }
        for value in ["opaque", "nSyntheticOnly", "123", "a/b%?", &"a".repeat(256)] {
            let id = DeviceId::new(value).unwrap();
            assert_eq!(id.as_str(), value);
            assert_eq!(
                serde_json::from_str::<DeviceId>(&serde_json::to_string(&id).unwrap()).unwrap(),
                id
            );
        }
        assert!(serde_json::from_str::<DeviceId>("123").is_err());
        assert!(serde_json::from_str::<EnrollmentProvider>("\"unsupported\"").is_err());
        for kind in [Kind::RunPod, Kind::Hetzner, Kind::DockerHost] {
            let provider = EnrollmentProvider::new(kind);
            assert_eq!(
                serde_json::from_str::<EnrollmentProvider>(&serde_json::to_string(&provider).unwrap()).unwrap(),
                provider
            );
        }
    }

    #[test]
    fn row_and_encoded_byte_caps_fail_without_discarding_existing_evidence() {
        let mut ledger = EnrollmentLedger::default();
        for _ in 0..MAX_ENTRIES {
            ledger.arm(attempt("worker")).unwrap();
        }
        let before = ledger.clone();
        assert_eq!(ledger.arm(attempt("worker")), Err(Error::Bound));
        assert_eq!(ledger, before);
        let mut oversized = serde_json::to_value(&ledger).unwrap();
        let duplicate = oversized["attempts"][0].clone();
        oversized["attempts"].as_array_mut().unwrap().push(duplicate);
        assert!(serde_json::from_value::<EnrollmentLedger>(oversized).is_err());

        let mut ledger = EnrollmentLedger::default();
        let mut exceeded = false;
        for index in 0..MAX_ENTRIES {
            let mut current = attempt(&format!("{}-{index}", "w".repeat(95)));
            current.tailnet_id = TailnetId::new(&"t".repeat(64)).unwrap();
            ledger.arm(current).unwrap();
        }
        for current in ledger.attempts().to_vec() {
            let before = ledger.clone();
            if ledger.record(current.attempt_id, DeviceId::new(&"\"".repeat(256)).unwrap()) == Err(Error::Bound) {
                assert_eq!(ledger, before);
                exceeded = true;
                break;
            }
        }
        assert!(exceeded, "encoded byte cap applies before the row cap");
    }

    #[test]
    fn allocation_request_is_retained_and_manual_reasons_are_distinct() {
        let mut ledger = populated();
        let mut current = attempt("old-worker");
        current.allocation_request_id = AllocationRequestId::new("replacement-allocation").unwrap();
        ledger.arm(current.clone()).unwrap();
        ledger
            .record(current.attempt_id, DeviceId::new("nSyntheticOnly").unwrap())
            .unwrap();
        assert_eq!(ledger.receipts().len(), 2);
        assert_eq!(
            ledger.receipts()[1].allocation_request_id,
            current.allocation_request_id
        );
        for (reason, code, text) in [
            (
                ManualCleanupReason::DeviceIdentityUnavailable,
                "device_identity_unavailable",
                "not recorded",
            ),
            (
                ManualCleanupReason::ApiCredentialMissing,
                "api_credential_missing",
                "no API credential",
            ),
            (ManualCleanupReason::Offline, "offline", "offline"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), code);
            assert!(reason.message().contains(text));
            assert!(reason.message().contains("manually"));
        }
    }

    #[test]
    fn unknown_duplicate_and_null_metadata_are_refused() {
        for value in [
            serde_json::json!({"receipts":[],"attempts":[],"token":"synthetic"}),
            serde_json::json!({"receipts":null}),
            serde_json::json!({"attempts":null}),
        ] {
            assert!(serde_json::from_value::<EnrollmentLedger>(value).is_err());
        }
        let original = populated();
        for field in ["receipts", "attempts"] {
            let mut value = serde_json::to_value(&original).unwrap();
            let duplicate = value[field][0].clone();
            value[field].as_array_mut().unwrap().push(duplicate);
            assert!(serde_json::from_value::<EnrollmentLedger>(value).is_err());
        }
        let mut value = serde_json::to_value(&original).unwrap();
        value["receipts"][0]["hostname"] = serde_json::json!("display-only");
        assert!(serde_json::from_value::<EnrollmentLedger>(value).is_err());
        assert!(serde_json::from_str::<EnrollmentLedger>("{\"receipts\":[],\"receipts\":[]}").is_err());
    }

    #[test]
    fn confirmed_receipt_round_trips_without_losing_identity_or_downgrading_on_repeated_capture() {
        let mut ledger = populated();
        let receipt = ledger.receipts()[0].clone();
        ledger.confirm_removed(&receipt).unwrap();
        let before = ledger.clone();
        assert_eq!(ledger.confirm_removed(&receipt), Err(Error::Conflict));
        assert_eq!(ledger, before);
        let current = attempt("old-worker");
        ledger.arm(current.clone()).unwrap();
        ledger.record(current.attempt_id, receipt.device_id.clone()).unwrap();
        assert_eq!(ledger.receipts().len(), 1);
        assert!(ledger.receipts()[0].node_deleted);
        assert_eq!(ledger.receipts()[0].device_id, receipt.device_id);
        let encoded = serde_json::to_vec(&ledger).unwrap();
        assert_eq!(serde_json::from_slice::<EnrollmentLedger>(&encoded).unwrap(), ledger);
        assert_eq!(ledger.attempts(), before.attempts());
    }
}
