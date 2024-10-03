use watcher::*;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
struct MyModule {
    value: u8,
}

#[derive(Debug, Clone, Copy)]
struct MyModule2 {
    value2: u8,
}

impl MyModule {
    fn inc(&mut self) -> u8 {
        self.value = self.value.wrapping_add(10);
        self.value
    }
}

impl MyModule2 {
    fn inc(&mut self) -> u8 {
        self.value2 = self.value2.wrapping_add(10);
        self.value2
    }
}

impl Module for MyModule {
    fn resolve(&mut self, src: PathBuf, dest: PathBuf) -> Resolved {
        self.inc();
        println!("src: {:?}, dest: {:?}", src, dest);
        Resolved::Continue
    }
}

impl Module for MyModule2 {
    fn resolve(&mut self, src: PathBuf, dest: PathBuf) -> Resolved {
        self.inc();
        println!("src: {:?}, dest: {:?}", src, dest);
        Resolved::Continue
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let task1 = Task::new()
        .set_label("task1")
        .set_destination(r"..\")
        .with_module(MyModule { value: 1 })
        .on_create()
        .watch_files()
        .finish();

    let task2 = Task::new()
        .set_label("task2")
        .with_module(MyModule2 { value2: 230 })
        .on_create()
        .watch_files()
        .finish();

    let mut app = Watch::new(Config::default());
    app
        //
        .watch(r"r:\dev\a1\", |r| {
            r.recursive_mode().add(&task1);
        })
        .watch(r"r:\dev\a2\", |r| {
            r.add(&task1).add(&task2);
        })
        .watch(r"r:\dev\a3\", |r| {
            r.add(&task1).add(&task2);
        })
        .start()
        .unwrap();
}
