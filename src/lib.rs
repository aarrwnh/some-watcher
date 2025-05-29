use std::sync::LazyLock;

mod ruleset;
pub use ruleset::*;

mod watcher;
pub use watcher::{Config, Watch, Msg};

pub use notify::Result;
pub use notify::EventKind;

static IGNORED_COLOR: &str = "38;5;238";
static SEP: LazyLock<String> = LazyLock::new(|| color!(IGNORED_COLOR, std::path::MAIN_SEPARATOR));

#[macro_export]
macro_rules! color {
    ($c:expr, $w:expr) => {
        format!("\x1b[{}m{}\x1b[0m", $c, $w)
    };
}

pub trait ColoredPath {
    /// Prepares path for display.
    fn color_path(&self) -> String;
}

#[derive(PartialEq)]
enum ColorHelp {
    Y, // yes
    N, // no
    M, // maybe
}

impl ColoredPath for std::path::PathBuf {
    fn color_path(&self) -> String {
        use std::path::Component::*;
        use ColorHelp::*;

        let mut res = Vec::from([]);
        for (i, c) in self.components().rev().enumerate() {
            if let Some((h, s)) = match c {
                CurDir => Some((Y, ".")),
                ParentDir => Some((Y, "..")),
                RootDir if i == 0 => Some((N, "#")),
                // idx=0 do not color tail part
                Normal(c) => Some((if i == 0 { N } else { Y }, c.to_str().unwrap())),
                // idx=1
                Prefix(c) => Some((if i == 1 { M } else { Y }, c.as_os_str().to_str().unwrap())),
                _ => None,
            } {
                res.insert(0, if h == Y || h == M {
                    let code = match i {
                        _ if h == M => 37,
                        1 => 36,
                        _ => 37,
                    };
                    color!(code, s)
                } else {
                    s.to_string()
                });
            };
        }
        res.join(&SEP)
    }
}

fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string()
}
