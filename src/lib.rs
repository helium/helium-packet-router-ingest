pub mod gwmp;
pub mod http_roaming;
pub mod region;
pub mod settings;
pub mod uplink;

pub type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;
