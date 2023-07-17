use config::{Config, File};
use duration_string::DurationString;
use std::{net::SocketAddr, path::PathBuf};

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
    /// Send PRStartNotif message after sending Downlink to gateway.
    /// Chirpstack does nothing with the notif message and returns an 400.
    #[arg(long, default_value = "false")]
    pub send_pr_start_notif: bool,
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

pub fn from_path(path: PathBuf) -> Settings {
    let filename = path.to_str().expect("filename");
    Config::builder()
        .add_source(File::with_name(filename).required(false))
        .build()
        .and_then(|config| config.try_deserialize())
        .expect("valid config file")
}
