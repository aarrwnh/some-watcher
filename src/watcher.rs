#![allow(clippy::unused_io_amount)]
#![allow(unused_must_use)]

use notify::*;
use notify_debouncer_full::*;

use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
    time::Duration,
};

use crate::*;

/// Internal
#[derive(Debug)]
pub(crate) enum QueueTask {
    Move { src: PathBuf, dest: PathBuf },
    Path(PathBuf),
    Info(String),
    Ok(String),
    Err(String),
    None,
}

impl QueueTask {
    #[inline]
    fn print(self) {
        match self {
            Self::Move { .. } => todo!(),
            Self::Path(src) => println!(" {}  {}", color!(33, ""), src.print()),
            Self::Info(msg) => println!(" {}  {}", color!(37, ""), msg),
            Self::Ok(msg) => println!(" {}  {}", color!(32, ""), msg),
            Self::Err(msg) => println!(" {}  {}", color!(31, ""), msg),
            Self::None => todo!(),
        }
    }
}

#[derive(Default, Debug)]
pub struct Config {
    /// Location for duplicate files for later inspection
    pub dump_folder: PathBuf,
    // TODO: naybe
    // ignore_path_length: bool,
}

pub struct Watch {
    config: Config,
}

impl Watch {
    pub fn new(config: Config) -> Self {
        let dump_folder = &config.dump_folder;
        if dump_folder.try_exists().is_err() {
            std::fs::create_dir_all(dump_folder).expect("should create new dump folder");
        }
        Self { config }
    }

    pub fn start(&self, rules: &[Rule]) -> notify::Result<()> {
        let (queue_tx, queue_rx) = mpsc::channel::<QueueTask>();
        thread::scope(|s| {
            // create watchers for each directory
            rules.iter().for_each(|rule| {
                thread::Builder::new()
                    .name(rule.src_path.to_str().unwrap().to_string())
                    .spawn_scoped(s, || {
                        log::info!("Watching {}", rule.src_path.print());
                        if let Err(error) = self.watch_one(&queue_tx, rule) {
                            log::error!("Error: {error:?}");
                        }
                    })
                    .unwrap();
            });

            thread::Builder::new()
                .spawn_scoped(s, move || {
                    queue_rx.iter().for_each(|task| self.handle_task(task));
                })
                .unwrap();
        });
        Ok(())
    }

    fn handle_task(&self, task: QueueTask) {
        // log::info!("{:?}", task);
        match task {
            q @ QueueTask::Err(_)
            | q @ QueueTask::Ok(_)
            | q @ QueueTask::Info(_)
            | q @ QueueTask::Path(_) => q.print(),

            QueueTask::Move { src, mut dest } => {
                // TODO: what to do with this?
                // if dest.to_string_lossy().len() > 300 {
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
                    Ok(_) => QueueTask::Ok(dest.print()).print(),
                    Err(err) => {
                        QueueTask::Err(format!("{}  {}", src.print(), color!(31, err))).print();
                    }
                }
            }
            QueueTask::None => {}
        }
    }

    fn watch_one(&self, scheduler: &Sender<QueueTask>, rule: &Rule) -> notify::Result<()> {
        let mut src_path = rule.src_path.to_path_buf();
        let recursive_mode = if src_path.ends_with("*") {
            src_path.pop(); // remove *
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };

        let (tx, rx) = mpsc::channel();
        let mut debouncer = new_debouncer(Duration::from_secs(2), None, tx)?;
        debouncer.watcher().watch(&src_path, recursive_mode)?;

        for result in rx {
            match result {
                Ok(events) => {
                    events.iter().for_each(move |event| {
                        // dbg!(event);
                        for action in &rule.jobs {
                            let event_check = match action.events.as_ref() {
                                None => continue,
                                Some(me) => me.lock().unwrap(),
                            };

                            if !event_check(event.kind) {
                                continue;
                            }

                            let path = event.paths.last().unwrap();
                            match action.watched_types {
                                WatchingKind::Dirs if !path.is_dir() => continue,
                                WatchingKind::Files if !path.is_file() => continue,
                                _ => {}
                            }
                            if let Some(f) = action.parse(path.clone()) {
                                scheduler.send(f).unwrap();
                            }
                        }
                    });
                }
                Err(errors) => errors.iter().for_each(|error| log::error!("{error:?}")),
            }
        }

        Ok(())
    }
}
