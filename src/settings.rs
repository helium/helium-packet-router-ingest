use crate::{gwmp::settings::GwmpSettings, http_roaming::settings::HttpSettings};
use config::{Config, File};
use std::path::PathBuf;

#[derive(serde::Deserialize)]
struct ProtocolSetting {
    protocol: Protocol,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Gwmp,
    Http,
}

pub fn protocol_from_path(path: &PathBuf) -> Protocol {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    let c: ProtocolSetting = config
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("config with protocol");

    c.protocol
}

pub fn http_from_path(path: &PathBuf) -> HttpSettings {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("valid config file")
}

pub fn gwmp_from_path(path: &PathBuf) -> GwmpSettings {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("valid config file")
}
