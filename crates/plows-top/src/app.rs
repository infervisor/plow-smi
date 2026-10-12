//! Application state management — rich state for htop-like display.

use std::collections::VecDeque;

use sysinfo::{Pid, ProcessesToUpdate, System};

use plows_exporter::collector::GpuSnapshot;
use plows_exporter::collector::system::SystemSnapshot;

use crate::gpu::GpuCollector;
use crate::system::SystemCollector;

/// Maximum history length for sparklines.
const HISTORY_LEN: usize = 120;

/// View tabs the user can switch between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewTab {
    Overview,
    GpuDetail,
    Processes,
}

/// A unified process entry for the process table.
#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_bytes: u64,
    /// GPU index if it's a GPU process (None for CPU-only).
    pub gpu_index: Option<u32>,
    /// GPU memory used (0 if not a GPU process).
    pub gpu_mem_bytes: u64,
    /// Process type: "CPU", "C" (compute), "G" (graphics), "C+G".
    pub proc_type: String,
}

/// Application state holding all collected data.
pub struct App {
    gpu_collector: GpuCollector,
    sys_collector: SystemCollector,
    sysinfo: System,

    // --- GPU data ---
    pub gpus: Vec<GpuSnapshot>,
    /// e.g. `nvidia:1 amd:2`
    pub backend_summary: String,
    pub driver_version: String,
    pub cuda_version: String,
    pub device_count: u32,
    pub selected_gpu: usize,

    // --- System data ---
    pub system: SystemSnapshot,
    pub per_core_usage: Vec<f32>,
    pub cpu_count: usize,

    // --- History ---
    pub cpu_history: VecDeque<f64>,
    pub mem_history: VecDeque<f64>,
    pub gpu_util_history: Vec<VecDeque<f64>>,
    pub mem_util_history: Vec<VecDeque<f64>>,
    pub gpu_temp_history: Vec<VecDeque<f64>>,
    pub gpu_power_history: Vec<VecDeque<f64>>,
    pub net_rx_history: VecDeque<f64>,
    pub net_tx_history: VecDeque<f64>,

    // --- Network rates ---
    pub net_rx_rate: u64,
    pub net_tx_rate: u64,
    last_net_rx: u64,
    last_net_tx: u64,

    // --- Processes ---
    pub processes: Vec<ProcessEntry>,

    // --- UI state ---
    pub running: bool,
    pub tick_count: u64,
    pub current_tab: ViewTab,
    pub show_help: bool,
    pub scroll_offset: u16,
}

impl App {
    /// Create and initialize the app state.
    pub fn new() -> Self {
        let gpu_collector = GpuCollector::new();
        let sys_collector = SystemCollector::new();

        let gpus = gpu_collector.collect();
        let system = sys_collector.collect();
        let device_count = gpu_collector.device_count;
        let backend_summary = gpu_collector.backend_summary.clone();
        let driver_version = gpu_collector.driver_version.clone();
        let cuda_version = gpu_collector.cuda_version.clone();

        let mut sysinfo = System::new_all();
        sysinfo.refresh_cpu_all();
        let per_core_usage: Vec<f32> = sysinfo.cpus().iter().map(|c| c.cpu_usage()).collect();
        let cpu_count = sysinfo.cpus().len();

        let gpu_util_history = vec![VecDeque::with_capacity(HISTORY_LEN); device_count as usize];
        let mem_util_history = vec![VecDeque::with_capacity(HISTORY_LEN); device_count as usize];
        let gpu_temp_history = vec![VecDeque::with_capacity(HISTORY_LEN); device_count as usize];
        let gpu_power_history = vec![VecDeque::with_capacity(HISTORY_LEN); device_count as usize];

        // Initial network totals
        let (last_net_rx, last_net_tx) = system
            .networks
            .iter()
            .fold((0u64, 0u64), |(rx, tx), n| {
                (rx + n.total_received_bytes, tx + n.total_transmitted_bytes)
            });

        Self {
            gpu_collector,
            sys_collector,
            sysinfo,
            gpus,
            system,
            backend_summary,
            driver_version,
            cuda_version,
            device_count,
            selected_gpu: 0,
            per_core_usage,
            cpu_count,
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            mem_history: VecDeque::with_capacity(HISTORY_LEN),
            gpu_util_history,
            mem_util_history,
            gpu_temp_history,
            gpu_power_history,
            net_rx_history: VecDeque::with_capacity(HISTORY_LEN),
            net_tx_history: VecDeque::with_capacity(HISTORY_LEN),
            net_rx_rate: 0,
            net_tx_rate: 0,
            last_net_rx,
            last_net_tx,
            processes: Vec::new(),
            running: true,
            tick_count: 0,
            current_tab: ViewTab::Overview,
            show_help: false,
            scroll_offset: 0,
        }
    }

    /// Refresh all metrics.
    pub fn tick(&mut self) {
        self.gpus = self.gpu_collector.collect();
        self.device_count = self.gpus.len() as u32;
        if self.selected_gpu >= self.gpus.len() && !self.gpus.is_empty() {
            self.selected_gpu = self.gpus.len() - 1;
        }
        self.ensure_gpu_history_len(self.gpus.len());

        self.system = self.sys_collector.collect();
        self.tick_count += 1;

        // Refresh per-core CPU
        self.sysinfo.refresh_cpu_all();
        self.per_core_usage = self.sysinfo.cpus().iter().map(|c| c.cpu_usage()).collect();

        // CPU history
        push_history(&mut self.cpu_history, self.system.cpu_usage_percent);

        // Memory history
        let mem_pct = if self.system.memory_total_bytes > 0 {
            (self.system.memory_used_bytes as f64 / self.system.memory_total_bytes as f64) * 100.0
        } else {
            0.0
        };
        push_history(&mut self.mem_history, mem_pct);

        // Network rates
        let (cur_rx, cur_tx) = self.system.networks.iter().fold((0u64, 0u64), |(rx, tx), n| {
            (rx + n.total_received_bytes, tx + n.total_transmitted_bytes)
        });
        self.net_rx_rate = cur_rx.saturating_sub(self.last_net_rx);
        self.net_tx_rate = cur_tx.saturating_sub(self.last_net_tx);
        self.last_net_rx = cur_rx;
        self.last_net_tx = cur_tx;

        push_history(
            &mut self.net_rx_history,
            self.net_rx_rate as f64 / 1024.0, // KB/s
        );
        push_history(
            &mut self.net_tx_history,
            self.net_tx_rate as f64 / 1024.0,
        );

        // GPU histories
        for (i, gpu) in self.gpus.iter().enumerate() {
            if i < self.gpu_util_history.len() {
                let util = gpu.gpu_utilization_percent.unwrap_or(0) as f64;
                push_history(&mut self.gpu_util_history[i], util);

                // VRAM occupancy (used/total), not memory-controller activity,
                // so the VRAM% chart matches the VRAM gauge.
                let mem_util = match (gpu.memory_used_bytes, gpu.memory_total_bytes) {
                    (Some(used), Some(total)) if total > 0 => {
                        used as f64 / total as f64 * 100.0
                    }
                    _ => gpu.memory_utilization_percent.unwrap_or(0) as f64,
                };
                push_history(&mut self.mem_util_history[i], mem_util);

                let temp = gpu.temperature_celsius.unwrap_or(0) as f64;
                push_history(&mut self.gpu_temp_history[i], temp);

                let power_pct = if let (Some(usage), Some(limit)) =
                    (gpu.power_usage_mw, gpu.power_limit_mw)
                {
                    if limit > 0 {
                        (usage as f64 / limit as f64) * 100.0
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                push_history(&mut self.gpu_power_history[i], power_pct);
            }
        }
    }

    /// Select next GPU.
    pub fn next_gpu(&mut self) {
        if !self.gpus.is_empty() {
            self.selected_gpu = (self.selected_gpu + 1) % self.gpus.len();
        }
    }

    /// Select previous GPU.
    pub fn prev_gpu(&mut self) {
        if !self.gpus.is_empty() {
            self.selected_gpu = self.selected_gpu.checked_sub(1).unwrap_or(self.gpus.len() - 1);
        }
    }

    fn ensure_gpu_history_len(&mut self, n: usize) {
        while self.gpu_util_history.len() < n {
            self.gpu_util_history
                .push(VecDeque::with_capacity(HISTORY_LEN));
            self.mem_util_history
                .push(VecDeque::with_capacity(HISTORY_LEN));
            self.gpu_temp_history
                .push(VecDeque::with_capacity(HISTORY_LEN));
            self.gpu_power_history
                .push(VecDeque::with_capacity(HISTORY_LEN));
        }
    }

    /// Switch tab.
    pub fn next_tab(&mut self) {
        self.current_tab = match self.current_tab {
            ViewTab::Overview => ViewTab::GpuDetail,
            ViewTab::GpuDetail => ViewTab::Processes,
            ViewTab::Processes => ViewTab::Overview,
        };
    }

    /// Collect processes (GPU + top system processes).
    pub fn collect_processes(&mut self) {
        let mut entries = Vec::new();

        // GPU processes
        let gpu_procs = self.gpu_collector.collect_processes();
        let gpu_pids: Vec<u32> = gpu_procs.iter().map(|p| p.pid).collect();

        // Refresh sysinfo processes for the GPU pids to get names/cpu/mem
        self.sysinfo.refresh_processes(ProcessesToUpdate::All, true);

        for gp in &gpu_procs {
            let (name, cpu, mem) = if let Some(proc) = self.sysinfo.process(Pid::from_u32(gp.pid)) {
                (
                    proc.name().to_string_lossy().to_string(),
                    proc.cpu_usage(),
                    proc.memory(),
                )
            } else {
                (format!("pid:{}", gp.pid), 0.0, 0)
            };

            entries.push(ProcessEntry {
                pid: gp.pid,
                name,
                cpu_percent: cpu,
                mem_bytes: mem,
                gpu_index: Some(gp.gpu_index),
                gpu_mem_bytes: gp.gpu_memory_bytes,
                proc_type: gp.process_type.clone(),
            });
        }

        // Top CPU processes (sorted by CPU%, top 30 non-GPU)
        let mut sys_procs: Vec<ProcessEntry> = self
            .sysinfo
            .processes()
            .iter()
            .filter(|(pid, _)| !gpu_pids.contains(&pid.as_u32()))
            .map(|(pid, proc)| ProcessEntry {
                pid: pid.as_u32(),
                name: proc.name().to_string_lossy().to_string(),
                cpu_percent: proc.cpu_usage(),
                mem_bytes: proc.memory(),
                gpu_index: None,
                gpu_mem_bytes: 0,
                proc_type: "CPU".to_string(),
            })
            .collect();

        sys_procs.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal));
        sys_procs.truncate(30);

        // GPU processes first, then top CPU processes
        entries.extend(sys_procs);

        // Sort final list: GPU processes at top (sorted by GPU mem), then CPU (sorted by CPU%)
        entries.sort_by(|a, b| {
            // GPU processes first
            match (a.gpu_index.is_some(), b.gpu_index.is_some()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (true, true) => b.gpu_mem_bytes.cmp(&a.gpu_mem_bytes),
                (false, false) => b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
            }
        });

        self.processes = entries;
    }

    /// Toggle help overlay.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    pub fn quit(&mut self) {
        self.running = false;
    }
}

/// Push a value to a history deque, evicting old entries.
fn push_history(deque: &mut VecDeque<f64>, value: f64) {
    if deque.len() >= HISTORY_LEN {
        deque.pop_front();
    }
    deque.push_back(value);
}
