//! The app's state machine: keys, ticks and data in; commands out. No I/O here.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::azure::auth::AuthError;
use crate::azure::discovery::AppInstance;
use crate::azure::http::AzError;
use crate::config::Config;
use crate::data::alerts::{AlertState, evaluate};
use crate::data::dashboard::DashboardUpdate;
pub use crate::data::dashboard::View;
use crate::data::store::Store;
use crate::data::time::{TimeRange, refresh_step};

const TOAST_SECS: u64 = 3;
const AUTH_RETRY_SECS: u64 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Sessions,
    FailedRequests,
    ServerResponse,
    PageLoad,
    ExceptionsDeps,
    Cpu,
    Availability,
    BrowserExceptions,
    MemoryIo,
    FailuresMain,
    Health,
    DependencyFailures,
}

/// Panels in hotkey order: 1–9, then 0.
pub fn panels(view: View) -> &'static [PanelId] {
    use PanelId::*;
    match view {
        View::Overview => &[
            Sessions,
            FailedRequests,
            ServerResponse,
            PageLoad,
            ExceptionsDeps,
            Cpu,
            Availability,
            BrowserExceptions,
            MemoryIo,
        ],
        View::Failures => &[
            FailuresMain,
            Health,
            Sessions,
            ServerResponse,
            PageLoad,
            BrowserExceptions,
            Cpu,
            MemoryIo,
            DependencyFailures,
            Availability,
        ],
    }
}

const HOTKEYS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];

pub fn hotkey(view: View, p: PanelId) -> Option<char> {
    panels(view)
        .iter()
        .position(|x| *x == p)
        .and_then(|i| HOTKEYS.get(i).copied())
}

#[derive(Clone, Debug)]
pub enum DataRequest {
    Refresh {
        instance: AppInstance,
        range: TimeRange,
        view: View,
    },
    Discover,
}

/// Each event moves through the channel exactly once, so the size
/// difference between variants costs nothing worth a heap allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DataEvent {
    Dashboard(DashboardUpdate),
    Instances(Result<Vec<AppInstance>, AzError>),
}

#[derive(Clone, Debug)]
pub enum Command {
    Fetch(DataRequest),
    SaveConfig(Config),
    Copy(String),
    OpenUrl(String),
    Quit,
}

pub struct App {
    pub config: Config,
    pub instance: Option<AppInstance>,
    pub view: View,
    pub focus: Option<PanelId>,
    pub range: TimeRange,
    pub refresh_secs: u64,
    pub store: Store,
    pub alerts: AlertState,
    pub help: bool,
    pub auth_problem: Option<AuthError>,
    pub toast: Option<(String, Instant)>,
    pub refreshing: bool,
    pub next_refresh: Option<Instant>,
    /// Set when the range or instance changes while a refresh is in flight:
    /// that refresh's result belongs to the old selection and is dropped.
    discard_in_flight: bool,
}

impl App {
    pub fn new(
        config: Config,
        instance: Option<AppInstance>,
        range: TimeRange,
        refresh_secs: u64,
    ) -> Self {
        Self {
            config,
            instance,
            view: View::Overview,
            focus: None,
            range,
            refresh_secs,
            store: Store::default(),
            alerts: AlertState::default(),
            help: false,
            auth_problem: None,
            toast: None,
            refreshing: false,
            next_refresh: None,
            discard_in_flight: false,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>, now: Instant) {
        self.toast = Some((msg.into(), now));
    }

    pub fn seconds_to_refresh(&self, now: Instant) -> Option<u64> {
        self.next_refresh
            .map(|t| t.saturating_duration_since(now).as_secs())
    }

    /// Starts a refresh unless one is running or there is no instance.
    fn fetch_now(&mut self) -> Vec<Command> {
        match (&self.instance, self.refreshing) {
            (Some(inst), false) => {
                self.refreshing = true;
                vec![Command::Fetch(DataRequest::Refresh {
                    instance: inst.clone(),
                    range: self.range,
                    view: self.view,
                })]
            }
            _ => Vec::new(),
        }
    }

    /// Forgets data for the old selection and fetches for the new one.
    fn reset_data(&mut self, now: Instant) -> Vec<Command> {
        self.store.clear();
        self.alerts = AlertState::default();
        if self.refreshing {
            self.discard_in_flight = true;
            self.next_refresh = Some(now);
            Vec::new()
        } else {
            self.fetch_now()
        }
    }

    pub fn on_tick(&mut self, now: Instant) -> Vec<Command> {
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) >= Duration::from_secs(TOAST_SECS))
        {
            self.toast = None;
        }
        if self.next_refresh.is_none_or(|t| now >= t) {
            self.fetch_now()
        } else {
            Vec::new()
        }
    }

    pub fn on_data(&mut self, ev: DataEvent, now: Instant, wall: DateTime<Utc>) -> Vec<Command> {
        match ev {
            DataEvent::Dashboard(u) => {
                self.refreshing = false;
                if std::mem::take(&mut self.discard_in_flight) {
                    return self.fetch_now();
                }
                let auth = u.errors.iter().find_map(|(_, e)| match e {
                    AzError::Auth(a) => Some(a.clone()),
                    _ => None,
                });
                self.store.apply(u, wall);
                self.alerts = evaluate(&self.config.alerts, &self.store);
                let wait = if auth.is_some() {
                    AUTH_RETRY_SECS
                } else {
                    self.refresh_secs
                };
                self.auth_problem = auth;
                self.next_refresh = Some(now + Duration::from_secs(wait));
                Vec::new()
            }
            DataEvent::Instances(_) => Vec::new(),
        }
    }

    fn move_focus(&mut self, forward: bool) {
        let list = panels(self.view);
        let n = list.len();
        let next = match self.focus.and_then(|f| list.iter().position(|p| *p == f)) {
            None => {
                if forward {
                    0
                } else {
                    n - 1
                }
            }
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
        };
        self.focus = Some(list[next]);
    }

    pub fn on_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Command> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return vec![Command::Quit];
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.view = match self.view {
                    View::Overview => View::Failures,
                    View::Failures => View::Overview,
                };
                self.focus = None;
                if self.view == View::Failures {
                    return self.fetch_now();
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let Some(i) = HOTKEYS.iter().position(|h| *h == c)
                    && let Some(p) = panels(self.view).get(i)
                {
                    self.focus = Some(*p);
                }
            }
            KeyCode::Right | KeyCode::Down => self.move_focus(true),
            KeyCode::Left | KeyCode::Up => self.move_focus(false),
            KeyCode::Esc => {
                if self.help {
                    self.help = false;
                } else {
                    self.focus = None;
                }
            }
            KeyCode::Char('r') => return self.fetch_now(),
            KeyCode::Char(c @ ('+' | '-')) => {
                self.refresh_secs = refresh_step(self.refresh_secs, c == '+');
                self.config.refresh_secs = self.refresh_secs;
                self.next_refresh = Some(now + Duration::from_secs(self.refresh_secs));
                return vec![Command::SaveConfig(self.config.clone())];
            }
            KeyCode::Char('t') => {
                self.range = self.range.next();
                self.config.range = self.range;
                let mut cmds = vec![Command::SaveConfig(self.config.clone())];
                cmds.extend(self.reset_data(now));
                return cmds;
            }
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('q') => return vec![Command::Quit],
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::azure::auth::AuthError;
    use crate::data::catalog::SeriesId;
    use crate::data::dashboard::{AlertWindow, Series, Source};
    use crate::data::demo::demo_instances;
    use chrono::TimeZone;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    pub(crate) fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(crate) fn ch(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }

    pub(crate) fn app() -> App {
        App::new(
            Config::default(),
            Some(demo_instances().remove(0)),
            TimeRange::H4,
            30,
        )
    }

    fn wall() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, 13, 0, 0).unwrap()
    }

    fn good_update() -> DashboardUpdate {
        let mut u = DashboardUpdate::default();
        u.series.insert(
            SeriesId::RequestsFailed,
            Series {
                points: vec![(wall(), Some(1.0))],
            },
        );
        u.alert_window = Some(AlertWindow {
            failed_5m: 12.0,
            ..Default::default()
        });
        u
    }

    fn fetches(cmds: &[Command]) -> usize {
        cmds.iter()
            .filter(|c| matches!(c, Command::Fetch(DataRequest::Refresh { .. })))
            .count()
    }

    #[test]
    fn tab_toggles_view_and_clears_focus() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('2'), now);
        assert_eq!(a.focus, Some(PanelId::FailedRequests));
        a.on_key(k(KeyCode::Tab), now);
        assert_eq!(a.view, View::Failures);
        assert_eq!(a.focus, None);
        a.on_key(k(KeyCode::BackTab), now);
        assert_eq!(a.view, View::Overview);
    }

    #[test]
    fn digit_focuses_panel() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('2'), now);
        assert_eq!(a.focus, Some(PanelId::FailedRequests));
        a.on_key(ch('0'), now);
        assert_eq!(
            a.focus,
            Some(PanelId::FailedRequests),
            "no panel 0 on overview"
        );
        a.on_key(k(KeyCode::Tab), now);
        a.on_key(ch('0'), now);
        assert_eq!(a.focus, Some(PanelId::Availability));
        assert_eq!(hotkey(View::Failures, PanelId::FailuresMain), Some('1'));
        assert_eq!(hotkey(View::Overview, PanelId::Health), None);
    }

    #[test]
    fn arrows_cycle_focus() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(k(KeyCode::Right), now);
        assert_eq!(a.focus, Some(PanelId::Sessions));
        a.on_key(k(KeyCode::Left), now);
        assert_eq!(a.focus, Some(PanelId::MemoryIo));
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.focus, Some(PanelId::Sessions));
        a.on_key(k(KeyCode::Esc), now);
        assert_eq!(a.focus, None);
    }

    #[test]
    fn tick_starts_one_refresh() {
        let mut a = app();
        let t0 = Instant::now();
        assert_eq!(fetches(&a.on_tick(t0)), 1);
        assert!(a.refreshing);
        assert_eq!(fetches(&a.on_tick(t0 + Duration::from_secs(5))), 0);
        a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert!(!a.refreshing);
        assert_eq!(fetches(&a.on_tick(t0 + Duration::from_secs(29))), 0);
        assert_eq!(a.seconds_to_refresh(t0 + Duration::from_secs(20)), Some(10));
        assert_eq!(fetches(&a.on_tick(t0 + Duration::from_secs(30))), 1);
    }

    #[test]
    fn no_instance_no_refresh() {
        let mut a = App::new(Config::default(), None, TimeRange::H4, 30);
        assert!(a.on_tick(Instant::now()).is_empty());
    }

    #[test]
    fn failed_refresh_keeps_data() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert!(a.alerts.failures());
        let mut bad = DashboardUpdate::default();
        bad.errors
            .push((Source::CountMetrics, AzError::Network("down".into())));
        a.on_tick(t0 + Duration::from_secs(30));
        a.on_data(
            DataEvent::Dashboard(bad),
            t0 + Duration::from_secs(31),
            wall(),
        );
        assert!(a.store.has_data(SeriesId::RequestsFailed));
        assert!(
            a.alerts.failures(),
            "alerts are computed from the kept data"
        );
    }

    #[test]
    fn plus_minus_step_refresh_and_save() {
        let mut a = app();
        let now = Instant::now();
        let cmds = a.on_key(ch('+'), now);
        assert_eq!(a.refresh_secs, 60);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::SaveConfig(cfg) if cfg.refresh_secs == 60))
        );
        assert_eq!(a.seconds_to_refresh(now), Some(60));
        a.on_key(ch('-'), now);
        a.on_key(ch('-'), now);
        a.on_key(ch('-'), now);
        assert_eq!(a.refresh_secs, 15);
    }

    #[test]
    fn t_cycles_range_clears_store_and_fetches() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        let cmds = a.on_key(ch('t'), t0);
        assert_eq!(a.range, TimeRange::H12);
        assert!(!a.store.has_data(SeriesId::RequestsFailed));
        assert!(!a.alerts.failures());
        assert_eq!(fetches(&cmds), 1);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Command::SaveConfig(cfg) if cfg.range == TimeRange::H12))
        );
    }

    #[test]
    fn range_change_discards_in_flight_result() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        let cmds = a.on_key(ch('t'), t0);
        assert_eq!(fetches(&cmds), 0, "a refresh is already in flight");
        let cmds = a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert!(
            !a.store.has_data(SeriesId::RequestsFailed),
            "old-range data dropped"
        );
        assert_eq!(fetches(&cmds), 1, "refetches for the new range");
        assert!(a.refreshing);
    }

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut a = app();
        let now = Instant::now();
        assert!(matches!(a.on_key(ch('q'), now).as_slice(), [Command::Quit]));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(a.on_key(ctrl_c, now).as_slice(), [Command::Quit]));
    }

    #[test]
    fn r_refreshes_now_unless_running() {
        let mut a = app();
        let now = Instant::now();
        assert_eq!(fetches(&a.on_key(ch('r'), now)), 1);
        assert_eq!(fetches(&a.on_key(ch('r'), now)), 0);
    }

    #[test]
    fn help_toggles_and_esc_closes_it() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('?'), now);
        assert!(a.help);
        a.on_key(ch('2'), now);
        a.on_key(k(KeyCode::Esc), now);
        assert!(!a.help);
        assert_eq!(
            a.focus,
            Some(PanelId::FailedRequests),
            "esc closed help only"
        );
    }

    #[test]
    fn auth_error_sets_auth_problem() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        let mut u = DashboardUpdate::default();
        u.errors.push((
            Source::Scalars,
            AzError::Auth(AuthError::NotLoggedIn("run az login".into())),
        ));
        a.on_data(DataEvent::Dashboard(u), t0, wall());
        assert!(matches!(a.auth_problem, Some(AuthError::NotLoggedIn(_))));
        assert_eq!(a.seconds_to_refresh(t0), Some(15));
        a.on_tick(t0 + Duration::from_secs(15));
        a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert!(a.auth_problem.is_none());
    }

    #[test]
    fn data_never_changes_view() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_key(k(KeyCode::Tab), t0);
        a.on_key(ch('2'), t0);
        for i in 0..3 {
            a.on_tick(t0 + Duration::from_secs(i * 30));
            a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        }
        assert_eq!(a.view, View::Failures);
        assert_eq!(a.focus, Some(PanelId::Health));
    }

    #[test]
    fn switch_to_failures_fetches_when_idle() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert_eq!(fetches(&a.on_key(k(KeyCode::Tab), t0)), 1);
        assert_eq!(
            fetches(&a.on_key(k(KeyCode::Tab), t0)),
            0,
            "back to overview: no fetch"
        );
    }

    #[test]
    fn toasts_expire() {
        let mut a = app();
        let t0 = Instant::now();
        a.toast("copied", t0);
        a.on_tick(t0 + Duration::from_secs(1));
        assert!(a.toast.is_some());
        a.on_tick(t0 + Duration::from_secs(4));
        assert!(a.toast.is_none());
    }
}
