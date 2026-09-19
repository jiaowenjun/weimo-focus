use std::io::IsTerminal;

use record_event::api::ApiClient;
use record_event::start_workflow::{SystemClock, TerminalCountdown, run_start_event};
use record_event::workflow::{InquirePrompter, WorkflowError};

const SERVER_URL: &str = "http://127.0.0.1:9123";

fn main() {
    if std::env::args().len() != 1 {
        eprintln!("用法: start-event");
        std::process::exit(2);
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("start-event 需要在交互式终端中运行");
        std::process::exit(2);
    }
    let api = match ApiClient::new(SERVER_URL) {
        Ok(api) => api,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let exit_code = match run_start_event(
        &api,
        &mut InquirePrompter,
        &mut SystemClock,
        &mut TerminalCountdown,
    ) {
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
    };
    std::process::exit(exit_code);
}
