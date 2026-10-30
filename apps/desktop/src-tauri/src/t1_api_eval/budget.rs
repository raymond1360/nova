//! Serialized simulated metering. Missing usage/cancellation retains worst-case reservations.
use super::Result;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

#[derive(Debug, Default)]
pub(super) struct AdmissionFault {
    closed: AtomicBool,
    lock_deadline: AtomicBool,
    poisoned: AtomicBool,
    unledgered: AtomicU64,
    counter_overflow: AtomicBool,
}
#[derive(Clone, Copy)]
pub(super) enum FaultKind {
    LockDeadline,
    Poisoned,
    ClosedWhileContended,
}
#[derive(Debug, Serialize)]
pub struct FaultSnapshot {
    pub failed_closed: bool,
    pub lock_deadline: bool,
    pub poisoned: bool,
    pub unledgered_received: Option<u64>,
    pub unledgered_denied: Option<u64>,
    pub response_400_unconfirmed: Option<u64>,
    pub association_unknown: bool,
    pub counter_overflow: bool,
    pub ledger_counts_complete: bool,
    pub scope: &'static str,
}
impl AdmissionFault {
    pub(super) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    pub(super) fn record_unledgered(&self, kind: FaultKind) {
        // Publish the permanent safety decision before attempting any accounting.
        self.closed.store(true, Ordering::SeqCst);
        match kind {
            FaultKind::LockDeadline => self.lock_deadline.store(true, Ordering::SeqCst),
            FaultKind::Poisoned => self.poisoned.store(true, Ordering::SeqCst),
            FaultKind::ClosedWhileContended => (),
        }
        if self
            .unledgered
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .is_err()
        {
            self.counter_overflow.store(true, Ordering::SeqCst);
        }
    }
    fn snapshot(&self) -> FaultSnapshot {
        let count = self.unledgered.load(Ordering::SeqCst);
        let overflow = self.counter_overflow.load(Ordering::SeqCst);
        let closed = self.is_closed();
        FaultSnapshot { failed_closed: closed, lock_deadline: self.lock_deadline.load(Ordering::SeqCst),
            poisoned: self.poisoned.load(Ordering::SeqCst), unledgered_received: (!overflow).then_some(count),
            unledgered_denied: (!overflow).then_some(count), response_400_unconfirmed: (!overflow).then_some(count),
            association_unknown: closed, counter_overflow: overflow, ledger_counts_complete: !closed,
            scope: "broker-fault snapshot, not atomic with ledger; received/denied in Budget are ledger-only; unledgered associations and HTTP delivery are unconfirmed" }
    }
}
fn serialize_fault<S: serde::Serializer>(
    fault: &Arc<AdmissionFault>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    fault.snapshot().serialize(s)
}

#[derive(Debug, Clone, Serialize)]
pub struct Attempt {
    pub reserved: u32,
    pub usage: Option<u32>,
    pub settled: bool,
    pub uncertain: bool,
}
#[derive(Debug, Serialize)]
pub struct Budget {
    pub simulated: bool,
    /// Ledger-only count; broker_fault separately accounts for requests that could not acquire this lock.
    pub received: u32,
    pub admitted: u32,
    pub denied: u32,
    pub cancelled: bool,
    pub closed: bool,
    pub abort_requested: bool,
    pub max_requests: u32,
    pub max_tokens: u32,
    pub repair_limit: u8,
    pub repairs: u8,
    pub attempts: BTreeMap<String, Attempt>,
    pub denials: Vec<String>,
    pub arithmetic_error: bool,
    #[serde(serialize_with = "serialize_fault")]
    broker_fault: Arc<AdmissionFault>,
}
impl Budget {
    pub fn mock(repair_limit: u8) -> Result<Self> {
        if repair_limit > 1 {
            return Err("repair limit must be zero or one".into());
        }
        Ok(Self {
            simulated: true,
            received: 0,
            admitted: 0,
            denied: 0,
            cancelled: false,
            closed: false,
            abort_requested: false,
            max_requests: 8,
            max_tokens: 12000,
            repair_limit,
            repairs: 0,
            attempts: BTreeMap::new(),
            denials: Vec::new(),
            arithmetic_error: false,
            broker_fault: Arc::new(AdmissionFault::default()),
        })
    }
    pub(super) fn fault_handle(&self) -> Arc<AdmissionFault> {
        self.broker_fault.clone()
    }
    pub fn fault_snapshot(&self) -> FaultSnapshot {
        self.broker_fault.snapshot()
    }
    pub fn committed_tokens(&self) -> Result<u64> {
        if self.attempts.len() > 8 {
            return Err("ledger entry bound".into());
        }
        self.attempts
            .values()
            .map(|a| {
                if a.uncertain || !a.settled {
                    u64::from(a.reserved.max(a.usage.unwrap_or(0)))
                } else {
                    u64::from(a.usage.unwrap_or(a.reserved))
                }
            })
            .try_fold(0u64, |sum, n| {
                sum.checked_add(n)
                    .filter(|n| *n <= 8 * u64::from(u32::MAX))
                    .ok_or_else(|| "token accounting overflow".into())
            })
    }
    pub fn close(&mut self, abort: bool) {
        self.closed = true;
        self.abort_requested |= abort;
    }
    fn arithmetic_failure(&mut self) -> Result<()> {
        self.arithmetic_error = true;
        self.close(true);
        Err("budget arithmetic/configuration failure".into())
    }
    /// Caller must hold the single run Mutex, including receive+reserve (no check-then-act).
    pub fn reserve(&mut self, id: &str, worst: u32) -> Result<()> {
        self.received = match self.received.checked_add(1) {
            Some(n) => n,
            None => return self.arithmetic_failure(),
        };
        let total = match self.committed_tokens() {
            Ok(n) => n,
            Err(_) => return self.arithmetic_failure(),
        };
        if self.max_requests != 8 || self.max_tokens != 12000 {
            return self.arithmetic_failure();
        }
        let allowed = !id.is_empty()
            && self.received <= self.max_requests
            && !self.closed
            && !self.broker_fault.is_closed()
            && !self.cancelled
            && worst > 0
            && worst <= 12000
            && !self.attempts.contains_key(id)
            && self.attempts.values().all(|a| a.settled && !a.uncertain)
            && total
                .checked_add(u64::from(worst))
                .is_some_and(|x| x <= u64::from(self.max_tokens));
        if !allowed {
            self.denied = match self.denied.checked_add(1) {
                Some(n) => n,
                None => return self.arithmetic_failure(),
            };
            self.denials.push(id.into());
            self.closed = true;
            self.abort_requested = true;
            return Err("admission denied; request retained in ledger".into());
        }
        self.admitted = match self.admitted.checked_add(1) {
            Some(n) => n,
            None => return self.arithmetic_failure(),
        };
        self.attempts.insert(
            id.into(),
            Attempt {
                reserved: worst,
                usage: None,
                settled: false,
                uncertain: false,
            },
        );
        Ok(())
    }
    pub fn settle(&mut self, id: &str, usage: Option<u32>) -> Result<()> {
        if self.committed_tokens().is_err() {
            return self.arithmetic_failure();
        }
        if !self.attempts.contains_key(id) {
            self.close(true);
            return Err("unknown attempt".into());
        }
        let a = self.attempts.get_mut(id).ok_or("unknown attempt")?;
        if a.settled || a.uncertain {
            self.close(true);
            return Err("duplicate/late settlement".into());
        }
        a.settled = true;
        a.usage = usage;
        if self.cancelled || usage.is_none() || usage.is_some_and(|u| u > a.reserved || u > 12000) {
            a.uncertain = true;
            self.closed = true;
            self.abort_requested = true;
            return Err("usage uncertain; worst reservation retained".into());
        }
        Ok(())
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.closed = true;
        self.abort_requested = true;
        for a in self.attempts.values_mut().filter(|a| !a.settled) {
            a.uncertain = true;
        }
    }
    pub fn repair(&mut self) -> Result<()> {
        if self.closed || self.broker_fault.is_closed() || self.repairs >= self.repair_limit {
            return Err("repair not admitted".into());
        }
        self.repairs = match self.repairs.checked_add(1) {
            Some(n) => n,
            None => return self.arithmetic_failure(),
        };
        Ok(())
    }
}

#[cfg(test)]
mod fault_tests {
    use super::*;
    #[test]
    fn fault_counter_overflow_is_unknown_and_permanently_closed() {
        let state = AdmissionFault {
            unledgered: AtomicU64::new(u64::MAX),
            ..Default::default()
        };
        state.record_unledgered(FaultKind::LockDeadline);
        let snapshot = state.snapshot();
        assert!(
            snapshot.failed_closed && snapshot.counter_overflow && !snapshot.ledger_counts_complete
        );
        assert_eq!(snapshot.unledgered_received, None);
        assert_eq!(snapshot.unledgered_denied, None);
        assert_eq!(snapshot.response_400_unconfirmed, None);
    }
}
