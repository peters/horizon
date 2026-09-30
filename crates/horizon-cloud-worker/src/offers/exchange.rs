//! Process-wide background reference-rate cache shared by host and companion requests.
use horizon_cloud::offers::exchange::{OffsetDateTime, Rates};
use std::{
    sync::{
        Mutex, OnceLock,
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_hours(6);
const RETRY: Duration = Duration::from_secs(30);
type Answer = (Option<Rates>, Instant);

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

#[derive(Default)]
struct Cache {
    rates: Option<(Rates, Instant)>,
    failed_at: Option<Instant>,
    job: Option<Receiver<Answer>>,
}

pub(super) fn quote(allow_fetch: bool) -> Option<Rates> {
    if cfg!(test) {
        return None;
    }
    CACHE
        .get_or_init(|| Mutex::new(Cache::default()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .quote_with(allow_fetch, || Rates::fetch().ok())
        .cloned()
}

impl Cache {
    fn fresh(&self) -> Option<&Rates> {
        self.rates
            .as_ref()
            .filter(|(rates, at)| at.elapsed() < FRESH && rates.current(OffsetDateTime::now_utc().date()))
            .map(|(rates, _)| rates)
    }

    fn poll(&mut self) {
        let answer = self.job.as_ref().map(Receiver::try_recv);
        match answer {
            Some(Ok((rates, at))) => {
                self.job = None;
                if let Some(rates) = rates {
                    self.rates = Some((rates, at));
                    self.failed_at = None;
                } else {
                    self.failed_at = Some(at);
                }
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.job = None;
                self.failed_at = Some(Instant::now());
            }
            _ => {}
        }
    }

    fn quote_with(
        &mut self,
        allow_fetch: bool,
        fetch: impl FnOnce() -> Option<Rates> + Send + 'static,
    ) -> Option<&Rates> {
        self.poll();
        if allow_fetch
            && self.fresh().is_none()
            && self.job.is_none()
            && self.failed_at.is_none_or(|at| at.elapsed() >= RETRY)
        {
            let (sender, receiver) = mpsc::channel();
            self.job = Some(receiver);
            std::thread::spawn(move || {
                let answer = fetch();
                let _ = sender.send((answer, Instant::now()));
            });
        }
        self.fresh()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn slow_fetch_never_blocks_answers_and_finished_rates_are_reused() {
        let mut cache = Cache::default();
        assert!(
            cache
                .quote_with(false, || panic!("short deadline must not fetch"))
                .is_none()
        );
        assert!(cache.job.is_none());
        let (release, gate) = mpsc::channel();
        assert!(
            cache
                .quote_with(true, move || {
                    gate.recv().unwrap();
                    Some(Rates {
                        date: OffsetDateTime::now_utc().date().to_string(),
                        usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
                    })
                })
                .is_none()
        );
        assert!(
            cache
                .quote_with(true, || panic!("pending job must be shared"))
                .is_none()
        );
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while cache.quote_with(false, || None).is_none() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            cache
                .quote_with(true, || panic!("fresh rates must be reused"))
                .unwrap()
                .dollars(10.0, "EUR"),
            Some(12.0)
        );
    }

    #[test]
    fn failed_fetch_backs_off_and_expired_rates_are_not_comparable() {
        let (sender, receiver) = mpsc::channel();
        sender.send((None, Instant::now())).unwrap();
        let mut cache = Cache {
            job: Some(receiver),
            ..Cache::default()
        };
        assert!(
            cache
                .quote_with(true, || panic!("failed rates must back off"))
                .is_none()
        );
        assert!(cache.job.is_none());
        cache.rates = Some((
            Rates {
                date: "2000-01-01".into(),
                usd_per_unit: BTreeMap::from([("USD".into(), 1.0), ("EUR".into(), 1.2)]),
            },
            Instant::now(),
        ));
        assert!(cache.quote_with(false, || None).is_none());
    }
}
