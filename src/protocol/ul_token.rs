use helium_proto::Region;
use hex::FromHex;

use crate::protocol::uplink::GatewayB58;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Token {
    pub timestamp: u64,
    pub gateway: GatewayB58,
    pub region: Region,
    pub packet_type: PacketType,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum PacketType {
    Join,
    Data,
}

impl FromHex for Token {
    type Error = anyhow::Error;

    fn from_hex<T: AsRef<[u8]>>(hex: T) -> std::result::Result<Self, Self::Error> {
        let s = hex::decode(hex).unwrap();
        Ok(serde_json::from_slice(&s[..]).unwrap())
    }
}

pub fn make_join_token(gateway: GatewayB58, timestamp: u64, region: Region) -> String {
    let token = Token {
        gateway,
        timestamp,
        region,
        packet_type: PacketType::Join,
    };
    hex::encode(serde_json::to_string(&token).unwrap())
}

pub fn make_data_token(gateway: GatewayB58, timestamp: u64, region: Region) -> String {
    let token = Token {
        gateway,
        timestamp,
        region,
        packet_type: PacketType::Data,
    };
    hex::encode(serde_json::to_string(&token).unwrap())
}
