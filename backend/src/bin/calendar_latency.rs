use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::Parser;
use tracker_backend::ProcessCalendarAdapter;
use tracker_backend::calendar::{CalendarEvent, CalendarPort};
use tracker_backend::domain::CalendarAction;

const LOOP_SCRIPT: &str = r#"
on run argv
    set calendarName to item 1 of argv
    set marker to item 2 of argv

    tell application "Calendar"
        set matchingCalendars to calendars whose name is calendarName
        if (count of matchingCalendars) is 0 then return "calendar_missing"
        if (count of matchingCalendars) is greater than 1 then return "calendar_ambiguous"
        set targetCalendar to item 1 of matchingCalendars
        repeat with existingEvent in (every event of targetCalendar)
            try
                if (description of existingEvent) contains marker then
                    return "existing"
                end if
            end try
        end repeat
        return "missing"
    end tell
end run
"#;

const BULK_DESCRIPTION_SCRIPT: &str = r#"
on run argv
    set calendarName to item 1 of argv
    set marker to item 2 of argv

    tell application "Calendar"
        set matchingCalendars to calendars whose name is calendarName
        if (count of matchingCalendars) is 0 then return "calendar_missing"
        if (count of matchingCalendars) is greater than 1 then return "calendar_ambiguous"
        set targetCalendar to item 1 of matchingCalendars
        set descriptions to description of every event of targetCalendar
        repeat with existingDescription in descriptions
            try
                if (contents of existingDescription) contains marker then return "existing"
            end try
        end repeat
        return "missing"
    end tell
end run
"#;

const PRODUCTION_SCRIPT: &str = include_str!("../../assets/calendar.applescript");

const WHOSE_SCRIPT: &str = r#"
on run argv
    set calendarName to item 1 of argv
    set marker to item 2 of argv

    tell application "Calendar"
        set matchingCalendars to calendars whose name is calendarName
        if (count of matchingCalendars) is 0 then return "calendar_missing"
        if (count of matchingCalendars) is greater than 1 then return "calendar_ambiguous"
        set targetCalendar to item 1 of matchingCalendars
        return (count of (every event of targetCalendar whose description contains marker)) as string
    end tell
end run
"#;

const DATE_WHOSE_SCRIPT: &str = r#"
on run argv
    set calendarName to item 1 of argv
    set marker to item 2 of argv
    set yearNumber to item 3 of argv as integer
    set monthNumber to item 4 of argv as integer
    set dayNumber to item 5 of argv as integer

    set fromDate to current date
    set hours of fromDate to 0
    set minutes of fromDate to 0
    set seconds of fromDate to 0
    set day of fromDate to 1
    set year of fromDate to yearNumber
    set month of fromDate to item monthNumber of {January, February, March, April, May, June, July, August, September, October, November, December}
    set day of fromDate to dayNumber
    set toDate to fromDate + (24 * hours)

    tell application "Calendar"
        set matchingCalendars to calendars whose name is calendarName
        if (count of matchingCalendars) is 0 then return "calendar_missing"
        if (count of matchingCalendars) is greater than 1 then return "calendar_ambiguous"
        set targetCalendar to item 1 of matchingCalendars
        return (count of (every event of targetCalendar whose (start date is greater than or equal to fromDate and start date is less than toDate) and description contains marker)) as string
    end tell
end run
"#;

#[derive(Debug, Parser)]
#[command(about = "Measure macOS Calendar lookup paths without creating events")]
struct Args {
    #[arg(long, default_value = "学习")]
    calendar: String,
    #[arg(long)]
    marker: String,
    #[arg(long, default_value = "2026-08-30")]
    event_date: String,
    #[arg(long, default_value = "21:00:00")]
    start_time: String,
    #[arg(long, default_value = "21:30:00")]
    end_time: String,
    #[arg(long, default_value = "《教材-数量关系》")]
    title: String,
    #[arg(long, default_value_t = 5)]
    runs: usize,
    #[arg(long, default_value_t = 90)]
    timeout_seconds: u64,
    #[arg(long)]
    only: Option<String>,
}

#[derive(Debug)]
struct CommandResult {
    elapsed: Duration,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct Sample {
    name: &'static str,
    elapsed: Vec<Duration>,
}

impl Sample {
    fn report(&self) {
        let mut values = self
            .elapsed
            .iter()
            .map(Duration::as_secs_f64)
            .collect::<Vec<_>>();
        values.sort_by(f64::total_cmp);
        let average = values.iter().sum::<f64>() / values.len() as f64;
        let p50 = values[values.len() / 2];
        println!(
            "{:<24} min={:>8.3} ms p50={:>8.3} ms avg={:>8.3} ms max={:>8.3} ms",
            self.name,
            values[0] * 1_000.0,
            p50 * 1_000.0,
            average * 1_000.0,
            values[values.len() - 1] * 1_000.0,
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args = Args::parse();
    if args.runs == 0 {
        return Err("--runs must be greater than zero".into());
    }
    const CASE_NAMES: &[&str] = &[
        "open -ga Calendar",
        "loop, no open",
        "bulk descriptions, no open",
        "production script, no open",
        "whose, no open",
        "date whose, no open",
        "open + whose",
        "open + production script",
        "Rust adapter (open+production script)",
    ];
    if let Some(selected) = args.only.as_deref()
        && !CASE_NAMES.contains(&selected)
    {
        return Err(format!(
            "unknown --only case {selected:?}; choose one of {CASE_NAMES:?}"
        ));
    }

    let date_parts = split_parts(&args.event_date, "event_date", 3)?;
    let start_parts = split_parts(&args.start_time, "start_time", 3)?;
    let end_parts = split_parts(&args.end_time, "end_time", 3)?;
    let timeout = Duration::from_secs(args.timeout_seconds);
    let marker_args = vec![args.calendar.clone(), args.marker.clone()];
    let preflight = run_osascript(WHOSE_SCRIPT, &marker_args, timeout)?;
    let marker_count = preflight
        .stdout
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("marker preflight returned {:?}", preflight.stdout.trim()))?;
    if marker_count == 0 {
        return Err(format!(
            "marker {:?} was not found in calendar {:?}; refusing to run a script that could create an event",
            args.marker, args.calendar
        ));
    }

    println!(
        "Calendar latency experiment: calendar={:?}, marker_count={}, runs={}",
        args.calendar, marker_count, args.runs
    );
    println!("All cases are existing-marker/read-only paths; no event creation is expected.");
    println!(
        "preflight whose query: {:.3} ms",
        preflight.elapsed.as_secs_f64() * 1_000.0
    );

    let mut samples = Vec::new();
    let mut add_case =
        |name: &'static str, operation: &mut dyn FnMut() -> Result<Duration, String>| {
            if args.only.as_deref().is_none_or(|selected| selected == name) {
                samples.push(measure(name, args.runs, operation)?);
            }
            Ok::<(), String>(())
        };
    add_case("open -ga Calendar", &mut || {
        run_open_calendar().map(|result| result.elapsed)
    })?;
    add_case("loop, no open", &mut || {
        let result = run_osascript(LOOP_SCRIPT, &marker_args, timeout)?;
        expect_output(result, "existing")
    })?;
    add_case("bulk descriptions, no open", &mut || {
        let result = run_osascript(BULK_DESCRIPTION_SCRIPT, &marker_args, timeout)?;
        expect_output(result, "existing")
    })?;
    add_case("production script, no open", &mut || {
        let result = run_osascript(
            PRODUCTION_SCRIPT,
            &production_args(&args, &date_parts, &start_parts, &end_parts),
            timeout,
        )?;
        expect_output(result, "existing")
    })?;
    add_case("whose, no open", &mut || {
        let result = run_osascript(WHOSE_SCRIPT, &marker_args, timeout)?;
        expect_count(result, marker_count)
    })?;
    add_case("date whose, no open", &mut || {
        let mut script_args = marker_args.clone();
        script_args.extend(date_parts.iter().cloned());
        let result = run_osascript(DATE_WHOSE_SCRIPT, &script_args, timeout)?;
        expect_count(result, marker_count)
    })?;
    add_case("open + whose", &mut || {
        run_open_calendar()?;
        let result = run_osascript(WHOSE_SCRIPT, &marker_args, timeout)?;
        expect_count(result, marker_count)
    })?;
    add_case("open + production script", &mut || {
        run_open_calendar()?;
        let result = run_osascript(
            PRODUCTION_SCRIPT,
            &production_args(&args, &date_parts, &start_parts, &end_parts),
            timeout,
        )?;
        expect_output(result, "existing")
    })?;

    let event = CalendarEvent {
        calendar_name: args.calendar.clone(),
        title: args.title,
        event_date: args.event_date,
        start_time: args.start_time,
        end_time: args.end_time,
        description: "calendar-latency existing-marker probe".into(),
        marker: args.marker,
    };
    if args
        .only
        .as_deref()
        .is_none_or(|selected| selected == "Rust adapter (open+production script)")
    {
        let adapter = ProcessCalendarAdapter::new();
        let mut adapter_elapsed = Vec::with_capacity(args.runs);
        for _ in 0..args.runs {
            let started = Instant::now();
            let result = adapter
                .create_or_find(event.clone())
                .await
                .map_err(|error| format!("ProcessCalendarAdapter failed: {error:?}"))?;
            if result != CalendarAction::Existing {
                return Err(format!(
                    "ProcessCalendarAdapter returned {result:?}; refusing to continue"
                ));
            }
            adapter_elapsed.push(started.elapsed());
        }
        samples.push(Sample {
            name: "Rust adapter (open+production script)",
            elapsed: adapter_elapsed,
        });
    }

    println!("\nResults:");
    for sample in &samples {
        sample.report();
    }
    println!(
        "\nEvent date parts used by date-scoped case: {:?}; start={:?}, end={:?}",
        date_parts, start_parts, end_parts
    );
    Ok(())
}

fn measure<F>(name: &'static str, runs: usize, mut operation: F) -> Result<Sample, String>
where
    F: FnMut() -> Result<Duration, String>,
{
    let mut elapsed = Vec::with_capacity(runs);
    for _ in 0..runs {
        elapsed.push(operation()?);
    }
    Ok(Sample { name, elapsed })
}

fn split_parts(value: &str, name: &str, expected: usize) -> Result<Vec<String>, String> {
    let parts = value
        .split(['-', ':'])
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parts.len() != expected || parts.iter().any(|part| part.is_empty()) {
        return Err(format!(
            "{name} must have {expected} numeric components: {value:?}"
        ));
    }
    Ok(parts)
}

fn run_open_calendar() -> Result<CommandResult, String> {
    let started = Instant::now();
    let output = Command::new("/usr/bin/open")
        .args(["-g", "-a", "Calendar"])
        .output()
        .map_err(|error| format!("open failed to start: {error}"))?;
    let result = CommandResult {
        elapsed: started.elapsed(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    if !output.status.success() {
        return Err(format!(
            "open exited with {}: {}",
            output.status,
            result.stderr.trim()
        ));
    }
    Ok(result)
}

fn run_osascript(
    script: &str,
    arguments: &[String],
    timeout: Duration,
) -> Result<CommandResult, String> {
    let started = Instant::now();
    let mut child = Command::new("/usr/bin/osascript")
        .arg("-")
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("osascript failed to start: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "osascript stdin is unavailable".to_owned())?
        .write_all(script.as_bytes())
        .map_err(|error| format!("failed to write AppleScript: {error}"))?;
    let output = wait_with_timeout(child, timeout)?;
    let result = CommandResult {
        elapsed: started.elapsed(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    if !output.status.success() {
        return Err(format!(
            "osascript exited with {}: {}",
            output.status,
            result.stderr.trim()
        ));
    }
    Ok(result)
}

fn expect_count(result: CommandResult, expected: usize) -> Result<Duration, String> {
    let actual = result.stdout.trim().parse::<usize>().map_err(|_| {
        format!(
            "marker query returned {:?}, stderr={:?}",
            result.stdout.trim(),
            result.stderr.trim()
        )
    })?;
    if actual != expected {
        return Err(format!(
            "marker query returned {actual}, expected {expected}"
        ));
    }
    Ok(result.elapsed)
}

fn expect_output(result: CommandResult, expected: &str) -> Result<Duration, String> {
    let actual = result.stdout.trim();
    if actual != expected {
        return Err(format!(
            "AppleScript returned {actual:?}, expected {expected:?}, stderr={:?}",
            result.stderr.trim()
        ));
    }
    Ok(result.elapsed)
}

fn production_args(
    args: &Args,
    date_parts: &[String],
    start_parts: &[String],
    end_parts: &[String],
) -> Vec<String> {
    let mut values = vec![args.calendar.clone(), args.title.clone()];
    values.extend(date_parts.iter().cloned());
    values.extend(start_parts.iter().cloned());
    values.extend(date_parts.iter().cloned());
    values.extend(end_parts.iter().cloned());
    values.push("calendar-latency existing-marker probe".into());
    values.push(args.marker.clone());
    values
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|error| format!("osascript failed: {error}"));
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("osascript exceeded {}s timeout", timeout.as_secs()));
            }
            Err(error) => return Err(format!("failed to poll osascript: {error}")),
        }
    }
}
