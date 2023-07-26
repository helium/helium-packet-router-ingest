use crate::{gwmp::settings::GwmpSettings, http_roaming::settings::HttpSettings};
use config::{Config, File};
use std::path::PathBuf;

pub fn http_from_path(path: Option<PathBuf>) -> HttpSettings {
    let mut config =
        Config::builder().add_source(File::with_name("./settings/default.toml").required(true));

    if let Some(path) = path {
        let filename = path.to_str().expect("filename");
        config = config.add_source(File::with_name(filename));
    }

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("valid config file")
}

pub fn gwmp_from_path(path: Option<PathBuf>) -> GwmpSettings {
    let mut config =
        Config::builder().add_source(File::with_name("./settings/default.toml").required(true));

    if let Some(path) = path {
        let filename = path.to_str().expect("filename");
        config = config.add_source(File::with_name(filename));
    }

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("valid config file")
}
