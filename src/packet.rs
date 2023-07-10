pub use packet_down::PacketDown;
pub use packet_up::{PacketHash, PacketUp, RoutingInfo};

mod packet_up {

    use helium_crypto::PublicKeyBinary;
    use helium_proto::services::router::PacketRouterPacketUpV1;
    use lorawan::parser::{DataHeader, PhyPayload, EUI64};

    #[derive(Debug, PartialEq, Eq, Hash, Clone)]
    pub struct PacketHash(pub String);

    #[derive(Debug, Clone)]
    pub struct PacketUp {
        packet: PacketRouterPacketUpV1,
        recv_time: u64,
    }

    type Eui = String;
    type DevAddr = String;
    type GatewayB58 = String;

    impl PacketUp {
        pub fn gateway_b58(&self) -> GatewayB58 {
            PublicKeyBinary::from(&self.packet.gateway[..]).to_string()
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
                        let devaddr = phy.fhdr().dev_addr().to_string();
                        RoutingInfo::devaddr(devaddr)
                    }
                    lorawan::parser::DataPayload::Decrypted(_) => RoutingInfo::Unknown,
                },
            }
        }

        pub fn gateway_mac_str(&self) -> String {
            let hash = xxhash_rust::xxh64::xxh64(&self.packet.gateway[1..], 0);
            let hash = hash.to_be_bytes();
            hex::encode(hash)
        }

        pub fn region(&self) -> String {
            self.packet.region().to_string()
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

        pub fn datarate_index(&self) -> u32 {
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

        pub fn frequency_mhz(&self) -> f64 {
            // Truncate -> Round -> Truncate
            let freq = (self.packet.frequency as f64) / 1_000.0;
            freq.round() / 1_000.0
        }

        /// This is the time the NS received the packet.
        pub fn recv_time(&self) -> String {
            use chrono::{DateTime, NaiveDateTime, Utc};
            let dt = DateTime::<Utc>::from_utc(
                NaiveDateTime::from_timestamp_millis(self.recv_time as i64)
                    .expect("valid timestamp"),
                Utc,
            );
            dt.to_rfc3339()
        }

        pub fn timestamp(&self) -> u64 {
            self.packet.timestamp
        }
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

    #[cfg(test)]
    mod test {
        use crate::packet::packet_up::RoutingInfo;
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
}

mod packet_down {
    use crate::{
        roaming::{PRStartAnsDownlink, XmitDataReq},
        Result,
    };
    use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};

    #[derive(Debug)]
    pub enum PacketDown {
        JoinAccept(Box<PRStartAnsDownlink>),
        Downlink(Box<XmitDataReq>),
    }

    impl PacketDown {
        pub fn gateway(&self) -> String {
            match self {
                Self::JoinAccept(pr_start) => pr_start.dl_meta_data.fns_ul_token.gateway.clone(),
                Self::Downlink(xmit) => xmit.dl_meta_data.fns_ul_token.gateway.clone(),
            }
        }

        pub fn transaction_id(&self) -> u64 {
            match self {
                Self::JoinAccept(pr_start) => pr_start.transaction_id,
                Self::Downlink(xmit) => xmit.transaction_id,
            }
        }

        pub fn payload(&self) -> Vec<u8> {
            hex::decode(match self {
                PacketDown::JoinAccept(inner) => inner.phy_payload.clone(),
                PacketDown::Downlink(inner) => inner.phy_payload.clone(),
            })
            .expect("hex decode downlink payload")
        }

        fn to_windows(&self) -> Result<(Option<WindowV1>, Option<WindowV1>)> {
            match (self.rx1(), self.rx2()) {
                (None, None) => anyhow::bail!("downlink contains no rx windows"),
                windows => Ok(windows),
            }
        }

        fn rx1(&self) -> Option<WindowV1> {
            match self {
                PacketDown::JoinAccept(value) => {
                    let freq1 = value.dl_meta_data.dl_freq_1;
                    let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 5_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_1.into(),
                        frequency: freq1,
                        immediate: false,
                    })
                }
                PacketDown::Downlink(value) => {
                    let freq1 = value.dl_meta_data.dl_freq_1;
                    let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 5_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_1.into(),
                        frequency: freq1,
                        immediate: false,
                    })
                }
            }
        }

        fn rx2(&self) -> Option<WindowV1> {
            match self {
                PacketDown::JoinAccept(value) => {
                    let freq2 = value.dl_meta_data.dl_freq_2;
                    let freq2 = (freq2 * 1_000_000.0) as u32; // mHz -> Hz
                    Some(WindowV1 {
                        timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 6_000_000) as u32)
                            as u64,
                        datarate: value.dl_meta_data.data_rate_2.into(),
                        frequency: freq2,
                        immediate: false,
                    })
                }
                PacketDown::Downlink(_value) => None,
            }
        }

        pub fn as_ref(&self) -> &Self {
            self
        }
    }

    impl From<PRStartAnsDownlink> for PacketDown {
        fn from(value: PRStartAnsDownlink) -> Self {
            Self::JoinAccept(Box::new(value))
        }
    }

    impl From<XmitDataReq> for PacketDown {
        fn from(value: XmitDataReq) -> Self {
            Self::Downlink(Box::new(value))
        }
    }

    impl From<PacketDown> for PacketRouterPacketDownV1 {
        fn from(value: PacketDown) -> Self {
            let (rx1, rx2) = value.to_windows().expect("valid rx windows");
            Self {
                payload: value.payload(),
                rx1,
                rx2,
            }
        }
    }
}
