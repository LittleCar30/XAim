//! Only credential references go into SQLite and backups.
use crate::errors::{Result, XaimError};
use crate::types::Config;

const PREFIX: &str = "xaim-vault-v1:";

pub(crate) fn is_reference(key: &str) -> bool {
    key.trim().starts_with(PREFIX)
}

pub fn seal(config: &Config) -> Result<Config> {
    let mut stored = config.clone();
    for model in std::iter::once(&mut stored.model)
        .chain(stored.model_profiles.iter_mut().map(|p| &mut p.config))
    {
        if let Some(secret) = &model.api_key {
            if secret.is_empty() {
                model.api_key = None;
                continue;
            }
            if secret.starts_with(PREFIX) {
                return Err(XaimError::Config("请填写 API Key，不能填写凭据引用".into()));
            }
            // Content addressing keeps old references valid if a later database write fails.
            let id = blake3::hash(secret.as_bytes()).to_hex().to_string();
            put(&id, secret)?;
            model.api_key = Some(format!("{PREFIX}{id}"));
        }
    }
    Ok(stored)
}

pub fn unseal(config: Config) -> Config {
    unseal_with(config, get)
}

fn unseal_with(mut config: Config, resolve: impl Fn(&str) -> Result<Option<String>>) -> Config {
    for model in std::iter::once(&mut config.model)
        .chain(config.model_profiles.iter_mut().map(|p| &mut p.config))
    {
        if let Some(id) = model
            .api_key
            .as_deref()
            .and_then(|key| key.strip_prefix(PREFIX))
        {
            match resolve(id) {
                Ok(secret) => model.api_key = secret,
                Err(error) => {
                    tracing::warn!(%error, "无法读取系统凭据，请重新填写 API Key");
                    model.api_key = None;
                }
            }
        }
    }
    config
}

pub fn needs_migration(config: &Config) -> bool {
    std::iter::once(&config.model)
        .chain(config.model_profiles.iter().map(|p| &p.config))
        .any(|model| {
            model
                .api_key
                .as_deref()
                .is_some_and(|key| !key.starts_with(PREFIX))
        })
}

#[cfg(not(test))]
fn put(id: &str, secret: &str) -> Result<()> {
    keyring::Entry::new(crate::data::APP_ID, id)
        .and_then(|entry| entry.set_password(secret))
        .map_err(|error| XaimError::Config(format!("无法保存系统凭据：{error}")))
}

fn get_system(id: &str) -> Result<Option<String>> {
    match keyring::Entry::new(crate::data::APP_ID, id).and_then(|entry| entry.get_password()) {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(XaimError::Config(format!("无法读取系统凭据：{error}"))),
    }
}

#[cfg(not(test))]
fn get(id: &str) -> Result<Option<String>> {
    get_system(id)
}

/// Only explicit ignored live tests may access the user's real credential vault.
#[cfg(test)]
pub(crate) fn unseal_live(config: Config) -> Config {
    unseal_with(config, get_system)
}

#[cfg(test)]
fn vault() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, String>> {
    static VAULT: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    > = std::sync::OnceLock::new();
    VAULT.get_or_init(Default::default)
}
#[cfg(test)]
fn put(id: &str, secret: &str) -> Result<()> {
    vault().lock().unwrap().insert(id.into(), secret.into());
    Ok(())
}
#[cfg(test)]
fn get(id: &str) -> Result<Option<String>> {
    Ok(vault().lock().unwrap().get(id).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{cache, Store};

    #[test]
    #[cfg(windows)]
    fn windows_credential_store_roundtrip() {
        let username = format!("test-{}", uuid::Uuid::new_v4());
        let entry = keyring::Entry::new("com.xaim.app.tests", &username).unwrap();
        struct Cleanup(keyring::Entry);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.delete_credential();
            }
        }
        let entry = Cleanup(entry);
        entry.0.set_password("native-vault-test-only").unwrap();
        assert_eq!(entry.0.get_password().unwrap(), "native-vault-test-only");
    }

    #[test]
    fn stores_only_references_and_restores_credentials_without_changing_config() {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.model.api_key = Some("credential-test-secret".into());
        crate::config::model::save(&store, &config).unwrap();
        let stored = cache::get_cache(&store.conn(), crate::config::model::CONFIG_KEY)
            .unwrap()
            .unwrap();
        assert!(!stored.contains("credential-test-secret"));
        assert!(stored.contains(PREFIX));
        assert_eq!(crate::config::model::load(&store), config);
    }

    #[test]
    fn migrates_legacy_plaintext_and_keeps_configuration_when_credentials_are_missing() {
        let store = Store::open_in_memory().unwrap();
        let mut config = Config::default();
        config.model.api_key = Some("legacy-credential-test".into());
        cache::set_cache(
            &store.conn(),
            crate::config::model::CONFIG_KEY,
            &serde_json::to_string(&config).unwrap(),
            None,
        )
        .unwrap();
        crate::config::model::migrate_credentials(&store).unwrap();
        let raw = cache::get_cache(&store.conn(), crate::config::model::CONFIG_KEY)
            .unwrap()
            .unwrap();
        assert!(!raw.contains("legacy-credential-test"));
        assert_eq!(crate::config::model::load(&store), config);
        config.model.api_key = Some(format!("{PREFIX}absent-in-test-vault"));
        let restored = unseal(config.clone());
        assert_eq!(restored.model.name, config.model.name);
        assert_eq!(restored.model.api_key, None);
    }
}
