//! Shared exporter entry point.
pub fn run(config: plows_exporter::config::Config) -> anyhow::Result<()> {
    plows_exporter::server::run(config)
}
