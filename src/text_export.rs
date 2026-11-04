//! Shared spreadsheet and plain-text table exports.
use crate::error::{AppError, Result};
use std::path::Path;

#[derive(Clone, Copy)]
pub enum Format {
    Csv,
    Markdown,
}
impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Markdown => "md",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Markdown => "Markdown",
        }
    }
}

pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}
impl Table {
    pub fn encode(&self, format: Format) -> Vec<u8> {
        match format {
            Format::Csv => {
                let mut text = String::from("\u{feff}");
                for row in std::iter::once(&self.headers).chain(&self.rows) {
                    text.push_str(
                        &row.iter()
                            .map(|v| crate::bom::csv_field(v))
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                    text.push_str("\r\n");
                }
                text.into_bytes()
            }
            Format::Markdown => {
                let mut text = String::new();
                let mut row = |cells: &[String]| {
                    text.push_str("| ");
                    text.push_str(
                        &cells
                            .iter()
                            .map(|s| markdown_cell(s))
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    text.push_str(" |\n");
                };
                row(&self.headers);
                row(&vec!["---".to_owned(); self.headers.len()]);
                for cells in &self.rows {
                    row(cells);
                }
                text.into_bytes()
            }
        }
    }
}

fn markdown_cell(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace('\\', "&#92;")
        .replace('`', "&#96;")
        .replace('*', "&#42;")
        .replace('_', "&#95;")
        .replace('[', "&#91;")
        .replace(']', "&#93;")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "<br>")
}

/// Escape plain inspection output for the selectable Markdown text widget.
pub fn selectable_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            let mut escaped = String::new();
            for ch in line.chars() {
                if ch.is_ascii_punctuation() {
                    escaped.push('\\');
                }
                escaped.push(ch);
            }
            escaped
        })
        .collect::<Vec<_>>()
        .join("  \n")
}

pub fn write(output: &Path, bytes: &[u8]) -> Result<()> {
    if output.is_dir() {
        return Err(AppError::Other(format!(
            "{} is a folder. Choose a filename ending in .csv or .md.",
            output.display()
        )));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = parent.join(format!(
        ".seex-export-{}-{}.tmp",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| io_error("save export", output, e))?;
    let result = (|| -> std::io::Result<()> {
        use std::io::Write;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, output)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| io_error("save export", output, e))
}

pub fn io_error(action: &str, path: &Path, error: std::io::Error) -> AppError {
    let help = if error.kind() == std::io::ErrorKind::PermissionDenied
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)))
    {
        " Close any application locking this file, or choose a writable folder and a new filename."
    } else {
        ""
    };
    AppError::Other(format!(
        "Cannot {action} '{}': {error}.{help}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_escapes_table_and_html_content_without_losing_unicode() {
        let table = Table {
            headers: vec!["description".into()],
            rows: vec![vec!["中文 | <script>\n±20% Ω ℃".into()]],
        };
        let text = String::from_utf8(table.encode(Format::Markdown)).unwrap();
        assert!(text.contains("| --- |"));
        assert!(text.contains("&#124;"));
        assert!(text.contains("&lt;script&gt;<br>±20% Ω ℃"));
        assert!(!text.contains("<script>"));
    }

    #[test]
    fn access_errors_include_the_path_and_recovery_action() {
        let message = io_error(
            "save export",
            Path::new("inventory.csv"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        )
        .to_string();
        assert!(message.contains("inventory.csv"));
        assert!(message.contains("writable folder"));
    }
}
