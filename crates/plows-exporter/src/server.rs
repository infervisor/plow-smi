use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::{routing::get, Router};
use tracing::{error, info, warn};

// Use the library crate so all Lazy statics are shared
#[cfg(any(feature = "gpu", feature = "system"))]
use crate::collector;
use crate::collector::manager::CollectorManager;
use crate::config::Config;
use crate::exporter::prometheus::{health_handler, metrics_handler};
use crate::metrics::gpu_metrics::update_gpu_metrics;

#[cfg(feature = "system")]
use crate::collector::system::SystemCollector;
#[cfg(feature = "system")]
use crate::metrics::system_metrics::update_system_metrics;

#[cfg(feature = "tpu")]
use crate::collector::tpu::TpuCollector;

use crate::collector::network::InterfaceFilter;
use crate::collector::wireguard::{WgCommand, WgState};
use crate::metrics::network_metrics::NetworkMetrics;
use crate::metrics::registry::REGISTRY;
use crate::metrics::wireguard_metrics::WireGuardMetrics;

/// Shared server implementation for both exporter entry points.
pub fn run(config: Config) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()?
        .block_on(serve(config))
}

async fn serve(config: Config) -> anyhow::Result<()> {
    anyhow::ensure!(config.interval > 0, "collection interval must be positive");
    #[cfg(not(feature = "tpu"))]
    anyhow::ensure!(
        !config.tpu,
        "TPU support requires building with --features tpu"
    );
    #[cfg(not(feature = "system"))]
    anyhow::ensure!(
        !config.system,
        "system support requires building with --features system"
    );
    #[cfg(not(feature = "gpu"))]
    anyhow::ensure!(
        !config.gpu_enabled() || config.all,
        "GPU support requires building with --features gpu"
    );
    let addr = SocketAddr::new(config.bind.parse()?, config.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(version = env!("CARGO_PKG_VERSION"), "Starting GPU Exporter");

    // Validate configuration
    if config.no_collectors_enabled() {
        warn!("No collectors enabled! Use --nvidia, --amd, --intel, --system, --tpu, or --all");
        warn!("Run with --help for usage information");
        info!("Defaulting to --all");
    }

    // Build collector manager
    #[allow(unused_mut)] // Mutated only when GPU/TPU features are enabled.
    let mut manager = CollectorManager::new();

    // Register unified GPU collector (plows-gpu: NVIDIA / AMD / Intel)
    #[cfg(feature = "gpu")]
    {
        let default_all = config.no_collectors_enabled();
        let filter = collector::gpu::VendorFilter {
            nvidia: config.nvidia_enabled() || default_all,
            amd: config.amd_enabled() || default_all,
            intel: config.intel_enabled() || default_all,
            apple: config.apple_enabled() || default_all,
        };
        if filter.any() {
            info!(
                "Registering plows-gpu collector (nvidia={} amd={} intel={} apple={})",
                filter.nvidia, filter.amd, filter.intel, filter.apple
            );
            let gpu = collector::gpu::GpuCollector::new(filter);
            if let Err(e) = manager.register(Box::new(gpu)).await {
                warn!(error = %e, "GPU collector failed to initialize (continuing without it)");
            }
        }
    }

    // Register TPU collector
    #[cfg(feature = "tpu")]
    if config.tpu_enabled() || config.no_collectors_enabled() {
        info!("Registering Google Cloud TPU collector...");
        let tpu = TpuCollector::new();
        if let Err(e) = manager.register(Box::new(tpu)).await {
            warn!(error = %e, "TPU collector failed to initialize (continuing without it)");
        }
    }

    // Register System collector
    #[cfg(feature = "system")]
    let system_collector = if config.system_enabled() || config.no_collectors_enabled() {
        info!("Registering System collector...");
        let mut sys = SystemCollector::new()?;
        match collector::Collector::init(&mut sys).await {
            Ok(_) => {
                info!("System collector initialized");
                Some(sys)
            }
            Err(e) => {
                warn!(error = %e, "System collector failed to initialize");
                None
            }
        }
    } else {
        None
    };

    let interval = Duration::from_secs(config.interval);
    let manager = Arc::new(manager);

    // Spawn GPU metrics collection loop
    if manager.collector_count() > 0 {
        let manager_clone = Arc::clone(&manager);
        tokio::spawn(async move {
            info!("GPU metrics collection loop started");
            loop {
                let manager = Arc::clone(&manager_clone);
                let runtime = tokio::runtime::Handle::current();
                match tokio::task::spawn_blocking(move || {
                    let snapshots = runtime.block_on(manager.collect_all());
                    update_gpu_metrics(&snapshots);
                })
                .await
                {
                    Ok(()) => {}
                    Err(e) => error!(error = %e, "GPU collection task failed"),
                }
                tokio::time::sleep(interval).await;
            }
        });
    } else {
        warn!("No GPU collectors active — only system metrics will be available");
    }

    // Spawn system metrics collection loop
    #[cfg(feature = "system")]
    if let Some(sys_collector) = system_collector {
        let sys_interval = interval;
        let sys_collector = Arc::new(sys_collector);
        tokio::spawn(async move {
            info!("System metrics collection loop started");
            loop {
                let sys = Arc::clone(&sys_collector);
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    update_system_metrics(&sys.collect_system_snapshot());
                })
                .await
                {
                    error!(error = %e, "System collection task failed");
                }
                tokio::time::sleep(sys_interval).await;
            }
        });
    }

    start_network_collectors(&config, interval)?;

    // Build HTTP server
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .route("/health", get(health_handler))
        .route("/healthz", get(health_handler));

    info!(%addr, "HTTP server starting");
    info!("Prometheus metrics available at http://{}/metrics", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| {
            error!(error = %e, "HTTP server error");
            anyhow::anyhow!("Server error: {e}")
        })?;

    info!("Shutting down gracefully...");

    Ok(())
}

/// Register the network, WireGuard and probe collectors and start their
/// samplers. Interfaces are read at scrape time; `wg` and the prober run on
/// their own threads so a slow `wg` or a probe round never holds one of the
/// runtime's blocking workers.
fn start_network_collectors(config: &Config, interval: Duration) -> anyhow::Result<()> {
    #[cfg(not(feature = "probe"))]
    anyhow::ensure!(
        !config.probe_enabled(),
        "probes require building with --features probe"
    );
    let default_all = config.no_collectors_enabled();
    let hostname = whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string());

    if config.network_enabled() || default_all {
        let filter = InterfaceFilter::new(config.network_include.clone());
        REGISTRY.register(Box::new(NetworkMetrics::new(hostname.clone(), filter)))?;
        info!("Network interface collector registered");
    }

    #[allow(unused_variables)] // Read only by the probe feature.
    let wg_state = if config.wireguard_enabled() || default_all {
        match WgCommand::locate() {
            Some(wg) => {
                info!(binary = %wg.binary().display(), "WireGuard collector registered");
                let state = Arc::new(RwLock::new(WgState::default()));
                REGISTRY.register(Box::new(WireGuardMetrics::new(
                    hostname.clone(),
                    Arc::clone(&state),
                )))?;
                let shared = Arc::clone(&state);
                std::thread::Builder::new()
                    .name("plows-wireguard".into())
                    .spawn(move || sample_wireguard(&wg, &shared, interval))?;
                Some(state)
            }
            None => {
                warn!("WireGuard metrics disabled: `wg` (wireguard-tools) not found");
                None
            }
        }
    } else {
        None
    };

    #[cfg(feature = "probe")]
    if config.probe_enabled() {
        use crate::collector::probe::{self, ProbeConfig, ProbeState};
        use crate::metrics::probe_metrics::ProbeMetrics;
        let state = Arc::new(RwLock::new(ProbeState::new()));
        REGISTRY.register(Box::new(ProbeMetrics::new(hostname, Arc::clone(&state))))?;
        let cfg = ProbeConfig {
            targets: config.probe.clone(),
            wireguard_peers: config.probe_wireguard_peers,
            interval: Duration::from_millis(config.probe_interval_ms),
        };
        info!(
            targets = cfg.targets.len(),
            wireguard_peers = cfg.wireguard_peers,
            "Probe collector registered"
        );
        std::thread::Builder::new()
            .name("plows-probe".into())
            .spawn(move || probe::run(cfg, wg_state, state))?;
    }
    Ok(())
}

/// Read `wg` every interval. A failure is logged when it starts or changes,
/// not on every read, and is visible as `wireguard_read_success 0`.
fn sample_wireguard(wg: &WgCommand, state: &RwLock<WgState>, interval: Duration) {
    let mut last_error: Option<String> = None;
    loop {
        let next = match wg.read() {
            Ok(interfaces) => {
                if last_error.take().is_some() {
                    info!("WireGuard read recovered");
                }
                WgState {
                    ok: true,
                    interfaces,
                }
            }
            Err(e) => {
                if last_error.as_deref() != Some(e.as_str()) {
                    warn!(error = %e, "WireGuard read failed (needs CAP_NET_ADMIN)");
                }
                last_error = Some(e);
                WgState::default()
            }
        };
        *state.write().unwrap_or_else(|e| e.into_inner()) = next;
        std::thread::sleep(interval);
    }
}

/// Wait for SIGINT or SIGTERM for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C"),
        _ = terminate => info!("Received SIGTERM"),
    }
}
