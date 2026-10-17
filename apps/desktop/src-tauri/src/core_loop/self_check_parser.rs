//! WP2A: parses the developer AI's structured self-check block out of its
//! captured process output. Deliberately a strict, structural parse (fixed
//! start/end markers + JSON) — never a free-text search for words like
//! "通过"/"passed". Anything that doesn't match the expected shape exactly
//! is `invalid_format`, which the completed gate treats identically to a
//! missing self-check (see `storage::tasks::evaluate_completion_gate`).

use serde::Deserialize;

/// Markers the development/fix prompt requires the AI to wrap its
/// structured self-check JSON in. Chosen to be extremely unlikely to
/// appear in ordinary output otherwise.
pub const SELF_CHECK_START_MARKER: &str = "===NOVA_SELF_CHECK_START===";
pub const SELF_CHECK_END_MARKER: &str = "===NOVA_SELF_CHECK_END===";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCoverageEntry {
    item: String,
    status: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSelfCheckBlock {
    requirement_coverage: Vec<RawCoverageEntry>,
    check_results: Vec<String>,
    unfinished_items: Vec<String>,
    blocking_issues: Vec<String>,
    self_reported_status: String,
}

/// The outcome of parsing one self-check block, ready to persist via
/// `Database::record_self_check`. `raw_block_redacted` is always set
/// (even on `Invalid`) so the original text remains available for
/// technical-detail display/audit regardless of parse outcome.
pub enum ParsedSelfCheck {
    Valid {
        self_reported_status: String,
        requirement_coverage_json: String,
        check_results_json: String,
        unfinished_items_json: String,
        blocking_issues_json: String,
        raw_block_redacted: String,
    },
    Invalid {
        raw_block_redacted: String,
    },
}

/// Extracts the text strictly between the first `SELF_CHECK_START_MARKER`
/// and the first following `SELF_CHECK_END_MARKER`. `None` if either
/// marker is missing, in the wrong order, or the extracted text is blank
/// after trimming (an empty/title-only block, per the "no space for a
/// disguised empty self-check" requirement).
fn extract_block(output: &str) -> Option<&str> {
    if output.matches(SELF_CHECK_START_MARKER).count() != 1
        || output.matches(SELF_CHECK_END_MARKER).count() != 1 { return None; }
    let start = output.find(SELF_CHECK_START_MARKER)?;
    if output.find(SELF_CHECK_END_MARKER)? < start { return None; }
    let after_start = start + SELF_CHECK_START_MARKER.len();
    let end_relative = output[after_start..].find(SELF_CHECK_END_MARKER)?;
    let block = output[after_start..after_start + end_relative].trim();
    if block.is_empty() {
        None
    } else {
        Some(block)
    }
}

/// Parses one round's self-check out of the developer AI's captured
/// process output (`ExecutionResult::stdout`). Never panics — any
/// malformed input becomes [`ParsedSelfCheck::Invalid`], never an `Err`,
/// since "the AI produced garbage" is an expected, handled outcome, not an
/// exceptional one.
pub fn parse_self_check(raw_output: &str) -> ParsedSelfCheck {
    let raw_block_redacted = crate::security::redact_sensitive_text(raw_output);
    let Some(block) = extract_block(raw_output) else {
        return ParsedSelfCheck::Invalid { raw_block_redacted };
    };
    let parsed: RawSelfCheckBlock = match serde_json::from_str(block) {
        Ok(value) => value,
        Err(_) => return ParsedSelfCheck::Invalid { raw_block_redacted },
    };
    if !matches!(
        parsed.self_reported_status.as_str(),
        "self_reported_complete" | "self_reported_incomplete" | "self_reported_blocked"
    ) {
        return ParsedSelfCheck::Invalid { raw_block_redacted };
    }
    if parsed.requirement_coverage.iter().any(|e| e.item.trim().is_empty()
        || !matches!(e.status.as_str(), "covered" | "partial" | "not_covered")) {
        return ParsedSelfCheck::Invalid { raw_block_redacted };
    }
    // Re-serialize each field independently (rather than storing the raw
    // sub-slice) so the persisted JSON is always exactly what the rest of
    // the codebase expects to deserialize back out, regardless of
    // formatting/whitespace the AI used in its own output.
    let requirement_coverage_json = serde_json::to_string(
        &parsed
            .requirement_coverage
            .iter()
            .map(|entry| serde_json::json!({"item": entry.item, "status": entry.status}))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let check_results_json = serde_json::to_string(&parsed.check_results).unwrap_or_else(|_| "[]".to_string());
    let unfinished_items_json = serde_json::to_string(&parsed.unfinished_items).unwrap_or_else(|_| "[]".to_string());
    let blocking_issues_json = serde_json::to_string(&parsed.blocking_issues).unwrap_or_else(|_| "[]".to_string());

    ParsedSelfCheck::Valid {
        self_reported_status: parsed.self_reported_status,
        requirement_coverage_json,
        check_results_json,
        unfinished_items_json,
        blocking_issues_json,
        raw_block_redacted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(body: &str) -> String {
        format!("some AI chatter\n{SELF_CHECK_START_MARKER}\n{body}\n{SELF_CHECK_END_MARKER}\nmore chatter")
    }

    #[test]
    fn parses_a_well_formed_complete_block() {
        let output = wrap(
            r#"{"requirement_coverage":[{"item":"goal:0","status":"covered"}],"check_results":["ran eslint"],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"self_reported_complete"}"#,
        );
        match parse_self_check(&output) {
            ParsedSelfCheck::Valid { self_reported_status, unfinished_items_json, .. } => {
                assert_eq!(self_reported_status, "self_reported_complete");
                assert_eq!(unfinished_items_json, "[]");
            }
            ParsedSelfCheck::Invalid { .. } => panic!("expected a valid parse"),
        }
    }

    #[test]
    fn missing_markers_is_invalid() {
        let output = "no markers here at all";
        assert!(matches!(parse_self_check(output), ParsedSelfCheck::Invalid { .. }));
    }

    #[test]
    fn empty_block_is_invalid() {
        let output = wrap("   ");
        assert!(matches!(parse_self_check(&output), ParsedSelfCheck::Invalid { .. }));
    }

    #[test]
    fn malformed_json_is_invalid() {
        let output = wrap("{not valid json");
        assert!(matches!(parse_self_check(&output), ParsedSelfCheck::Invalid { .. }));
    }

    #[test]
    fn unknown_self_reported_status_is_invalid() {
        let output = wrap(
            r#"{"requirement_coverage":[],"check_results":[],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"done"}"#,
        );
        assert!(matches!(parse_self_check(&output), ParsedSelfCheck::Invalid { .. }));
    }

    #[test]
    fn incomplete_status_with_empty_arrays_still_parses_as_valid_but_reports_incomplete() {
        // Parsing succeeds (the block is well-formed) — it is the
        // completed gate's job (not the parser's) to reject
        // self_reported_incomplete regardless of empty arrays.
        let output = wrap(
            r#"{"requirement_coverage":[],"check_results":[],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"self_reported_incomplete"}"#,
        );
        match parse_self_check(&output) {
            ParsedSelfCheck::Valid { self_reported_status, .. } => {
                assert_eq!(self_reported_status, "self_reported_incomplete");
            }
            ParsedSelfCheck::Invalid { .. } => panic!("a well-formed incomplete block still parses"),
        }
    }
}
