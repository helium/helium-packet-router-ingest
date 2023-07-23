use config::{Config, File};
use duration_string::DurationString;
use std::{net::SocketAddr, path::PathBuf, str::FromStr};

#[derive(Debug, Clone, clap::Args, serde::Deserialize)]
pub struct Settings {
    /// Roaming Settings.
    #[command(flatten)]
    pub roaming: RoamingSettings,
    /// Network Settings.
    #[command(flatten)]
    pub network: NetworkSettings,

    /// How long before Packets are cleaned out of being deduplicated in duration time.
    #[arg(long, default_value = "10s")]
    pub cleanup_window: DurationString,

    /// Listen address for Metrics endpoint.
    #[arg(long, default_value = "")]
    pub metrics_listen: SocketAddr,
}

#[derive(Debug, Clone, clap::Args, serde::Deserialize)]
pub struct RoamingSettings {
    /// Which Protocol Version is this service interacting with.
    /// Notable differences between 1.0 and 1.1 is the addition of:
    /// - PRStartNotif
    /// - SenderNSID
    /// - ReceiverNSID
    #[arg(long, value_enum, default_value = "1.1")]
    #[serde(with = "protocol_version_serde")]
    pub protocol_version: ProtocolVersion,
    /// Helium forwarding NetID, for LNSs to identify which network to back through for downlinking.
    /// (default: C00053)
    #[arg(long, default_value = "C00053")]
    pub helium_net_id: String,
    /// NetID of the network operating the http forwarder
    #[arg(long, default_value = "000000")]
    pub target_net_id: String,
    /// Identity of the this NS, unique to HTTP forwarder
    #[arg(long, default_value = "6081FFFE12345678")]
    pub sender_nsid: String,
    /// Identify of the receiving NS
    #[arg(long, default_value = "0000000000000000")]
    pub receiver_nsid: String,
    /// How long were the packets held in duration time (1250ms, 1.2s)
    #[arg(long, default_value = "1250ms")]
    pub dedup_window: DurationString,
    /// When present, this string is inserted into the "AUTHORIZATION"
    /// header of all http requests.
    #[arg(long)]
    pub authorization_header: Option<String>,
}

#[derive(Debug, Clone, clap::ValueEnum, serde::Deserialize, serde::Serialize)]
pub enum ProtocolVersion {
    V1_0,
    V1_1,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::V1_1
    }
}

#[derive(Debug, Clone, clap::Args, serde::Deserialize)]
pub struct NetworkSettings {
    /// LNS Endpoint
    #[arg(long, default_value = "http://localhost:9005")]
    pub lns_endpoint: String,
    /// Http Server for Downlinks
    #[arg(long, default_value = "0.0.0.0:9002")]
    pub downlink_listen: SocketAddr,
    /// Grpc Server for Uplinks
    #[arg(long, default_value = "0.0.0.0:9000")]
    pub uplink_listen: SocketAddr,
}

pub fn from_path(path: Option<PathBuf>) -> Settings {
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

impl FromStr for ProtocolVersion {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1.0" => Ok(ProtocolVersion::V1_0),
            "1.1" => Ok(ProtocolVersion::V1_1),
            _ => Err(format!("Unknown ProtocolVersion: {}", s)),
        }
    }
}

impl ToString for ProtocolVersion {
    fn to_string(&self) -> String {
        match self {
            ProtocolVersion::V1_0 => "1.0".to_string(),
            ProtocolVersion::V1_1 => "1.1".to_string(),
        }
    }
}

mod protocol_version_serde {
    use std::str::FromStr;

    use super::ProtocolVersion;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ProtocolVersion, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version_str = String::deserialize(deserializer)?;
        ProtocolVersion::from_str(&version_str).map_err(serde::de::Error::custom)
    }
}
