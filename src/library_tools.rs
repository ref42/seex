//! Offline library operations. Source files are never modified.
use crate::error::{AppError, Result};
use std::path::Path;

pub fn merge(input: &Path, output: &Path) -> Result<String> {
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

pub fn inspect(path: &Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut lines = vec![path.display().to_string()];
    match extension.as_str() {
        "schlib" => {
            for record in crate::altium::merge::read_schlib_records(path)? {
                lines.push(format!(
                    "{} | {} | ID: {} | parts: {}",
                    record.name,
                    record.description,
                    record.identity.as_deref().unwrap_or("unavailable"),
                    record.header_part_count
                ));
            }
        }
        "pcblib" => {
            let library = crate::altium::merge::read_pcblib_records(path)?;
            lines.push(format!("Embedded models: {}", library.models.len()));
            for component in library.components {
                lines.push(format!(
                    "{} | primitives: {}",
                    component.name, component.primitive_count
                ));
            }
        }
        "kicad_sym" | "kicad_mod" => {
            let text = std::fs::read_to_string(path)?;
            let root = crate::kicad::sexpr::parse(&text)?;
            if root.head() == Some("footprint") || root.head() == Some("module") {
                lines.push(format!("Footprint: {}", root.value(1).unwrap_or("unnamed")));
                describe(&root, &mut lines);
            } else if root.head() == Some("kicad_symbol_lib") {
                for symbol in root.children.iter().filter(|n| n.head() == Some("symbol")) {
                    lines.push(format!("Symbol: {}", symbol.value(1).unwrap_or("unnamed")));
                    describe(symbol, &mut lines);
                }
            } else {
                return Err(AppError::Other("Unsupported KiCad library root".into()));
            }
        }
        _ => return Err(AppError::Other("Unsupported library format".into())),
    }
    Ok(lines.join("\n"))
}

fn describe(node: &crate::kicad::sexpr::Node, lines: &mut Vec<String>) {
    for child in &node.children {
        match child.head() {
            Some("property") => lines.push(format!(
                "  {}: {}",
                child.value(1).unwrap_or(""),
                child.value(2).unwrap_or("")
            )),
            Some("descr" | "tags" | "model") => lines.push(format!(
                "  {}: {}",
                child.value(0).unwrap_or(""),
                child.value(1).unwrap_or("")
            )),
            _ => (),
        }
    }
}
