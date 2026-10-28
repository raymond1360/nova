//! Serialized simulated metering. Missing usage/cancellation retains worst-case reservations.
use super::Result;
use serde::Serialize;
use std::collections::BTreeMap;

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
        })
    }
    pub fn committed_tokens(&self) -> u32 {
        self.attempts
            .values()
            .map(|a| {
                if a.uncertain || !a.settled {
                    a.reserved.max(a.usage.unwrap_or(0))
                } else {
                    a.usage.unwrap_or(a.reserved)
                }
            })
            .sum()
    }
    /// Caller must hold the single run Mutex, including receive+reserve (no check-then-act).
    pub fn reserve(&mut self, id: &str, worst: u32) -> Result<()> {
        self.received += 1;
        let allowed = !id.is_empty()
            && self.received <= self.max_requests
            && !self.closed
            && !self.cancelled
            && worst > 0
            && !self.attempts.contains_key(id)
            && self.attempts.values().all(|a| a.settled && !a.uncertain)
            && self
                .committed_tokens()
                .checked_add(worst)
                .is_some_and(|x| x <= self.max_tokens);
        if !allowed {
            self.denied += 1;
            self.denials.push(id.into());
            self.closed = true;
            self.abort_requested = true;
            return Err("admission denied; request retained in ledger".into());
        }
        self.admitted += 1;
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
        let a = self.attempts.get_mut(id).ok_or("unknown attempt")?;
        if a.settled || a.uncertain {
            return Err("duplicate/late settlement".into());
        }
        a.settled = true;
        a.usage = usage;
        if self.cancelled || usage.is_none() || usage.is_some_and(|u| u > a.reserved) {
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
        if self.closed || self.repairs >= self.repair_limit {
            return Err("repair not admitted".into());
        }
        self.repairs += 1;
        Ok(())
    }
}
