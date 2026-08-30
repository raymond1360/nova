use url::Url;

use crate::error::{NovaError, NovaResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowedEndpoint {
    Models,
    ChatCompletions,
    TokenUsage,
}

impl AllowedEndpoint {
    pub fn path(self) -> &'static str {
        match self {
            Self::Models => "/v1/models",
            Self::ChatCompletions => "/v1/chat/completions",
            Self::TokenUsage => "/api/usage/token/",
        }
    }
}

pub fn normalize_base_url(raw: &str) -> NovaResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NovaError::Config("Base URL 不能为空".to_string()));
    }
    let parsed =
        Url::parse(trimmed).map_err(|_| NovaError::Config("Base URL 不是有效 URL".to_string()))?;
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(NovaError::Config(
            "Base URL 不允许包含用户名或密码".to_string(),
        ));
    }
    if parsed.scheme() != "https" {
        let localhost_dev = cfg!(debug_assertions)
            && parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if !localhost_dev {
            return Err(NovaError::Config(
                "仅允许 HTTPS；开发模式只允许 localhost HTTP".to_string(),
            ));
        }
    }
    let host = parsed.host_str().unwrap_or_default();
    let allowed_prod = parsed.scheme() == "https" && host.eq_ignore_ascii_case("api.sdkmax.com");
    let allowed_dev = cfg!(debug_assertions) && matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !allowed_prod && !allowed_dev {
        return Err(NovaError::Config(
            "仅允许 api.sdkmax.com 或开发模式 localhost".to_string(),
        ));
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err(NovaError::Config("Base URL 不应包含接口路径".to_string()));
    }
    let mut normalized = parsed;
    normalized.set_path("");
    normalized.set_query(None);
    normalized.set_fragment(None);
    Ok(normalized.as_str().trim_end_matches('/').to_string())
}

pub fn endpoint_url(base_url: &str, endpoint: AllowedEndpoint) -> NovaResult<String> {
    let normalized = normalize_base_url(base_url)?;
    Ok(format!("{}{}", normalized, endpoint.path()))
}

pub fn assert_allowed_path(path: &str) -> NovaResult<()> {
    let allowed = [
        AllowedEndpoint::Models.path(),
        AllowedEndpoint::ChatCompletions.path(),
        AllowedEndpoint::TokenUsage.path(),
    ];
    if allowed.contains(&path) {
        Ok(())
    } else {
        Err(NovaError::Config("该接口未开放给 Nova 桌面端".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_sdkmax_url() {
        assert_eq!(
            normalize_base_url("https://api.sdkmax.com/").unwrap(),
            "https://api.sdkmax.com"
        );
    }

    #[test]
    fn rejects_credentials_and_paths() {
        assert!(normalize_base_url("https://u:p@api.sdkmax.com").is_err());
        assert!(normalize_base_url("https://api.sdkmax.com/api/channel/").is_err());
    }

    #[test]
    fn allows_only_explicit_paths() {
        assert!(assert_allowed_path("/v1/models").is_ok());
        assert!(assert_allowed_path("/api/channel/").is_err());
        assert!(assert_allowed_path("/api/user/manage").is_err());
    }
}
