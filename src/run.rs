//! The runtime: terminal setup, the event loop, the data service and effects.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::{Local, Utc};
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures_util::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::app::{App, Command, DataEvent, DataRequest};
use crate::azure::auth::AzCliTokenSource;
use crate::azure::http::{AzError, AzureHttp, Endpoints};
use crate::cli::Cli;
use crate::config::{self, Config};
use crate::data::backend::{AzureBackend, Backend};
use crate::data::dashboard::{ALL_SOURCES, DashboardUpdate, refresh_dashboard};
use crate::data::demo::{DemoBackend, demo_instances};
use crate::data::time::{DEFAULT_REFRESH, REFRESH_STEPS, TimeRange};
use crate::ui::theme::{Theme, detect_depth, resolve_depth};

/// Runs each request in its own task so a slow refresh never delays a
/// drill-down or discovery, and sends the result back as an event.
pub fn spawn_data_service(
    backend: Arc<dyn Backend>,
    mut rx: mpsc::UnboundedReceiver<DataRequest>,
    tx: mpsc::UnboundedSender<DataEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let backend = backend.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let ev = answer_request(backend.as_ref(), req, Duration::from_secs(300)).await;
                let _ = tx.send(ev);
            });
        }
    })
}

/// Bound the whole request, including authentication and retries. Dropping
/// timed-out futures cancels their work; panics still produce a completion event.
async fn answer_request(backend: &dyn Backend, req: DataRequest, limit: Duration) -> DataEvent {
    let work = async {
        match &req {
            DataRequest::Refresh { instance, range } => {
                DataEvent::Dashboard(refresh_dashboard(backend, instance, *range, Utc::now()).await)
            }
            DataRequest::Discover => DataEvent::Instances(backend.instances().await),
            DataRequest::Drill {
                frame_id,
                instance,
                range,
                target,
            } => DataEvent::Drill {
                frame_id: *frame_id,
                result: crate::data::drill::load(backend, instance, *range, target, Utc::now())
                    .await,
            },
        }
    };
    let failure = match std::panic::AssertUnwindSafe(tokio::time::timeout(limit, work))
        .catch_unwind()
        .await
    {
        Ok(Ok(ev)) => return ev,
        Ok(Err(_)) => "timed out",
        Err(_) => "task panicked",
    };
    match req {
        DataRequest::Refresh { .. } => DataEvent::Dashboard(DashboardUpdate {
            errors: ALL_SOURCES
                .into_iter()
                .map(|s| (s, AzError::Network(format!("refresh {failure}"))))
                .collect(),
            ..Default::default()
        }),
        DataRequest::Discover => {
            DataEvent::Instances(Err(AzError::Network(format!("discovery {failure}"))))
        }
        DataRequest::Drill { frame_id, .. } => DataEvent::Drill {
            frame_id,
            result: Err(AzError::Network(format!("drill {failure}"))),
        },
    }
}

struct Shutdown {
    #[cfg(unix)]
    term: tokio::signal::unix::Signal,
    #[cfg(unix)]
    hup: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl Shutdown {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            term: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
            #[cfg(unix)]
            hup: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?,
            #[cfg(unix)]
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
        })
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.term.recv() => {},
            _ = self.hup.recv() => {},
            _ = self.interrupt.recv() => {},
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    }
}

/// Appends error metadata only. Requests, headers and token responses never
/// reach this writer. JSON escaping keeps multiline messages on one log line.
fn log_event(out: &mut dyn Write, ev: &DataEvent) -> std::io::Result<()> {
    let errors: Vec<(String, &AzError)> = match ev {
        DataEvent::Dashboard(u) => u
            .errors
            .iter()
            .map(|(s, e)| (format!("{s:?}"), e))
            .collect(),
        DataEvent::Instances(Err(e)) => vec![("Discovery".into(), e)],
        DataEvent::Drill {
            frame_id,
            result: Err(e),
        } => vec![(format!("Drill({frame_id})"), e)],
        _ => Vec::new(),
    };
    for (source, error) in errors {
        let entry = serde_json::json!({"at": Utc::now().to_rfc3339(), "source": source, "error": error.to_string()});
        writeln!(out, "{entry}")?;
    }
    out.flush()
}

fn open_error_log(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Windows reports key releases too; only presses and repeats are input.
pub fn accept_key(ev: &KeyEvent) -> bool {
    ev.kind != KeyEventKind::Release
}

/// A refresh interval from the config file that is not one of the steps
/// falls back to the default.
pub fn normalize_refresh(secs: u64) -> u64 {
    if REFRESH_STEPS.contains(&secs) {
        secs
    } else {
        DEFAULT_REFRESH
    }
}

/// Clipboard and browser side effects.
pub struct Effects {
    clipboard: Option<arboard::Clipboard>,
}

impl Default for Effects {
    fn default() -> Self {
        Self::new()
    }
}

impl Effects {
    pub fn new() -> Self {
        Self {
            clipboard: arboard::Clipboard::new().ok(),
        }
    }

    /// Copies via the system clipboard, falling back to the terminal's
    /// OSC 52 sequence (works over SSH and in most modern terminals).
    pub fn copy(&mut self, text: &str) -> Result<(), String> {
        if let Some(c) = self.clipboard.as_mut()
            && c.set_text(text.to_string()).is_ok()
        {
            return Ok(());
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut out = std::io::stdout();
        write!(out, "\x1b]52;c;{encoded}\x07").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())
    }

    pub fn open(&self, url: &str) -> Result<(), String> {
        open::that_detached(url).map_err(|e| format!("could not open browser: {e}"))
    }
}

struct Setup {
    app: App,
    theme: Theme,
    backend: Arc<dyn Backend>,
    config_path: Option<PathBuf>,
    demo: bool,
    warning: Option<String>,
    /// Commands to run before the first event (e.g. discovery for --app).
    initial: Vec<Command>,
}

fn setup(cli: &Cli) -> Setup {
    let config_path = cli.config.clone().or_else(config::default_path);
    let (config, warning) = config_path
        .as_deref()
        .map(config::load)
        .unwrap_or((Config::default(), None));
    let range: TimeRange = cli.range.unwrap_or(config.range);
    let refresh = cli
        .refresh
        .unwrap_or(normalize_refresh(config.refresh_secs));
    let backend: Arc<dyn Backend> = if cli.demo {
        Arc::new(DemoBackend::new())
    } else {
        let tokens = Arc::new(AzCliTokenSource::new());
        Arc::new(AzureBackend::new(AzureHttp::new(
            tokens,
            Endpoints::default(),
        )))
    };
    let instance = match (&cli.app, cli.demo) {
        (Some(_), _) => None,
        (None, true) => demo_instances().into_iter().next(),
        (None, false) => config.last_instance.as_ref().map(|s| s.to_instance()),
    };
    let depth = resolve_depth(
        cli.color.unwrap_or(config.color),
        detect_depth(|k| std::env::var(k).ok()),
    );
    let mut app = App::new(config, instance, range, refresh);
    let initial = match &cli.app {
        Some(arg) => app.open_picker(false, Some(arg.clone())),
        None if app.instance.is_none() => app.open_picker(false, None),
        None => Vec::new(),
    };
    Setup {
        app,
        theme: Theme::new(depth),
        backend,
        config_path,
        demo: cli.demo,
        warning,
        initial,
    }
}

pub async fn run(cli: Cli) -> Result<(), String> {
    let mut s = setup(&cli);
    if let Some(w) = s.warning.take() {
        s.app.toast_for(w, Instant::now(), Duration::from_secs(30));
    }
    let shutdown = Shutdown::new().map_err(|e| format!("could not listen for shutdown: {e}"))?;
    let log = cli
        .log
        .as_deref()
        .map(open_error_log)
        .transpose()
        .map_err(|e| format!("could not open error log: {e}"))?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, s, shutdown, log).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    s: Setup,
    mut shutdown: Shutdown,
    mut log: Option<std::fs::File>,
) -> Result<(), String> {
    let Setup {
        mut app,
        theme,
        backend,
        config_path,
        demo,
        initial,
        ..
    } = s;
    let (req_tx, req_rx) = mpsc::unbounded_channel();
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    spawn_data_service(backend, req_rx, ev_tx);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut effects = Effects::new();
    let mut dirty = true;
    let mut pending = initial;
    loop {
        for cmd in pending.drain(..) {
            match cmd {
                Command::Fetch(r) => {
                    let _ = req_tx.send(r);
                }
                Command::SaveConfig(cfg) => {
                    if !demo
                        && let Some(p) = &config_path
                        && let Err(e) = config::persist(p, &cfg)
                    {
                        app.toast(e, Instant::now());
                    }
                }
                Command::Copy(text) => match effects.copy(&text) {
                    Ok(()) => app.toast("copied", Instant::now()),
                    Err(e) => app.toast(e, Instant::now()),
                },
                Command::OpenUrl(url) => {
                    if let Err(e) = effects.open(&url) {
                        app.toast(e, Instant::now());
                    }
                }
                Command::Quit => return Ok(()),
            }
            dirty = true;
        }
        if dirty {
            terminal
                .draw(|f| {
                    crate::ui::draw(f, &app, &theme, Local::now().fixed_offset(), Instant::now())
                })
                .map_err(|e| e.to_string())?;
            dirty = false;
        }
        let cmds = tokio::select! {
            _ = shutdown.recv() => return Ok(()),
            maybe = events.next() => match maybe {
                Some(Ok(Event::Key(k))) if accept_key(&k) => {
                    dirty = true;
                    app.on_key(k, Instant::now())
                }
                Some(Ok(Event::Resize(..))) => {
                    dirty = true;
                    Vec::new()
                }
                Some(Ok(_)) => Vec::new(),
                Some(Err(e)) => return Err(e.to_string()),
                None => return Ok(()),
            },
            Some(ev) = ev_rx.recv() => {
                dirty = true;
                if let Some(out) = log.as_mut() && let Err(e) = log_event(out, &ev) {
                    app.toast(format!("could not write error log: {e}"), Instant::now());
                    log = None;
                }
                app.on_data(ev, Instant::now(), Utc::now())
            }
            _ = tick.tick() => {
                dirty = true;
                app.on_tick(Instant::now(), Utc::now())
            }
        };
        pending = cmds;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::backend::fake::FakeBackend;
    use crate::data::demo::{DemoBackend, demo_instances};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn channels() -> (
        mpsc::UnboundedSender<DataRequest>,
        mpsc::UnboundedReceiver<DataRequest>,
        mpsc::UnboundedSender<DataEvent>,
        mpsc::UnboundedReceiver<DataEvent>,
    ) {
        let (a, b) = mpsc::unbounded_channel();
        let (c, d) = mpsc::unbounded_channel();
        (a, b, c, d)
    }

    #[tokio::test]
    async fn data_service_answers_refresh() {
        let (req_tx, req_rx, ev_tx, mut ev_rx) = channels();
        spawn_data_service(Arc::new(DemoBackend::new()), req_rx, ev_tx);
        req_tx
            .send(DataRequest::Refresh {
                instance: demo_instances().remove(0),
                range: TimeRange::H1,
            })
            .unwrap();
        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), ev_rx.recv())
            .await
            .unwrap()
            .unwrap();
        match ev {
            DataEvent::Dashboard(u) => assert!(!u.series.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn data_service_runs_requests_concurrently() {
        let (req_tx, req_rx, ev_tx, mut ev_rx) = channels();
        let backend = FakeBackend::new()
            .with_instances(demo_instances())
            .with_query_delay(std::time::Duration::from_millis(300));
        spawn_data_service(Arc::new(backend), req_rx, ev_tx);
        req_tx
            .send(DataRequest::Refresh {
                instance: demo_instances().remove(0),
                range: TimeRange::H1,
            })
            .unwrap();
        req_tx.send(DataRequest::Discover).unwrap();
        assert!(matches!(
            ev_rx.recv().await,
            Some(DataEvent::Instances(Ok(_)))
        ));
        assert!(matches!(ev_rx.recv().await, Some(DataEvent::Dashboard(_))));
    }

    #[tokio::test]
    async fn watchdog_turns_timeouts_and_panics_into_dashboard_errors() {
        let hung = FakeBackend::new().with_query_delay(Duration::from_secs(60));
        let panicked = FakeBackend::new().with_metrics(|_| panic!("backend bug"));
        for (backend, message) in [(&hung, "timed out"), (&panicked, "task panicked")] {
            let ev = answer_request(
                backend,
                DataRequest::Refresh {
                    instance: demo_instances().remove(0),
                    range: TimeRange::H4,
                },
                Duration::from_millis(10),
            )
            .await;
            let DataEvent::Dashboard(u) = ev else {
                panic!("dashboard completion missing")
            };
            assert_eq!(u.errors.len(), ALL_SOURCES.len());
            assert!(
                u.errors
                    .iter()
                    .all(|(_, e)| e.to_string().contains(message))
            );
            let mut app = crate::app::tests::app();
            let now = Instant::now();
            app.on_tick(now, Utc::now());
            app.on_data(DataEvent::Dashboard(u), now, Utc::now());
            assert!(!app.refreshing);
            assert_eq!(
                app.on_tick(now + Duration::from_secs(30), Utc::now()).len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn watchdog_completes_stalled_drill_requests_too() {
        let backend = FakeBackend::new().with_query_delay(Duration::from_secs(60));
        let ev = answer_request(
            &backend,
            DataRequest::Drill {
                frame_id: 42,
                instance: demo_instances().remove(0),
                range: TimeRange::H4,
                target: crate::data::drill::DrillTarget::FailingEndpoints,
            },
            Duration::from_millis(10),
        )
        .await;
        assert!(
            matches!(ev, DataEvent::Drill { frame_id: 42, result: Err(AzError::Network(m)) } if m.contains("timed out"))
        );
    }

    #[test]
    fn error_log_appends_full_messages_with_sources_and_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("errors.jsonl");
        let message = format!("403: {}\nsecond line", "detailed reason ".repeat(20));
        let ev = DataEvent::Dashboard(DashboardUpdate {
            errors: vec![(
                crate::data::dashboard::Source::Tops,
                AzError::Network(message.clone()),
            )],
            ..Default::default()
        });
        log_event(&mut open_error_log(&path).unwrap(), &ev).unwrap();
        log_event(
            &mut open_error_log(&path).unwrap(),
            &DataEvent::Instances(Err(AzError::Network("discovery failed".into()))),
        )
        .unwrap();
        log_event(
            &mut open_error_log(&path).unwrap(),
            &DataEvent::Dashboard(DashboardUpdate::default()),
        )
        .unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["source"], "Tops");
        assert_eq!(rows[0]["error"], format!("network: {message}"));
        assert!(chrono::DateTime::parse_from_rfc3339(rows[0]["at"].as_str().unwrap()).is_ok());
        assert_eq!(rows[1]["source"], "Discovery");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn accept_key_ignores_release() {
        let mut ev = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(accept_key(&ev));
        ev.kind = KeyEventKind::Release;
        assert!(!accept_key(&ev));
        let repeat = KeyEvent::new_with_kind_and_state(
            KeyCode::Down,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
            KeyEventState::NONE,
        );
        assert!(accept_key(&repeat));
    }

    fn cli(args: &[&str]) -> (Cli, tempfile::TempDir) {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config.toml");
        let mut all = vec!["appi-dashboard", "--config", cfg.to_str().unwrap()];
        all.extend_from_slice(args);
        (Cli::try_parse_from(all).unwrap(), dir)
    }

    fn discovers(cmds: &[Command]) -> bool {
        cmds.iter()
            .any(|c| matches!(c, Command::Fetch(DataRequest::Discover)))
    }

    #[test]
    fn app_flag_opens_picker_pending_resolution() {
        let (cli, _dir) = cli(&["--demo", "--app", "demo-app-staging"]);
        let s = setup(&cli);
        assert!(s.app.instance.is_none());
        let p = s.app.picker.as_ref().unwrap();
        assert_eq!(p.pending_app.as_deref(), Some("demo-app-staging"));
        assert!(!p.can_cancel);
        assert!(discovers(&s.initial));
    }

    #[test]
    fn demo_without_app_starts_on_first_demo_instance() {
        let (cli, _dir) = cli(&["--demo"]);
        let s = setup(&cli);
        assert_eq!(s.app.instance.as_ref().unwrap().name, "demo-app-prod");
        assert!(s.app.picker.is_none());
        assert!(s.initial.is_empty());
    }

    #[test]
    fn no_remembered_instance_opens_picker() {
        let (cli, _dir) = cli(&[]);
        let s = setup(&cli);
        assert!(s.app.instance.is_none());
        assert!(s.app.picker.is_some());
        assert!(discovers(&s.initial));
    }

    #[test]
    fn startup_refresh_falls_back_to_default_for_unknown_values() {
        assert_eq!(normalize_refresh(60), 60);
        assert_eq!(normalize_refresh(45), 30);
        assert_eq!(normalize_refresh(0), 30);
    }
}
