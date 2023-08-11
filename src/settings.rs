use crate::{gwmp::settings::GwmpSettings, http_roaming::settings::HttpSettings, Result};
use anyhow::Context;
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

pub fn protocol_from_path(path: &PathBuf) -> Result<Protocol> {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    let c: ProtocolSetting = config
        .build()
        .and_then(|config| config.try_deserialize())
        .context(format!("Determining protocol from {path:?}"))?;

    Ok(c.protocol)
}

pub fn http_from_path(path: &PathBuf) -> Result<HttpSettings> {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .context(format!("Reading HTTP Roaming settings from {path:?}"))
}

pub fn gwmp_from_path(path: &PathBuf) -> Result<GwmpSettings> {
    let config = Config::builder()
        .add_source(File::with_name("./settings/default.toml").required(true))
        .add_source(File::with_name(path.to_str().expect("filename")));

    config
        .build()
        .and_then(|config| config.try_deserialize())
        .context(format!("Reading GWMP settings from {path:?}"))
}
