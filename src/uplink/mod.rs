use helium_crypto::PublicKeyBinary;
use helium_proto::{
    services::router::{envelope_down_v1, EnvelopeDownV1, PacketRouterPacketDownV1},
    Region,
};
use lorawan::parser::{DevAddr as LoraDevAddr, EUI64 as LoraEui};
use semtech_udp::MacAddress;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tokio::sync::mpsc::Sender;
use tonic::Status;

use self::packet::PacketUp;

pub mod ingest;
pub mod packet;

#[derive(Debug, Clone)]
pub struct Gateway {
    pub b58: GatewayB58,
    pub mac: GatewayMac,
    pub region: Region,
    pub tx: GatewayTx,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct PacketHash(pub String);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize)]
pub struct Eui(pub String);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize)]
pub struct DevAddr(pub String);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct GatewayB58(pub String);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize)]
pub struct GatewayMac(pub String);

#[derive(Debug, Clone)]
pub struct GatewayTx(pub Sender<Result<EnvelopeDownV1, Status>>);

impl Gateway {
    pub fn new(packet: &PacketUp, downlink_sender: Sender<Result<EnvelopeDownV1, Status>>) -> Self {
        Self {
            b58: packet.gateway_b58(),
            mac: packet.gateway_mac(),
            region: packet.region(),
            tx: GatewayTx(downlink_sender),
        }
    }
}
impl GatewayTx {
    pub async fn send_downlink(&self, downlink: PacketRouterPacketDownV1) {
        let _ = self
            .0
            .send(Ok(EnvelopeDownV1 {
                data: Some(envelope_down_v1::Data::Packet(downlink)),
            }))
            .await;
    }
}
impl From<LoraEui<&[u8]>> for Eui {
    fn from(value: LoraEui<&[u8]>) -> Self {
        let slice = value.as_ref();

        let mut reversed = Vec::with_capacity(slice.len());
        for &byte in slice.iter().rev() {
            reversed.push(byte);
        }

        let rev_eui = LoraEui::new(reversed).unwrap();
        Self(rev_eui.to_string())
    }
}
impl From<LoraDevAddr<&[u8]>> for DevAddr {
    fn from(value: LoraDevAddr<&[u8]>) -> Self {
        Self(value.to_string())
    }
}

impl From<&Vec<u8>> for GatewayB58 {
    fn from(value: &Vec<u8>) -> Self {
        let b58 = PublicKeyBinary::from(&value[..]).to_string();
        Self(b58)
    }
}

impl From<&Vec<u8>> for GatewayMac {
    fn from(value: &Vec<u8>) -> Self {
        let hash = xxhash_rust::xxh64::xxh64(&value[1..], 0);
        let hash = hash.to_be_bytes();
        Self(hex::encode(hash))
    }
}

impl From<GatewayB58> for GatewayMac {
    fn from(value: GatewayB58) -> Self {
        let data = PublicKeyBinary::from_str(&value.0).expect("valid b58");
        let hash = xxhash_rust::xxh64::xxh64(data.as_ref(), 0);
        let hash = hash.to_be_bytes();
        Self(hex::encode(hash))
    }
}

impl From<GatewayMac> for MacAddress {
    fn from(value: GatewayMac) -> Self {
        MacAddress::from_str(&value.0).expect("mac address from GatewayMac")
    }
}

impl From<&str> for PacketHash {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}
