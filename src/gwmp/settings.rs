use std::{collections::HashMap, net::SocketAddr};

use helium_proto::Region;
use serde::{
    de::{self, Visitor},
    Deserializer,
};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GwmpSettings {
    /// Listen address for Metrics endpoint.
    #[serde(deserialize_with = "deserialize_socket_addr")]
    pub metrics_listen: SocketAddr,
    /// Grpc Server for Uplinks
    #[serde(deserialize_with = "deserialize_socket_addr")]
    pub uplink_listen: SocketAddr,
    /// Address to forward over UDP
    #[serde(deserialize_with = "deserialize_socket_addr")]
    pub lns_endpoint: SocketAddr,
    /// Region Port mapping for gwmp.
    /// If a region is not present, the port specific in `lns_endpoint` will be used.
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

fn deserialize_socket_addr<'de, D>(deserializer: D) -> Result<SocketAddr, D::Error>
where
    D: Deserializer<'de>,
{
    struct SocketAddrVisitor;

    impl<'de> Visitor<'de> for SocketAddrVisitor {
        type Value = SocketAddr;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a valid socket address")
        }

        fn visit_str<E>(self, value: &str) -> Result<SocketAddr, E>
        where
            E: de::Error,
        {
            value
                .parse()
                .map_err(|_err| de::Error::invalid_value(de::Unexpected::Str(value), &self))
        }
    }

    deserializer.deserialize_str(SocketAddrVisitor)
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
        let _a = Testing { regions };

        // println!("{}", toml::to_string(&_a).unwrap());
    }
}
