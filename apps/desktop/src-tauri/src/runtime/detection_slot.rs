//! Serialized single-run slot for a cancellable background task.
//!
//! `AppState.detection_cancel` originally stored a bare
//! `Arc<Mutex<Option<CancellationToken>>>>` that any caller could
//! unconditionally overwrite and unconditionally clear. Two overlapping
//! `detect_runtime_environment` invocations could then race: the second
//! call's token silently replaced the first's, so `cancel` could target the
//! wrong run, and the first call finishing could clear the *second* run's
//! token out from under it. `DetectionSlot` fixes this by only ever
//! allowing one run to be "in" the slot at a time — a second attempt while
//! one is active is rejected outright via [`DetectionSlot::try_start`]
//! rather than allowed to silently replace it.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

/// A slot that holds at most one active run's [`CancellationToken`] at a
/// time.
#[derive(Clone)]
pub struct DetectionSlot {
    current: Arc<Mutex<Option<CancellationToken>>>,
}

/// The slot already has an active run; the caller must not start another
/// one concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyRunning;

/// RAII handle returned by a successful [`DetectionSlot::try_start`]. As
/// long as this is alive, the slot reports a run in progress. Dropping it —
/// on normal completion, an early return, *or* an unexpected panic while
/// the caller awaits its work — clears the slot, so a single stuck/panicked
/// run can never wedge the slot forever.
pub struct DetectionHandle {
    slot: Arc<Mutex<Option<CancellationToken>>>,
}

impl Drop for DetectionHandle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.slot.lock() {
            *guard = None;
        }
    }
}

impl DetectionSlot {
    pub fn new() -> Self {
        Self {
            current: Arc::new(Mutex::new(None)),
        }
    }

    /// Attempt to start a new run. Returns a fresh [`CancellationToken`] and
    /// a [`DetectionHandle`] that must be kept alive for the duration of the
    /// run (its `Drop` clears the slot). Returns [`AlreadyRunning`] instead
    /// of ever overwriting an existing run's token.
    ///
    /// The lock is only ever held for this brief check-and-set — callers
    /// must not hold anything derived from it across an `.await`.
    pub fn try_start(&self) -> Result<(CancellationToken, DetectionHandle), AlreadyRunning> {
        let mut guard = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_some() {
            return Err(AlreadyRunning);
        }
        let cancel = CancellationToken::new();
        *guard = Some(cancel.clone());
        drop(guard);
        Ok((
            cancel,
            DetectionHandle {
                slot: self.current.clone(),
            },
        ))
    }

    /// Cancel whichever run currently holds the slot, if any. Never affects
    /// a run that has already finished and cleared itself out, and can
    /// never target a run other than the one currently in the slot because
    /// at most one run can ever be in the slot at a time.
    pub fn cancel_current(&self) {
        if let Ok(guard) = self.current.lock() {
            if let Some(cancel) = guard.as_ref() {
                cancel.cancel();
            }
        }
    }

    #[cfg(test)]
    fn is_running(&self) -> bool {
        self.current.lock().unwrap().is_some()
    }
}

impl Default for DetectionSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn second_try_start_is_rejected_while_first_is_active() {
        let slot = DetectionSlot::new();
        let (_cancel1, _handle1) = slot.try_start().expect("first start should succeed");
        assert!(matches!(slot.try_start(), Err(AlreadyRunning)));
    }

    #[test]
    fn dropping_the_handle_frees_the_slot_for_reuse() {
        let slot = DetectionSlot::new();
        {
            let (_cancel, _handle) = slot.try_start().expect("first start should succeed");
            assert!(slot.is_running());
        }
        assert!(
            !slot.is_running(),
            "slot must be cleared once the handle drops"
        );
        assert!(
            slot.try_start().is_ok(),
            "a fresh run must be allowed after the previous one ended"
        );
    }

    #[test]
    fn cancel_current_only_cancels_the_active_run() {
        let slot = DetectionSlot::new();
        let (cancel1, handle1) = slot.try_start().expect("start");
        assert!(!cancel1.is_cancelled());
        slot.cancel_current();
        assert!(
            cancel1.is_cancelled(),
            "cancel_current must cancel the currently active run"
        );
        drop(handle1);

        // A brand new run must not start already-cancelled just because a
        // previous, unrelated run was cancelled.
        let (cancel2, _handle2) = slot
            .try_start()
            .expect("second start after the first ended");
        assert!(
            !cancel2.is_cancelled(),
            "a new run's token must not inherit a previous run's cancellation"
        );
    }

    #[test]
    fn cancel_current_on_an_empty_slot_is_a_harmless_no_op() {
        let slot = DetectionSlot::new();
        slot.cancel_current(); // must not panic
    }

    #[test]
    fn a_stale_completion_cannot_clear_a_newer_run_because_only_one_run_ever_exists() {
        // This is the exact bug the review found: an earlier design let an
        // unconditional `*guard = None` at the end of one call clear a
        // *different*, newer call's token. With `DetectionSlot`, that
        // scenario cannot arise structurally: `try_start` refuses to admit
        // a second run while the first is active, so there is never a
        // "newer" run for a stale completion to stomp on. This test proves
        // the sequence explicitly: start run A, attempt (and fail) to start
        // run B while A is active, finish A, then start what is genuinely a
        // new run B' and confirm it has its own independent, uncancelled
        // token.
        let slot = DetectionSlot::new();
        let (cancel_a, handle_a) = slot.try_start().expect("run A starts");
        assert!(
            matches!(slot.try_start(), Err(AlreadyRunning)),
            "run B must be rejected while A is active"
        );
        drop(handle_a); // run A finishes and clears the slot
        assert!(!cancel_a.is_cancelled());

        let (cancel_b, _handle_b) = slot.try_start().expect("run B starts after A finished");
        assert!(
            !cancel_b.is_cancelled(),
            "run B must start with a fresh, uncancelled token"
        );
    }

    #[test]
    fn concurrent_try_start_calls_admit_exactly_one_winner() {
        let slot = DetectionSlot::new();
        let successes = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let mut handles = Vec::new();
        // Keep the one successful handle alive for the whole race so a
        // second thread's `try_start` cannot slip in after the first
        // finishes releasing it before we have counted the winners.
        let winner_handle: Arc<Mutex<Option<DetectionHandle>>> = Arc::new(Mutex::new(None));

        for _ in 0..16 {
            let slot = slot.clone();
            let successes = successes.clone();
            let barrier = barrier.clone();
            let winner_handle = winner_handle.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                if let Ok((_cancel, handle)) = slot.try_start() {
                    successes.fetch_add(1, Ordering::SeqCst);
                    *winner_handle.lock().unwrap() = Some(handle);
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            successes.load(Ordering::SeqCst),
            1,
            "exactly one of 16 concurrent try_start calls must win"
        );
    }

    #[tokio::test]
    async fn slot_survives_a_panic_in_the_awaited_work_via_raii_cleanup() {
        let slot = DetectionSlot::new();
        let slot_for_task = slot.clone();
        let result = tokio::spawn(async move {
            let (_cancel, _handle) = slot_for_task.try_start().expect("start");
            tokio::time::sleep(Duration::from_millis(10)).await;
            panic!("simulated panic while a detection run is in flight");
        })
        .await;
        assert!(result.is_err(), "the spawned task should have panicked");
        assert!(
            !slot.is_running(),
            "the slot must be cleared even though the awaited work panicked, via DetectionHandle::drop"
        );
        assert!(
            slot.try_start().is_ok(),
            "a new run must be startable after the panic"
        );
    }
}
