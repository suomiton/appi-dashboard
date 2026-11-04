//! The per-user config file: remembered instances, refresh, range, alerts, labels.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::azure::discovery::AppInstance;
use crate::data::alerts::AlertConfig;
use crate::data::time::{DEFAULT_REFRESH, TimeRange};

const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedInstance {
    pub name: String,
    pub resource_id: String,
    pub app_id: String,
    pub tenant_id: String,
    #[serde(default)]
    pub subscription_name: String,
    #[serde(default)]
    pub resource_group: String,
}

impl From<&AppInstance> for SavedInstance {
    fn from(i: &AppInstance) -> Self {
        Self {
            name: i.name.clone(),
            resource_id: i.resource_id.clone(),
            app_id: i.app_id.clone(),
            tenant_id: i.tenant_id.clone(),
            subscription_name: i.subscription_name.clone(),
            resource_group: i.resource_group.clone(),
        }
    }
}

impl SavedInstance {
    pub fn to_instance(&self) -> AppInstance {
        let subscription_id = self
            .resource_id
            .split('/')
            .skip_while(|p| !p.eq_ignore_ascii_case("subscriptions"))
            .nth(1)
            .unwrap_or_default()
            .to_string();
        AppInstance {
            name: self.name.clone(),
            resource_id: self.resource_id.clone(),
            resource_group: self.resource_group.clone(),
            subscription_id,
            subscription_name: self.subscription_name.clone(),
            tenant_id: self.tenant_id.clone(),
            app_id: self.app_id.clone(),
            location: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    #[default]
    Auto,
    Truecolor,
    #[serde(rename = "256")]
    #[value(name = "256")]
    Ansi256,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub refresh_secs: u64,
    pub range: TimeRange,
    pub color: ColorMode,
    pub last_instance: Option<SavedInstance>,
    pub recent: Vec<SavedInstance>,
    pub alerts: AlertConfig,
    /// Badge text per App Insights resource ID (keys compared case-insensitively).
    pub labels: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_secs: DEFAULT_REFRESH,
            range: TimeRange::default(),
            color: ColorMode::default(),
            last_instance: None,
            recent: Vec::new(),
            alerts: AlertConfig::default(),
            labels: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Makes `inst` the last used instance and moves it to the front of the
    /// recent list (deduplicated by resource ID, at most 10 entries).
    pub fn remember(&mut self, inst: &AppInstance) {
        let saved = SavedInstance::from(inst);
        self.recent
            .retain(|r| !r.resource_id.eq_ignore_ascii_case(&saved.resource_id));
        self.recent.insert(0, saved.clone());
        self.recent.truncate(MAX_RECENT);
        self.last_instance = Some(saved);
    }

    pub fn label_for(&self, resource_id: &str) -> Option<&str> {
        self.labels
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(resource_id))
            .map(|(_, v)| v.as_str())
    }
}

pub fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.github", "suomiton", "appi-dashboard")
        .map(|d| d.config_dir().join("config.toml"))
}

/// Loads the config. A missing file is the default; an unreadable or
/// invalid one is the default plus a warning for the UI.
pub fn load(path: &Path) -> (Config, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("ignoring invalid {}: {e}", path.display())),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), None),
        Err(e) => (
            Config::default(),
            Some(format!("could not read {}: {e}", path.display())),
        ),
    }
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes the config atomically: a temp file in the same directory is
/// synced and then renamed over the target, so readers (including other
/// running copies) never see a half-written file.
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let text = toml::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "config.toml".into());
    let tmp = dir.join(format!(
        "{file_name}.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    let mut attempt = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::demo::demo_instances;

    fn inst(n: usize) -> AppInstance {
        let mut i = demo_instances().remove(0);
        i.name = format!("app-{n}");
        i.resource_id = format!(
            "/subscriptions/x/resourceGroups/rg/providers/microsoft.insights/components/app-{n}"
        );
        i
    }

    #[test]
    fn load_missing_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let (c, warn) = load(&dir.path().join("nope.toml"));
        assert_eq!(c, Config::default());
        assert!(warn.is_none());
        assert_eq!(c.refresh_secs, 30);
        assert_eq!(c.range, TimeRange::H4);
    }

    #[test]
    fn load_invalid_gives_default_and_warning() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "refresh_secs = [[[").unwrap();
        let (c, warn) = load(&p);
        assert_eq!(c, Config::default());
        assert!(warn.unwrap().contains("config.toml"));
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("config.toml");
        let mut c = Config {
            refresh_secs: 60,
            range: TimeRange::H12,
            color: ColorMode::Ansi256,
            ..Default::default()
        };
        c.remember(&inst(1));
        c.labels.insert("/subscriptions/x/a".into(), "PROD".into());
        save(&p, &c).unwrap();
        let (back, warn) = load(&p);
        assert!(warn.is_none());
        assert_eq!(back, c);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "refresh_secs = 120\n[alerts]\nfailed_requests_5m = 2\n").unwrap();
        let (c, _) = load(&p);
        assert_eq!(c.refresh_secs, 120);
        assert_eq!(c.range, TimeRange::H4);
        assert_eq!(c.alerts.failed_requests_5m, 2.0);
        assert_eq!(c.alerts.server_exceptions_5m, 10.0);
    }

    #[test]
    fn remember_dedupes_and_caps_at_10() {
        let mut c = Config::default();
        for n in 0..12 {
            c.remember(&inst(n));
        }
        let mut again = inst(3);
        again.resource_id = again.resource_id.to_uppercase();
        c.remember(&again);
        assert_eq!(c.recent.len(), 10);
        assert_eq!(c.recent[0].name, "app-3");
        assert_eq!(c.recent.iter().filter(|r| r.name == "app-3").count(), 1);
        assert_eq!(c.last_instance.as_ref().unwrap().name, "app-3");
    }

    #[test]
    fn label_for_is_case_insensitive() {
        let mut c = Config::default();
        c.labels
            .insert("/subscriptions/x/app".into(), "PROD".into());
        assert_eq!(c.label_for("/Subscriptions/X/App"), Some("PROD"));
        assert_eq!(c.label_for("/other"), None);
    }

    #[test]
    fn saved_instance_roundtrip() {
        let i = inst(1);
        assert_eq!(SavedInstance::from(&i).to_instance().app_id, i.app_id);
        assert_eq!(SavedInstance::from(&i).to_instance().name, i.name);
    }

    #[test]
    fn concurrent_saves_never_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let p = std::sync::Arc::new(dir.path().join("config.toml"));
        save(&p, &Config::default()).unwrap();
        let writer = |refresh: u64| {
            let p = p.clone();
            std::thread::spawn(move || {
                let c = Config {
                    refresh_secs: refresh,
                    ..Default::default()
                };
                for _ in 0..50 {
                    save(&p, &c).unwrap();
                }
            })
        };
        let reader = {
            let p = p.clone();
            std::thread::spawn(move || {
                for _ in 0..100 {
                    let (_, warn) = load(&p);
                    assert!(warn.is_none(), "{warn:?}");
                }
            })
        };
        let (a, b) = (writer(15), writer(300));
        a.join().unwrap();
        b.join().unwrap();
        reader.join().unwrap();
        let (c, warn) = load(&p);
        assert!(warn.is_none());
        assert!(c.refresh_secs == 15 || c.refresh_secs == 300);
    }

    #[test]
    fn default_path_ends_with_config_toml() {
        if let Some(p) = default_path() {
            assert!(p.ends_with("config.toml"));
        }
    }
}
