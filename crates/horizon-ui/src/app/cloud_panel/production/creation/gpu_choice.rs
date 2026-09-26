//! A GPU type for one new cloud, chosen from those in stock where it may be placed, in
//! place of the machine's `gpu_types` preferences.
use super::{
    super::prices::State,
    costs::money,
    placement::{Stock, chip},
};
use crate::theme;
use egui::{RichText, Ui};
use horizon_core::{
    cloud_panel::Placement,
    cloud_runtime::prices::{Availability, GpuPrice, PriceList, Profile},
};

/// GPU types in stock in `within` (every allowed data center when empty), cheapest first,
/// with any in `chosen` kept even when sold out so the current choice stays visible.
fn offers<'a>(list: &'a PriceList, within: &[String], chosen: &[String]) -> Vec<(&'a GpuPrice, Availability)> {
    let mut offers: Vec<(&GpuPrice, Availability)> = list
        .gpus
        .iter()
        .map(|gpu| (gpu, list.gpu_availability(&gpu.id, within)))
        .filter(|(gpu, availability)| *availability != Availability::None || chosen.contains(&gpu.id))
        .collect();
    offers.sort_by(|(a, _), (b, _)| a.hourly.total_cmp(&b.hourly).then_with(|| a.name.cmp(&b.name)));
    offers
}

/// GPU type chips for a GPU profile, once prices are known. Returns a newly chosen placement.
pub(super) fn gpu_field(ui: &mut Ui, prices: &State, profile: &Profile, current: &Placement) -> Option<Placement> {
    if !profile.gpu {
        return None;
    }
    let (list, preferences) = &prices.list.as_ref()?.value;
    let offers = offers(list, &current.data_centers, &current.gpu_types);
    ui.add_space(6.0);
    ui.label(RichText::new("GPU type").size(14.0).strong().color(theme::FG()));
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        let order = match preferences.gpu_types.len() {
            0 => "none set".to_owned(),
            1 => "1 type".to_owned(),
            count => format!("{count} types, in order"),
        };
        if chip(ui, "Your preferences", &order, None, current.gpu_types.is_empty()) {
            chosen = Some(Placement {
                gpu_types: Vec::new(),
                ..current.clone()
            });
        }
        for (gpu, availability) in &offers {
            let selected = current.gpu_types == [gpu.id.clone()];
            let detail = format!("{} GB · {}/h", gpu.memory_gb, money(gpu.hourly));
            let stock = if *availability == Availability::None {
                Stock::No
            } else {
                Stock::Yes
            };
            if chip(ui, &gpu.name, &detail, Some(stock), selected) {
                chosen = Some(Placement {
                    gpu_types: vec![gpu.id.clone()],
                    ..current.clone()
                });
            }
        }
    });
    let hidden = list.gpus.len() - offers.len();
    if hidden > 0 {
        ui.small(format!("{hidden} more GPU types without stock here right now."));
    }
    chosen.filter(|placement| placement != current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::prices::{DataCenter, RUNPOD_STORAGE};

    #[test]
    fn offers_are_gpus_in_stock_where_the_cloud_may_go_cheapest_first() {
        let gpu = |id: &str, hourly| GpuPrice {
            id: id.into(),
            name: id.into(),
            memory_gb: 24,
            hourly,
        };
        let center = |id: &str, gpus: &[(&str, Availability)]| DataCenter {
            id: id.into(),
            region: "EUROPE".into(),
            workspace_storage: false,
            gpus: gpus.iter().map(|&(gpu, level)| (gpu.into(), level)).collect(),
        };
        let list = PriceList {
            provider: "RunPod",
            cpu: Vec::new(),
            gpus: vec![gpu("l4", 0.49), gpu("a5000", 0.27), gpu("a6000", 0.53)],
            data_centers: vec![
                center("EU-RO-1", &[("l4", Availability::Low), ("a5000", Availability::High)]),
                center("US-MO-2", &[("a6000", Availability::High)]),
            ],
            regions: std::collections::BTreeMap::new(),
            storage: RUNPOD_STORAGE,
        };
        let ids = |offers: Vec<(&GpuPrice, Availability)>| -> Vec<String> {
            offers.into_iter().map(|(gpu, _)| gpu.id.clone()).collect()
        };
        assert_eq!(ids(offers(&list, &[], &[])), ["a5000", "l4", "a6000"]);
        assert_eq!(ids(offers(&list, &["EU-RO-1".into()], &[])), ["a5000", "l4"]);
        // A chosen type stays listed after it sells out where the cloud may go.
        assert_eq!(
            ids(offers(&list, &["EU-RO-1".into()], &["a6000".into()])),
            ["a5000", "l4", "a6000"]
        );
    }
}
