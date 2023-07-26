use std::net::SocketAddr;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GwmpSettings {
    /// Listen address for Metrics endpoint.
    pub metrics_listen: SocketAddr,
    /// Grpc Server for Uplinks
    pub uplink_listen: SocketAddr,
    /// Address to forward over UDP
    pub lns_endpoint: SocketAddr,
}

impl Default for GwmpSettings {
    fn default() -> Self {
        Self {
            metrics_listen: "0.0.0.0:9002".parse().unwrap(),
            uplink_listen: "0.0.0.0:9000".parse().unwrap(),
            lns_endpoint: "http://localhost:1700".parse().unwrap(),
        }
    }
}
