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

pub fn redact_sensitive_text(input: &str) -> String {
    let mut text = input.to_string();
    for marker in [
        "Authorization: Bearer ",
        "authorization: bearer ",
        "Cookie: ",
        "cookie: ",
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
    redact_sk_like(&text)
}

fn redact_sk_like(input: &str) -> String {
    input
        .split_whitespace()
        .map(|part| {
            if part.starts_with("sk-") && part.len() > 10 {
                "[REDACTED_KEY]".to_string()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
}
