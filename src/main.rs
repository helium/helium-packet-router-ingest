use clap::Parser;
use hpr_http_rs::{
    app::{self, MsgSender},
    downlink_ingest,
    settings::{self, Settings},
    uplink_ingest,
};
use std::path::PathBuf;

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
    let http_listen_addr = settings.network.downlink_listen;
    let grpc_listen_addr = settings.network.uplink_listen;
    let outgoing_addr = settings.network.lns_endpoint.clone();
    let dedup_window = settings.roaming.dedup_window;

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
