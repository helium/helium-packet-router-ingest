use clap::Parser;
use hpr_http_rs::{
    gwmp::{self, settings::GwmpSettings},
    http_roaming::{self, settings::HttpSettings},
    settings::{self, Protocol},
    uplink, Result,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::{net::SocketAddr, path::PathBuf};
use tokio::spawn;

#[derive(Debug, clap::Parser)]
struct Cli {
    /// Path to settings.toml
    path: PathBuf,
}

#[tokio::main]
async fn main() -> Result {
    tracing_subscriber::fmt().init();

    let cli = Cli::parse();
    tracing::debug!(?cli, "opts");

    match settings::protocol_from_path(&cli.path)? {
        Protocol::Http => {
            let settings = settings::http_from_path(&cli.path)?;
            run_http(settings).await
        }
        Protocol::Gwmp => {
            let settings = settings::gwmp_from_path(&cli.path)?;
            run_gwmp(settings).await
        }
    }
    Ok(())
}

pub async fn run_http(settings: HttpSettings) {
    let protocol_version = &settings.roaming.protocol_version;
    let metrics_listen_addr = settings.metrics_listen;
    let http_listen_addr = settings.downlink_listen;
    let grpc_listen_addr = settings.uplink_listen;
    let outgoing_addr = settings.lns_endpoint.clone();
    let dedup_window = settings.roaming.dedup_window;

    tracing::info!("=== HTTP =============================");
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

    tracing::info!("=== GWMP ============================");
    tracing::info!("metrics listen  :: {metrics_listen_addr}");
    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("Region mappings ::");
    for (region, port) in settings.region_port_mapping.iter() {
        tracing::info!("  {:<13} :: {port}", region.to_string());
    }
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
