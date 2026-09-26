//! Ranked cloud offers for agents' `cloud_offers` requests, from the prices New cloud
//! keeps current.
use super::HorizonApp;
use horizon_core::browser::manifest::provider_usage::UsageRequest;

impl HorizonApp {
    /// Offers for `request`, an error, or `None` while prices are being fetched.
    pub(in crate::app) fn cloud_offers_answer(
        &mut self,
        request: &UsageRequest,
        ctx: &egui::Context,
    ) -> Option<Result<serde_json::Value, String>> {
        use horizon_core::cloud_runtime::offers::{Requirements, offers};
        let requirements = serde_json::from_value::<Requirements>(request.cloud_offers.clone()?)
            .map_err(|error| error.to_string())
            .and_then(|requirements| requirements.validate().map(|()| requirements).map_err(str::to_owned));
        let requirements = match requirements {
            Ok(requirements) => requirements,
            Err(error) => return Some(Err(format!("cloud_offers_invalid_request: {error}"))),
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return Some(Err("cloud_offers_unavailable: Horizon has no cloud settings".to_owned()));
        };
        let prices = &mut self.cloud_prototype.production.prices;
        prices.poll();
        // A failed fetch is reported once; the next request asks the provider again.
        if let Some(error) = prices.list_error.take() {
            return Some(Err(format!("cloud_offers_unavailable: {error}")));
        }
        prices.request_fresh_list(&root, ctx);
        let fetched = prices.fresh_list()?;
        let (list, _) = &fetched.value;
        let observed = std::time::SystemTime::now()
            .checked_sub(fetched.at.elapsed())
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|at| u64::try_from(at.as_millis()).unwrap_or(u64::MAX));
        Some(Ok(serde_json::json!({
            "provider": list.provider,
            "observed_at_millis": observed,
            "observed_seconds_ago": fetched.at.elapsed().as_secs(),
            "offers": offers(list, &requirements),
        })))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::prices::{
        Availability, CpuFlavorPrice, DataCenter, GpuPrice, Preferences, PriceList, RUNPOD_STORAGE,
    };

    fn request(requirements: &serde_json::Value) -> UsageRequest {
        serde_json::from_value(serde_json::json!({
            "request_id": "offers", "actor": "horizon:agent",
            "host_instance": horizon_core::browser::manifest::host_instance(),
            "deadline_at_millis": i64::MAX, "cloud_offers": requirements, "claimed": true
        }))
        .unwrap()
    }

    fn priced(app: &mut HorizonApp) {
        let list = PriceList {
            provider: "RunPod",
            cpu: vec![CpuFlavorPrice {
                id: "cpu3c".into(),
                name: "Compute-Optimized".into(),
                per_vcpu_hour: 0.03,
            }],
            gpus: vec![GpuPrice {
                id: "NVIDIA RTX A5000".into(),
                name: "RTX A5000".into(),
                memory_gb: 24,
                hourly: 0.27,
            }],
            data_centers: vec![DataCenter {
                id: "EU-RO-1".into(),
                region: "EUROPE".into(),
                workspace_storage: true,
                gpus: vec![("NVIDIA RTX A5000".into(), Availability::High)],
            }],
            regions: std::collections::BTreeMap::new(),
            storage: RUNPOD_STORAGE,
        };
        app.cloud_prototype
            .production
            .prices
            .answered(list, Preferences::default(), Vec::new());
    }

    #[test]
    fn agents_get_ranked_offers_from_current_prices_or_a_clear_error() {
        let (temp, mut app) = crate::app::test_support::test_app();
        let ctx = egui::Context::default();
        let answer = |app: &mut HorizonApp, requirements: serde_json::Value| {
            app.cloud_offers_answer(&request(&requirements), &ctx)
        };
        app.cloud_prototype.root = None;
        assert_eq!(
            answer(&mut app, serde_json::json!({})),
            Some(Err("cloud_offers_unavailable: Horizon has no cloud settings".to_owned()))
        );
        app.cloud_prototype.root = Some(temp.path().to_path_buf());
        // Without prices, and with fetching disabled in tests, the request keeps waiting.
        assert_eq!(answer(&mut app, serde_json::json!({})), None);
        app.cloud_prototype.production.prices.list_error = Some("Missing RunPod API key".into());
        assert_eq!(
            answer(&mut app, serde_json::json!({})),
            Some(Err("cloud_offers_unavailable: Missing RunPod API key".to_owned()))
        );
        assert!(
            app.cloud_prototype.production.prices.list_error.is_none(),
            "the next request asks again"
        );
        let invalid = answer(&mut app, serde_json::json!({"max_hourly": -1}));
        assert!(matches!(invalid, Some(Err(error)) if error.starts_with("cloud_offers_invalid_request")));
        let unknown = answer(&mut app, serde_json::json!({"rent": true}));
        assert!(matches!(unknown, Some(Err(error)) if error.starts_with("cloud_offers_invalid_request")));

        priced(&mut app);
        let offers = answer(&mut app, serde_json::json!({"gpu": true, "hours": 2}))
            .unwrap()
            .unwrap();
        assert_eq!(offers["provider"], "RunPod");
        assert_eq!(offers["observed_seconds_ago"], 0);
        assert_eq!(offers["offers"][0]["id"], "NVIDIA RTX A5000");
        assert_eq!(offers["offers"][0]["regions_in_stock"][0], "EUROPE");
        let cpu = answer(&mut app, serde_json::json!({"min_vcpu": 8})).unwrap().unwrap();
        assert_eq!(cpu["offers"][0]["vcpu"], 8);
        assert_eq!(cpu["offers"][0]["availability"], "checked_at_creation");
    }
}
