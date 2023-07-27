use std::{collections::HashMap, net::SocketAddr};

use helium_proto::Region;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GwmpSettings {
    /// Listen address for Metrics endpoint.
    pub metrics_listen: SocketAddr,
    /// Grpc Server for Uplinks
    pub uplink_listen: SocketAddr,
    /// Address to forward over UDP
    /// If region does not contain specific port, this port will be used as default.
    pub lns_endpoint: SocketAddr,
    /// Region Port mapping for gwmp
    pub region_port_mapping: HashMap<Region, u16>,
}

impl Default for GwmpSettings {
    fn default() -> Self {
        Self {
            metrics_listen: "0.0.0.0:9002".parse().unwrap(),
            uplink_listen: "0.0.0.0:9000".parse().unwrap(),
            lns_endpoint: "http://localhost:1700".parse().unwrap(),
            region_port_mapping: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use helium_proto::Region;

    #[derive(serde::Serialize)]
    struct Testing {
        regions: HashMap<Region, u16>,
    }

    #[test]
    fn serialize_map_with_region() {
        let mut regions = HashMap::new();
        regions.insert(Region::Us915, 1700);
        regions.insert(Region::Au915, 1701);
        regions.insert(Region::As9231, 1702);
        let a = Testing { regions };

        println!("{}", toml::to_string(&a).unwrap());
    }
}
