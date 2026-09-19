//! Process lifecycle coordination (BUILD PLAN §4-xv / §12).
//!
//! One [`Shutdown`] coordinator per process:
//!   * `signal(reason)` — idempotent, records the first reason, wakes every
//!     waiter (module loops, the HTTP server's graceful shutdown, workers).
//!   * `wait()` / `subscribe()` — modules stop their loops and finish the
//!     in-flight unit of work; the server stops accepting and drains.
//!   * `run_phase(name, timeout, fut)` — the shutdown sequence executes
//!     ordered, individually time-bounded phases (stop feeds → cancel/land
//!     pending work → flush journals → persist state → close pools) so one
//!     hung subsystem can never block exit past the operator's deadline.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{info, warn};

/// Process-wide shutdown coordinator.
pub struct Shutdown {
    signalled: AtomicBool,
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
    /// `std` mutex on purpose: `signal()` must stay callable from sync
    /// contexts (signal handlers); the critical section never awaits.
    reason: std::sync::Mutex<Option<String>>,
}

impl Shutdown {
    pub fn new() -> Arc<Self> {
        let (tx, rx) = watch::channel(false);
        Arc::new(Shutdown {
            signalled: AtomicBool::new(false),
            tx,
            rx,
            reason: std::sync::Mutex::new(None),
        })
    }

    /// Request shutdown. Idempotent: the FIRST reason wins and later signals
    /// only wake stragglers.
    pub fn signal(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let first = !self.signalled.swap(true, Ordering::SeqCst);
        if first {
            info!(%reason, "shutdown requested");
            if let Ok(mut guard) = self.reason.lock() {
                *guard = Some(reason);
            }
            let _ = self.tx.send(true);
        }
    }

    pub fn is_signalled(&self) -> bool {
        self.signalled.load(Ordering::SeqCst)
    }

    /// The first recorded shutdown reason, if any.
    pub fn reason(&self) -> Option<String> {
        self.reason.lock().ok().and_then(|g| g.clone())
    }

    /// Resolve when shutdown has been requested (returns immediately if it
    /// already was).
    pub async fn wait(&self) {
        if self.is_signalled() {
            return;
        }
        let mut rx = self.rx.clone();
        // Ignore send errors: the sender lives as long as this struct.
        let _ = rx.wait_for(|v| *v).await;
    }

    /// A receiver for select!-style loops and axum's graceful shutdown.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }

    /// Future that resolves when shutdown is requested — for
    /// `with_graceful_shutdown`.
    pub async fn notifiers(&self) {
        self.wait().await
    }

    /// Run one shutdown phase with its own deadline. Returns `true` when the
    /// phase finished in time; a `false` is logged loudly but never panics —
    /// shutdown must always progress.
    pub async fn run_phase<F>(&self, name: &str, timeout: Duration, fut: F) -> bool
    where
        F: std::future::Future<Output = ()>,
    {
        match tokio::time::timeout(timeout, fut).await {
            Ok(()) => {
                info!(phase = name, "shutdown phase complete");
                true
            }
            Err(_) => {
                warn!(
                    phase = name,
                    timeout_ms = timeout.as_millis() as u64,
                    "shutdown phase TIMED OUT — continuing"
                );
                false
            }
        }
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        // `Shutdown::new` returns Arc; Default is for struct-literal contexts
        // (tests) that want an owned coordinator.
        let (tx, rx) = watch::channel(false);
        Shutdown {
            signalled: AtomicBool::new(false),
            tx,
            rx,
            reason: std::sync::Mutex::new(None),
        }
    }
}

/// Install OS signal handlers (SIGINT/SIGTERM) that drive the coordinator.
/// Safe to call once from main; on platforms without unix signals the
/// ctrl_c path still works.
pub fn install_signal_handlers(shutdown: Arc<Shutdown>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut term = signal(SignalKind::terminate())
                .map_err(|e| warn!(error = %e, "could not install SIGTERM handler"))
                .ok();
            let mut int = signal(SignalKind::interrupt())
                .map_err(|e| warn!(error = %e, "could not install SIGINT handler"))
                .ok();
            tokio::select! {
                _ = async {
                    match term.as_mut() {
                        Some(t) => { t.recv().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => shutdown.signal("SIGTERM"),
                _ = async {
                    match int.as_mut() {
                        Some(i) => { i.recv().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => shutdown.signal("SIGINT"),
                _ = tokio::signal::ctrl_c() => shutdown.signal("ctrl-c"),
            }
        }
        #[cfg(not(unix))]
        {
            if tokio::signal::ctrl_c().await.is_ok() {
                shutdown.signal("ctrl-c");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signal_wakes_waiters_once_and_records_first_reason() {
        let s = Shutdown::new();
        assert!(!s.is_signalled());
        let waiter = {
            let s2 = s.clone();
            tokio::spawn(async move {
                s2.wait().await;
                "woke"
            })
        };
        tokio::task::yield_now().await;
        s.signal("first");
        s.signal("second");
        assert!(s.is_signalled());
        assert_eq!(waiter.await.unwrap(), "woke");
        // wait() returns immediately once signalled.
        s.wait().await;
        assert_eq!(s.reason().as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn subscribe_reflects_signal() {
        let s = Shutdown::new();
        let mut rx = s.subscribe();
        assert!(!*rx.borrow());
        s.signal("test");
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
    }

    #[tokio::test]
    async fn run_phase_reports_completion_and_timeout() {
        let s = Shutdown::new();
        assert!(
            s.run_phase("fast", Duration::from_millis(500), async {})
                .await
        );
        let slow = s
            .run_phase(
                "slow",
                Duration::from_millis(50),
                tokio::time::sleep(Duration::from_secs(30)),
            )
            .await;
        assert!(!slow, "timed-out phase reports false, does not hang");
    }

    #[tokio::test]
    async fn default_coordinator_works_like_new() {
        let s = Shutdown::default();
        s.signal("d");
        s.wait().await;
        assert!(s.is_signalled());
    }
}
