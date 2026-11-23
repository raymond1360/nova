//! Provider metadata and protocol boundary; credentials are exclusively in the OS vault.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Sdkmax,
    Openai,
    Claude,
    Kimi,
    Deepseek,
    Gemini,
    OpenaiCompatible,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    OpenaiChat,
    OpenaiResponses,
    AnthropicMessages,
    DeepseekChat,
    GeminiContent,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub id: String,
    pub provider: Provider,
    pub base_url: String,
    pub model: String,
}
impl Config {
    pub fn protocol(&self) -> Protocol {
        match self.provider {
            Provider::Openai => Protocol::OpenaiResponses,
            Provider::Claude => Protocol::AnthropicMessages,
            Provider::Gemini => Protocol::GeminiContent,
            Provider::Deepseek => Protocol::DeepseekChat,
            _ => Protocol::OpenaiChat,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.id).is_err()
            || self.model.trim() != self.model
            || self.model.is_empty()
            || self.model.len() > 128
            || self.model.chars().any(char::is_control)
        {
            return Err("API_CONFIG_INVALID");
        }
        let u = url::Url::parse(&self.base_url).map_err(|_| "API_BASE_URL_INVALID")?;
        if u.scheme() != "https"
            || !u.username().is_empty()
            || u.password().is_some()
            || u.query().is_some()
            || u.fragment().is_some()
            || u.host_str().is_none()
            || u.path().contains('%')
            || self.base_url.ends_with('/')
        {
            return Err("API_BASE_URL_INVALID");
        }
        let expected = match self.provider {
            Provider::Sdkmax => Some(("api.sdkmax.com", "/v1")),
            Provider::Openai => Some(("api.openai.com", "/v1")),
            Provider::Claude => Some(("api.anthropic.com", "/v1")),
            Provider::Kimi => Some(("api.moonshot.ai", "/v1")),
            Provider::Deepseek => Some(("api.deepseek.com", "/v1")),
            Provider::Gemini => Some(("generativelanguage.googleapis.com", "/v1beta")),
            Provider::OpenaiCompatible => None,
        };
        if expected.is_some() && u.port().is_some() {
            return Err("API_OFFICIAL_URL_MISMATCH");
        }
        if expected.is_some_and(|(h, p)| u.host_str() != Some(h) || u.path() != p) {
            return Err("API_OFFICIAL_URL_MISMATCH");
        }
        if self.base_url.len() > 512
            || crate::security::redact_sensitive_text(&self.base_url) != self.base_url
        {
            return Err("API_CONFIG_INVALID");
        }
        Ok(())
    }
    pub fn require_adapter(&self) -> Result<()> {
        self.validate()?;
        if self.provider != Provider::Deepseek {
            return Err("API_ADAPTER_NOT_IMPLEMENTED");
        };
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigView {
    pub config: Config,
    pub revision: i64,
    pub has_key: bool,
    pub adapter_ready: bool,
}
#[derive(Clone)]
pub(crate) struct StoredConfig {
    pub view: ConfigView,
    pub slot: String,
}
fn sql<T>(v: rusqlite::Result<T>) -> Result<T> {
    v.map_err(|_| "API_DATABASE_ERROR")
}
pub fn list_configs(db: &Database) -> Result<Vec<ConfigView>> {
    let mut stmt = sql(db
        .conn()
        .prepare("SELECT config_json,revision,has_key FROM api_model_configs ORDER BY id"))?;
    let rows = sql(stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, bool>(2)?,
        ))
    }))?;
    rows.map(|r| {
        let (raw, revision, has_key) = sql(r)?;
        let config: Config = decode(raw.as_bytes(), 8192)?;
        let adapter_ready = config.provider == Provider::Deepseek;
        Ok(ConfigView {
            config,
            revision,
            has_key,
            adapter_ready,
        })
    })
    .collect()
}
pub(crate) fn load_config(db: &Database, id: &str) -> Result<StoredConfig> {
    let (raw, revision, has_key, slot): (String, i64, bool, String) = sql(db.conn().query_row(
        "SELECT config_json,revision,has_key,credential_slot FROM api_model_configs WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ))?;
    if slot
        .strip_prefix("nova-api-only/")
        .is_none_or(|s| uuid::Uuid::parse_str(s).is_err())
    {
        return Err("API_CREDENTIAL_SLOT_INVALID");
    }
    let config: Config = decode(raw.as_bytes(), 8192)?;
    config.validate()?;
    if config.id != id {
        return Err("API_CONFIG_ID_MISMATCH");
    };
    let adapter_ready = config.provider == Provider::Deepseek;
    Ok(StoredConfig {
        view: ConfigView {
            config,
            revision,
            has_key,
            adapter_ready,
        },
        slot,
    })
}
pub fn save_config(
    db: &Database,
    vault: &dyn CredentialStore,
    config: Config,
    key: Option<String>,
) -> Result<ConfigView> {
    config.validate()?;
    let old: Option<(i64, String, bool)> = sql(db
        .conn()
        .query_row(
            "SELECT revision,credential_slot,has_key FROM api_model_configs WHERE id=?1",
            [&config.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional())?;
    if key.is_none() {
        let current = load_config(db, &config.id)?;
        if current.view.config.provider != config.provider
            || current.view.config.base_url != config.base_url
        {
            return Err("API_NEW_ENDPOINT_REQUIRES_NEW_KEY");
        };
        acquire(&current, vault)?.scan_json(encode(&config)?.as_bytes(), 8192)?;
    }
    let rev = old.as_ref().map_or(1, |r| r.0 + 1);
    let slot = if key.is_some() {
        format!("nova-api-only/{}", uuid::Uuid::new_v4())
    } else {
        old.as_ref().ok_or("API_KEY_REQUIRED")?.1.clone()
    };
    if let Some(mut value) = key {
        let lease = SecretLease::real(value.as_bytes().to_vec());
        if lease.is_err() {
            unsafe { value.as_bytes_mut().fill(0) };
            return Err("API_KEY_INVALID");
        }
        let scan = lease.unwrap().scan_json(encode(&config)?.as_bytes(), 8192);
        if scan.is_err() {
            unsafe { value.as_bytes_mut().fill(0) };
            return Err("API_CONFIG_CONTAINS_KEY");
        }
        let saved = vault
            .save_api_key(&slot, &value)
            .map_err(|_| "API_CREDENTIAL_SAVE_FAILED");
        unsafe { value.as_bytes_mut().fill(0) };
        saved?;
    }
    let has_key = old.as_ref().map_or(true, |r| r.2) || old.as_ref().is_some_and(|r| r.1 != slot);
    let saved = if let Some(previous) = &old {
        db.conn().execute("UPDATE api_model_configs SET config_json=?1,revision=?2,has_key=?3,credential_slot=?4 WHERE id=?5 AND revision=?6",rusqlite::params![encode(&config)?,rev,has_key,slot,config.id,previous.0])
    } else {
        db.conn().execute("INSERT INTO api_model_configs(id,config_json,revision,has_key,credential_slot) VALUES(?1,?2,?3,?4,?5)",rusqlite::params![config.id,encode(&config)?,rev,has_key,slot])
    };
    if !matches!(saved, Ok(1)) {
        if old.as_ref().is_none_or(|r| r.1 != slot) {
            let _ = vault.delete_api_key(&slot);
        }
        return Err("API_CONFIG_CONFLICT");
    }
    if let Some(o) = old {
        if o.1 != slot {
            let _ = vault.delete_api_key(&o.1);
        }
    }
    Ok(load_config(db, &config.id)?.view)
}
pub fn delete_key(db: &Database, vault: &dyn CredentialStore, id: &str) -> Result<ConfigView> {
    let c = load_config(db, id)?;
    vault
        .delete_api_key(&c.slot)
        .map_err(|_| "API_CREDENTIAL_DELETE_FAILED")?;
    sql(db.conn().execute(
        "UPDATE api_model_configs SET has_key=0,revision=revision+1 WHERE id=?1",
        [id],
    ))?;
    Ok(load_config(db, id)?.view)
}
pub(crate) fn acquire(c: &StoredConfig, vault: &dyn CredentialStore) -> Result<SecretLease> {
    if !c.view.has_key {
        return Err("API_KEY_MISSING");
    }
    let mut key = vault
        .read_api_key(&c.slot)
        .map_err(|_| "API_CREDENTIAL_READ_FAILED")?
        .ok_or("API_KEY_MISSING")?;
    let result = SecretLease::real(key.as_bytes().to_vec());
    unsafe { key.as_bytes_mut().fill(0) };
    result
}
