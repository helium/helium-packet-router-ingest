use crate::{
    protocol::{PRStartAns, PRStartAnsPlain, XmitDataReq},
    region::downlink_datarate,
    settings::RoamingSettings,
    Result,
};
use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};

use super::{
    ul_token::PacketType, uplink::GatewayB58, ClassMode, DLMetaData, HttpResponse,
    HttpResponseMessageType, HttpResponseResult,
};

#[derive(Debug, Clone, PartialEq)]
pub struct PacketDown {
    pub downlink: PacketRouterPacketDownV1,
    pub gateway_b58: GatewayB58,
    pub http_response: HttpResponse,
}

pub fn parse_http_payload(
    value: serde_json::Value,
    settings: &RoamingSettings,
) -> Result<Option<PacketDown>> {
    use serde_json::from_value;

    if let Ok(pr_start) = from_value::<PRStartAns>(value.clone()) {
        return Ok(Some(PacketDown {
            downlink: pr_start.to_packet_down(),
            gateway_b58: pr_start.gateway(),
            http_response: pr_start.http_body(settings),
        }));
    }
    if let Ok(xmit) = from_value::<XmitDataReq>(value.clone()) {
        return Ok(Some(PacketDown {
            downlink: xmit.to_packet_down(),
            gateway_b58: xmit.gateway(),
            http_response: xmit.http_body(settings),
        }));
    }
    if let Ok(plain) = from_value::<PRStartAnsPlain>(value.clone()) {
        tracing::trace!(?plain, "no downlink");
        return Ok(None);
    }

    tracing::warn!(?value, "could not parse");
    anyhow::bail!("unparseable message");
}

pub trait ToPacketDown {
    fn payload(&self) -> Vec<u8> {
        hex::decode(self.phy_payload()).expect("encoded payload")
    }
    fn phy_payload(&self) -> String;
    fn gateway(&self) -> GatewayB58;
    fn to_packet_down(&self) -> PacketRouterPacketDownV1;
    fn http_body(&self, settings: &RoamingSettings) -> HttpResponse;
}

impl ToPacketDown for PRStartAns {
    fn phy_payload(&self) -> String {
        self.phy_payload.clone()
    }

    fn gateway(&self) -> GatewayB58 {
        self.dl_meta_data.fns_ul_token.gateway.clone()
    }

    fn to_packet_down(&self) -> PacketRouterPacketDownV1 {
        PacketRouterPacketDownV1 {
            payload: self.payload(),
            rx1: self
                .dl_meta_data
                .rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
            rx2: self
                .dl_meta_data
                .rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
        }
    }

    /// PRStartReq was a join_request,
    /// PRStartAns was a join_accept,
    /// PRStartNotif, the join_accept was forwarded to the gateway.
    fn http_body(&self, settings: &RoamingSettings) -> HttpResponse {
        HttpResponse {
            protocol_version: "1.1".to_string(),
            sender_id: settings.helium_net_id.clone(),
            receiver_id: settings.target_net_id.clone(),
            transaction_id: self.transaction_id,
            message_type: HttpResponseMessageType::PRStartNotif,
            sender_nsid: settings.sender_nsid.clone(),
            receiver_nsid: settings.receiver_nsid.clone(),
            result: HttpResponseResult::Success,
        }
    }
}

impl ToPacketDown for XmitDataReq {
    fn phy_payload(&self) -> String {
        self.phy_payload.clone()
    }

    fn gateway(&self) -> GatewayB58 {
        self.dl_meta_data.fns_ul_token.gateway.clone()
    }

    fn to_packet_down(&self) -> PacketRouterPacketDownV1 {
        match self.dl_meta_data.class_mode {
            ClassMode::C => PacketRouterPacketDownV1 {
                payload: self.payload(),
                rx1: self.dl_meta_data.class_c_window(),
                rx2: None,
            },
            ClassMode::A => PacketRouterPacketDownV1 {
                payload: self.payload(),
                rx1: self
                    .dl_meta_data
                    .rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
                rx2: self
                    .dl_meta_data
                    .rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
            },
        }
    }

    /// Downlink was received and forwarded to the gateway.
    fn http_body(&self, settings: &RoamingSettings) -> HttpResponse {
        HttpResponse {
            protocol_version: "1.1".to_string(),
            sender_id: settings.helium_net_id.clone(),
            receiver_id: settings.target_net_id.clone(),
            transaction_id: self.transaction_id,
            message_type: HttpResponseMessageType::XmitDataAns,
            sender_nsid: settings.sender_nsid.clone(),
            receiver_nsid: settings.receiver_nsid.clone(),
            result: HttpResponseResult::Success,
        }
    }
}

impl DLMetaData {
    fn datarate(&self, dr: u8) -> i32 {
        downlink_datarate(self.fns_ul_token.region, dr)
            .expect("valid dr")
            .into()
    }

    fn rx_delay(&self) -> Option<u64> {
        match self.fns_ul_token.packet_type {
            PacketType::Join => Some(5),
            PacketType::Data => self.rx_delay_1,
        }
    }

    fn rx1_window(&self, timestamp: u64) -> Option<WindowV1> {
        if let (Some(freq), Some(data_rate), Some(mut rx_delay)) =
            (self.dl_freq_1, self.data_rate_1, self.rx_delay())
        {
            if rx_delay < 2 {
                rx_delay = 1;
            }
            return Some(WindowV1 {
                timestamp: add_delay(timestamp, rx_delay * 1_000_000),
                frequency: mhz_to_hz(freq),
                datarate: self.datarate(data_rate),
                immediate: false,
            });
        }
        None
    }

    fn rx2_window(&self, timestamp: u64) -> Option<WindowV1> {
        if let (Some(freq), Some(data_rate), Some(mut rx_delay)) =
            (self.dl_freq_2, self.data_rate_2, self.rx_delay())
        {
            if rx_delay < 2 {
                rx_delay = 1;
            }
            return Some(WindowV1 {
                timestamp: add_delay(timestamp, (rx_delay + 1) * 1_000_000),
                frequency: mhz_to_hz(freq),
                datarate: self.datarate(data_rate),
                immediate: false,
            });
        }
        None
    }

    fn class_c_window(&self) -> Option<WindowV1> {
        if let (Some(freq), Some(data_rate)) = (self.dl_freq_2, self.data_rate_2) {
            return Some(WindowV1 {
                timestamp: 0,
                frequency: mhz_to_hz(freq),
                datarate: self.datarate(data_rate),
                immediate: true,
            });
        }
        None
    }
}

pub fn mhz_to_hz(mhz: f64) -> u32 {
    // NOTE: f64 is important, if it goes down to f32 we start to see rounding errors.
    (mhz * 1_000_000.0) as u32
}

fn add_delay(timestamp: u64, add: u64) -> u64 {
    // Timestamp value needs to be truncated into u32 space
    ((timestamp + add) as u32) as u64
}

#[cfg(test)]
mod test {
    use super::PacketDown;
    use crate::{
        protocol::{
            downlink::ToPacketDown,
            ul_token::{make_data_token, make_join_token},
            PRStartAns,
        },
        region::{downlink_datarate, Region},
        settings::RoamingSettings,
        Result,
    };
    use duration_string::DurationString;
    use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};
    use std::time::Duration;

    impl Default for RoamingSettings {
        fn default() -> Self {
            Self {
                helium_net_id: Default::default(),
                target_net_id: Default::default(),
                sender_nsid: Default::default(),
                receiver_nsid: Default::default(),
                dedup_window: DurationString::new(Duration::from_millis(1250)),
                send_pr_start_notif: Default::default(),
                authorization_header: Default::default(),
            }
        }
    }

    fn parse_http_payload(value: serde_json::Value) -> Result<Option<PacketDown>> {
        super::parse_http_payload(value, &RoamingSettings::default())
    }

    fn join_accept_payload() -> serde_json::Value {
        let token = make_join_token("test-gateway".to_string(), 100, Region::Us915);
        serde_json::json!({
            "ProtocolVersion": "1.1",
            "SenderID": "000024",
            "ReceiverID": "c00053",
            "TransactionID":  193858937,
            "MessageType": "PRStartAns",
            "Result": {"ResultCode": "Success"},
            "PHYPayload": "209c7848d681b589da4b8e5544460a693383bb7e5d47b849ef0d290bafb20872ae",
            "DevEUI": "0000000000000003",
            "Lifetime": null,
            "FNwkSIntKey": null,
            "NwkSKey": {
                "KEKLabel": "",
                "AESKey": "0e2baf26327308e63afe62be15edea6a"
            },
            "FCntUp": 0,
            "ServiceProfile": null,
            "DLMetaData": {
                "DevEUI": "0000000000000003",
                "FPort": null,
                "FCntDown": null,
                "Confirmed": false,
                "DLFreq1": 925.1,
                "DLFreq2": 923.3,
                "RXDelay1": 3,
                "ClassMode": "A",
                "DataRate1": 10,
                "DataRate2": 8,
                "FNSULToken": token,
                "GWInfo": [{"FineRecvTime": null,"RSSI": null,"SNR": null,"Lat": null,"Lon": null,"DLAllowed": null}],
              "HiPriorityFlag": false
            },
            "DevAddr": "48000037"
        })
    }

    fn unconfirmed_downlink_payload() -> serde_json::Value {
        let token = make_data_token("test-gateway".to_string(), 100, Region::Us915);
        serde_json::json!({
            "ProtocolVersion":"1.1",
            "SenderID":"000024",
            "ReceiverID":"c00053",
            "TransactionID":274631693,
            "MessageType":"XmitDataReq",
            "PHYPayload":"6073000048ab00000300020070030000ff01063d32ce60",
            "ULMetaData":null,
            "DLMetaData":{
                "DevEUI":"0000000000000003",
                "FPort":null,
                "FCntDown":null,
                "Confirmed":false,
                "DLFreq1":926.9,
                "DLFreq2":923.3,
                "RXDelay1":1,
                "ClassMode":"A",
                "DataRate1":10,
                "DataRate2":8,
                "FNSULToken":token,
                "GWInfo":[{
                    "FineRecvTime":null,
                    "RSSI":null,
                    "SNR":null,
                    "Lat":null,
                    "Lon":null,
                    "DLAllowed":null
                }],
                "HiPriorityFlag":false}
            }
        )
    }

    #[test]
    fn mic_failed_response() {
        let value = serde_json::json!({
            "ProtocolVersion": "1.1",
            "SenderID": "000024",
            "ReceiverID": "c00053",
            "TransactionID": 517448448,
            "MessageType": "PRStartAns",
            "Result": {
                "ResultCode": "MICFailed",
                "Description": "Invalid MIC"
            },
            "Lifetime": null,
            "FNwkSIntKey": null,
            "NwkSKey": null,
            "FCntUp": null,
            "ServiceProfile": null,
            "DLMetaData": null
        });
        assert!(parse_http_payload(value).expect("parseable").is_none());
    }

    #[test]
    fn join_accept_payload_decode() {
        let value = join_accept_payload();
        let pr_start: PRStartAns = serde_json::from_value(value).expect("to packet down");
        let downlink = pr_start.to_packet_down();

        // This ensures downlink payloads are decoded, and not just turned into Vec<u8>
        assert_eq!(
            [
                32, 156, 120, 72, 214, 129, 181, 137, 218, 75, 142, 85, 68, 70, 10, 105, 51, 131,
                187, 126, 93, 71, 184, 73, 239, 13, 41, 11, 175, 178, 8, 114, 174
            ],
            lorawan::parser::parse(downlink.payload)
                .expect("parse valid join accept")
                .as_ref()
        );
    }

    #[test]
    fn join_accept_rx_1_and_2() {
        let value = join_accept_payload();
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("Not a downlink") };

        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    32, 156, 120, 72, 214, 129, 181, 137, 218, 75, 142, 85, 68, 70, 10, 105, 51,
                    131, 187, 126, 93, 71, 184, 73, 239, 13, 41, 11, 175, 178, 8, 114, 174
                ],
                rx1: Some(WindowV1 {
                    timestamp: 5000100,
                    frequency: 925100000,
                    datarate: downlink_datarate(Region::Us915, 10).unwrap().into(),
                    immediate: false
                }),
                rx2: Some(WindowV1 {
                    timestamp: 6000100,
                    frequency: 923300000,
                    datarate: downlink_datarate(Region::Us915, 8).unwrap().into(),
                    immediate: false
                })
            },
            packet_down.downlink
        );
    }

    #[test]
    fn join_accept_rx1_only() {
        let token = make_join_token("test-gateway".to_string(), 100, Region::Eu868);
        let value = serde_json::json!({"ProtocolVersion":"1.1","SenderNSID":"f03d290000000101","ReceiverNSID":"6081fffe12345678","SenderID":"600013","ReceiverID":"c00053","TransactionID":1152841626,"MessageType":"PRStartAns","Result":{"ResultCode":"Success"},"Lifetime":0,"DevEUI":"0018b20000002487","SenderToken":"0108f03d290000000101","PHYPayload":"202cf4a93d978c060233bbaa88d20f48673136ea147f5ad92e8b015a581a8d74cc","DLMetaData":{"DevEUI":"0018b20000002487","RXDelay1":5,"DLFreq1":869.525,"DataRate1":0,"FNSULToken":token,"ClassMode":"A","HiPriorityFlag":false}});
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("not a downlink") };
        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    32, 44, 244, 169, 61, 151, 140, 6, 2, 51, 187, 170, 136, 210, 15, 72, 103, 49,
                    54, 234, 20, 127, 90, 217, 46, 139, 1, 90, 88, 26, 141, 116, 204
                ],
                rx1: Some(WindowV1 {
                    timestamp: 5000100,
                    frequency: 869525000,
                    datarate: downlink_datarate(Region::Eu868, 0).unwrap().into(),
                    immediate: false
                }),
                rx2: None,
            },
            packet_down.downlink
        );
    }

    #[test]
    fn join_accept_rx2_only() {
        let token = make_join_token("test-gateway".to_string(), 100, Region::Eu868);
        let value = serde_json::json!({"ProtocolVersion":"1.1","SenderNSID":"f03d290000000101","ReceiverNSID":"6081fffe12345678","SenderID":"600013","ReceiverID":"c00053","TransactionID":1152841626,"MessageType":"PRStartAns","Result":{"ResultCode":"Success"},"Lifetime":0,"DevEUI":"0018b20000002487","SenderToken":"0108f03d290000000101","PHYPayload":"202cf4a93d978c060233bbaa88d20f48673136ea147f5ad92e8b015a581a8d74cc","DLMetaData":{"DevEUI":"0018b20000002487","RXDelay1":5,"DLFreq2":869.525,"DataRate2":0,"FNSULToken":token,"ClassMode":"A","HiPriorityFlag":false}});
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("not a downlink") };
        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    32, 44, 244, 169, 61, 151, 140, 6, 2, 51, 187, 170, 136, 210, 15, 72, 103, 49,
                    54, 234, 20, 127, 90, 217, 46, 139, 1, 90, 88, 26, 141, 116, 204
                ],
                rx1: None,
                rx2: Some(WindowV1 {
                    timestamp: 6000100,
                    frequency: 869525000,
                    datarate: downlink_datarate(Region::Eu868, 0).unwrap().into(),
                    immediate: false
                })
            },
            packet_down.downlink
        );
    }

    #[test]
    fn xmit_rx_1_and_2() {
        let value = unconfirmed_downlink_payload();
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("Not a downlink") };

        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    96, 115, 0, 0, 72, 171, 0, 0, 3, 0, 2, 0, 112, 3, 0, 0, 255, 1, 6, 61, 50, 206,
                    96,
                ],
                rx1: Some(WindowV1 {
                    timestamp: 1000100,
                    frequency: 926900000,
                    datarate: downlink_datarate(Region::Us915, 10).unwrap().into(),
                    immediate: false,
                }),
                rx2: Some(WindowV1 {
                    timestamp: 2000100,
                    frequency: 923300000,
                    datarate: downlink_datarate(Region::Us915, 8).unwrap().into(),
                    immediate: false,
                }),
            },
            packet_down.downlink
        );
    }

    #[test]
    fn xmit_rx1_only() {
        let token = make_data_token("test-gateway".to_string(), 100, Region::Us915);
        let value = serde_json::json!({ "ProtocolVersion":"1.1", "SenderID":"000024", "ReceiverID":"c00053", "TransactionID":274631693, "MessageType":"XmitDataReq", "PHYPayload":"6073000048ab00000300020070030000ff01063d32ce60", "ULMetaData":null, "DLMetaData":{ "DevEUI":"0000000000000003", "FPort":null, "FCntDown":null, "Confirmed":false, "DLFreq1":926.9, "RXDelay1":1, "ClassMode":"A", "DataRate1":10, "FNSULToken":token, "GWInfo":[{ "FineRecvTime":null, "RSSI":null, "SNR":null, "Lat":null, "Lon":null, "DLAllowed":null }], "HiPriorityFlag":false} } );
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("Not a downlink") };

        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    96, 115, 0, 0, 72, 171, 0, 0, 3, 0, 2, 0, 112, 3, 0, 0, 255, 1, 6, 61, 50, 206,
                    96,
                ],
                rx1: Some(WindowV1 {
                    timestamp: 1000100,
                    frequency: 926900000,
                    datarate: downlink_datarate(Region::Us915, 10).unwrap().into(),
                    immediate: false,
                }),
                rx2: None,
            },
            packet_down.downlink
        );
    }

    #[test]
    fn xmit_x2_only() {
        let token = make_data_token("test-gateway".to_string(), 100, Region::Us915);
        let value = serde_json::json!({ "ProtocolVersion":"1.1", "SenderID":"000024", "ReceiverID":"c00053", "TransactionID":274631693, "MessageType":"XmitDataReq", "PHYPayload":"6073000048ab00000300020070030000ff01063d32ce60", "ULMetaData":null, "DLMetaData":{ "DevEUI":"0000000000000003", "FPort":null, "FCntDown":null, "Confirmed":false, "DLFreq2":923.3, "RXDelay1":1, "ClassMode":"A", "DataRate2":8, "FNSULToken":token, "GWInfo":[{ "FineRecvTime":null, "RSSI":null, "SNR":null, "Lat":null, "Lon":null, "DLAllowed":null }], "HiPriorityFlag":false} } );
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("Not a downlink") };

        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    96, 115, 0, 0, 72, 171, 0, 0, 3, 0, 2, 0, 112, 3, 0, 0, 255, 1, 6, 61, 50, 206,
                    96,
                ],
                rx1: None,
                rx2: Some(WindowV1 {
                    timestamp: 2000100,
                    frequency: 923300000,
                    datarate: downlink_datarate(Region::Us915, 8).unwrap().into(),
                    immediate: false,
                }),
            },
            packet_down.downlink
        );
    }

    #[test]
    fn xmit_class_c() {
        let token = make_data_token("test-gateway".to_string(), 100, Region::Us915);
        let value = serde_json::json!({
            "ProtocolVersion":"1.1",
            "SenderID":"000024",
            "ReceiverID":"c00053",
            "TransactionID":274631693,
            "MessageType":"XmitDataReq",
            "PHYPayload":"6073000048ab00000300020070030000ff01063d32ce60",
            "ULMetaData":null,
            "DLMetaData":{
                "DevEUI":"0000000000000003",
                "FPort":null,
                "FCntDown":null,
                "Confirmed":false,
                "DLFreq2":923.3,
                "RXDelay1":1,
                "ClassMode":"C",
                "DataRate2":8,
                "FNSULToken":token,
                "GWInfo":[{
                    "FineRecvTime":null,
                    "RSSI":null,
                    "SNR":null,
                    "Lat":null,
                    "Lon":null,
                    "DLAllowed":null
                }],
                "HiPriorityFlag":false}
            }
        );
        let Some(packet_down) = parse_http_payload(value).expect("parseable") else { panic!("Not a downlink") };

        assert_eq!(
            PacketRouterPacketDownV1 {
                payload: vec![
                    96, 115, 0, 0, 72, 171, 0, 0, 3, 0, 2, 0, 112, 3, 0, 0, 255, 1, 6, 61, 50, 206,
                    96,
                ],
                rx1: Some(WindowV1 {
                    timestamp: 0,
                    frequency: 923300000,
                    datarate: downlink_datarate(Region::Us915, 8).unwrap().into(),
                    immediate: true,
                }),
                rx2: None,
            },
            packet_down.downlink
        );
    }
}
