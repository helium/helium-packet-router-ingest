use crate::{settings::RoamingSettings, ul_token::Token, uplink::GatewayB58, Result};
use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};
use std::collections::HashMap;

pub type TransactionID = u64;

pub trait PacketDownTrait {
    fn payload(&self) -> Vec<u8> {
        hex::decode(self.phy_payload()).expect("encoded payload")
    }
    fn phy_payload(&self) -> String;
    fn gateway(&self) -> GatewayB58;
    fn to_packet_down(&self) -> PacketRouterPacketDownV1;
    fn transaction_id(&self) -> TransactionID;
    fn http_body(&self, settings: &RoamingSettings) -> Option<String>;
}

pub type PacketDown = Box<dyn PacketDownTrait + Send + Sync>;

impl core::fmt::Debug for PacketDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PacketDown")
    }
}

#[derive(Debug)]
pub enum HttpPayloadResp {
    Downlink(Box<dyn PacketDownTrait + Send + Sync>),
    Noop,
}

pub fn parse_http_payload(value: serde_json::Value) -> Result<HttpPayloadResp> {
    use serde_json::from_value;

    if let Ok(pr_start) = from_value::<PRStartAnsDownlink>(value.clone()) {
        return Ok(HttpPayloadResp::Downlink(Box::new(pr_start)));
    }
    if let Ok(xmit) = from_value::<XmitDataReq>(value.clone()) {
        return Ok(HttpPayloadResp::Downlink(Box::new(xmit)));
    }
    if let Ok(_plain) = from_value::<PRStartAnsPlain>(value.clone()) {
        return Ok(HttpPayloadResp::Noop);
    }

    tracing::warn!(?value, "could not parse");
    anyhow::bail!("unparseable message");
}

pub fn mhz_to_hz(mhz: f64) -> u32 {
    // NOTE: f64 is important, if it goes down to f32 we start to see rounding errors.
    (mhz * 1_000_000.0) as u32
}

/// Timestamp value needs to be truncated into u32 space
fn add_delay(timestamp: u64, add: u64) -> u64 {
    ((timestamp + add) as u32) as u64
}

impl PacketDownTrait for PRStartAnsDownlink {
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
                .rx
                .to_rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
            rx2: self
                .dl_meta_data
                .rx
                .to_rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
        }
    }

    fn transaction_id(&self) -> TransactionID {
        self.transaction_id
    }

    /// PRStartReq was a join_request,
    /// PRStartAns was a join_accept,
    /// PRStartNotif, the join_accept was forwarded to the gateway.
    fn http_body(&self, settings: &RoamingSettings) -> Option<String> {
        Some(
            serde_json::to_string(&serde_json::json!({
                "ProtocolVersion": "1.1",
                "SenderID": settings.helium_net_id,
                "ReceiverID": settings.target_net_id,
                "TransactionID": self.transaction_id,
                "MessageType": "PRStartNotif",
                "SenderNSID": settings.sender_nsid,
                "ReceiverNSID": settings.receiver_nsid,
                "Result": {"ResultCode": "Success"}
            }))
            .expect("pr_start_notif json"),
        )
    }
}

impl PacketDownTrait for XmitDataReq {
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
                .rx
                .to_rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
            rx2: self
                .dl_meta_data
                .rx
                .to_rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
        }
    }

    fn transaction_id(&self) -> TransactionID {
        self.transaction_id
    }

    /// Downlink was received and forwarded to the gateway.
    fn http_body(&self, settings: &RoamingSettings) -> Option<String> {
        Some(
            serde_json::to_string(&serde_json::json!({
                "ProtocolVersion": "1.1",
                "MessageType": "XmitDataAns",
                "SenderID": settings.helium_net_id,
                "ReceiverID": settings.target_net_id,
                "SenderNSID": settings.sender_nsid,
                "ReceiverNSID": settings.receiver_nsid,
                "TransactionID": self.transaction_id,
                "Result": {"ResultCode": "Success"},
            }))
            .expect("xmit_data_ans json"),
        )
    }
}

impl RxWindows {
    fn to_rx1_window(&self, timestamp: u64) -> Option<WindowV1> {
        if let (Some(freq), Some(data_rate), Some(mut rx_delay)) =
            (self.dl_freq_1, self.data_rate_1, self.rx_delay_1)
        {
            if rx_delay < 2 {
                rx_delay = 1;
            }
            return Some(WindowV1 {
                timestamp: add_delay(timestamp, rx_delay * 1_000_000),
                frequency: mhz_to_hz(freq),
                datarate: data_rate.into(),
                immediate: false,
            });
        }
        None
    }
    fn to_rx2_window(&self, timestamp: u64) -> Option<WindowV1> {
        if let (Some(freq), Some(data_rate), Some(mut rx_delay)) =
            (self.dl_freq_2, self.data_rate_2, self.rx_delay_1)
        {
            if rx_delay < 2 {
                rx_delay = 1;
            }
            return Some(WindowV1 {
                timestamp: add_delay(timestamp, (rx_delay + 1) * 1_000_000),
                frequency: mhz_to_hz(freq),
                datarate: data_rate.into(),
                immediate: false,
            });
        }
        None
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct XmitDataReq {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "DLMetaData")]
    pub dl_meta_data: XmitDLMetaData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct XmitDLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "ClassMode")]
    pub class_mode: String,
    #[serde(flatten)]
    pub rx: RxWindows,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RxWindows {
    #[serde(rename = "DLFreq1")]
    pub dl_freq_1: Option<f64>,
    #[serde(rename = "DataRate1")]
    pub data_rate_1: Option<u8>,
    #[serde(rename = "RXDelay1")]
    pub rx_delay_1: Option<u64>,

    #[serde(rename = "DLFreq2")]
    pub dl_freq_2: Option<f64>,
    #[serde(rename = "DataRate2")]
    pub data_rate_2: Option<u8>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsPlain {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Result")]
    pub result: PRStartAnsResult,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsDownlink {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "MessageType")]
    pub message_type: String,
    #[serde(rename = "Result")]
    pub result: PRStartAnsResult,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "DLMetaData")]
    pub dl_meta_data: PRStartDLMetaData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsResult {
    #[serde(rename = "ResultCode")]
    pub result_code: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartDLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FPort")]
    pub f_port: Option<String>,
    #[serde(rename = "FCntDown")]
    pub f_cnt_down: Option<String>,
    #[serde(rename = "Confirmed")]
    pub confirmed: Option<bool>,

    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "GWInfo")]
    pub gw_info: Option<Vec<HashMap<String, Option<String>>>>,
    #[serde(rename = "HiPriorityFlag")]
    pub hi_priority_flag: bool,

    #[serde(flatten)]
    pub rx: RxWindows,
}

#[cfg(test)]
mod test {
    use helium_proto::services::router::WindowV1;

    use super::{PRStartAnsPlain, XmitDataReq};
    use crate::downlink::{parse_http_payload, PRStartAnsDownlink, PacketDownTrait};

    fn join_accept_payload() -> serde_json::Value {
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
                "RXDelay1": 5,
                "ClassMode": "A",
                "DataRate1": 10,
                "DataRate2": 8,
                "FNSULToken": "7b2274696d657374616d70223a343034383533323435322c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
                "GWInfo": [{"FineRecvTime": null,"RSSI": null,"SNR": null,"Lat": null,"Lon": null,"DLAllowed": null}],
              "HiPriorityFlag": false
            },
            "DevAddr": "48000037"
        })
    }

    fn unconfirmed_downlink_payload() -> serde_json::Value {
        serde_json::json!({
            "ProtocolVersion":"1.0",
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
                "FNSULToken":"7b2274696d657374616d70223a31303739333532372c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
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
    fn xmit_data_req_to_packet_down_v1() {
        let value = unconfirmed_downlink_payload();
        let _x = parse_http_payload(value).expect("parseable");
    }

    #[test]
    fn join_accept_to_packet_down_v1() {
        let value = join_accept_payload();
        let pr_start: PRStartAnsDownlink = serde_json::from_value(value).expect("to packet down");
        // println!("packet: {pr_start:#?}");
        let down = pr_start.to_packet_down();
        // println!("down: {down:#?}");
        assert_eq!(
            pr_start.dl_meta_data.fns_ul_token.timestamp + 5_000_000,
            down.rx1.expect("rx1_window").timestamp
        );
        assert_eq!(
            pr_start.dl_meta_data.fns_ul_token.timestamp + 6_000_000,
            down.rx2.expect("rx2_window").timestamp
        );
    }

    #[test]
    fn mic_failed() {
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
        let _packet: PRStartAnsPlain = serde_json::from_value(value).expect("to pr plain");
        // println!("plain: {packet:#?}");
    }

    #[test]
    fn join_accept_payload_decode() {
        let value = join_accept_payload();
        let pr_start: PRStartAnsDownlink = serde_json::from_value(value).expect("to packet down");
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
    fn rx_1_and_2_downlink() {
        let value = serde_json::json!({
            "ProtocolVersion": "1.1",
            "SenderID": "600013",
            "ReceiverID": "c00053",
            "TransactionID": 1234,
            "MessageType": "XmitDataReq",
            "PHYPayload":
                "60c04e26e020000000a754ba934840c3bc120989b532ee4613e06e3dd5d95d9d1ceb9e20b1f2",
            "DLMetaData": {
                "DevEUI": "6081f9c306a777fd",

                "RXDelay1": 1,
                "DLFreq1": 925.1,
                "DataRate1": 10,

                "DLFreq2": 923.3,
                "DataRate2": 8,

                "FNSULToken": "7b2274696d657374616d70223a31303739333532372c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
                "ClassMode": "A",
                "HiPriorityFlag": false,
                "GWInfo": []
            }
        });
        let xmit: XmitDataReq = serde_json::from_value(value).expect("to xmit");
        let downlink = xmit.to_packet_down();
        assert_eq!(
            Some(WindowV1 {
                timestamp: xmit.dl_meta_data.fns_ul_token.timestamp + 1_000_000,
                frequency: 925100000,
                datarate: 10,
                immediate: false
            }),
            downlink.rx1
        );
        assert!(downlink.rx1.is_some());
        assert!(downlink.rx2.is_some());
    }

    #[test]
    fn join_accept_rx2_only() {
        let value = serde_json::json!({"ProtocolVersion":"1.1","SenderNSID":"f03d290000000101","ReceiverNSID":"6081fffe12345678","SenderID":"600013","ReceiverID":"c00053","TransactionID":1152841626,"MessageType":"PRStartAns","Result":{"ResultCode":"Success"},"Lifetime":0,"DevEUI":"0018b20000002487","SenderToken":"0108f03d290000000101","PHYPayload":"202cf4a93d978c060233bbaa88d20f48673136ea147f5ad92e8b015a581a8d74cc","DLMetaData":{"DevEUI":"0018b20000002487","RXDelay1":5,"DLFreq2":869.525,"DataRate2":0,"FNSULToken":"7b2274696d657374616d70223a31303739333532372c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d","ClassMode":"A","HiPriorityFlag":false}});
        let pr_start: PRStartAnsDownlink = serde_json::from_value(value).expect("to pr_start");

        let downlink = pr_start.to_packet_down();
        assert!(downlink.rx1.is_none());
        assert!(downlink.rx2.is_some());
    }
}
