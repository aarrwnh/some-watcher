mod ruleset;
pub use ruleset::*;

mod watcher;
pub use watcher::{Config, Watch};

pub use notify::Result;

#[macro_export]
macro_rules! color {
    ($c:expr, $w:expr) => {
        format!("\x1b[{}m{}\x1b[0m", $c, $w)
    };
}

pub trait Printable {
    /// Prepare path for display
    fn print(&self) -> String;
}

impl Printable for std::path::PathBuf {
    fn print(&self) -> String {
        let sep = color!("38;5;238", '/');
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
