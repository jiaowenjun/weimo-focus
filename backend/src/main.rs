use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;
#[cfg(target_os = "macos")]
use tracker_backend::ProcessCalendarAdapter;
#[cfg(not(target_os = "macos"))]
use tracker_backend::UnsupportedCalendarAdapter;
use tracker_backend::{
    CalendarPort, DatabaseLock, SystemClock, TrackerError, TrackerService, router,
};

#[derive(Debug, Parser)]
#[command(about = "Weimo time tracker 本地后端")]
struct Cli {
    #[arg(long, global = true, default_value = "./data/tracker.sqlite3")]
    database: PathBuf,

    #[arg(long, global = true, default_value = "127.0.0.1:9123")]
    listen: SocketAddr,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    if let Err(error) = run(Cli::parse()).await {
        tracing::error!(error = %error, "tracker backend 退出");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), TrackerError> {
    let _database_lock = DatabaseLock::acquire(&cli.database)?;
    serve(cli.database, cli.listen).await
}

async fn serve(database: PathBuf, listen: SocketAddr) -> Result<(), TrackerError> {
    if listen.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(TrackerError::field(
            "listen",
            "第一阶段只允许监听 127.0.0.1",
        ));
    }
    let calendar: Arc<dyn CalendarPort> = platform_calendar();
    let service = Arc::new(TrackerService::open(&database, calendar, Arc::new(SystemClock)).await?);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| TrackerError::Internal(format!("无法监听 {listen}: {error}")))?;
    info!(address = %listen, "tracker backend 已启动");
    axum::serve(listener, router(service))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| TrackerError::Internal(format!("HTTP 服务失败: {error}")))
}

#[cfg(target_os = "macos")]
fn platform_calendar() -> Arc<dyn CalendarPort> {
    Arc::new(ProcessCalendarAdapter::new())
}

#[cfg(not(target_os = "macos"))]
fn platform_calendar() -> Arc<dyn CalendarPort> {
    Arc::new(UnsupportedCalendarAdapter)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
