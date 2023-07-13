use crate::{
    settings::Settings,
    uplink::{mhz_to_hz, GatewayB58, PacketUp, PacketUpTrait, RoutingInfo},
    Result,
};
use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};
use hex::FromHex;
use std::collections::HashMap;

pub type TransactionID = u64;

pub trait PacketDownTrait {
    fn gateway(&self) -> GatewayB58;
    fn to_packet_down(&self) -> PacketRouterPacketDownV1;
    fn transaction_id(&self) -> TransactionID;
    fn http_body(&self, settings: &Settings) -> Option<String>;
}

pub type PacketDown = Box<dyn PacketDownTrait + Send + Sync>;

impl core::fmt::Debug for PacketDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PacketDownTraitable")
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

/// PRStartReq was a join_request,
/// PRStartAns was a join_accept,
/// PRStartNotif, the join_accept was forwarded to the gateway.
pub fn make_pr_start_notif(transaction_id: TransactionID, http_config: &Settings) -> String {
    serde_json::to_string(&serde_json::json!({
        "ProtocolVersion": "1.1",
        "SenderID": http_config.helium_net_id,
        "ReceiverID": http_config.target_net_id,
        "TransactionID": transaction_id,
        "MessageType": "PRStartNotif",
        "SenderNSID": http_config.sender_nsid,
        "ReceiverNSID": http_config.receiver_nsid,
        "Result": {"ResultCode": "Success"}
    }))
    .expect("pr_start_notif json")
}

/// Downlink was received and forwarded to the gateway.
pub fn make_xmit_data_ans(xmit: &XmitDataReq, http_config: &Settings) -> String {
    serde_json::to_string(&serde_json::json!({
        "ProtocolVersion": "1.1",
        "MessageType": "XmitDataAns",
        "SenderID": http_config.helium_net_id,
        "ReceiverID": http_config.target_net_id,
        "SenderNSID": http_config.sender_nsid,
        "ReceiverNSID": http_config.receiver_nsid,
        "TransactionID": xmit.transaction_id,
        "Result": {"ResultCode": "Success"},
        "DLFreq1": xmit.dl_meta_data.dl_freq_1
    }))
    .expect("xmit_data_ans json")
}

/// Uplinks
pub fn make_pr_start_req(packets: Vec<PacketUp>, config: &Settings) -> Result<String> {
    let packet = packets.first().expect("at least one packet");

    let (routing_key, routing_value) = match packet.routing_info() {
        RoutingInfo::Eui { dev, .. } => ("DevEUI", dev),
        RoutingInfo::DevAddr(devaddr) => ("DevAddr", devaddr),
        RoutingInfo::Unknown => todo!("should never get here"),
    };

    let mut gw_info = vec![];
    for packet in packets.iter() {
        gw_info.push(serde_json::json!({
            "ID": packet.gateway_mac_str(),
            "RFRegion": packet.region(),
            "RSSI": packet.rssi(),
            "SNR": packet.snr(),
            "DLAllowed": true
        }));
    }

    Ok(serde_json::to_string(&serde_json::json!({
        "ProtocolVersion" : "1.1",
        "MessageType": "PRStartReq",
        "SenderNSID": config.sender_nsid,
        "ReceiverNSID": config.receiver_nsid,
        "DedupWindowSize": config.dedup_window.to_string(),
        "SenderID": config.helium_net_id,
        "ReceiverID": config.target_net_id,
        "PHYPayload": packet.json_payload(),
        "ULMetaData": {
            routing_key: routing_value,
            "DataRate": packet.datarate_index(),
            "ULFreq": packet.frequency_mhz(),
            "RecvTime": packet.recv_time(),
            "RFRegion": packet.region(),
            "FNSULToken": make_token(packet.gateway_b58(), packet.timestamp()),
            "GWCnt": packets.len(),
            "GWInfo": gw_info
        }
    }))
    .expect("pr_start_req json"))
}

impl PacketDownTrait for PRStartAnsDownlink {
    fn gateway(&self) -> GatewayB58 {
        self.dl_meta_data.fns_ul_token.gateway.clone()
    }

    fn to_packet_down(&self) -> PacketRouterPacketDownV1 {
        PacketRouterPacketDownV1 {
            payload: self.phy_payload.clone().into(),
            rx1: self
                .dl_meta_data
                .to_rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
            rx2: self
                .dl_meta_data
                .to_rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
        }
    }

    fn transaction_id(&self) -> TransactionID {
        self.transaction_id
    }

    fn http_body(&self, settings: &Settings) -> Option<String> {
        Some(make_pr_start_notif(self.transaction_id, settings))
    }
}

impl PacketDownTrait for XmitDataReq {
    fn gateway(&self) -> GatewayB58 {
        self.dl_meta_data.fns_ul_token.gateway.clone()
    }

    fn to_packet_down(&self) -> PacketRouterPacketDownV1 {
        PacketRouterPacketDownV1 {
            payload: self.phy_payload.clone().into(),
            rx1: self
                .dl_meta_data
                .to_rx1_window(self.dl_meta_data.fns_ul_token.timestamp),
            rx2: self
                .dl_meta_data
                .to_rx2_window(self.dl_meta_data.fns_ul_token.timestamp),
        }
    }

    fn transaction_id(&self) -> TransactionID {
        self.transaction_id
    }

    fn http_body(&self, settings: &Settings) -> Option<String> {
        Some(make_xmit_data_ans(self, settings))
    }
}

/// Timestamp value needs to be truncated into u32 space
fn add_delay(timestamp: u64, add: u64) -> u64 {
    ((timestamp + add) as u32) as u64
}

impl PRStartAnsDLMetaData {
    fn to_rx1_window(&self, timestamp: u64) -> Option<WindowV1> {
        // Join 1 window
        Some(WindowV1 {
            timestamp: add_delay(timestamp, 5_000_000),
            frequency: mhz_to_hz(self.dl_freq_1),
            datarate: self.data_rate_1.into(),
            immediate: false,
        })
    }

    fn to_rx2_window(&self, timestamp: u64) -> Option<WindowV1> {
        // Join 2 window
        Some(WindowV1 {
            timestamp: add_delay(timestamp, 6_000_000),
            frequency: mhz_to_hz(self.dl_freq_2),
            datarate: self.data_rate_2.into(),
            immediate: false,
        })
    }
}

impl DLMetaData {
    fn to_rx1_window(&self, timestamp: u64) -> Option<WindowV1> {
        // Join 1 window
        Some(WindowV1 {
            timestamp: add_delay(timestamp, 5_000_000),
            frequency: mhz_to_hz(self.dl_freq_1),
            datarate: self.data_rate_1.into(),
            immediate: false,
        })
    }

    fn to_rx2_window(&self, _timestamp: u64) -> Option<WindowV1> {
        None
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Token {
    pub timestamp: u64,
    pub gateway: GatewayB58,
}

impl FromHex for Token {
    type Error = anyhow::Error;

    fn from_hex<T: AsRef<[u8]>>(hex: T) -> std::result::Result<Self, Self::Error> {
        let s = hex::decode(hex).unwrap();
        Ok(serde_json::from_slice(&s[..]).unwrap())
    }
}

fn make_token(gateway: GatewayB58, timestamp: u64) -> String {
    let token = Token { gateway, timestamp };
    hex::encode(serde_json::to_string(&token).unwrap())
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
    pub dl_meta_data: DLMetaData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "DLFreq1")]
    pub dl_freq_1: f32,
    #[serde(rename = "DataRate1")]
    pub data_rate_1: u8,
    #[serde(rename = "RXDelay1")]
    pub rx_delay_1: u8,
    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "ClassMode")]
    pub class_mode: String,
    #[serde(rename = "HiPriorityFlag")]
    pub high_priority: bool,
    #[serde(rename = "GWInfo")]
    pub gw_info: Vec<GWInfo>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GWInfo {
    #[serde(rename = "ULToken")]
    pub ul_token: Option<String>,
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
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FCntUp")]
    pub f_cnt_up: u32,
    #[serde(rename = "DLMetaData")]
    pub dl_meta_data: PRStartAnsDLMetaData,
    #[serde(rename = "DevAddr")]
    pub dev_addr: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsResult {
    #[serde(rename = "ResultCode")]
    pub result_code: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAnsDLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FPort")]
    pub f_port: Option<String>,
    #[serde(rename = "FCntDown")]
    pub f_cnt_down: Option<String>,
    #[serde(rename = "Confirmed")]
    pub confirmed: bool,
    #[serde(rename = "DLFreq1")]
    pub dl_freq_1: f32,
    #[serde(rename = "DLFreq2")]
    pub dl_freq_2: f32,
    #[serde(rename = "RXDelay1")]
    pub rx_delay_1: u8,
    #[serde(rename = "ClassMode")]
    pub class_mode: String,
    #[serde(rename = "DataRate1")]
    pub data_rate_1: u8,
    #[serde(rename = "DataRate2")]
    pub data_rate_2: u8,
    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "GWInfo")]
    pub gw_info: Vec<HashMap<String, Option<String>>>,
    #[serde(rename = "HiPriorityFlag")]
    pub hi_priority_flag: bool,
}

#[cfg(test)]
mod test {
    use super::PRStartAnsPlain;
    use crate::downlink::{parse_http_payload, PRStartAnsDownlink};

    #[test]
    fn xmit_data_req_to_packet_down_v1() {
        let value = serde_json::json!({
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
        );
        let x = parse_http_payload(value).expect("parseable");
        println!("{x:?}");

        // let xmit: XmitDataReq = serde_json::from_value(value).unwrap();
        // println!("xmit: {xmit:#?}");
        // let packet: PacketDown = xmit.into();
        // let down: PacketRouterPacketDownV1 = packet.into();
        // println!("down: {down:#?}");
    }

    #[test]
    fn join_accept_to_packet_down_v1() {
        let value = serde_json::json!( {
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
        });
        let pr_start: PRStartAnsDownlink = serde_json::from_value(value).expect("to packet down");
        println!("packet: {pr_start:#?}");
        // let packet: PacketDown = pr_start.into();
        // let down: PacketRouterPacketDownV1 = packet.into();
        // println!("down: {down:#?}");
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
        let packet: PRStartAnsPlain = serde_json::from_value(value).expect("to pr plain");
        println!("plain: {packet:#?}");
    }
}
