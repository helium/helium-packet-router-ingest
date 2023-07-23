use self::ul_token::Token;
use crate::{region, settings::ProtocolVersion};

pub mod downlink;
pub mod ul_token;
pub mod uplink;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "MessageType")]
pub struct PRStartReq {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "SenderNSID")]
    pub sender_nsid: String,
    #[serde(rename = "ReceiverNSID")]
    pub receiver_nsid: String,
    #[serde(rename = "DedupWindowSize")]
    pub dedup_window_size: String,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "PHYPayload")]
    pub phy_payload: String,
    #[serde(rename = "ULMetaData")]
    pub ul_meta_data: ULMetaData,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ULMetaData {
    #[serde(rename = "DevAddr")]
    pub devaddr: Option<String>,
    #[serde(rename = "DevEUI")]
    pub dev_eui: Option<String>,
    #[serde(rename = "DataRate")]
    pub data_rate: region::DR,
    #[serde(rename = "ULFreq")]
    pub ul_freq: f64,
    #[serde(rename = "RecvTime")]
    pub recv_time: String,
    #[serde(rename = "RFRegion")]
    pub rf_region: region::Region,
    #[serde(rename = "FNSULToken")]
    pub fns_ul_token: String,
    #[serde(rename = "GWCnt")]
    pub gw_cnt: usize,
    #[serde(rename = "GWInfo")]
    pub gw_info: Vec<GWInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GWInfo {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "RFRegion")]
    pub region: region::Region,
    #[serde(rename = "RSSI")]
    pub rssi: i32,
    #[serde(rename = "SNR")]
    pub snr: f32,
    #[serde(rename = "DLAllowed")]
    pub dl_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HttpResponse {
    #[serde(rename = "ProtocolVersion")]
    pub protocol_version: String,
    #[serde(rename = "MessageType")]
    pub message_type: HttpResponseMessageType,
    #[serde(rename = "SenderID")]
    pub sender_id: String,
    #[serde(rename = "ReceiverID")]
    pub receiver_id: String,
    #[serde(rename = "TransactionID")]
    pub transaction_id: u64,
    #[serde(rename = "SenderNSID")]
    pub sender_nsid: String,
    #[serde(rename = "ReceiverNSID")]
    pub receiver_nsid: String,
    #[serde(rename = "Result")]
    pub result: HttpResponseResult,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum HttpResponseMessageType {
    PRStartNotif,
    XmitDataAns,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "ResultCode")]
pub enum HttpResponseResult {
    Success,
    MICFailed,
    XmitFailed,
}

impl HttpResponse {
    pub fn success(mut self) -> Self {
        self.result = HttpResponseResult::Success;
        self
    }

    pub fn mic_failed(mut self) -> Self {
        self.result = HttpResponseResult::MICFailed;
        self
    }

    pub fn xmit_failed(mut self) -> Self {
        self.result = HttpResponseResult::XmitFailed;
        self
    }

    pub fn should_send_for_protocol(&self, protocol_version: &ProtocolVersion) -> bool {
        match (&self.message_type, protocol_version) {
            (HttpResponseMessageType::PRStartNotif, ProtocolVersion::V1_0) => false,
            (HttpResponseMessageType::PRStartNotif, ProtocolVersion::V1_1) => true,
            (HttpResponseMessageType::XmitDataAns, ProtocolVersion::V1_0) => true,
            (HttpResponseMessageType::XmitDataAns, ProtocolVersion::V1_1) => true,
        }
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
    pub dl_meta_data: DLMetaData,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PRStartAns {
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
    pub dl_meta_data: DLMetaData,
}

/// This type exists to parse a PRStartAns that contains no downlink,
/// rather than making DLMetaData optional.
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
pub struct PRStartAnsResult {
    #[serde(rename = "ResultCode")]
    pub result_code: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub enum ClassMode {
    A,
    C,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct DLMetaData {
    #[serde(rename = "DevEUI")]
    pub dev_eui: String,
    #[serde(rename = "FNSULToken", with = "hex::serde")]
    pub fns_ul_token: Token,
    #[serde(rename = "ClassMode")]
    pub class_mode: ClassMode,

    // rx windows
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
