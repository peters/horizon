//! The machine and hourly price that the tooltip of + New cloud shows: the
//! cheapest worker for a quick start, ranked as `cloud_offers` ranks offers. A repository
//! with its own profile can ask for more; New cloud then shows its own machine and price.
use super::HorizonApp;
use horizon_core::cloud_runtime::{offers::Requirements, repository::launch::quick_start};
use serde_json::Value;

/// What the tooltip says while the prices come in.
pub(in crate::app) const CHECKING: &str = "Checking machine prices…";

impl HorizonApp {
    /// The tooltip line of + New cloud: `CHECKING` while prices are fetched, and nothing when no
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
        match self.ranked_offers(&requirements, i64::MAX, false, ctx) {
            None => Some(CHECKING.to_owned()),
            Some(Ok(answer)) => line(&answer),
            Some(Err(_)) => None,
        }
    }
}

/// The cheapest offer of a ranked answer, as the tooltip shows it. Without a complete
/// comparison (no exchange rate), offers in different currencies cannot be ranked, so
/// the tooltip shows none of them.
pub(super) fn line(answer: &Value) -> Option<String> {
    let comparison = answer.get("comparison")?;
    // A machine that Horizon cannot create is never the tooltip's offer, also when it is
    // the cheapest: an Azure row carries `rentable: false` until PR 5 can create one.
    let offers: Vec<&Value> = comparison
        .get("offers")?
        .as_array()?
        .iter()
        .filter(|offer| offer.get("rentable").and_then(Value::as_bool) != Some(false))
        .collect();
    let complete = comparison.get("complete").and_then(Value::as_bool) == Some(true);
    let one_currency = offers
        .windows(2)
        .all(|pair| pair[0].get("currency") == pair[1].get("currency"));
    let offer = if complete {
        offers.first().copied()?
    } else if one_currency {
        offers.iter().copied().min_by(|a, b| {
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
    fn the_line_never_names_a_machine_that_cannot_be_created() {
        let rentable = json!({
            "provider": "Hetzner", "currency": "EUR", "vcpu": 4, "memory_gb": 8,
            "hourly": 0.02, "estimated_total": 0.2, "rentable": true
        });
        let azure = json!({
            "provider": "Azure", "currency": "USD", "vcpu": 4, "memory_gb": 8,
            "hourly": 0.01, "estimated_total": 0.01, "rentable": false
        });
        let answer = json!({"comparison": {"complete": true, "offers": [azure.clone(), rentable]}});
        assert_eq!(
            line(&answer).as_deref(),
            Some("from €0.0200/h · 4 vCPU · 8 GB · Hetzner"),
            "the cheapest machine that Horizon cannot create is skipped"
        );
        assert_eq!(
            line(&json!({"comparison": {"complete": true, "offers": [azure]}})),
            None,
            "an answer of uncreatable machines alone gives no line"
        );
    }

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
