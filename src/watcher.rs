#![allow(clippy::unused_io_amount)]
#![allow(unused_must_use)]

use crossbeam_channel::{bounded, Sender};
use notify::event::{ModifyKind, RemoveKind, RenameMode};
use notify::*;
use notify_debouncer_full::*;

use std::{fs, path::PathBuf, thread, time::Duration};

use crate::*;

const ICON_NOTHING: &str = "";
const ICON_INFO: &str = "";
const ICON_SUCCESS: &str = "";
const ICON_WARNING: &str = "";

/// Internal
pub(crate) enum QueueTask {
    Move { src: PathBuf, dest: PathBuf },
    Path(PathBuf),
    Info(String),
    Ok(String),
    Err(String),
    None,
}

struct Schedule<'e>(QueueTask, &'e str);

impl QueueTask {
    fn print_done(self, event_name: &str) {
        let (code, icon, msg) = match self {
            Self::Path(src) => (33, ICON_NOTHING, src.color_path()),
            Self::Info(msg) => (37, ICON_INFO, msg),
            Self::Ok(msg) => (32, ICON_SUCCESS, msg),
            Self::Err(msg) => (31, ICON_WARNING, msg),
            _ => return,
        };
        let e = &event_name[..3].to_uppercase();
        println!(" {} {} {msg}", color!(IGNORED_COLOR, e), color!(code, icon));
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
        self.rules.push(Ruleset::new(path.into()).unwrap());
        f(self.rules.last_mut().unwrap());
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
                    for Schedule(queue_task, event_name) in queue_rx {
                        self.handle_move_task(queue_task, event_name);
                    }
                })
                .unwrap();
        });
        Ok(())
    }

    fn handle_move_task(&self, task: QueueTask, event_name: &str) {
        match task {
            QueueTask::None => return,

            q @ QueueTask::Err(_)
            | q @ QueueTask::Ok(_)
            | q @ QueueTask::Info(_)
            | q @ QueueTask::Path(_) => q,

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
        }
        .print_done(event_name);
    }

    fn watch_one(
        &self,
        scheduler: &'_ Sender<Schedule<'_>>,
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
        debouncer.watcher().watch(path, *recursive_mode)?;

        let mode = if *recursive_mode == RecursiveMode::Recursive {
            "*"
        } else {
            ""
        };
        println!("\x1b[37m# watching {}\x1b[0m", path.join(mode).display());

        for result in rx {
            match result {
                Ok(events) => {
                    events.iter().for_each(|event| {
                        let path = event.paths.last().unwrap();
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
                                    if !f(event.kind) {
                                        continue;
                                    }
                                }
                            };

                            let queue_task = task.parse(path.to_owned(), inner.dest.to_owned());
                            let event_name = kind_to_str(event.kind);
                            scheduler.send(Schedule(queue_task, event_name));
                        }
                    });
                }
                Err(errors) => errors.iter().for_each(|error| eprintln!("{error:?}")),
            }
        }

        Ok(())
    }
}

// TODO: this is temp?
fn kind_to_str<'a>(event_kind: EventKind) -> &'a str {
    match event_kind {
        EventKind::Any => "any",
        EventKind::Access(_) => "access",
        EventKind::Create(_) => "create",

        EventKind::Modify(modify) => match modify {
            ModifyKind::Data(_) => "ModifyKind::Data(_)",
            ModifyKind::Metadata(_) => "ModifyKind::Metadata(_)",
            ModifyKind::Name(rename) => match rename {
                RenameMode::Any => "RenameMode::Any",
                RenameMode::To => "RenameMode::To",
                RenameMode::From => "RenameMode::From",
                RenameMode::Both => "RenameMode::Both",
                RenameMode::Other => "RenameMode::Other",
            },
            ModifyKind::Other => "ModifyKind::Other",
            _ => "modify",
        },

        EventKind::Remove(remove) => match remove {
            RemoveKind::Any => "RemoveKind::Any",
            RemoveKind::File => "RemoveKind::File",
            RemoveKind::Folder => "RemoveKind::Folder",
            RemoveKind::Other => "RemoveKind::Other",
        },

        EventKind::Other => "other",
    }
}
