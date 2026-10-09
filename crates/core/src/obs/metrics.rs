//! A minimal, dependency-free Prometheus metrics registry.
//!
//! Design rules (see the audit's observability requirements):
//!
//! * **Stable names** — every series is created with an explicit `bot_*` name
//!   and help text; names never embed dynamic data.
//! * **Bounded labels** — label *values* must come from closed sets (module
//!   names, execution modes, matched route patterns, RPC method names baked
//!   into call sites). Nothing user-controlled (symbols, wallet addresses,
//!   signatures, URLs) may ever become a label.
//! * **No duplicate-registration panics** — registration is get-or-create:
//!   asking for the same `(name, labels)` twice returns the same series.
//! * **Deterministic encoding** — series live in `BTreeMap`s, so
//!   [`Registry::encode`] produces byte-stable output for a given state.
//! * **Low overhead** — handles are `Arc<Atomic…>`; incrementing a metric does
//!   not touch the registry lock. Only series *creation* and *encoding* lock.
//!
//! The exposition format is Prometheus text format 0.0.4 (counter / gauge /
//! histogram with `_bucket{le=…}`, `_sum` and `_count` lines).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Default histogram bucket bounds for millisecond latencies. Covers the
/// sub-millisecond paper path up to 30 s RPC/confirm stalls.
pub const LATENCY_BUCKETS_MS: &[u64] = &[
    5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000,
];

/// A sorted label set: the identity of one time series.
type Labels = Vec<(String, String)>;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    name: String,
    labels: Labels,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Counter,
    Gauge,
    Histogram,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Histogram => "histogram",
        }
    }
}

/// Per-metric-name metadata, recorded on first registration.
struct Meta {
    help: String,
    kind: Kind,
    /// Histogram bucket bounds (empty for counters/gauges).
    buckets: Vec<u64>,
}

/// A monotonically increasing counter.
#[derive(Clone)]
pub struct Counter(Arc<AtomicU64>);

impl Counter {
    /// Increment by one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Increment by `n`.
    pub fn inc_by(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    /// Set to an absolute value. Only for mirroring an authoritative
    /// cumulative counter maintained elsewhere (e.g. `ModuleState`), where the
    /// value is guaranteed non-decreasing.
    pub fn set(&self, v: u64) {
        self.0.store(v, Ordering::Relaxed);
    }

    /// Current value.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A value that can go up and down.
#[derive(Clone)]
pub struct Gauge(Arc<AtomicI64>);

impl Gauge {
    /// Set to `v`.
    pub fn set(&self, v: i64) {
        self.0.store(v, Ordering::Relaxed);
    }

    /// Add one.
    pub fn inc(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Subtract one.
    pub fn dec(&self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }

    /// Current value.
    pub fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

struct HistogramInner {
    /// Upper bounds, ascending.
    buckets: Vec<u64>,
    /// `counts[i]` = observations `<= buckets[i]` (cumulative, Prometheus
    /// semantics). One extra slot is not needed: `+Inf` is `count`.
    counts: Vec<AtomicU64>,
    sum: AtomicU64,
    count: AtomicU64,
}

/// A fixed-bucket histogram of `u64` observations (milliseconds in practice).
#[derive(Clone)]
pub struct Histogram(Arc<HistogramInner>);

impl Histogram {
    /// Record one observation.
    pub fn observe(&self, v: u64) {
        let inner = &self.0;
        for (i, bound) in inner.buckets.iter().enumerate() {
            if v <= *bound {
                inner.counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        inner.sum.fetch_add(v, Ordering::Relaxed);
        inner.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of observations.
    pub fn count(&self) -> u64 {
        self.0.count.load(Ordering::Relaxed)
    }

    /// Sum of all observed values.
    pub fn sum(&self) -> u64 {
        self.0.sum.load(Ordering::Relaxed)
    }

    /// Cumulative count for bucket `i` (observations `<= buckets[i]`).
    pub fn bucket(&self, i: usize) -> u64 {
        self.0
            .counts
            .get(i)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Bucket upper bounds.
    pub fn bounds(&self) -> &[u64] {
        &self.0.buckets
    }
}

#[derive(Default)]
struct RegistryInner {
    counters: BTreeMap<Key, Counter>,
    gauges: BTreeMap<Key, Gauge>,
    histograms: BTreeMap<Key, Histogram>,
    meta: BTreeMap<String, Meta>,
}

/// The metrics registry: owns every series and renders the exposition format.
pub struct Registry {
    inner: Mutex<RegistryInner>,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    /// An empty registry. Tests build private registries; production code uses
    /// [`global`].
    pub fn new() -> Self {
        Registry {
            inner: Mutex::new(RegistryInner::default()),
        }
    }

    fn key(name: &str, labels: &[(&str, &str)]) -> Key {
        let mut labels: Labels = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        // Sorted so that label order at the call site never splits a series.
        labels.sort();
        Key {
            name: name.to_string(),
            labels,
        }
    }

    /// Record name-level metadata on first use; later registrations of the
    /// same name reuse it (help/kind/buckets are first-writer-wins).
    fn note_meta(inner: &mut RegistryInner, name: &str, help: &str, kind: Kind, buckets: Vec<u64>) {
        inner.meta.entry(name.to_string()).or_insert(Meta {
            help: help.to_string(),
            kind,
            buckets,
        });
    }

    /// Get-or-create a counter series.
    pub fn counter(&self, name: &str, help: &str, labels: &[(&str, &str)]) -> Counter {
        let key = Self::key(name, labels);
        let mut inner = self.lock();
        Self::note_meta(&mut inner, name, help, Kind::Counter, Vec::new());
        inner
            .counters
            .entry(key)
            .or_insert_with(|| Counter(Arc::new(AtomicU64::new(0))))
            .clone()
    }

    /// Get-or-create a gauge series.
    pub fn gauge(&self, name: &str, help: &str, labels: &[(&str, &str)]) -> Gauge {
        let key = Self::key(name, labels);
        let mut inner = self.lock();
        Self::note_meta(&mut inner, name, help, Kind::Gauge, Vec::new());
        inner
            .gauges
            .entry(key)
            .or_insert_with(|| Gauge(Arc::new(AtomicI64::new(0))))
            .clone()
    }

    /// Get-or-create a histogram series. `buckets` are the upper bounds used
    /// on first registration of this name; later calls reuse them.
    pub fn histogram(
        &self,
        name: &str,
        help: &str,
        labels: &[(&str, &str)],
        buckets: &[u64],
    ) -> Histogram {
        let key = Self::key(name, labels);
        let mut inner = self.lock();
        let bounds = match inner.meta.get(name) {
            Some(m) if !m.buckets.is_empty() => m.buckets.clone(),
            _ => buckets.to_vec(),
        };
        Self::note_meta(&mut inner, name, help, Kind::Histogram, bounds.clone());
        inner
            .histograms
            .entry(key)
            .or_insert_with(|| {
                Histogram(Arc::new(HistogramInner {
                    counts: (0..bounds.len()).map(|_| AtomicU64::new(0)).collect(),
                    buckets: bounds,
                    sum: AtomicU64::new(0),
                    count: AtomicU64::new(0),
                }))
            })
            .clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        // A poisoned lock means a panic while holding it; the maps are still
        // structurally valid, so recover rather than cascade the failure.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of distinct series (all kinds). Used by tests and diagnostics.
    pub fn series_count(&self) -> usize {
        let inner = self.lock();
        inner.counters.len() + inner.gauges.len() + inner.histograms.len()
    }

    /// Render every series in the Prometheus text exposition format (0.0.4).
    /// Output is deterministic: names sort lexicographically, then labels.
    pub fn encode(&self) -> String {
        let inner = self.lock();
        let mut out = String::with_capacity(inner.meta.len() * 256);

        for (name, meta) in &inner.meta {
            let _ = writeln!(out, "# HELP {name} {}", escape_help(&meta.help));
            let _ = writeln!(out, "# TYPE {name} {}", meta.kind.as_str());
            match meta.kind {
                Kind::Counter => {
                    for (key, c) in inner.counters.range(range_for(name)) {
                        let _ = writeln!(
                            out,
                            "{}{} {}",
                            key.name,
                            render_labels(&key.labels),
                            c.get()
                        );
                    }
                }
                Kind::Gauge => {
                    for (key, g) in inner.gauges.range(range_for(name)) {
                        let _ = writeln!(
                            out,
                            "{}{} {}",
                            key.name,
                            render_labels(&key.labels),
                            g.get()
                        );
                    }
                }
                Kind::Histogram => {
                    for (key, h) in inner.histograms.range(range_for(name)) {
                        for (i, bound) in h.bounds().iter().enumerate() {
                            let mut labels = key.labels.clone();
                            labels.push(("le".to_string(), bound.to_string()));
                            labels.sort();
                            let _ = writeln!(
                                out,
                                "{name}_bucket{} {}",
                                render_labels(&labels),
                                h.bucket(i)
                            );
                        }
                        let mut inf = key.labels.clone();
                        inf.push(("le".to_string(), "+Inf".to_string()));
                        inf.sort();
                        let _ = writeln!(out, "{name}_bucket{} {}", render_labels(&inf), h.count());
                        let _ =
                            writeln!(out, "{name}_sum{} {}", render_labels(&key.labels), h.sum());
                        let _ = writeln!(
                            out,
                            "{name}_count{} {}",
                            render_labels(&key.labels),
                            h.count()
                        );
                    }
                }
            }
        }
        out
    }
}

/// BTree range covering exactly the keys with this metric name.
fn range_for(name: &str) -> (std::ops::Bound<Key>, std::ops::Bound<Key>) {
    use std::ops::Bound;
    let low = Key {
        name: name.to_string(),
        labels: Vec::new(),
    };
    // Exclusive upper bound: the smallest key greater than every key with this
    // name. Appending the maximum char makes the name sort above all of its
    // ASCII-prefixed extensions (metric names are ASCII `bot_*`), and an empty
    // label set is the minimum for that name.
    let mut high_name = name.to_string();
    high_name.push('\u{10FFFF}');
    let high = Key {
        name: high_name,
        labels: Vec::new(),
    };
    (Bound::Included(low), Bound::Excluded(high))
}

fn render_labels(labels: &Labels) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut s = String::from("{");
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{k}=\"{}\"", escape_label_value(v));
    }
    s.push('}');
    s
}

fn escape_label_value(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '\n' => s.push_str("\\n"),
            other => s.push(other),
        }
    }
    s
}

fn escape_help(v: &str) -> String {
    let mut s = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            other => s.push(other),
        }
    }
    s
}

/// The process-wide registry. All production instrumentation records here;
/// `/metrics` encodes from here.
pub fn global() -> &'static Registry {
    static GLOBAL: OnceLock<Registry> = OnceLock::new();
    GLOBAL.get_or_init(Registry::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments_and_reads_back() {
        let reg = Registry::new();
        let c = reg.counter("bot_test_total", "test counter", &[("a", "1")]);
        assert_eq!(c.get(), 0);
        c.inc();
        c.inc_by(4);
        assert_eq!(c.get(), 5);
        // A different label value is a different series.
        let other = reg.counter("bot_test_total", "test counter", &[("a", "2")]);
        assert_eq!(other.get(), 0);
        assert_eq!(c.get(), 5);
    }

    #[test]
    fn registration_is_get_or_create_not_duplicate() {
        let reg = Registry::new();
        let c1 = reg.counter("bot_dup_total", "h", &[("k", "v")]);
        let c2 = reg.counter("bot_dup_total", "h", &[("k", "v")]);
        c1.inc();
        assert_eq!(c2.get(), 1, "same (name,labels) must be the same series");
        assert_eq!(reg.series_count(), 1);
        // Label order at the call site must not split the series.
        let c3 = reg.counter("bot_pair_total", "h", &[("b", "2"), ("a", "1")]);
        let c4 = reg.counter("bot_pair_total", "h", &[("a", "1"), ("b", "2")]);
        c3.inc();
        assert_eq!(c4.get(), 1);
        assert_eq!(reg.series_count(), 2);
    }

    #[test]
    fn gauge_supports_negative_values() {
        let reg = Registry::new();
        let g = reg.gauge("bot_test_gauge", "h", &[]);
        g.set(-5);
        assert_eq!(g.get(), -5);
        g.inc();
        g.dec();
        g.dec();
        assert_eq!(g.get(), -6);
    }

    #[test]
    fn histogram_buckets_are_cumulative_and_inclusive() {
        let reg = Registry::new();
        let h = reg.histogram("bot_test_ms", "h", &[], &[10, 100, 1000]);
        for v in [5, 10, 99, 100, 101, 5000] {
            h.observe(v);
        }
        assert_eq!(h.count(), 6);
        assert_eq!(h.sum(), 5 + 10 + 99 + 100 + 101 + 5000);
        // <= 10: {5, 10}
        assert_eq!(h.bucket(0), 2);
        // <= 100: {5,10,99,100} — the bound itself is inclusive.
        assert_eq!(h.bucket(1), 4);
        // <= 1000: adds 101.
        assert_eq!(h.bucket(2), 5);
    }

    #[test]
    fn encode_is_deterministic_and_wellformed() {
        let reg = Registry::new();
        reg.counter("bot_b_total", "Second metric.", &[("m", "x")])
            .inc();
        reg.gauge("bot_a_gauge", "First metric.", &[]).set(7);
        reg.histogram("bot_h_ms", "Hist.", &[("m", "y")], &[10])
            .observe(3);

        let text = reg.encode();
        let again = reg.encode();
        assert_eq!(text, again, "encoding must be deterministic");

        // Names sort lexicographically: a < b < h.
        let a = text.find("bot_a_gauge").unwrap();
        let b = text.find("bot_b_total").unwrap();
        let h = text.find("bot_h_ms").unwrap();
        assert!(a < b && b < h);

        assert!(text.contains("# HELP bot_a_gauge First metric.\n"));
        assert!(text.contains("# TYPE bot_a_gauge gauge\n"));
        assert!(text.contains("bot_a_gauge 7\n"));
        assert!(text.contains("# TYPE bot_b_total counter\n"));
        assert!(text.contains("bot_b_total{m=\"x\"} 1\n"));
        assert!(text.contains("# TYPE bot_h_ms histogram\n"));
        assert!(text.contains("bot_h_ms_bucket{le=\"10\",m=\"y\"} 1\n"));
        assert!(text.contains("bot_h_ms_bucket{le=\"+Inf\",m=\"y\"} 1\n"));
        assert!(text.contains("bot_h_ms_sum{m=\"y\"} 3\n"));
        assert!(text.contains("bot_h_ms_count{m=\"y\"} 1\n"));
    }

    #[test]
    fn encode_escapes_label_values() {
        let reg = Registry::new();
        reg.counter(
            "bot_esc_total",
            "h",
            &[("v", "quo\"te back\\slash nl\nend")],
        )
        .inc();
        let text = reg.encode();
        assert!(text.contains(r#"bot_esc_total{v="quo\"te back\\slash nl\nend"} 1"#));
        assert!(!text.contains("quo\"te back\\slash nl\nend\" 1"));
    }

    #[test]
    fn concurrent_increments_are_not_lost() {
        let reg = Arc::new(Registry::new());
        let c = reg.counter("bot_conc_total", "h", &[("k", "v")]);
        let mut handles = Vec::new();
        for _ in 0..8 {
            let c = c.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..1_000 {
                    c.inc();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(c.get(), 8_000);
    }

    #[test]
    fn histogram_buckets_are_first_registration_wins() {
        let reg = Registry::new();
        let h1 = reg.histogram("bot_hm_ms", "h", &[("a", "1")], &[10, 20]);
        let h2 = reg.histogram("bot_hm_ms", "h", &[("a", "2")], &[999]);
        assert_eq!(h1.bounds(), h2.bounds(), "same name shares bucket bounds");
        assert_eq!(h2.bounds(), &[10, 20]);
    }

    #[test]
    fn global_registry_is_usable() {
        // Just prove the singleton initializes and records; no value asserts
        // (other tests share the process-wide registry).
        global()
            .gauge("bot_obs_selftest", "Self-test gauge.", &[])
            .set(1);
        assert!(global().encode().contains("bot_obs_selftest 1"));
    }
}
