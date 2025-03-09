#![allow(clippy::unused_io_amount)]
#![allow(unused_must_use)]

use crossbeam_channel::{bounded, Sender};
use notify::*;
use notify_debouncer_full::new_debouncer;

use std::fmt::Debug;
use std::sync::Mutex;
use std::{fs, path::PathBuf, thread, time::Duration};

use crate::*;

const ICON_NOTHING: &str = "";
const ICON_INFO: &str = "";
const ICON_SUCCESS: &str = " "; // 
const ICON_WARNING: &str = "";

static EVENT_BUFFER: LazyLock<Mutex<Buffer<(String, EventKind)>>> =
    LazyLock::new(|| Mutex::new(Buffer::with_capacity(9)));

/// Internal
pub(crate) enum QueueTask {
    Move { src: PathBuf, dest: PathBuf },
    Path(PathBuf),
    Info(String),
    Ok(String),
    Err(String),
    None,
}

struct Schedule(QueueTask);

impl QueueTask {
    fn print_done(self) {
        let (code, icon, msg) = match self {
            Self::Path(src) => (33, ICON_NOTHING, src.color_path()),
            Self::Info(msg) => (37, ICON_INFO, msg),
            Self::Ok(msg) => (32, ICON_SUCCESS, msg),
            Self::Err(msg) => (31, ICON_WARNING, msg),
            _ => return,
        };
        println!(" {} {msg}", color!(code, icon));
    }
}

#[derive(Default, Debug)]
pub struct Config {
    /// Location for duplicate files for later inspection
    pub dump_folder: PathBuf,
    // TODO: naybe
    // ignore_path_length: bool,
    /// See [notify::Config]
    pub poll_interval: Option<Duration>,
    pub tick_rate: Option<Duration>,
}

pub struct Watch<'a> {
    config: Config,
    rules: Vec<Ruleset<'a>>,
}

impl<'a> Watch<'a> {
    pub fn new(mut config: Config) -> Self {
        let dump_folder = &config.dump_folder;
        if dump_folder.try_exists().is_err() {
            std::fs::create_dir_all(dump_folder).expect("should create new dump folder");
        }
        config.poll_interval.get_or_insert(Duration::from_secs(2));
        Self {
            config,
            rules: Vec::new(),
        }
    }

    pub fn watch(&mut self, path: &str, f: impl FnOnce(&mut Ruleset<'a>)) -> &mut Self {
        let path: PathBuf = path.into();
        match path.exists() {
            true => {
                self.rules.push(Ruleset::new(path).unwrap());
                f(self.rules.last_mut().unwrap());
            }
            false => println!("\x1b[31m# skipping {}\x1b[0m", path.display()),
        }
        self
    }

    pub fn start(&mut self) -> notify::Result<()> {
        let (queue_tx, queue_rx) = bounded(1);
        thread::scope(|s| {
            // create watchers for each directory
            for rule in &self.rules {
                thread::Builder::new()
                    .name(format!("watcher#{}", rule.watched_path.display()))
                    .spawn_scoped(s, || {
                        if let Err(error) = self.watch_one(&queue_tx, rule) {
                            use notify::ErrorKind as E;
                            match error.kind {
                                E::PathNotFound => {
                                    eprintln!("Notfound {}", rule.watched_path.color_path())
                                }
                                _ => eprintln!("Error: {error:?}"),
                            };
                        };
                    })
                    .unwrap();
            }

            thread::Builder::new()
                .name("queue_rx".into())
                .spawn_scoped(s, || {
                    for Schedule(queue_task) in queue_rx {
                        self.handle_move_task(queue_task);
                    }
                })
                .unwrap();
        });
        Ok(())
    }

    fn handle_move_task(&self, task: QueueTask) {
        match task {
            QueueTask::Move { src, mut dest } => {
                // TODO: what to do with this?
                // if dest.to_string_lossy().len() > 259 {
                //     panic!("{}", dest.print());
                // }

                if let Ok(true) = dest.try_exists() {
                    std::mem::swap(&mut dest, &mut self.config.dump_folder.clone());
                    dest.push(src.file_name().unwrap());
                }

                if dest.exists() {
                    // add timestamp if more duplicates are possible
                    let ext = dest.extension().unwrap().to_str().unwrap().to_string();
                    dest.set_extension(ext + "." + &crate::timestamp());
                }

                if dest.file_stem().is_some() {
                    let mut temp = dest.clone();
                    temp.pop();
                    fs::create_dir(temp);
                }

                match fs::rename(&src, &dest) {
                    Ok(_) => QueueTask::Ok(dest.color_path()),
                    Err(err) => {
                        QueueTask::Err(format!("{}  {}", src.color_path(), color!(31, err)))
                    }
                }
            }
            rest => rest,
        }
        .print_done();
    }

    fn watch_one(
        &self,
        scheduler: &'_ Sender<Schedule>,
        rule: &'_ Ruleset<'_>,
    ) -> notify::Result<()> {
        let Ruleset {
            watched_path: path,
            recursive_mode,
            ..
        } = rule;

        let (tx, rx) = bounded(1);
        let mut debouncer = new_debouncer(
            rule.poll_interval
                .or(self.config.poll_interval)
                .expect("poll_interval"),
            self.config.tick_rate,
            tx,
        )?;
        debouncer.watch(path, *recursive_mode)?;

        let mode = if *recursive_mode == RecursiveMode::Recursive {
            "*"
        } else {
            ""
        };

        println!("\x1b[37m# watching {}\x1b[0m", path.join(mode).display());

        'recv: for result in rx {
            match result {
                Ok(events) => {
                    let Some(buf) = &mut EVENT_BUFFER.lock().ok() else {
                        continue 'recv;
                    };

                    events.iter().for_each(|event| {
                        let path = event.paths.last().expect("last event path");
                        let file_stem = path.file_stem().unwrap().to_string_lossy().to_string();
                        let prev = buf.get_with_key(&file_stem);

                        for inner in &rule.tasks {
                            let task = inner.task.lock().unwrap();

                            match task.watched_types {
                                WatchingKind::Dirs if !path.is_dir() => continue,
                                WatchingKind::Files if !path.is_file() => continue,
                                _ => {}
                            }

                            match task.event_check {
                                None => continue,
                                Some(f) => {
                                    if !f(event.kind, prev) {
                                        continue;
                                    }
                                }
                            };

                            let queue_task = task.parse(path.to_owned(), inner.dest.to_owned());
                            scheduler.send(Schedule(queue_task));
                        }

                        buf.push((file_stem, event.kind));
                    });
                }
                Err(errors) => errors.iter().for_each(|error| eprintln!("{error:?}")),
            }
        }

        Ok(())
    }
}

struct Buffer<T>(Vec<T>);

impl<T> Default for Buffer<T> {
    fn default() -> Self {
        Self::with_capacity(9)
    }
}

impl<T> Buffer<T> {
    fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    fn push(&mut self, item: T) {
        if self.0.len() == self.0.capacity() {
            self.0.remove(0);
        }
        self.0.push(item);
    }

    fn remove(&mut self, idx: usize) -> T {
        self.0.remove(idx)
    }
}

impl<K: std::cmp::PartialEq, V> Buffer<(K, V)> {
    fn get_with_key(&mut self, needle: &K) -> Option<V> {
        self.0
            .iter()
            .position(|(k, _)| *k == *needle)
            .map(|idx| self.remove(idx).1)
    }
}
