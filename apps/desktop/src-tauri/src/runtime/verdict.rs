//! Phase 3 remediation (Major 2): structured JSON reviewer verdict protocol.
//!
//! Replaces the original free-text `NOVA_REVIEW_VERDICT: <value>` line-scan
//! (`ai_adapter::parse_review_verdict`, kept only as a doc-comment memory of
//! the old convention) with strict parsing of exactly one JSON object:
//!
//! ```json
//! {"schema_version": 1, "task_id": "...", "result_generation": 1, "round": 1, "verdict": "approve", "summary": "...", "findings": ["..."]}
//! ```
//!
//! Every rejection path below routes to [`VerdictParseError`] rather than
//! ever guessing — `core_loop` treats *any* `Err` here identically to a
//! review-process failure (`insufficient_evidence` / `needs_user_action`),
//! never as a silent default verdict. This closes a prompt-injection vector
//! the old line-scanner was exposed to: project file content or reviewer
//! prose containing the literal string `NOVA_REVIEW_VERDICT: APPROVE` could
//! previously be mistaken for the real verdict. A JSON object must also
//! carry the exact `task_id`/`result_generation`/`round` Nova expects for
//! *this* invocation, so even a syntactically valid verdict object copied
//! from a different round/task can never be accepted.

use std::collections::HashMap;

use serde::Deserialize;

use crate::runtime::ai_adapter::ReviewVerdict;

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictParseError {
    /// No `{...}` object found anywhere in the output.
    NoJsonFound,
    /// More than one fenced ```json block (or top-level object) was found —
    /// deliberately treated as ambiguous rather than guessing which one is
    /// authoritative (e.g. a markdown example block plus a real verdict).
    MultipleJsonCandidates,
    /// The located object text did not parse as valid JSON, or contained a
    /// duplicate key.
    InvalidJson(String),
    /// Content other than whitespace surrounds the JSON object, and it was
    /// not extracted from a single fenced code block (which permits prose
    /// around it).
    TrailingContent,
    /// The object parsed, but is missing a required field or has the wrong
    /// type for one.
    MissingOrInvalidField(String),
    UnsupportedSchemaVersion(i64),
    UnknownVerdict(String),
    TaskIdMismatch {
        expected: String,
        found: String,
    },
    GenerationMismatch {
        expected: i64,
        found: i64,
    },
    RoundMismatch {
        expected: i64,
        found: i64,
    },
}

impl std::fmt::Display for VerdictParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoJsonFound => write!(f, "输出中未找到结构化 JSON 结论对象"),
            Self::MultipleJsonCandidates => {
                write!(f, "输出中存在多个可能的 JSON 结论对象，无法确定唯一结论")
            }
            Self::InvalidJson(detail) => write!(f, "JSON 结论对象解析失败：{detail}"),
            Self::TrailingContent => write!(f, "JSON 结论对象周围存在非空白的额外内容"),
            Self::MissingOrInvalidField(field) => {
                write!(f, "JSON 结论对象缺少或字段类型错误：{field}")
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "不支持的 schema_version：{version}")
            }
            Self::UnknownVerdict(value) => write!(f, "未知的 verdict 取值：{value}"),
            Self::TaskIdMismatch { expected, found } => {
                write!(f, "task_id 不匹配：期望 {expected}，实际 {found}")
            }
            Self::GenerationMismatch { expected, found } => {
                write!(f, "result_generation 不匹配：期望 {expected}，实际 {found}")
            }
            Self::RoundMismatch { expected, found } => {
                write!(f, "round 不匹配：期望 {expected}，实际 {found}")
            }
        }
    }
}

/// The current task context a verdict must match exactly.
pub struct VerdictContext<'a> {
    pub task_id: &'a str,
    pub result_generation: i64,
    pub round: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVerdict {
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub findings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawPayload {
    schema_version: i64,
    task_id: String,
    result_generation: i64,
    round: i64,
    verdict: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    findings: Vec<String>,
}

/// Parses `text` (a reviewer process's full, already exit-code-0-gated
/// stdout — the caller must check the exit code separately, never call this
/// on the output of a non-zero-exit process and treat a parse success as
/// meaningful) into a [`ParsedVerdict`], or a specific [`VerdictParseError`].
pub fn parse_structured_verdict(
    text: &str,
    ctx: &VerdictContext<'_>,
) -> Result<ParsedVerdict, VerdictParseError> {
    let candidate = extract_single_json_candidate(text)?;
    let map = parse_object_rejecting_duplicate_keys(&candidate)
        .map_err(VerdictParseError::InvalidJson)?;
    let payload: RawPayload = serde_json::from_value(serde_json::Value::Object(map))
        .map_err(|error| VerdictParseError::MissingOrInvalidField(error.to_string()))?;

    if payload.schema_version != SCHEMA_VERSION {
        return Err(VerdictParseError::UnsupportedSchemaVersion(
            payload.schema_version,
        ));
    }
    if payload.task_id != ctx.task_id {
        return Err(VerdictParseError::TaskIdMismatch {
            expected: ctx.task_id.to_string(),
            found: payload.task_id,
        });
    }
    if payload.result_generation != ctx.result_generation {
        return Err(VerdictParseError::GenerationMismatch {
            expected: ctx.result_generation,
            found: payload.result_generation,
        });
    }
    if payload.round != ctx.round {
        return Err(VerdictParseError::RoundMismatch {
            expected: ctx.round,
            found: payload.round,
        });
    }
    let verdict = match payload.verdict.as_str() {
        "approve" => ReviewVerdict::Approve,
        "changes_required" => ReviewVerdict::ChangesRequired,
        "insufficient_evidence" => ReviewVerdict::InsufficientEvidence,
        other => return Err(VerdictParseError::UnknownVerdict(other.to_string())),
    };

    Ok(ParsedVerdict {
        verdict,
        summary: payload.summary,
        findings: payload.findings,
    })
}

/// Locates exactly one JSON-object candidate in `text`: either the single
/// fenced ` ```json ... ``` ` (or bare ` ``` ... ``` ` whose content starts
/// with `{`) code block, if exactly one exists, or — if no fenced block
/// exists at all — the entire trimmed text, which must then be nothing but
/// one JSON object (no surrounding prose). Two or more fenced candidates is
/// always an error, never a "pick the last one" guess.
fn extract_single_json_candidate(text: &str) -> Result<String, VerdictParseError> {
    let fenced = find_fenced_json_blocks(text);
    match fenced.len() {
        1 => Ok(fenced.into_iter().next().unwrap()),
        0 => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(VerdictParseError::NoJsonFound);
            }
            let (object_text, start, end) =
                extract_first_balanced_object(trimmed).ok_or(VerdictParseError::NoJsonFound)?;
            if start != 0 || end != trimmed.len() {
                return Err(VerdictParseError::TrailingContent);
            }
            Ok(object_text)
        }
        _ => Err(VerdictParseError::MultipleJsonCandidates),
    }
}

/// Scans for fenced Markdown code blocks (` ``` ` ... ` ``` `, optionally
/// tagged `json`) whose trimmed content begins with `{` — i.e. blocks that
/// look like they are meant to carry the verdict object, as opposed to a
/// code sample in some other language. Returns each such block's raw
/// content (not the fence markers themselves).
fn find_fenced_json_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut buffer: Option<Vec<&str>> = None;
    for line in text.lines() {
        let trimmed_line = line.trim_start();
        if let Some(ref mut current) = buffer {
            if trimmed_line.starts_with("```") {
                let content = current.join("\n");
                if content.trim_start().starts_with('{') {
                    blocks.push(content);
                }
                buffer = None;
            } else {
                current.push(line);
            }
        } else if trimmed_line.starts_with("```") {
            buffer = Some(Vec::new());
        }
    }
    blocks
}

/// Finds the first `{...}` span in `text` with balanced braces, correctly
/// skipping over braces that appear inside JSON string literals (including
/// escaped quotes/backslashes within those strings). Returns the object's
/// own text plus its `[start, end)` byte range within `text`.
fn extract_first_balanced_object(text: &str) -> Option<(String, usize, usize)> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    let mut index = start;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let end = index + 1;
                        return Some((text[start..end].to_string(), start, end));
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

/// Parses `text` as a single JSON object, rejecting a duplicate top-level
/// key outright rather than silently keeping only the last occurrence
/// (`serde_json`'s own default `Map`/struct deserialization has no built-in
/// way to detect this — it always keeps the last value for a repeated key).
fn parse_object_rejecting_duplicate_keys(
    text: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    struct DuplicateCheckingVisitor;

    impl<'de> serde::de::Visitor<'de> for DuplicateCheckingVisitor {
        type Value = serde_json::Map<String, serde_json::Value>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON object with no duplicate keys")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut seen: HashMap<String, ()> = HashMap::new();
            let mut result = serde_json::Map::new();
            while let Some((key, value)) = access.next_entry::<String, serde_json::Value>()? {
                if seen.insert(key.clone(), ()).is_some() {
                    return Err(serde::de::Error::custom(format!("重复字段：{key}")));
                }
                result.insert(key, value);
            }
            Ok(result)
        }
    }

    use serde::Deserializer as _;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    deserializer
        .deserialize_map(DuplicateCheckingVisitor)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> VerdictContext<'static> {
        VerdictContext {
            task_id: "task-1",
            result_generation: 1,
            round: 1,
        }
    }

    fn valid_json(verdict: &str) -> String {
        format!(
            r#"{{"schema_version":1,"task_id":"task-1","result_generation":1,"round":1,"verdict":"{verdict}","summary":"ok","findings":["f1","f2"]}}"#
        )
    }

    #[test]
    fn parses_the_three_normal_verdicts() {
        assert_eq!(
            parse_structured_verdict(&valid_json("approve"), &ctx())
                .unwrap()
                .verdict,
            ReviewVerdict::Approve
        );
        assert_eq!(
            parse_structured_verdict(&valid_json("changes_required"), &ctx())
                .unwrap()
                .verdict,
            ReviewVerdict::ChangesRequired
        );
        assert_eq!(
            parse_structured_verdict(&valid_json("insufficient_evidence"), &ctx())
                .unwrap()
                .verdict,
            ReviewVerdict::InsufficientEvidence
        );
    }

    #[test]
    fn parses_from_a_single_fenced_json_block_with_surrounding_prose() {
        let text = format!(
            "Here is my review.\n\n```json\n{}\n```\n\nThanks.",
            valid_json("approve")
        );
        let parsed = parse_structured_verdict(&text, &ctx()).unwrap();
        assert_eq!(parsed.verdict, ReviewVerdict::Approve);
    }

    #[test]
    fn rejects_multiple_fenced_json_blocks() {
        let text = format!(
            "example:\n```json\n{}\n```\nreal:\n```json\n{}\n```",
            valid_json("approve"),
            valid_json("changes_required")
        );
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::MultipleJsonCandidates)
        );
    }

    #[test]
    fn rejects_duplicate_verdict_field() {
        let text = r#"{"schema_version":1,"task_id":"task-1","result_generation":1,"round":1,"verdict":"approve","verdict":"changes_required","summary":"x"}"#;
        let result = parse_structured_verdict(text, &ctx());
        assert!(matches!(result, Err(VerdictParseError::InvalidJson(_))));
    }

    #[test]
    fn rejects_missing_required_field() {
        let text =
            r#"{"schema_version":1,"task_id":"task-1","result_generation":1,"verdict":"approve"}"#;
        assert!(matches!(
            parse_structured_verdict(text, &ctx()),
            Err(VerdictParseError::MissingOrInvalidField(_))
        ));
    }

    #[test]
    fn rejects_wrong_task_id() {
        let text = valid_json("approve").replace("task-1", "task-2");
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::TaskIdMismatch {
                expected: "task-1".to_string(),
                found: "task-2".to_string(),
            })
        );
    }

    #[test]
    fn rejects_wrong_result_generation() {
        let text =
            valid_json("approve").replace("\"result_generation\":1", "\"result_generation\":2");
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::GenerationMismatch {
                expected: 1,
                found: 2,
            })
        );
    }

    #[test]
    fn rejects_wrong_round() {
        let text = valid_json("approve").replace("\"round\":1", "\"round\":2");
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::RoundMismatch {
                expected: 1,
                found: 2,
            })
        );
    }

    #[test]
    fn rejects_truncated_json() {
        let text = r#"{"schema_version":1,"task_id":"task-1","result_generation":1,"round":1,"verdict":"appro"#;
        assert!(matches!(
            parse_structured_verdict(text, &ctx()),
            Err(VerdictParseError::NoJsonFound) | Err(VerdictParseError::InvalidJson(_))
        ));
    }

    #[test]
    fn a_markdown_example_alongside_project_content_does_not_get_silently_accepted() {
        // Simulates a reviewer echoing back an example from the review
        // package's own instructions ("your output should look like ```json
        // {...}```") plus separate real prose — two fenced json-shaped
        // blocks is ambiguous and must be rejected, not guessed.
        let text = format!(
            "The instructions say to format like:\n```json\n{}\n```\n\nMy actual answer:\n```json\n{}\n```",
            valid_json("approve"),
            valid_json("insufficient_evidence")
        );
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::MultipleJsonCandidates)
        );
    }

    #[test]
    fn project_file_content_containing_the_word_approve_is_never_mistaken_for_a_verdict() {
        let text = "I looked at config.json which contains: approve_by_default = true\nNo verdict object was produced due to an internal error.";
        assert_eq!(
            parse_structured_verdict(text, &ctx()),
            Err(VerdictParseError::NoJsonFound)
        );
    }

    #[test]
    fn unknown_verdict_value_is_rejected() {
        let text = valid_json("maybe");
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::UnknownVerdict("maybe".to_string()))
        );
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let text = valid_json("approve").replace("\"schema_version\":1", "\"schema_version\":99");
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::UnsupportedSchemaVersion(99))
        );
    }

    #[test]
    fn extra_text_before_and_after_an_unfenced_object_is_rejected() {
        let text = format!(
            "Sure, here you go: {} -- hope that helps!",
            valid_json("approve")
        );
        assert_eq!(
            parse_structured_verdict(&text, &ctx()),
            Err(VerdictParseError::TrailingContent)
        );
    }

    #[test]
    fn a_bare_unfenced_object_with_only_whitespace_around_it_is_accepted() {
        let text = format!("  \n{}\n  ", valid_json("approve"));
        assert!(parse_structured_verdict(&text, &ctx()).is_ok());
    }

    #[test]
    fn findings_round_trip() {
        let parsed = parse_structured_verdict(&valid_json("changes_required"), &ctx()).unwrap();
        assert_eq!(parsed.findings, vec!["f1".to_string(), "f2".to_string()]);
        assert_eq!(parsed.summary, "ok");
    }

    #[test]
    fn unknown_extra_field_is_tolerated_but_verdict_still_parses() {
        // The spec requires rejecting garbage that *replaces* the object,
        // not every conceivable additional field a newer schema might add;
        // `#[serde(default)]`-free required fields still gate correctness.
        let text = valid_json("approve").replacen('}', r#","reviewer_model":"gpt-5"}"#, 1);
        assert!(parse_structured_verdict(&text, &ctx()).is_ok());
    }
}
