#![allow(clippy::type_complexity, clippy::should_implement_trait)]

use notify::{
    event::{ModifyKind, RenameMode},
    *,
};
use notify_debouncer_full::*;
use regex::Regex;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::watcher::QueueTask;

pub trait Module: Sync + Send + 'static {
    fn resolve(&mut self, src: PathBuf, dest: PathBuf) -> Resolved;
}

/// Control flow.
#[derive(Debug, Default)]
#[non_exhaustive]
pub enum Resolved {
    Move {
        dest: PathBuf,
    },
    Path(PathBuf),
    Info(String),
    Ok(String),
    Err(String),
    /// Continue with default file move.
    Continue,
    #[default]
    None,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) enum WatchingKind {
    Files,
    Dirs,
    #[default]
    All,
}

// ----------------------------------------------------------------------------------
//   - Task -
// ----------------------------------------------------------------------------------
type EventCheck = dyn Fn(EventKind) -> bool + Send + Sync;

#[derive(Default)]
pub struct Task<'a> {
    label: Option<&'a str>,
    /// A [`WatchingKind`] to filter watch events
    pub(crate) watched_types: WatchingKind,
    /// [`EventKind`]
    pub(crate) event_check: Option<&'a EventCheck>,
    /// Destination path
    pub(crate) destination: Option<PathBuf>,
    /// Regexp pattern
    match_pattern: Option<Regex>,

    inner: Option<Arc<Mutex<dyn Module>>>,
}

pub(crate) struct InnerTask<'a> {
    pub(crate) task: Arc<Mutex<Task<'a>>>,
    /// Normalized path for moved files
    pub(crate) dest: PathBuf,
}

impl<'a> Task<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_path_match_pattern(mut self, m: &str) -> Self {
        let a = Regex::new(m).unwrap();
        self.match_pattern.replace(a);
        self
    }

    /// Set destination path. Path can be relative.
    ///
    /// Not calling this method will use the watched path.
    pub fn set_destination(mut self, p: &str) -> Self {
        let path = PathBuf::from(p);
        if !path.starts_with(".") && path.is_dir() {
            std::fs::create_dir_all(&path).expect("created destination path");
        }
        self.destination.replace(path);
        self
    }

    /// For log printing?
    pub fn set_label(mut self, s: &'a str) -> Self {
        self.label.replace(s);
        self
    }

    pub const fn watch_files(mut self) -> Self {
        self.watched_types = WatchingKind::Files;
        self
    }

    pub const fn watch_dirs(mut self) -> Self {
        self.watched_types = WatchingKind::Dirs;
        self
    }

    pub const fn watch_all(self) -> Self {
        self
    }

    fn set_event(mut self, f: &'a EventCheck) -> Self {
        self.event_check.replace(f);
        self
    }

    pub fn on_modified(self) -> Self {
        self.set_event(&|kind| matches!(kind, EventKind::Modify(_)))
    }

    pub fn on_create(self) -> Self {
        self.set_event(&|kind| matches!(kind, EventKind::Create(_)))
    }

    pub fn on_rename(self) -> Self {
        self.set_event(&|kind| {
            matches!(
                kind,
                EventKind::Modify(ModifyKind::Name(RenameMode::To))
                    | EventKind::Modify(ModifyKind::Name(RenameMode::Both))
            )
        })
    }

    pub fn with_module<T: Module>(mut self, module: T) -> Self {
        self.inner.replace(Arc::new(Mutex::new(module)));
        self
    }

    pub fn finish(self) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(self))
    }

    pub(crate) fn parse(&self, src: PathBuf, mut dest: PathBuf) -> QueueTask {
        if !src.exists()
            || cfg!(target_os = "windows") && src.extension().is_some_and(|e| e == "part")
        {
            return QueueTask::None;
        }

        // filter events only if regex pattern match was provided
        if let Some(re) = &self.match_pattern {
            if re.captures(src.to_str().unwrap()).is_none() {
                return QueueTask::None;
            }
        }

        dest.push(src.file_name().unwrap());

        if let Some(x) = &self.inner {
            match x.lock().unwrap().resolve(src.clone(), dest.clone()) {
                Resolved::Move { dest: mut new_path } => {
                    std::mem::swap(&mut dest, &mut new_path);
                }
                Resolved::Path(path) => return QueueTask::Path(path),
                Resolved::Info(msg) => return QueueTask::Info(msg),
                Resolved::Ok(msg) => return QueueTask::Ok(msg),
                Resolved::Err(msg) => return QueueTask::Err(msg),
                Resolved::None => return QueueTask::None,
                Resolved::Continue => {}
            }
        }

        if src.cmp(&dest) == std::cmp::Ordering::Equal {
            // if paths are the same, as that will obviously should do nothing
            return QueueTask::None;
        }

        assert!(&dest.file_stem().is_some(), "filename can't be missing");
        QueueTask::Move { src, dest }
    }
}

// ----------------------------------------------------------------------------------
//   - Ruleset -
// ----------------------------------------------------------------------------------
pub struct Ruleset<'a> {
    pub(crate) watched_path: PathBuf,
    pub(crate) tasks: Vec<InnerTask<'a>>,
    pub(crate) poll_interval: Option<std::time::Duration>,
    pub(crate) recursive_mode: RecursiveMode,
}

impl<'a> Ruleset<'a> {
    pub fn new(watched_path: PathBuf) -> notify::Result<Self> {
        if watched_path.to_owned().ends_with("*") {
            panic!();
        }

        if !watched_path.exists() {
            return Err(notify::Error::path_not_found());
        }

        Ok(Self {
            watched_path,
            recursive_mode: RecursiveMode::NonRecursive,
            tasks: Vec::new(),
            poll_interval: None,
        })
    }

    /// Change directory watch mode to recursive.
    pub fn recursive_mode(&mut self) -> &mut Self {
        self.recursive_mode = RecursiveMode::Recursive;
        self
    }

    /// Modify polling interval of each rule (watching dir) instead of using global.
    pub fn with_poll_interval(&mut self, d: std::time::Duration) -> &mut Self {
        self.poll_interval.replace(d);
        self
    }

    pub fn finish(&self) -> Arc<&Self> {
        Arc::new(self)
    }

    pub fn add(&mut self, task: &Arc<Mutex<Task<'a>>>) -> &mut Self {
        let mut dest: Option<_> = None;

        // adjust task to parent rule
        let _ = task.lock().is_ok_and(|task| {
            if task.event_check.is_none() {
                panic!(
                    "required watch event for {} task missing ",
                    task.label.unwrap()
                );
            }

            // resolve and normalize final destination
            match &task.destination {
                Some(dst) => {
                    if cfg!(target_os = "windows") {
                        let path = self.watched_path.clone();
                        dest.replace(strip(path, dst.to_str().unwrap()));
                    } else {
                        unimplemented!();
                    }
                }
                // inherit from parent if empty
                None => {
                    dest.replace(self.watched_path.clone());
                }
            };
            true
        });

        self.tasks.push(InnerTask {
            task: Arc::clone(task),
            dest: dest.expect("could not resolve path"),
        });
        self
    }
}

const DIR_UP: &str = r"..\";
const DIR_CUR: &str = r".\";

pub fn strip(mut p: PathBuf, s: &str) -> PathBuf {
    match s {
        n if n.starts_with(DIR_UP) => match p.pop() {
            true => strip(p, n.strip_prefix(DIR_UP).unwrap()),
            false => strip(p, n.trim_start_matches(DIR_UP)),
        },
        n if n.starts_with(DIR_CUR) => strip(p, n.strip_prefix(DIR_CUR).unwrap()),
        _ => {
            p.push(s);
            p
        }
    }
}

#[rustfmt::skip]
#[test]
fn normalize_path() {
    let from = PathBuf::from;
    assert_eq!(Some(r"c:\a\e\"), strip(from(r"c:\a\b\c\"), r"..\..\e\").to_str());
    assert_eq!(Some(r"c:\a\"), strip(from(r"c:\a\b\c\"), r"..\..\").to_str());
    assert_eq!(Some(r"c:\"), strip(from(r"c:\a\"), r"..\..\..\").to_str());
    assert_eq!(Some(r"c:\a\b\"), strip(from(r"c:\a\"), r".\b\").to_str());
    assert_eq!(Some(r"c:\b\"), strip(from(r"c:\"), r".\b\").to_str());
}

// #[derive(Clone)]
// pub struct ShareableConfig {
//     inner: Arc<Mutex<Config>>,
// }

// impl From<Config> for ShareableConfig {
//     fn from(value: Config) -> Self {
//         Self {
//             inner: Arc::new(Mutex::new(value)),
//         }
//     }
// }

// impl ShareableConfig {
//     pub fn mutate(&self, mut f: impl FnMut(&mut Config)) {
//         f(&mut self.inner.lock().unwrap())
//     }

//     pub fn get<T>(&self, mut f: impl FnMut(&Config) -> T) -> T {
//         f(&self.inner.lock().unwrap())
//     }
// }
