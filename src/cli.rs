//! Command-line flags.

use std::path::PathBuf;

use crate::config::ColorMode;
use crate::data::time::{REFRESH_STEPS, TimeRange};

#[derive(clap::Parser, Debug)]
#[command(
    name = "appi-dashboard",
    version,
    about = "A btop-style terminal dashboard for Azure Application Insights"
)]
pub struct Cli {
    /// App Insights instance name or full resource ID
    #[arg(long)]
    pub app: Option<String>,
    /// Time range: 30m, 1h, 4h, 12h or 24h
    #[arg(long, value_parser = parse_range)]
    pub range: Option<TimeRange>,
    /// Refresh interval in seconds: 15, 30, 60, 120 or 300
    #[arg(long, value_parser = parse_refresh)]
    pub refresh: Option<u64>,
    /// Config file path (default: the per-user config directory)
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Show made-up demo data instead of querying Azure
    #[arg(long)]
    pub demo: bool,
    /// Color depth
    #[arg(long, value_enum)]
    pub color: Option<ColorMode>,
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// Check Azure access and print what the dashboard would fetch
    Probe {
        #[arg(long)]
        app: Option<String>,
        #[arg(long, value_parser = parse_range)]
        range: Option<TimeRange>,
    },
}

pub fn parse_range(s: &str) -> Result<TimeRange, String> {
    TimeRange::parse(s).ok_or_else(|| "must be one of 30m, 1h, 4h, 12h, 24h".to_string())
}

pub fn parse_refresh(s: &str) -> Result<u64, String> {
    let allowed = REFRESH_STEPS
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    s.trim()
        .parse::<u64>()
        .ok()
        .filter(|v| REFRESH_STEPS.contains(v))
        .ok_or_else(|| format!("must be one of {allowed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_flags() {
        let c = Cli::try_parse_from([
            "appi-dashboard",
            "--range",
            "12h",
            "--refresh",
            "60",
            "--app",
            "demo-app-prod",
            "--demo",
            "--color",
            "256",
        ])
        .unwrap();
        assert_eq!(c.range, Some(TimeRange::H12));
        assert_eq!(c.refresh, Some(60));
        assert_eq!(c.app.as_deref(), Some("demo-app-prod"));
        assert!(c.demo);
        assert_eq!(c.color, Some(ColorMode::Ansi256));
        assert!(c.command.is_none());
    }

    #[test]
    fn rejects_unknown_refresh() {
        let e = Cli::try_parse_from(["appi-dashboard", "--refresh", "45"]).unwrap_err();
        assert!(e.to_string().contains("15, 30, 60, 120, 300"), "{e}");
    }

    #[test]
    fn rejects_unknown_range() {
        assert!(Cli::try_parse_from(["appi-dashboard", "--range", "2h"]).is_err());
    }

    #[test]
    fn probe_subcommand() {
        let c = Cli::try_parse_from(["appi-dashboard", "probe", "--app", "x", "--range", "1h"])
            .unwrap();
        match c.command {
            Some(Cmd::Probe { app, range }) => {
                assert_eq!(app.as_deref(), Some("x"));
                assert_eq!(range, Some(TimeRange::H1));
            }
            other => panic!("{other:?}"),
        }
    }
}
