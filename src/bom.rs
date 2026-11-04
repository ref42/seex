use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::monitor::{self, MonitorState};

pub fn spawn_save_matched(state: Arc<Mutex<MonitorState>>, format: crate::text_export::Format) {
    if let Ok(mut s) = state.lock() {
        if s.bom_running {
            return;
        }
        s.bom_running = true;
        s.bom_last_result = Some(format!("Exporting BOM {}...", format.label()));
    }

    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|err| err.to_string())
            .and_then(|runtime| runtime.block_on(save_matched_inner(Arc::clone(&state), format)));

        let message = match result {
            Ok(message) => message,
            Err(err) => format!("Export failed: {}", err),
        };

        if let Ok(mut s) = state.lock() {
            s.bom_running = false;
            s.bom_last_result = Some(message.clone());
            s.add_debug_log(message);
        }
    });
}

async fn save_matched_inner(
    state: Arc<Mutex<MonitorState>>,
    format: crate::text_export::Format,
) -> Result<String, String> {
    let (path, entries, language) = {
        let m = state.lock().map_err(|_| "State lock failed".to_string())?;
        if m.matched.is_empty() {
            return Ok("No matched results to export".to_string());
        }
        (
            PathBuf::from(&m.matched_save_path).with_extension(format.extension()),
            m.matched.clone(),
            m.bom_language.clone(),
        )
    };

    if let Err(err) = ensure_parent_dir(&path) {
        return Err(crate::text_export::io_error("create export folder", &path, err).to_string());
    }

    let metadata = load_bom_metadata(&entries, &language).await;
    crate::text_export::write(&path, &bom_table(&entries, &metadata).encode(format))
        .map_err(|e| e.to_string())?;
    let unavailable = metadata
        .iter()
        .filter(|row| row.mpn.is_empty() && row.description.is_empty())
        .count();
    Ok(format!(
        "Exported BOM {} to {} ({}; {} rows; metadata unavailable for {})",
        format.label(),
        path.display(),
        metadata_language_label(&language),
        entries.len(),
        unavailable
    ))
}

fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

#[cfg(test)]
fn write_bom_csv(
    output: &Path,
    entries: &[monitor::MatchedEntry],
    metadata: &[BomRow],
) -> Result<(), String> {
    crate::text_export::write(
        output,
        &bom_table(entries, metadata).encode(crate::text_export::Format::Csv),
    )
    .map_err(|e| e.to_string())
}

fn bom_table(entries: &[monitor::MatchedEntry], metadata: &[BomRow]) -> crate::text_export::Table {
    let headers = "lcsc_id,quantity,mpn,manufacturer,description,package,datasheet_url,component_url,capture_count,first_seen,last_seen,metadata_status".split(',').map(str::to_owned).collect();
    let rows = entries
        .iter()
        .map(|entry| {
            let row = metadata
                .iter()
                .find(|row| row.lcsc_id == entry.lcsc_id)
                .cloned()
                .unwrap_or_else(|| BomRow::from_entry(entry));
            let status = if row.mpn.is_empty() && row.description.is_empty() {
                "unavailable"
            } else {
                "available"
            };
            vec![
                entry.lcsc_id.clone(),
                "1".into(),
                row.mpn,
                row.manufacturer,
                row.description,
                row.package_name,
                row.datasheet_url,
                format!("https://www.lcsc.com/product-detail/{}.html", entry.lcsc_id),
                entry.count.to_string(),
                entry.first_seen.clone(),
                entry.last_seen.clone(),
                status.into(),
            ]
        })
        .collect();
    crate::text_export::Table { headers, rows }
}

#[derive(Clone)]
struct BomRow {
    lcsc_id: String,
    mpn: String,
    manufacturer: String,
    description: String,
    package_name: String,
    datasheet_url: String,
}

impl BomRow {
    fn from_entry(entry: &monitor::MatchedEntry) -> Self {
        Self {
            lcsc_id: entry.lcsc_id.clone(),
            mpn: String::new(),
            manufacturer: String::new(),
            description: String::new(),
            package_name: String::new(),
            datasheet_url: String::new(),
        }
    }
}

async fn load_bom_metadata(entries: &[monitor::MatchedEntry], language: &str) -> Vec<BomRow> {
    let english = language.eq_ignore_ascii_case("english");
    let lcsc_client = if english {
        Some(crate::lcsc::LcscClient::new())
    } else {
        None
    };
    let lceda_client = crate::lceda::LcedaClient::new();
    let mut rows = Vec::new();
    for entry in entries {
        let row = if let Some(client) = &lcsc_client {
            match client.product_detail(&entry.lcsc_id).await {
                Ok(product) => BomRow {
                    lcsc_id: product.sku,
                    mpn: product.mpn.unwrap_or_default(),
                    manufacturer: product.manufacturer.unwrap_or_default(),
                    description: product.description.unwrap_or_default(),
                    package_name: product
                        .properties
                        .iter()
                        .find(|property| property.name.eq_ignore_ascii_case("package"))
                        .map(|property| property.value.clone())
                        .unwrap_or_default(),
                    datasheet_url: product.datasheet_url.unwrap_or_default(),
                },
                Err(_) => BomRow::from_entry(entry),
            }
        } else {
            match lceda_client.select_item(&entry.lcsc_id, 1).await {
                Ok(item) => BomRow {
                    lcsc_id: item.lcsc_id().unwrap_or_else(|| entry.lcsc_id.clone()),
                    mpn: attribute_text(&item.raw, "Manufacturer Part"),
                    manufacturer: item.manufacturer.clone(),
                    description: first_non_empty([
                        attribute_text(&item.raw, "LCSC Part Name"),
                        value_text(&item.raw, &["description"]),
                        item.display_name().to_string(),
                    ]),
                    package_name: first_non_empty([
                        attribute_text(&item.raw, "Package"),
                        attribute_text(&item.raw, "Supplier Footprint"),
                        attribute_text(&item.raw, "Footprint"),
                    ]),
                    datasheet_url: attribute_text(&item.raw, "Datasheet"),
                },
                Err(_) => BomRow::from_entry(entry),
            }
        };
        rows.push(row);
    }
    rows
}

fn attribute_text(value: &Value, name: &str) -> String {
    value
        .get("attributes")
        .and_then(Value::as_object)
        .and_then(|attrs| attrs.get(name))
        .and_then(value_to_csv_text)
        .unwrap_or_default()
}

fn value_text(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        current = next;
    }
    value_to_csv_text(current).unwrap_or_default()
}

fn value_to_csv_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn first_non_empty(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
}

pub(crate) fn csv_field(value: &str) -> String {
    // Quoting alone does not prevent spreadsheets interpreting formulas.
    let trimmed = value.trim_start();
    let safe = if trimmed.starts_with(['=', '+', '-', '@']) || value.starts_with(['\t', '\r', '\n'])
    {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    let escaped = safe.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

fn metadata_language_label(language: &str) -> &'static str {
    if language.eq_ignore_ascii_case("english") {
        "English metadata"
    } else {
        "Chinese metadata"
    }
}

#[cfg(test)]
mod csv_tests {
    use super::*;
    #[test]
    fn exported_file_has_bom_and_separates_quantity_from_captures() {
        let path = std::env::temp_dir().join(format!(
            "seex-csv-{}-{}.csv",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let entry = monitor::MatchedEntry {
            lcsc_id: "C2040".into(),
            first_seen: "2026-11-04".into(),
            last_seen: "2026-11-04".into(),
            count: 7,
            source_text: "PRIVATE CLIPBOARD".into(),
        };
        write_bom_csv(&path, &[entry], &[]).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert!(data.starts_with(b"\xEF\xBB\xBF"));
        let text = std::str::from_utf8(&data[3..]).unwrap();
        assert!(text.contains("\r\n\"C2040\",\"1\","));
        assert!(text.contains(",\"7\","));
        assert!(text.contains("unavailable"));
        assert!(!text.contains("PRIVATE CLIPBOARD"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn bom_markdown_includes_same_metadata_without_clipboard_text() {
        let entry = monitor::MatchedEntry {
            lcsc_id: "C2040".into(),
            first_seen: "today".into(),
            last_seen: "today".into(),
            count: 7,
            source_text: "PRIVATE".into(),
        };
        let mut row = BomRow::from_entry(&entry);
        row.description = "中文 ±20% 1.1Ω -25℃".into();
        let text = String::from_utf8(
            bom_table(&[entry], &[row]).encode(crate::text_export::Format::Markdown),
        )
        .unwrap();
        assert!(text.contains("中文 ±20% 1.1Ω -25℃"));
        assert!(text.contains("| C2040 | 1 |"));
        assert!(text.contains("| 7 |"));
        assert!(!text.contains("PRIVATE"));
    }

    #[test]
    fn csv_preserves_unicode_quotes_and_blocks_formulas() {
        assert_eq!(
            csv_field("中文,\"quoted\"\nline"),
            "\"中文,\"\"quoted\"\"\nline\""
        );
        assert_eq!(csv_field(" =HYPERLINK(1)"), "\"' =HYPERLINK(1)\"");
        assert_eq!(csv_field("C2040"), "\"C2040\"");
    }
}
