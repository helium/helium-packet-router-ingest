use clap::Parser;
use hpr_http_rs::{
    gwmp::{self, settings::GwmpSettings},
    http_roaming::{self, settings::HttpSettings},
    settings, uplink,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::{net::SocketAddr, path::PathBuf};
use tokio::spawn;

#[derive(Debug, clap::Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Commands {
    /// Run the program with a toml settings file.
    ServeHttp { path: Option<PathBuf> },
    /// Run the program to forward traffic over UDP
    ServeGwmp { path: Option<PathBuf> },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let cli = Cli::parse();
    tracing::debug!(?cli, "opts");

    match cli.command {
        Commands::ServeHttp { path } => {
            let settings = settings::http_from_path(path);
            run_http(settings).await
        }
        Commands::ServeGwmp { path } => {
            let settings = settings::gwmp_from_path(path);
            run_gwmp(settings).await
        }
    }
}

pub async fn run_http(settings: HttpSettings) {
    let protocol_version = &settings.roaming.protocol_version;
    let metrics_listen_addr = settings.metrics_listen;
    let http_listen_addr = settings.network.downlink_listen;
    let grpc_listen_addr = settings.network.uplink_listen;
    let outgoing_addr = settings.network.lns_endpoint.clone();
    let dedup_window = settings.roaming.dedup_window;

    tracing::info!("=====================================");
    tracing::info!("protocol        :: {protocol_version:?}");
    tracing::info!("metrics listen  :: {metrics_listen_addr}");
    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("downlink listen :: {http_listen_addr}");
    tracing::info!("uplink post     :: {outgoing_addr}");
    tracing::info!("dedup window    :: {dedup_window}");
    tracing::info!("=====================================");

    start_metrics(metrics_listen_addr);

    let (sender, receiver) = http_roaming::MsgSender::new();

    let _ = tokio::try_join!(
        spawn(http_roaming::app::start(
            sender.clone(),
            receiver,
            settings.clone()
        )),
        spawn(uplink::ingest::start(sender.clone(), grpc_listen_addr)),
        spawn(http_roaming::downlink_ingest::start(
            sender.clone(),
            http_listen_addr,
            settings.roaming
        ))
    );
}

pub async fn run_gwmp(settings: GwmpSettings) {
    let metrics_listen_addr = settings.metrics_listen;
    let grpc_listen_addr = settings.uplink_listen;

    tracing::info!("=====================================");
    tracing::info!("metrics listen  :: {metrics_listen_addr}");
    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("=====================================");

    start_metrics(metrics_listen_addr);

    let (sender, receiver) = gwmp::MsgSender::new();

    let _ = tokio::try_join!(
        spawn(gwmp::app::start(sender.clone(), receiver, settings.clone())),
        spawn(uplink::ingest::start(sender.clone(), grpc_listen_addr)),
    );
}

fn start_metrics(listen_addr: SocketAddr) {
    match PrometheusBuilder::new()
        .with_http_listener(listen_addr)
        .install()
    {
        Ok(()) => tracing::info!("metrics listening"),
        Err(err) => tracing::error!(?err, "failed to install prometheus endpoing"),
    };
}
