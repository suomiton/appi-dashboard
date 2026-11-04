//! The runtime: terminal setup, the event loop, the data service and effects.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use chrono::{Local, Utc};
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::app::{App, Command, DataEvent, DataRequest};
use crate::azure::auth::AzCliTokenSource;
use crate::azure::http::{AzureHttp, Endpoints};
use crate::cli::Cli;
use crate::config::{self, Config};
use crate::data::backend::{AzureBackend, Backend};
use crate::data::dashboard::refresh_dashboard;
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
                let ev = match req {
                    DataRequest::Refresh {
                        instance,
                        range,
                        view,
                    } => DataEvent::Dashboard(
                        refresh_dashboard(backend.as_ref(), &instance, range, view, Utc::now())
                            .await,
                    ),
                    DataRequest::Discover => DataEvent::Instances(backend.instances().await),
                    DataRequest::Drill {
                        frame_id,
                        instance,
                        range,
                        target,
                    } => DataEvent::Drill {
                        frame_id,
                        result: crate::data::drill::load(
                            backend.as_ref(),
                            &instance,
                            range,
                            &target,
                            Utc::now(),
                        )
                        .await,
                    },
                };
                let _ = tx.send(ev);
            });
        }
    })
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
        s.app.toast(w, Instant::now());
    }
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, s).await;
    ratatui::restore();
    result
}

async fn event_loop(terminal: &mut ratatui::DefaultTerminal, s: Setup) -> Result<(), String> {
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
                app.on_data(ev, Instant::now(), Utc::now())
            }
            _ = tick.tick() => {
                dirty = true;
                app.on_tick(Instant::now())
            }
        };
        pending = cmds;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
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
                view: View::Overview,
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
                view: View::Overview,
            })
            .unwrap();
        req_tx.send(DataRequest::Discover).unwrap();
        assert!(matches!(
            ev_rx.recv().await,
            Some(DataEvent::Instances(Ok(_)))
        ));
        assert!(matches!(ev_rx.recv().await, Some(DataEvent::Dashboard(_))));
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

    fn cli(args: &[&str]) -> Cli {
        use clap::Parser;
        let dir = std::env::temp_dir().join(format!("appi-test-{}", std::process::id()));
        let cfg = dir.join("config.toml");
        let mut all = vec!["appi-dashboard", "--config", cfg.to_str().unwrap()];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).unwrap()
    }

    fn discovers(cmds: &[Command]) -> bool {
        cmds.iter()
            .any(|c| matches!(c, Command::Fetch(DataRequest::Discover)))
    }

    #[test]
    fn app_flag_opens_picker_pending_resolution() {
        let s = setup(&cli(&["--demo", "--app", "demo-app-staging"]));
        assert!(s.app.instance.is_none());
        let p = s.app.picker.as_ref().unwrap();
        assert_eq!(p.pending_app.as_deref(), Some("demo-app-staging"));
        assert!(!p.can_cancel);
        assert!(discovers(&s.initial));
    }

    #[test]
    fn demo_without_app_starts_on_first_demo_instance() {
        let s = setup(&cli(&["--demo"]));
        assert_eq!(s.app.instance.as_ref().unwrap().name, "demo-app-prod");
        assert!(s.app.picker.is_none());
        assert!(s.initial.is_empty());
    }

    #[test]
    fn no_remembered_instance_opens_picker() {
        let s = setup(&cli(&[]));
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
