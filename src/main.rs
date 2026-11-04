use std::sync::Arc;

use appi_dashboard::azure::auth::{AzCliTokenSource, StaticTokenSource, TokenSource};
use appi_dashboard::azure::http::{AzureHttp, Endpoints};
use appi_dashboard::cli::{Cli, Cmd};
use appi_dashboard::data::backend::{AzureBackend, Backend};
use appi_dashboard::data::demo::DemoBackend;
use appi_dashboard::data::time::TimeRange;
use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::Probe { ref app, range }) => {
            let (tokens, backend): (Arc<dyn TokenSource>, Box<dyn Backend>) = if cli.demo {
                (
                    Arc::new(StaticTokenSource("demo".into())),
                    Box::new(DemoBackend::new()),
                )
            } else {
                let tokens: Arc<dyn TokenSource> = Arc::new(AzCliTokenSource::new());
                let http = AzureHttp::new(tokens.clone(), Endpoints::default());
                (tokens, Box::new(AzureBackend::new(http)))
            };
            let range = range.or(cli.range).unwrap_or(TimeRange::H4);
            let mut stdout = std::io::stdout();
            match appi_dashboard::probe::run_probe(
                tokens.as_ref(),
                backend.as_ref(),
                app.as_deref().or(cli.app.as_deref()),
                range,
                &mut stdout,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => std::process::exit(1),
                Err(e) => {
                    eprintln!("probe: {e}");
                    std::process::exit(1);
                }
            }
        }
        None => println!("appi-dashboard {}", env!("CARGO_PKG_VERSION")),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
