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
    #[arg(long, default_value = "TODO")]
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
        Commands::Serve(http_config) => server::run_http(http_config).await,
    }
}

mod server {
    use crate::{
        deduplicator, grpc,
        packet_info::{PacketHash, PacketMeta, RoutingInfo},
        HttpConfig, Result,
    };
    use helium_proto::services::router::{EnvelopeDownV1, PacketRouterPacketUpV1};
    use std::{collections::HashMap, time::Duration};
    use tokio::sync::mpsc::Sender;
    use tonic::Status;

    #[derive(Debug, Clone)]
    pub struct MsgSender(Sender<Msg>);

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
        GrpcConnect(Vec<u8>, Sender<Result<EnvelopeDownV1, Status>>),
        /// A packet has been received
        GrpcPacket(PacketRouterPacketUpV1),
        /// A gateway stream has ended
        GrpcDisconnect(Vec<u8>),
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
                    packet.gateway.to_ascii_lowercase(),
                    downlink_sender.clone(),
                ))
                .await
                .expect("gateway_connect");
        }

        pub async fn gateway_disconnect(&self, gateway: Vec<u8>) {
            self.0
                .send(Msg::GrpcDisconnect(gateway))
                .await
                .expect("gateway_disconnect");
        }

        pub async fn packet(&self, packet: PacketRouterPacketUpV1) {
            self.0.send(Msg::GrpcPacket(packet)).await.expect("packet");
        }
    }

    pub async fn run_http(http_config: HttpConfig) {
        console_subscriber::init();

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
                    Msg::GrpcPacket(packet) => {
                        deduplicator.handle_packet(packet);
                    }
                    Msg::DedupDone(hash) => {
                        let packets = deduplicator.get_packets(&hash);
                        tracing::info!(num_packets = packets.len(), "deduplication done");
                        match make_payload(packets, &http_config) {
                            Ok(body) => {
                                println!(
                                    "sending\n{}",
                                    serde_json::to_string_pretty(&body).unwrap()
                                );
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

        let grpc_thread = grpc::start(MsgSender(tx.clone()));

        let _ = tokio::try_join!(grpc_thread, reader_thread);
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
                "FNSULToken": make_token(),
                "GWCnt": packets.len(),
                "GWInfo": gw_info
            }
        }))
    }

    fn make_token() -> String {
        "TODO".to_string()
    }
}

mod packet_info {
    use helium_proto::services::router::PacketRouterPacketUpV1;
    use lorawan::parser::{DataHeader, PhyPayload};

    pub type PacketHash = String;
    type Eui = String;

    pub enum RoutingInfo {
        Eui { app: Eui, dev: Eui },
        Devaddr(Eui),
        Unknown,
    }
    pub trait PacketMeta {
        fn routing_info(&self) -> RoutingInfo;
        fn frequency_mhz(&self) -> f64;
        fn recv_time(&self) -> String;
        fn datarate_index(&self) -> u32;
        // fn gateway_b58(&self) -> Result<String>;
        fn gateway_mac_str(&self) -> String;
        // fn gateway_mac(&self) -> Result<semtech_udp::MacAddress>;
    }

    impl PacketMeta for PacketRouterPacketUpV1 {
        fn routing_info(&self) -> RoutingInfo {
            match lorawan::parser::parse(self.payload.clone()).expect("valid packet") {
                PhyPayload::JoinAccept(_) => RoutingInfo::Unknown,
                PhyPayload::JoinRequest(request) => {
                    let app = request.app_eui().to_string();
                    let dev = request.dev_eui().to_string();
                    RoutingInfo::Eui { app, dev }
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
            let hash = hex::encode(hash);
            hash
        }

        fn frequency_mhz(&self) -> f64 {
            // Truncate -> Round -> Truncate
            let freq = (self.frequency as f64) / 1_000.0;
            freq.round() / 1_000.0
        }

        fn recv_time(&self) -> String {
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

mod grpc {

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
                            gateway = Some(packet.gateway.clone())
                        }
                        sender.packet(packet).await;
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
