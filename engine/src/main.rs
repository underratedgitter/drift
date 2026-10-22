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
            "Drift engine with a browser interface (the Mac app embeds the same engine)\n  --name NAME\n  --data DIRECTORY\n  --receive DIRECTORY   save verified received files here, e.g. ~/Downloads/Drift\n  --host LOCAL_IPV4\n  --port 0\n  --ui-port 9418\n  --ui dist/ui\n  --no-discovery\n\nOpen the private loopback URL printed at startup. No account required."
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
    let receive = std::env::args()
        .any(|a| a == "--receive")
        .then(|| PathBuf::from(option("--receive", "")));
    let running = drift_engine::launch(drift_engine::Options {
        root: root.clone(),
        name: name.clone(),
        host: host.clone(),
        bind: SocketAddr::from(([0, 0, 0, 0], port)),
        discover: !std::env::args().any(|a| a == "--no-discovery"),
        inbox: receive.clone(),
    })
    .await?;
    let ui_port: u16 = option("--ui-port", "9418").parse()?;
    let assets = PathBuf::from(option("--ui", "dist/ui"));
    anyhow::ensure!(
        assets.join("index.html").is_file(),
        "UI assets not found. Run npm run build, then start from the repository with --ui dist/ui"
    );
    let (address, ui_task) = drift_engine::web::serve(
        running.engine.clone(),
        SocketAddr::from(([127, 0, 0, 1], ui_port)),
        assets,
    )
    .await?;
    println!(
        "Drift: {name}\nLAN listener: {host}:{}\nFiles: {}\n\nPrivate interface (keep this URL on this device):\nhttp://{address}/#{}",
        running.address.port(),
        receive.unwrap_or_else(|| root.join("received")).display(),
        running.engine.ui_token
    );
    tokio::signal::ctrl_c().await?;
    ui_task.abort();
    running.stop().await;
    Ok(())
}
