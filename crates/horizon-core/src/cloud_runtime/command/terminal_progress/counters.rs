use super::{Progress, Transfer};
use std::collections::BTreeMap;

#[derive(Default)]
struct Layer {
    current: u64,
    total: Option<u64>,
    done: bool,
}

pub(super) struct Counters {
    kind: Transfer,
    layers: BTreeMap<String, Layer>,
    file_completed: u64,
    file_percent: u64,
    file_activity: u64,
}

impl Counters {
    pub fn new(kind: Transfer) -> Self {
        Self {
            kind,
            layers: BTreeMap::new(),
            file_completed: 0,
            file_percent: 0,
            file_activity: 0,
        }
    }

    pub fn observe(&mut self, line: &str) {
        match self.kind {
            Transfer::Image | Transfer::Pull => self.image(line),
            Transfer::File(total) => {
                let mut words = line.split_whitespace();
                if let Some(percent) = words.find_map(|word| {
                    word.strip_suffix('%')?
                        .parse::<u64>()
                        .ok()
                        .filter(|value| *value <= 100)
                }) && let Some(bytes) = words.next().and_then(scp_bytes)
                {
                    if bytes.min(total) > self.file_completed || percent > self.file_percent {
                        self.file_activity = self.file_activity.saturating_add(1);
                    }
                    self.file_completed = self.file_completed.max(bytes.min(total));
                    self.file_percent = self.file_percent.max(percent);
                }
            }
        }
    }

    pub fn activity(&self) -> u64 {
        self.file_activity
    }

    fn image(&mut self, line: &str) {
        let Some((id, status)) = line.trim().split_once(':') else {
            return;
        };
        if !(12..=64).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
        let layer = self.layers.entry(id.to_owned()).or_default();
        if status.contains("Layer already exists")
            || status.contains("Already exists")
            || status.contains("Mounted from")
        {
            layer.done = true;
            layer.total = Some(0);
            layer.current = 0;
        } else if status.contains("Pushed") || status.contains("Pull complete") {
            layer.done = true;
            if let Some(total) = layer.total {
                layer.current = total;
            }
        } else if status.contains("Download complete") {
            if let Some(total) = layer.total {
                layer.current = total;
            }
        } else if !status.contains("Extracting")
            && let Some((_, amount)) = status.rsplit_once(']')
            && let Some((current, total)) = amount.trim().split_once('/')
            && let (Some(current), Some(total)) = (size(current), size(total))
        {
            layer.current = current.min(total);
            layer.total = Some(total);
            layer.done = false;
        }
    }

    pub fn snapshot(&self, name: &str) -> Progress {
        match self.kind {
            Transfer::File(total) => Progress {
                detail: "Uploading committed source".into(),
                completed: self.file_completed,
                total: Some(total),
                transferred: Some(self.file_completed),
                ..Progress::default()
            },
            Transfer::Image | Transfer::Pull => {
                let completed = self.layers.values().map(|layer| layer.current).sum();
                let known =
                    !self.layers.is_empty() && self.layers.values().all(|layer| layer.total.is_some() || layer.done);
                let done = self.layers.values().filter(|layer| layer.done).count();
                Progress {
                    detail: if self.layers.is_empty() {
                        name.into()
                    } else {
                        format!("{done}/{} layers complete · Docker-reported bytes", self.layers.len())
                    },
                    completed,
                    total: known.then(|| self.layers.values().filter_map(|layer| layer.total).sum()),
                    transferred: Some(completed),
                    ..Progress::default()
                }
            }
        }
    }
}

fn scp_bytes(value: &str) -> Option<u64> {
    let value = value.strip_suffix('B').unwrap_or(value);
    let (number, scale) = match value.as_bytes().last()? {
        b'K' => (&value[..value.len() - 1], 1024_u64),
        b'M' => (&value[..value.len() - 1], 1024_u64.pow(2)),
        b'G' => (&value[..value.len() - 1], 1024_u64.pow(3)),
        b'T' => (&value[..value.len() - 1], 1024_u64.pow(4)),
        _ => (value, 1),
    };
    number.parse::<u64>().ok()?.checked_mul(scale)
}

fn size(value: &str) -> Option<u64> {
    let value = value.trim();
    let split = value.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let scale: u64 = match value[split..].trim() {
        "B" => 1,
        "kB" | "KB" => 1000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1024,
        "MiB" => 1_048_576,
        "GiB" => 1_073_741_824,
        _ => return None,
    };
    let (whole, fraction) = value[..split].split_once('.').unwrap_or((&value[..split], ""));
    let whole = u128::from(whole.parse::<u64>().ok()?) * u128::from(scale);
    let fractional = if fraction.is_empty() {
        0
    } else {
        u128::from(fraction.parse::<u64>().ok()?) * u128::from(scale)
            / 10_u128.checked_pow(u32::try_from(fraction.len()).ok()?)?
    };
    u64::try_from(whole.checked_add(fractional)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_layers_keep_eta_unknown_and_cached_layers_do_not_inflate_transfer() {
        let mut counters = Counters::new(Transfer::Image);
        counters.observe("aaaaaaaaaaaa: Pushing [==>] 1.5MB/10MB");
        counters.observe("bbbbbbbbbbbb: Waiting");
        assert_eq!(counters.snapshot("push").total, None);
        counters.observe("bbbbbbbbbbbb: Layer already exists");
        let progress = counters.snapshot("push");
        assert_eq!(progress.completed, 1_500_000);
        assert_eq!(progress.transferred, Some(1_500_000));
        assert_eq!(progress.total, Some(10_000_000));
        counters.observe("aaaaaaaaaaaa: Pushed");
        assert_eq!(counters.snapshot("push").completed, 10_000_000);
    }

    #[test]
    fn pulling_does_not_count_extraction_as_another_download() {
        let mut counters = Counters::new(Transfer::Pull);
        counters.observe("aaaaaaaaaaaa: Downloading [==>] 2MB/10MB");
        counters.observe("aaaaaaaaaaaa: Download complete");
        counters.observe("aaaaaaaaaaaa: Extracting [==>] 5MB/20MB");
        assert_eq!(counters.snapshot("pull").completed, 10_000_000);
        assert_eq!(counters.snapshot("pull").total, Some(10_000_000));
    }
}
