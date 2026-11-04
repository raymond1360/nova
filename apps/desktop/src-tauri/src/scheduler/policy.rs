//! Pure, provider-independent decisions. Task legality is owned by storage::tasks.
use super::{contract::*, Result};
use crate::models::TaskState;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub fn product_auto_allowed(recorded: u64) -> bool {
    recorded < 1
}
pub fn internal_auto_allowed() -> bool {
    false
}
pub fn phase(state: TaskState) -> Result<&'static str> {
    Ok(match state {
        TaskState::Draft => "DRAFT",
        TaskState::AwaitingConfirmation => "CONFIRMATION_PENDING",
        TaskState::Confirmed => "PACKAGE_RECORDED",
        TaskState::Queued => "HANDOFF_READY",
        TaskState::Running => "DEVELOPMENT_RECORDED",
        TaskState::Verifying => "MATERIAL_COLLECTION",
        TaskState::SafeFixing => "MANUAL_REPAIR_REFERENCED",
        TaskState::PreviewReady => "MATERIALS_RECORDED",
        TaskState::AwaitingUserAcceptance => "AWAITING_PM",
        TaskState::NeedsUserAction => "PM_DECISION_REQUIRED",
        TaskState::Interrupted => "PAUSED",
        TaskState::Cancelled => "CANCELLED",
        _ => return Err("STORE_INCONSISTENT"),
    })
}
#[derive(Clone)]
pub struct Candidate {
    pub receipt_id: String,
    pub report: Report,
}
#[derive(Clone, Default, Serialize)]
pub struct Projection {
    pub selected: BTreeMap<ReportType, String>,
    pub conflicts: Vec<ReportType>,
    pub missing: Vec<ReportType>,
    pub blocking: Vec<String>,
    pub nonblocking_open: Vec<String>,
    pub has_development: bool,
    pub complete: bool,
}
pub fn project(
    required: &[ReportType],
    candidates: &[Candidate],
    resolutions: &[Candidate],
    invalidated: bool,
) -> Projection {
    let mut p = Projection::default();
    for kind in required {
        let group: Vec<_> = candidates
            .iter()
            .filter(|c| c.report.report_type == *kind)
            .collect();
        let ids: BTreeSet<_> = group.iter().map(|c| c.receipt_id.clone()).collect();
        let choice = if group.len() == 1 {
            Some(group[0])
        } else if group.len() > 1 {
            resolutions
                .iter()
                .rev()
                .filter_map(|c| c.report.context.slot_resolution.as_ref())
                .find(|r| r.report_type == *kind)
                .filter(|r| {
                    r.candidate_receipt_ids
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<_>>()
                        == ids
                })
                .and_then(|r| {
                    group
                        .iter()
                        .find(|c| c.receipt_id == r.selected_receipt_id)
                        .copied()
                })
        } else {
            None
        };
        if *kind == ReportType::Development && !group.is_empty() {
            p.has_development = true;
        }
        let Some(c) = choice else {
            if group.is_empty() {
                p.missing.push(*kind)
            } else {
                p.conflicts.push(*kind)
            }
            continue;
        };
        p.selected.insert(*kind, c.receipt_id.clone());
        if matches!(
            c.report.claimed_conclusion,
            Conclusion::Fail | Conclusion::Blocked
        ) {
            p.blocking.push(format!("{:?}_CLAIM", kind));
        } else if c.report.claimed_conclusion != Conclusion::Pass {
            p.missing.push(*kind);
        }
        if !c
            .report
            .claims
            .author
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            p.blocking.push("AUTHOR_CLAIM_MISSING".into());
        }
        for i in &c.report.issues {
            if i.status == IssueStatus::Open {
                if i.blocking {
                    p.blocking.push(i.id.clone())
                } else {
                    p.nonblocking_open.push(i.id.clone())
                }
            }
        }
    }
    if invalidated {
        p.blocking.push("NO_NEW_DELIVERY".into())
    }
    p.complete = p.missing.is_empty() && p.conflicts.is_empty() && p.blocking.is_empty();
    p
}
pub fn validate_history(old: &Report, new: &Report) -> Result<()> {
    let next = issue_map(new);
    for i in &old.issues {
        let n = next.get(i.id.as_str()).ok_or("ISSUE_HISTORY_GAP")?;
        if n.first_seen_sha != i.first_seen_sha
            || (i.status == IssueStatus::Closed && n.status == IssueStatus::Open)
        {
            return Err("ISSUE_HISTORY_GAP");
        }
        if i.status == IssueStatus::Open
            && n.status == IssueStatus::Closed
            && n.closed_sha.as_deref() == Some(old.claimed_delivery_sha.as_str())
        {
            return Err("ISSUE_CLOSE_VERSION_REQUIRED");
        }
    }
    Ok(())
}
