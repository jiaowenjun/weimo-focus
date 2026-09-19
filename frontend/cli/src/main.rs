use std::io::IsTerminal;

use clap::{Parser, Subcommand};
use tracker_cli::api::{ApiClient, TrackerApi};
use tracker_cli::types::{EventView, SyncOutcomeStatus, SyncStatus};
use tracker_cli::workflow::{InquirePrompter, WorkflowError, run_entry};

#[derive(Debug, Parser)]
#[command(about = "Weimo time tracker HTTP 客户端")]
struct Cli {
    #[arg(long, global = true, default_value = "http://127.0.0.1:9123")]
    server: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 交互式新增一条事件
    Entry {
        /// 只保存到 SQLite，不立即同步 Calendar
        #[arg(long)]
        save_only: bool,
    },
    /// 查看待同步事件
    Pending {
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    /// 重试一条事件或同步全部 pending 事件
    Sync {
        #[arg(long)]
        event_id: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    let api = match ApiClient::new(&cli.server) {
        Ok(api) => api,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let exit_code = match cli.command {
        Command::Entry { save_only } => run_entry_command(&api, save_only),
        Command::Pending { limit } => pending_command(&api, limit),
        Command::Sync { event_id } => sync_command(&api, event_id.as_deref()),
    };
    std::process::exit(exit_code);
}

fn run_entry_command(api: &ApiClient, save_only: bool) -> i32 {
    if !std::io::stdin().is_terminal() {
        eprintln!("entry 命令需要在交互式终端中运行");
        return 2;
    }
    match run_entry(api, &mut InquirePrompter, save_only) {
        Ok(code) => code,
        Err(WorkflowError::Cancelled) => {
            println!("已取消，数据库未修改。");
            0
        }
        Err(WorkflowError::Interrupted) => 130,
        Err(WorkflowError::NotInteractive) => 2,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn pending_command(api: &ApiClient, limit: u32) -> i32 {
    match api.pending(limit) {
        Ok(page) => {
            if page.items.is_empty() {
                println!("没有待同步事件。");
            } else {
                for event in &page.items {
                    println!("{}", format_event(event));
                    if let Some(error) = &event.last_sync_error {
                        println!("  上次错误: {}: {}", error.code, error.message);
                    }
                }
                println!("共 {} 条待同步事件。", page.total_count);
            }
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn sync_command(api: &ApiClient, event_id: Option<&str>) -> i32 {
    if let Some(event_id) = event_id {
        return match api.sync_event(event_id) {
            Ok(result) => {
                println!("{}", format_event(&result.event));
                print_sync_outcome(result.sync.status, result.sync.error.as_ref())
            }
            Err(error) => {
                eprintln!("{error}");
                1
            }
        };
    }
    match api.sync_pending() {
        Ok(result) => {
            for item in &result.results {
                println!("{}", format_event(&item.event));
                if let Some(error) = &item.sync.error {
                    println!("  同步失败: {}: {}", error.code, error.message);
                }
            }
            println!(
                "同步完成：尝试 {} 条，成功 {} 条，失败 {} 条。",
                result.attempted, result.succeeded, result.failed
            );
            i32::from(result.failed > 0)
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn print_sync_outcome(
    status: SyncOutcomeStatus,
    error: Option<&tracker_cli::types::OperationError>,
) -> i32 {
    match status {
        SyncOutcomeStatus::Succeeded => {
            println!("已同步到 Calendar。");
            0
        }
        SyncOutcomeStatus::AlreadySynced => {
            println!("事件已经同步，无需重试。");
            0
        }
        SyncOutcomeStatus::Failed => {
            if let Some(error) = error {
                eprintln!("已保存但尚未同步：{}: {}", error.code, error.message);
            } else {
                eprintln!("已保存但尚未同步。");
            }
            1
        }
        SyncOutcomeStatus::NotRequested => {
            println!("事件已保存为待同步状态。");
            0
        }
    }
}

fn format_event(event: &EventView) -> String {
    let status = match event.sync_status {
        SyncStatus::Pending => "pending",
        SyncStatus::Synced => "synced",
    };
    format!(
        "{} [{}] {} {} {}-{} {}-{}（数量 {}） {status}",
        event.id,
        event.calendar_name,
        event.title,
        event.event_date,
        event.start_time,
        event.end_time,
        event.first_item,
        event.last_item,
        event.quantity,
    )
}
