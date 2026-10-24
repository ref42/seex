//! OrCAD Capture symbol export.
//!
//! Capture's native `.olb` format is an undocumented binary container, but Capture
//! (16.6 and later, including OrCAD X) turns a "Library XML" file into an `.olb`
//! (`File > Import > Library XML`). This module writes that XML.
//!
//! The element layout, attribute names and enumerations below mirror files exported
//! by Capture 17.2 itself (`File > Export > Library XML`):
//!
//! * coordinates are integer Capture units of 0.01 inch with Y pointing down — one
//!   EasyEDA pixel (10 mil) is exactly one Capture unit;
//! * pins are `SymbolPinScalar` elements whose `hotptX/Y` is the wire connection point
//!   and `startX/Y` the end touching the body; pin numbers live in `PhysicalPart`;
//! * pin types: 0 input, 1 bidirectional, 2 output, 3 open collector, 4 passive,
//!   5 tri-state, 6 open emitter, 7 power;
//! * fill styles: 0 solid, 1 none;
//! * a symbol with EasyEDA sub-parts becomes a heterogeneous package
//!   (`isHomogeneous="0"`) with one `LibPart` per section, named after the package
//!   plus the section letter (`RK3576_C42388007A`, ...B), as Capture stores
//!   MAX628 in its own `MiscPower.olb`.
//!
//! Emitted elements: `Rect`, `Line`, `Polygon`, `Ellipse`, `Arc` and `CommentText` (all
//! in Capture's `olb.xsd`); arcs smaller than a Capture unit become line segments.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write as _;

use crate::kicad::symbol::{
    KiArc, KiCircle, KiPin, KiPolyline, KiRectangle, KiSymbol, KiSymbolUnit, KiText, PinStyle,
    PinType,
};

/// Capture's standard pin grid (0.1 inch).
pub const CAPTURE_GRID: i64 = 10;
/// Default Capture colour index used by its own exports for symbol graphics.
const SYMBOL_COLOR: &str = "48";

/// One Capture part, ready to be written into a Library XML file.
#[derive(Debug, Clone)]
pub struct CapturePart {
    /// Package / cell name inside the `.olb`.
    pub name: String,
    /// `<Package>` XML fragment for this part.
    pub xml: String,
    /// Non-fatal conversion notes (off-grid pins, renamed duplicate pins, ...).
    pub warnings: Vec<String>,
}

/// Build the Capture part for `symbol`, linked to the Allegro footprint `footprint`.
/// Whether the part shows pin names / pin numbers (Capture's part-level switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinLabels {
    pub names: bool,
    pub numbers: bool,
}

impl Default for PinLabels {
    fn default() -> Self {
        Self {
            names: true,
            numbers: true,
        }
    }
}

pub fn build_capture_part_with(
    symbol: &KiSymbol,
    part_name: &str,
    footprint: &str,
    labels: PinLabels,
) -> CapturePart {
    let mut warnings = Vec::new();
    let sections = sections(symbol);
    // One section is an ordinary part; EasyEDA sub-parts become the sections of a
    // heterogeneous Capture package (U1A, U1B, ...), each drawn on its own, the
    // way Capture's own libraries store them: one `LibPart` per section, named
    // after the package plus the section letter, each with its own pin numbers.
    let heterogeneous = sections.len() > 1;
    let mut jogs = 0;
    let mut lib_parts = String::new();
    for (index, mut geometry) in sections.into_iter().enumerate() {
        jogs += align_to_grid(&mut geometry);
        let cell = if heterogeneous {
            format!("{part_name}{}", section_letter(index))
        } else {
            part_name.to_string()
        };
        lib_parts.push_str(&lib_part_xml(
            symbol,
            &geometry,
            &cell,
            part_name,
            labels,
            &mut warnings,
        ));
    }
    if jogs > 0 {
        warnings.push(format!(
            "{jogs} pin(s) sat between grid lines; moved onto the 0.1\" grid with a short jog to the body"
        ));
    }

    let mut xml = String::new();
    let _ = writeln!(xml, "  <Package>");
    let _ = writeln!(
        xml,
        "    <Defn alphabeticNumbering=\"1\" isHomogeneous=\"{}\" name=\"{}\" pcbFootprint=\"{}\" pcbLib=\"\" refdesPrefix=\"{}\" timestamp=\"0\" timezone=\"0\"/>",
        if heterogeneous { 0 } else { 1 },
        xml_escape(part_name),
        xml_escape(footprint),
        xml_escape(&refdes_prefix(&symbol.reference))
    );
    xml.push_str(&lib_parts);
    let _ = writeln!(xml, "  </Package>");

    CapturePart {
        name: part_name.to_string(),
        xml,
        warnings,
    }
}

/// Section letters as Capture numbers them: A..Z, then AA, AB, ...
fn section_letter(index: usize) -> String {
    let mut index = index;
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + (index % 26) as u8);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_default()
}

/// One `<LibPart>`: the drawing and pin numbers of one section.
fn lib_part_xml(
    symbol: &KiSymbol,
    geometry: &KiSymbolUnit,
    cell: &str,
    part_name: &str,
    labels: PinLabels,
    warnings: &mut Vec<String>,
) -> String {
    // Capture's part editor takes the body outline's top-left corner as the part
    // origin (every part Cadence ships has its outline at 0,0): with the origin
    // anywhere else, a pin moved in the editor is redrawn from the origin. Lay the
    // part out once to find the outline, then again shifted onto the origin.
    let mut layout = CaptureLayout::new(geometry);
    let mut scratch = Vec::new();
    let (_, _, outline) = draw_unit(geometry, &layout, &mut scratch);
    layout.dx -= outline.x1 as f64;
    layout.dy -= outline.y1 as f64;
    let (graphics, pins, bbox) = draw_unit(geometry, &layout, warnings);

    let mut pin_xml = String::new();
    let mut physical = String::new();
    for (position, pin) in pins.iter().enumerate() {
        write_pin(&mut pin_xml, pin, position);
        let _ = writeln!(
            physical,
            "        <PinNumber>\n          <Defn number=\"{}\" position=\"{}\"/>\n        </PinNumber>",
            xml_escape(&pin.number),
            position
        );
    }

    let reference = refdes_prefix(&symbol.reference);
    let value = if symbol.value.trim().is_empty() {
        part_name.to_string()
    } else {
        symbol.value.clone()
    };

    let mut xml = String::new();
    let _ = writeln!(xml, "    <LibPart>");
    let _ = writeln!(xml, "      <Defn CellName=\"{}\"/>", xml_escape(cell));
    let _ = writeln!(xml, "      <NormalView>");
    let _ = writeln!(xml, "        <Defn suffix=\".Normal\"/>");
    // Labels go outside the pins so top/bottom pin names stay readable.
    let mut extent = bbox;
    for pin in &pins {
        extent.add(pin.hot.0, pin.hot.1);
    }
    write_display_prop(
        &mut xml,
        "Part Reference",
        bbox.x1,
        extent.y1 - CAPTURE_GRID,
    );
    write_display_prop(&mut xml, "Value", bbox.x1, extent.y2 + CAPTURE_GRID);
    for (name, value) in user_properties(symbol) {
        let _ = writeln!(
            xml,
            "        <SymbolUserProp>\n          <Defn name=\"{}\" val=\"{}\"/>\n        </SymbolUserProp>",
            xml_escape(name),
            xml_escape(&value)
        );
    }
    let _ = writeln!(
        xml,
        "        <SymbolColor>\n          <Defn val=\"{SYMBOL_COLOR}\"/>\n        </SymbolColor>"
    );
    let _ = writeln!(
        xml,
        "        <SymbolBBox>\n          <Defn x1=\"{}\" x2=\"{}\" y1=\"{}\" y2=\"{}\"/>\n        </SymbolBBox>",
        bbox.x1, bbox.x2, bbox.y1, bbox.y2
    );
    write_flag(&mut xml, 8, "IsPinNumbersVisible", labels.numbers);
    write_flag(&mut xml, 8, "IsPinNamesRotated", true);
    write_flag(&mut xml, 8, "IsPinNamesVisible", labels.names);
    let _ = writeln!(
        xml,
        "        <ContentsLibName>\n          <Defn name=\"\"/>\n        </ContentsLibName>"
    );
    let _ = writeln!(
        xml,
        "        <ContentsViewName>\n          <Defn name=\"\"/>\n        </ContentsViewName>"
    );
    let _ = writeln!(
        xml,
        "        <ContentsViewType>\n          <Defn type=\"0\"/>\n        </ContentsViewType>"
    );
    let _ = writeln!(
        xml,
        "        <PartValue>\n          <Defn name=\"{}\"/>\n        </PartValue>",
        xml_escape(&value)
    );
    let _ = writeln!(
        xml,
        "        <Reference>\n          <Defn name=\"{}\"/>\n        </Reference>",
        xml_escape(&reference)
    );
    xml.push_str(&graphics);
    xml.push_str(&pin_xml);
    let _ = writeln!(xml, "      </NormalView>");
    let _ = writeln!(xml, "      <PhysicalPart>\n        <Defn/>");
    xml.push_str(&physical);
    let _ = writeln!(xml, "      </PhysicalPart>");
    let _ = writeln!(xml, "    </LibPart>");
    xml
}

/// Wrap `<Package>` fragments into a complete Capture Library XML document.
pub fn render_library_xml(olb_name: &str, packages: &[String]) -> String {
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\" ?>\n");
    xml.push_str("<Lib xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\n");
    let _ = writeln!(xml, "  <Defn name=\"{}\"/>\n", xml_escape(olb_name));
    xml.push_str(&default_values_xml());
    for package in packages {
        xml.push('\n');
        xml.push_str(package);
    }
    xml.push_str("\n</Lib>\n");
    xml
}

/// Every `<Package>` of a Capture Library XML document SeEx wrote, as (part name,
/// the block as it stands in the file), in document order. An append keeps these
/// and writes them back with the new parts.
pub fn package_blocks(xml: &str) -> Vec<(String, String)> {
    const OPEN: &str = "<Package>";
    const CLOSE: &str = "</Package>";
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        let Some(len) = rest[start..].find(CLOSE) else {
            break;
        };
        let block = &rest[start..start + len + CLOSE.len()];
        rest = &rest[start + len + CLOSE.len()..];
        let name = block
            .find(" name=\"")
            .map(|at| &block[at + 7..])
            .and_then(|value| value.find('"').map(|end| &value[..end]));
        if let Some(name) = name {
            blocks.push((xml_unescape(name), format!("  {block}\n")));
        }
    }
    blocks
}

fn xml_unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Capture-safe ASCII identifier: letters, digits, `_` and `-` only.
pub fn capture_identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = false;
    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '-' {
            ch
        } else {
            '_'
        };
        if mapped == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        out.push(mapped);
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "PART".to_string()
    } else {
        trimmed
    }
}

fn refdes_prefix(reference: &str) -> String {
    let prefix: String = reference
        .trim()
        .trim_end_matches('?')
        .chars()
        .filter(|ch| ch.is_ascii_alphabetic())
        .collect();
    if prefix.is_empty() {
        "U".to_string()
    } else {
        prefix
    }
}

fn user_properties(symbol: &KiSymbol) -> Vec<(&'static str, String)> {
    let mut props = Vec::new();
    let mut push = |name: &'static str, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            props.push((name, value.to_string()));
        }
    };
    push("Manufacturer", &symbol.manufacturer);
    push("Description", &symbol.description);
    push("Datasheet", &symbol.datasheet);
    push("LCSC Part", &symbol.lcsc_id);
    push("JLC Part", &symbol.jlc_id);
    props
}

/// The sections of the Capture part, each in its own coordinate space (EasyEDA
/// px, Y up): the whole symbol, or one per EasyEDA sub-part. Graphics and pins
/// the symbol has outside its sub-parts belong to every section, as KiCad's
/// unit 0 does.
fn sections(symbol: &KiSymbol) -> Vec<KiSymbolUnit> {
    let common = KiSymbolUnit {
        pins: symbol.pins.clone(),
        rectangles: symbol.rectangles.clone(),
        circles: symbol.circles.clone(),
        arcs: symbol.arcs.clone(),
        polylines: symbol.polylines.clone(),
        texts: symbol.texts.clone(),
    };
    let units: Vec<&KiSymbolUnit> = symbol
        .units
        .iter()
        .filter(|unit| unit_has_content(unit))
        .collect();
    if units.is_empty() {
        return vec![common];
    }
    units
        .into_iter()
        .map(|unit| {
            let mut section = unit.clone();
            append_translated(&mut section, &common, 0.0);
            section
        })
        .collect()
}

/// Direction from a pin's connection point towards the body (EasyEDA px, Y up).
fn pin_direction(pin: &KiPin) -> (f64, f64) {
    let angle = (((180 + pin.rotation) % 360 + 360) % 360) as f64;
    let (sin, cos) = angle.to_radians().sin_cos();
    (cos.round(), sin.round())
}

/// True when `point` lies on the outline of the box `b` (x0, y0, x1, y1).
fn on_box_edge(point: (f64, f64), b: [f64; 4]) -> bool {
    const TOL: f64 = 0.5;
    let (x, y) = point;
    let within_x = x >= b[0] - TOL && x <= b[2] + TOL;
    let within_y = y >= b[1] - TOL && y <= b[3] + TOL;
    ((x - b[0]).abs() < TOL || (x - b[2]).abs() < TOL) && within_y
        || ((y - b[1]).abs() < TOL || (y - b[3]).abs() < TOL) && within_x
}

/// The sub-grid offset most pins share, as (x, y) in (-grid/2, grid/2].
fn common_offset(geometry: &KiSymbolUnit, grid: f64) -> (f64, f64) {
    let offset = |v: f64| {
        let r = v - (v / grid).floor() * grid; // [0, grid)
        let r = if r > grid / 2.0 + 1e-6 { r - grid } else { r };
        (r * 100.0).round() / 100.0
    };
    let mut counts: HashMap<(i64, i64), usize> = HashMap::new();
    for pin in &geometry.pins {
        let key = (
            (offset(pin.pos_x) * 100.0) as i64,
            (offset(pin.pos_y) * 100.0) as i64,
        );
        *counts.entry(key).or_default() += 1;
    }
    let best = counts
        .into_iter()
        .max_by_key(|&((x, y), n)| (n, -(x.abs() + y.abs())));
    best.map(|((x, y), _)| (x as f64 / 100.0, y as f64 / 100.0))
        .unwrap_or((0.0, 0.0))
}

fn translate_unit(unit: &mut KiSymbolUnit, dx: f64, dy: f64) {
    for pin in &mut unit.pins {
        pin.pos_x += dx;
        pin.pos_y += dy;
    }
    for rect in &mut unit.rectangles {
        rect.x1 += dx;
        rect.x2 += dx;
        rect.y1 += dy;
        rect.y2 += dy;
    }
    for circle in &mut unit.circles {
        circle.cx += dx;
        circle.cy += dy;
    }
    for arc in &mut unit.arcs {
        arc.start_x += dx;
        arc.mid_x += dx;
        arc.end_x += dx;
        arc.start_y += dy;
        arc.mid_y += dy;
        arc.end_y += dy;
    }
    for polyline in &mut unit.polylines {
        for point in &mut polyline.points {
            point.0 += dx;
            point.1 += dy;
        }
    }
    for text in &mut unit.texts {
        text.x += dx;
        text.y += dy;
    }
}

/// Puts every pin's connection point on Capture's 0.1" grid, where wires snap.
///
/// The edges of body rectangles that pins attach to move outward to the grid (other
/// edges keep their drawn position) and the pins slide along the edge to the nearest
/// grid point, keeping a whole-grid length. A pin
/// drawn without a body rectangle is lengthened to the next grid line; when it sits
/// between grid lines it moves onto one and a short jog joins it to the drawing.
/// Returns the number of jogs added.
fn align_to_grid(geometry: &mut KiSymbolUnit) -> usize {
    let grid = CAPTURE_GRID as f64;
    // Most EasyEDA symbols that are off the grid are off as a whole (drawn on a
    // 0.05" grid): move the whole drawing so the pins land on the grid, keeping
    // every pin where it meets the body and every line where it was.
    let (shift_x, shift_y) = common_offset(geometry, grid);
    if shift_x != 0.0 || shift_y != 0.0 {
        translate_unit(geometry, -shift_x, -shift_y);
    }
    let down = |v: f64| ((v / grid) + 1e-6).floor() * grid;
    let up = |v: f64| ((v / grid) - 1e-6).ceil() * grid;
    let starts: Vec<(f64, f64)> = geometry
        .pins
        .iter()
        .map(|pin| {
            let (dx, dy) = pin_direction(pin);
            let len = pin.length.abs();
            (pin.pos_x + len * dx, pin.pos_y + len * dy)
        })
        .collect();

    // Bodies: rectangles with pins on their edges, before and after snapping.
    // Rectangle index, drawn box, snapped box, and which edges carry pins (x1, y1,
    // x2, y2 order).
    let mut bodies: Vec<(usize, [f64; 4], [f64; 4], [bool; 4])> = Vec::new();
    for (index, rect) in geometry.rectangles.iter_mut().enumerate() {
        let old = [
            rect.x1.min(rect.x2),
            rect.y1.min(rect.y2),
            rect.x1.max(rect.x2),
            rect.y1.max(rect.y2),
        ];
        if !starts.iter().any(|&s| on_box_edge(s, old)) {
            continue;
        }
        // Only edges that carry pins move to the grid; the others keep the drawn
        // shape (a varistor's flat box stays flat), stretched just enough to
        // reach every pin row after the pins slide onto the grid.
        const TOL: f64 = 0.5;
        let mut new = old;
        let mut pinned = [false; 4];
        let (mut row_lo, mut row_hi) = (f64::MAX, f64::MIN);
        let (mut column_lo, mut column_hi) = (f64::MAX, f64::MIN);
        for (pin, &start) in geometry.pins.iter().zip(&starts) {
            if !on_box_edge(start, old) {
                continue;
            }
            let (dx, dy) = pin_direction(pin);
            if dx != 0.0 && dy == 0.0 {
                if (start.0 - old[0]).abs() < TOL {
                    new[0] = down(old[0]);
                    pinned[0] = true;
                } else if (start.0 - old[2]).abs() < TOL {
                    new[2] = up(old[2]);
                    pinned[2] = true;
                }
                let row = snap(start.1, grid);
                row_lo = row_lo.min(row);
                row_hi = row_hi.max(row);
            } else if dy != 0.0 && dx == 0.0 {
                if (start.1 - old[1]).abs() < TOL {
                    new[1] = down(old[1]);
                    pinned[1] = true;
                } else if (start.1 - old[3]).abs() < TOL {
                    new[3] = up(old[3]);
                    pinned[3] = true;
                }
                let column = snap(start.0, grid);
                column_lo = column_lo.min(column);
                column_hi = column_hi.max(column);
            }
        }
        new[0] = new[0].min(column_lo);
        new[2] = new[2].max(column_hi);
        new[1] = new[1].min(row_lo);
        new[3] = new[3].max(row_hi);
        rect.x1 = new[0];
        rect.y1 = new[1];
        rect.x2 = new[2];
        rect.y2 = new[3];
        bodies.push((index, old, new, pinned));
    }

    let mut used: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
    let mut stretched: Vec<(usize, (f64, f64))> = Vec::new();
    let mut jogs = Vec::new();
    for (pin, &start) in geometry.pins.iter_mut().zip(&starts) {
        let (dx, dy) = pin_direction(pin);
        if (dx == 0.0) == (dy == 0.0) {
            continue;
        }
        let horizontal = dx != 0.0;
        let body = bodies
            .iter()
            .find(|(_, old, _, _)| on_box_edge(start, *old));
        let (mut hot, mut new_start) = match body {
            Some((_, _, b, _)) => {
                let s = if horizontal {
                    (if dx > 0.0 { b[0] } else { b[2] }, snap(start.1, grid))
                } else {
                    (snap(start.0, grid), if dy > 0.0 { b[1] } else { b[3] })
                };
                let len = (pin.length.abs() / grid).round().max(1.0) * grid;
                ((s.0 - dx * len, s.1 - dy * len), s)
            }
            None => {
                let (hx, hy) = (pin.pos_x, pin.pos_y);
                // Outward is against the direction towards the body.
                let hot = if horizontal {
                    (if dx > 0.0 { down(hx) } else { up(hx) }, snap(hy, grid))
                } else {
                    (snap(hx, grid), if dy > 0.0 { down(hy) } else { up(hy) })
                };
                let s = if horizontal {
                    (start.0, hot.1)
                } else {
                    (hot.0, start.1)
                };
                let mut hot = hot;
                // Keep at least half a grid of visible pin.
                if ((hot.0 - s.0).abs() + (hot.1 - s.1).abs()) < grid / 2.0 {
                    hot = (hot.0 - dx * grid, hot.1 - dy * grid);
                }
                (hot, s)
            }
        };
        let key = |p: (f64, f64)| (p.0.round() as i64, p.1.round() as i64);
        let along = |p: (f64, f64), d: f64| {
            if horizontal {
                (p.0, p.1 + d)
            } else {
                (p.0 + d, p.1)
            }
        };
        let mut grow_body = false;
        if used.contains(&key(hot)) {
            match body {
                // Pins EasyEDA stacked on one spot (shield, GND) spread along their
                // edge: inside the edge first, then past an end whose side edge has
                // no pins (the body grows there). With no room they stay stacked.
                Some((_, _, b, pinned)) => {
                    let (lo, hi) = if horizontal {
                        (b[1], b[3])
                    } else {
                        (b[0], b[2])
                    };
                    let (lo_free, hi_free) = if horizontal {
                        (!pinned[1], !pinned[3])
                    } else {
                        (!pinned[0], !pinned[2])
                    };
                    let position = |p: (f64, f64)| if horizontal { p.1 } else { p.0 };
                    let mut chosen = None;
                    'search: for pass in 0..2 {
                        for step in 1..=16 {
                            for sign in [1.0, -1.0] {
                                let d = sign * step as f64 * grid;
                                let (h, s) = (along(hot, d), along(new_start, d));
                                let v = position(s);
                                let fits = if pass == 0 {
                                    v > lo + 1e-6 && v < hi - 1e-6
                                } else {
                                    (v >= lo - 1e-6 && v <= hi + 1e-6)
                                        || (v > hi && hi_free)
                                        || (v < lo && lo_free)
                                };
                                if fits && !used.contains(&key(h)) {
                                    chosen = Some((h, s, v < lo || v > hi));
                                    break 'search;
                                }
                            }
                        }
                    }
                    if let Some((h, s, outside)) = chosen {
                        hot = h;
                        new_start = s;
                        grow_body = outside;
                    }
                }
                None => {
                    let mut tries = 0;
                    while used.contains(&key(hot)) && tries < 8 {
                        hot = along(hot, grid);
                        new_start = along(new_start, grid);
                        tries += 1;
                    }
                }
            }
        }
        used.insert(key(hot));
        if body.is_none()
            && ((new_start.0 - start.0).abs() > 0.01 || (new_start.1 - start.1).abs() > 0.01)
        {
            jogs.push((start, new_start));
        }
        pin.pos_x = hot.0;
        pin.pos_y = hot.1;
        pin.length = ((hot.0 - new_start.0).abs() + (hot.1 - new_start.1).abs()).max(1.0);
        if let (Some((index, _, _, _)), true) = (body, grow_body) {
            stretched.push((*index, new_start));
        }
    }
    for (index, point) in stretched {
        let rect = &mut geometry.rectangles[index];
        let (x1, x2) = (rect.x1.min(rect.x2), rect.x1.max(rect.x2));
        let (y1, y2) = (rect.y1.min(rect.y2), rect.y1.max(rect.y2));
        let grow = |lo: f64, hi: f64, v: f64| (lo.min(v), hi.max(v));
        let ((x1, x2), (y1, y2)) = (grow(x1, x2, point.0), grow(y1, y2, point.1));
        rect.x1 = x1;
        rect.x2 = x2;
        rect.y1 = y1;
        rect.y2 = y2;
    }
    let count = jogs.len();
    for (a, b) in jogs {
        geometry.polylines.push(KiPolyline {
            points: vec![a, b],
            stroke_width: 0.0,
            fill: false,
        });
    }
    count
}

fn unit_has_content(unit: &KiSymbolUnit) -> bool {
    !unit.pins.is_empty()
        || !unit.rectangles.is_empty()
        || !unit.circles.is_empty()
        || !unit.arcs.is_empty()
        || !unit.polylines.is_empty()
        || !unit.texts.is_empty()
}

fn unit_bounds(unit: &KiSymbolUnit) -> Option<(f64, f64, f64, f64)> {
    let mut bounds = Bounds::default();
    for pin in &unit.pins {
        bounds.add(pin.pos_x, pin.pos_y);
    }
    for rect in &unit.rectangles {
        bounds.add(rect.x1, rect.y1);
        bounds.add(rect.x2, rect.y2);
    }
    for circle in &unit.circles {
        bounds.add(circle.cx - circle.radius, circle.cy - circle.radius);
        bounds.add(circle.cx + circle.radius, circle.cy + circle.radius);
    }
    for arc in &unit.arcs {
        bounds.add(arc.start_x, arc.start_y);
        bounds.add(arc.mid_x, arc.mid_y);
        bounds.add(arc.end_x, arc.end_y);
    }
    for polyline in &unit.polylines {
        for &(x, y) in &polyline.points {
            bounds.add(x, y);
        }
    }
    bounds.get()
}

fn append_translated(target: &mut KiSymbolUnit, source: &KiSymbolUnit, dx: f64) {
    for pin in &source.pins {
        let mut pin = pin.clone();
        pin.pos_x += dx;
        target.pins.push(pin);
    }
    for rect in &source.rectangles {
        let mut rect = rect.clone();
        rect.x1 += dx;
        rect.x2 += dx;
        target.rectangles.push(rect);
    }
    for circle in &source.circles {
        let mut circle = circle.clone();
        circle.cx += dx;
        target.circles.push(circle);
    }
    for arc in &source.arcs {
        let mut arc = arc.clone();
        arc.start_x += dx;
        arc.mid_x += dx;
        arc.end_x += dx;
        target.arcs.push(arc);
    }
    for polyline in &source.polylines {
        let mut polyline = polyline.clone();
        for point in &mut polyline.points {
            point.0 += dx;
        }
        target.polylines.push(polyline);
    }
    for text in &source.texts {
        let mut text = text.clone();
        text.x += dx;
        target.texts.push(text);
    }
}

/// Graphics XML, pins and body outline of one laid-out unit.
fn draw_unit(
    geometry: &KiSymbolUnit,
    layout: &CaptureLayout,
    warnings: &mut Vec<String>,
) -> (String, Vec<CapturePin>, IntBox) {
    layout.graphics.set(IntBox::empty());
    let mut graphics = String::new();
    for rect in &geometry.rectangles {
        write_rect(&mut graphics, layout, rect);
    }
    for circle in &geometry.circles {
        write_circle(&mut graphics, layout, circle);
    }
    for arc in &geometry.arcs {
        write_arc(&mut graphics, layout, arc);
    }
    for polyline in &geometry.polylines {
        write_polyline(&mut graphics, layout, polyline);
    }
    for text in &geometry.texts {
        write_comment_text(&mut graphics, layout, text);
    }
    let mut pins = capture_pins(&geometry.pins, layout, warnings);
    let bbox = layout.body_bbox(&mut pins, &mut graphics);
    (graphics, pins, bbox)
}

/// Maps EasyEDA px (Y up) to integer Capture units (Y down).
///
/// The translation puts the first pin's connection point exactly on the 0.1" grid
/// and the body near the origin, like symbols drawn in Capture itself.
struct CaptureLayout {
    dx: f64,
    dy: f64,
    /// Bounding box of everything drawn so far (excluding pins).
    graphics: Cell<IntBox>,
}

impl CaptureLayout {
    fn new(geometry: &KiSymbolUnit) -> Self {
        let grid = CAPTURE_GRID as f64;
        let (min_x, min_y) = unit_bounds(geometry)
            .map(|(min_x, _, _, max_y)| (min_x, -max_y))
            .unwrap_or((0.0, 0.0));
        let (pin_x, pin_y) = geometry
            .pins
            .first()
            .map(|pin| (pin.pos_x, -pin.pos_y))
            .unwrap_or((0.0, 0.0));
        // Shift the body towards the origin in whole grid steps, then add the
        // sub-grid correction that lands the reference pin on the grid.
        let dx = -snap(min_x, grid) + (snap(pin_x, grid) - pin_x);
        let dy = -snap(min_y, grid) + (snap(pin_y, grid) - pin_y);
        Self {
            dx,
            dy,
            graphics: Cell::new(IntBox::empty()),
        }
    }

    /// Capture coordinates before rounding to whole units.
    fn point_f(&self, x: f64, y_up: f64) -> (f64, f64) {
        (x + self.dx, -y_up + self.dy)
    }

    fn point(&self, x: f64, y_up: f64) -> (i64, i64) {
        (
            (x + self.dx).round() as i64,
            (-y_up + self.dy).round() as i64,
        )
    }

    /// The part body outline (`SymbolBBox`), laid out like Capture's own parts: on
    /// the 0.1" grid, around every graphic and pin start, with each pin starting on
    /// its edge. Capture's part editor keeps graphics inside this outline and snaps
    /// moves to the grid, so a tight, off-grid outline makes lines jump or refuse to
    /// move. A pin whose body end lay inside the outline is shortened to the edge and
    /// a lead line drawn from the edge to where it used to end; a pin whose tip would
    /// sit on or inside the outline is lengthened to one grid step outside it.
    fn body_bbox(&self, pins: &mut [CapturePin], graphics: &mut String) -> IntBox {
        let grid = CAPTURE_GRID;
        let mut bbox = self.graphics.get();
        for pin in pins.iter() {
            bbox.add(pin.start.0, pin.start.1);
        }
        if bbox.is_empty() {
            bbox.add(0, 0);
            bbox.add(grid, grid);
        }
        let down = |v: i64| v.div_euclid(grid) * grid;
        let up = |v: i64| (v + grid - 1).div_euclid(grid) * grid;
        let mut body = IntBox {
            x1: down(bbox.x1),
            y1: down(bbox.y1),
            x2: up(bbox.x2),
            y2: up(bbox.y2),
        };
        if body.x1 == body.x2 {
            body.x2 += grid;
        }
        if body.y1 == body.y2 {
            body.y2 += grid;
        }
        for pin in pins.iter_mut() {
            let ((hx, hy), (sx, sy)) = (pin.hot, pin.start);
            // The edge the pin leaves from, and its tip at least one grid step
            // outside it: a tip on or inside the outline would make a zero-length
            // pin, which Capture draws as a bare marker.
            let (edge, hot) = if hy == sy && hx < sx {
                ((body.x1, sy), (hx.min(body.x1 - grid), hy))
            } else if hy == sy && hx > sx {
                ((body.x2, sy), (hx.max(body.x2 + grid), hy))
            } else if hx == sx && hy < sy {
                ((sx, body.y1), (hx, hy.min(body.y1 - grid)))
            } else if hx == sx && hy > sy {
                ((sx, body.y2), (hx, hy.max(body.y2 + grid)))
            } else {
                continue;
            };
            if edge != pin.start {
                write_line(graphics, self, edge, pin.start);
                pin.start = edge;
            }
            pin.hot = hot;
        }
        body
    }

    fn record(&self, x: i64, y: i64) {
        let mut bbox = self.graphics.get();
        bbox.add(x, y);
        self.graphics.set(bbox);
    }
}

#[derive(Debug, Clone, Copy)]
struct IntBox {
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
}

impl IntBox {
    const fn empty() -> Self {
        Self {
            x1: i64::MAX,
            y1: i64::MAX,
            x2: i64::MIN,
            y2: i64::MIN,
        }
    }

    fn is_empty(&self) -> bool {
        self.x1 > self.x2
    }

    fn add(&mut self, x: i64, y: i64) {
        self.x1 = self.x1.min(x);
        self.y1 = self.y1.min(y);
        self.x2 = self.x2.max(x);
        self.y2 = self.y2.max(y);
    }
}

#[derive(Debug, Clone)]
struct CapturePin {
    name: String,
    number: String,
    hot: (i64, i64),
    start: (i64, i64),
    pin_type: u8,
    dot: bool,
    clock: bool,
}

fn capture_pins(
    pins: &[KiPin],
    layout: &CaptureLayout,
    warnings: &mut Vec<String>,
) -> Vec<CapturePin> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut off_grid = Vec::new();
    let mut out = Vec::with_capacity(pins.len());

    for pin in pins {
        let length = if pin.length.abs() < 1.0 {
            warnings.push(format!(
                "pin {} had zero length; using {} units",
                pin.number, CAPTURE_GRID
            ));
            CAPTURE_GRID as f64
        } else {
            pin.length.abs()
        };
        // KiCad orientation: direction from the connection point towards the body.
        let angle = (((180 + pin.rotation) % 360 + 360) % 360) as f64;
        let (sin, cos) = angle.to_radians().sin_cos();
        let hot = layout.point(pin.pos_x, pin.pos_y);
        let start = layout.point(pin.pos_x + length * cos, pin.pos_y + length * sin);
        if hot.0 % CAPTURE_GRID != 0 || hot.1 % CAPTURE_GRID != 0 {
            off_grid.push(pin.number.clone());
        }

        let base_number = if pin.number.trim().is_empty() {
            (out.len() + 1).to_string()
        } else {
            pin.number.trim().to_string()
        };
        let count = seen.entry(base_number.clone()).or_insert(0);
        *count += 1;
        let number = if *count == 1 {
            base_number
        } else {
            let renamed = format!("{}_{}", base_number, count);
            warnings.push(format!(
                "duplicate pin number {} renamed to {}",
                base_number, renamed
            ));
            renamed
        };
        let name = if pin.name.trim().is_empty() {
            number.clone()
        } else {
            pin.name.trim().to_string()
        };

        out.push(CapturePin {
            name,
            number,
            hot,
            start,
            pin_type: capture_pin_type(pin.pin_type),
            dot: matches!(
                pin.style,
                PinStyle::Inverted
                    | PinStyle::InvertedClock
                    | PinStyle::InputLow
                    | PinStyle::OutputLow
            ),
            clock: matches!(
                pin.style,
                PinStyle::Clock
                    | PinStyle::InvertedClock
                    | PinStyle::ClockLow
                    | PinStyle::EdgeClockHigh
            ),
        });
    }

    if !off_grid.is_empty() {
        warnings.push(format!(
            "pins not on the 0.1\" grid (wires may not snap): {}",
            off_grid.join(", ")
        ));
    }
    out
}

fn capture_pin_type(pin_type: PinType) -> u8 {
    match pin_type {
        PinType::Input => 0,
        PinType::Bidirectional => 1,
        PinType::Output => 2,
        PinType::OpenCollector => 3,
        PinType::Passive | PinType::Unspecified | PinType::NoConnect => 4,
        PinType::TriState => 5,
        PinType::OpenEmitter => 6,
        PinType::PowerIn | PinType::PowerOut => 7,
    }
}

fn write_pin(out: &mut String, pin: &CapturePin, position: usize) {
    let _ = writeln!(out, "        <SymbolPinScalar>");
    let _ = writeln!(
        out,
        "          <Defn hotptX=\"{}\" hotptY=\"{}\" name=\"{}\" position=\"{}\" startX=\"{}\" startY=\"{}\" type=\"{}\" visible=\"1\"/>",
        pin.hot.0,
        pin.hot.1,
        xml_escape(&pin.name),
        position,
        pin.start.0,
        pin.start.1,
        pin.pin_type
    );
    write_flag(out, 10, "IsLong", true);
    write_flag(out, 10, "IsClock", pin.clock);
    write_flag(out, 10, "IsDot", pin.dot);
    write_flag(out, 10, "IsLeftPointing", false);
    write_flag(out, 10, "IsRightPointing", false);
    write_flag(out, 10, "IsNetStyle", false);
    write_flag(out, 10, "IsNoConnect", false);
    write_flag(out, 10, "IsGlobal", false);
    write_flag(out, 10, "IsNumberVisible", true);
    let _ = writeln!(out, "        </SymbolPinScalar>");
}

fn write_flag(out: &mut String, indent: usize, name: &str, value: bool) {
    let pad = " ".repeat(indent);
    let _ = writeln!(
        out,
        "{pad}<{name}>\n{pad}  <Defn val=\"{}\"/>\n{pad}</{name}>",
        u8::from(value)
    );
}

fn write_display_prop(out: &mut String, name: &str, x: i64, y: i64) {
    let _ = writeln!(
        out,
        "        <SymbolDisplayProp>\n          <Defn locX=\"{x}\" locY=\"{y}\" name=\"{name}\" rotation=\"0\" textJustification=\"0\"/>\n          <PropFont>\n            <Defn charset=\"0\" escapement=\"0\" height=\"-9\" italic=\"0\" name=\"\" orientation=\"0\" weight=\"400\" width=\"4\"/>\n          </PropFont>\n          <PropColor>\n            <Defn val=\"{SYMBOL_COLOR}\"/>\n          </PropColor>\n          <PropDispType>\n            <Defn ValueIfValueExist=\"0\" val=\"1\"/>\n          </PropDispType>\n        </SymbolDisplayProp>"
    );
}

fn write_rect(out: &mut String, layout: &CaptureLayout, rect: &KiRectangle) {
    let (ax, ay) = layout.point(rect.x1, rect.y1);
    let (bx, by) = layout.point(rect.x2, rect.y2);
    let (x1, x2) = (ax.min(bx), ax.max(bx));
    let (y1, y2) = (ay.min(by), ay.max(by));
    if x1 == x2 || y1 == y2 {
        write_line(out, layout, (x1, y1), (x2, y2));
        return;
    }
    layout.record(x1, y1);
    layout.record(x2, y2);
    // Body rectangles are left unfilled so pin names stay readable (Capture default).
    let _ = writeln!(
        out,
        "        <Rect>\n          <Defn fillStyle=\"1\" hatchStyle=\"0\" lineStyle=\"0\" lineWidth=\"0\" x1=\"{x1}\" x2=\"{x2}\" y1=\"{y1}\" y2=\"{y2}\"/>\n        </Rect>"
    );
}

fn write_line(out: &mut String, layout: &CaptureLayout, a: (i64, i64), b: (i64, i64)) {
    if a == b {
        return;
    }
    layout.record(a.0, a.1);
    layout.record(b.0, b.1);
    let _ = writeln!(
        out,
        "        <Line>\n          <Defn lineStyle=\"0\" lineWidth=\"0\" x1=\"{}\" x2=\"{}\" y1=\"{}\" y2=\"{}\"/>\n        </Line>",
        a.0, b.0, a.1, b.1
    );
}

fn write_polygon(out: &mut String, layout: &CaptureLayout, points: &[(i64, i64)], filled: bool) {
    let mut points: Vec<(i64, i64)> = dedup_points(points);
    if points.len() < 3 {
        if points.len() == 2 {
            write_line(out, layout, points[0], points[1]);
        }
        return;
    }
    // Capture closes polygons by repeating the first point.
    if points.first() != points.last() {
        points.push(points[0]);
    }
    let _ = writeln!(
        out,
        "        <Polygon>\n          <Defn fillStyle=\"{}\" hatchStyle=\"0\" lineStyle=\"0\" lineWidth=\"0\"/>",
        if filled { 0 } else { 1 }
    );
    for (x, y) in points {
        layout.record(x, y);
        let _ = writeln!(
            out,
            "          <PolygonPoint>\n            <Defn x=\"{x}\" y=\"{y}\"/>\n          </PolygonPoint>"
        );
    }
    let _ = writeln!(out, "        </Polygon>");
}

fn write_polyline_points(out: &mut String, layout: &CaptureLayout, points: &[(i64, i64)]) {
    let points = dedup_points(points);
    for pair in points.windows(2) {
        write_line(out, layout, pair[0], pair[1]);
    }
}

fn write_circle(out: &mut String, layout: &CaptureLayout, circle: &KiCircle) {
    // Native Capture ellipse (bounding box), as in olb.xsd; fillStyle 0 = solid, 1 = none.
    // Round the corner, then add the rounded diameter, so a circle keeps its size
    // (rounding both corners can shrink a small one by a whole unit).
    let diameter = (2.0 * circle.radius).round().max(1.0) as i64;
    let (x1, y1) = layout.point(circle.cx - circle.radius, circle.cy + circle.radius);
    let (x2, y2) = (x1 + diameter, y1 + diameter);
    // Keep tiny markers (e.g. pin-1 dots) visible after rounding to 0.01".
    let (x2, y2) = (x2.max(x1 + 1), y2.max(y1 + 1));
    layout.record(x1, y1);
    layout.record(x2, y2);
    let _ = writeln!(
        out,
        "        <Ellipse>\n          <Defn fillStyle=\"{}\" hatchStyle=\"0\" lineStyle=\"0\" lineWidth=\"0\" x1=\"{x1}\" x2=\"{x2}\" y1=\"{y1}\" y2=\"{y2}\"/>\n        </Ellipse>",
        if circle.fill { 0 } else { 1 }
    );
}

fn write_arc(out: &mut String, layout: &CaptureLayout, arc: &KiArc) {
    let start = (arc.start_x, arc.start_y);
    let mid = (arc.mid_x, arc.mid_y);
    let end = (arc.end_x, arc.end_y);
    let Some((cx, cy, radius)) = circumcircle(start, mid, end) else {
        let points = [start, mid, end].map(|(x, y)| layout.point(x, y));
        write_polyline_points(out, layout, &points);
        return;
    };

    let a0 = (start.1 - cy).atan2(start.0 - cx);
    let am = (mid.1 - cy).atan2(mid.0 - cx);
    let a1 = (end.1 - cy).atan2(end.0 - cx);
    let ccw = normalize_angle(am - a0) < normalize_angle(a1 - a0);
    let sweep = if ccw {
        normalize_angle(a1 - a0)
    } else {
        -normalize_angle(a0 - a1)
    };
    let segments = ((segments_for_radius(radius) as f64) * sweep.abs() / std::f64::consts::TAU)
        .ceil()
        .max(2.0) as usize;
    let points: Vec<(i64, i64)> = (0..=segments)
        .map(|index| {
            let angle = a0 + sweep * index as f64 / segments as f64;
            layout.point(cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect();
    // Arcs too small for whole Capture units stay line segments.
    if radius < 1.5 || sweep.abs() < 1e-3 {
        write_polyline_points(out, layout, &points);
        return;
    }
    for &(x, y) in &points {
        layout.record(x, y);
    }
    // Native Capture arc: the circle's bounding box plus start and end points; Capture
    // draws counter-clockwise on screen (decreasing angle with Y down) from start to
    // end, as its own Discrete.olb inductor does. Sweeps near a full turn are split.
    let pieces = if sweep.abs() > 1.5 * std::f64::consts::PI {
        2
    } else {
        1
    };
    // The bounding box is in whole units, so pick the representable circle that
    // follows the drawn arc best (rounding the box corners would squash and shift
    // small arcs such as inductor bumps).
    let samples: Vec<(f64, f64)> = (0..=8)
        .map(|i| {
            let angle = a0 + sweep * i as f64 / 8.0;
            layout.point_f(cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect();
    // The written end points are whole units and other lines meet them there: the
    // circle should pass through them.
    let ends = [layout.point(start.0, start.1), layout.point(end.0, end.1)]
        .map(|(x, y)| (x as f64, y as f64));
    let (fcx, fcy) = layout.point_f(cx, cy);
    let mut best = (f64::MAX, (fcx, fcy, radius));
    for half in [0.0, 0.5] {
        let near = |v: f64| (v - half).round() + half;
        for i in -2..=2 {
            for j in -2..=2 {
                for k in -2..=2 {
                    let (ccx, ccy, r) = (
                        near(fcx) + i as f64,
                        near(fcy) + j as f64,
                        near(radius) + k as f64,
                    );
                    if r < 1.0 {
                        continue;
                    }
                    let miss = |&(x, y): &(f64, f64)| ((x - ccx).hypot(y - ccy) - r).powi(2);
                    let err: f64 = samples.iter().map(miss).sum::<f64>()
                        + 4.0 * ends.iter().map(miss).sum::<f64>();
                    if err < best.0 {
                        best = (err, (ccx, ccy, r));
                    }
                }
            }
        }
    }
    let (bcx, bcy, br) = best.1;
    let (x1, y1) = ((bcx - br).round() as i64, (bcy - br).round() as i64);
    let (x2, y2) = ((bcx + br).round() as i64, (bcy + br).round() as i64);
    for piece in 0..pieces {
        let from = a0 + sweep * piece as f64 / pieces as f64;
        let to = a0 + sweep * (piece + 1) as f64 / pieces as f64;
        let at = |angle: f64| layout.point(cx + radius * angle.cos(), cy + radius * angle.sin());
        // Counter-clockwise in EasyEDA's Y-up frame is counter-clockwise on screen.
        let (first, last) = if sweep > 0.0 {
            (at(from), at(to))
        } else {
            (at(to), at(from))
        };
        let _ = writeln!(
            out,
            "        <Arc>\n          <Defn endX=\"{}\" endY=\"{}\" lineStyle=\"0\" lineWidth=\"0\" startX=\"{}\" startY=\"{}\" x1=\"{x1}\" x2=\"{x2}\" y1=\"{y1}\" y2=\"{y2}\"/>\n        </Arc>",
            last.0, last.1, first.0, first.1
        );
    }
}

fn write_polyline(out: &mut String, layout: &CaptureLayout, polyline: &KiPolyline) {
    let points: Vec<(i64, i64)> = polyline
        .points
        .iter()
        .map(|&(x, y)| layout.point(x, y))
        .collect();
    // A filled shape is closed whether or not its last point repeats the first
    // (EasyEDA's MOSFET arrows and diode triangles don't).
    if polyline.fill && dedup_points(&points).len() >= 3 {
        write_polygon(out, layout, &points, true);
    } else {
        write_polyline_points(out, layout, &points);
    }
}

fn write_comment_text(out: &mut String, layout: &CaptureLayout, text: &KiText) {
    let content = text.text.trim();
    if content.is_empty() {
        return;
    }
    // EasyEDA anchors text at its baseline; Capture at the top-left of the text box.
    let (x, baseline) = layout.point(text.x, text.y);
    let height = CAPTURE_GRID;
    let y = baseline - height;
    let width = (content.chars().count() as i64 * 5).max(5);
    layout.record(x, y);
    layout.record(x + width, baseline);
    let _ = writeln!(
        out,
        "        <CommentText>\n          <Defn locX=\"{x}\" locY=\"{y}\" name=\"{}\" textJustification=\"0\" x1=\"{x}\" x2=\"{}\" y1=\"{y}\" y2=\"{baseline}\"/>\n          <TextFont>\n            <Defn charset=\"0\" escapement=\"0\" height=\"-9\" italic=\"0\" name=\"Arial\" orientation=\"0\" weight=\"400\" width=\"4\"/>\n          </TextFont>\n        </CommentText>",
        xml_escape(content),
        x + width
    );
}

fn dedup_points(points: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(points.len());
    for &point in points {
        if out.last() != Some(&point) {
            out.push(point);
        }
    }
    out
}

fn segments_for_radius(radius: f64) -> usize {
    // Capture units are coarse (0.01"); more segments than this only adds rounding noise.
    (radius.abs() * 2.0).clamp(8.0, 36.0) as usize
}

fn circumcircle(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> Option<(f64, f64, f64)> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-9 {
        return None;
    }
    let a2 = a.0 * a.0 + a.1 * a.1;
    let b2 = b.0 * b.0 + b.1 * b.1;
    let c2 = c.0 * c.0 + c.1 * c.1;
    let ux = (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d;
    let uy = (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d;
    let radius = ((a.0 - ux).powi(2) + (a.1 - uy).powi(2)).sqrt();
    Some((ux, uy, radius))
}

fn normalize_angle(angle: f64) -> f64 {
    angle.rem_euclid(std::f64::consts::TAU)
}

/// Nearest multiple of `grid`, halves rounding up, so pins half a grid off all move
/// the same way and keep their spacing.
fn snap(value: f64, grid: f64) -> f64 {
    (value / grid + 0.5 + 1e-9).floor() * grid
}

#[derive(Default)]
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
    any: bool,
}

impl Bounds {
    fn add(&mut self, x: f64, y: f64) {
        if self.any {
            self.min_x = self.min_x.min(x);
            self.min_y = self.min_y.min(y);
            self.max_x = self.max_x.max(x);
            self.max_y = self.max_y.max(y);
        } else {
            *self = Bounds {
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
                any: true,
            };
        }
    }

    fn get(&self) -> Option<(f64, f64, f64, f64)> {
        self.any
            .then_some((self.min_x, self.min_y, self.max_x, self.max_y))
    }
}

pub(crate) fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Control characters are not allowed in XML 1.0 attributes.
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// `DefaultValues` block copied from Capture 17.2's own Library XML exports.
fn default_values_xml() -> String {
    let mut xml = String::from("  <DefaultValues>\n    <Defn/>\n");
    for index in 0..24 {
        let (height, width) = if matches!(index, 3 | 4 | 17..=23) {
            ("8", "0")
        } else {
            ("-9", if index == 13 { "5" } else { "4" })
        };
        let name = if index == 13 { "Courier New" } else { "Arial" };
        let _ = writeln!(
            xml,
            "    <DefaultFont>\n      <Defn charset=\"0\" escapement=\"0\" height=\"{height}\" index=\"{index}\" italic=\"0\" name=\"{name}\" orientation=\"0\" weight=\"400\" width=\"{width}\"/>\n    </DefaultFont>"
        );
    }
    xml.push_str("    <DefaultPageRec>\n      <Defn ANSIGridRefs=\"1\" BorderDisplayed=\"1\" BorderPrinted=\"1\" GridRefDisplayed=\"1\" GridRefPrinted=\"1\" HorizontalLabelCount=\"5\" HorizontalLabelIsAscending=\"0\" HorizontalLabelIsChar=\"0\" HorizontalLabelWidth=\"2540\" IsMetric=\"1\" PinToPin=\"2540\" TitleBlockDisplayed=\"1\" TitleBlockPrinted=\"1\" VerticalLabelCount=\"4\" VerticalLabelIsAscending=\"0\" VerticalLabelIsChar=\"1\" VerticalLabelWidth=\"2540\"/>\n    </DefaultPageRec>\n");
    xml.push_str("    <DefaultPlacedInstIsPrimitive>\n      <Defn val=\"0\"/>\n    </DefaultPlacedInstIsPrimitive>\n");
    xml.push_str("    <DefaultDrawnInstIsPrimitive>\n      <Defn val=\"0\"/>\n    </DefaultDrawnInstIsPrimitive>\n");
    let fields = [
        "1ST PART FIELD",
        "2ND PART FIELD",
        "3RD PART FIELD",
        "4TH PART FIELD",
        "5TH PART FIELD",
        "6TH PART FIELD",
        "7TH PART FIELD",
        "PCB Footprint",
    ];
    for (index, field) in fields.iter().enumerate() {
        let _ = writeln!(
            xml,
            "    <DefaultPartFieldMapping>\n      <Defn index=\"{}\" val=\"{field}\"/>\n    </DefaultPartFieldMapping>",
            index + 1
        );
    }
    xml.push_str("  </DefaultValues>\n");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_xml_round_trips_its_packages() {
        let parts = [
            "  <Package>\n    <Defn name=\"R_A&amp;B_C1\" pcbFootprint=\"resc1005x40n\"/>\n  </Package>\n"
                .to_string(),
            "  <Package>\n    <Defn name=\"U_C2\" pcbFootprint=\"sot95p280x145-5n\"/>\n  </Package>\n"
                .to_string(),
        ];
        let xml = render_library_xml("Parts.olb", &parts);
        let blocks = package_blocks(&xml);
        assert_eq!(
            blocks
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["R_A&B_C1", "U_C2"]
        );
        let again: Vec<String> = blocks.into_iter().map(|(_, block)| block).collect();
        assert_eq!(render_library_xml("Parts.olb", &again), xml);
    }

    #[test]
    fn identifiers_are_plain_ascii() {
        assert_eq!(capture_identifier("10kΩ 0402 (1%)"), "10k_0402_1");
        assert_eq!(capture_identifier("***"), "PART");
    }
}
