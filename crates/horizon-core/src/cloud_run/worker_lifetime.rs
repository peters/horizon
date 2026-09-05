//! Explicit execution lifetime, independent of controller ownership leases.

use super::{CloudProtocolError, CloudProvider, WorkerTarget};
use serde::{Deserialize, Deserializer, Serialize};

/// Persistent execution is the product default, never inferred from missing
/// or malformed legacy metadata. Time limits require an explicit policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkerLifetime {
    #[default]
    Persistent,
    TimeLimited {
        seconds: u32,
    },
}

impl WorkerLifetime {
    #[must_use]
    pub const fn time_limit_seconds(self) -> Option<u32> {
        match self {
            Self::Persistent => None,
            Self::TimeLimited { seconds } => Some(seconds),
        }
    }

    pub(super) const fn is_valid(self) -> bool {
        !matches!(self, Self::TimeLimited { seconds: 0 })
    }
}

/// The bounded v1 representation is kept byte-for-byte. Persistent targets use
/// a separate explicit marker; neither omission nor null selects persistence.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkerTargetSnapshot {
    provider: CloudProvider,
    profile: String,
    image: String,
    disk_gib: u32,
    #[serde(default, deserialize_with = "present_value", skip_serializing_if = "Option::is_none")]
    lease_seconds: Option<u32>,
    #[serde(default, deserialize_with = "present_value", skip_serializing_if = "Option::is_none")]
    lifetime: Option<PersistentMarker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_hourly_cost_micros: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PersistentMarker {
    Persistent,
}

fn present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<WorkerTargetSnapshot> for WorkerTarget {
    type Error = CloudProtocolError;

    fn try_from(value: WorkerTargetSnapshot) -> Result<Self, Self::Error> {
        let lifetime = match (value.lease_seconds, value.lifetime) {
            (Some(seconds), None) if seconds > 0 => WorkerLifetime::TimeLimited { seconds },
            (None, Some(PersistentMarker::Persistent)) => WorkerLifetime::Persistent,
            _ => return Err(CloudProtocolError::InvalidWorkerLifetime),
        };
        Ok(Self {
            provider: value.provider,
            profile: value.profile,
            image: value.image,
            disk_gib: value.disk_gib,
            lifetime,
            max_hourly_cost_micros: value.max_hourly_cost_micros,
        })
    }
}

impl From<WorkerTarget> for WorkerTargetSnapshot {
    fn from(value: WorkerTarget) -> Self {
        Self {
            provider: value.provider,
            profile: value.profile,
            image: value.image,
            disk_gib: value.disk_gib,
            lease_seconds: value.lifetime.time_limit_seconds(),
            lifetime: matches!(value.lifetime, WorkerLifetime::Persistent).then_some(PersistentMarker::Persistent),
            max_hourly_cost_micros: value.max_hourly_cost_micros,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target_fields() -> serde_json::Value {
        json!({
            "provider": "local_docker",
            "profile": "development",
            "image": format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "disk_gib": 20
        })
    }

    #[test]
    fn persistent_default_requires_an_explicit_wire_marker() {
        assert_eq!(WorkerLifetime::default(), WorkerLifetime::Persistent);
        let mut fields = target_fields();
        assert!(serde_json::from_value::<WorkerTarget>(fields.clone()).is_err());
        fields["lifetime"] = json!("persistent");
        let target: WorkerTarget = serde_json::from_value(fields.clone()).expect("explicit persistent target");
        assert_eq!(target.lifetime, WorkerLifetime::Persistent);
        assert_eq!(target.lifetime.time_limit_seconds(), None);
        assert_eq!(serde_json::to_value(target).expect("serialize target"), fields);
    }

    #[test]
    fn legacy_time_limit_keeps_its_exact_wire_representation() {
        let legacy = r#"{"provider":"local_docker","profile":"development","image":"registry.example/worker","disk_gib":20,"lease_seconds":900,"max_hourly_cost_micros":500000}"#;
        let target: WorkerTarget = serde_json::from_str(legacy).expect("legacy target");
        assert_eq!(target.lifetime, WorkerLifetime::TimeLimited { seconds: 900 });
        assert_eq!(target.lifetime.time_limit_seconds(), Some(900));
        assert_eq!(serde_json::to_string(&target).expect("serialize legacy"), legacy);
    }

    #[test]
    fn missing_conflicting_null_or_unknown_lifetime_never_means_persistent() {
        for policy in [
            json!({}),
            json!({"lease_seconds": null}),
            json!({"lifetime": null}),
            json!({"lease_seconds": 0}),
            json!({"lease_seconds": -1}),
            json!({"lease_seconds": 4_294_967_296_u64}),
            json!({"lease_seconds": "900"}),
            json!({"lifetime": "unknown"}),
            json!({"lifetime": true}),
            json!({"lifetime": "persistent", "lease_seconds": 900}),
            json!({"lifetime": "persistent", "lease_seconds": null}),
            json!({"lifetime": null, "lease_seconds": 900}),
            json!({"lifetime": "persistent", "unexpected": true}),
        ] {
            let mut fields = target_fields();
            fields
                .as_object_mut()
                .expect("target object")
                .extend(policy.as_object().expect("policy object").clone());
            assert!(
                serde_json::from_value::<WorkerTarget>(fields).is_err(),
                "accepted {policy}"
            );
        }
        assert!(!WorkerLifetime::TimeLimited { seconds: 0 }.is_valid());
    }
}
