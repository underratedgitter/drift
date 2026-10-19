use anyhow::{Context, Result};
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};

fn option(name: &str, default: &str) -> String {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|p| p[0] == name)
        .map(|p| p[1].clone())
        .unwrap_or(default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help") {
        println!(
            "Drift LAN transfer\n  --name NAME\n  --data DIRECTORY\n  --host LOCAL_IPV4\n  --port 0\n  --ui-port 9418\n  --phone-port 9443\n  --no-phone\n  --ui dist/ui\n  --no-discovery\n\nOpen the private loopback URL printed at startup. No account required."
        );
        return Ok(());
    }
    let host = option("--host", &drift_engine::network::lan_ipv4());
    let _: Ipv4Addr = host.parse().context("--host must be an IPv4 address")?;
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    let root = PathBuf::from(option("--data", &format!("{home}/.drift")));
    let name = option(
        "--name",
        &std::env::var("HOSTNAME").unwrap_or_else(|_| "My device".into()),
    );
    let port: u16 = option("--port", "0").parse()?;
    let running = drift_engine::start(
        root.clone(),
        name.clone(),
        host.clone(),
        SocketAddr::from(([0, 0, 0, 0], port)),
        !std::env::args().any(|a| a == "--no-discovery"),
    )
    .await?;
    let ui_port: u16 = option("--ui-port", "9418").parse()?;
    let assets = PathBuf::from(option("--ui", "dist/ui"));
    anyhow::ensure!(
        assets.join("index.html").is_file(),
        "UI assets not found. Run npm run build, then start from drift/ with --ui dist/ui"
    );
    let _phone = if std::env::args().any(|a| a == "--no-phone") {
        None
    } else {
        let port: u16 = option("--phone-port", "9443").parse()?;
        let phone = drift_engine::phone::start(
            running.engine.clone(),
            host.clone(),
            SocketAddr::from(([0, 0, 0, 0], port)),
            assets.clone(),
        )
        .await?;
        println!(
            "Phone HTTPS gateway: {} (configure certificate trust in Devices)",
            phone.hub.origin
        );
        Some(phone)
    };
    let (address, ui_task) = drift_engine::web::serve(
        running.engine.clone(),
        SocketAddr::from(([127, 0, 0, 1], ui_port)),
        assets,
    )
    .await?;
    println!(
        "Drift: {name}\nLAN listener: {host}:{}\nFiles: {}\n\nPrivate interface (keep this URL on this device):\nhttp://{address}/#{}",
        running.address.port(),
        root.join("received").display(),
        running.engine.ui_token
    );
    if std::env::var("DRIFT_DESKTOP_PARENT").as_deref() == Ok("1") {
        use tokio::io::AsyncReadExt;
        let mut stdin = tokio::io::stdin();
        let mut byte = [0];
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = stdin.read(&mut byte) => {} }
    } else {
        tokio::signal::ctrl_c().await?;
    }
    running.shutdown();
    ui_task.abort();
    Ok(())
}
