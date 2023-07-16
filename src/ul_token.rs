use crate::uplink::GatewayB58;
use helium_proto::Region;
use hex::FromHex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Token {
    pub timestamp: u64,
    pub gateway: GatewayB58,
    pub region: Region,
}

impl FromHex for Token {
    type Error = anyhow::Error;

    fn from_hex<T: AsRef<[u8]>>(hex: T) -> std::result::Result<Self, Self::Error> {
        let s = hex::decode(hex).unwrap();
        Ok(serde_json::from_slice(&s[..]).unwrap())
    }
}

pub fn make_token(gateway: GatewayB58, timestamp: u64, region: Region) -> String {
    let token = Token {
        gateway,
        timestamp,
        region,
    };
    hex::encode(serde_json::to_string(&token).unwrap())
}
