pub mod app;
pub mod deduplicator;
pub mod downlink_ingest;
pub mod protocol;
pub mod region;
pub mod settings;
pub mod uplink_ingest;

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;
