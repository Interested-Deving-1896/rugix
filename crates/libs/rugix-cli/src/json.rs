//! Utilities for printing JSON output to stdout.
//!
//! When stdout is a terminal, JSON is pretty-printed with ANSI colors.
//!
//! When stdout is piped, compact JSON is emitted.

use std::io::{self, Write};

use serde::Serialize;

use crate::style::{Color, Modifier, RESET_ALL};

/// Write a serializable value as JSON to stdout.
///
/// When `compact` is `false` and stdout is a terminal, the output is pretty-printed with
/// syntax colors. Otherwise, compact JSON is emitted.
pub fn print_json(value: &impl Serialize, compact: bool) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    if compact || crate::stdout_is_piped() {
        serde_json::to_writer(&mut stdout, value).map_err(io::Error::other)?;
    } else {
        let json_value = serde_json::to_value(value).map_err(io::Error::other)?;
        let colored = format_colored(&json_value, 0);
        stdout.write_all(colored.as_bytes())?;
    }
    stdout.write_all(b"\n")?;
    Ok(())
}

/// Format a JSON value as a pretty-printed string with ANSI colors.
fn format_colored(value: &serde_json::Value, indent: usize) -> String {
    let mut out = String::new();
    write_value(&mut out, value, indent);
    out
}

fn write_value(out: &mut String, value: &serde_json::Value, indent: usize) {
    match value {
        serde_json::Value::Null => {
            write_styled(out, "null", Color::DarkGray, true);
        }
        serde_json::Value::Bool(b) => {
            write_styled(out, &b.to_string(), Color::Magenta, false);
        }
        serde_json::Value::Number(n) => {
            write_styled(out, &n.to_string(), Color::Yellow, false);
        }
        serde_json::Value::String(s) => {
            let escaped = serde_json::to_string(s).unwrap();
            write_styled(out, &escaped, Color::Green, false);
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in arr.iter().enumerate() {
                write_indent(out, indent + 1);
                write_value(out, item, indent + 1);
                if i + 1 < arr.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent);
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (key, val)) in map.iter().enumerate() {
                write_indent(out, indent + 1);
                let escaped_key = serde_json::to_string(key).unwrap();
                write_styled(out, &escaped_key, Color::Cyan, true);
                out.push_str(": ");
                write_value(out, val, indent + 1);
                if i + 1 < map.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent);
            out.push('}');
        }
    }
}

fn write_indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

fn write_styled(out: &mut String, text: &str, color: Color, bold: bool) {
    out.push_str(color.foreground_ansi_sequence());
    if bold {
        out.push_str(Modifier::Bold.enable_ansi_sequence());
    }
    out.push_str(text);
    out.push_str(RESET_ALL);
}
