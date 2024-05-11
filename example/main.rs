use watcher::*;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let app = Watch::new(Config {
        dump_folder: r"d:\Desktop\__DUPLICATES__".into(),
    });

    app.start(&[watch_location(r"d:\Desktop").add(
        ActionBuilder::default()
            .watch_modified_event()
            .watch_files()
            .with_callback(zip::Zips)
            .set_path_match_pattern(".zip")
            .set_destination(r"d:\Desktop\zips"),
    )])?;

    Ok(())
}

mod zip {
    use watcher::*;
    use std::path::PathBuf;

    pub struct Zips;

    impl Module for Zips {
        fn resolve(&self, src: PathBuf, dest: PathBuf) -> Resolved {
            if src.file_stem().is_some() {
                if src.file_name().unwrap().to_str().unwrap().contains("rar") {
                    // Move to destination without any changes
                    return Resolved::Continue;
                }

                // Create subfolder
                let sub_foldername = "rar-0001";
                let mut c = dest.to_path_buf();
                c.push(sub_foldername);
                let _ = std::fs::create_dir_all(&c);
                c.push(src.file_name().unwrap());

                return Resolved::Move { dest: c };
            }
            Resolved::None
        }
    }
}
