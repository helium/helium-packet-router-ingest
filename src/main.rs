use clap::Parser;
use hpr_http_rs::{
    app::{self, MsgSender},
    downlink_ingest,
    settings::{self, Settings},
    uplink_ingest,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::{net::SocketAddr, path::PathBuf};

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
    tracing_subscriber::fmt().init();

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
    let metrics_listen_addr = settings.metrics_listen;
    let http_listen_addr = settings.network.downlink_listen;
    let grpc_listen_addr = settings.network.uplink_listen;
    let outgoing_addr = settings.network.lns_endpoint.clone();
    let dedup_window = settings.roaming.dedup_window;

    tracing::info!("=====================================");
    tracing::info!("metrics listen  :: {metrics_listen_addr}");
    tracing::info!("uplink listen   :: {grpc_listen_addr}");
    tracing::info!("downlink listen :: {http_listen_addr}");
    tracing::info!("uplink post     :: {outgoing_addr}");
    tracing::info!("dedup window    :: {dedup_window}");
    tracing::info!("=====================================");

    start_metrics(metrics_listen_addr);

    let (sender, receiver) = MsgSender::new();

    let _ = tokio::try_join!(
        app::start(sender.clone(), receiver, settings.clone()),
        uplink_ingest::start(sender.clone(), grpc_listen_addr),
        downlink_ingest::start(sender.clone(), http_listen_addr, settings.roaming)
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
