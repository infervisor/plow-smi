use clap::Parser;
use plows_exporter::config::Config;

fn main() -> anyhow::Result<()> {
    let config = Config::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| config.log_level.clone().into()),
        )
        .init();
    plows_exporter::server::run(config)
}
