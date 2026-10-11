//! Private project permissions. Project files cannot create grants.
use crate::errors::{Result, XaimError};
use crate::store::{self, Store};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecutionMode {
    Disabled,
    #[default]
    Ask,
    Project,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitNetwork {
    #[serde(default)]
    pub mode: ProxyMode,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyMode {
    #[default]
    System,
    Direct,
    Custom,
}

impl GitNetwork {
    pub fn validate(&self) -> Result<()> {
        if self.mode == ProxyMode::Custom {
            let url = reqwest::Url::parse(&self.url)
                .map_err(|_| XaimError::Config("代理地址无效".into()))?;
            if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/" && !url.path().is_empty()
            {
                return Err(XaimError::Config(
                    "代理应为不含账号密码的 HTTP、HTTPS 或 SOCKS5 地址".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionScope {
    pub id: String,
    pub tool: String,
    pub command: String,
    pub location: String,
    pub network: String,
    pub impact: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExecutionSettings {
    pub mode: ExecutionMode,
    pub grants: Vec<ExecutionScope>,
    pub git_network: GitNetwork,
}

fn key(id: i64) -> String {
    format!("project:{id}:execution")
}

pub fn load(store: &Store, id: i64) -> Result<ExecutionSettings> {
    let conn = store.conn();
    store::project::get_project(&conn, id)?
        .ok_or_else(|| XaimError::Config("项目不存在".into()))?;
    let settings: ExecutionSettings = store::cache::get_cache(&conn, &key(id))?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    settings.git_network.validate()?;
    Ok(settings)
}

// All read-modify-write operations hold the connection lock through the write.
pub fn configure(
    store: &Store,
    id: i64,
    mode: ExecutionMode,
    network: GitNetwork,
) -> Result<ExecutionSettings> {
    network.validate()?;
    let mut settings = load(store, id)?;
    let conn = store.conn();
    if let Some(s) = store::cache::get_cache(&conn, &key(id))? {
        settings = serde_json::from_str(&s)?;
    }
    settings.mode = mode;
    settings.git_network = network;
    store::cache::set_cache(&conn, &key(id), &serde_json::to_string(&settings)?, None)?;
    Ok(settings)
}

pub fn remember(store: &Store, id: i64, scope: ExecutionScope) -> Result<()> {
    let conn = store.conn();
    let mut settings: ExecutionSettings = store::cache::get_cache(&conn, &key(id))?
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    if !settings.grants.iter().any(|g| g.id == scope.id) {
        if settings.grants.len() >= 256 {
            return Err(XaimError::Config(
                "项目授权已达上限，请撤销不再使用的授权".into(),
            ));
        }
        settings.grants.push(scope);
    }
    settings.mode = ExecutionMode::Project;
    store::cache::set_cache(&conn, &key(id), &serde_json::to_string(&settings)?, None)
}

pub fn revoke(store: &Store, id: i64, scope_id: &str) -> Result<ExecutionSettings> {
    let mut settings = load(store, id)?;
    let conn = store.conn();
    if let Some(s) = store::cache::get_cache(&conn, &key(id))? {
        settings = serde_json::from_str(&s)?;
    }
    settings.grants.retain(|g| g.id != scope_id);
    store::cache::set_cache(&conn, &key(id), &serde_json::to_string(&settings)?, None)?;
    Ok(settings)
}
