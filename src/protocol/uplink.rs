use crate::region;
use crate::{settings::RoamingSettings, Result};
use helium_crypto::PublicKeyBinary;
use helium_proto::services::router::PacketRouterPacketUpV1;
use helium_proto::Region;
use lorawan::parser::EUI64;
use lorawan::parser::{DataHeader, PhyPayload};

use super::ul_token::{make_data_token, make_join_token};
use super::{GWInfo, PRStartReq, ULMetaData};

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct PacketHash(pub String);
pub type Eui = String;
pub type DevAddr = String;
pub type GatewayB58 = String;

#[derive(Debug, Clone)]
pub struct PacketUp {
    packet: PacketRouterPacketUpV1,
    recv_time: u64,
}

impl PacketUp {
    pub fn new(packet: PacketRouterPacketUpV1, recv_time: u64) -> Self {
        Self { packet, recv_time }
    }
}

impl From<PacketRouterPacketUpV1> for PacketUp {
    fn from(value: PacketRouterPacketUpV1) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        Self::new(value, now)
    }
}

#[derive(Debug, PartialEq)]
pub enum RoutingInfo {
    Eui { app: Eui, dev: Eui },
    DevAddr(DevAddr),
    Unknown,
}

pub trait PacketUpTrait {
    fn gateway_b58(&self) -> GatewayB58;
    fn hash(&self) -> PacketHash;
    fn routing_info(&self) -> RoutingInfo;
    fn gateway_mac_str(&self) -> String;
    fn rssi(&self) -> i32;
    fn snr(&self) -> f32;
    fn region(&self) -> Region;
    fn json_payload(&self) -> String;
    fn datarate_index(&self) -> region::DR;
    fn frequency_mhz(&self) -> f64;
    fn recv_time(&self) -> String;
    fn timestamp(&self) -> u64;
}

/// Uplinks
pub fn make_pr_start_req(packets: &[PacketUp], config: &RoamingSettings) -> Result<PRStartReq> {
    let packet = packets.first().expect("at least one packet");

    let (devaddr, dev_eui, token) = match packet.routing_info() {
        RoutingInfo::Eui { dev, .. } => (
            None,
            Some(dev),
            make_join_token(packet.gateway_b58(), packet.timestamp(), packet.region()),
        ),
        RoutingInfo::DevAddr(devaddr) => (
            Some(devaddr),
            None,
            make_data_token(packet.gateway_b58(), packet.timestamp(), packet.region()),
        ),
        RoutingInfo::Unknown => todo!("should never get here"),
    };

    let mut gw_info = vec![];
    for packet in packets.iter() {
        gw_info.push(GWInfo {
            id: packet.gateway_mac_str(),
            region: packet.region(),
            rssi: packet.rssi(),
            snr: packet.snr(),
            dl_allowed: true,
        });
    }

    Ok(PRStartReq {
        protocol_version: "1.1".to_string(),
        sender_nsid: config.sender_nsid.to_owned(),
        receiver_nsid: config.receiver_nsid.to_owned(),
        dedup_window_size: config.dedup_window.to_string(),
        sender_id: config.helium_net_id.to_owned(),
        receiver_id: config.target_net_id.to_owned(),
        phy_payload: packet.json_payload(),
        ul_meta_data: ULMetaData {
            devaddr,
            dev_eui,
            data_rate: packet.datarate_index(),
            ul_freq: packet.frequency_mhz(),
            recv_time: packet.recv_time(),
            rf_region: packet.region(),
            fns_ul_token: token,
            gw_cnt: gw_info.len(),
            gw_info,
        },
    })
}

pub fn hz_to_mhz(hz: u32) -> f64 {
    // NOTE: f64 is important, if it goes down to f32 we start to see rounding errors.
    // Truncate -> Round -> Truncate
    let freq = hz / 1_000;
    freq as f64 / 1_000.0
}

impl RoutingInfo {
    pub fn eui(app: EUI64<&[u8]>, dev: EUI64<&[u8]>) -> Self {
        Self::Eui {
            app: EUI64::new(Self::reversed(app.as_ref()))
                .unwrap()
                .to_string(),
            dev: EUI64::new(Self::reversed(dev.as_ref()))
                .unwrap()
                .to_string(),
        }
    }
    pub fn devaddr(devaddr: DevAddr) -> Self {
        Self::DevAddr(devaddr)
    }

    fn reversed(slice: &[u8]) -> Vec<u8> {
        let mut flipped = Vec::with_capacity(slice.len());
        for &byte in slice.iter().rev() {
            flipped.push(byte);
        }
        flipped
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

    fn region(&self) -> Region {
        self.packet.region()
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

    fn datarate_index(&self) -> region::DR {
        region::uplink_datarate(self.region(), self.packet.datarate())
            .expect("valid uplink datarate index")
    }

    fn frequency_mhz(&self) -> f64 {
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

impl From<&str> for PacketHash {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(test)]
mod test {
    use lorawan::parser::PhyPayload;

    use crate::protocol::uplink::RoutingInfo;

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

    #[test]
    fn join_accept_parse() {
        let bytes =
            hex::decode("20aaf0dbee7ea66c06c5b16d4d1aa23557eab691b9bbb22864831aaa2832d9d9c0")
                .unwrap();
        let _x = lorawan::parser::parse(bytes);
        // println!("x: {_x:?}");
    }
}
