use crate::uplink::packet::PacketUp;
use helium_proto::{
    services::router::{PacketRouterPacketDownV1, WindowV1},
    DataRate,
};
use semtech_udp::{
    push_data::{self, CRC},
    MacAddress,
};
use std::str::FromStr;

pub fn to_packet_down(value: &semtech_udp::pull_resp::TxPk) -> PacketRouterPacketDownV1 {
    let datarate = DataRate::from_str(&value.datr.to_string()).expect("valid downlink datarate");
    PacketRouterPacketDownV1 {
        payload: value.data.as_ref().to_owned(),
        rx1: Some(WindowV1 {
            timestamp: match value.time.is_immediate() {
                true => 0,
                false => match value.time.tmst() {
                    Some(tmst) => tmst as u64,
                    None => 0,
                },
            },
            frequency: (value.freq * 1_000_000.0) as u32,
            datarate: datarate.into(),
            immediate: value.time.is_immediate(),
        }),
        rx2: None,
    }
}

impl From<PacketUp> for push_data::Packet {
    fn from(value: PacketUp) -> Self {
        let gateway_mac: MacAddress = value.gateway_mac().into();
        let freq = value.frequency_mhz();
        let packet = value.packet;
        let datarate =
            semtech_udp::DataRate::from_str(&packet.datarate().to_string()).expect("something");
        let payload = packet.payload;

        let rxpk = vec![push_data::RxPk::V1(push_data::RxPkV1 {
            chan: 0,
            codr: semtech_udp::CodingRate::_4_5,
            // FIXME: need to base64?
            size: payload.iter().len() as u64,
            data: payload,
            datr: datarate,
            freq,
            lsnr: packet.snr,
            modu: semtech_udp::Modulation::LORA,
            rfch: 0,
            rssi: packet.rssi,
            rssis: None,
            stat: CRC::OK,
            tmst: packet.timestamp as u32,
            time: None,
        })];
        // FIXME: Stat needs to be extended to support regi key
        Self {
            gateway_mac,
            random_token: rand::random(),
            data: push_data::Data {
                rxpk: Some(rxpk),
                stat: None,
            },
        }
    }
}
