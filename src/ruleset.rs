#![allow(clippy::type_complexity)]

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

pub trait Module {
    fn resolve(&self, src: PathBuf, dest: PathBuf) -> Resolved;
}

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
    /// continue with default move file
    Continue,
    /// do nothing
    #[default]
    None,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum WatchingKind {
    Files,
    Dirs,
    #[default]
    All,
}

#[derive(Clone, Default)]
pub struct Job {
    /// Label to display with print
    label: Option<&'static str>,
    /// TODO: maybe later for crossterm
    description: Option<&'static str>,
    /// A [`WatchingKind`] to filter watch events
    pub(crate) watched_types: WatchingKind,
    /// [`EventKind`]
    pub(crate) events: Option<Box<Arc<Mutex<dyn (Fn(EventKind) -> bool) + Send + 'static>>>>,
    /// Destination path
    dest: Option<PathBuf>,
    /// Regexp pattern
    match_pattern: Option<Regex>,

    inner: Option<Box<Arc<Mutex<dyn Module + Send + 'static>>>>,
}

const NO_DESCRIPTION: &str = "no description";

impl Job {
    pub fn builder() -> Self {
        Self::default()
    }

    pub fn set_path_match_pattern(mut self, m: &str) -> Self {
        let a = Regex::new(m).unwrap();
        self.match_pattern.replace(a);
        self
    }

    /// Set destination path. Path can be relative.
    ///
    /// Not calling this method will use the root path [`Rule.src_path`].
    pub fn set_destination(mut self, p: &str) -> Self {
        let path = PathBuf::from(p);
        if path.is_dir() {
            std::fs::create_dir_all(&path).expect("created destination path");
        }
        self.dest.replace(path);
        self
    }

    /// For log printing?
    pub fn set_label(mut self, s: &'static str) -> Self {
        self.label.replace(s);
        self
    }

    /// TODO
    pub fn set_description(mut self, s: &'static str) -> Self {
        self.description.replace(s);
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

    fn set_event(mut self, callback: impl (Fn(EventKind) -> bool) + Send + 'static) -> Self {
        self.events
            .replace(Box::new(Arc::new(Mutex::new(callback))));
        self
    }

    pub fn on_modified(self) -> Self {
        self.set_event(|kind| matches!(kind, EventKind::Modify(_)))
    }

    pub fn on_create(self) -> Self {
        self.set_event(|kind| matches!(kind, EventKind::Create(_)))
    }

    pub fn on_rename(self) -> Self {
        self.set_event(|kind| {
            matches!(
                kind,
                EventKind::Modify(ModifyKind::Name(RenameMode::To))
                    | EventKind::Modify(ModifyKind::Name(RenameMode::Both))
            )
        })
    }

    pub fn with_callback(mut self, callback: impl Module + Send + 'static) -> Self {
        self.inner.replace(Box::new(Arc::new(Mutex::new(callback))));
        self
    }
}

pub(crate) trait JobParser {
    fn parse(&self, src: PathBuf) -> QueueTask;

    #[allow(dead_code)]
    fn short(&self) -> &str;
}

impl JobParser for Job {
    #[allow(dead_code)]
    fn short(&self) -> &str {
        match &self.description {
            Some(s) => &s[0..(if s.len() >= 10 { 10 } else { s.len() })],
            None => NO_DESCRIPTION,
        }
    }

    fn parse(&self, src: PathBuf) -> QueueTask {
        if src.extension().is_some_and(|e| e == "part") {
            return QueueTask::None;
        }

        // filter events only if regex pattern match was provided
        if let Some(re) = &self.match_pattern {
            let hay = src.to_str().unwrap();
            match re.captures(hay) {
                Some(_) => {},
                None => return QueueTask::None,
            };
        }

        let mut dest = self.dest.as_ref().unwrap().clone();
        dest.push(src.file_name().unwrap());

        if let Some(x) = &self.inner {
            // format destination path in the inner
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

        // TODO: handle folders (atm, can be handled in each module)
        if self.watched_types == WatchingKind::Dirs {
            return QueueTask::Path(src);
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
//   - Rule -
// ----------------------------------------------------------------------------------

#[derive(Clone)]
pub struct Rule {
    pub src_path: PathBuf,
    pub jobs: Vec<Job>,
}

impl Rule {
    pub const fn new(src_path: PathBuf) -> Self {
        Self {
            src_path,
            jobs: Vec::new(),
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, mut job: Job) -> Self {
        if job.events.is_none() {
            panic!(
                "required watch event for {} action missing ",
                job.label.unwrap()
            );
        }

        // assert destination path
        match job.dest.clone() {
            Some(dest) => {
                if cfg!(target_os = "windows") {
                    let mut src_path = self.src_path.clone();
                    // TODO: not sure this is the best way
                    let p = dest.to_str().unwrap().to_string();

                    if let Some(n) = p.strip_prefix(match p.to_owned() {
                        s if s.starts_with(r"..\") => {
                            src_path.pop();
                            r"..\"
                        }
                        s if s.starts_with(r".\") => r".\",
                        _ => "",
                    }) {
                        if n != p {
                            let mut new_dest = PathBuf::from(&src_path);
                            new_dest.push(n);
                            job.dest.replace(new_dest);
                        }
                    };
                }
            }
            // use root watching path if empty
            None => {
                job.dest.replace(self.src_path.to_path_buf());
            }
        };

        self.jobs.push(job);
        self
    }
}

/// Wrapper for [`Rule::new`]
pub fn watch_location(s: &'static str) -> Rule {
    Rule::new(s.into())
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
