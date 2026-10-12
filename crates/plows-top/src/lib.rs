//! plows-top — Professional terminal GPU & system monitor library.
//!
//! Like htop, but for GPUs and system resources. Shows real-time GPU utilization,
//! memory, power, temperature, clocks, per-core CPU usage, memory/swap, network I/O,
//! disk usage, and history sparklines — all in a beautiful terminal UI.
//!
//! Controls:
//!   q / Esc      — Quit
//!   Tab / 1 / 2  — Switch tabs (Overview / GPU Detail)
//!   ← / → / h/l — Switch between GPUs
//!   r            — Force refresh
//!   ?            — Toggle help
//!   ↑ / ↓ / j/k  — Scroll focused panel (PgUp/PgDn: by 10)
//!   c / g        — Focus CPU / GPU panel (Overview)

pub mod app;
pub mod gpu;
pub mod system;
pub mod ui;

use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use app::{OverviewFocus, ViewTab};

/// Refresh rate (1 second).
const TICK_RATE: Duration = Duration::from_secs(1);

/// Run the TUI GPU & system monitor application.
pub fn run() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Run app
    let result = run_app(&mut terminal);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }

    Ok(())
}

/// Main event loop.
fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> color_eyre::Result<()> {
    let mut app = app::App::new();
    let mut last_tick = Instant::now();

    // Initial tick to populate history
    app.tick();

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    // If help is showing, any key dismisses it
                    if app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    let overview = app.current_tab == ViewTab::Overview;
                    match key.code {
                        // Quit
                        KeyCode::Char('q') | KeyCode::Esc => app.quit(),

                        // Tab switching
                        KeyCode::Tab => app.next_tab(),
                        KeyCode::Char('1') => app.current_tab = ViewTab::Overview,
                        KeyCode::Char('2') => app.current_tab = ViewTab::GpuDetail,
                        KeyCode::Char('3') => app.current_tab = ViewTab::Processes,

                        // GPU navigation
                        KeyCode::Left | KeyCode::Char('h') => app.prev_gpu(),
                        KeyCode::Right | KeyCode::Char('l') => app.next_gpu(),

                        // Refresh
                        KeyCode::Char('r') => app.tick(),

                        // Help
                        KeyCode::Char('?') => app.toggle_help(),

                        // Scroll
                        KeyCode::Char('j') | KeyCode::Down if overview => app.scroll_overview(1),
                        KeyCode::Char('k') | KeyCode::Up if overview => app.scroll_overview(-1),
                        KeyCode::PageDown if overview => app.scroll_overview(10),
                        KeyCode::PageUp if overview => app.scroll_overview(-10),
                        KeyCode::Char('j') | KeyCode::Down => app.scroll_down(),
                        KeyCode::Char('k') | KeyCode::Up => app.scroll_up(),

                        // Overview panel focus
                        KeyCode::Char('c') => app.overview_focus = OverviewFocus::Cpu,
                        KeyCode::Char('g') => app.overview_focus = OverviewFocus::Gpu,

                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            app.tick();
            app.collect_processes();
            last_tick = Instant::now();
        }

        if !app.running {
            break;
        }
    }

    Ok(())
}
