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
    /// Identity of the this NS, unique to HTTP forwarder
    #[arg(long, default_value = "6081FFFE12345678")]
    pub sender_nsid: String,
    /// Identify of the receiving NS
    #[arg(long, default_value = "0000000000000000")]
    pub receiver_nsid: String,
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
    /// Send PRStartNotif message after sending Downlink to gateway.
    /// Chirpstack does nothing with the notif message and returns an 400.
    #[arg(long, default_value = "false")]
    pub send_pr_start_notif: bool,
    /// LNS Endpoint
    #[arg(long, default_value = "http://localhost:9005")]
    pub lns_endpoint: String,
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
        deduplicator, grpc,
        http::{self, PRStartAnsDownlink, XmitDataReq},
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
    pub struct MsgSender(pub Sender<Msg>);

    #[derive(Debug)]
    pub struct GatewayTx(pub Sender<Result<EnvelopeDownV1, Status>>);

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

    type GW = String;
    type TransactionID = u64;

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
        Uplink(UplinkMsg),
        Downlink(DownlinkMsg),
        Gateway(GatewayMsg),
    }

    #[derive(Debug)]
    pub enum UplinkMsg {
        Receive(PacketRouterPacketUpV1),
        Send(PacketHash),
        Cleanup(PacketHash),
    }

    #[derive(Debug)]
    pub enum DownlinkMsg {
        JoinAccept(PRStartAnsDownlink),
        Downlink(XmitDataReq),
    }

    impl DownlinkMsg {
        fn gateway(&self) -> String {
            match self {
                Self::JoinAccept(pr_start) => pr_start.dl_meta_data.fns_ul_token.gateway.clone(),
                Self::Downlink(xmit) => xmit.dl_meta_data.fns_ul_token.gateway.clone(),
            }
        }
        fn transaction_id(&self) -> u64 {
            match self {
                Self::JoinAccept(pr_start) => pr_start.transaction_id,
                Self::Downlink(xmit) => xmit.transaction_id,
            }
        }

        fn packet_down(&self) -> PacketRouterPacketDownV1 {
            match self {
                Self::JoinAccept(p) => p.into(),
                Self::Downlink(p) => p.into(),
            }
        }
    }

    #[derive(Debug)]
    pub enum GatewayMsg {
        Connect(GW, GatewayTx),
        Disconnect(GW),
    }

    impl MsgSender {
        pub async fn dedup_done(&self, key: String) {
            self.0
                .send(Msg::Uplink(UplinkMsg::Send(key)))
                .await
                .expect("dedup done");
        }

        pub async fn dedup_cleanup(&self, key: String) {
            self.0
                .send(Msg::Uplink(UplinkMsg::Cleanup(key)))
                .await
                .expect("dedup_cleanup");
        }

        pub async fn gateway_connect(
            &self,
            packet: &PacketRouterPacketUpV1,
            downlink_sender: GatewayTx,
        ) {
            self.0
                .send(Msg::Gateway(GatewayMsg::Connect(
                    packet.gateway_b58(),
                    downlink_sender,
                )))
                .await
                .expect("gateway_connect");
        }

        pub async fn gateway_disconnect(&self, gateway: GW) {
            self.0
                .send(Msg::Gateway(GatewayMsg::Disconnect(gateway)))
                .await
                .expect("gateway_disconnect");
        }

        pub async fn uplink(&self, packet: PacketRouterPacketUpV1) {
            self.0
                .send(Msg::Uplink(UplinkMsg::Receive(packet)))
                .await
                .expect("uplink");
        }

        pub async fn join_accept_downlink(&self, pr_start_ans: PRStartAnsDownlink) {
            self.0
                .send(Msg::Downlink(DownlinkMsg::JoinAccept(pr_start_ans)))
                .await
                .expect("join accept downlink");
        }

        pub async fn data_downlink(&self, xmit_req: XmitDataReq) {
            self.0
                .send(Msg::Downlink(DownlinkMsg::Downlink(xmit_req)))
                .await
                .expect("data downlink");
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
            let mut gateway_map: HashMap<String, GatewayTx> = HashMap::new();
            tracing::info!("reading thread started");

            let http_send = |body: String| async {
                reqwest::Client::new()
                    .post(http_config.lns_endpoint.clone())
                    .body(body)
                    .send()
                    .await
            };

            while let Some(msg) = rx.recv().await {
                match msg {
                    Msg::Uplink(inner) => match inner {
                        UplinkMsg::Receive(packet) => {
                            // Is there a way to make it clear that deduplicating triggers send later?
                            deduplicator.handle_packet(packet);
                        }
                        UplinkMsg::Send(packet_hash) => {
                            let packets = deduplicator.get_packets(&packet_hash);
                            tracing::info!(num_packets = packets.len(), "deduplication done");
                            match make_payload(packets, &http_config) {
                                Ok(body) => {
                                    let _ = http_send(body).await;
                                }
                                Err(_) => {
                                    tracing::warn!("ignoring invalid packet");
                                }
                            };
                        }
                        UplinkMsg::Cleanup(packet_hash) => {
                            deduplicator.remove_packets(&packet_hash);
                        }
                    },
                    Msg::Downlink(source) => {
                        let gw = source.gateway();
                        let transaction_id = source.transaction_id();
                        match gateway_map.get(&gw) {
                            Some(gateway) => {
                                gateway.send_downlink(source.packet_down()).await;
                                tracing::info!(gw, "uplink sent");

                                match source {
                                    DownlinkMsg::JoinAccept(_) => {
                                        if http_config.send_pr_start_notif {
                                            let body =
                                                make_pr_start_notif(transaction_id, &http_config);
                                            let _ = http_send(body).await;
                                            tracing::debug!("http notif");
                                        }
                                    }
                                    DownlinkMsg::Downlink(xmit) => {
                                        let body = make_xmit_data_ans(&xmit, &http_config);
                                        let _ = http_send(body).await;
                                        tracing::debug!("http xmit ans");
                                    }
                                }
                            }
                            None => tracing::warn!(?gw, "join accept for unknown gateway"),
                        }
                    }
                    Msg::Gateway(inner) => match inner {
                        GatewayMsg::Connect(gw, sender) => {
                            let _prev_val = gateway_map.insert(gw, sender);
                            tracing::info!(size = gateway_map.len(), "gateway connect");
                        }
                        GatewayMsg::Disconnect(gw) => {
                            let _prev_val = gateway_map.remove(&gw);
                            tracing::info!(size = gateway_map.len(), "gateway disconnect");
                        }
                    },
                }
            }
        });

        let http_thread = http::start(MsgSender(tx.clone()));
        let grpc_thread = grpc::start(MsgSender(tx.clone()));

        let _ = tokio::try_join!(grpc_thread, reader_thread, http_thread);
    }

    fn make_pr_start_notif(transaction_id: TransactionID, http_config: &HttpConfig) -> String {
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

    fn make_xmit_data_ans(xmit: &XmitDataReq, http_config: &HttpConfig) -> String {
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

    // #[instrument]
    fn make_payload(packets: Vec<PacketRouterPacketUpV1>, config: &HttpConfig) -> Result<String> {
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

        Ok(serde_json::to_string(&serde_json::json!({
            "ProtocolVersion" : "1.1",
            "MessageType": "PRStartReq",
            "SenderNSID": config.sender_nsid,
            "ReceiverNSID": config.receiver_nsid,
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
        .expect("pr_start_req json"))
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
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
        match serde_json::from_value::<PRStartAnsDownlink>(downlink.clone()) {
            Ok(pr_start) => {
                sender.join_accept_downlink(pr_start).await;
                (StatusCode::ACCEPTED, "Downlink Accepted")
            }
            Err(_) => match serde_json::from_value::<PRStartAnsPlain>(downlink.clone()) {
                Ok(_plain) => (StatusCode::ACCEPTED, "Answer Accepted"),
                Err(_) => match serde_json::from_value::<XmitDataReq>(downlink) {
                    Ok(xmit) => {
                        sender.data_downlink(xmit).await;
                        (StatusCode::ACCEPTED, "Xmit Accepted")
                    }
                    Err(_) => {
                        tracing::error!(
                            "could not make pr_start_ans or xmit_data_req from downlink"
                        );
                        (StatusCode::BAD_REQUEST, "unknown")
                    }
                },
            },
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

    impl From<&XmitDataReq> for PacketRouterPacketDownV1 {
        fn from(value: &XmitDataReq) -> Self {
            let freq = value.dl_meta_data.dl_freq_1;
            let freq = (freq * 1_000_000.0) as u32; // mHz -> Hz
            let payload =
                hex::decode(value.phy_payload.clone()).expect("hex decode downlink payload");
            tracing::info!(
                before = ?value.phy_payload,
                after = ?payload,
                "to packet down"
            );
            Self {
                payload,
                rx1: Some(WindowV1 {
                    timestamp: ((value.dl_meta_data.fns_ul_token.timestamp
                        + (value.dl_meta_data.rx_delay_1 as u64 * 1_000_000))
                        as u32) as u64,
                    datarate: value.dl_meta_data.data_rate_1.into(),
                    frequency: freq,
                    immediate: false,
                }),
                rx2: None,
            }
        }
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

    impl From<&PRStartAnsDownlink> for PacketRouterPacketDownV1 {
        fn from(value: &PRStartAnsDownlink) -> Self {
            let freq1 = value.dl_meta_data.dl_freq_1;
            let freq1 = (freq1 * 1_000_000.0) as u32; // mHz -> Hz

            let freq2 = value.dl_meta_data.dl_freq_2;
            let freq2 = (freq2 * 1_000_000.0) as u32; // mHz -> Hz

            let payload =
                hex::decode(value.phy_payload.clone()).expect("hex decode downlink payload");
            Self {
                payload,
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
        #![allow(unused)]
        use super::PRStartAnsPlain;
        use crate::http::{PRStartAnsDownlink, XmitDataReq};
        use helium_proto::services::router::{PacketRouterPacketDownV1, WindowV1};

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
            let packet: &XmitDataReq = &serde_json::from_value(value).unwrap();
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
            let packet: &PRStartAnsDownlink =
                &serde_json::from_value(value).expect("to packet down");
            println!("packet: {packet:#?}");
            let down: PacketRouterPacketDownV1 = packet.into();
            println!("down: {down:#?}");
        }

        #[test]
        fn mic_failed() {
            let value = serde_json::json!({"ProtocolVersion":"1.1","SenderID":"000024","ReceiverID":"c00053","TransactionID":517448448,"MessageType":"PRStartAns","Result":{"ResultCode":"MICFailed","Description":"Invalid MIC"},"Lifetime":null,"FNwkSIntKey":null,"NwkSKey":null,"FCntUp":null,"ServiceProfile":null,"DLMetaData":null});
            let packet: PRStartAnsPlain = serde_json::from_value(value).expect("to pr plain");
            println!("plain: {packet:#?}");
        }
    }
}

mod grpc {

    use crate::server::MsgSender;
    use crate::{packet_info::PacketMeta, server::GatewayTx};
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
                                .gateway_connect(&packet, GatewayTx(downlink_sender.clone()))
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
