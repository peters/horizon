//! One estimated-cost ordering shared by UI, CLI and MCP, retaining billing currency.
use super::exchange::Rates;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ComparedOffer {
    #[serde(flatten)]
    pub offer: Value,
    pub estimated_total_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Comparison {
    pub currency: &'static str,
    pub exchange_date: Option<String>,
    pub complete: bool,
    pub offers: Vec<ComparedOffer>,
}

/// Unsupported or unquoted currencies never acquire a comparable price.
#[must_use]
pub fn dollars(amount: f64, currency: &str, rates: Option<&Rates>) -> Option<f64> {
    if currency == "USD" {
        return (amount.is_finite() && amount >= 0.0).then_some(amount);
    }
    rates?.dollars(amount, currency)
}

/// Native-currency ordering needs no exchange quote; cross-currency ordering does.
#[must_use]
pub fn in_currency(amount: f64, currency: &str, target: &str, rates: Option<&Rates>) -> Option<f64> {
    if currency == target {
        return (amount.is_finite() && amount >= 0.0).then_some(amount);
    }
    let usd = dollars(amount, currency, rates)?;
    let per_target = dollars(1.0, target, rates)?;
    let total = usd / per_target;
    total.is_finite().then_some(total)
}

/// Add the common-currency answer without changing existing provider sections.
/// Whether this answer needs an external currency quote.
#[must_use]
pub fn needs_rates(answer: &Value) -> bool {
    let mut sections = vec![answer];
    if let Some(others) = answer.get("other_providers").and_then(Value::as_array) {
        sections.extend(others);
    }
    sections
        .iter()
        .flat_map(|section| section.get("offers").and_then(Value::as_array).into_iter().flatten())
        .any(|offer| offer["currency"].as_str().is_some_and(|currency| currency != "USD"))
}

/// Read-only clients fetch reference rates only when an offer needs conversion.
/// A failed rate fetch preserves the native offers with an incomplete comparison.
pub fn fetch_append(answer: &mut Value) {
    let rates = needs_rates(answer).then(Rates::fetch).and_then(Result::ok);
    append(answer, rates.as_ref());
}

pub fn append(answer: &mut Value, rates: Option<&Rates>) {
    let exchange_needed = needs_rates(answer);
    let mut sections = vec![&*answer];
    if let Some(others) = answer.get("other_providers").and_then(Value::as_array) {
        sections.extend(others);
    }
    let mut complete = sections.iter().all(|section| {
        section.get("error").is_none() && section.get("comparison_incomplete").and_then(Value::as_bool) != Some(true)
    });
    let mut offers: Vec<_> = sections
        .iter()
        .flat_map(|section| section.get("offers").and_then(Value::as_array).into_iter().flatten())
        .map(|offer| {
            let total = offer
                .get("estimated_total")
                .and_then(Value::as_f64)
                .zip(offer.get("currency").and_then(Value::as_str))
                .and_then(|(amount, currency)| dollars(amount, currency, rates));
            complete &= total.is_some();
            ComparedOffer {
                offer: offer.clone(),
                estimated_total_usd: total,
            }
        })
        .collect();
    offers.sort_by(|a, b| {
        a.estimated_total_usd
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.estimated_total_usd.unwrap_or(f64::INFINITY))
            .then_with(|| a.offer["provider"].as_str().cmp(&b.offer["provider"].as_str()))
            .then_with(|| a.offer["id"].as_str().cmp(&b.offer["id"].as_str()))
            .then_with(|| a.offer["location"].as_str().cmp(&b.offer["location"].as_str()))
    });
    let comparison = Comparison {
        currency: "USD",
        exchange_date: rates
            .filter(|rates| exchange_needed && rates.current(time::OffsetDateTime::now_utc().date()))
            .map(|rates| rates.date.clone()),
        complete,
        offers,
    };
    answer["comparison"] = serde_json::json!(comparison);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn native_dollar_and_empty_answers_do_not_publish_a_cached_exchange_date() {
        let rates = Rates {
            date: time::OffsetDateTime::now_utc().date().to_string(),
            usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
        };
        for mut answer in [
            serde_json::json!({"offers": []}),
            serde_json::json!({"offers": [{"currency": "USD", "estimated_total": 1.0}]}),
        ] {
            append(&mut answer, Some(&rates));
            assert_eq!(answer["comparison"]["complete"], true);
            assert!(answer["comparison"]["exchange_date"].is_null());
        }
    }

    #[test]
    fn comparison_uses_estimated_totals_and_keeps_the_worker_identity_and_invoice_currency() {
        let mut answer = serde_json::json!({"offers":[
            {"provider":"RunPod","currency":"USD","id":"cpu-4-8","estimated_total":1.1}
        ], "other_providers":[{"offers":[
            {"provider":"Hetzner","currency":"EUR","id":"cx33","location":"hel1","estimated_total":1.0}
        ]}]});
        let rates = Rates {
            date: time::OffsetDateTime::now_utc().date().to_string(),
            usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
        };
        append(&mut answer, Some(&rates));
        assert_eq!(in_currency(1.0, "EUR", "EUR", None), Some(1.0));
        assert_eq!(in_currency(1.0, "EUR", "USD", None), None);
        assert_eq!(in_currency(1.2, "USD", "EUR", Some(&rates)), Some(1.0));
        assert_eq!(in_currency(f64::NAN, "EUR", "EUR", None), None);
        assert_eq!(answer["comparison"]["offers"][0]["provider"], "RunPod");
        assert_eq!(answer["comparison"]["offers"][1]["currency"], "EUR");
        assert_eq!(answer["comparison"]["offers"][1]["location"], "hel1");
        assert_eq!(answer["comparison"]["offers"][1]["estimated_total_usd"], 1.2);
        assert_eq!(answer["comparison"]["complete"], true);
        assert_eq!(answer["comparison"]["exchange_date"], rates.date);
        append(&mut answer, None);
        assert_eq!(answer["comparison"]["complete"], false);
        assert!(answer["comparison"]["offers"][1]["estimated_total_usd"].is_null());
        answer["other_providers"][0]["error"] = serde_json::json!("catalog unavailable");
        append(&mut answer, Some(&rates));
        assert_eq!(answer["comparison"]["complete"], false);
    }
}
