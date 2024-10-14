use normalize_path::NormalizePath;
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
    /// Destination path. By default watched path from [`Ruleset`] is used.
    pub(crate) destination: Option<PathBuf>,
    /// Filter path events only if regex pattern match was provided.
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
            panic!(
                "Asterisk (*) not allowed as suffix in path. Use recursive_mode() method instead."
            );
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
        let watched_path = self.watched_path.clone();
        let mut dest: Option<_> = None;
        // adjust task to parent rule
        let _ = task.lock().is_ok_and(|task| {
            if task.event_check.is_none() {
                panic!(
                    "required watch event for {} task missing ",
                    task.label.unwrap()
                );
            }

            dest.replace(match &task.destination {
                Some(dst) => watched_path.join(dst),
                // inherit from parent if empty
                None => watched_path,
            });

            true
        });

        self.tasks.push(InnerTask {
            task: Arc::clone(task),
            dest: dest.expect("could not resolve path").normalize(),
        });
        self
    }
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
