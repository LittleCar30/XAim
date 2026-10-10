//! User defaults -> shared .xaim/config.json -> private project overrides.
use serde::{Deserialize, Serialize};

use crate::errors::{Result, XaimError};
use crate::store::{self, Store};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_timeout_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    pub defaults: RuntimeOverrides,
    pub shared: RuntimeOverrides,
    pub local: RuntimeOverrides,
    pub effective: RuntimeOverrides,
}

impl RuntimeOverrides {
    fn overlay(&mut self, next: &Self) {
        if next.task_timeout_sec.is_some() {
            self.task_timeout_sec = next.task_timeout_sec;
        }
        if next.max_turns.is_some() {
            self.max_turns = next.max_turns;
        }
    }
    fn validate(&self) -> Result<()> {
        if self
            .task_timeout_sec
            .is_some_and(|n| !(30..=86400).contains(&n))
            || self.max_turns.is_some_and(|n| !(1..=256).contains(&n))
        {
            return Err(XaimError::Config(
                "任务时限应为 30–86400 秒，工具循环轮数应为 1–256".into(),
            ));
        }
        Ok(())
    }
}

pub fn load(store: &Store, project_id: Option<i64>) -> Result<RuntimeSettings> {
    let read = |key: &str| -> Result<RuntimeOverrides> {
        let value = store::cache::get_cache(&store.conn(), key)?;
        let settings: RuntimeOverrides = value
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or_default();
        settings.validate()?;
        Ok(settings)
    };
    let defaults = read("runtime:defaults")?;
    let mut shared = RuntimeOverrides::default();
    let mut local = RuntimeOverrides::default();
    if let Some(id) = project_id {
        let project = store::project::get_project(&store.conn(), id)?
            .ok_or_else(|| XaimError::Config("项目不存在".into()))?;
        let path = crate::agent::workspace::safe_path(
            std::path::Path::new(&project.path),
            ".xaim/config.json",
        )?;
        match std::fs::metadata(&path) {
            Ok(metadata) => {
                if metadata.len() > 64 * 1024 {
                    return Err(XaimError::Config("项目配置超过 64 KB".into()));
                }
                shared = serde_json::from_slice(&std::fs::read(path)?)?;
                shared.validate()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        local = read(&format!("project:{id}:runtime"))?;
    }
    let mut effective = RuntimeOverrides {
        task_timeout_sec: Some(1800),
        max_turns: Some(32),
    };
    effective.overlay(&defaults);
    effective.overlay(&shared);
    effective.overlay(&local);
    Ok(RuntimeSettings {
        defaults,
        shared,
        local,
        effective,
    })
}

pub fn save(store: &Store, project_id: Option<i64>, settings: RuntimeOverrides) -> Result<()> {
    settings.validate()?;
    let key = if let Some(id) = project_id {
        store::project::get_project(&store.conn(), id)?
            .ok_or_else(|| XaimError::Config("项目不存在".into()))?;
        format!("project:{id}:runtime")
    } else {
        "runtime:defaults".into()
    };
    store::cache::set_cache(
        &store.conn(),
        &key,
        &serde_json::to_string(&settings)?,
        None,
    )
}

#[tauri::command]
pub async fn get_runtime_settings(
    project_id: Option<i64>,
    state: tauri::State<'_, crate::state::AppState>,
) -> std::result::Result<RuntimeSettings, String> {
    load(&state.store, project_id).map_err(String::from)
}

#[tauri::command]
pub async fn set_runtime_settings(
    project_id: Option<i64>,
    settings: RuntimeOverrides,
    state: tauri::State<'_, crate::state::AppState>,
) -> std::result::Result<(), String> {
    save(&state.store, project_id, settings).map_err(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_local_overrides_shared_and_user_defaults_and_can_inherit_again() {
        let store = Store::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let id =
            store::project::insert_project(&store.conn(), &root.path().to_string_lossy(), "repo")
                .unwrap();
        save(
            &store,
            None,
            RuntimeOverrides {
                task_timeout_sec: Some(300),
                max_turns: Some(8),
            },
        )
        .unwrap();
        std::fs::create_dir(root.path().join(".xaim")).unwrap();
        std::fs::write(root.path().join(".xaim/config.json"), r#"{"maxTurns":12}"#).unwrap();
        let effective = load(&store, Some(id)).unwrap().effective;
        assert_eq!(
            effective,
            RuntimeOverrides {
                task_timeout_sec: Some(300),
                max_turns: Some(12)
            }
        );
        save(
            &store,
            Some(id),
            RuntimeOverrides {
                task_timeout_sec: Some(600),
                max_turns: None,
            },
        )
        .unwrap();
        assert_eq!(
            load(&store, Some(id)).unwrap().effective,
            RuntimeOverrides {
                task_timeout_sec: Some(600),
                max_turns: Some(12)
            }
        );
        save(&store, Some(id), RuntimeOverrides::default()).unwrap();
        assert_eq!(load(&store, Some(id)).unwrap().effective, effective);
        assert_eq!(load(&store, None).unwrap().effective.max_turns, Some(8));
    }

    #[test]
    fn refuses_invalid_limits_unknown_project_and_unknown_shared_keys() {
        let store = Store::open_in_memory().unwrap();
        assert!(save(
            &store,
            None,
            RuntimeOverrides {
                task_timeout_sec: Some(1),
                max_turns: None
            }
        )
        .is_err());
        assert!(save(
            &store,
            None,
            RuntimeOverrides {
                task_timeout_sec: None,
                max_turns: Some(0)
            }
        )
        .is_err());
        assert!(save(&store, Some(999), RuntimeOverrides::default()).is_err());
        let root = tempfile::tempdir().unwrap();
        let id =
            store::project::insert_project(&store.conn(), &root.path().to_string_lossy(), "repo")
                .unwrap();
        std::fs::create_dir(root.path().join(".xaim")).unwrap();
        std::fs::write(root.path().join(".xaim/config.json"), r#"{"maxTurn":12}"#).unwrap();
        assert!(
            load(&store, Some(id)).is_err(),
            "typos must not silently change execution defaults"
        );
    }
}
