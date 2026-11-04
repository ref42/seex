use crate::{kicad, library_tools};
use std::fs;

#[test]
fn kicad_merge_preserves_properties_and_rejects_conflicts_without_output() {
    let root = std::env::temp_dir().join(format!(
        "seex-merge-test-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    let symbol = |value: &str| {
        format!(
            "(kicad_symbol_lib (version 20231120) (generator test) (symbol \"Handmade\" (property \"LCSC\" \"{value}\") (symbol \"Handmade_0_1\" (rectangle (start 0 0) (end 1 1)))) )"
        )
    };
    fs::write(root.join("a.kicad_sym"), symbol("C2040")).unwrap();
    fs::write(root.join("b.kicad_sym"), symbol("C2040")).unwrap();
    let out = root.join("Merged");
    let summary = kicad::merge::merge_library_directory(&root, &out, false).unwrap();
    assert_eq!(summary.symbols, 1);
    let report = library_tools::inspect(&root.join("Merged.kicad_sym")).unwrap();
    assert!(report.contains("C2040") && report.contains("Handmade"));
    let bytes = fs::read(root.join("Merged.kicad_sym")).unwrap();
    assert!(kicad::merge::merge_library_directory(&root, &out, false).is_err());
    assert_eq!(bytes, fs::read(root.join("Merged.kicad_sym")).unwrap());
    fs::write(root.join("b.kicad_sym"), symbol("C4242")).unwrap();
    assert!(kicad::merge::merge_library_directory(&root, &root.join("Conflict"), false).is_err());
    assert!(!root.join("Conflict.kicad_sym").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn resuming_monitoring_does_not_import_private_clipboard() {
    let mut state = crate::monitor::MonitorState::new();
    state.set_monitoring(false);
    assert!(!state.process_clipboard_change("C2040".into()));
    state.set_monitoring(true);
    assert!(!state.process_clipboard_change("C2040".into()));
    assert!(state.matched.is_empty());
    assert!(state.process_clipboard_change("C4242".into()));
    assert_eq!(state.get_unique_ids(), vec!["C4242"]);
}

#[test]
fn library_csv_exports_the_inspected_snapshot_and_custom_properties() {
    let root = std::env::temp_dir().join(format!(
        "seex-library-csv-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    fs::create_dir(&root).unwrap();
    let source = root.join("parts.kicad_sym");
    fs::write(
        &source,
        r#"(kicad_symbol_lib (version 20231120)
        (symbol "Part A" (property "LCSC" "C2040")
            (property "Description" "中文,\"quoted\"")
            (property "Custom" "=1+1"))
        (symbol "Part B" (property "Notes" "first\nsecond")))"#,
    )
    .unwrap();
    let inspection = library_tools::read_library(&source).unwrap();
    assert_eq!(inspection.rows.len(), 2);
    assert_eq!(inspection.rows[0].id, "C2040");
    assert_eq!(inspection.rows[0].description, "中文,\"quoted\"");
    let original = fs::read(&source).unwrap();
    assert!(inspection.export_csv(&source).is_err());
    assert_eq!(fs::read(&source).unwrap(), original);
    // Export uses the displayed data even if the library has since moved.
    fs::remove_file(&source).unwrap();
    let output = root.join("inventory.csv");
    fs::write(&output, b"old export").unwrap();
    inspection.export_csv(&output).unwrap();
    let bytes = fs::read(output).unwrap();
    assert!(bytes.starts_with(b"\xEF\xBB\xBF"));
    let text = std::str::from_utf8(&bytes[3..]).unwrap();
    assert!(text.contains("\"property:Custom\""));
    assert!(text.contains("\"property:Notes\""));
    assert!(text.contains("\"中文,\"\"quoted\"\"\""));
    assert!(text.contains("\"'=1+1\""));
    assert!(text.contains("\"first\nsecond\""));
    assert!(text.ends_with("\r\n"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pcb_inventory_csv_includes_all_displayed_counts() {
    let source = std::path::PathBuf::from("example.PcbLib");
    let inspection = library_tools::LibraryInspection {
        source,
        format: "pcblib".into(),
        embedded_models: Some(30),
        rows: vec![library_tools::LibraryRow {
            name: "C0603".into(),
            primitives: Some(50),
            ..Default::default()
        }],
    };
    assert!(inspection.report().contains("primitives: 50"));
    let output = std::env::temp_dir().join(format!(
        "seex-pcb-inventory-{}-{}.csv",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    inspection.export_csv(&output).unwrap();
    let csv = fs::read_to_string(&output).unwrap();
    assert!(csv.contains("\"primitive_count\",\"embedded_model_count\""));
    assert!(csv.contains("\"C0603\",\"\",\"\",\"\",\"50\",\"30\""));
    fs::remove_file(output).unwrap();
}

#[test]
fn library_markdown_preserves_unicode_and_properties() {
    let inspection = library_tools::LibraryInspection {
        source: "parts.SchLib".into(),
        format: "schlib".into(),
        embedded_models: None,
        rows: vec![library_tools::LibraryRow {
            name: "R1".into(),
            description: "±20% 1.1Ω -25℃ 中文".into(),
            ..Default::default()
        }],
    };
    let output = std::env::temp_dir().join(format!(
        "seex-inventory-md-{}-{}.md",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    inspection
        .export(&output, crate::text_export::Format::Markdown)
        .unwrap();
    let text = fs::read_to_string(&output).unwrap();
    assert!(text.contains("±20% 1.1Ω -25℃ 中文"));
    assert!(text.contains("| --- |"));
    fs::remove_file(output).unwrap();
}

/// Run locally with SEEX_TEST_SCHLIB pointing to an external regression sample.
#[test]
#[ignore = "requires an external Altium library sample"]
fn external_altium_unicode_sample() {
    let source = std::env::var_os("SEEX_TEST_SCHLIB").expect("set SEEX_TEST_SCHLIB");
    let inspection = library_tools::read_library(std::path::Path::new(&source)).unwrap();
    let text = inspection.report();
    assert!(text.contains("℃"));
    assert!(text.contains("±20%"));
    assert!(text.contains("1.1Ω"));
}
