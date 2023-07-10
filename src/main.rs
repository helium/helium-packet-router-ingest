use actions::MsgSender;
use clap::Parser;
use settings::Settings;
use std::path::PathBuf;

mod deduplicator;
mod downlink_ingest;
mod packet;
mod roaming;
mod settings;
mod uplink_ingest;

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Commands {
    /// Run the program with command line arguments you provide
    Serve(Settings),
    /// Run the program with a toml settings file.
    ServeConfig { path: PathBuf },
}

#[tokio::main]
async fn main() {
    #[cfg(not(feature = "console"))]
    tracing_subscriber::fmt().init();
    #[cfg(feature = "console")]
    console_subscriber::init();

    let cli = Cli::parse();
    tracing::debug!(?cli, "opts");

    match cli.command {
        Commands::Serve(settings) => run(settings).await,
        Commands::ServeConfig { path } => {
            let settings = settings::from_path(path);
            run(settings).await
        }
    }
}

pub async fn run(settings: Settings) {
    let http_listen_addr = settings.downlink_listen;
    let grpc_listen_addr = settings.uplink_listen;
    let outgoing_addr = settings.lns_endpoint.clone();
    let dedup_window = settings.dedup_window;

    tracing::info!("=====================================");
    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("downlink listen :: {http_listen_addr}");
    tracing::info!("uplink post     :: {outgoing_addr}");
    tracing::info!("dedup window    :: {dedup_window}");
    tracing::info!("=====================================");

    let (sender, receiver) = MsgSender::new();

    let _ = tokio::try_join!(
        app::start(sender.clone(), receiver, settings),
        uplink_ingest::start(sender.clone(), grpc_listen_addr),
        downlink_ingest::start(sender.clone(), http_listen_addr)
    );
}

mod actions {
    use crate::{
        packet::{PacketDown, PacketHash, PacketUp},
        uplink_ingest::{GatewayID, GatewayTx},
    };
    use tokio::sync::mpsc::{Receiver, Sender};

    #[derive(Debug, Clone)]
    pub struct MsgSender(pub Sender<Msg>);
    pub type MsgReceiver = Receiver<Msg>;

    /// This message contains the entirety of the lifecycle of routing packets from hpr through http.
    ///
    /// At a gateway's first packet, we register a downlink handler for a gateway.
    /// Once a gateway is known packets will be ingested normally.
    ///
    /// Packets are sent to the deduplicator, which will group packets by hash.
    /// A `dedup_window` timer is started, after which the packets are forwarded to the roamer.
    /// After, a `cleanup_window` timer is started to prevent immediate double delivery of late packets.
    ///   Setting this too short may result in double delivery of packet groups.
    ///   Too long may lead to memory issues.
    #[derive(Debug)]
    pub enum Msg {
        /// Incoming Packet from a Gateway.
        UplinkReceive(PacketUp),
        /// Deduplication timer has elapsed.
        UplinkSend(PacketHash),
        /// Cleanup timer has elapsed.
        UplinkCleanup(PacketHash),
        /// Downlink received for gateway from HTTP handler.
        Downlink(PacketDown),
        /// Gateway has Connected.
        GatewayConnect(GatewayID, GatewayTx),
        /// Gateway has Disconnected.
        GatewayDisconnect(GatewayID),
    }

    impl MsgSender {
        pub fn new() -> (MsgSender, MsgReceiver) {
            let (tx, rx) = tokio::sync::mpsc::channel(512);
            (MsgSender(tx), rx)
        }

        pub async fn uplink_receive(&self, packet: PacketUp) {
            self.0
                .send(Msg::UplinkReceive(packet))
                .await
                .expect("uplink");
        }

        pub async fn uplink_send(&self, key: PacketHash) {
            self.0.send(Msg::UplinkSend(key)).await.expect("dedup done");
        }

        pub async fn uplink_cleanup(&self, key: PacketHash) {
            self.0
                .send(Msg::UplinkCleanup(key))
                .await
                .expect("dedup_cleanup");
        }

        pub async fn gateway_connect(&self, packet: &PacketUp, downlink_sender: GatewayTx) {
            self.0
                .send(Msg::GatewayConnect(packet.gateway_b58(), downlink_sender))
                .await
                .expect("gateway_connect");
        }

        pub async fn gateway_disconnect(&self, gateway: GatewayID) {
            self.0
                .send(Msg::GatewayDisconnect(gateway))
                .await
                .expect("gateway_disconnect");
        }

        pub async fn downlink(&self, downlink: impl Into<PacketDown>) {
            self.0
                .send(Msg::Downlink(downlink.into()))
                .await
                .expect("downlink");
        }
    }
}

mod app {
    use crate::{
        actions::{Msg, MsgSender},
        deduplicator::{Deduplicator, HandlePacket},
        packet::{PacketDown, PacketHash},
        roaming,
        settings::Settings,
        uplink_ingest::{GatewayID, GatewayTx},
    };
    use std::collections::HashMap;
    use tokio::sync::mpsc::Receiver;

    pub struct App {
        deduplicator: Deduplicator,
        gateway_map: HashMap<GatewayID, GatewayTx>,
        settings: Settings,
        message_tx: MsgSender,
        message_rx: Receiver<Msg>,
    }

    pub enum UpdateAction {
        Noop,
        StartTimerForNewPacket(PacketHash),
        SendDownlink(GatewayTx, PacketDown, Option<String>),
        SendUplink(String),
    }

    impl App {
        pub fn new(message_tx: MsgSender, message_rx: Receiver<Msg>, settings: Settings) -> Self {
            Self {
                deduplicator: Deduplicator::new(),
                gateway_map: HashMap::new(),
                message_tx,
                message_rx,
                settings,
            }
        }
    }

    pub fn start(
        message_tx: MsgSender,
        message_rx: Receiver<Msg>,
        settings: Settings,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut app = App::new(message_tx, message_rx, settings);
            while let Some(msg) = app.message_rx.recv().await {
                let action = update(&mut app, msg).await;
                handle_update_action(&app, action).await;
            }
        })
    }

    pub async fn handle_update_action(app: &App, action: UpdateAction) {
        match action {
            UpdateAction::Noop => {}
            UpdateAction::StartTimerForNewPacket(hash) => {
                let dedup = app.settings.dedup_window.into();
                let cleanup = app.settings.cleanup_window.into();
                let sender = app.message_tx.clone();
                tokio::spawn(async move {
                    use tokio::time::sleep;
                    sleep(dedup).await;
                    sender.uplink_send(hash.clone()).await;
                    sleep(cleanup).await;
                    sender.uplink_cleanup(hash).await;
                });
            }
            UpdateAction::SendDownlink(gw, downlink, http) => {
                let gateway_name = downlink.gateway();
                gw.send_downlink(downlink.into()).await;
                tracing::info!(gw = gateway_name, "downlink sent");

                if let Some(body) = http {
                    let res = reqwest::Client::new()
                        .post(app.settings.lns_endpoint.clone())
                        .body(body.clone())
                        .send()
                        .await;
                    tracing::info!(?body, ?res, "post");
                }
            }
            UpdateAction::SendUplink(body) => {
                let res = reqwest::Client::new()
                    .post(app.settings.lns_endpoint.clone())
                    .body(body.clone())
                    .send()
                    .await;
                tracing::info!(?body, ?res, "post");
            }
        }
    }

    async fn update(app: &mut App, msg: Msg) -> UpdateAction {
        match msg {
            Msg::UplinkReceive(packet) => match app.deduplicator.handle_packet(packet) {
                HandlePacket::New(hash) => UpdateAction::StartTimerForNewPacket(hash),
                HandlePacket::Existing => UpdateAction::Noop,
            },
            Msg::UplinkSend(packet_hash) => {
                let packets = app.deduplicator.get_packets(&packet_hash);
                tracing::info!(num_packets = packets.len(), "deduplication done");
                match roaming::make_pr_start_req(packets, &app.settings) {
                    Ok(body) => UpdateAction::SendUplink(body),
                    Err(_) => {
                        tracing::warn!("ignoring invalid packet");
                        UpdateAction::Noop
                    }
                }
            }
            Msg::UplinkCleanup(packet_hash) => {
                app.deduplicator.remove_packets(&packet_hash);
                UpdateAction::Noop
            }
            Msg::Downlink(source) => {
                let gw = source.gateway();
                let transaction_id = source.transaction_id();
                match app.gateway_map.get(&gw) {
                    None => {
                        tracing::warn!(?gw, "join accept for unknown gateway");
                        UpdateAction::Noop
                    }
                    Some(gateway) => match source.as_ref() {
                        PacketDown::JoinAccept(_) => {
                            let http = if app.settings.send_pr_start_notif {
                                tracing::debug!("http notif");
                                let body =
                                    roaming::make_pr_start_notif(transaction_id, &app.settings);
                                Some(body)
                            } else {
                                None
                            };
                            UpdateAction::SendDownlink(gateway.clone(), source, http)
                        }
                        PacketDown::Downlink(xmit) => {
                            let body = roaming::make_xmit_data_ans(xmit, &app.settings);
                            tracing::debug!("http xmit ans");
                            UpdateAction::SendDownlink(gateway.clone(), source, Some(body))
                        }
                    },
                }
            }
            Msg::GatewayConnect(gw, sender) => {
                let _prev_val = app.gateway_map.insert(gw, sender);
                tracing::info!(size = app.gateway_map.len(), "gateway connect");
                UpdateAction::Noop
            }
            Msg::GatewayDisconnect(gw) => {
                let _prev_val = app.gateway_map.remove(&gw);
                tracing::info!(size = app.gateway_map.len(), "gateway disconnect");
                UpdateAction::Noop
            }
        }
    }
}
