//! plows-top — Professional terminal GPU & system monitor.
//!
//! Like htop, but for GPUs and system resources. Real-time monitoring of:
//! • Per-core CPU usage with htop-style colored bars
//! • Memory, swap, disk, network I/O
//! • All GPU utilization, VRAM, temperature, power, clocks
//! • History sparklines for CPU, GPU, and network
//!
//! Controls:
//!   q / Esc      — Quit
//!   Tab / 1 / 2  — Switch tabs (Overview / GPU Detail)
//!   ← / → / h/l — Switch between GPUs
//!   r            — Force refresh
//!   ?            — Toggle help

use clap::Parser;

#[derive(Parser)]
#[command(name = "plows-top", version, about = "Terminal GPU & system monitor")]
struct Cli {}

fn main() -> anyhow::Result<()> {
    // Handle --help/--version before opening a terminal, including in CI.
    Cli::parse();
    plows_top::run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn informational_flags_do_not_require_a_terminal() {
        for (flag, kind) in [
            ("--help", ErrorKind::DisplayHelp),
            ("--version", ErrorKind::DisplayVersion),
        ] {
            assert_eq!(
                Cli::try_parse_from(["plows-top", flag])
                    .err()
                    .unwrap()
                    .kind(),
                kind
            );
        }
        assert!(Cli::try_parse_from(["plows-top"]).is_ok());
    }
}
