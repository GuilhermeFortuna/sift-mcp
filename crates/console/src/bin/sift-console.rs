use std::{net::SocketAddr, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = console::ConsoleConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--listen" => {
                config.listen = args
                    .next()
                    .ok_or("--listen requires an address")?
                    .parse::<SocketAddr>()?
            }
            "--assets" => {
                config.asset_path = PathBuf::from(args.next().ok_or("--assets requires a path")?)
            }
            "--database" => {
                config.database_path =
                    PathBuf::from(args.next().ok_or("--database requires a path")?)
            }
            "--collect" => {
                let mode = args.next().ok_or("--collect requires on or off")?;
                match mode.as_str() {
                    "on" => config.collect = true,
                    "off" => config.collect = false,
                    _ => {
                        return Err(format!(
                            "invalid --collect value: {mode} (expected 'on' or 'off')"
                        )
                        .into());
                    }
                }
            }
            "--help" => {
                println!(
                    "sift-console --listen 127.0.0.1:7331 --assets ui/dist --database console.sqlite3 --collect on"
                );
                return Ok(());
            }
            _ => return Err(format!("unknown option: {flag}").into()),
        }
    }
    console::serve(config).await
}
