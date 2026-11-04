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

/// Deterministic, non-executing handoff. Conclusions and issue details remain
/// human claims; state/projection only choose the material recipient.
pub fn handoff_plan(
    phase: &str,
    p: &Projection,
    candidates: &[Candidate],
    invalidated: bool,
    closed: bool,
    hold: Option<&str>,
) -> serde_json::Value {
    use serde_json::json;
    let mut conflicts = vec![];
    let mut blockers = vec![];
    let mut open = vec![];
    for kind in &p.conflicts {
        let mut ids = candidates
            .iter()
            .filter(|c| c.report.report_type == *kind)
            .map(|c| c.receipt_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        conflicts.push(json!({"slot":kind,"candidate_receipt_ids":ids,"required_reference":"SLOT_RESOLUTION with exact complete candidate set, then CONTINUE_MATERIALS","claim_source":"HUMAN_ATTESTED"}));
    }
    for c in candidates
        .iter()
        .filter(|c| p.selected.get(&c.report.report_type) == Some(&c.receipt_id))
    {
        for i in &c.report.issues {
            if i.status == IssueStatus::Open {
                let row = json!({"issue_id":i.id,"blocking":i.blocking,"severity":i.severity,"slot":c.report.report_type,"receipt_id":c.receipt_id,"source":"HUMAN_ATTESTED"});
                if i.blocking {
                    blockers.push(row)
                } else {
                    open.push(row)
                }
            }
        }
        if matches!(
            c.report.claimed_conclusion,
            Conclusion::Fail | Conclusion::Blocked
        ) {
            blockers.push(json!({"issue_id":null,"blocking":true,"reason":"CLAIMED_FAIL_OR_BLOCKED","claimed_conclusion":c.report.claimed_conclusion,"slot":c.report.report_type,"receipt_id":c.receipt_id,"source":"HUMAN_ATTESTED"}));
        }
        if !c
            .report
            .claims
            .author
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            blockers.push(json!({"issue_id":null,"blocking":true,"reason":"AUTHOR_CLAIM_MISSING","slot":c.report.report_type,"receipt_id":c.receipt_id,"source":"HUMAN_ATTESTED"}));
        }
    }
    let (action, recipient, reason) = if let Some(reason) = hold {
        ("STOP_RECONCILE_IDENTITY", "PM", reason)
    } else if closed {
        (
            "REGISTRATION_CLOSED",
            "PM",
            "CLOSURE_REFERENCE_RECORDED_NOT_USER_ACCEPTANCE",
        )
    } else if phase == "PAUSED" {
        ("PAUSED", "PM", "EXPLICIT_CONTINUE_REFERENCE_REQUIRED")
    } else if phase == "CANCELLED" {
        ("CANCELLED", "PM", "REGISTRATION_CANCELLED")
    } else if invalidated {
        (
            "NO_NEW_DELIVERY",
            "PM",
            "NEW_LEGALLY_REFERENCED_DELIVERY_REQUIRED",
        )
    } else if !conflicts.is_empty() {
        ("REQUEST_SLOT_RESOLUTION", "PM", "SLOT_CONFLICT")
    } else if !p.blocking.is_empty() {
        ("HANDOFF_BLOCKED", "PM", "BLOCKING_CLAIMS_REQUIRE_DECISION")
    } else if phase == "PM_DECISION_REQUIRED" {
        ("REQUEST_CONTINUE_REFERENCE", "PM", "NO_AUTOMATIC_RESUME")
    } else if phase == "AWAITING_PM" && p.complete {
        (
            "DELIVER_TO_PM_FOR_CLOSURE",
            "PM",
            "MATERIALS_REGISTERED_NOT_MACHINE_VERIFIED",
        )
    } else if !p.has_development && phase == "HANDOFF_READY" {
        (
            "DELIVER_TO_DEVELOPER",
            "DEVELOPER",
            "DEVELOPMENT_RECEIPT_REQUIRED",
        )
    } else if p.has_development
        && p.missing.contains(&ReportType::Review)
        && p.missing.contains(&ReportType::Ci)
    {
        (
            "COLLECT_REVIEW_AND_VALIDATION",
            "REVIEWER_AND_PM",
            "MISSING_REVIEW_AND_CI",
        )
    } else if p.has_development && p.missing.contains(&ReportType::Review) {
        (
            "DELIVER_TO_REVIEWER",
            "READ_ONLY_REVIEWER",
            "MISSING_REVIEW",
        )
    } else if p.has_development && p.missing.contains(&ReportType::Ci) {
        (
            "REQUEST_PM_VALIDATION",
            "PM",
            "MISSING_CI_PM_MUST_ARRANGE_AUTHORIZED_VALIDATION",
        )
    } else {
        (
            "STOP_RECONCILE_STATE",
            "PM",
            "STATE_OR_MATERIALS_REQUIRE_RECONCILIATION",
        )
    };
    json!({"rule_id":"SH1_HANDOFF_V2","action":action,"recipient":recipient,"reason":reason,"missing_slots":p.missing,"conflicts":conflicts,"blocking_claims":blockers,"nonblocking_open_claims":open,"suggestion_only":true,"execution_authorized":false,"claims_are_not_machine_verification":true})
}
