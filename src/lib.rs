mod ruleset;
pub use ruleset::*;

mod watcher;
pub use watcher::{Config, Watch};

pub use notify::Result;

static IGNORED_COLOR: &str = "38;5;238";

#[macro_export]
macro_rules! color {
    ($c:expr, $w:expr) => {
        format!("\x1b[{}m{}\x1b[0m", $c, $w)
    };
}

pub trait PrintablePath {
    /// Prepares path for display
    fn prepare_path(&self) -> String;
}

impl PrintablePath for std::path::PathBuf {
    fn prepare_path(&self) -> String {
        let sep = color!(IGNORED_COLOR, '/');
        let s = self.to_string_lossy();
        let parts = s.split('\\').collect::<Vec<_>>();
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == parts.len() - 2 {
                    color!(36, p)
                } else if i < parts.len() - 1 {
                    color!(37, p)
                } else {
                    p.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(&sep)
    }
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}
