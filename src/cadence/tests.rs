use crate::cadence::allegro::{
    AllegroFootprint, StepModel, StepTransform, allegro_identifier, build_allegro_footprint_with,
    footprint_name_for, library_script,
};
use crate::cadence::capture::{
    CAPTURE_GRID, CapturePart, PinLabels, build_capture_part_with, capture_identifier,
    render_library_xml,
};
use crate::kicad::footprint::{
    Drill, KiArc as FpArc, KiCircle as FpCircle, KiFootprint, KiLine, KiPad, PadShape, PadType,
};
use crate::kicad::symbol::{
    KiCircle, KiPin, KiPolyline, KiRectangle, KiSymbol, KiSymbolUnit, PinStyle, PinType,
};

fn build_allegro_footprint(footprint: &KiFootprint, name: &str, refdes: &str) -> AllegroFootprint {
    build_allegro_footprint_with(footprint, name, refdes, None)
}

fn build_capture_part(symbol: &KiSymbol, name: &str, footprint: &str) -> CapturePart {
    build_capture_part_with(symbol, name, footprint, PinLabels::default())
}

/// Padstacks a footprint script creates, in order.
fn padstacks(script: &str) -> Vec<String> {
    script
        .match_indices("seex_padstack(\"")
        .map(|(at, call)| {
            let rest = &script[at + call.len()..];
            rest[..rest.find('"').unwrap()].to_string()
        })
        .collect()
}

fn pin(number: &str, name: &str, x: f64, y: f64, rotation: i32, pin_type: PinType) -> KiPin {
    KiPin {
        number: number.to_string(),
        name: name.to_string(),
        pin_type,
        style: PinStyle::Line,
        pos_x: x,
        pos_y: y,
        rotation,
        length: 10.0,
    }
}

/// Op-amp style symbol in the shared model (EasyEDA px, Y up), body 40x40.
pub(crate) fn sample_symbol() -> KiSymbol {
    KiSymbol {
        name: "LM321_C7950".to_string(),
        reference: "U?".to_string(),
        value: "LM321 <op-amp> & \"rail\"".to_string(),
        description: "Single op-amp".to_string(),
        footprint: String::new(),
        datasheet: "https://example.test/ds?a=1&b=2".to_string(),
        manufacturer: "TI".to_string(),
        lcsc_id: "C7950".to_string(),
        jlc_id: String::new(),
        units: Vec::new(),
        pins: vec![
            // Left side pins point right (KiCad orientation 0 => EasyEDA rotation 180).
            pin("3", "+IN", -10.0, 10.0, 180, PinType::Input),
            pin("4", "-IN", -10.0, -10.0, 180, PinType::Input),
            // Right side pin.
            pin("1", "OUT", 50.0, 0.0, 0, PinType::Output),
            // Top / bottom power pins (KiCad orientation 270 / 90 => EasyEDA 90 / 270).
            pin("5", "V+", 20.0, 30.0, 90, PinType::PowerIn),
            pin("2", "V-", 20.0, -30.0, 270, PinType::PowerIn),
        ],
        rectangles: vec![KiRectangle {
            x1: 0.0,
            y1: 20.0,
            x2: 40.0,
            y2: -20.0,
            stroke_width: 1.0,
            fill: true,
        }],
        circles: vec![KiCircle {
            cx: 35.0,
            cy: 0.0,
            radius: 2.0,
            stroke_width: 1.0,
            fill: false,
        }],
        arcs: Vec::new(),
        polylines: vec![KiPolyline {
            points: vec![(5.0, 15.0), (30.0, 0.0), (5.0, -15.0), (5.0, 15.0)],
            stroke_width: 1.0,
            fill: true,
        }],
        texts: Vec::new(),
    }
}

fn attr(xml: &str, element_start: usize, name: &str) -> String {
    let rest = &xml[element_start..];
    let key = format!(" {name}=\"");
    let start = rest.find(&key).expect("attribute present") + key.len();
    let end = rest[start..].find('"').unwrap() + start;
    rest[start..end].to_string()
}

#[test]
fn capture_pins_land_on_grid_and_point_away_from_body() {
    let part = build_capture_part(&sample_symbol(), "LM321_C7950", "c7950_lm321");
    let xml = &part.xml;
    let mut count = 0;
    for (index, _) in xml.match_indices("<SymbolPinScalar>") {
        count += 1;
        let hx: i64 = attr(xml, index, "hotptX").parse().unwrap();
        let hy: i64 = attr(xml, index, "hotptY").parse().unwrap();
        let sx: i64 = attr(xml, index, "startX").parse().unwrap();
        let sy: i64 = attr(xml, index, "startY").parse().unwrap();
        assert_eq!(hx % CAPTURE_GRID, 0, "hotptX on grid");
        assert_eq!(hy % CAPTURE_GRID, 0, "hotptY on grid");
        assert_eq!((hx - sx).abs() + (hy - sy).abs(), 10, "pin length kept");
        match attr(xml, index, "name").as_str() {
            "+IN" | "-IN" => assert!(sx > hx, "left pins extend right into the body"),
            "OUT" => assert!(sx < hx, "right pin extends left into the body"),
            // Capture Y grows downwards: V+ is on top so its body end is below it.
            "V+" => assert!(sy > hy),
            "V-" => assert!(sy < hy),
            other => panic!("unexpected pin {other}"),
        }
    }
    assert_eq!(count, 5);
    assert!(part.warnings.is_empty(), "{:?}", part.warnings);
}

#[test]
fn capture_part_has_capture_structure_and_metadata() {
    let part = build_capture_part(&sample_symbol(), "LM321_C7950", "c7950_lm321");
    let xml = &part.xml;
    assert!(xml.contains("refdesPrefix=\"U\""));
    assert!(xml.contains("pcbFootprint=\"c7950_lm321\""));
    assert!(xml.contains("<Defn name=\"LM321 &lt;op-amp&gt; &amp; &quot;rail&quot;\"/>"));
    assert!(xml.contains("val=\"https://example.test/ds?a=1&amp;b=2\""));
    assert!(xml.contains("<Defn name=\"LCSC Part\" val=\"C7950\"/>"));
    // Pin types: input 0, output 2, power 7.
    assert!(xml.contains("name=\"+IN\" position=\"0\""));
    assert!(xml.contains("type=\"7\""));
    assert!(xml.contains("type=\"2\""));
    // Physical pin numbers map positions to package pins.
    assert!(xml.contains("<Defn number=\"3\" position=\"0\"/>"));
    assert!(xml.contains("<Defn number=\"2\" position=\"4\"/>"));
    // Body rectangle unfilled, triangle polygon filled.
    assert!(xml.contains("<Rect>\n          <Defn fillStyle=\"1\""));
    assert!(xml.contains("<Polygon>\n          <Defn fillStyle=\"0\""));
    // Order inside NormalView follows Capture's own exports.
    let order = [
        "<SymbolDisplayProp>",
        "<SymbolUserProp>",
        "<SymbolColor>",
        "<SymbolBBox>",
        "<IsPinNumbersVisible>",
        "<PartValue>",
        "<Reference>",
        "<Rect>",
        "<SymbolPinScalar>",
        "</NormalView>",
        "<PhysicalPart>",
    ];
    let positions: Vec<usize> = order.iter().map(|tag| xml.find(tag).unwrap()).collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn capture_renames_duplicate_pin_numbers() {
    let mut symbol = sample_symbol();
    symbol.pins[1].number = "3".to_string();
    let part = build_capture_part(&symbol, "X", "x");
    assert!(part.xml.contains("<Defn number=\"3_2\""));
    assert!(
        part.warnings
            .iter()
            .any(|w| w.contains("duplicate pin number 3"))
    );
}

#[test]
fn sub_parts_become_sections_of_a_heterogeneous_package() {
    let mut symbol = sample_symbol();
    let unit = KiSymbolUnit {
        pins: symbol.pins.clone(),
        rectangles: symbol.rectangles.clone(),
        ..KiSymbolUnit::default()
    };
    let mut second = unit.clone();
    for pin in &mut second.pins {
        pin.number = format!("{}{}", pin.number, 0);
    }
    symbol.units = vec![unit, second];
    symbol.pins.clear();
    symbol.rectangles.clear();
    symbol.circles.clear();
    symbol.polylines.clear();

    let part = build_capture_part(&symbol, "DUAL", "dual");
    let xml = &part.xml;
    // As Capture stores MAX628 in MiscPower.olb: one LibPart per section, named
    // after the package plus the section letter.
    assert!(xml.contains("isHomogeneous=\"0\""));
    assert_eq!(xml.matches("<LibPart>").count(), 2);
    assert!(xml.contains("<Defn CellName=\"DUALA\"/>"));
    assert!(xml.contains("<Defn CellName=\"DUALB\"/>"));
    assert_eq!(xml.matches("<PhysicalPart>").count(), 2);
    assert_eq!(xml.matches("<SymbolPinScalar>").count(), 10);
    // Each section is drawn on its own, at the origin, with its own pin numbers.
    let sections: Vec<&str> = xml.split("<LibPart>").skip(1).collect();
    for section in &sections {
        assert_eq!(section.matches("<Rect>").count(), 1);
        assert_capture_body_rules(&format!("<LibPart>{section}"));
    }
    assert!(sections[0].contains("number=\"1\""));
    assert!(sections[1].contains("number=\"10\""));
    assert!(!sections[1].contains("number=\"1\" "));
    // Pins never share a connection point.
    for section in &sections {
        let mut hot: Vec<(i64, i64)> = pin_points(section)
            .into_iter()
            .map(|(_, x, y, _, _)| (x, y))
            .collect();
        let count = hot.len();
        hot.sort();
        hot.dedup();
        assert_eq!(hot.len(), count);
    }
}

#[test]
fn single_symbols_stay_homogeneous() {
    let part = build_capture_part(&sample_symbol(), "LM321_C7950", "c7950_lm321");
    assert!(part.xml.contains("isHomogeneous=\"1\""));
    assert_eq!(part.xml.matches("<LibPart>").count(), 1);
    assert!(part.xml.contains("<Defn CellName=\"LM321_C7950\"/>"));
}

#[test]
fn library_xml_wraps_packages() {
    let part = build_capture_part(&sample_symbol(), "LM321_C7950", "c7950_lm321");
    let xml = render_library_xml("SeEx.olb", &[part.xml.clone(), part.xml]);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\""));
    assert_eq!(xml.matches("<Package>").count(), 2);
    assert_eq!(xml.matches("<DefaultFont>").count(), 24);
    assert!(xml.trim_end().ends_with("</Lib>"));
}

#[test]
fn identifiers_are_tool_safe() {
    assert_eq!(
        capture_identifier("STM32F103C8T6 (LQFP-48)"),
        "STM32F103C8T6_LQFP-48"
    );
    assert_eq!(capture_identifier("中文"), "PART");
    assert_eq!(allegro_identifier("SOP-8_L4.9", 31), "sop-8_l4_9");
    let long = footprint_name_for("C2040", "RP2040 very long package title that keeps going");
    assert!(long.len() <= 31);
    assert!(long.starts_with("c2040_"));
}

fn smd_pad(number: &str, x: f64, y: f64, w: f64, h: f64, rotation: f64) -> KiPad {
    KiPad {
        number: number.to_string(),
        pad_type: PadType::Smd,
        shape: PadShape::Rect,
        pos_x: x,
        pos_y: y,
        size_x: w,
        size_y: h,
        rotation,
        layers: vec!["F.Cu".into(), "F.Paste".into(), "F.Mask".into()],
        drill: None,
        polygon: None,
    }
}

/// SOIC-like footprint with every pad flavour the converter can emit (EasyEDA px, Y down).
pub(crate) fn sample_footprint() -> KiFootprint {
    let mut pads = vec![
        smd_pad("1", -10.0, -15.0, 6.0, 2.4, 0.0),
        smd_pad("2", -10.0, 15.0, 6.0, 2.4, 0.0),
        smd_pad("2", 10.0, 15.0, 6.0, 2.4, 90.0),
    ];
    pads.push(KiPad {
        number: "3".into(),
        pad_type: PadType::ThroughHole,
        shape: PadShape::Oval,
        pos_x: 20.0,
        pos_y: 0.0,
        size_x: 8.0,
        size_y: 5.0,
        rotation: 0.0,
        layers: vec!["*.Cu".into(), "*.Mask".into()],
        drill: Some(Drill {
            diameter: 5.0,
            width: Some(3.0),
            offset_x: 0.0,
            offset_y: 0.0,
        }),
        polygon: None,
    });
    pads.push(KiPad {
        number: "4".into(),
        pad_type: PadType::ThroughHole,
        shape: PadShape::Circle,
        pos_x: 30.0,
        pos_y: 0.0,
        size_x: 6.0,
        size_y: 6.0,
        rotation: 0.0,
        layers: vec!["*.Cu".into(), "*.Mask".into()],
        drill: Some(Drill {
            diameter: 4.0,
            width: None,
            offset_x: 0.0,
            offset_y: 0.0,
        }),
        polygon: None,
    });
    pads.push(KiPad {
        number: "EP".into(),
        pad_type: PadType::Smd,
        shape: PadShape::Custom,
        pos_x: 0.0,
        pos_y: 0.0,
        size_x: 0.01,
        size_y: 0.01,
        rotation: 0.0,
        layers: vec!["F.Cu".into(), "F.Paste".into(), "F.Mask".into()],
        drill: None,
        polygon: Some(
            "\n\t\t(primitives \n\t\t\t(gr_poly \n\t\t\t\t(pts (xy -1.00 -0.50) (xy 1.00 -0.50) (xy 1.00 0.50) (xy -1.00 0.50)\n\t\t\t\t) \n\t\t\t\t(width 0.1) \n\t\t\t)\n\t\t)\n\t"
                .into(),
        ),
    });
    // Via (unnumbered plated) and a mounting hole (NPTH).
    pads.push(KiPad {
        number: String::new(),
        pad_type: PadType::ThroughHole,
        shape: PadShape::Circle,
        pos_x: 0.0,
        pos_y: 0.0,
        size_x: 2.4,
        size_y: 2.4,
        rotation: 0.0,
        layers: vec!["*.Cu".into(), "*.Mask".into()],
        drill: Some(Drill {
            diameter: 1.2,
            width: None,
            offset_x: 0.0,
            offset_y: 0.0,
        }),
        polygon: None,
    });
    pads.push(KiPad {
        number: String::new(),
        pad_type: PadType::NpThroughHole,
        shape: PadShape::Circle,
        pos_x: -30.0,
        pos_y: 0.0,
        size_x: 12.0,
        size_y: 12.0,
        rotation: 0.0,
        layers: vec!["*.Cu".into(), "*.Mask".into()],
        drill: Some(Drill {
            diameter: 12.0,
            width: None,
            offset_x: 0.0,
            offset_y: 0.0,
        }),
        polygon: None,
    });

    let line = |x1: f64, y1: f64, x2: f64, y2: f64, layer: &str| KiLine {
        start_x: x1,
        start_y: y1,
        end_x: x2,
        end_y: y2,
        width: 0.6,
        layer: layer.into(),
    };
    KiFootprint {
        name: "test".into(),
        pads,
        tracks: Vec::new(),
        circles: vec![FpCircle {
            center_x: -14.0,
            center_y: -20.0,
            end_x: -13.0,
            end_y: -20.0,
            width: 0.2,
            layer: "F.SilkS".into(),
            fill: true,
        }],
        arcs: vec![FpArc {
            start_x: -5.0,
            start_y: -20.0,
            mid_x: 0.0,
            mid_y: -15.0,
            end_x: 5.0,
            end_y: -20.0,
            width: 0.6,
            layer: "F.SilkS".into(),
        }],
        texts: Vec::new(),
        lines: vec![
            line(-20.0, -20.0, 20.0, -20.0, "F.SilkS"),
            line(-20.0, 20.0, 20.0, 20.0, "F.Fab"),
            line(-40.0, -25.0, 40.0, -25.0, "F.CrtYd"),
            line(40.0, -25.0, 40.0, 25.0, "F.CrtYd"),
            line(40.0, 25.0, -40.0, 25.0, "F.CrtYd"),
            line(-40.0, 25.0, -40.0, -25.0, "F.CrtYd"),
            line(0.0, 0.0, 5.0, 5.0, "Cmts.User"),
        ],
        model_3d: None,
    }
}

#[test]

fn allegro_script_creates_shared_padstacks_and_pins() {
    let fp = build_allegro_footprint(&sample_footprint(), "c1_test", "U?");
    let script = &fp.script;

    // 6 x 2.4 px = 1.524 x 0.6096 mm -> one shared SMD padstack for pads 1 and both 2s.
    assert!(
        padstacks(&fp.script).contains(&"sx_sr152x61".to_string()),
        "{:?}",
        padstacks(&fp.script)
    );
    assert_eq!(script.matches("seex_padstack(\"sx_sr152x61\"").count(), 1);
    // Duplicate pad number renamed, rotation preserved.
    assert!(script.contains("seex_pin(\"sx_sr152x61\" \"2_2\" 2.5400 -3.8100 90.0000)"));
    // Y is flipped: EasyEDA y = -15 px (up on screen) -> +3.81 mm in Allegro.
    assert!(script.contains("seex_pin(\"sx_sr152x61\" \"1\" -2.5400 3.8100 0.0000)"));
    // Oblong pad with an oval slot, plated.
    assert!(script.contains("seex_slot(1.2700 0.7620 t)"));
    assert!(script.contains("'OBLONG"));
    // Round plated pad keeps the DEFAULT_INTERNAL layer with a fallback without it.
    assert!(script.contains("seex_padstack(\"sx_tc152d102\" seex_drill(1.0160 t)"));
    // Mechanical hole: non-plated, no pin number.
    assert!(script.contains("seex_padstack(\"sx_h305\" seex_drill(3.0480 nil)"));
    assert!(script.contains("seex_pin(\"sx_h305\" nil -7.6200 0.0000 0.0000)"));
    // Custom polygon pad: anchor pad plus three shapes.
    assert!(script.contains("seex_shape(\"ETCH/TOP\""));
    assert!(script.contains("seex_shape(\"PACKAGE GEOMETRY/PASTEMASK_TOP\""));
    // Courtyard becomes the place bound (80 x 50 px = 20.32 x 12.7 mm).
    assert!(script.contains(
        "seex_place_bound(\"PACKAGE GEOMETRY/PLACE_BOUND_TOP\" -10.1600 -6.3500 10.1600 6.3500)"
    ));
    assert!(script.contains("\"REF DES/SILKSCREEN_TOP\" \"U*\""));
    // Silkscreen / assembly graphics, arc, filled dot.
    assert!(script.contains("seex_line(\"PACKAGE GEOMETRY/SILKSCREEN_TOP\""));
    // The component frame is the silkscreen: no assembly outline.
    assert!(!script.contains("seex_line(\"PACKAGE GEOMETRY/ASSEMBLY_TOP\""));
    assert!(script.contains("seex_text(\"REF DES/ASSEMBLY_TOP\""));
    assert!(script.contains("seex_arc(\"PACKAGE GEOMETRY/SILKSCREEN_TOP\""));
    assert!(script.contains("seex_circle(\"PACKAGE GEOMETRY/SILKSCREEN_TOP\""));

    let joined = fp.warnings.join("\n");
    assert!(joined.contains("1 unnumbered via"), "{joined}");
    assert!(joined.contains("2_2"));
    assert!(joined.contains("custom polygon"));
    assert!(joined.contains("Cmts.User"));
}

#[test]
fn allegro_script_parentheses_balance() {
    let fp = build_allegro_footprint(&sample_footprint(), "c1_test", "R");
    let mut depth = 0i64;
    let mut in_string = false;
    let mut in_comment = false;
    let mut escaped = false;
    for ch in fp.script.chars() {
        if in_comment {
            in_comment = ch != '\n';
            continue;
        }
        if in_string {
            match (escaped, ch) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            ';' => in_comment = true,
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                assert!(depth >= 0, "unbalanced ')'");
            }
            _ => {}
        }
    }
    assert_eq!(depth, 0);
    assert!(!in_string);
}

#[test]
fn allegro_names_long_padstacks_with_hash() {
    let mut footprint = sample_footprint();
    footprint.pads = vec![KiPad {
        number: "1".into(),
        pad_type: PadType::ThroughHole,
        shape: PadShape::Oval,
        pos_x: 0.0,
        pos_y: 0.0,
        size_x: 123.4,
        size_y: 98.7,
        rotation: 0.0,
        layers: vec!["*.Cu".into()],
        drill: Some(Drill {
            diameter: 77.7,
            width: Some(55.5),
            offset_x: 0.0,
            offset_y: 0.0,
        }),
        polygon: None,
    }];
    let fp = build_allegro_footprint(&footprint, "big", "J");
    let stacks = padstacks(&fp.script);
    assert_eq!(stacks.len(), 1);
    assert!(stacks[0].len() <= 20, "{}", stacks[0]);
    assert!(stacks[0].starts_with("sx_to"));
}

#[test]
fn square_smd_pads_use_allegro_square_figure() {
    let mut footprint = sample_footprint();
    footprint.pads.truncate(1);
    footprint.pads[0].size_x = 3.15;
    footprint.pads[0].size_y = 3.15;
    let fp = build_allegro_footprint(&footprint, "c2286_led", "LED?");
    assert!(
        padstacks(&fp.script).contains(&"sx_sq80".to_string()),
        "{:?}",
        padstacks(&fp.script)
    );
    assert!(fp.script.contains("'SQUARE"));
    assert!(
        !fp.script
            .contains("seex_pad(\"TOP\" 'RECTANGLE 0.8001 0.8001)")
    );
}

#[test]
fn allegro_script_maps_step_model() {
    let fp = build_allegro_footprint_with(
        &sample_footprint(),
        "c1_test",
        "U",
        Some(StepModel {
            file: "c1_test.step",
            dir: std::path::Path::new("E:\\Libs\\3D\\Package_SO"),
            transform: StepTransform {
                rotation: [0.0, 0.0, 90.0],
                offset: [0.0, -0.127, 0.7],
            },
        }),
    );
    assert!(
        fp.script.contains(
            "  seex_step(\"c1_test.step\" \"E:/Libs/3D/Package_SO\" 0.0000 0.0000 90.0000 0.0000 -0.1270 0.7000)\n  seex_end(\"c1_test\")"
        ),
        "{}",
        fp.script
    );
    assert!(
        !build_allegro_footprint(&sample_footprint(), "c1_test", "U")
            .script
            .contains("  seex_step(")
    );
}

#[test]
fn padstacks_carry_every_required_layer_and_are_checked() {
    let footprint = sample_footprint();
    let fp = build_allegro_footprint(&footprint, "test_tht", "J");
    // Plated holes: regular, anti-pad and thermal relief on outer and internal layers.
    for layer in ["TOP", "DEFAULT_INTERNAL", "BOTTOM"] {
        for kind in ["ANTI", "THERMAL"] {
            assert!(
                fp.script
                    .contains(&format!("seex_padt(\"{layer}\" '{kind} ")),
                "{layer} {kind} missing:\n{}",
                fp.script
            );
        }
    }
    assert!(fp.script.contains("seex_pad(\"SOLDERMASK_BOTTOM\" "));
    // Every padstack is read back and checked.
    for stack in &padstacks(&fp.script) {
        assert!(
            fp.script
                .contains(&format!("seex_check_padstack(\"{stack}\" ")),
            "{stack} not checked"
        );
    }
    assert!(fp.script.contains("list(\"ETCH/TOP\" \"THERMAL\")"));

    let mut smd = sample_footprint();
    smd.pads.truncate(1);
    smd.pads[0].pad_type = PadType::Smd;
    smd.pads[0].drill = None;
    smd.pads[0].layers = vec!["F.Cu".into(), "F.Paste".into(), "F.Mask".into()];
    let fp = build_allegro_footprint(&smd, "test_smd", "R");
    assert!(fp.script.contains("seex_pad(\"PASTEMASK_TOP\" "));
    assert!(fp.script.contains(
        "list(list(\"ETCH/TOP\" \"REGULAR\") list(\"PIN/SOLDERMASK_TOP\" \"REGULAR\") list(\"PIN/PASTEMASK_TOP\" \"REGULAR\")) nil)"
    ));
}

fn pin_points(xml: &str) -> Vec<(String, i64, i64, i64, i64)> {
    xml.match_indices("<SymbolPinScalar>")
        .map(|(index, _)| {
            (
                attr(xml, index, "name"),
                attr(xml, index, "hotptX").parse().unwrap(),
                attr(xml, index, "hotptY").parse().unwrap(),
                attr(xml, index, "startX").parse().unwrap(),
                attr(xml, index, "startY").parse().unwrap(),
            )
        })
        .collect()
}

#[test]
fn off_grid_symbols_are_snapped_to_the_grid() {
    // EasyEDA symbol drawn on a 0.05" grid: the body and the right pins sit half a
    // grid off, and a free-standing pin (no body) is off in both directions.
    let mut symbol = sample_symbol();
    symbol.rectangles[0].x2 = 45.0;
    symbol.pins[2].pos_x = 55.0; // OUT, on the moved right edge
    symbol.pins[2].pos_y = 5.0;
    symbol
        .pins
        .push(pin("6", "NC", 72.5, -15.0, 0, PinType::NoConnect));
    let part = build_capture_part(&symbol, "T_C1", "t");
    for (name, hx, hy, sx, sy) in pin_points(&part.xml) {
        assert_eq!(hx % CAPTURE_GRID, 0, "{name} hotptX {hx}");
        assert_eq!(hy % CAPTURE_GRID, 0, "{name} hotptY {hy}");
        assert!(
            (hx - sx).abs() + (hy - sy).abs() >= 5,
            "{name} keeps a visible pin"
        );
    }
    // The body grew to the grid.
    let rect = part.xml.find("<Rect>").unwrap();
    for key in ["x1", "x2", "y1", "y2"] {
        let v: i64 = attr(&part.xml, rect, key).parse().unwrap();
        assert_eq!(v % CAPTURE_GRID, 0, "body {key} {v}");
    }
}

/// Capture's own parts (all of Discrete.olb) keep the body outline on the 0.1"
/// grid with its top-left corner at the part origin, start every pin on it and draw
/// everything inside it; the part editor relies on that to move graphics and pins.
fn assert_capture_body_rules(xml: &str) {
    let problems = capture_body_problems(xml);
    assert!(problems.is_empty(), "{problems:?}");
}

fn capture_body_problems(xml: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(bbox) = xml.find("<SymbolBBox>") else {
        return vec!["no SymbolBBox".to_string()];
    };
    let get = |key: &str| attr(xml, bbox, key).parse::<i64>().unwrap();
    let (x1, x2, y1, y2) = (get("x1"), get("x2"), get("y1"), get("y2"));
    if [x1, x2, y1, y2].iter().any(|v| v % CAPTURE_GRID != 0) || x1 >= x2 || y1 >= y2 {
        problems.push(format!("outline {x1} {x2} {y1} {y2} off grid"));
    }
    if (x1, y1) != (0, 0) {
        problems.push(format!(
            "outline starts at ({x1}, {y1}), not at the part origin"
        ));
    }
    for (name, hx, hy, sx, sy) in pin_points(xml) {
        if (hx - sx).abs() + (hy - sy).abs() < CAPTURE_GRID {
            problems.push(format!(
                "pin {name} is shorter than 0.1\" ({hx}, {hy}) to ({sx}, {sy})"
            ));
        }
        let on_edge = ((sx == x1 || sx == x2) && (y1..=y2).contains(&sy))
            || ((sy == y1 || sy == y2) && (x1..=x2).contains(&sx));
        if !on_edge {
            problems.push(format!(
                "pin {name} starts at ({sx}, {sy}), off the outline {x1} {x2} {y1} {y2}"
            ));
        }
    }
    let mut at = 0;
    while let Some(found) = xml[at..].find("<Line>") {
        let index = at + found;
        for (kx, ky) in [("x1", "y1"), ("x2", "y2")] {
            let x: i64 = attr(xml, index, kx).parse().unwrap();
            let y: i64 = attr(xml, index, ky).parse().unwrap();
            if !((x1..=x2).contains(&x) && (y1..=y2).contains(&y)) {
                problems.push(format!(
                    "line point ({x}, {y}) outside the outline {x1} {x2} {y1} {y2}"
                ));
            }
        }
        at = index + 6;
    }
    problems
}

#[test]
fn filled_shapes_are_closed_polygons() {
    // EasyEDA's MOSFET arrow: three points, filled, last point not repeated.
    let mut symbol = sample_symbol();
    symbol.polylines = vec![KiPolyline {
        points: vec![(0.0, 0.0), (-6.0, 2.0), (-6.0, -2.0)],
        stroke_width: 1.0,
        fill: true,
    }];
    let part = build_capture_part(&symbol, "T_Q2", "t");
    let polygon = part
        .xml
        .find("<Polygon>")
        .expect("filled triangle is a polygon");
    assert_eq!(attr(&part.xml, polygon, "fillStyle"), "0", "solid fill");
    let end = polygon + part.xml[polygon..].find("</Polygon>").unwrap();
    assert_eq!(
        part.xml[polygon..end].matches("<PolygonPoint>").count(),
        4,
        "closed: 3 corners + first again"
    );
}

#[test]
fn drawn_symbols_get_a_capture_style_outline() {
    // Schottky diode as EasyEDA draws it: bar and triangle between 0.05" grid lines,
    // pins ending on the drawing rather than on a body box.
    let mut symbol = sample_symbol();
    symbol.rectangles.clear();
    symbol.circles.clear();
    symbol.pins = vec![
        pin("1", "K", -20.0, 0.0, 180, PinType::Passive),
        pin("2", "A", 20.0, 0.0, 0, PinType::Passive),
    ];
    symbol.pins[0].length = 15.0;
    symbol.pins[1].length = 15.0;
    symbol.polylines = vec![
        KiPolyline {
            points: vec![(-5.0, 6.0), (-5.0, -6.0)],
            stroke_width: 1.0,
            fill: false,
        },
        KiPolyline {
            points: vec![(5.0, 6.0), (-5.0, 0.0), (5.0, -6.0), (5.0, 6.0)],
            stroke_width: 1.0,
            fill: true,
        },
    ];
    let part = build_capture_part(&symbol, "T_D1", "t");
    assert_capture_body_rules(&part.xml);
    // The pins still reach the drawing: lead lines close the gap to the bar/triangle.
    assert!(part.xml.matches("<Line>").count() >= 3, "{}", part.xml);

    // MOSFET gate: a 0.05" pin whose tip falls on the outline once it is on the grid
    // must not become a zero-length pin.
    let mut mosfet = symbol.clone();
    mosfet.pins[0].pos_x = -10.0;
    mosfet.pins[0].length = 5.0;
    let part = build_capture_part(&mosfet, "T_Q1", "t");
    assert_capture_body_rules(&part.xml);

    // Box symbols already follow the rules and keep their pins.
    let part = build_capture_part(&sample_symbol(), "LM321_C7950", "c7950_lm321");
    assert_capture_body_rules(&part.xml);
}

#[test]
fn bodies_keep_their_shape_on_edges_without_pins() {
    // Varistor: a flat 0.2" x 0.08" box with pins left and right and a diagonal
    // that crosses it. Only the pin edges may move to the grid.
    let mut symbol = sample_symbol();
    symbol.pins = vec![
        pin("1", "1", -20.0, 0.0, 180, PinType::Passive),
        pin("2", "2", 30.0, 0.0, 0, PinType::Passive),
    ];
    symbol.rectangles[0] = KiRectangle {
        x1: -10.0,
        y1: 4.0,
        x2: 20.0,
        y2: -4.0,
        stroke_width: 1.0,
        fill: false,
    };
    symbol.circles.clear();
    symbol.polylines = vec![KiPolyline {
        points: vec![(-10.0, -8.0), (-5.0, -8.0), (15.0, 8.0)],
        stroke_width: 1.0,
        fill: false,
    }];
    let part = build_capture_part(&symbol, "T_RV1", "t");
    let rect = part.xml.find("<Rect>").unwrap();
    let get = |key: &str| attr(&part.xml, rect, key).parse::<i64>().unwrap();
    assert_eq!(
        (get("y2") - get("y1")).abs(),
        8,
        "the box stays 0.08\" tall"
    );
    assert_eq!((get("x2") - get("x1")).abs(), 30);
    for (name, hx, hy, _, _) in pin_points(&part.xml) {
        assert_eq!(hx % CAPTURE_GRID, 0, "{name}");
        assert_eq!(hy % CAPTURE_GRID, 0, "{name}");
    }
}

#[test]

fn half_grid_symbols_move_as_a_whole() {
    // A two-pin header drawn on the 0.05" grid: both pins sit half a grid off.
    // The whole drawing moves, so the pins stay 0.1" apart and on the body.
    let mut symbol = sample_symbol();
    symbol.pins = vec![
        pin("1", "1", -20.0, 5.0, 180, PinType::Passive),
        pin("2", "2", -20.0, -5.0, 180, PinType::Passive),
    ];
    symbol.rectangles[0] = KiRectangle {
        x1: -10.0,
        y1: 25.0,
        x2: 20.0,
        y2: -15.0,
        stroke_width: 1.0,
        fill: false,
    };
    symbol.polylines.clear();
    let part = build_capture_part(&symbol, "T_J1", "t");
    let pins = pin_points(&part.xml);
    let ys: Vec<i64> = pins.iter().map(|p| p.2).collect();
    assert_eq!((ys[0] - ys[1]).abs(), 10, "{pins:?}");
    assert!(
        part.warnings.iter().all(|w| !w.contains("jog")),
        "{:?}",
        part.warnings
    );
    let rect = part.xml.find("<Rect>").unwrap();
    let get = |key: &str| attr(&part.xml, rect, key).parse::<i64>().unwrap();
    assert_eq!((get("y2") - get("y1")).abs(), 40);
}

#[test]
fn stacked_pins_stay_on_the_body() {
    // Three shield pins EasyEDA put on one spot under the body: they are spread
    // along the bottom edge and the body widens to keep them attached.
    let mut symbol = sample_symbol();
    symbol.pins = vec![
        pin("1", "VBUS", -10.0, 10.0, 180, PinType::Passive),
        pin("6", "SH", 10.0, -10.0, 270, PinType::Passive),
        pin("7", "SH", 10.0, -10.0, 270, PinType::Passive),
        pin("8", "SH", 10.0, -10.0, 270, PinType::Passive),
    ];
    symbol.rectangles[0] = KiRectangle {
        x1: 0.0,
        y1: 20.0,
        x2: 20.0,
        y2: 0.0,
        stroke_width: 1.0,
        fill: false,
    };
    symbol.polylines.clear();
    symbol.circles.clear();
    let part = build_capture_part(&symbol, "T_J2", "t");
    let rect = part.xml.find("<Rect>").unwrap();
    let get = |key: &str| attr(&part.xml, rect, key).parse::<i64>().unwrap();
    let (x1, x2) = (get("x1").min(get("x2")), get("x1").max(get("x2")));
    let bottom = get("y1").max(get("y2"));
    let mut xs = Vec::new();
    for (name, hx, _, sx, sy) in pin_points(&part.xml) {
        if name == "SH" {
            assert_eq!(sy, bottom, "attached to the bottom edge");
            assert!(sx >= x1 && sx <= x2, "{sx} within {x1}..{x2}");
            xs.push(hx);
        }
    }
    xs.sort();
    xs.dedup();
    assert_eq!(xs.len(), 3, "spread apart");
}

#[test]
fn assembly_refdes_follows_the_fabrication_conventions() {
    use crate::cadence::allegro::assembly_refdes_layout as layout;
    // Always centred on the component: baseline half a text height below.
    // 0402 (1.0 x 0.5 mm): the smallest size.
    let (h, rot, x, y) = layout(Some((-0.5, -0.25, 0.5, 0.25)), "R*");
    assert_eq!((h, rot), (0.5, 0.0));
    assert!(x.abs() < 1e-9 && (y + 0.25).abs() < 1e-9);
    // 0603 (1.6 x 0.8 mm): "R*" at 0.59 mm; "LED*" at 0.5 mm, still centred.
    let (h, _, _, y) = layout(Some((-0.8, -0.4, 0.8, 0.4)), "R*");
    assert!((h - 1.6 / 2.7).abs() < 1e-9);
    assert!((y + h / 2.0).abs() < 1e-9);
    let (h, _, x, y) = layout(Some((1.0, 2.0, 2.6, 2.8)), "LED*");
    assert_eq!(h, 0.5);
    assert!((x - 1.8).abs() < 1e-9 && (y - 2.15).abs() < 1e-9);
    // SOIC-8 standing (3.9 x 4.9 mm): along the long axis, full size.
    let (h, rot, x, _) = layout(Some((-1.95, -2.45, 1.95, 2.45)), "U*");
    assert_eq!((h, rot), (1.0, 90.0));
    assert!((x - 0.5).abs() < 1e-9);
    // SOT-23 lying (2.9 x 1.3 mm): "Q*" at 1.0 mm.
    let (h, rot, ..) = layout(Some((-1.45, -0.65, 1.45, 0.65)), "Q*");
    assert!((h - 1.0).abs() < 1e-9 && rot == 0.0);
}

#[test]
fn silkscreen_refdes_fits_the_part() {
    use crate::cadence::allegro::{silk_refdes_height as height, silk_stroke};
    // 0402 (place bound about 1.4 mm wide): the smallest size.
    assert_eq!(height(Some((-0.7, -0.5, 0.7, 0.5)), "R*"), 0.6);
    // 0603 LED (2.7 mm): "LED*" spans the part.
    assert!((height(Some((-1.35, -0.75, 1.35, 0.75)), "LED*") - 2.7 / 3.6).abs() < 1e-9);
    // SOT-23 and larger: the full 1.0 mm.
    assert_eq!(height(Some((-1.7, -1.6, 1.7, 1.6)), "Q*"), 1.0);
    assert_eq!(height(None, "U*"), 1.0);
    assert!((silk_stroke(1.0) - 0.15).abs() < 1e-9);
    assert!((silk_stroke(0.6) - 0.10).abs() < 1e-9);
}

#[test]
fn library_script_builds_every_footprint_in_its_folder() {
    let script = library_script(
        "Parts",
        std::path::Path::new("E:\\Libs\\Parts_allegro"),
        &["resc1005x40n".to_string(), "sot95p280x145-5n".to_string()],
    );
    assert!(script.contains("skill load(\"E:/Libs/Parts_allegro/Parts.il\")"));
    assert!(script.contains(
        "seex_build_library(\"E:/Libs/Parts_allegro\" list(\"resc1005x40n\" \"sot95p280x145-5n\"))"
    ));
    assert!(script.contains("axlOpenDesign(?design design ?mode \"wf\")"));
    let code: String = script
        .lines()
        .filter(|line| !line.trim_start().starts_with(';'))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(code.matches('(').count(), code.matches(')').count());
}
