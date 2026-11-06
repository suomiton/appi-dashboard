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
            Ok(c) => {
                let warning = unknown_keys(&text)
                    .filter(|keys| !keys.is_empty())
                    .map(|keys| {
                        format!(
                            "unknown settings in {}: {}",
                            path.display(),
                            keys.join(", ")
                        )
                    });
                (c, warning)
            }
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

fn unknown_keys(text: &str) -> Option<Vec<String>> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let known = [
        "refresh_secs",
        "range",
        "color",
        "last_instance",
        "recent",
        "alerts",
        "labels",
    ];
    let mut keys: Vec<String> = table
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .cloned()
        .collect();
    if let Some(alerts) = table.get("alerts").and_then(|v| v.as_table()) {
        let known = [
            "failed_requests_5m",
            "server_exceptions_5m",
            "dependency_failures_5m",
            "availability_below_pct",
            "response_avg_above_ms",
        ];
        keys.extend(
            alerts
                .keys()
                .filter(|k| !known.contains(&k.as_str()))
                .map(|k| format!("alerts.{k}")),
        );
    }
    keys.sort();
    Some(keys)
}

/// Saves the settings the app itself manages (refresh interval, range and
/// remembered instances) into the file as it is on disk now, so labels,
/// alert thresholds and other hand edits survive. Refuses to write when the
/// file exists but cannot be read or parsed, rather than replacing it.
pub fn persist(path: &Path, app: &Config) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return save(path, app).map_err(|e| format!("could not save {}: {e}", path.display()));
        }
        Err(e) => {
            return Err(format!(
                "not saving settings: cannot read {}: {e}",
                path.display()
            ));
        }
    };
    let invalid = |e| format!("not saving settings: {} has errors ({e})", path.display());
    toml::from_str::<Config>(&text).map_err(|e| invalid(e.to_string()))?;
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| invalid(e.to_string()))?;
    let mut updates = toml::to_string_pretty(app)
        .map_err(|e| invalid(e.to_string()))?
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| invalid(e.to_string()))?;
    for key in ["refresh_secs", "range", "last_instance", "recent"] {
        let Some(mut replacement) = updates.remove(key) else {
            doc.remove(key);
            continue;
        };
        // Keep comments attached to scalar settings, including inline comments.
        if let (Some(old), Some(new)) = (
            doc.get(key).and_then(|v| v.as_value()),
            replacement.as_value_mut(),
        ) {
            *new.decor_mut() = old.decor().clone();
        }
        doc[key] = replacement;
    }
    write_atomic(path, &doc.to_string())
        .map_err(|e| format!("could not save {}: {e}", path.display()))
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes the config atomically: a temp file in the same directory is
/// synced and then renamed over the target, so readers (including other
/// running copies) never see a half-written file.
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    let text = toml::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    write_atomic(path, &text)
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
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
    fn persist_keeps_hand_edits() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "refresh_secs = 30\n[alerts]\nfailed_requests_5m = 2\n[labels]\n\"/subscriptions/x/app\" = \"PROD\"\n",
        )
        .unwrap();
        let mut app_cfg = Config {
            refresh_secs: 60,
            ..Default::default()
        };
        app_cfg.remember(&inst(1));
        persist(&p, &app_cfg).unwrap();
        let (back, warn) = load(&p);
        assert!(warn.is_none());
        assert_eq!(back.refresh_secs, 60);
        assert_eq!(back.last_instance.as_ref().unwrap().name, "app-1");
        assert_eq!(
            back.alerts.failed_requests_5m, 2.0,
            "hand-edited alert kept"
        );
        assert_eq!(
            back.label_for("/subscriptions/x/app"),
            Some("PROD"),
            "labels kept"
        );
    }

    #[test]
    fn persist_preserves_comments_formatting_and_unknown_settings() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let text = "# My dashboard\nrefresh_secs = 30 # polling\nrange = '4h' # history\ncolor = '256'\nrefresh_sec = 120 # typo\n\n[alerts] # custom thresholds\nfailed_requests_5m = 2 # noisy app\nunknown_threshold = 9\n\n[plugin]\ncustom = 'leave me' # extension\n";
        std::fs::write(&p, text).unwrap();
        let (mut cfg, warning) = load(&p);
        let warning = warning.unwrap();
        for key in ["refresh_sec", "alerts.unknown_threshold", "plugin"] {
            assert!(warning.contains(key));
        }
        cfg.refresh_secs = 60;
        cfg.range = TimeRange::H12;
        cfg.remember(&inst(1));
        persist(&p, &cfg).unwrap();
        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(saved.contains("refresh_secs = 60 # polling"), "{saved}");
        assert!(saved.contains("# history"));
        assert!(saved.starts_with("# My dashboard\n"));
        assert!(saved.contains("color = '256'"));
        assert!(saved.contains("refresh_sec = 120 # typo"));
        assert!(saved.contains("[alerts] # custom thresholds\nfailed_requests_5m = 2 # noisy app\nunknown_threshold = 9"));
        assert!(saved.contains("[plugin]\ncustom = 'leave me' # extension"));
        let (back, _) = load(&p);
        assert_eq!(back.refresh_secs, 60);
        assert_eq!(back.range, TimeRange::H12);
        assert_eq!(back.last_instance.unwrap().name, "app-1");
        assert_eq!(back.alerts.failed_requests_5m, 2.0);
    }

    #[test]
    fn persist_refuses_to_overwrite_an_invalid_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "refresh_secs = [[[").unwrap();
        let err = persist(&p, &Config::default()).unwrap_err();
        assert!(err.contains("not saving"), "{err}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "refresh_secs = [[[");
    }

    #[test]
    fn persist_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("new").join("config.toml");
        persist(
            &p,
            &Config {
                range: TimeRange::H12,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(load(&p).0.range, TimeRange::H12);
    }

    #[test]
    fn default_path_ends_with_config_toml() {
        if let Some(p) = default_path() {
            assert!(p.ends_with("config.toml"));
        }
    }
}
