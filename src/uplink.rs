use helium_crypto::PublicKeyBinary;
use helium_proto::services::router::PacketRouterPacketUpV1;
use lorawan::parser::EUI64;
use lorawan::parser::{DataHeader, PhyPayload};

pub trait PacketUpTrait {
    fn gateway_b58(&self) -> GatewayB58;
    fn hash(&self) -> PacketHash;
    fn routing_info(&self) -> RoutingInfo;
    fn gateway_mac_str(&self) -> String;
    fn rssi(&self) -> i32;
    fn snr(&self) -> f32;
    fn region(&self) -> String;
    fn json_payload(&self) -> String;
    fn datarate_index(&self) -> u32;
    fn frequency_mhz(&self) -> f32;
    fn recv_time(&self) -> String;
    fn timestamp(&self) -> u64;
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct PacketHash(pub String);
pub type Eui = String;
pub type DevAddr = String;
pub type GatewayB58 = String;

/// HTTP Roaming uses mHz, proto uses Hz
pub fn mhz_to_hz(mhz: f32) -> u32 {
    (mhz * 1_000_000.0) as u32
}

pub fn hz_to_mhz(hz: u32) -> f32 {
    // Truncate -> Round -> Truncate
    let freq = (hz as f32) / 1_000.0;
    freq.round() / 1_000.0
}

#[derive(Debug, PartialEq)]
pub enum RoutingInfo {
    Eui { app: Eui, dev: Eui },
    DevAddr(DevAddr),
    Unknown,
}

impl RoutingInfo {
    pub fn eui(app: EUI64<&[u8]>, dev: EUI64<&[u8]>) -> Self {
        Self::Eui {
            app: EUI64::new(flip_endianness(app.as_ref()))
                .unwrap()
                .to_string(),
            dev: EUI64::new(flip_endianness(dev.as_ref()))
                .unwrap()
                .to_string(),
        }
    }
    pub fn devaddr(devaddr: DevAddr) -> Self {
        Self::DevAddr(devaddr)
    }
}

fn flip_endianness(slice: &[u8]) -> Vec<u8> {
    let mut flipped = Vec::with_capacity(slice.len());
    for &byte in slice.iter().rev() {
        flipped.push(byte);
    }
    flipped
}

#[derive(Debug, Clone)]
pub struct PacketUp {
    packet: PacketRouterPacketUpV1,
    recv_time: u64,
}

impl From<PacketRouterPacketUpV1> for PacketUp {
    fn from(value: PacketRouterPacketUpV1) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Self {
            packet: value,
            recv_time: now,
        }
    }
}

impl PacketUpTrait for PacketUp {
    fn gateway_b58(&self) -> GatewayB58 {
        PublicKeyBinary::from(&self.packet.gateway[..]).to_string()
    }

    fn hash(&self) -> PacketHash {
        use sha2::{Digest, Sha256};
        PacketHash(String::from_utf8_lossy(&Sha256::digest(&self.packet.payload)).into())
    }

    fn routing_info(&self) -> RoutingInfo {
        let payload = &self.packet.payload;
        tracing::trace!(?payload, "payload");
        match lorawan::parser::parse(payload.clone()).expect("valid packet") {
            PhyPayload::JoinAccept(_) => RoutingInfo::Unknown,
            PhyPayload::JoinRequest(request) => {
                let app = request.app_eui();
                let dev = request.dev_eui();
                RoutingInfo::eui(app, dev)
            }
            PhyPayload::Data(payload) => match payload {
                lorawan::parser::DataPayload::Encrypted(phy) => {
                    let devaddr = phy.fhdr().dev_addr().to_string();
                    RoutingInfo::devaddr(devaddr)
                }
                lorawan::parser::DataPayload::Decrypted(_) => RoutingInfo::Unknown,
            },
        }
    }

    fn gateway_mac_str(&self) -> String {
        let hash = xxhash_rust::xxh64::xxh64(&self.packet.gateway[1..], 0);
        let hash = hash.to_be_bytes();
        hex::encode(hash)
    }

    fn region(&self) -> String {
        self.packet.region().to_string()
    }

    fn rssi(&self) -> i32 {
        self.packet.rssi
    }

    fn snr(&self) -> f32 {
        self.packet.snr
    }

    fn json_payload(&self) -> String {
        hex::encode(&self.packet.payload)
    }

    fn datarate_index(&self) -> u32 {
        // FIXME: handle different region
        use helium_proto::DataRate;
        match self.packet.datarate() {
            DataRate::Sf12bw125 => todo!(),
            DataRate::Sf11bw125 => todo!(),
            DataRate::Sf10bw125 => 0,
            DataRate::Sf9bw125 => 1,
            DataRate::Sf8bw125 => 2,
            DataRate::Sf7bw125 => 3,
            DataRate::Sf12bw250 => 8,
            DataRate::Sf11bw250 => 9,
            DataRate::Sf10bw250 => 10,
            DataRate::Sf9bw250 => 11,
            DataRate::Sf8bw250 => 12,
            DataRate::Sf7bw250 => 13,
            DataRate::Sf12bw500 => todo!(),
            DataRate::Sf11bw500 => todo!(),
            DataRate::Sf10bw500 => todo!(),
            DataRate::Sf9bw500 => todo!(),
            DataRate::Sf8bw500 => 4,
            DataRate::Sf7bw500 => todo!(),
            DataRate::Lrfhss1bw137 => todo!(),
            DataRate::Lrfhss2bw137 => todo!(),
            DataRate::Lrfhss1bw336 => todo!(),
            DataRate::Lrfhss2bw336 => todo!(),
            DataRate::Lrfhss1bw1523 => 5,
            DataRate::Lrfhss2bw1523 => 6,
            DataRate::Fsk50 => todo!(),
        }
    }

    fn frequency_mhz(&self) -> f32 {
        hz_to_mhz(self.packet.frequency)
    }

    /// This is the time the NS received the packet.
    fn recv_time(&self) -> String {
        use chrono::{DateTime, NaiveDateTime, Utc};
        let dt = DateTime::<Utc>::from_utc(
            NaiveDateTime::from_timestamp_millis(self.recv_time as i64).expect("valid timestamp"),
            Utc,
        );
        dt.to_rfc3339()
    }

    fn timestamp(&self) -> u64 {
        self.packet.timestamp
    }
}

#[cfg(test)]
mod test {
    use crate::uplink::RoutingInfo;
    use lorawan::parser::PhyPayload;

    #[test]
    fn eui_parse() {
        let bytes = [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 196, 160, 173, 225, 146, 91,
        ];

        if let PhyPayload::JoinRequest(request) = lorawan::parser::parse(bytes).unwrap() {
            assert_eq!(
                RoutingInfo::Eui {
                    app: "0000000000000000".to_string(),
                    dev: "0000000000000003".to_string()
                },
                RoutingInfo::eui(request.app_eui(), request.dev_eui())
            );
        } else {
            assert!(false);
        }
    }
}
