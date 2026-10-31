//! Probe-only admission. Never converts live inputs into SYNTHETIC contracts.
use super::{
    budget::{AdmissionFault, Budget, FaultKind},
    Result,
};
use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Preflight = 0,
    Open = 1,
    Stopped = 2,
}
pub struct Ledger {
    pub budget: Budget,
    read: bool,
    edit: bool,
    read_complete: bool,
    prompt: bool,
}
pub struct RunState {
    state: AtomicU8,
    pub ledger: Mutex<Ledger>,
    fault: Arc<AdmissionFault>,
}
impl RunState {
    pub fn new() -> Result<Arc<Self>> {
        let budget = Budget::mock(0)?;
        let fault = budget.fault_handle();
        Ok(Arc::new(Self {
            state: AtomicU8::new(0),
            ledger: Mutex::new(Ledger {
                budget,
                read: false,
                edit: false,
                read_complete: false,
                prompt: false,
            }),
            fault,
        }))
    }
    pub fn state(&self) -> State {
        match self.state.load(Ordering::SeqCst) & 3 {
            0 => State::Preflight,
            1 => State::Open,
            _ => State::Stopped,
        }
    }
    pub fn stop(&self) {
        self.state.store(2, Ordering::SeqCst)
    }
    pub fn reject_received(&self, deadline: Instant) -> Result<()> {
        self.stop();
        let mut g = self.request_lock(deadline)?;
        g.budget.reserve("", 0)
    }
    pub fn open_once(&self, preflight_complete: bool) -> Result<()> {
        if !preflight_complete || self.fault.is_closed() {
            self.stop();
            return Err("preflight incomplete/fault".into());
        }
        if self
            .state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.stop();
            return Err("duplicate or stopped opening".into());
        }
        Ok(())
    }
    fn lock(&self, deadline: Instant) -> Result<MutexGuard<'_, Ledger>> {
        loop {
            if Instant::now() >= deadline {
                self.stop();
                return Err("ledger deadline".into());
            }
            match self.ledger.try_lock() {
                Ok(g) => return Ok(g),
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    self.stop();
                    return Err("ledger poisoned; counts unknown".into());
                }
                Err(_) => {
                    if Instant::now() >= deadline {
                        self.stop();
                        return Err("ledger deadline; counts unknown".into());
                    }
                    std::thread::sleep(Duration::from_millis(1))
                }
            }
        }
    }
    fn request_lock(&self, deadline: Instant) -> Result<MutexGuard<'_, Ledger>> {
        self.lock(deadline).map_err(|error| {
            self.fault.record_unledgered(if error.contains("poisoned") {
                FaultKind::Poisoned
            } else {
                FaultKind::LockDeadline
            });
            error
        })
    }
    pub fn prompt_once(&self, deadline: Instant) -> Result<()> {
        let mut g = self.lock(deadline)?;
        if self.state() != State::Open || self.fault.is_closed() || g.prompt {
            self.stop();
            return Err("prompt decision rejected".into());
        }
        // Prompt decision and stop share the same atomic word. If stop wins,
        // compare_exchange cannot overwrite terminal Stopped with an open state.
        if self
            .state
            .compare_exchange(1, 5, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            self.stop();
            return Err("prompt lost stop race".into());
        }
        g.prompt = true;
        Ok(())
    }
    pub fn read_completed(&self, deadline: Instant) -> Result<()> {
        let mut g = self.lock(deadline)?;
        if !g.read || g.read_complete || self.state() != State::Open {
            self.stop();
            return Err("unexpected read completion".into());
        }
        g.read_complete = true;
        Ok(())
    }
    /// Waiting never holds the ledger lock. Deadline is shared with the connection.
    pub fn admit(&self, id: &str, edit: bool, deadline: Instant) -> Result<()> {
        loop {
            let mut g = self.request_lock(deadline)?;
            if self.state() != State::Open
                || self.fault.is_closed()
                || !g.prompt
                || (!edit && g.read)
                || (edit && (!g.read || g.edit))
            {
                let r = g.budget.reserve("", 0);
                self.stop();
                return r;
            }
            if edit && !g.read_complete {
                drop(g);
                if Instant::now() >= deadline {
                    let mut g = self.request_lock(deadline)?;
                    let r = g.budget.reserve("", 0);
                    self.stop();
                    return r;
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            let r = g.budget.reserve(id, 1500);
            if r.is_err() {
                self.stop();
                return r;
            }
            if edit {
                g.edit = true
            } else {
                g.read = true
            }
            return Ok(());
        }
    }
    pub fn settle_injected(&self, id: &str, usage: Option<u32>, deadline: Instant) -> Result<()> {
        let mut g = self.lock(deadline)?;
        let r = g.budget.settle(id, usage);
        if r.is_err() {
            self.stop()
        }
        r
    }
    pub fn snapshot(&self, deadline: Instant) -> Result<serde_json::Value> {
        let g = self.lock(deadline)?;
        Ok(
            serde_json::json!({"state":format!("{:?}",self.state()),"ledger":g.budget,"usage_source":"INJECTED","native_facts_verified":false}),
        )
    }
    pub fn proposal_ready(&self, deadline: Instant) -> Result<()> {
        let g = self.lock(deadline)?;
        if self.state() != State::Open
            || self.fault.is_closed()
            || !g.prompt
            || !g.read
            || !g.read_complete
            || !g.edit
            || g.budget.admitted != 2
            || g.budget.received != 2
            || g.budget
                .attempts
                .values()
                .any(|a| !a.settled || a.uncertain)
        {
            self.stop();
            return Err("proposal before complete read/edit ledger".into());
        }
        Ok(())
    }
}
