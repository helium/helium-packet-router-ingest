use clap::Parser;

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Commands {
    Serve(HttpConfig),
}

#[derive(Default, Debug, Clone, clap::Args)]
pub struct HttpConfig {
    /// Identity of the NS, unique to HTTP forwarder
    #[arg(long, default_value = "6081FFFE12345678")]
    pub sender_nsid: String,
    /// How long were the packets held in ms
    #[arg(long, default_value = "1250")]
    pub dedupe_window_size: u64,
    /// Helium forwarding NetID, for LNSs to identify which network to back through for downlinking.
    /// (default: C00053)
    #[arg(long, default_value = "C00053")]
    pub helium_net_id: String,
    /// NetID of the network operating the http forwarder
    #[arg(long, default_value = "000000")]
    pub target_net_id: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let cli = Cli::parse();
    tracing::info!(?cli, "opts");

    match cli.command {
        Commands::Serve(http_config) => server::run(http_config).await,
    }
}

mod server {
    use crate::{
        deduplicator, grpc, http,
        packet_info::{PacketHash, PacketMeta, RoutingInfo},
        HttpConfig, Result,
    };
    use helium_proto::services::router::{
        envelope_down_v1, EnvelopeDownV1, PacketRouterPacketDownV1, PacketRouterPacketUpV1,
    };
    use hex::FromHex;
    use std::{collections::HashMap, str::FromStr, time::Duration};
    use tokio::sync::mpsc::Sender;
    use tonic::Status;

    #[derive(Debug, Clone)]
    pub struct MsgSender(Sender<Msg>);

    type GW = String;

    /// This message contains the entirety of the lifecycle of routing packets from hpr through http.
    ///
    /// At a gateways first packet, we register a downlink handler handler, the other side of the incoming stream.
    /// Once a gateway is known packets will be ingested normally.
    ///
    /// They are sent to the deduplicator, which will hold onto packets by hash until the DedupWindow has elapsed and the message `DedupDone` is sent.
    /// The packets will be turned into their JSON payload and sent over to the roaming party.
    ///
    /// After `cleanup_timeout` has elapsed, a packet_hash will cleanup after itself.
    /// Setting this too short may result in double delivery of packet groups.
    /// Too long may lead to memory issues.
    #[derive(Debug)]
    pub enum Msg {
        /// First contact with a gateway. Use the Sender for Downlinks.
        GrpcConnect(GW, Sender<Result<EnvelopeDownV1, Status>>),
        /// A packet has been received
        GrpcUplink(PacketRouterPacketUpV1),
        /// A gateway stream has ended
        GrpcDisconnect(GW),
        ///
        HttpDownlink(GW, PacketRouterPacketDownV1),
        /// The DedupWindow has elapsed for a PacketHash.s
        DedupDone(PacketHash),
        /// The Cleanup window has elapsed for a PacketHash.
        DedupCleanup(PacketHash),
    }

    impl MsgSender {
        pub async fn dedup_done(&self, key: String) {
            self.0.send(Msg::DedupDone(key)).await.expect("dedup_done");
        }

        pub async fn dedup_cleanup(&self, key: String) {
            self.0
                .send(Msg::DedupCleanup(key))
                .await
                .expect("dedup_cleanup");
        }

        pub async fn gateway_connect(
            &self,
            packet: &PacketRouterPacketUpV1,
            downlink_sender: Sender<Result<EnvelopeDownV1, Status>>,
        ) {
            self.0
                .send(Msg::GrpcConnect(
                    packet.gateway_b58(),
                    downlink_sender.clone(),
                ))
                .await
                .expect("gateway_connect");
        }

        pub async fn gateway_disconnect(&self, gateway: GW) {
            self.0
                .send(Msg::GrpcDisconnect(gateway))
                .await
                .expect("gateway_disconnect");
        }

        pub async fn uplink(&self, packet: PacketRouterPacketUpV1) {
            self.0.send(Msg::GrpcUplink(packet)).await.expect("uplink");
        }

        pub async fn downlink(&self, gw: GW, packet: PacketRouterPacketDownV1) {
            self.0
                .send(Msg::HttpDownlink(gw, packet))
                .await
                .expect("downlink");
        }
    }

    pub async fn run(http_config: HttpConfig) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(512);

        let mut deduplicator = deduplicator::Deduplicator::new(
            MsgSender(tx.clone()),
            Duration::from_millis(1250),
            Duration::from_secs(10),
        );

        let reader_thread = tokio::spawn(async move {
            let mut downlink_map = HashMap::new();
            tracing::info!("reading thread started");
            while let Some(msg) = rx.recv().await {
                match msg {
                    Msg::GrpcConnect(gw, chan) => {
                        let inserted = downlink_map.insert(gw, chan);
                        tracing::info!(
                            inserted = inserted.is_none(),
                            size = downlink_map.len(),
                            "gateway connect"
                        );
                    }
                    Msg::GrpcDisconnect(gw) => {
                        let removed = downlink_map.remove(&gw);
                        tracing::info!(
                            removed = removed.is_some(),
                            size = downlink_map.len(),
                            "gateway disconnect"
                        );
                    }
                    Msg::GrpcUplink(packet) => {
                        deduplicator.handle_packet(packet);
                    }
                    Msg::HttpDownlink(gw, downlink) => match downlink_map.get(&gw) {
                        Some(sender) => {
                            let _ = sender
                                .send(Ok(EnvelopeDownV1 {
                                    data: Some(envelope_down_v1::Data::Packet(downlink)),
                                }))
                                .await;
                            tracing::info!(gw, "downlink sent");
                        }
                        None => {
                            tracing::warn!(?gw, ?downlink_map, "cannot downlink to unknown gateway")
                        }
                    },
                    Msg::DedupDone(hash) => {
                        let packets = deduplicator.get_packets(&hash);
                        tracing::info!(num_packets = packets.len(), "deduplication done");
                        match make_payload(packets, &http_config) {
                            Ok(body) => {
                                let local_chirpstack_url = "http://127.0.0.1:9005";
                                let res = reqwest::Client::new()
                                    .post(local_chirpstack_url)
                                    // serde_json::to_string_pretty(&body).unwrap()
                                    .body(serde_json::to_string(&body).expect("turning into json"))
                                    .send()
                                    .await;
                                tracing::info!(?body, ?local_chirpstack_url, ?res, "uplink",);
                            }
                            Err(_) => {
                                tracing::warn!("ignoring invalid packet");
                            }
                        };
                    }
                    Msg::DedupCleanup(hash) => {
                        deduplicator.remove_packets(&hash);
                    }
                }
            }
        });

        let http_thread = http::start(MsgSender(tx.clone()));
        let grpc_thread = grpc::start(MsgSender(tx.clone()));

        let _ = tokio::try_join!(grpc_thread, reader_thread, http_thread);
    }

    // #[instrument]
    fn make_payload(
        packets: Vec<PacketRouterPacketUpV1>,
        config: &HttpConfig,
    ) -> Result<serde_json::Value> {
        let packet = packets.first().expect("at least one packet");

        let (routing_key, routing_value) = match packet.routing_info() {
            RoutingInfo::Eui { dev, .. } => ("DevEUI", dev),
            RoutingInfo::Devaddr(devaddr) => ("DevAddr", devaddr),
            RoutingInfo::Unknown => todo!("should never get here"),
        };

        let mut gw_info = vec![];
        for packet in packets.iter() {
            gw_info.push(serde_json::json!({
                "ID": packet.gateway_mac_str(),
                "RFRegion": packet.region().to_string(),
                "RSSI": packet.rssi,
                "SNR": packet.snr,
                "DLAllowed": true
            }));
        }

        Ok(serde_json::json!({
            "ProtocolVersion" : "1.1",
            "MessageType": "PRStartReq",
            "SenderNSID": config.sender_nsid,
            "DedupWindowSize": config.dedupe_window_size,
            "SenderID": config.helium_net_id,
            "ReceiverID": config.target_net_id,
            "PHYPayload": hex::encode(&packet.payload),
            "ULMetaData": {
                routing_key: routing_value,
                "DataRate": packet.datarate_index(),
                "ULFreq": packet.frequency_mhz(),
                "RecvTime": packet.recv_time(),
                "RFRegion": packet.region(),
                "FNSULToken": make_token(packet.gateway_b58(), packet.timestamp),
                "GWCnt": packets.len(),
                "GWInfo": gw_info
            }
        }))
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
    pub struct Token {
        pub timestamp: u64,
        pub gateway: GW,
    }

    impl TryFrom<String> for Token {
        type Error = anyhow::Error;

        fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
            let s = hex::decode(value).unwrap();
            Ok(serde_json::from_slice(&s[..]).unwrap())
        }
    }

    impl FromStr for Token {
        type Err = anyhow::Error;

        fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
            let s = hex::decode(s).unwrap();
            Ok(serde_json::from_slice(&s[..]).unwrap())
        }
    }

    impl FromHex for Token {
        type Error = anyhow::Error;

        fn from_hex<T: AsRef<[u8]>>(hex: T) -> std::result::Result<Self, Self::Error> {
            let s = hex::decode(hex).unwrap();
            Ok(serde_json::from_slice(&s[..]).unwrap())
        }
    }

    fn make_token(gateway: GW, timestamp: u64) -> String {
        let token = Token { gateway, timestamp };
        hex::encode(serde_json::to_string(&token).unwrap())
    }
}

mod packet_info {
    use helium_crypto::PublicKey;
    use helium_proto::services::router::PacketRouterPacketUpV1;
    use lorawan::parser::{DataHeader, PhyPayload, EUI64};

    pub type PacketHash = String;
    type Eui = String;

    #[derive(Debug, PartialEq)]
    pub enum RoutingInfo {
        Eui { app: Eui, dev: Eui },
        Devaddr(Eui),
        Unknown,
    }

    impl RoutingInfo {
        fn new_eui(app: EUI64<&[u8]>, dev: EUI64<&[u8]>) -> Self {
            Self::Eui {
                app: EUI64::new(flip_endianness(app.as_ref()))
                    .unwrap()
                    .to_string(),
                dev: EUI64::new(flip_endianness(dev.as_ref()))
                    .unwrap()
                    .to_string(),
            }
        }
    }

    fn flip_endianness(slice: &[u8]) -> Vec<u8> {
        let mut flipped = Vec::with_capacity(slice.len());

        for &byte in slice.iter().rev() {
            flipped.push(byte);
        }

        flipped
    }

    pub trait PacketMeta {
        fn routing_info(&self) -> RoutingInfo;
        fn frequency_mhz(&self) -> f64;
        fn recv_time(&self) -> String;
        fn datarate_index(&self) -> u32;
        fn gateway_b58(&self) -> String;
        fn gateway_mac_str(&self) -> String;
        // fn gateway_mac(&self) -> Result<semtech_udp::MacAddress>;
    }

    impl PacketMeta for PacketRouterPacketUpV1 {
        fn gateway_b58(&self) -> String {
            PublicKey::try_from(&self.gateway[..]).unwrap().to_string()
        }

        fn routing_info(&self) -> RoutingInfo {
            tracing::trace!(?self.payload, "payload");
            match lorawan::parser::parse(self.payload.clone()).expect("valid packet") {
                PhyPayload::JoinAccept(_) => RoutingInfo::Unknown,
                PhyPayload::JoinRequest(request) => {
                    let app = request.app_eui();
                    let dev = request.dev_eui();
                    RoutingInfo::new_eui(app, dev)
                }
                PhyPayload::Data(payload) => match payload {
                    lorawan::parser::DataPayload::Encrypted(phy) => {
                        let devaddr = phy.fhdr().dev_addr().to_string();
                        RoutingInfo::Devaddr(devaddr)
                    }
                    lorawan::parser::DataPayload::Decrypted(_) => RoutingInfo::Unknown,
                },
            }
        }

        fn gateway_mac_str(&self) -> String {
            let hash = xxhash_rust::xxh64::xxh64(&self.gateway[1..], 0);
            let hash = hash.to_be_bytes();
            hex::encode(hash)
        }

        fn frequency_mhz(&self) -> f64 {
            // Truncate -> Round -> Truncate
            let freq = (self.frequency as f64) / 1_000.0;
            freq.round() / 1_000.0
        }

        fn recv_time(&self) -> String {
            // FIXME: This time should be when the NS received the packet, not the time the gw sent it.
            // fn current_timestamp() -> u64 {
            //     SystemTime::now()
            //         .duration_since(UNIX_EPOCH)
            //         .unwrap()
            //         .as_millis() as u64
            // }
            use chrono::{DateTime, NaiveDateTime, Utc};

            let dt = DateTime::<Utc>::from_utc(
                NaiveDateTime::from_timestamp_millis(self.timestamp as i64)
                    .expect("valid timestamp"),
                Utc,
            );
            dt.to_rfc3339()
        }

        fn datarate_index(&self) -> u32 {
            use helium_proto::DataRate;
            match self.datarate() {
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
    }

    mod test {
        #![allow(unused_imports)]
        use crate::packet_info::RoutingInfo;
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
                    RoutingInfo::new_eui(request.app_eui(), request.dev_eui())
                );
            } else {
                assert!(false);
            }
        }
    }
}

mod deduplicator {
    use crate::{packet_info::PacketHash, server::MsgSender};
    use helium_proto::services::router::PacketRouterPacketUpV1;
    use std::{collections::HashMap, time::Duration};

    pub struct Deduplicator {
        // The store of packets
        packets: HashMap<PacketHash, Vec<PacketRouterPacketUpV1>>,
        // Where to send collections of packets that have gone through the dedupe cycle.
        sender: MsgSender,
        /// DedupWindow from Http Roaming Spec
        dedupe_timeout: Duration,
        /// How long to wait before clearing out a packet hash.
        /// This will impact if you receive duplicate packets if set too short.
        cleanup_timeout: Duration,
    }

    impl Deduplicator {
        pub fn new(sender: MsgSender, dedupe_timeout: Duration, cleanup_timeout: Duration) -> Self {
            Self {
                packets: HashMap::new(),
                sender,
                dedupe_timeout,
                cleanup_timeout,
            }
        }

        /// If we've never seen a packet before we will:
        /// - Insert the packet to collect the rest.
        /// - Wait for the DedupWindow, then ask for the packet to be sent.
        /// - Wait for the cleanupWindow, then remove all copies of the packet.
        pub fn handle_packet(&mut self, packet: PacketRouterPacketUpV1) {
            self.packets
                .entry(self.hash(&packet))
                .and_modify(|bucket| bucket.push(packet.clone()))
                .or_insert_with_key(|key| {
                    let sender = self.sender.clone();
                    let key = key.to_owned();
                    let dedupe_timeout = self.dedupe_timeout;
                    let cleanup_timeout = self.cleanup_timeout;
                    tokio::spawn(async move {
                        tokio::time::sleep(dedupe_timeout).await;
                        sender.dedup_done(key.clone()).await;
                        tokio::time::sleep(cleanup_timeout).await;
                        sender.dedup_cleanup(key).await;
                    });
                    vec![packet]
                });
        }

        pub fn get_packets(&mut self, hash: &String) -> Vec<PacketRouterPacketUpV1> {
            self.packets
                .get(hash)
                .expect("packets exist for hash")
                .to_owned()
        }

        pub fn remove_packets(&mut self, hash: &String) {
            self.packets.remove(hash);
        }

        pub fn hash(&self, packet: &PacketRouterPacketUpV1) -> String {
            use sha2::{Digest, Sha256};
            String::from_utf8_lossy(&Sha256::digest(&packet.payload)).into()
        }
    }
}

mod http {
    use std::collections::HashMap;

    use crate::server::{MsgSender, Token};
    use axum::{
        body::Bytes,
        extract,
        response::IntoResponse,
        routing::{get, post, Router},
        Extension,
    };
    use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};
    use reqwest::StatusCode;
    use tracing::instrument;

    #[instrument]
    pub fn start(sender: MsgSender) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let app = Router::new()
                .route("/api/downlink", post(downlink_post))
                .route("/*key", post(catchall_post))
                .route("/", post(catchall_post))
                .route(
                    "/health",
                    get(|| async {
                        tracing::info!("health check");
                        "ok"
                    }),
                )
                .layer(Extension(sender));

            let addr = "0.0.0.0:9002".parse().expect("valid serve addr");
            tracing::info!(addr = "0.0.0.0:9002", "setup");
            axum::Server::bind(&addr)
                .serve(app.into_make_service())
                .await
                .expect("serve http");
        })
    }

    async fn catchall_post(_sender: Extension<MsgSender>, body: Bytes) -> impl IntoResponse {
        tracing::info!(?body, "catch all request");
    }

    async fn downlink_post(
        sender: Extension<MsgSender>,
        extract::Json(downlink): extract::Json<serde_json::Value>,
    ) -> impl IntoResponse {
        tracing::info!(?downlink, "http downlink");
        let downlink: PRStartAns = serde_json::from_value(downlink).unwrap();
        let gateway = downlink.dl_meta_data.fns_ul_token.gateway.clone();
        let packet_down = downlink.into();
        sender.downlink(gateway, packet_down).await;
        (StatusCode::ACCEPTED, "Downlink Accepted")
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct XmitDataReq {
        #[serde(rename = "ProtocolVersion")]
        protocol_version: String,
        #[serde(rename = "SenderID")]
        sender_id: String,
        #[serde(rename = "ReceiverID")]
        receiver_id: String,
        #[serde(rename = "SenderNSID")]
        sender_nsid: String,
        #[serde(rename = "ReceiverNSID")]
        receiver_nsid: String,
        #[serde(rename = "TransactionID")]
        transaction_id: usize,
        #[serde(rename = "MessageType")]
        message_type: String,
        #[serde(rename = "PHYPayload")]
        phy_payload: String,
        #[serde(rename = "DLMetaData")]
        dl_meta_data: DLMetaData,
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct DLMetaData {
        #[serde(rename = "DevEUI")]
        dev_eui: String,
        #[serde(rename = "DLFreq1")]
        dl_freq_1: f32,
        #[serde(rename = "DataRate1")]
        data_rate_1: u8,
        #[serde(rename = "RXDelay1")]
        rx_delay_1: u8,
        #[serde(rename = "FNSULToken", with = "hex::serde")]
        fns_ul_token: Token,
        #[serde(rename = "ClassMode")]
        class_mode: String,
        #[serde(rename = "HiPriorityFlag")]
        high_priority: bool,
        #[serde(rename = "GWInfo")]
        gw_info: Vec<GWInfo>,
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct GWInfo {
        #[serde(rename = "ULToken")]
        ul_token: String,
    }

    impl From<XmitDataReq> for PacketRouterPacketDownV1 {
        fn from(value: XmitDataReq) -> Self {
            let freq = value.dl_meta_data.dl_freq_1;
            let freq = (freq * 1_000_000.0) as u32; // mHz -> Hz
            Self {
                payload: value.phy_payload.into(),
                rx1: Some(WindowV1 {
                    timestamp: 1,
                    datarate: value.dl_meta_data.data_rate_1.into(),
                    frequency: freq,
                    immediate: false,
                }),
                rx2: None,
            }
        }
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct PRStartAns {
        #[serde(rename = "ProtocolVersion")]
        protocol_version: String,
        #[serde(rename = "SenderID")]
        sender_id: String,
        #[serde(rename = "ReceiverID")]
        receiver_id: String,
        #[serde(rename = "TransactionID")]
        transaction_id: u64,
        #[serde(rename = "MessageType")]
        message_type: String,
        #[serde(rename = "Result")]
        result: PRStartAnsResult,
        #[serde(rename = "PHYPayload")]
        phy_payload: String,
        #[serde(rename = "DevEUI")]
        dev_eui: String,
        #[serde(rename = "FCntUp")]
        f_cnt_up: u32,
        #[serde(rename = "DLMetaData")]
        dl_meta_data: PRStartAnsDLMetaData,
        #[serde(rename = "DevAddr")]
        dev_addr: String,
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct PRStartAnsResult {
        #[serde(rename = "ResultCode")]
        result_code: String,
    }

    #[allow(unused)]
    #[derive(Debug, serde::Deserialize)]
    struct PRStartAnsDLMetaData {
        #[serde(rename = "DevEUI")]
        dev_eui: String,
        #[serde(rename = "FPort")]
        f_port: Option<String>,
        #[serde(rename = "FCntDown")]
        f_cnt_down: Option<String>,
        #[serde(rename = "Confirmed")]
        confirmed: bool,
        #[serde(rename = "DLFreq1")]
        dl_freq_1: f32,
        #[serde(rename = "DLFreq2")]
        dl_freq_2: f32,
        #[serde(rename = "RXDelay1")]
        rx_delay_1: u8,
        #[serde(rename = "ClassMode")]
        class_mode: String,
        #[serde(rename = "DataRate1")]
        data_rate_1: u8,
        #[serde(rename = "DataRate2")]
        data_rate_2: u8,
        #[serde(rename = "FNSULToken", with = "hex::serde")]
        fns_ul_token: Token,
        #[serde(rename = "GWInfo")]
        gw_info: Vec<HashMap<String, Option<String>>>,
        #[serde(rename = "HiPriorityFlag")]
        hi_priority_flag: bool,
    }

    impl From<PRStartAns> for PacketRouterPacketDownV1 {
        fn from(value: PRStartAns) -> Self {
            let freq1 = value.dl_meta_data.dl_freq_1;
            let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz

            let freq2 = value.dl_meta_data.dl_freq_2;
            let freq2 = (freq2 * 1_000_000.0) as u32; // mHz -> Hz
            Self {
                payload: value.phy_payload.into(),
                rx1: Some(WindowV1 {
                    timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 5_000_000) as u32)
                        as u64,
                    datarate: value.dl_meta_data.data_rate_1.into(),
                    frequency: freq1,
                    immediate: false,
                }),
                rx2: Some(WindowV1 {
                    timestamp: ((value.dl_meta_data.fns_ul_token.timestamp + 6_000_000) as u32)
                        as u64,
                    datarate: value.dl_meta_data.data_rate_2.into(),
                    frequency: freq2,
                    immediate: false,
                }),
            }
        }
    }

    mod test {
        #[allow(unused)]
        use crate::http::{PRStartAns, XmitDataReq};
        #[allow(unused)]
        use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};

        #[test]
        fn xmit_data_req_to_packet_down_v1() {
            let value = serde_json::json!({
                "ProtocolVersion": "1.1",
                "SenderID": "000042",
                "ReceiverID": "0xC00053",
                "SenderNSID": "downlink-test-body-sender-nsid",
                "ReceiverNSID": "receiver-nsid",
                "TransactionID": 12348675,
                "MessageType": "XmitDataReq",
                "PHYPayload": "this-is-a-payload",
                "DLMetaData": {
                    "DevEUI": "0xaabbffccfeeff001",
                    "DLFreq1": 904.3,
                    "DataRate1": 0,
                    "RXDelay1": 1,
                    "FNSULToken": "7b2274696d657374616d70223a343034383533323435322c2267617465776179223a2231336a6e776e5a594c446777394b64347a7033336379783474424e514a346a4d6f4e76485469467976556b41676b6851557a39227d",
                    "GWInfo": [
                        {"ULToken": "another-token"}
                    ],
                    "ClassMode": "A",
                    "HiPriorityFlag": false
                }
            });
            let packet: XmitDataReq = serde_json::from_value(value).unwrap();
            println!("packet: {packet:#?}");
            let down: PacketRouterPacketDownV1 = packet.into();
            println!("down: {down:#?}");
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
            let packet: PRStartAns = serde_json::from_value(value).expect("to packet down");
            println!("packet: {packet:#?}");
            let down: PacketRouterPacketDownV1 = packet.into();
            println!("down: {down:#?}");
        }
    }
}

mod grpc {

    use crate::packet_info::PacketMeta;
    use crate::server::MsgSender;
    use helium_proto::services::router::{
        envelope_up_v1, packet_server::Packet, packet_server::PacketServer, EnvelopeDownV1,
        EnvelopeUpV1,
    };
    use tokio_stream::wrappers::ReceiverStream;
    use tonic::{Request, Response, Status, Streaming};
    use tracing::instrument;

    type Result<T = (), E = anyhow::Error> = anyhow::Result<T, E>;

    #[instrument]
    pub fn start(sender: MsgSender) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let addr = "0.0.0.0:9000".parse().expect("valid serve addr");
            tonic::transport::Server::builder()
                .add_service(PacketServer::new(Gateways::new(sender)))
                .serve(addr)
                .await
                .unwrap();
        })
    }

    #[derive(Debug)]
    struct Gateways {
        sender: MsgSender,
    }

    impl Gateways {
        pub fn new(sender: MsgSender) -> Self {
            Self { sender }
        }
    }

    #[tonic::async_trait]
    impl Packet for Gateways {
        type routeStream = ReceiverStream<Result<EnvelopeDownV1, Status>>;

        async fn route(
            &self,
            request: Request<Streaming<EnvelopeUpV1>>,
        ) -> Result<Response<Self::routeStream>, Status> {
            let sender = self.sender.clone();
            let mut req = request.into_inner();

            let (downlink_sender, downlink_receiver) = tokio::sync::mpsc::channel(128);
            tracing::info!("connection");

            tokio::spawn(async move {
                let mut gateway = None;
                while let Ok(Some(msg)) = req.message().await {
                    if let Some(envelope_up_v1::Data::Packet(packet)) = msg.data {
                        if gateway.is_none() {
                            sender
                                .gateway_connect(&packet, downlink_sender.clone())
                                .await;
                            gateway = Some(packet.gateway_b58())
                        }
                        sender.uplink(packet).await;
                    } else {
                        tracing::warn!(?msg.data, "ignoring message");
                    }
                }

                tracing::info!("gateway went down");
                sender
                    .gateway_disconnect(gateway.expect("packet was sent by gateway"))
                    .await;
            });

            Ok(Response::new(ReceiverStream::new(downlink_receiver)))
        }
    }
}
