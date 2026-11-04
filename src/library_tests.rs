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
