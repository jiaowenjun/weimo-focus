use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use time::{Date, PrimitiveDateTime, Time};
use tracing::{info, warn};

use crate::domain::{
    CalendarAction, OperationErrorView, format_date, format_time, parse_date, parse_time,
};

const CALENDAR_APP: &str = "Calendar";
const STARTUP_RETRIES: usize = 10;
const STARTUP_DELAY: Duration = Duration::from_millis(300);
const APPLE_SCRIPT: &str = include_str!("../assets/calendar.applescript");

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CalendarErrorCode {
    Missing,
    Ambiguous,
    PermissionDenied,
    Unavailable,
    PlatformUnsupported,
}

impl CalendarErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "calendar_missing",
            Self::Ambiguous => "calendar_ambiguous",
            Self::PermissionDenied => "calendar_permission_denied",
            Self::Unavailable => "calendar_unavailable",
            Self::PlatformUnsupported => "calendar_platform_unsupported",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CalendarError {
    pub code: CalendarErrorCode,
    pub message: String,
}

impl CalendarError {
    pub fn new(code: CalendarErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn to_view(&self) -> OperationErrorView {
        OperationErrorView {
            code: self.code.as_str().to_owned(),
            message: self.message.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub calendar_name: String,
    pub title: String,
    pub event_date: String,
    pub start_time: String,
    pub end_time: String,
    pub description: String,
    pub marker: String,
}

#[async_trait]
pub trait CalendarPort: Send + Sync {
    async fn create_or_find(&self, event: CalendarEvent) -> Result<CalendarAction, CalendarError>;

    fn is_supported(&self) -> bool;
}

#[derive(Debug, Default)]
pub struct UnsupportedCalendarAdapter;

#[async_trait]
impl CalendarPort for UnsupportedCalendarAdapter {
    async fn create_or_find(&self, _event: CalendarEvent) -> Result<CalendarAction, CalendarError> {
        Err(CalendarError::new(
            CalendarErrorCode::PlatformUnsupported,
            "当前平台不支持 macOS Calendar",
        ))
    }

    fn is_supported(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
struct CommandOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

trait CommandRunner: Send {
    fn run(
        &mut self,
        program: &str,
        arguments: &[String],
        stdin: Option<&str>,
    ) -> io::Result<CommandOutput>;
}

#[derive(Debug, Default)]
struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(
        &mut self,
        program: &str,
        arguments: &[String],
        stdin: Option<&str>,
    ) -> io::Result<CommandOutput> {
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn()?;
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("child stdin is unavailable"))?
                .write_all(input.as_bytes())?;
        }
        let output = child.wait_with_output()?;
        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

trait Sleeper: Send {
    fn sleep(&mut self, duration: Duration);
}

#[derive(Debug, Default)]
struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&mut self, duration: Duration) {
        thread::sleep(duration);
    }
}

struct ProcessCalendarInner {
    runner: Box<dyn CommandRunner>,
    sleeper: Box<dyn Sleeper>,
}

impl ProcessCalendarInner {
    fn start_calendar(&mut self, marker: &str) -> Result<(), CalendarError> {
        info!(marker = %marker, "启动 Calendar 应用");
        let arguments = vec!["-g".into(), "-a".into(), CALENDAR_APP.into()];
        let output = self
            .runner
            .run("open", &arguments, None)
            .map_err(|error| unavailable(format!("无法启动 Calendar: {error}")))?;
        if output.exit_code == 0 {
            return Ok(());
        }
        Err(classify_process_error(&output))
    }

    fn run(&mut self, event: &CalendarEvent) -> Result<CalendarAction, CalendarError> {
        let date = parse_calendar_date(&event.event_date)?;
        let start = parse_calendar_time(&event.start_time)?;
        let end = parse_calendar_time(&event.end_time)?;
        let mut arguments = vec![event.calendar_name.clone(), event.title.clone()];
        arguments.extend(datetime_arguments(PrimitiveDateTime::new(date, start)));
        arguments.extend(datetime_arguments(PrimitiveDateTime::new(date, end)));
        arguments.push(event.description.clone());
        arguments.push(event.marker.clone());

        self.start_calendar(&event.marker)?;
        for attempt in 1..=STARTUP_RETRIES {
            info!(
                marker = %event.marker,
                attempt,
                max_attempts = STARTUP_RETRIES,
                "发送 Calendar Apple Event"
            );
            let mut script_arguments = vec!["-".to_owned()];
            script_arguments.extend(arguments.clone());
            let output = self
                .runner
                .run("osascript", &script_arguments, Some(APPLE_SCRIPT))
                .map_err(|error| unavailable(format!("无法启动 osascript: {error}")))?;
            if output.exit_code == 0 {
                info!(
                    marker = %event.marker,
                    attempt,
                    result = %output.stdout.trim(),
                    "Calendar Apple Event 完成"
                );
                return match output.stdout.trim() {
                    "created" => Ok(CalendarAction::Created),
                    "existing" => Ok(CalendarAction::Existing),
                    "missing" => Err(CalendarError::new(
                        CalendarErrorCode::Missing,
                        format!("日历不存在: {}", event.calendar_name),
                    )),
                    value => Err(unavailable(format!("Calendar 返回未知结果: {value:?}"))),
                };
            }

            let detail = command_detail(&output);
            if !detail.contains("(-600)") {
                return Err(classify_process_error(&output));
            }
            if attempt == STARTUP_RETRIES {
                return Err(unavailable(format!(
                    "Calendar 启动后仍不可用（重试 {STARTUP_RETRIES} 次）: {detail}"
                )));
            }
            warn!(
                marker = %event.marker,
                attempt,
                retries_remaining = STARTUP_RETRIES - attempt,
                "Calendar 尚未就绪，等待后重试 Apple Event"
            );
            self.sleeper.sleep(STARTUP_DELAY);
            self.start_calendar(&event.marker)?;
        }
        unreachable!("retry loop always returns")
    }
}

#[derive(Clone)]
pub struct ProcessCalendarAdapter {
    inner: Arc<Mutex<ProcessCalendarInner>>,
}

impl std::fmt::Debug for ProcessCalendarAdapter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ProcessCalendarAdapter").finish()
    }
}

impl Default for ProcessCalendarAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessCalendarAdapter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProcessCalendarInner {
                runner: Box::new(SystemCommandRunner),
                sleeper: Box::new(ThreadSleeper),
            })),
        }
    }

    #[cfg(test)]
    fn with_dependencies(
        runner: impl CommandRunner + 'static,
        sleeper: impl Sleeper + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProcessCalendarInner {
                runner: Box::new(runner),
                sleeper: Box::new(sleeper),
            })),
        }
    }
}

#[async_trait]
impl CalendarPort for ProcessCalendarAdapter {
    async fn create_or_find(&self, event: CalendarEvent) -> Result<CalendarAction, CalendarError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            inner
                .lock()
                .map_err(|_| unavailable("Calendar adapter 锁已损坏"))?
                .run(&event)
        })
        .await
        .map_err(|error| unavailable(format!("Calendar worker 失败: {error}")))?
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "macos")
    }
}

fn parse_calendar_date(value: &str) -> Result<Date, CalendarError> {
    parse_date(value, "event_date")
        .map_err(|error| unavailable(format!("后端事件日期无效: {error}")))
}

fn parse_calendar_time(value: &str) -> Result<Time, CalendarError> {
    parse_time(value, "event_time")
        .map_err(|error| unavailable(format!("后端事件时间无效: {error}")))
}

fn datetime_arguments(value: PrimitiveDateTime) -> Vec<String> {
    let date = format_date(value.date());
    let time = format_time(value.time());
    let mut values: Vec<String> = date.split('-').map(str::to_owned).collect();
    values.extend(time.split(':').map(str::to_owned));
    values
}

fn command_detail(output: &CommandOutput) -> String {
    let detail = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    detail.trim().to_owned()
}

fn classify_process_error(output: &CommandOutput) -> CalendarError {
    let detail = command_detail(output);
    let normalized = detail.to_ascii_lowercase();
    if normalized.contains("multiple calendars have the same name") {
        return CalendarError::new(CalendarErrorCode::Ambiguous, detail);
    }
    if detail.contains("(-1743)")
        || normalized.contains("not authorized")
        || normalized.contains("not permitted")
    {
        return CalendarError::new(CalendarErrorCode::PermissionDenied, detail);
    }
    unavailable(if detail.is_empty() {
        format!("Calendar 命令退出码 {}", output.exit_code)
    } else {
        detail
    })
}

fn unavailable(message: impl Into<String>) -> CalendarError {
    CalendarError::new(CalendarErrorCode::Unavailable, message)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    #[derive(Default)]
    struct ScriptedRunner {
        outputs: VecDeque<CommandOutput>,
        programs: Vec<String>,
    }

    impl CommandRunner for ScriptedRunner {
        fn run(
            &mut self,
            program: &str,
            _arguments: &[String],
            _stdin: Option<&str>,
        ) -> io::Result<CommandOutput> {
            self.programs.push(program.to_owned());
            Ok(self.outputs.pop_front().unwrap())
        }
    }

    #[derive(Default)]
    struct RecordingSleeper {
        calls: usize,
    }

    impl Sleeper for RecordingSleeper {
        fn sleep(&mut self, _duration: Duration) {
            self.calls += 1;
        }
    }

    fn output(exit_code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn event() -> CalendarEvent {
        CalendarEvent {
            calendar_name: "学习".into(),
            title: "资料分析".into(),
            event_date: "2026-08-30".into(),
            start_time: "21:00:00".into(),
            end_time: "21:30:00".into(),
            description: "description".into(),
            marker: "marker".into(),
        }
    }

    #[tokio::test]
    async fn starts_calendar_and_retries_minus_600() {
        let runner = ScriptedRunner {
            outputs: VecDeque::from([
                output(0, "", ""),
                output(1, "", "Calendar got an error: (-600)"),
                output(0, "", ""),
                output(0, "created\n", ""),
            ]),
            programs: Vec::new(),
        };
        let adapter =
            ProcessCalendarAdapter::with_dependencies(runner, RecordingSleeper::default());

        assert_eq!(
            adapter.create_or_find(event()).await.unwrap(),
            CalendarAction::Created
        );
    }

    #[test]
    fn script_uses_bulk_marker_query_without_calendar_mutation() {
        assert!(APPLE_SCRIPT.contains(
            "set matchingEvents to (every event of targetCalendar whose description contains marker)"
        ));
        assert!(APPLE_SCRIPT.contains("if (count of matchingEvents) is greater than 0"));
        assert!(
            !APPLE_SCRIPT.contains("repeat with existingEvent in (every event of targetCalendar)")
        );
        assert!(!APPLE_SCRIPT.contains("description of existingEvent"));
        assert!(!APPLE_SCRIPT.contains("make new calendar"));
        assert!(!APPLE_SCRIPT.contains("delete calendar"));
    }
}
