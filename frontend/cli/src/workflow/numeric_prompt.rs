use std::io::{self, Write, stderr};

use crossterm::cursor::MoveToColumn;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};

pub(super) enum PromptError {
    Cancelled,
    Interrupted,
    Io(io::Error),
}

impl From<io::Error> for PromptError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub(super) fn prompt(message: &str, default: i64, minimum: i64) -> Result<i64, PromptError> {
    let mut terminal = RawTerminal::new()?;
    let mut input = NumberInput::new(default, minimum);
    terminal.render_prompt(message, &input)?;

    loop {
        match event::read()? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                match handle_key(&mut input, key) {
                    KeyResult::Redraw => terminal.render_prompt(message, &input)?,
                    KeyResult::Submit => match input.value() {
                        Ok(value) => {
                            terminal.render_answer(message, value)?;
                            return Ok(value);
                        }
                        Err(ValueError::Invalid) => {
                            terminal.render_error("请输入有效整数。")?;
                            terminal.render_prompt(message, &input)?;
                        }
                        Err(ValueError::BelowMinimum) => {
                            terminal.render_error(&format!("结束不能小于起始 {minimum}。"))?;
                            terminal.render_prompt(message, &input)?;
                        }
                    },
                    KeyResult::Cancel => {
                        terminal.render_cancelled(message)?;
                        return Err(PromptError::Cancelled);
                    }
                    KeyResult::Interrupt => return Err(PromptError::Interrupted),
                    KeyResult::Ignore => {}
                }
            }
            Event::Paste(value) => {
                input.push_text(&value);
                terminal.render_prompt(message, &input)?;
            }
            Event::Resize(_, _) => terminal.render_prompt(message, &input)?,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct NumberInput {
    text: String,
    initial: i64,
    minimum: i64,
}

impl NumberInput {
    fn new(default: i64, minimum: i64) -> Self {
        let initial = default.max(minimum);
        Self {
            text: initial.to_string(),
            initial,
            minimum,
        }
    }

    fn display(&self) -> &str {
        &self.text
    }

    fn value(&self) -> Result<i64, ValueError> {
        let value = self.text.parse().map_err(|_| ValueError::Invalid)?;
        if value < self.minimum {
            Err(ValueError::BelowMinimum)
        } else {
            Ok(value)
        }
    }

    fn adjust(&mut self, delta: i64) {
        let current = self.value().unwrap_or(self.initial);
        let adjusted = current.saturating_add(delta).max(self.minimum);
        self.text = adjusted.to_string();
    }

    fn push(&mut self, value: char) {
        if value.is_ascii_digit() || (value == '-' && self.text.is_empty()) {
            self.text.push(value);
        }
    }

    fn push_text(&mut self, value: &str) {
        value.chars().for_each(|character| self.push(character));
    }

    fn pop(&mut self) {
        self.text.pop();
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ValueError {
    Invalid,
    BelowMinimum,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum KeyResult {
    Redraw,
    Submit,
    Cancel,
    Interrupt,
    Ignore,
}

fn handle_key(input: &mut NumberInput, key: KeyEvent) -> KeyResult {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => KeyResult::Interrupt,
            KeyCode::Char('d' | 'g') => KeyResult::Cancel,
            _ => KeyResult::Ignore,
        };
    }

    match key.code {
        KeyCode::Enter => KeyResult::Submit,
        KeyCode::Esc => KeyResult::Cancel,
        KeyCode::Backspace => {
            input.pop();
            KeyResult::Redraw
        }
        KeyCode::Up => {
            input.adjust(1);
            KeyResult::Redraw
        }
        KeyCode::Down => {
            input.adjust(-1);
            KeyResult::Redraw
        }
        KeyCode::Right => {
            input.adjust(10);
            KeyResult::Redraw
        }
        KeyCode::Left => {
            input.adjust(-10);
            KeyResult::Redraw
        }
        KeyCode::Char(value) => {
            input.push(value);
            KeyResult::Redraw
        }
        _ => KeyResult::Ignore,
    }
}

struct RawTerminal {
    output: io::Stderr,
}

impl RawTerminal {
    fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut output = stderr();
        if let Err(error) = execute!(output, EnableBracketedPaste) {
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }
        Ok(Self { output })
    }

    fn render_prompt(&mut self, message: &str, input: &NumberInput) -> io::Result<()> {
        queue!(
            self.output,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            crossterm::style::Print(format!("? {message} {}", input.display()))
        )?;
        self.output.flush()
    }

    fn render_error(&mut self, message: &str) -> io::Result<()> {
        queue!(
            self.output,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            crossterm::style::Print(format!("! {message}\r\n"))
        )?;
        self.output.flush()
    }

    fn render_answer(&mut self, message: &str, value: i64) -> io::Result<()> {
        queue!(
            self.output,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            crossterm::style::Print(format!("? {message} {value}\r\n"))
        )?;
        self.output.flush()
    }

    fn render_cancelled(&mut self, message: &str) -> io::Result<()> {
        queue!(
            self.output,
            MoveToColumn(0),
            Clear(ClearType::CurrentLine),
            crossterm::style::Print(format!("? {message} <canceled>\r\n"))
        )?;
        self.output.flush()
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = execute!(self.output, DisableBracketedPaste);
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn default_is_rendered_in_the_input_text() {
        let input = NumberInput::new(176, 175);

        assert_eq!(input.display(), "176");
    }

    #[test]
    fn arrow_keys_apply_requested_steps_and_clamp_to_minimum() {
        let mut input = NumberInput::new(176, 175);

        assert_eq!(handle_key(&mut input, key(KeyCode::Up)), KeyResult::Redraw);
        assert_eq!(input.value(), Ok(177));
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Down)),
            KeyResult::Redraw
        );
        assert_eq!(input.value(), Ok(176));
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Right)),
            KeyResult::Redraw
        );
        assert_eq!(input.value(), Ok(186));
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Left)),
            KeyResult::Redraw
        );
        assert_eq!(input.value(), Ok(176));
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Left)),
            KeyResult::Redraw
        );
        assert_eq!(input.value(), Ok(175));
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Down)),
            KeyResult::Redraw
        );
        assert_eq!(input.value(), Ok(175));
    }

    #[test]
    fn manual_input_and_paste_are_supported() {
        let mut input = NumberInput::new(176, 175);

        input.pop();
        input.pop();
        input.pop();
        input.push_text("205");
        assert_eq!(input.value(), Ok(205));
        input.push_text("abc");
        assert_eq!(input.value(), Ok(205));
    }

    #[test]
    fn submit_rejects_values_below_start() {
        let mut input = NumberInput::new(176, 175);

        input.text = "174".into();
        assert_eq!(
            handle_key(&mut input, key(KeyCode::Enter)),
            KeyResult::Submit
        );
        assert_eq!(input.value(), Err(ValueError::BelowMinimum));
    }
}
