use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;

use crate::policy::{
    CONNECTIVITY_PROBE_INTERVAL, CONNECTIVITY_PROBE_TIMEOUT, CONNECTIVITY_WAIT_BUDGET,
};

// Over plain HTTP a captive portal answers 200 with its own page; only a real 204 proves
// internet reachability. HTTPS cannot be used here: a portal breaks the handshake instead
// of returning 200, making the check unable to distinguish captive from offline.
const PROBE_URL: &str = "http://www.gstatic.com/generate_204";
const PROBE_STATUS: reqwest::StatusCode = reqwest::StatusCode::NO_CONTENT;

static GATE: OnceLock<ConnectivityGate> = OnceLock::new();

/// Delays boot-time refreshes until the network answers. macOS exposes user sessions before
/// Wi-Fi finishes associating, so an ungated boot refresh fails and the error state lingers
/// until the next loop cycle. One bounded wait here prevents that. After the wait settles
/// (online or budget spent) the latch keeps later callers from ever waiting again.
pub async fn ensure_network_ready() {
    GATE.get_or_init(ConnectivityGate::new)
        .ensure_ready(
            CONNECTIVITY_WAIT_BUDGET,
            CONNECTIVITY_PROBE_INTERVAL,
            Arc::new(probe_once),
        )
        .await;
}

struct ConnectivityGate {
    settled: AtomicBool,
}

impl ConnectivityGate {
    fn new() -> Self {
        Self {
            settled: AtomicBool::new(false),
        }
    }

    async fn ensure_ready(
        &self,
        budget: Duration,
        interval: Duration,
        probe: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        let deadline = Instant::now() + budget;
        loop {
            // Late joiners must abort promptly once another caller settles the gate.
            if self.settled.load(Ordering::Relaxed) {
                return;
            }
            let probe = probe.clone();
            let reachable = tauri::async_runtime::spawn_blocking(move || probe())
                .await
                .unwrap_or(false);
            if reachable {
                crate::app_debug!("network", "connectivity confirmed");
                break;
            }
            if Instant::now() >= deadline {
                crate::app_debug!("network", "connectivity budget spent; proceeding anyway");
                break;
            }
            tokio::time::sleep(interval).await;
        }
        self.settled.store(true, Ordering::Relaxed);
    }
}

// The blocking client owns a runtime that panics if dropped inside an async context, so it
// lives for the process lifetime in a static; clones are Arc-backed and cheap.
fn probe_client() -> Option<Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(CONNECTIVITY_PROBE_TIMEOUT)
                .timeout(CONNECTIVITY_PROBE_TIMEOUT)
                .user_agent(concat!("QuotaGauge/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .clone()
}

fn probe_once() -> bool {
    match probe_client() {
        Some(client) => client
            .get(PROBE_URL)
            .send()
            .map(|response| response.status() == PROBE_STATUS)
            .unwrap_or(false),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};

    use super::{ConnectivityGate, PROBE_STATUS, PROBE_URL};

    const TEST_BUDGET: Duration = Duration::from_secs(5);

    fn counting_probe(
        counter: Arc<AtomicUsize>,
        result: bool,
    ) -> Arc<dyn Fn() -> bool + Send + Sync> {
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            result
        })
    }

    #[test]
    fn probe_endpoint_is_the_google_empty_reply_check() {
        assert_eq!(PROBE_URL, "http://www.gstatic.com/generate_204");
        assert_eq!(PROBE_STATUS, reqwest::StatusCode::NO_CONTENT);
    }

    #[test]
    fn gate_waits_through_failures_then_settles() {
        let gate = ConnectivityGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let probe: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || counted.fetch_add(1, Ordering::SeqCst) >= 2);

        tauri::async_runtime::block_on(async {
            gate.ensure_ready(TEST_BUDGET, Duration::from_millis(5), probe)
                .await;
        });

        assert!(calls.load(Ordering::SeqCst) >= 3);
        assert!(gate.settled.load(Ordering::Relaxed));
    }

    #[test]
    fn gate_latches_and_never_probes_again() {
        let gate = ConnectivityGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting_probe(calls.clone(), true);

        tauri::async_runtime::block_on(async {
            gate.ensure_ready(TEST_BUDGET, Duration::from_millis(1), probe.clone())
                .await;
            gate.ensure_ready(TEST_BUDGET, Duration::from_millis(1), probe)
                .await;
        });

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn gate_gives_up_after_budget_and_still_latches() {
        let gate = ConnectivityGate::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting_probe(calls.clone(), false);
        let budget = Duration::from_millis(50);

        let started = Instant::now();
        tauri::async_runtime::block_on(async {
            gate.ensure_ready(budget, Duration::from_millis(5), probe.clone())
                .await;
        });

        assert!(started.elapsed() >= budget);
        assert!(started.elapsed() < TEST_BUDGET);
        assert!(gate.settled.load(Ordering::Relaxed));

        let calls_before = calls.load(Ordering::SeqCst);
        tauri::async_runtime::block_on(async {
            gate.ensure_ready(budget, Duration::from_millis(5), probe)
                .await;
        });
        assert_eq!(calls.load(Ordering::SeqCst), calls_before);
    }

    #[test]
    fn late_joiner_aborts_once_another_caller_settles() {
        let gate = Arc::new(ConnectivityGate::new());
        let slow_probe: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(|| {
            std::thread::sleep(Duration::from_millis(200));
            false
        });

        let waiting = gate.clone();
        let waiter_probe = slow_probe;
        let started = Instant::now();
        let waiter = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                waiting
                    .ensure_ready(TEST_BUDGET, Duration::from_millis(5), waiter_probe)
                    .await;
            });
        });
        std::thread::sleep(Duration::from_millis(500));
        gate.settled.store(true, Ordering::Relaxed);
        waiter.join().expect("waiter thread panicked");

        // Without the in-loop settled check this would run for the full 5s budget.
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
