pub fn mask_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 8 {
        return "********".to_string();
    }
    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

pub fn mask_request_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() <= 12 {
        return trimmed.to_string();
    }
    format!("{}...{}", &trimmed[..6], &trimmed[trimmed.len() - 4..])
}

/// Redact sensitive material from text that may be logged, shown in an
/// error, or displayed in a collapsed diagnostic panel.
///
/// This is the single redaction entry point shared by the SDKMAX chat error
/// path and the Phase 1 process-execution boundary (`runtime::process`,
/// including the `program`/`args` it echoes back) — every place Nova
/// surfaces external or request text goes through the same rules. Applied
/// in three passes, each preserving whitespace/newlines exactly (CLI
/// stdout/stderr is multi-line and must stay readable):
///
/// 1. Known header lines — `Authorization:`, `Proxy-Authorization:`,
///    `Cookie:`, `Set-Cookie:` (value redacted to end of line, matching how
///    HTTP headers actually work — a `Cookie:` line can carry several
///    `name=value` pairs separated by `;`, all of which must be hidden, not
///    just the first) — plus bare `Bearer `/`Basic ` scheme tokens with no
///    `Authorization:` prefix (redacted to the end of that one token only,
///    so unrelated trailing text on the same line survives). All matching
///    is case-insensitive.
/// 2. Generic `key=value` / `key: value` / JSON `"key": "value"` pairs
///    whose key name contains a sensitive substring (password, secret,
///    token, key, credential) — covers API keys, `access_token`/
///    `refresh_token`, environment-variable-style secrets, and
///    `?token=`/`&key=`-style URL query parameters, without swallowing the
///    next query parameter or JSON field.
/// 3. Bare prefixed tokens with no surrounding `key=` shape at all —
///    OpenAI/Anthropic-style `sk-...` and GitHub `ghp_`/`gho_`/`ghu_`/
///    `ghs_`/`ghr_`/`github_pat_` tokens.
pub fn redact_sensitive_text(input: &str) -> String {
    let text = redact_header_lines(input);
    let text = redact_key_value_secrets(&text);
    redact_prefixed_tokens(&text)
}

/// Markers whose value is redacted to the end of the current line — real
/// HTTP header values for these can legitimately contain whitespace,
/// commas, or semicolons that are still part of the sensitive value
/// (multiple `Cookie:` pairs, Basic-auth base64 padding, custom schemes).
const LINE_MARKERS: [&str; 4] = [
    "authorization:",
    "proxy-authorization:",
    "cookie:",
    "set-cookie:",
];

/// Markers whose value is redacted only up to the next whitespace/
/// delimiter — a bare scheme token has no internal whitespace, and
/// stopping there avoids swallowing unrelated trailing prose on the same
/// log line.
const TOKEN_MARKERS: [&str; 2] = ["bearer ", "basic "];

fn redact_header_lines(input: &str) -> String {
    let mut text = input.to_string();
    for marker in LINE_MARKERS {
        text = redact_after_marker(&text, marker, true);
    }
    for marker in TOKEN_MARKERS {
        text = redact_after_marker(&text, marker, false);
    }
    text
}

/// Case-insensitively find every occurrence of `marker` in `input` and
/// replace the text immediately following it (up to end-of-line if
/// `till_eol`, otherwise up to the next whitespace/delimiter) with
/// `[REDACTED]`. The marker text itself, and everything not matched, keeps
/// its original casing/formatting.
fn redact_after_marker(input: &str, marker: &str, till_eol: bool) -> String {
    let lower = input.to_ascii_lowercase();
    let mut result = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find(marker) {
        let marker_start = cursor + offset;
        let marker_end = marker_start + marker.len();
        result.push_str(&input[cursor..marker_end]);

        let value_start = marker_end;
        let value_end = if till_eol {
            input[value_start..]
                .find('\n')
                .map(|i| value_start + i)
                .unwrap_or(input.len())
        } else {
            input[value_start..]
                .find(|c: char| is_value_terminator(c))
                .map(|i| value_start + i)
                .unwrap_or(input.len())
        };
        if value_end > value_start {
            result.push_str("[REDACTED]");
        }
        cursor = value_end;
    }
    result.push_str(&input[cursor..]);
    result
}

const SENSITIVE_KEY_SUBSTRINGS: [&str; 6] =
    ["password", "passwd", "secret", "token", "key", "credential"];

fn key_is_sensitive(key: &str) -> bool {
    SENSITIVE_KEY_SUBSTRINGS
        .iter()
        .any(|marker| key.contains(marker))
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn is_value_terminator(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | ';' | '&' | '"' | '\'' | ')' | ']' | '}')
}

/// Redact `key=value`, `key: value`, and JSON `"key": "value"` pairs (env
/// vars, CLI flags, URL query parameters, JSON request/response bodies)
/// whose key name looks sensitive. Operates on a `Vec<char>` so multi-byte
/// unicode text and an optional quote around the key/value are handled
/// correctly, and unrelated text is copied through untouched.
fn redact_key_value_secrets(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut result = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if !is_key_char(chars[i]) {
            result.push(chars[i]);
            i += 1;
            continue;
        }
        let key_start = i;
        let mut j = i;
        while j < chars.len() && is_key_char(chars[j]) {
            j += 1;
        }
        let key: String = chars[key_start..j]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();

        // Allow an optional closing quote right after the key, for JSON
        // `"key"`, then skip whitespace before the separator.
        let mut sep_index = j;
        if sep_index < chars.len() && matches!(chars[sep_index], '"' | '\'') {
            sep_index += 1;
        }
        while sep_index < chars.len() && chars[sep_index].is_whitespace() {
            sep_index += 1;
        }
        let separator = chars.get(sep_index).copied();

        if !key_is_sensitive(&key) || !matches!(separator, Some('=') | Some(':')) {
            result.extend(chars[key_start..j].iter());
            i = j;
            continue;
        }

        // Emit the key, any quote/whitespace, and the separator unchanged.
        result.extend(chars[key_start..=sep_index].iter());
        let mut value_start = sep_index + 1;
        while value_start < chars.len() && chars[value_start].is_whitespace() {
            result.push(chars[value_start]);
            value_start += 1;
        }

        if value_start < chars.len() && matches!(chars[value_start], '"' | '\'') {
            let quote = chars[value_start];
            result.push(quote);
            let mut value_end = value_start + 1;
            while value_end < chars.len() && chars[value_end] != quote {
                value_end += 1;
            }
            if value_end > value_start + 1 {
                result.push_str("[REDACTED]");
            }
            if value_end < chars.len() {
                result.push(quote);
                i = value_end + 1;
            } else {
                i = value_end;
            }
        } else {
            let mut value_end = value_start;
            while value_end < chars.len() && !is_value_terminator(chars[value_end]) {
                value_end += 1;
            }
            if value_end > value_start {
                result.push_str("[REDACTED]");
            }
            i = value_end;
        }
    }
    result
}

const SENSITIVE_TOKEN_PREFIXES: [&str; 7] =
    ["sk-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Redact bare tokens (no `key=`/`key:` shape) that start with a known
/// secret-key prefix, e.g. OpenAI/Anthropic `sk-...` or GitHub `ghp_...`.
/// Scans character-by-character so surrounding whitespace/newlines are
/// preserved exactly.
fn redact_prefixed_tokens(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut result = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if !is_token_char(chars[i]) {
            result.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < chars.len() && is_token_char(chars[j]) {
            j += 1;
        }
        let token: String = chars[start..j].iter().collect();
        if token.len() > 10
            && SENSITIVE_TOKEN_PREFIXES
                .iter()
                .any(|p| token.starts_with(p))
        {
            result.push_str("[REDACTED_KEY]");
        } else {
            result.push_str(&token);
        }
        i = j;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_api_key_without_revealing_middle() {
        let fake_key = ["sk", "abcdefghijklmnopqrstuvwxyz"].join("-");
        assert_eq!(mask_secret(&fake_key), "sk-a...wxyz");
        assert_eq!(mask_secret("short"), "********");
    }

    #[test]
    fn redacts_auth_and_key_like_values() {
        let first_key = ["sk", "secret-secret"].join("-");
        let second_key = ["sk", "abcdefghi"].join("-");
        let cleaned = redact_sensitive_text(&format!(
            "Authorization: Bearer {first_key} Cookie: sid=123 {second_key}"
        ));
        assert!(!cleaned.contains(&first_key));
        assert!(!cleaned.contains("sid=123"));
        assert!(cleaned.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_bare_bearer_token_without_authorization_prefix() {
        let cleaned =
            redact_sensitive_text("sending request with Bearer eyJhbGciOiJIUzI1NiJ9.payload");
        assert!(!cleaned.contains("eyJhbGciOiJIUzI1NiJ9"));
        assert!(cleaned.contains("Bearer"));
    }

    #[test]
    fn redacts_cookie_header_value() {
        let cleaned = redact_sensitive_text("Set-Cookie: session=abc123; Path=/");
        assert!(!cleaned.contains("abc123"));
    }

    #[test]
    fn redacts_env_style_api_key_and_password() {
        let cleaned =
            redact_sensitive_text("NOVA_API_KEY=fake-1234567890abcdef PASSWORD=hunter2secret");
        assert!(!cleaned.contains("fake-1234567890abcdef"));
        assert!(!cleaned.contains("hunter2secret"));
        assert!(cleaned.contains("NOVA_API_KEY=[REDACTED]"));
        assert!(cleaned.contains("PASSWORD=[REDACTED]"));
    }

    #[test]
    fn redacts_access_and_refresh_tokens() {
        let cleaned =
            redact_sensitive_text("access_token: abc.def.ghi refresh_token=zzz999yyy888xxx777");
        assert!(!cleaned.contains("abc.def.ghi"));
        assert!(!cleaned.contains("zzz999yyy888xxx777"));
    }

    #[test]
    fn redacts_url_query_token_and_key_without_eating_next_param() {
        let cleaned = redact_sensitive_text(
            "https://example.com/cb?access_token=abcDEF123&state=keep-me&api_key=zzzZZZ999",
        );
        assert!(!cleaned.contains("abcDEF123"));
        assert!(!cleaned.contains("zzzZZZ999"));
        assert!(cleaned.contains("state=keep-me"));
    }

    #[test]
    fn redacts_github_token_prefixes() {
        let ghp = ["ghp", "1234567890abcdefghijklmnopqrstuvwxyz"].join("_");
        let cleaned = redact_sensitive_text(&format!("GITHUB_TOKEN={ghp}"));
        assert!(!cleaned.contains(&ghp));
    }

    #[test]
    fn preserves_newlines_and_unrelated_multiline_output() {
        let input = "line one is fine\nsecond line also fine\nthird line: api_key=shouldnotshow123";
        let cleaned = redact_sensitive_text(input);
        assert_eq!(cleaned.lines().count(), 3);
        assert!(cleaned.starts_with("line one is fine\nsecond line also fine\n"));
        assert!(!cleaned.contains("shouldnotshow123"));
    }

    #[test]
    fn does_not_redact_unrelated_key_value_pairs() {
        let cleaned = redact_sensitive_text("status=ready count=3 name=nova");
        assert_eq!(cleaned, "status=ready count=3 name=nova");
    }

    // --- Major 4 remediation coverage -------------------------------------

    #[test]
    fn redacts_authorization_basic_scheme_fully() {
        let fake = "dXNlcjpwYXNzd29yZA==";
        let cleaned = redact_sensitive_text(&format!("Authorization: Basic {fake}"));
        assert!(!cleaned.contains(fake));
    }

    #[test]
    fn redacts_any_authorization_scheme_case_insensitively() {
        let fake = "custom-scheme-token-value-123";
        let cleaned = redact_sensitive_text(&format!("AUTHORIZATION: CustomScheme {fake}"));
        assert!(!cleaned.contains(fake));
    }

    #[test]
    fn redacts_proxy_authorization_header() {
        let fake = "proxytoken1234567890";
        let cleaned = redact_sensitive_text(&format!("Proxy-Authorization: Bearer {fake}"));
        assert!(!cleaned.contains(fake));
    }

    #[test]
    fn redacts_full_cookie_line_not_just_first_segment_before_semicolon() {
        let cleaned =
            redact_sensitive_text("Cookie: sid=aaa111; auth=bbb222; theme=dark; region=us");
        assert!(!cleaned.contains("aaa111"));
        assert!(!cleaned.contains("bbb222"));
        // Even the non-secret-sounding trailing pairs on the same Cookie
        // header line must be hidden too — the whole header value is
        // sensitive by convention, not just the parts with scary names.
        assert!(!cleaned.contains("dark"));
        assert!(!cleaned.contains("us"));
    }

    #[test]
    fn cookie_redaction_does_not_bleed_into_the_next_line() {
        let cleaned = redact_sensitive_text("Cookie: sid=aaa111\nnext line is fine: value=42");
        assert!(!cleaned.contains("aaa111"));
        assert!(cleaned.contains("next line is fine: value=42"));
    }

    #[test]
    fn redacts_json_quoted_authorization_and_token_fields_mixed_case() {
        let json = r#"{"Authorization":"Bearer sekrit-value-here","Access_Token":"another-secret-value","name":"nova"}"#;
        let cleaned = redact_sensitive_text(json);
        assert!(!cleaned.contains("sekrit-value-here"));
        assert!(!cleaned.contains("another-secret-value"));
        assert!(cleaned.contains(r#""name":"nova""#));
    }

    #[test]
    fn redacts_single_quoted_key_value_pairs() {
        let cleaned =
            redact_sensitive_text("config: {'api_key': 'abc123secretXYZ', 'debug': true}");
        assert!(!cleaned.contains("abc123secretXYZ"));
        assert!(cleaned.contains("'debug': true"));
    }

    #[test]
    fn redacts_multiple_query_parameters_with_mixed_case_keys() {
        let cleaned = redact_sensitive_text("url?Token=AAA111bbb&Api-Key=CCC222ddd&ok=1");
        // Hyphenated key names ("Api-Key") must also be recognized.
        assert!(!cleaned.contains("AAA111bbb"));
        assert!(!cleaned.contains("CCC222ddd"));
        assert!(cleaned.contains("ok=1"));
    }

    #[test]
    fn preserves_multiline_text_around_a_json_secret_field() {
        let input = "request failed\n{\"password\":\"hunter2\"}\nretrying in 5s";
        let cleaned = redact_sensitive_text(input);
        assert!(!cleaned.contains("hunter2"));
        assert!(cleaned.starts_with("request failed\n"));
        assert!(cleaned.trim_end().ends_with("retrying in 5s"));
    }

    #[test]
    fn program_args_style_fake_token_is_redacted() {
        // Simulates what `runtime::process::ExecutionResult` now redacts
        // program/args through: a would-be argument carrying a fabricated
        // token must not survive.
        let fake = ["sk", "programargstoken12345"].join("-");
        let args = ["--token".to_string(), fake.clone()];
        let cleaned: Vec<String> = args.iter().map(|a| redact_sensitive_text(a)).collect();
        assert!(!cleaned.iter().any(|a| a.contains(&fake)));
    }

    #[test]
    fn truncation_boundary_mid_token_still_hides_the_visible_fragment() {
        // If output was truncated mid-token (e.g. by the process executor's
        // output cap) before redaction ever runs, the *visible* fragment
        // must still not obviously read as a live credential prefix once
        // whatever of it survives is processed — here that means the
        // still-intact prefix triggers the same bare-token redaction as a
        // complete key.
        let fake_prefix = ["sk", "truncatedfragment123"].join("-");
        let truncated = &fake_prefix[..fake_prefix.len() - 3];
        let cleaned = redact_sensitive_text(truncated);
        assert!(!cleaned.contains(truncated));
    }
}
