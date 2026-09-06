pub mod api;
pub mod collector;
pub mod history;
pub mod registry;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

#[derive(Debug, Clone)]
pub struct ConsoleConfig {
    pub listen: SocketAddr,
    pub database_path: PathBuf,
    pub asset_path: PathBuf,
}
impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 7331),
            database_path: PathBuf::from("console.sqlite3"),
            asset_path: PathBuf::from("ui/dist"),
        }
    }
}
pub async fn serve(config: ConsoleConfig) -> Result<(), Box<dyn std::error::Error>> {
    if !config.listen.ip().is_loopback() {
        return Err("console listen address must be loopback".into());
    }
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    axum::serve(listener, api::router()).await?;
    Ok(())
}
