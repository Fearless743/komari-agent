//! Persistent per-interface traffic accounting used by `month_rotate`.
//!
//! Mirrors `monitoring/netstatic/static.go`. The on-disk JSON shape is:
//!
//! ```json
//! {
//!   "interfaces": {"eth0": [{"timestamp": 1700000000, "tx": 1, "rx": 2}]},
//!   "config": {"data_preserve_day": 31, "detect_interval": 2, "save_interval": 600}
//! }
//! ```
//!
//! The Go implementation samples every two seconds on a background goroutine
//! and flushes the in-memory cache into one record every `save_interval`
//! seconds. This crate has no background scheduler, so the reporting loop
//! samples; deltas are still accumulated in a cache and only merged into a
//! single on-disk record when `save_interval` elapses. This keeps disk writes
//! and record counts independent of the report interval.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

/// Default number of days traffic samples are kept.
pub const DEFAULT_DATA_PRESERVE_DAYS: f64 = 31.0;
/// Default sampling interval in the Go agent (seconds).
pub const DEFAULT_DETECT_INTERVAL: f64 = 2.0;
/// Default save interval in the Go agent (seconds).
pub const DEFAULT_SAVE_INTERVAL: f64 = 600.0;

/// One traffic delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficData {
    pub timestamp: u64,
    /// Bytes transmitted since the previous sample.
    pub tx: u64,
    /// Bytes received since the previous sample.
    pub rx: u64,
}

/// Accounting configuration stored alongside the samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetStaticConfig {
    #[serde(default = "default_preserve_days")]
    pub data_preserve_day: f64,
    #[serde(default = "default_detect_interval")]
    pub detect_interval: f64,
    #[serde(default = "default_save_interval")]
    pub save_interval: f64,
    /// Retained for on-disk compatibility. The live interface filter lives in
    /// [`crate::NicFilter`]; this list is ignored and cleared on load.
    #[serde(default)]
    pub nics: Vec<String>,
}

impl Default for NetStaticConfig {
    fn default() -> Self {
        Self {
            data_preserve_day: DEFAULT_DATA_PRESERVE_DAYS,
            detect_interval: DEFAULT_DETECT_INTERVAL,
            save_interval: DEFAULT_SAVE_INTERVAL,
            nics: Vec::new(),
        }
    }
}

impl NetStaticConfig {
    /// Go's `configOrDefault`: explicit zero values are replaced by defaults.
    pub fn normalized(mut self) -> Self {
        if self.data_preserve_day == 0.0 {
            self.data_preserve_day = DEFAULT_DATA_PRESERVE_DAYS;
        }
        if self.detect_interval == 0.0 {
            self.detect_interval = DEFAULT_DETECT_INTERVAL;
        }
        if self.save_interval == 0.0 {
            self.save_interval = DEFAULT_SAVE_INTERVAL;
        }
        self
    }

    fn save_interval_seconds(&self) -> u64 {
        let interval = self.clone().normalized().save_interval;
        if !interval.is_finite() || interval <= 0.0 {
            DEFAULT_SAVE_INTERVAL as u64
        } else {
            interval as u64
        }
    }
}

fn default_preserve_days() -> f64 {
    DEFAULT_DATA_PRESERVE_DAYS
}

fn default_detect_interval() -> f64 {
    DEFAULT_DETECT_INTERVAL
}

fn default_save_interval() -> f64 {
    DEFAULT_SAVE_INTERVAL
}

/// Whole traffic database (`net_static.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NetStatic {
    #[serde(default)]
    pub interfaces: BTreeMap<String, Vec<TrafficData>>,
    #[serde(default)]
    pub config: NetStaticConfig,
}

/// Mutable in-memory database bound to a file.
///
/// A missing or corrupt file is tolerated: corrupt files are renamed to
/// `<path>.bak` and accounting restarts empty, matching the Go loader.
#[derive(Debug, Clone)]
pub struct NetStaticStore {
    path: PathBuf,
    data: NetStatic,
    last_counters: BTreeMap<String, (u64, u64)>,
    /// Deltas accumulated since the last flush, one entry per report.
    pending: BTreeMap<String, Vec<TrafficData>>,
    last_flush: Option<u64>,
}

impl NetStaticStore {
    /// Load the database at `path`.
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut store = Self {
            path,
            data: NetStatic::default(),
            last_counters: BTreeMap::new(),
            pending: BTreeMap::new(),
            last_flush: None,
        };
        store.reload();
        store
    }

    fn reload(&mut self) {
        let content = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(_) => return,
        };
        if content.trim().is_empty() {
            return;
        }
        match serde_json::from_str::<NetStatic>(&content) {
            Ok(mut data) => {
                data.config = data.config.normalized();
                // The persisted allow-list predates the current runtime
                // filter and may be stale (renamed/deleted interfaces). The
                // report layer owns filtering now.
                data.config.nics.clear();
                self.data = data;
            }
            Err(_) => {
                let backup = self.path.with_extension("json.bak");
                let _ = fs::rename(&self.path, backup);
            }
        }
    }

    /// Replace the accounting configuration. Zero values are normalised to
    /// the Go defaults.
    pub fn set_config(&mut self, config: NetStaticConfig) {
        self.data.config = config.normalized();
    }

    /// Path of the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current flushed database contents.
    pub fn data(&self) -> &NetStatic {
        &self.data
    }

    /// Deltas waiting for the next flush.
    pub fn pending(&self) -> &BTreeMap<String, Vec<TrafficData>> {
        &self.pending
    }

    /// Record one sample of cumulative interface counters, accepting every
    /// interface.
    ///
    /// The first sample for an interface only primes the delta tracker, which
    /// matches Go's `sampleOnceLocked`. Zero deltas are not stored.
    pub fn record(&mut self, counters: &[(String, u64, u64)], timestamp: u64) -> bool {
        self.record_filtered(counters, timestamp, |_| true)
    }

    /// Record one sample, ignoring interfaces rejected by `allowed`.
    ///
    /// Returns `true` when the accumulated cache was flushed into a single
    /// database record because `save_interval` elapsed. The caller should then
    /// persist the database.
    pub fn record_filtered<F>(
        &mut self,
        counters: &[(String, u64, u64)],
        timestamp: u64,
        allowed: F,
    ) -> bool
    where
        F: Fn(&str) -> bool,
    {
        for (name, tx, rx) in counters {
            if !allowed(name) {
                continue;
            }

            if let Some((previous_tx, previous_rx)) = self.last_counters.get(name).copied() {
                let delta_tx = safe_delta(*tx, previous_tx);
                let delta_rx = safe_delta(*rx, previous_rx);
                if delta_tx > 0 || delta_rx > 0 {
                    self.pending
                        .entry(name.clone())
                        .or_default()
                        .push(TrafficData {
                            timestamp,
                            tx: delta_tx,
                            rx: delta_rx,
                        });
                }
            }
            self.last_counters.insert(name.clone(), (*tx, *rx));
        }

        self.flush_if_due(timestamp)
    }

    fn flush_if_due(&mut self, timestamp: u64) -> bool {
        let interval = self.data.config.save_interval_seconds();
        match self.last_flush {
            None => {
                self.last_flush = Some(timestamp);
                false
            }
            Some(previous) if timestamp.saturating_sub(previous) >= interval => {
                self.flush(timestamp)
            }
            Some(_) => false,
        }
    }

    /// Force the cache into one database record.
    ///
    /// Returns `true` when records were written. The caller is responsible for
    /// calling [`NetStaticStore::save`] afterwards.
    pub fn flush(&mut self, timestamp: u64) -> bool {
        self.last_flush = Some(timestamp);
        if self.pending.is_empty() {
            return false;
        }

        for (name, samples) in std::mem::take(&mut self.pending) {
            let mut aggregate = TrafficData {
                timestamp,
                tx: 0,
                rx: 0,
            };
            for sample in samples {
                aggregate.tx = aggregate.tx.saturating_add(sample.tx);
                aggregate.rx = aggregate.rx.saturating_add(sample.rx);
            }
            if aggregate.tx > 0 || aggregate.rx > 0 {
                self.data
                    .interfaces
                    .entry(name)
                    .or_default()
                    .push(aggregate);
            }
        }
        self.purge_expired(timestamp);
        true
    }

    /// Sum traffic for samples whose timestamp is within `[start, end]`
    /// (`0` means unbounded, like the Go filters).
    ///
    /// Pending cache entries are included so callers see up-to-the-second
    /// totals without forcing a flush.
    pub fn totals_between(&self, start: u64, end: u64) -> BTreeMap<String, TrafficData> {
        let mut totals: BTreeMap<String, TrafficData> = BTreeMap::new();
        for (name, samples) in &self.data.interfaces {
            for sample in samples {
                if in_range(sample.timestamp, start, end) {
                    add_traffic(totals.entry(name.clone()).or_default(), *sample);
                }
            }
        }
        for (name, samples) in &self.pending {
            for sample in samples {
                if in_range(sample.timestamp, start, end) {
                    add_traffic(totals.entry(name.clone()).or_default(), *sample);
                }
            }
        }
        totals.retain(|_, total| total.tx > 0 || total.rx > 0);
        totals
    }

    /// Persist the flushed database atomically.
    ///
    /// Pending cache entries are intentionally not written; call
    /// [`NetStaticStore::flush`] first. Errors are returned so the caller can
    /// log them without failing the report.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }

        let mut data = self.data.clone();
        data.config = data.config.normalized();
        data.config.nics.clear();
        let encoded = serde_json::to_vec(&data).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, encoded)?;
        fs::rename(temporary, &self.path)
    }

    /// Remove samples older than `data_preserve_day`.
    pub fn purge_expired(&mut self, now: u64) {
        let preserve_seconds = (self.data.config.data_preserve_day * 86_400.0).max(0.0) as u64;
        let cutoff = now.saturating_sub(preserve_seconds);
        self.data.interfaces.retain(|_, samples| {
            samples.retain(|sample| sample.timestamp >= cutoff);
            !samples.is_empty()
        });
        self.pending.retain(|_, samples| {
            samples.retain(|sample| sample.timestamp >= cutoff);
            !samples.is_empty()
        });
    }
}

fn add_traffic(total: &mut TrafficData, sample: TrafficData) {
    total.tx = total.tx.saturating_add(sample.tx);
    total.rx = total.rx.saturating_add(sample.rx);
}

fn in_range(timestamp: u64, start: u64, end: u64) -> bool {
    (start == 0 || timestamp >= start) && (end == 0 || timestamp <= end)
}

/// Counter delta with wrap/reset protection: a counter that moves backwards is
/// treated as zero instead of underflowing.
///
/// Go subtracts `uint64` directly and would report an enormous rate after a
/// counter reset; this clamp is a deliberate fix (see D-016).
pub fn safe_delta(current: u64, previous: u64) -> u64 {
    current.saturating_sub(previous)
}

/// Go's `utils.GetLastResetDate`: the most recent reset day.
///
/// `reset_day` outside `1..=31` returns `now` unchanged (callers use the whole
/// current timestamp, not midnight). A day that does not exist in a month
/// rolls over to the first day of the following month, which is how Go handles
/// e.g. day 31 in February.
pub fn last_reset_date(reset_day: i64, now: NaiveDate) -> NaiveDate {
    if !(1..=31).contains(&reset_day) {
        return now;
    }
    let reset_day = reset_day as u32;

    let this_month = actual_reset_date(now.year(), now.month(), reset_day);
    if now >= this_month {
        return this_month;
    }

    let (previous_year, previous_month) = if now.month() == 1 {
        (now.year() - 1, 12)
    } else {
        (now.year(), now.month() - 1)
    };
    actual_reset_date(previous_year, previous_month, reset_day)
}

fn actual_reset_date(year: i32, month: u32, reset_day: u32) -> NaiveDate {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date"));
    let last_day = first_of_month
        .checked_add_months(chrono::Months::new(1))
        .and_then(|date| date.pred_opt())
        .expect("valid month end");
    if reset_day <= last_day.day() {
        return NaiveDate::from_ymd_opt(year, month, reset_day).unwrap_or(first_of_month);
    }

    first_of_month
        .checked_add_months(chrono::Months::new(1))
        .unwrap_or(first_of_month)
}

/// Unix timestamp (seconds) of the current reset point.
///
/// For an invalid `reset_day` Go returns the current time (including time of
/// day), so no traffic from the current day is discarded.
pub fn current_reset_timestamp(reset_day: i64) -> u64 {
    let now = Local::now();
    if !(1..=31).contains(&reset_day) {
        return u64::try_from(now.timestamp()).unwrap_or(0);
    }

    let date = last_reset_date(reset_day, now.date_naive());
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
    let local = now
        .timezone()
        .from_local_datetime(&midnight)
        .earliest()
        .unwrap_or(now);
    u64::try_from(local.timestamp()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn counters(tx: u64, rx: u64) -> Vec<(String, u64, u64)> {
        vec![("eth0".to_owned(), tx, rx)]
    }

    #[test]
    fn records_deltas_and_guards_counter_wrap() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("net_static.json");
        let mut store = NetStaticStore::load(&path);

        store.record(&counters(100, 200), 1_000);
        // First sample only primes the tracker.
        assert!(store.data().interfaces.is_empty());

        assert!(!store.record(&counters(160, 260), 1_002));
        assert_eq!(store.pending()["eth0"][0].tx, 60);
        assert_eq!(store.pending()["eth0"][0].rx, 60);
        assert!(store.flush(1_002));
        let samples = &store.data().interfaces["eth0"];
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].tx, 60);
        assert_eq!(samples[0].rx, 60);

        // Counter reset: treated as zero and not stored.
        store.record(&counters(10, 20), 1_004);
        assert!(!store.flush(1_004));
        assert_eq!(store.data().interfaces["eth0"].len(), 1);

        store.save().unwrap();
        let reloaded = NetStaticStore::load(&path);
        assert_eq!(reloaded.data(), store.data());
    }

    #[test]
    fn caches_deltas_until_save_interval_instead_of_one_record_per_report() {
        let directory = tempfile::TempDir::new().unwrap();
        let mut store = NetStaticStore::load(directory.path().join("net_static.json"));
        store.set_config(NetStaticConfig {
            save_interval: 600.0,
            ..NetStaticConfig::default()
        });

        store.record(&counters(0, 0), 1_000);
        for second in 1..=10 {
            assert!(
                !store.record(&counters(second * 100, second * 200), 1_000 + second),
                "no flush before save_interval"
            );
        }
        assert!(store.data().interfaces.is_empty());
        let pending = &store.pending()["eth0"];
        assert_eq!(pending.len(), 10);
        assert_eq!(pending.iter().map(|sample| sample.tx).sum::<u64>(), 1_000);
        assert_eq!(pending.iter().map(|sample| sample.rx).sum::<u64>(), 2_000);
        assert!(!directory.path().join("net_static.json").exists());

        assert!(store.record(&counters(1_100, 2_200), 1_600));
        assert!(store.data().interfaces["eth0"].len() == 1);
        assert_eq!(store.data().interfaces["eth0"][0].tx, 1_100);
        store.save().unwrap();
        assert!(directory.path().join("net_static.json").exists());
    }

    #[test]
    fn totals_between_only_includes_the_requested_window() {
        let directory = tempfile::TempDir::new().unwrap();
        let mut store = NetStaticStore::load(directory.path().join("net_static.json"));
        store.record(&counters(0, 0), 100);
        store.record(&counters(10, 20), 200);
        store.record(&counters(30, 40), 300);
        store.record(&counters(80, 100), 400);

        let totals = store.totals_between(250, 400);
        assert_eq!(totals["eth0"].tx, 70);
        assert_eq!(totals["eth0"].rx, 80);

        let all = store.totals_between(0, 0);
        assert_eq!(all["eth0"].tx, 80);
        assert_eq!(all["eth0"].rx, 100);
    }

    #[test]
    fn corrupt_database_is_backed_up_and_restarted() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("net_static.json");
        fs::write(&path, "{not json").unwrap();

        let store = NetStaticStore::load(&path);
        assert!(store.data().interfaces.is_empty());
        assert!(path.with_extension("json.bak").is_file());
        assert!(!path.exists());
    }

    #[test]
    fn zero_valued_config_is_normalised_on_load() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("net_static.json");
        fs::write(
            &path,
            r#"{"interfaces":{},"config":{"data_preserve_day":0,"detect_interval":0,"save_interval":0}}"#,
        )
        .unwrap();

        let store = NetStaticStore::load(&path);
        assert_eq!(
            store.data().config.data_preserve_day,
            DEFAULT_DATA_PRESERVE_DAYS
        );
        assert_eq!(store.data().config.detect_interval, DEFAULT_DETECT_INTERVAL);
        assert_eq!(store.data().config.save_interval, DEFAULT_SAVE_INTERVAL);
    }

    #[test]
    fn persisted_nic_allow_list_is_ignored_and_cleared() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("net_static.json");
        fs::write(
            &path,
            r#"{"interfaces":{},"config":{"data_preserve_day":31,"detect_interval":2,"save_interval":600,"nics":["eth9"]}}"#,
        )
        .unwrap();

        let mut store = NetStaticStore::load(&path);
        assert!(store.data().config.nics.is_empty());
        store.record(&counters(100, 200), 1_000);
        store.record(&counters(200, 300), 1_002);
        assert_eq!(store.pending()["eth0"][0].tx, 100);
    }

    #[test]
    fn last_reset_date_handles_short_months() {
        assert_eq!(last_reset_date(0, date(2024, 5, 10)), date(2024, 5, 10));
        assert_eq!(last_reset_date(15, date(2024, 5, 10)), date(2024, 4, 15));
        assert_eq!(last_reset_date(15, date(2024, 5, 20)), date(2024, 5, 15));
        // Day 31 in February rolls to March 1.
        assert_eq!(last_reset_date(31, date(2024, 2, 15)), date(2024, 1, 31));
        assert_eq!(last_reset_date(31, date(2024, 2, 29)), date(2024, 1, 31));
        assert_eq!(last_reset_date(31, date(2024, 3, 1)), date(2024, 3, 1));
        // Day 31 in April rolls to May 1; before May 1 the previous reset is
        // March 31.
        assert_eq!(last_reset_date(31, date(2024, 4, 15)), date(2024, 3, 31));
    }

    #[test]
    fn invalid_reset_day_uses_the_current_timestamp() {
        let before = u64::try_from(Local::now().timestamp()).unwrap();
        let zero = current_reset_timestamp(0);
        let high = current_reset_timestamp(32);
        let after = u64::try_from(Local::now().timestamp()).unwrap();

        assert!((before..=after).contains(&zero), "zero={zero}");
        assert!((before..=after).contains(&high), "high={high}");
    }
}
