use clap::Parser;

fn main() {
    let _cli = appi_dashboard::cli::Cli::parse();
    println!("appi-dashboard {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
    }
}
