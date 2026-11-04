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
use crate::data::drill::{Cell, DrillTarget, ScreenData, sort_rows};
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

/// Which drill-down screen a panel opens, if any.
pub fn drill_target(p: PanelId) -> Option<DrillTarget> {
    match p {
        PanelId::FailedRequests | PanelId::FailuresMain => Some(DrillTarget::FailingEndpoints),
        PanelId::ExceptionsDeps => Some(DrillTarget::ExceptionTypes { browser: false }),
        PanelId::BrowserExceptions => Some(DrillTarget::ExceptionTypes { browser: true }),
        PanelId::DependencyFailures => Some(DrillTarget::FailingDependencies),
        PanelId::ServerResponse => Some(DrillTarget::SlowEndpoints),
        PanelId::PageLoad => Some(DrillTarget::SlowPages),
        PanelId::Availability => Some(DrillTarget::AvailabilityTests),
        PanelId::Cpu => Some(DrillTarget::InstanceBreakdown {
            metric: crate::data::drill::InstanceMetric::Cpu,
        }),
        PanelId::MemoryIo => Some(DrillTarget::InstanceBreakdown {
            metric: crate::data::drill::InstanceMetric::Memory,
        }),
        PanelId::Sessions | PanelId::Health => None,
    }
}

/// Frames live in a short stack (a handful at most), so the size
/// difference between variants is not worth boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum FrameData {
    Loading,
    Ready(ScreenData),
    Failed(String),
}

/// One open drill-down screen.
#[derive(Clone, Debug)]
pub struct DrillFrame {
    pub id: u64,
    pub target: DrillTarget,
    pub data: FrameData,
    /// Selected row per sub-panel (list screens use index 0).
    pub selected: [usize; 3],
    /// Sort column and whether it sorts descending.
    pub sort: Option<(usize, bool)>,
    /// The focused sub-panel on multi-panel screens.
    pub sub: usize,
}

impl DrillFrame {
    /// Number of selectable rows in the focused sub-panel.
    pub fn rows(&self) -> usize {
        match &self.data {
            FrameData::Ready(ScreenData::List(l)) => l.rows.len(),
            FrameData::Ready(ScreenData::Endpoint(e)) => match self.sub {
                0 => e.exceptions.len(),
                1 => e.dependencies.len(),
                _ => e.recent.len(),
            },
            _ => 0,
        }
    }

    /// Number of sub-panels Left/Right cycles through.
    pub fn subs(&self) -> usize {
        match self.target {
            DrillTarget::EndpointFailures { .. } => 3,
            _ => 1,
        }
    }

    /// Where Enter goes from the selected row.
    pub fn next_target(&self) -> Option<DrillTarget> {
        match &self.data {
            FrameData::Ready(ScreenData::List(l)) => {
                l.rows.get(self.selected[0]).and_then(|r| r.next.clone())
            }
            FrameData::Ready(ScreenData::Endpoint(e)) => match self.sub {
                0 => {
                    e.exceptions
                        .get(self.selected[0])
                        .map(|x| DrillTarget::ExceptionOccurrences {
                            browser: false,
                            exception_type: x.name.clone(),
                        })
                }
                1 => e
                    .dependencies
                    .get(self.selected[1])
                    .map(|d| DrillTarget::DependencyCalls {
                        dep_type: d.dep_type.clone(),
                        target: d.target.clone(),
                    }),
                2 => e
                    .recent
                    .get(self.selected[2])
                    .map(|r| DrillTarget::RequestDetail {
                        operation_id: r.operation_id.clone(),
                        request_id: Some(r.request_id.clone()).filter(|id| !id.is_empty()),
                    }),
                _ => None,
            },
            _ => None,
        }
    }
}

const PAGE: usize = 10;

#[derive(Clone, Debug)]
pub enum DataRequest {
    Refresh {
        instance: AppInstance,
        range: TimeRange,
        view: View,
    },
    Discover,
    Drill {
        frame_id: u64,
        instance: AppInstance,
        range: TimeRange,
        target: DrillTarget,
    },
}

/// Each event moves through the channel exactly once, so the size
/// difference between variants costs nothing worth a heap allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DataEvent {
    Dashboard(DashboardUpdate),
    Instances(Result<Vec<AppInstance>, AzError>),
    Drill {
        frame_id: u64,
        result: Result<ScreenData, AzError>,
    },
}

#[derive(Clone, Debug)]
pub enum Command {
    Fetch(DataRequest),
    SaveConfig(Config),
    Copy(String),
    OpenUrl(String),
    Quit,
}

/// The instance picker's state.
#[derive(Clone, Debug)]
pub struct PickerState {
    pub query: String,
    /// `None` while discovery runs; `Some(Err)` shows the error.
    pub all: Option<Result<Vec<AppInstance>, String>>,
    pub selected: usize,
    pub can_cancel: bool,
    /// An `--app` value waiting for discovery to resolve it.
    pub pending_app: Option<String>,
}

impl PickerState {
    pub fn filtered(&self) -> Vec<&AppInstance> {
        match &self.all {
            Some(Ok(all)) => all
                .iter()
                .filter(|i| instance_matches(i, &self.query))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Case-insensitive subsequence match of `query` (spaces ignored) against
/// "name resource_group subscription_name".
pub fn instance_matches(inst: &AppInstance, query: &str) -> bool {
    let hay = format!(
        "{} {} {}",
        inst.name, inst.resource_group, inst.subscription_name
    )
    .to_lowercase();
    let mut hay = hay.chars();
    query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|q| hay.any(|h| h == q))
}

pub enum Resolution {
    One(AppInstance),
    Many(Vec<AppInstance>),
    None,
}

/// Resolves an `--app` value: a full resource ID, or a name.
pub fn resolve_app(arg: &str, all: &[AppInstance]) -> Resolution {
    let arg = arg.trim();
    let by_id = arg.to_lowercase().starts_with("/subscriptions/");
    let mut found: Vec<AppInstance> = all
        .iter()
        .filter(|i| {
            if by_id {
                i.resource_id.eq_ignore_ascii_case(arg)
            } else {
                i.name.eq_ignore_ascii_case(arg)
            }
        })
        .cloned()
        .collect();
    match found.len() {
        0 => Resolution::None,
        1 => Resolution::One(found.remove(0)),
        _ => Resolution::Many(found),
    }
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
    /// Set when the user switched to Failures while an Overview refresh was
    /// running: refetch as soon as it lands so the top lists appear.
    refetch_after_in_flight: bool,
    pub picker: Option<PickerState>,
    /// Open drill-down screens, innermost last.
    pub drill: Vec<DrillFrame>,
    next_frame_id: u64,
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
            refetch_after_in_flight: false,
            picker: None,
            drill: Vec::new(),
            next_frame_id: 1,
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
    /// Opens the instance picker and starts discovery.
    pub fn open_picker(&mut self, can_cancel: bool, pending_app: Option<String>) -> Vec<Command> {
        self.picker = Some(PickerState {
            query: String::new(),
            all: None,
            selected: 0,
            can_cancel,
            pending_app,
        });
        vec![Command::Fetch(DataRequest::Discover)]
    }

    fn select_instance(&mut self, inst: AppInstance, now: Instant) -> Vec<Command> {
        self.config.remember(&inst);
        self.drill.clear();
        self.instance = Some(inst);
        self.picker = None;
        self.auth_problem = None;
        let mut cmds = vec![Command::SaveConfig(self.config.clone())];
        cmds.extend(self.reset_data(now));
        cmds
    }

    fn on_instances(
        &mut self,
        result: Result<Vec<AppInstance>, AzError>,
        now: Instant,
    ) -> Vec<Command> {
        let Some(picker) = self.picker.as_mut() else {
            return Vec::new();
        };
        let result = result.map_err(|e| e.to_string());
        let pending = picker.pending_app.take();
        if let (Some(arg), Ok(all)) = (&pending, &result) {
            match resolve_app(arg, all) {
                Resolution::One(inst) => return self.select_instance(inst, now),
                Resolution::Many(_) => picker.query = arg.clone(),
                Resolution::None => {
                    picker.query = arg.clone();
                    picker.all = Some(result);
                    self.toast(format!("no instance named {arg}"), now);
                    return Vec::new();
                }
            }
        }
        picker.all = Some(result);
        picker.selected = 0;
        Vec::new()
    }

    fn on_picker_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Command> {
        let Some(picker) = self.picker.as_mut() else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                if picker.can_cancel {
                    self.picker = None;
                } else {
                    return vec![Command::Quit];
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.all = None;
                return vec![Command::Fetch(DataRequest::Discover)];
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.query.push(c);
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                let n = picker.filtered().len();
                picker.selected = (picker.selected + 1).min(n.saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(inst) = picker.filtered().get(picker.selected).map(|i| (*i).clone()) {
                    return self.select_instance(inst, now);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn fetch_frame(&mut self, target: DrillTarget) -> Option<(u64, Command)> {
        let inst = self.instance.clone()?;
        let id = self.next_frame_id;
        self.next_frame_id += 1;
        Some((
            id,
            Command::Fetch(DataRequest::Drill {
                frame_id: id,
                instance: inst,
                range: self.range,
                target,
            }),
        ))
    }

    fn push_frame(&mut self, target: DrillTarget) -> Vec<Command> {
        match self.fetch_frame(target.clone()) {
            Some((id, cmd)) => {
                let sub = match target {
                    DrillTarget::EndpointFailures { .. } => 2,
                    _ => 0,
                };
                self.drill.push(DrillFrame {
                    id,
                    target,
                    data: FrameData::Loading,
                    selected: [0; 3],
                    sort: None,
                    sub,
                });
                vec![cmd]
            }
            None => Vec::new(),
        }
    }

    fn reload_frame(&mut self) -> Vec<Command> {
        let Some(target) = self.drill.last().map(|f| f.target.clone()) else {
            return Vec::new();
        };
        match self.fetch_frame(target) {
            Some((id, cmd)) => {
                if let Some(f) = self.drill.last_mut() {
                    f.id = id;
                    f.data = FrameData::Loading;
                }
                vec![cmd]
            }
            None => Vec::new(),
        }
    }

    fn cycle_sort(&mut self) {
        let Some(f) = self.drill.last_mut() else {
            return;
        };
        let FrameData::Ready(ScreenData::List(l)) = &mut f.data else {
            return;
        };
        if l.columns.is_empty() {
            return;
        }
        let col = (f.sort.map_or(0, |(c, _)| c) + 1) % l.columns.len();
        let is_text = l
            .rows
            .first()
            .and_then(|r| r.cells.get(col))
            .is_some_and(|c| matches!(c, Cell::Text(_)));
        sort_rows(&mut l.rows, col);
        f.sort = Some((col, !is_text));
        f.selected[f.sub] = 0;
    }

    /// Keys while a drill-down screen is open. `None` means "not a drill
    /// key": the dashboard handles it (quit, help, interval, range…).
    /// `c` copies the operation ID on a request screen; `o` opens the
    /// request's transaction in the portal's Logs, or the resource overview.
    fn drill_action(&self, c: char) -> Vec<Command> {
        let (Some(frame), Some(inst)) = (self.drill.last(), self.instance.as_ref()) else {
            return Vec::new();
        };
        match (c, &frame.target) {
            ('c', DrillTarget::RequestDetail { operation_id, .. }) => {
                vec![Command::Copy(operation_id.clone())]
            }
            ('o', DrillTarget::RequestDetail { operation_id, .. }) => {
                vec![Command::OpenUrl(crate::data::drill::portal_logs_link(
                    &inst.tenant_id,
                    &inst.resource_id,
                    &crate::data::kql::transaction(operation_id),
                    self.range.iso(),
                ))]
            }
            ('o', _) => vec![Command::OpenUrl(crate::data::drill::portal_resource_link(
                &inst.tenant_id,
                &inst.resource_id,
            ))],
            _ => Vec::new(),
        }
    }

    fn on_drill_key(&mut self, key: KeyEvent) -> Option<Vec<Command>> {
        if let KeyCode::Char(c @ ('c' | 'o')) = key.code {
            return Some(self.drill_action(c));
        }
        let f = self.drill.last_mut()?;
        let rows = f.rows();
        let sel = &mut f.selected[f.sub];
        match key.code {
            KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Down => *sel = (*sel + 1).min(rows.saturating_sub(1)),
            KeyCode::PageUp => *sel = sel.saturating_sub(PAGE),
            KeyCode::PageDown => *sel = (*sel + PAGE).min(rows.saturating_sub(1)),
            KeyCode::Home => *sel = 0,
            KeyCode::End => *sel = rows.saturating_sub(1),
            KeyCode::Enter => {
                return Some(match f.next_target() {
                    Some(t) => self.push_frame(t),
                    None => Vec::new(),
                });
            }
            KeyCode::Esc => {
                self.drill.pop();
            }
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('r') => return Some(self.reload_frame()),
            KeyCode::Char(c) if c.is_ascii_digit() => {}
            KeyCode::Right => f.sub = (f.sub + 1) % f.subs(),
            KeyCode::Left => f.sub = (f.sub + f.subs() - 1) % f.subs(),
            _ => return None,
        }
        Some(Vec::new())
    }

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
                if std::mem::take(&mut self.refetch_after_in_flight) {
                    return self.fetch_now();
                }
                Vec::new()
            }
            DataEvent::Instances(result) => self.on_instances(result, now),
            DataEvent::Drill { frame_id, result } => {
                if let Some(f) = self.drill.iter_mut().find(|f| f.id == frame_id) {
                    f.data = match result {
                        Ok(d) => FrameData::Ready(d),
                        Err(e) => FrameData::Failed(e.to_string()),
                    };
                }
                Vec::new()
            }
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
        if self.picker.is_some() {
            return self.on_picker_key(key, now);
        }
        if self.help && key.code == KeyCode::Esc {
            self.help = false;
            return Vec::new();
        }
        if !self.drill.is_empty() {
            if let Some(cmds) = self.on_drill_key(key) {
                return cmds;
            }
            if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
                self.drill.clear();
            }
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.view = match self.view {
                    View::Overview => View::Failures,
                    View::Failures => View::Overview,
                };
                self.focus = None;
                if self.view == View::Failures {
                    if self.refreshing {
                        self.refetch_after_in_flight = true;
                    }
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
            KeyCode::Enter => {
                if let Some(t) = self.focus.and_then(drill_target) {
                    return self.push_frame(t);
                }
            }
            KeyCode::Char('t') => {
                self.drill.clear();
                self.range = self.range.next();
                self.config.range = self.range;
                let mut cmds = vec![Command::SaveConfig(self.config.clone())];
                cmds.extend(self.reset_data(now));
                return cmds;
            }
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Char('q') => return vec![Command::Quit],
            KeyCode::Char('i') => return self.open_picker(self.instance.is_some(), None),
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

    fn two_with_same_name() -> Vec<AppInstance> {
        let mut all = demo_instances();
        let mut dup = all[0].clone();
        dup.subscription_name = "Other Subscription".into();
        dup.resource_id = dup.resource_id.replace("demo-rg", "other-rg");
        all.push(dup);
        all
    }

    #[test]
    fn resolve_by_name_case_insensitive() {
        assert!(matches!(
            resolve_app("DEMO-APP-STAGING", &demo_instances()),
            Resolution::One(i) if i.name == "demo-app-staging"
        ));
    }

    #[test]
    fn resolve_by_resource_id() {
        let all = demo_instances();
        let id = all[1].resource_id.to_uppercase();
        assert!(
            matches!(resolve_app(&id, &all), Resolution::One(i) if i.name == "demo-app-staging")
        );
    }

    #[test]
    fn resolve_duplicate_names_is_many() {
        assert!(matches!(
            resolve_app("demo-app-prod", &two_with_same_name()),
            Resolution::Many(v) if v.len() == 2
        ));
    }

    #[test]
    fn resolve_unknown_is_none() {
        assert!(matches!(
            resolve_app("nope", &demo_instances()),
            Resolution::None
        ));
    }

    #[test]
    fn matches_subsequence() {
        let i = demo_instances().remove(0);
        assert!(instance_matches(&i, "dap"));
        assert!(instance_matches(&i, ""));
        assert!(instance_matches(&i, "DEMO sub"));
        assert!(!instance_matches(&i, "xyz"));
    }

    fn picker_app(can_cancel: bool) -> App {
        let mut a = if can_cancel {
            app()
        } else {
            App::new(Config::default(), None, TimeRange::H4, 30)
        };
        let cmds = a.open_picker(can_cancel, None);
        assert!(matches!(
            cmds.as_slice(),
            [Command::Fetch(DataRequest::Discover)]
        ));
        a.on_data(
            DataEvent::Instances(Ok(demo_instances())),
            Instant::now(),
            wall(),
        );
        a
    }

    #[test]
    fn typing_filters_and_enter_selects() {
        let mut a = picker_app(false);
        let now = Instant::now();
        for c in "stag".chars() {
            a.on_key(ch(c), now);
        }
        let picker = a.picker.as_ref().unwrap();
        assert_eq!(picker.filtered().len(), 1);
        let cmds = a.on_key(k(KeyCode::Enter), now);
        assert!(a.picker.is_none());
        assert_eq!(a.instance.as_ref().unwrap().name, "demo-app-staging");
        assert!(cmds.iter().any(|c| matches!(c, Command::SaveConfig(cfg)
            if cfg.last_instance.as_ref().is_some_and(|l| l.name == "demo-app-staging"))));
        assert_eq!(fetches(&cmds), 1);
    }

    #[test]
    fn backspace_and_arrows_in_picker() {
        let mut a = picker_app(false);
        let now = Instant::now();
        a.on_key(ch('x'), now);
        assert!(a.picker.as_ref().unwrap().filtered().is_empty());
        a.on_key(k(KeyCode::Backspace), now);
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.picker.as_ref().unwrap().selected, 1);
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.picker.as_ref().unwrap().selected, 1, "clamped");
        a.on_key(k(KeyCode::Enter), now);
        assert_eq!(a.instance.as_ref().unwrap().name, "demo-app-staging");
    }

    #[test]
    fn esc_without_instance_quits() {
        let mut a = picker_app(false);
        assert!(matches!(
            a.on_key(k(KeyCode::Esc), Instant::now()).as_slice(),
            [Command::Quit]
        ));
    }

    #[test]
    fn esc_with_instance_closes() {
        let mut a = picker_app(true);
        assert!(a.on_key(k(KeyCode::Esc), Instant::now()).is_empty());
        assert!(a.picker.is_none());
        assert!(a.instance.is_some());
    }

    #[test]
    fn i_opens_picker_and_discovers() {
        let mut a = app();
        let cmds = a.on_key(ch('i'), Instant::now());
        assert!(matches!(
            cmds.as_slice(),
            [Command::Fetch(DataRequest::Discover)]
        ));
        assert!(a.picker.as_ref().unwrap().can_cancel);
        assert!(a.picker.as_ref().unwrap().all.is_none(), "loading");
    }

    #[test]
    fn q_in_picker_types_not_quits() {
        let mut a = picker_app(true);
        assert!(a.on_key(ch('q'), Instant::now()).is_empty());
        assert_eq!(a.picker.as_ref().unwrap().query, "q");
    }

    #[test]
    fn ctrl_r_rediscovers() {
        let mut a = picker_app(true);
        let cmds = a.on_key(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        assert!(matches!(
            cmds.as_slice(),
            [Command::Fetch(DataRequest::Discover)]
        ));
        assert!(a.picker.as_ref().unwrap().all.is_none());
    }

    #[test]
    fn pending_app_one_match_selects_without_picker() {
        let mut a = App::new(Config::default(), None, TimeRange::H4, 30);
        a.open_picker(false, Some("demo-app-staging".into()));
        let cmds = a.on_data(
            DataEvent::Instances(Ok(demo_instances())),
            Instant::now(),
            wall(),
        );
        assert!(a.picker.is_none());
        assert_eq!(a.instance.as_ref().unwrap().name, "demo-app-staging");
        assert_eq!(fetches(&cmds), 1);
    }

    #[test]
    fn pending_app_many_or_none_keeps_picker() {
        let mut a = App::new(Config::default(), None, TimeRange::H4, 30);
        a.open_picker(false, Some("demo-app-prod".into()));
        a.on_data(
            DataEvent::Instances(Ok(two_with_same_name())),
            Instant::now(),
            wall(),
        );
        assert_eq!(a.picker.as_ref().unwrap().query, "demo-app-prod");
        assert!(a.instance.is_none());

        let mut b = App::new(Config::default(), None, TimeRange::H4, 30);
        b.open_picker(false, Some("nope".into()));
        b.on_data(
            DataEvent::Instances(Ok(demo_instances())),
            Instant::now(),
            wall(),
        );
        assert!(b.picker.is_some());
        assert!(
            b.toast
                .as_ref()
                .unwrap()
                .0
                .contains("no instance named nope")
        );
    }

    #[test]
    fn switching_instance_discards_in_flight_refresh() {
        let mut a = picker_app(true);
        let t0 = Instant::now();
        a.on_tick(t0);
        assert!(a.refreshing);
        a.on_key(ch('s'), t0);
        a.on_key(k(KeyCode::Enter), t0);
        let cmds = a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        assert!(!a.store.has_data(SeriesId::RequestsFailed));
        assert_eq!(fetches(&cmds), 1);
    }

    fn drill_requests(cmds: &[Command]) -> Vec<(u64, DrillTarget)> {
        cmds.iter()
            .filter_map(|c| match c {
                Command::Fetch(DataRequest::Drill {
                    frame_id, target, ..
                }) => Some((*frame_id, target.clone())),
                _ => None,
            })
            .collect()
    }

    fn list_frame() -> ScreenData {
        ScreenData::List(crate::data::drill::failing_endpoints_list(
            &crate::data::drill::tests::endpoints_table(),
            TimeRange::H4,
        ))
    }

    fn drilled() -> App {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('2'), now);
        let cmds = a.on_key(k(KeyCode::Enter), now);
        let (id, _) = drill_requests(&cmds)[0].clone();
        a.on_data(
            DataEvent::Drill {
                frame_id: id,
                result: Ok(list_frame()),
            },
            now,
            wall(),
        );
        a
    }

    #[test]
    fn enter_on_failed_panel_pushes_loading_frame_and_fetches() {
        let mut a = app();
        let now = Instant::now();
        assert!(
            a.on_key(k(KeyCode::Enter), now).is_empty(),
            "no focus: nothing"
        );
        a.on_key(ch('2'), now);
        let cmds = a.on_key(k(KeyCode::Enter), now);
        let reqs = drill_requests(&cmds);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].1, DrillTarget::FailingEndpoints);
        assert_eq!(a.drill.len(), 1);
        assert!(matches!(a.drill[0].data, FrameData::Loading));
        a.on_key(ch('1'), now);
        assert_eq!(
            a.focus,
            Some(PanelId::FailedRequests),
            "digits do not change focus in a drill"
        );
    }

    #[test]
    fn panels_without_drill_do_nothing() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('1'), now);
        assert!(a.on_key(k(KeyCode::Enter), now).is_empty());
        assert!(a.drill.is_empty());
    }

    #[test]
    fn esc_pops_back_to_dashboard_keeping_focus() {
        let mut a = drilled();
        a.on_key(k(KeyCode::Esc), Instant::now());
        assert!(a.drill.is_empty());
        assert_eq!(a.focus, Some(PanelId::FailedRequests));
    }

    #[test]
    fn stale_drill_result_ignored() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('2'), now);
        let id = drill_requests(&a.on_key(k(KeyCode::Enter), now))[0].0;
        a.on_key(k(KeyCode::Esc), now);
        a.on_data(
            DataEvent::Drill {
                frame_id: id,
                result: Ok(list_frame()),
            },
            now,
            wall(),
        );
        assert!(a.drill.is_empty());
    }

    #[test]
    fn failed_load_shows_error_and_r_reloads() {
        let mut a = app();
        let now = Instant::now();
        a.on_key(ch('2'), now);
        let id = drill_requests(&a.on_key(k(KeyCode::Enter), now))[0].0;
        a.on_data(
            DataEvent::Drill {
                frame_id: id,
                result: Err(AzError::Network("down".into())),
            },
            now,
            wall(),
        );
        assert!(matches!(&a.drill[0].data, FrameData::Failed(m) if m.contains("down")));
        let reqs = drill_requests(&a.on_key(ch('r'), now));
        assert_eq!(reqs.len(), 1);
        assert!(matches!(a.drill[0].data, FrameData::Loading));
        assert_ne!(reqs[0].0, id, "reload gets a new frame id");
    }

    #[test]
    fn navigation_clamps_and_enter_opens_row() {
        let mut a = drilled();
        let now = Instant::now();
        a.on_key(k(KeyCode::Up), now);
        assert_eq!(a.drill[0].selected[0], 0);
        a.on_key(k(KeyCode::End), now);
        assert_eq!(a.drill[0].selected[0], 3);
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.drill[0].selected[0], 3);
        a.on_key(k(KeyCode::Home), now);
        let reqs = drill_requests(&a.on_key(k(KeyCode::Enter), now));
        assert_eq!(
            reqs[0].1,
            DrillTarget::EndpointFailures {
                name: "POST /api/orders".into()
            }
        );
        assert_eq!(a.drill.len(), 2);
    }

    #[test]
    fn sort_cycles_columns() {
        let mut a = drilled();
        let now = Instant::now();
        a.on_key(ch('s'), now);
        assert_eq!(a.drill[0].sort, Some((1, true)));
        a.on_key(ch('s'), now);
        assert_eq!(a.drill[0].sort, Some((2, true)));
        a.on_key(ch('s'), now);
        a.on_key(ch('s'), now);
        assert_eq!(a.drill[0].sort, Some((0, false)), "text sorts ascending");
        let FrameData::Ready(ScreenData::List(l)) = &a.drill[0].data else {
            panic!()
        };
        assert_eq!(
            l.rows[0].cells[0],
            crate::data::drill::Cell::Text("GET /api/reports/export".into())
        );
    }

    #[test]
    fn tab_in_drill_clears_and_switches_view() {
        let mut a = drilled();
        a.on_key(k(KeyCode::Tab), Instant::now());
        assert!(a.drill.is_empty());
        assert_eq!(a.view, View::Failures);
    }

    #[test]
    fn range_change_in_drill_returns_to_dashboard() {
        let mut a = drilled();
        a.on_key(ch('t'), Instant::now());
        assert!(a.drill.is_empty());
        assert_eq!(a.range, TimeRange::H12);
    }

    #[test]
    fn drill_targets_for_panels() {
        assert_eq!(
            drill_target(PanelId::FailedRequests),
            Some(DrillTarget::FailingEndpoints)
        );
        assert_eq!(
            drill_target(PanelId::FailuresMain),
            Some(DrillTarget::FailingEndpoints)
        );
        assert_eq!(drill_target(PanelId::Health), None);
    }

    fn endpoint_frame(a: &mut App) {
        let (e, d, r) = crate::data::drill::tests::endpoint_tables();
        let data = crate::data::drill::endpoint_data("POST /api/orders", &e, &d, &r);
        let cmds = a.on_key(k(KeyCode::Enter), Instant::now());
        let id = drill_requests(&cmds)[0].0;
        a.on_data(
            DataEvent::Drill {
                frame_id: id,
                result: Ok(ScreenData::Endpoint(data)),
            },
            Instant::now(),
            wall(),
        );
    }

    #[test]
    fn endpoint_screen_opens_on_recent_failures() {
        let mut a = drilled();
        endpoint_frame(&mut a);
        assert_eq!(a.drill.len(), 2);
        assert_eq!(a.drill[1].sub, 2);
    }

    #[test]
    fn left_right_switch_sub_panels() {
        let mut a = drilled();
        endpoint_frame(&mut a);
        let now = Instant::now();
        a.on_key(k(KeyCode::Right), now);
        assert_eq!(a.drill[1].sub, 0, "wraps");
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.drill[1].selected, [1, 0, 0]);
        a.on_key(k(KeyCode::Left), now);
        a.on_key(k(KeyCode::Left), now);
        assert_eq!(a.drill[1].sub, 1);
        a.on_key(k(KeyCode::Down), now);
        assert_eq!(a.drill[1].selected[1], 0, "only one dependency row");
    }

    #[test]
    fn enter_on_recent_opens_request_detail() {
        let mut a = drilled();
        endpoint_frame(&mut a);
        let now = Instant::now();
        a.on_key(k(KeyCode::Down), now);
        let reqs = drill_requests(&a.on_key(k(KeyCode::Enter), now));
        assert_eq!(
            reqs[0].1,
            DrillTarget::RequestDetail {
                operation_id: "op-2".into(),
                request_id: Some("req-2".into())
            }
        );
    }

    fn request_app() -> App {
        let mut a = drilled();
        endpoint_frame(&mut a);
        let now = Instant::now();
        let cmds = a.on_key(k(KeyCode::Enter), now);
        let id = drill_requests(&cmds)[0].0;
        let data = crate::data::drill::RequestData {
            request: None,
            exception: None,
            transaction: vec![],
        };
        a.on_data(
            DataEvent::Drill {
                frame_id: id,
                result: Ok(ScreenData::Request(data)),
            },
            now,
            wall(),
        );
        a
    }

    #[test]
    fn c_copies_operation_id() {
        let mut a = request_app();
        let cmds = a.on_key(ch('c'), Instant::now());
        assert!(matches!(cmds.as_slice(), [Command::Copy(op)] if op == "op-1"));
    }

    #[test]
    fn o_opens_logs_link_on_request_and_resource_link_elsewhere() {
        let mut a = request_app();
        let cmds = a.on_key(ch('o'), Instant::now());
        assert!(
            matches!(cmds.as_slice(), [Command::OpenUrl(u)] if u.contains("Logs.ReactView") && u.contains("/timespan/PT4H"))
        );
        a.on_key(k(KeyCode::Esc), Instant::now());
        let cmds = a.on_key(ch('o'), Instant::now());
        assert!(matches!(cmds.as_slice(), [Command::OpenUrl(u)] if u.ends_with("/overview")));
        assert!(
            a.on_key(ch('c'), Instant::now()).is_empty(),
            "c only on request detail"
        );
    }

    #[test]
    fn tab_to_failures_during_refresh_refetches_when_done() {
        let mut a = app();
        let t0 = Instant::now();
        a.on_tick(t0);
        assert_eq!(
            fetches(&a.on_key(k(KeyCode::Tab), t0)),
            0,
            "a refresh is running"
        );
        let cmds = a.on_data(DataEvent::Dashboard(good_update()), t0, wall());
        let views: Vec<View> = cmds
            .iter()
            .filter_map(|c| match c {
                Command::Fetch(DataRequest::Refresh { view, .. }) => Some(*view),
                _ => None,
            })
            .collect();
        assert_eq!(views, [View::Failures], "refetches for the failures lists");
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
