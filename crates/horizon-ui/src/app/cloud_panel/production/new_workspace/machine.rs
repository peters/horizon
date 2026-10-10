//! The machine and hourly price that the New workspace menu shows under Cloud: the
//! cheapest worker for a quick start, ranked as `cloud_offers` ranks offers. A repository
//! with its own profile can ask for more; New cloud then shows its own machine and price.
use super::HorizonApp;
use horizon_core::cloud_runtime::{offers::Requirements, repository::launch::quick_start};
use serde_json::Value;

/// What the menu says under Cloud while the prices come in.
pub(in crate::app) const CHECKING: &str = "Checking machine prices…";

impl HorizonApp {
    /// The menu line for Cloud: `CHECKING` while prices are fetched, and nothing when no
    /// provider answers. Asks the providers only when their prices are stale.
    pub(in crate::app) fn new_cloud_machine_line(&mut self, ctx: &egui::Context) -> Option<String> {
        let config = quick_start::builtin().ok()?;
        let profile = config.profiles.get(quick_start::PROFILE)?;
        let requirements = Requirements {
            min_vcpu: Some(profile.cpu),
            min_memory_gb: Some(profile.memory_gb),
            storage_gb: Some(profile.storage.volume_gb),
            limit: Some(1),
            ..Requirements::default()
        };
        match self.ranked_offers(&requirements, i64::MAX, ctx) {
            None => Some(CHECKING.to_owned()),
            Some(Ok(answer)) => line(&answer),
            Some(Err(_)) => None,
        }
    }
}

/// The cheapest offer of a ranked answer, as the menu shows it. Without a complete
/// comparison (no exchange rate), offers in different currencies cannot be ranked, so
/// the menu shows none of them.
pub(super) fn line(answer: &Value) -> Option<String> {
    let comparison = answer.get("comparison")?;
    let offers = comparison.get("offers")?.as_array()?;
    let complete = comparison.get("complete").and_then(Value::as_bool) == Some(true);
    let one_currency = offers
        .windows(2)
        .all(|pair| pair[0].get("currency") == pair[1].get("currency"));
    let offer = if complete {
        offers.first()?
    } else if one_currency {
        offers.iter().min_by(|a, b| {
            let total = |offer: &Value| {
                offer
                    .get("estimated_total")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::INFINITY)
            };
            total(a).total_cmp(&total(b))
        })?
    } else {
        return None;
    };
    let vcpu = offer.get("vcpu")?.as_u64()?;
    let memory = offer.get("memory_gb")?.as_u64()?;
    let hourly = offer.get("hourly")?.as_f64().filter(|hourly| hourly.is_finite())?;
    let symbol = match offer.get("currency")?.as_str()? {
        "EUR" => "€",
        "USD" => "$",
        _ => return None,
    };
    let provider = offer.get("provider")?.as_str()?;
    Some(format!(
        "from {symbol}{hourly:.4}/h · {vcpu} vCPU · {memory} GB · {provider}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_cheapest_compared_offer_gives_the_line() {
        let offers = json!([
            {"provider": "Hetzner", "currency": "EUR", "vcpu": 2, "memory_gb": 4, "hourly": 0.0088, "estimated_total": 0.0112},
            {"provider": "RunPod", "currency": "USD", "vcpu": 2, "memory_gb": 4, "hourly": 0.06, "estimated_total": 0.07},
        ]);
        let answer = json!({"comparison": {"complete": true, "offers": offers}});
        assert_eq!(
            line(&answer).as_deref(),
            Some("from €0.0088/h · 2 vCPU · 4 GB · Hetzner")
        );
        assert_eq!(line(&json!({"comparison": {"offers": []}})), None);
        // Without exchange rates, two currencies cannot be ranked; one currency can.
        assert_eq!(
            line(&json!({"comparison": {"complete": false, "offers": offers}})),
            None
        );
        let euros = json!([
            {"provider": "Hetzner", "currency": "EUR", "vcpu": 4, "memory_gb": 8, "hourly": 0.02, "estimated_total": 0.03},
            {"provider": "Hetzner", "currency": "EUR", "vcpu": 2, "memory_gb": 4, "hourly": 0.0088, "estimated_total": 0.0112},
        ]);
        assert_eq!(
            line(&json!({"comparison": {"complete": false, "offers": euros}})).as_deref(),
            Some("from €0.0088/h · 2 vCPU · 4 GB · Hetzner")
        );
        assert_eq!(line(&json!({"offers": []})), None);
    }
}
