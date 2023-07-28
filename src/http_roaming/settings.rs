use std::{net::SocketAddr, str::FromStr};

use duration_string::DurationString;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct HttpSettings {
    /// Roaming Settings.
    pub roaming: RoamingSettings,
    /// How long before Packets are cleaned out of being deduplicated in duration time.
    pub cleanup_window: DurationString,
    /// Listen address for Metrics endpoint.
    pub metrics_listen: SocketAddr,
    /// LNS Endpoint
    pub lns_endpoint: String,
    /// Http Server for Downlinks
    pub downlink_listen: SocketAddr,
    /// Grpc Server for Uplinks
    pub uplink_listen: SocketAddr,
}

#[derive(Debug, Clone, clap::Args, serde::Deserialize)]
pub struct RoamingSettings {
    /// Which Protocol Version is this service interacting with.
    /// Notable differences between 1.0 and 1.1 is the addition of:
    /// - PRStartNotif
    /// - SenderNSID
    /// - ReceiverNSID
    #[serde(with = "protocol_version_serde")]
    pub protocol_version: ProtocolVersion,
    /// Helium forwarding NetID, for LNSs to identify which network to back through for downlinking.
    /// (default: C00053)
    pub sender_net_id: String,
    /// NetID of the network operating the http forwarder
    pub receiver_net_id: String,
    /// Identity of the this NS, unique to HTTP forwarder
    pub sender_nsid: String,
    /// Identify of the receiving NS
    pub receiver_nsid: String,
    /// How long were the packets held in duration time (1250ms, 1.2s)
    pub dedup_window: DurationString,
    /// When present, this string is inserted into the "AUTHORIZATION"
    /// header of all http requests.
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
