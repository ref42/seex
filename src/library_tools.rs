//! Offline library operations. Source files are never modified.
use crate::error::{AppError, Result};
use std::path::Path;

pub fn merge(input: &Path, output: &Path) -> Result<String> {
    if output.is_dir() {
        return Err(AppError::Other(format!(
            "{} is a folder. Choose a NEW library base filename inside a writable folder.",
            output.display()
        )));
    }
    merge_inner(input, output).map_err(|error| match error {
        AppError::Io(error) => crate::text_export::io_error(
            &format!("merge libraries from {} into", input.display()),
            output,
            error,
        ),
        other => other,
    })
}
fn merge_inner(input: &Path, output: &Path) -> Result<String> {
    let altium = crate::altium::merge::directory_has_libraries(input)?;
    let kicad = crate::kicad::merge::directory_has_libraries(input)?;
    if altium && kicad {
        return Err(AppError::Other("This folder contains both Altium and KiCad libraries. Place each format in its own folder before merging.".into()));
    }
    if altium {
        let summary = crate::altium::merge::merge_library_directory(input, output, false)?;
        Ok(format!(
            "Merged {} schematic components and {} footprints from {} files.\n{}",
            summary.schlib_components,
            summary.pcblib_components,
            summary.schlib_files + summary.pcblib_files,
            summary
                .outputs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ))
    } else if kicad {
        let summary = crate::kicad::merge::merge_library_directory(input, output, false)?;
        Ok(format!(
            "Merged {} symbols, {} footprints and {} models.\n{}",
            summary.symbols,
            summary.footprints,
            summary.models,
            summary
                .outputs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ))
    } else {
        Err(AppError::Other(
            "No supported libraries found in the selected folder.".into(),
        ))
    }
}

#[derive(Clone, Default)]
pub struct LibraryRow {
    pub name: String,
    pub description: String,
    pub id: String,
    pub parts: Option<usize>,
    pub primitives: Option<i32>,
    pub properties: std::collections::BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct LibraryInspection {
    pub source: std::path::PathBuf,
    pub format: String,
    pub embedded_models: Option<usize>,
    pub rows: Vec<LibraryRow>,
}

impl LibraryInspection {
    pub fn report(&self) -> String {
        let mut lines = vec![
            self.source.display().to_string(),
            format!("Components: {}", self.rows.len()),
        ];
        if let Some(count) = self.embedded_models {
            lines.push(format!("Embedded models: {count}"));
        }
        for row in &self.rows {
            let mut line = row.name.clone();
            if !row.description.is_empty() {
                line.push_str(&format!(" | {}", row.description));
            }
            if !row.id.is_empty() {
                line.push_str(&format!(" | ID: {}", row.id));
            }
            if let Some(count) = row.parts {
                line.push_str(&format!(" | parts: {count}"));
            }
            if let Some(count) = row.primitives {
                line.push_str(&format!(" | primitives: {count}"));
            }
            lines.push(line);
            for (key, value) in &row.properties {
                lines.push(format!("  {key}: {value}"));
            }
        }
        lines.join("\n")
    }

    pub fn export_csv(&self, output: &Path) -> Result<()> {
        self.export(output, crate::text_export::Format::Csv)
    }

    pub fn export(&self, output: &Path, format: crate::text_export::Format) -> Result<()> {
        let same_file = output
            .canonicalize()
            .ok()
            .zip(self.source.canonicalize().ok())
            .is_some_and(|(output, source)| output == source);
        if output == self.source || same_file {
            return Err(AppError::Other(
                "Choose an export path different from the source library".into(),
            ));
        }
        crate::text_export::write(output, &self.table().encode(format))
    }

    fn table(&self) -> crate::text_export::Table {
        let keys: std::collections::BTreeSet<_> = self
            .rows
            .iter()
            .flat_map(|r| r.properties.keys().cloned())
            .collect();
        let mut rows = Vec::new();
        let mut header: Vec<String> = [
            "source_library",
            "format",
            "name",
            "component_id",
            "description",
            "part_count",
            "primitive_count",
            "embedded_model_count",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        header.extend(keys.iter().map(|k| format!("property:{k}")));

        for row in &self.rows {
            let mut cells = vec![
                self.source.display().to_string(),
                self.format.clone(),
                row.name.clone(),
                row.id.clone(),
                row.description.clone(),
                row.parts.map(|n| n.to_string()).unwrap_or_default(),
                row.primitives.map(|n| n.to_string()).unwrap_or_default(),
                self.embedded_models
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            ];
            cells.extend(
                keys.iter()
                    .map(|key| row.properties.get(key).cloned().unwrap_or_default()),
            );
            rows.push(cells);
        }
        crate::text_export::Table {
            headers: header,
            rows,
        }
    }
}

#[cfg(test)]
pub fn inspect(path: &Path) -> Result<String> {
    Ok(read_library(path)?.report())
}

pub fn read_library(path: &Path) -> Result<LibraryInspection> {
    read_library_inner(path).map_err(|error| match error {
        AppError::Io(error) => crate::text_export::io_error("read library", path, error),
        other => other,
    })
}

fn read_library_inner(path: &Path) -> Result<LibraryInspection> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut inspection = LibraryInspection {
        source: path.to_owned(),
        format: extension.clone(),
        embedded_models: None,
        rows: Vec::new(),
    };
    match extension.as_str() {
        "schlib" => {
            for record in crate::altium::merge::read_schlib_records(path)? {
                inspection.rows.push(LibraryRow {
                    name: record.name,
                    description: record.description,
                    id: record.identity.unwrap_or_default(),
                    parts: Some(record.header_part_count),
                    ..Default::default()
                });
            }
        }
        "pcblib" => {
            let library = crate::altium::merge::read_pcblib_records(path)?;
            inspection.embedded_models = Some(library.models.len());
            for component in library.components {
                inspection.rows.push(LibraryRow {
                    name: component.name,
                    primitives: Some(component.primitive_count),
                    ..Default::default()
                });
            }
        }
        "kicad_sym" | "kicad_mod" => {
            let text = std::fs::read_to_string(path)?;
            let root = crate::kicad::sexpr::parse(&text)?;
            if matches!(root.head(), Some("footprint" | "module")) {
                inspection.rows.push(describe(&root));
            } else if root.head() == Some("kicad_symbol_lib") {
                for symbol in root.children.iter().filter(|n| n.head() == Some("symbol")) {
                    inspection.rows.push(describe(symbol));
                }
            } else {
                return Err(AppError::Other("Unsupported KiCad library root".into()));
            }
        }
        _ => return Err(AppError::Other("Unsupported library format".into())),
    }
    Ok(inspection)
}

fn describe(node: &crate::kicad::sexpr::Node) -> LibraryRow {
    let mut row = LibraryRow {
        name: node.value(1).unwrap_or("unnamed").to_owned(),
        ..Default::default()
    };
    for child in &node.children {
        let (key, value) = match child.head() {
            Some("property") => (child.value(1).unwrap_or(""), child.value(2).unwrap_or("")),
            Some("descr" | "tags" | "model") => {
                (child.value(0).unwrap_or(""), child.value(1).unwrap_or(""))
            }
            _ => continue,
        };
        if key.eq_ignore_ascii_case("description") || key == "descr" {
            row.description = value.to_owned();
        }
        if [
            "LCSC",
            "LCSC Part",
            "LCSC Part #",
            "SEEX_COMPONENT_ID",
            "NPNP_COMPONENT_ID",
        ]
        .iter()
        .any(|k| key.eq_ignore_ascii_case(k))
        {
            row.id = value.to_owned();
        }
        row.properties
            .entry(key.to_owned())
            .and_modify(|text| {
                text.push('\n');
                text.push_str(value);
            })
            .or_insert_with(|| value.to_owned());
    }
    row
}
