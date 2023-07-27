use super::{DevAddr, Eui, GatewayB58, GatewayMac, PacketHash};
use crate::region;
use helium_proto::services::router::PacketRouterPacketUpV1;
use helium_proto::Region;
use lorawan::parser::EUI64;
use lorawan::parser::{DataHeader, PhyPayload};

#[derive(Debug, Clone)]
pub struct PacketUp {
    pub packet: PacketRouterPacketUpV1,
    pub recv_time: u64,
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

impl PacketUp {
    pub fn new(packet: PacketRouterPacketUpV1, recv_time: u64) -> Self {
        Self { packet, recv_time }
    }

    pub fn gateway_b58(&self) -> GatewayB58 {
        let gw = &self.packet.gateway;
        gw.into()
    }

    pub fn gateway_mac(&self) -> GatewayMac {
        let gw = &self.packet.gateway;
        gw.into()
    }

    pub fn hash(&self) -> PacketHash {
        use sha2::{Digest, Sha256};
        PacketHash(String::from_utf8_lossy(&Sha256::digest(&self.packet.payload)).into())
    }

    pub fn routing_info(&self) -> RoutingInfo {
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
                    let devaddr = phy.fhdr().dev_addr();
                    RoutingInfo::devaddr(devaddr)
                }
                lorawan::parser::DataPayload::Decrypted(_) => RoutingInfo::Unknown,
            },
        }
    }

    pub fn region(&self) -> Region {
        self.packet.region()
    }

    pub fn rssi(&self) -> i32 {
        self.packet.rssi
    }

    pub fn snr(&self) -> f32 {
        self.packet.snr
    }

    pub fn json_payload(&self) -> String {
        hex::encode(&self.packet.payload)
    }

    pub fn datarate_index(&self) -> region::DR {
        region::uplink_datarate(self.region(), self.packet.datarate())
            .expect("valid uplink datarate index")
    }

    pub fn frequency_mhz(&self) -> f64 {
        hz_to_mhz(self.packet.frequency)
    }

    /// This is the time the NS received the packet.
    pub fn recv_time(&self) -> String {
        use chrono::{DateTime, NaiveDateTime, Utc};
        let dt = DateTime::<Utc>::from_utc(
            NaiveDateTime::from_timestamp_millis(self.recv_time as i64).expect("valid timestamp"),
            Utc,
        );
        dt.to_rfc3339()
    }

    pub fn timestamp(&self) -> u64 {
        self.packet.timestamp
    }
}

#[derive(Debug, PartialEq)]
pub enum RoutingInfo {
    Eui { app: Eui, dev: Eui },
    DevAddr(DevAddr),
    Unknown,
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
            app: app.into(),
            dev: dev.into(),
        }
    }
    pub fn devaddr(devaddr: lorawan::parser::DevAddr<&[u8]>) -> Self {
        Self::DevAddr(devaddr.into())
    }
}

#[cfg(test)]
mod test {
    use lorawan::parser::PhyPayload;

    use crate::uplink::{packet::RoutingInfo, Eui};

    #[test]
    fn eui_parse() {
        let bytes = [
            0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 196, 160, 173, 225, 146, 91,
        ];

        if let PhyPayload::JoinRequest(request) = lorawan::parser::parse(bytes).unwrap() {
            assert_eq!(
                RoutingInfo::Eui {
                    app: Eui("0000000000000000".to_string()),
                    dev: Eui("0000000000000003".to_string())
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
