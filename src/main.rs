use std::ffi::OsString;
use std::io::{IsTerminal, Write, stderr};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use ite_cli::app::{App, Effect};
use ite_cli::cli::Cli;
use ite_cli::config::Config;
use ite_cli::fstree;
use ite_cli::keys::Key;
use ite_cli::runner::run_shell;
use ite_cli::tree::Tree;

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
    let tree = load_tree(&cli)?;
    let mut app = App::new(tree, &config, cli.expand);
    // Query before entering the alternate screen; terminals that don't answer
    // are detected quickly and leave the reverse-video fallback in place.
    app.palette = query_palette();

    // The tree draws on stderr so the selected value on stdout can be piped.
    let mut tui = Tui::enter().map_err(|e| e.to_string())?;
    let outcome = event_loop(&mut app, &mut tui);
    drop(tui);

    for error in app.tree.errors() {
        eprintln!("ite: {error}");
    }

    if let Some(path) = ite_cli::profile::output_path() {
        ite_cli::profile::GLOBAL
            .write_to(std::path::Path::new(path))
            .map_err(|e| format!("cannot write profile: {e}"))?;
    }

    match outcome.map_err(|e| e.to_string())? {
        Outcome::Quit => Ok(ExitCode::from(130)),
        Outcome::Print(path) => {
            println!("{}", path.to_string_lossy());
            Ok(ExitCode::SUCCESS)
        }
        Outcome::ShellExit(status) => Ok(match status.and_then(|s| s.code()) {
            Some(code) => ExitCode::from(code.clamp(0, 255) as u8),
            None => ExitCode::SUCCESS,
        }),
    }
}

/// Where the tree comes from. Piped JSON is consumed to EOF before the TUI
/// starts; terminal input then falls back to /dev/tty (crossterm does this
/// whenever stdin is not a tty). JSON inputs detect JSONL from the content;
/// the Jsonl variants force that reading.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    Dir(PathBuf),
    JsonFile(PathBuf),
    JsonStdin,
    JsonlFile(PathBuf),
    JsonlStdin,
}

fn choose_source(cli: &Cli, stdin_is_tty: bool) -> Result<Source, String> {
    if let Some(path) = &cli.jsonl {
        if path.as_os_str() == "-" {
            if stdin_is_tty {
                return Err(
                    "--jsonl -: stdin is a terminal; pipe a JSON Lines document in".to_string(),
                );
            }
            return Ok(Source::JsonlStdin);
        }
        return Ok(Source::JsonlFile(path.clone()));
    }
    if let Some(path) = &cli.json {
        if path.as_os_str() == "-" {
            if stdin_is_tty {
                return Err("--json -: stdin is a terminal; pipe a JSON document in".to_string());
            }
            return Ok(Source::JsonStdin);
        }
        return Ok(Source::JsonFile(path.clone()));
    }
    match &cli.path {
        Some(path) => Ok(Source::Dir(path.clone())),
        None if stdin_is_tty => Ok(Source::Dir(PathBuf::from("."))),
        None => Ok(Source::JsonStdin),
    }
}

fn load_tree(cli: &Cli) -> Result<Tree, String> {
    let open = |path: &PathBuf| {
        std::fs::File::open(path)
            .map_err(|error| format!("cannot open JSON file {}: {error}", path.display()))
    };
    match choose_source(cli, std::io::stdin().is_terminal())? {
        Source::JsonStdin => ite_cli::json_tree::from_reader(std::io::stdin().lock())
            .map_err(|error| format!("stdin: {error}")),
        Source::JsonlStdin => ite_cli::json_tree::jsonl_from_reader(std::io::stdin().lock())
            .map_err(|error| format!("stdin: {error}")),
        Source::JsonFile(path) => ite_cli::json_tree::from_reader(open(&path)?)
            .map_err(|error| format!("{}: {error}", path.display())),
        Source::JsonlFile(path) => ite_cli::json_tree::jsonl_from_reader(open(&path)?)
            .map_err(|error| format!("{}: {error}", path.display())),
        Source::Dir(dir) => {
            if !dir.is_dir() {
                return Err(format!("{} is not a directory", dir.display()));
            }
            fstree::scan(&dir, cli.no_ignore).map_err(|e| e.to_string())
        }
    }
}

fn query_palette() -> Option<ite_cli::ui::Palette> {
    let palette =
        terminal_colorsaurus::color_palette(terminal_colorsaurus::QueryOptions::default()).ok()?;
    Some(ite_cli::ui::Palette {
        fg: palette.foreground.scale_to_8bit(),
        bg: palette.background.scale_to_8bit(),
    })
}

enum Outcome {
    Quit,
    Print(OsString),
    ShellExit(Option<std::process::ExitStatus>),
}

fn event_loop(app: &mut App, tui: &mut Tui) -> std::io::Result<Outcome> {
    loop {
        {
            let _span = ite_cli::profile::span("main::frame");
            tui.terminal.draw(|frame| {
                let area = frame.area();
                ite_cli::ui::draw(app, area, frame.buffer_mut());
            })?;
        }
        // While the index is incomplete, alternate short input polls with
        // cooperative work quanta; once complete, block on input as before.
        let ready = if app.has_work() {
            crossterm::event::poll(std::time::Duration::from_millis(2))?
        } else {
            true
        };
        let effect = if ready {
            handle_event(app, crossterm::event::read()?)
        } else {
            app.do_work();
            Effect::None
        };
        match effect {
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

const MOUSE_SCROLL_ROWS: isize = 3;

fn handle_event(app: &mut App, event: Event) -> Effect {
    match event {
        Event::Key(event) if event.kind != KeyEventKind::Release => {
            app.handle_key(Key::from_event(event))
        }
        Event::Mouse(event) => {
            match event.kind {
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    let delta = match event.kind {
                        MouseEventKind::ScrollDown => MOUSE_SCROLL_ROWS,
                        _ => -MOUSE_SCROLL_ROWS,
                    };
                    if app.keybinding_panel.contains(event.column, event.row) {
                        app.keybinding_panel.scroll_by(delta);
                    } else {
                        app.state.scroll_view_by(delta);
                    }
                }
                _ => {}
            }
            Effect::None
        }
        _ => Effect::None,
    }
}

/// RAII terminal guard: raw mode + alternate screen on stderr, with best-effort
/// keyboard enhancement so ctrl+enter and shift+arrows are distinguishable.
///
/// While active, fd 1 is pointed at stderr: crossterm writes terminal queries
/// (the keyboard-enhancement probe) to stdout, which would otherwise vanish
/// into the pipe this program's stdout usually feeds. `suspend` puts the real
/// stdout back, so shell bindings and the final selection see the pipe.
struct Tui {
    terminal: Terminal<CrosstermBackend<std::io::Stderr>>,
    enhanced: bool,
    active: bool,
    saved_stdout: Option<std::os::fd::OwnedFd>,
}

impl Tui {
    fn enter() -> std::io::Result<Self> {
        let mut tui = Self {
            terminal: Terminal::new(CrosstermBackend::new(stderr()))?,
            enhanced: false,
            active: false,
            saved_stdout: None,
        };
        // Raw mode and the stdout swap must be in place before probing
        // keyboard enhancement support, or the probe (or its response) is
        // lost whenever stdout is piped.
        tui.resume()?;
        tui.enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if tui.enhanced {
            execute!(
                stderr(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        Ok(tui)
    }

    fn resume(&mut self) -> std::io::Result<()> {
        self.saved_stdout = Some(rustix::io::dup(std::io::stdout())?);
        rustix::stdio::dup2_stdout(stderr())?;
        enable_raw_mode()?;
        execute!(stderr(), EnterAlternateScreen, EnableMouseCapture)?;
        if self.enhanced {
            execute!(
                stderr(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        self.active = true;
        // Not Terminal::clear: that would query the cursor position, a
        // stdout-answered round trip the terminal may never see. Entering the
        // alternate screen already cleared it; a fresh Terminal has empty
        // buffers, so the next draw repaints every cell.
        self.terminal = Terminal::new(CrosstermBackend::new(stderr()))?;
        Ok(())
    }

    fn suspend(&mut self) -> std::io::Result<()> {
        if !self.active {
            return Ok(());
        }
        if self.enhanced {
            execute!(stderr(), PopKeyboardEnhancementFlags)?;
        }
        execute!(stderr(), DisableMouseCapture, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        stderr().flush()?;
        if let Some(saved) = self.saved_stdout.take() {
            rustix::stdio::dup2_stdout(&saved)?;
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(path: Option<PathBuf>) -> Cli {
        Cli {
            path,
            json: None,
            jsonl: None,
            no_ignore: false,
            expand: None,
            config: Vec::new(),
        }
    }

    #[test]
    fn json_path_is_transformed_without_using_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("data.json");
        std::fs::write(&json, r#"["first", "second"]"#).unwrap();
        let mut cli = cli(None);
        cli.json = Some(json);

        let tree = load_tree(&cli).unwrap();

        assert_eq!(tree.name(tree.root_ids()[0]), "$ [2]");
    }

    #[test]
    fn directory_path_scans_the_requested_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();

        let tree = load_tree(&cli(Some(dir.path().to_path_buf()))).unwrap();

        assert_eq!(tree.name(tree.root_ids()[0]), "file.txt");
    }

    #[test]
    fn invalid_json_file_is_an_input_error() {
        let dir = tempfile::tempdir().unwrap();
        let json = dir.path().join("bad.json");
        std::fs::write(&json, "{]").unwrap();
        let mut cli = cli(None);
        cli.json = Some(json.clone());

        let error = load_tree(&cli).unwrap_err();

        assert!(error.starts_with(&format!("{}: invalid JSON input:", json.display())));
    }

    #[test]
    fn piped_stdin_without_a_path_selects_json_from_stdin() {
        assert_eq!(choose_source(&cli(None), false).unwrap(), Source::JsonStdin);
    }

    #[test]
    fn tty_stdin_without_a_path_selects_the_current_directory() {
        assert_eq!(
            choose_source(&cli(None), true).unwrap(),
            Source::Dir(PathBuf::from("."))
        );
    }

    #[test]
    fn a_directory_path_wins_over_piped_stdin() {
        assert_eq!(
            choose_source(&cli(Some(PathBuf::from("/some/dir"))), false).unwrap(),
            Source::Dir(PathBuf::from("/some/dir"))
        );
    }

    #[test]
    fn json_dash_selects_stdin() {
        let mut cli = cli(None);
        cli.json = Some(PathBuf::from("-"));
        assert_eq!(choose_source(&cli, false).unwrap(), Source::JsonStdin);
    }

    #[test]
    fn json_dash_with_a_tty_stdin_is_an_error() {
        let mut cli = cli(None);
        cli.json = Some(PathBuf::from("-"));
        let error = choose_source(&cli, true).unwrap_err();
        assert!(error.contains("stdin is a terminal"), "got: {error}");
    }

    #[test]
    fn json_file_is_unaffected_by_piped_stdin() {
        let mut cli = cli(None);
        cli.json = Some(PathBuf::from("data.json"));
        assert_eq!(
            choose_source(&cli, false).unwrap(),
            Source::JsonFile(PathBuf::from("data.json"))
        );
    }

    #[test]
    fn jsonl_flag_selects_the_jsonl_file_and_dash_selects_stdin() {
        let mut cli = cli(None);
        cli.jsonl = Some(PathBuf::from("log.jsonl"));
        assert_eq!(
            choose_source(&cli, true).unwrap(),
            Source::JsonlFile(PathBuf::from("log.jsonl"))
        );

        cli.jsonl = Some(PathBuf::from("-"));
        assert_eq!(choose_source(&cli, false).unwrap(), Source::JsonlStdin);
        assert!(choose_source(&cli, true).is_err());
    }

    #[test]
    fn json_flag_detects_jsonl_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("records.json");
        std::fs::write(&path, "1\n2\n").unwrap();
        let mut cli = cli(None);
        cli.json = Some(path);

        let tree = load_tree(&cli).unwrap();

        assert_eq!(tree.name(tree.root_ids()[0]), "$ [2]");
    }

    #[test]
    fn jsonl_flag_forces_the_jsonl_reading() {
        // A single scalar would detect as JSON; the flag forces JSONL.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.jsonl");
        std::fs::write(&path, "7\n").unwrap();
        let mut cli = cli(None);
        cli.jsonl = Some(path);

        let tree = load_tree(&cli).unwrap();

        assert_eq!(tree.name(tree.root_ids()[0]), "$ [1]");
    }

    #[test]
    fn mouse_wheel_scrolls_the_viewport_without_moving_focus() {
        let mut tree = Tree::new();
        for index in 0..8 {
            tree.push(
                None,
                format!("row {index}"),
                false,
                ite_cli::tree::ActionValues::new("", "", ""),
            );
        }
        let mut app = App::new(tree, &Config::default(), None);
        let area = ratatui::layout::Rect::new(0, 0, 20, 3);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        ite_cli::ui::draw(&mut app, area, &mut buffer);
        let focused = app.focused_id();

        let effect = handle_event(
            &mut app,
            Event::Mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }),
        );
        ite_cli::ui::draw(&mut app, area, &mut buffer);

        assert_eq!(effect, Effect::None);
        assert!(
            app.state.offset() > 0,
            "the viewport should remain scrolled after redraw"
        );
        assert_eq!(app.focused_id(), focused, "focus should not move");
    }

    #[test]
    fn mouse_wheel_over_the_keybinding_panel_scrolls_it_not_the_tree() {
        let mut tree = Tree::new();
        for index in 0..30 {
            tree.push(
                None,
                format!("row {index}"),
                false,
                ite_cli::tree::ActionValues::new("", "", ""),
            );
        }
        let mut app = App::new(tree, &Config::default(), None);
        app.handle_key(Key::parse("?").unwrap());
        let area = ratatui::layout::Rect::new(0, 0, 30, 12);
        let mut buffer = ratatui::buffer::Buffer::empty(area);
        ite_cli::ui::draw(&mut app, area, &mut buffer);
        let panel = app.keybinding_panel.area().unwrap();
        let tree_offset = app.state.offset();

        let effect = handle_event(
            &mut app,
            Event::Mouse(crossterm::event::MouseEvent {
                kind: crossterm::event::MouseEventKind::ScrollDown,
                column: panel.x + 1,
                row: panel.y + 1,
                modifiers: crossterm::event::KeyModifiers::NONE,
            }),
        );

        assert_eq!(effect, Effect::None);
        assert!(app.keybinding_panel.scroll() > 0);
        assert_eq!(app.state.offset(), tree_offset);
    }
}
