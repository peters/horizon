//! The worker as the provider last described it, under the profile in the Machine tab.
use super::super::Runtime;
use crate::theme;
use egui::RichText;

/// Label and value rows for the bound worker; empty before one is requested.
pub(super) fn facts(runtime: &Runtime) -> Vec<(&'static str, String)> {
    let Some(worker) = runtime.state.as_ref().and_then(|state| state.worker.as_ref()) else {
        return Vec::new();
    };
    let mut rows = vec![(
        "Worker",
        format!("{} · {} (last observed)", worker.id, worker.desired_status),
    )];
    if let Some(endpoint) = worker.ssh_endpoint() {
        rows.push(("SSH", host_port(endpoint.host().as_str(), endpoint.port())));
    }
    let disks = [
        worker.container_disk_in_gb.map(|gb| format!("{gb} GB container")),
        worker.volume_in_gb.map(|gb| {
            worker
                .volume_mount_path
                .as_deref()
                .map_or_else(|| format!("{gb} GB volume"), |path| format!("{gb} GB volume at {path}"))
        }),
        worker
            .network_volume
            .as_ref()
            .and_then(|volume| volume.id.as_deref())
            .map(|id| format!("network volume {id}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !disks.is_empty() {
        rows.push(("Storage", disks.join(" · ")));
    }
    if let Some(started) = &worker.last_started_at {
        rows.push(("Last start", started.clone()));
    }
    rows
}

pub(super) fn worker(ui: &mut egui::Ui, runtime: &Runtime) {
    let rows = facts(runtime);
    if rows.is_empty() {
        ui.small("Worker details appear once the provider reports a worker.");
        return;
    }
    egui::Grid::new("cloud-worker-facts")
        .num_columns(2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.add(egui::Label::new(RichText::new(label).size(13.0).color(theme::FG_DIM())).extend());
                ui.add(egui::Label::new(RichText::new(value).size(14.0).color(theme::FG())).wrap());
                ui.end_row();
            }
        });
}

/// `host:port`, with an IPv6 host bracketed so its colons do not run into the port's.
fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_record_lists_what_the_card_never_showed() {
        let runtime = Runtime {
            state: Some(
                serde_json::from_value(serde_json::json!({
                    "version":1,"cloud_id":"facts","repository":"/synthetic","revision":"a",
                    "profile":{"provider":"runpod","image":"registry.example/worker","cpu":8,"memory_gb":32},
                    "stage":"Ready","operation":{"state":"bound","worker_id":"k3x9"},"spec":null,"sessions":[],
                    "worker":{"id":"k3x9","name":"fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING",
                        "publicIp":"203.0.113.24","portMappings":{"22":22041},"containerDiskInGb":20,
                        "volumeInGb":50,"volumeMountPath":"/workspace","lastStartedAt":"2026-09-28T09:00:00Z"}
                }))
                .unwrap(),
            ),
            ..Default::default()
        };
        let rows = facts(&runtime);
        let find = |label: &str| {
            rows.iter()
                .find(|(name, _)| *name == label)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(find("Worker"), Some("k3x9 · RUNNING (last observed)"));
        assert_eq!(find("SSH"), Some("203.0.113.24:22041"));
        assert_eq!(find("Storage"), Some("20 GB container · 50 GB volume at /workspace"));
        assert_eq!(find("Last start"), Some("2026-09-28T09:00:00Z"));
        assert!(facts(&Runtime::default()).is_empty());
    }

    #[test]
    fn an_ipv6_ssh_host_is_bracketed_before_its_port() {
        assert_eq!(host_port("2001:db8::1", 22), "[2001:db8::1]:22");
        assert_eq!(host_port("203.0.113.24", 22041), "203.0.113.24:22041");
        assert_eq!(host_port("worker.example", 22), "worker.example:22");
    }
}
