use std::io::{Write, stderr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use crossterm::event::{
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use ite::app::{App, Effect};
use ite::cli::Cli;
use ite::config::Config;
use ite::fstree::FsTree;
use ite::keys::Key;
use ite::runner::run_shell;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("ite: {err}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(cli: &Cli) -> Result<Config, String> {
    if !cli.config.is_empty() {
        return Config::load_files(&cli.config);
    }
    match Config::user_config_path() {
        Some(path) if path.exists() => Config::load_files(&[path]),
        _ => Ok(Config::default()),
    }
}

fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse();
    let config = load_config(&cli)?;
    let dir = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let tree = FsTree::scan(&dir, cli.no_ignore).map_err(|e| e.to_string())?;
    let mut app = App::new(tree, &config, cli.expand);
    // Query before entering the alternate screen; terminals that don't answer
    // are detected quickly and leave the reverse-video fallback in place.
    app.palette = query_palette();

    // The tree draws on stderr so the selected path on stdout can be piped.
    let mut tui = Tui::enter().map_err(|e| e.to_string())?;
    let outcome = event_loop(&mut app, &mut tui);
    drop(tui);

    if let Some(path) = ite::profile::output_path() {
        ite::profile::GLOBAL
            .write_to(std::path::Path::new(path))
            .map_err(|e| format!("cannot write profile: {e}"))?;
    }

    match outcome.map_err(|e| e.to_string())? {
        Outcome::Quit => Ok(ExitCode::from(130)),
        Outcome::Print(path) => {
            println!("{}", path.display());
            Ok(ExitCode::SUCCESS)
        }
        Outcome::ShellExit(status) => Ok(match status.and_then(|s| s.code()) {
            Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
            None => ExitCode::SUCCESS,
        }),
    }
}

fn query_palette() -> Option<ite::ui::Palette> {
    let palette =
        terminal_colorsaurus::color_palette(terminal_colorsaurus::QueryOptions::default()).ok()?;
    Some(ite::ui::Palette {
        fg: palette.foreground.scale_to_8bit(),
        bg: palette.background.scale_to_8bit(),
    })
}

enum Outcome {
    Quit,
    Print(PathBuf),
    ShellExit(Option<std::process::ExitStatus>),
}

fn event_loop(app: &mut App, tui: &mut Tui) -> std::io::Result<Outcome> {
    loop {
        {
            let _span = ite::profile::span("main::frame");
            tui.terminal.draw(|frame| {
                let area = frame.area();
                ite::ui::draw(app, area, frame.buffer_mut());
            })?;
        }
        let Event::Key(event) = crossterm::event::read()? else {
            continue;
        };
        if event.kind == KeyEventKind::Release {
            continue;
        }
        match app.handle_key(Key::from_event(event)) {
            Effect::None => {}
            Effect::Quit => return Ok(Outcome::Quit),
            Effect::PrintAndExit(path) => return Ok(Outcome::Print(path)),
            Effect::RunShell {
                cmd,
                path,
                relpath,
                bg,
                exit,
            } => {
                if bg {
                    run_shell(&cmd, &path, &relpath, true)?;
                } else {
                    // Hand the terminal to the command, then take it back.
                    tui.suspend()?;
                    let status = run_shell(&cmd, &path, &relpath, false)?;
                    if exit {
                        return Ok(Outcome::ShellExit(status));
                    }
                    tui.resume()?;
                }
            }
        }
    }
}

/// RAII terminal guard: raw mode + alternate screen on stderr, with best-effort
/// keyboard enhancement so ctrl+enter and shift+arrows are distinguishable.
struct Tui {
    terminal: Terminal<CrosstermBackend<std::io::Stderr>>,
    enhanced: bool,
    active: bool,
}

impl Tui {
    fn enter() -> std::io::Result<Self> {
        // Raw mode must be active before probing keyboard enhancement support,
        // or the probe's response cannot be read reliably.
        enable_raw_mode()?;
        let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        let mut tui = Self {
            terminal: Terminal::new(CrosstermBackend::new(stderr()))?,
            enhanced,
            active: false,
        };
        tui.resume()?;
        Ok(tui)
    }

    fn resume(&mut self) -> std::io::Result<()> {
        enable_raw_mode()?;
        execute!(stderr(), EnterAlternateScreen)?;
        if self.enhanced {
            execute!(
                stderr(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        self.active = true;
        self.terminal.clear()
    }

    fn suspend(&mut self) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }
        if self.enhanced {
            execute!(stderr(), PopKeyboardEnhancementFlags)?;
        }
        execute!(stderr(), LeaveAlternateScreen)?;
        disable_raw_mode()?;
        stderr().flush()?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}
