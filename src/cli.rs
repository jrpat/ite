//! Command-line interface.

use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;

/// Argument to `--expand`: a depth or `all`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExpandSpec {
    Depth(usize),
    All,
}

impl FromStr for ExpandSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        s.parse()
            .map(Self::Depth)
            .map_err(|_| format!("expected a depth or `all`, got {s:?}"))
    }
}

/// ite — interactive tree explorer.
#[derive(Parser, Debug)]
#[command(name = "ite", version, about)]
pub struct Cli {
    /// Directory to explore (defaults to the current directory).
    pub path: Option<PathBuf>,

    /// JSON file to explore instead of a directory.
    #[arg(short, long, value_name = "PATH", conflicts_with = "path")]
    pub json: Option<PathBuf>,

    /// Do not respect ignore files (.gitignore etc.).
    #[arg(short = 'I', long)]
    pub no_ignore: bool,

    /// Expand all non-leaves at or below depth N, or `all` for everything.
    #[arg(short, long)]
    pub expand: Option<ExpandSpec>,

    /// Config file to load instead of the user config; repeatable.
    #[arg(short, long)]
    pub config: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("ite").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn defaults() {
        let cli = parse(&[]);
        assert_eq!(cli.path, None);
        assert_eq!(cli.json, None);
        assert!(!cli.no_ignore);
        assert_eq!(cli.expand, None);
        assert!(cli.config.is_empty());
    }

    #[test]
    fn positional_path() {
        assert_eq!(parse(&["/some/dir"]).path, Some(PathBuf::from("/some/dir")));
    }

    #[test]
    fn json_path_flag_and_alias() {
        assert_eq!(
            parse(&["--json", "data.json"]).json,
            Some(PathBuf::from("data.json"))
        );
        assert_eq!(
            parse(&["-j", "other.json"]).json,
            Some(PathBuf::from("other.json"))
        );
    }

    #[test]
    fn json_and_directory_paths_are_mutually_exclusive() {
        assert!(Cli::try_parse_from(["ite", "--json", "data.json", "/some/dir"]).is_err());
    }

    #[test]
    fn no_ignore_flag_and_alias() {
        assert!(parse(&["--no-ignore"]).no_ignore);
        assert!(parse(&["-I"]).no_ignore);
    }

    #[test]
    fn expand_depth_and_all() {
        assert_eq!(parse(&["--expand", "2"]).expand, Some(ExpandSpec::Depth(2)));
        assert_eq!(parse(&["-e", "all"]).expand, Some(ExpandSpec::All));
    }

    #[test]
    fn expand_rejects_garbage() {
        assert!(Cli::try_parse_from(["ite", "--expand", "banana"]).is_err());
    }

    #[test]
    fn repeated_config_flags() {
        let cli = parse(&["-c", "a.toml", "--config", "b.toml"]);
        assert_eq!(
            cli.config,
            vec![PathBuf::from("a.toml"), PathBuf::from("b.toml")]
        );
    }
}
