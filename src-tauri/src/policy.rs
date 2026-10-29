use std::time::Duration;

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const FAILURE_RETRY_BACKOFF: Duration = Duration::from_secs(60);
pub const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(10);
// Boot-time network gate: macOS exposes the session before Wi-Fi can reach the internet.
pub const CONNECTIVITY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub const CONNECTIVITY_PROBE_INTERVAL: Duration = Duration::from_secs(1);
pub const CONNECTIVITY_WAIT_BUDGET: Duration = Duration::from_secs(30);
// A timed-out refresh worker that never releases (hung keychain, stuck child) must
// not lock its provider out forever; the flight is force-released after this ceiling.
pub const STUCK_WORKER_CEILING: Duration = Duration::from_secs(5 * 60);
