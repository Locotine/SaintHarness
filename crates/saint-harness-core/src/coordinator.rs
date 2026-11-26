use dashmap::DashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WakeScope {
    SteerOnly = 1,
    DrainAll = 2,
}

impl WakeScope {
    pub fn widest(self, other: Self) -> Self {
        self.max(other)
    }
}

#[derive(Default)]
struct ExecState {
    running: bool,
    pending_wake: Option<WakeScope>,
    cancel: CancellationToken,
}

/// Doorbell coordinator eliminating idle-boundary races across concurrent session wakes.
#[derive(Clone, Default)]
pub struct SessionRunCoordinator {
    sessions: Arc<DashMap<String, Arc<Mutex<ExecState>>>>,
}

impl SessionRunCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    fn state_for(&self, session_id: &str) -> Arc<Mutex<ExecState>> {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(ExecState::default())))
            .clone()
    }

    /// Rings the doorbell for `session_id`. If already running, coalesces `pending_wake = widest(prev, scope)`
    /// so no wake is ever lost at the idle transition boundary.
    pub async fn wake<F, Fut>(&self, session_id: String, scope: WakeScope, work: F)
    where
        F: Fn(WakeScope, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let state_arc = self.state_for(&session_id);
        let token = {
            let mut st = state_arc.lock().await;
            if st.running {
                st.pending_wake = Some(match st.pending_wake {
                    Some(prev) => prev.widest(scope),
                    None => scope,
                });
                return;
            }
            st.running = true;
            st.pending_wake = None;
            st.cancel = CancellationToken::new();
            st.cancel.clone()
        };

        let work = Arc::new(work);
        tokio::spawn(async move {
            let mut current_scope = scope;
            let mut current_token = token;
            loop {
                (work)(current_scope, current_token).await;
                let mut st = state_arc.lock().await;
                if let Some(next_scope) = st.pending_wake.take() {
                    current_scope = next_scope;
                    st.cancel = CancellationToken::new();
                    current_token = st.cancel.clone();
                } else {
                    st.running = false;
                    break;
                }
            }
        });
    }

    pub async fn interrupt(&self, session_id: &str, resume_after: bool) {
        let state_arc = self.state_for(session_id);
        let mut st = state_arc.lock().await;
        st.cancel.cancel();
        if resume_after && st.running {
            st.pending_wake = Some(
                st.pending_wake
                    .map(|p| p.widest(WakeScope::DrainAll))
                    .unwrap_or(WakeScope::DrainAll),
            );
        }
    }

    pub async fn is_running(&self, session_id: &str) -> bool {
        let state_arc = self.state_for(session_id);
        state_arc.lock().await.running
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn fifty_concurrent_wakes_at_idle_boundary_drop_zero_work() {
        let coord = SessionRunCoordinator::new();
        let queued = Arc::new(AtomicUsize::new(0));
        let processed = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..50 {
            let c = coord.clone();
            let q = queued.clone();
            let p = processed.clone();
            handles.push(tokio::spawn(async move {
                q.fetch_add(1, Ordering::SeqCst);
                let q2 = q.clone();
                let p2 = p.clone();
                c.wake(
                    "s1".to_string(),
                    WakeScope::DrainAll,
                    move |_scope, _tok| {
                        let q3 = q2.clone();
                        let p3 = p2.clone();
                        async move {
                            loop {
                                let avail = q3.swap(0, Ordering::SeqCst);
                                if avail == 0 {
                                    break;
                                }
                                p3.fetch_add(avail, Ordering::SeqCst);
                                tokio::task::yield_now().await;
                            }
                        }
                    },
                )
                .await;
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        assert_eq!(processed.load(Ordering::SeqCst), 50);
    }
}
