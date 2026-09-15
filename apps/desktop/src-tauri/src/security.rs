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
/// path and the Phase 1 process-execution boundary (`runtime::process`), so
/// every place Nova surfaces external text goes through the same rules.
/// Applied in three passes, each preserving whitespace/newlines exactly
/// (CLI stdout/stderr is multi-line and must stay readable):
///
/// 1. Known header/scheme markers (`Authorization: Bearer`, `Cookie`, bare
///    `Bearer`) — the marker text itself is kept, only the value after it
///    is replaced.
/// 2. Generic `key=value` / `key: value` pairs whose key name contains a
///    sensitive substring (password, secret, token, key, credential) —
///    covers API keys, access/refresh tokens, env-var secrets, and
///    `?token=`/`&key=`-style URL query parameters.
/// 3. Bare prefixed tokens with no surrounding `key=` shape at all —
///    OpenAI/Anthropic-style `sk-...` and GitHub `ghp_`/`gho_`/`ghu_`/
///    `ghs_`/`ghr_`/`github_pat_` tokens.
pub fn redact_sensitive_text(input: &str) -> String {
    let text = redact_header_markers(input);
    let text = redact_key_value_secrets(&text);
    redact_prefixed_tokens(&text)
}

fn redact_header_markers(input: &str) -> String {
    let mut text = input.to_string();
    for marker in [
        "Authorization: Bearer ",
        "authorization: bearer ",
        "Cookie: ",
        "cookie: ",
        "Bearer ",
        "bearer ",
    ] {
        let mut search_from = 0;
        while let Some(offset) = text[search_from..].find(marker) {
            let start = search_from + offset;
            let value_start = start + marker.len();
            let value_end = text[value_start..]
                .find(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .map(|offset| value_start + offset)
                .unwrap_or(text.len());
            text.replace_range(value_start..value_end, "[REDACTED]");
            search_from = value_start + "[REDACTED]".len();
        }
    }
    text
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

/// Redact `key=value` and `key: value` pairs (env vars, CLI flags, URL query
/// parameters) whose key name looks sensitive, operating character-by-
/// character so unrelated whitespace/newlines/unicode text is untouched.
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
        let separator = chars.get(j).copied();
        if key_is_sensitive(&key) && matches!(separator, Some('=') | Some(':')) {
            result.extend(chars[key_start..j].iter());
            result.push(separator.unwrap());
            let mut value_start = j + 1;
            if separator == Some(':') && chars.get(value_start) == Some(&' ') {
                result.push(' ');
                value_start += 1;
            }
            let mut value_end = value_start;
            while value_end < chars.len() && !is_value_terminator(chars[value_end]) {
                value_end += 1;
            }
            if value_end > value_start {
                result.push_str("[REDACTED]");
            }
            i = value_end;
        } else {
            result.extend(chars[key_start..j].iter());
            i = j;
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
}
