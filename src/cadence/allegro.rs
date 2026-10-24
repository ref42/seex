//! Allegro / OrCAD PCB Editor footprint export.
//!
//! Allegro package symbols (`.dra` / `.psm`) and padstacks (`.pad`) are binary and
//! undocumented, and only Allegro itself writes them. SeEx therefore writes one
//! self-contained SKILL script per footprint, plus one library script that builds
//! them all: loaded in Allegro PCB Editor (`skill load("…/<library>.il")`), it opens a
//! new package symbol drawing for each footprint, runs its script and saves the
//! `.dra`, `.psm` and `.pad` files next to the scripts. Each footprint script can
//! also be loaded on its own into an empty package symbol drawing.
//!
//! A script creates the padstacks, places the pins, draws the silkscreen, the place
//! bound and the reference designator labels, and maps the STEP model. Every
//! Allegro call that has varied between releases runs under `errset` with a
//! fallback, so a single unsupported call degrades one feature instead of aborting.
//! SKILL needs Allegro PCB Designer or OrCAD PCB Designer Professional; the base
//! OrCAD PCB Designer tier does not run it.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::kicad::footprint::{KiArc, KiCircle, KiFootprint, KiLine, KiPad, PadShape, PadType};

/// EasyEDA footprint unit: 10 mil.
const PX_TO_MM: f64 = 0.254;
/// Solder mask opening added to every copper pad (total, i.e. 0.05 mm per side).
const MASK_EXPANSION_MM: f64 = 0.1;
/// Anti-pad / thermal relief of a plated hole on plane layers: pad + 0.5 mm
/// (0.25 mm clearance per side), the usual Allegro default.
const ANTI_EXPANSION_MM: f64 = 0.5;
/// Clearance between the outermost geometry and the generated place bound.
const PLACE_BOUND_CLEARANCE_MM: f64 = 0.25;
/// Allegro still truncates/limits legacy names; stay inside the conservative limits.
const MAX_SYMBOL_NAME: usize = 31;
const MAX_PADSTACK_NAME: usize = 20;

const SILK_TOP: &str = "PACKAGE GEOMETRY/SILKSCREEN_TOP";
/// The silkscreen reference designator sits this far above the place bound (its
/// baseline; Allegro anchors text at the baseline), centred over the part.
const REFDES_GAP_MM: f64 = 0.3;

// Line widths and text sizes follow the KiCad Library Conventions F5.1 / F5.2,
// which put the IPC drawing layers into numbers:
/// Silkscreen lines: 0.12 mm nominal (F5.1).
pub const SILK_LINE_MM: f64 = 0.12;
/// Assembly (fabrication) lines: 0.10 mm (F5.2).
pub const ASSEMBLY_LINE_MM: f64 = 0.10;
/// Silkscreen reference designator: 1.0 mm high, 0.15 mm strokes (F5.1) on
/// parts wide enough for it; on small parts it shrinks with the part, down to
/// 0.6 mm, so the label is about as wide as the part.
pub const SILK_TEXT_MM: f64 = 1.0;
pub const SILK_TEXT_MIN_MM: f64 = 0.6;
/// Silkscreen strokes never go below this, whatever the text height.
pub const SILK_STROKE_MIN_MM: f64 = 0.10;

/// Height of the silkscreen reference designator `label` over a part whose
/// place bound is `bound`: the label (at least three characters) spans the
/// part's width, between 0.6 and 1.0 mm high.
pub fn silk_refdes_height(bound: Option<(f64, f64, f64, f64)>, label: &str) -> f64 {
    let Some((x1, _, x2, _)) = bound else {
        return SILK_TEXT_MM;
    };
    let chars = label.chars().count().max(3) as f64;
    ((x2 - x1) / (chars * CHAR_ADVANCE)).clamp(SILK_TEXT_MIN_MM, SILK_TEXT_MM)
}

/// Stroke width for silkscreen text `height` mm high: 15 %, at least 0.10 mm.
pub fn silk_stroke(height: f64) -> f64 {
    (height * 0.15).max(SILK_STROKE_MIN_MM)
}
/// Assembly reference designator: 0.5-1.0 mm high, scaled so about four
/// characters fit the body; strokes 15 % of the height (F5.2).
pub const ASSEMBLY_TEXT_MIN_MM: f64 = 0.5;
pub const ASSEMBLY_TEXT_MAX_MM: f64 = 1.0;
/// Text block used for the silkscreen reference designator.
const SILK_TEXT_BLOCK: u32 = 3;
/// Text block used for the assembly reference designator.
const ASSEMBLY_TEXT_BLOCK: u32 = 2;
/// Advance of one character as a fraction of the text height.
const CHAR_ADVANCE: f64 = 0.9;

/// Height, rotation and position of the assembly reference designator `label`
/// for a body `(x1, y1, x2, y2)`: centred on the component along its long
/// axis, scaled so the label (at least three characters) fits, 0.5-1.0 mm.
pub fn assembly_refdes_layout(
    body: Option<(f64, f64, f64, f64)>,
    label: &str,
) -> (f64, f64, f64, f64) {
    let chars = label.chars().count().max(3) as f64;
    let Some((x1, y1, x2, y2)) = body else {
        return (ASSEMBLY_TEXT_MAX_MM, 0.0, 0.0, -ASSEMBLY_TEXT_MAX_MM / 2.0);
    };
    let (w, h) = (x2 - x1, y2 - y1);
    let (major, minor, rotation) = if h > w { (h, w, 90.0) } else { (w, h, 0.0) };
    let (cx, cy) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
    let height = (major / (chars * CHAR_ADVANCE))
        .min(minor * 0.9)
        .clamp(ASSEMBLY_TEXT_MIN_MM, ASSEMBLY_TEXT_MAX_MM);
    // Centred on the component: the baseline half a text height below (or
    // beside) the centre.
    if rotation == 0.0 {
        (height, rotation, cx, cy - height / 2.0)
    } else {
        (height, rotation, cx + height / 2.0, cy)
    }
}
const SILK_BOTTOM: &str = "PACKAGE GEOMETRY/SILKSCREEN_BOTTOM";
const ASSY_TOP: &str = "PACKAGE GEOMETRY/ASSEMBLY_TOP";
const ASSY_BOTTOM: &str = "PACKAGE GEOMETRY/ASSEMBLY_BOTTOM";
const PLACE_BOUND_TOP: &str = "PACKAGE GEOMETRY/PLACE_BOUND_TOP";

/// A generated Allegro footprint script.
#[derive(Debug, Clone)]
pub struct AllegroFootprint {
    /// Complete, self-contained SKILL script.
    pub script: String,
    /// Non-fatal conversion notes.
    pub warnings: Vec<String>,
}

/// What a script draws, kept to place the reference designators (mm, Y up).
#[derive(Debug, Clone, Default)]
struct Drawing {
    /// Pad centres and the half size of the larger pad side.
    pads: Vec<(f64, f64, f64)>,
    /// Lines and arcs as polylines: layer and points.
    lines: Vec<(String, Vec<(f64, f64)>)>,
    /// Circles: layer, centre and radius.
    circles: Vec<(String, (f64, f64), f64)>,
}

/// Allegro-safe lower-case identifier (`[a-z0-9_-]`), truncated to `max_len`.
pub fn allegro_identifier(name: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = false;
    for ch in name.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '-' {
            ch.to_ascii_lowercase()
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
    let mut out = out.trim_matches('_').to_string();
    if out.is_empty() {
        out = "seex_fp".to_string();
    }
    if out.len() > max_len {
        out.truncate(max_len);
        out = out.trim_end_matches(['_', '-']).to_string();
    }
    out
}

/// Package symbol name for an LCSC component: the LCSC id first so truncation never
/// makes two parts collide.
pub fn footprint_name_for(lcsc_id: &str, title: &str) -> String {
    allegro_identifier(&format!("{}_{}", lcsc_id, title), MAX_SYMBOL_NAME)
}

/// Rotation (degrees) and offset (mm) of a STEP model on its footprint, in
/// Allegro's axes (Y up).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StepTransform {
    pub rotation: [f64; 3],
    pub offset: [f64; 3],
}

/// A STEP model to map onto the footprint.
pub struct StepModel<'a> {
    /// File name; Allegro stores only this and finds the file through STEPPATH.
    pub file: &'a str,
    /// Folder holding the file.
    pub dir: &'a std::path::Path,
    pub transform: StepTransform,
}

/// Build the SKILL script for `footprint`, mapping `step` as its primary 3D model.
pub fn build_allegro_footprint_with(
    footprint: &KiFootprint,
    name: &str,
    refdes_prefix: &str,
    step: Option<StepModel>,
) -> AllegroFootprint {
    let mut builder = Builder::default();
    builder.pins_and_padstacks(footprint);
    builder.graphics(footprint);
    let place_bound = place_bound(footprint);
    // The component frame is the silkscreen: the assembly layer keeps only the
    // reference designator, centred on the body the assembly outline gave.
    let body = assembly_box(&builder.drawing);
    builder.drop_assembly_graphics();

    let refdes = format!("{}*", refdes_label(refdes_prefix));
    let mut script = String::new();
    let _ = writeln!(
        script,
        ";; {name}.il - Allegro package symbol generated by SeEx (https://github.com/ref42/seex)\n\
         ;;\n\
         ;; The library script in this folder builds every footprint at once. To build\n\
         ;; only this one:\n\
         ;; 1. Allegro PCB Editor: File > New, Drawing Type \"Package symbol\", name \"{name}\".\n\
         ;; 2. Command window:   skill load(\"<this folder>/{name}.il\")\n\
         ;; 3. File > Save writes {name}.dra; File > Create Symbol writes {name}.psm.\n\
         ;; Padstacks are written as .pad files to the current working directory; keep\n\
         ;; them on your PADPATH together with the .psm on your PSMPATH.\n"
    );
    script.push_str(HELPERS);
    let _ = writeln!(
        script,
        "\nwhen( seex_begin(\"{name}\" {})",
        f(drawing_half_extent(footprint))
    );
    for line in &builder.padstack_calls {
        let _ = writeln!(script, "  {line}");
    }
    for line in &builder.body {
        let _ = writeln!(script, "  {line}");
    }
    let silk_height = silk_refdes_height(place_bound, &refdes);
    let _ = writeln!(
        script,
        "  seex_text_block({SILK_TEXT_BLOCK} {} {} {})",
        f(silk_height),
        f(silk_height),
        f(silk_stroke(silk_height))
    );
    let body = body.or_else(|| pad_box(&builder.drawing));
    let (assembly_height, assembly_rotation, assembly_x, assembly_y) =
        assembly_refdes_layout(body, &refdes);
    let _ = writeln!(
        script,
        "  seex_text_block({ASSEMBLY_TEXT_BLOCK} {} {} {})",
        f(assembly_height),
        f(assembly_height),
        f(assembly_height * 0.15)
    );
    if let Some((x1, y1, x2, y2)) = place_bound {
        let _ = writeln!(
            script,
            "  seex_place_bound(\"{PLACE_BOUND_TOP}\" {} {} {} {})",
            f(x1),
            f(y1),
            f(x2),
            f(y2)
        );
        let label_y = y2 + REFDES_GAP_MM;
        let _ = writeln!(
            script,
            "  seex_text(\"REF DES/SILKSCREEN_TOP\" \"{refdes}\" {} {} {SILK_TEXT_BLOCK} 0.0)",
            f((x1 + x2) / 2.0),
            f(label_y)
        );
    } else {
        let _ = writeln!(
            script,
            "  seex_text(\"REF DES/SILKSCREEN_TOP\" \"{refdes}\" 0.0 1.0 {SILK_TEXT_BLOCK} 0.0)"
        );
    }
    let _ = writeln!(
        script,
        "  seex_text(\"REF DES/ASSEMBLY_TOP\" \"{refdes}\" {} {} {ASSEMBLY_TEXT_BLOCK} {})",
        f(assembly_x),
        f(assembly_y),
        f(assembly_rotation)
    );
    if let Some(step) = step {
        let [rx, ry, rz] = step.transform.rotation;
        let [ox, oy, oz] = step.transform.offset;
        let _ = writeln!(
            script,
            "  seex_step(\"{}\" \"{}\" {} {} {} {} {} {})",
            skill_string(step.file),
            skill_string(&step.dir.to_string_lossy().replace('\\', "/")),
            f(rx),
            f(ry),
            f(rz),
            f(ox),
            f(oy),
            f(oz)
        );
    }
    let _ = writeln!(script, "  seex_end(\"{name}\")\n)");

    AllegroFootprint {
        script,
        warnings: builder.warnings,
    }
}

/// Extent of the copper, for parts without a body outline.
fn pad_box(drawing: &Drawing) -> Option<(f64, f64, f64, f64)> {
    let points: Vec<(f64, f64)> = drawing
        .pads
        .iter()
        .flat_map(|&(x, y, half)| [(x - half, y - half), (x + half, y + half)])
        .collect();
    if points.is_empty() {
        return None;
    }
    let (x1, y1, x2, y2) = bounds(&points);
    (x2 - x1 > 1e-6 && y2 - y1 > 1e-6).then_some((x1, y1, x2, y2))
}

/// Extent of the assembly-top outline, i.e. the component body.
fn assembly_box(drawing: &Drawing) -> Option<(f64, f64, f64, f64)> {
    let points: Vec<(f64, f64)> = drawing
        .lines
        .iter()
        .filter(|(layer, _)| layer == ASSY_TOP)
        .flat_map(|(_, points)| points.iter().copied())
        .chain(
            drawing
                .circles
                .iter()
                .filter(|(layer, ..)| layer == ASSY_TOP)
                .flat_map(|(_, (x, y), r)| [(x - r, y - r), (x + r, y + r)]),
        )
        .collect();
    if points.is_empty() {
        return None;
    }
    let (x1, y1, x2, y2) = bounds(&points);
    (x2 - x1 > 1e-6 && y2 - y1 > 1e-6).then_some((x1, y1, x2, y2))
}

fn refdes_label(prefix: &str) -> String {
    let prefix: String = prefix
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

#[derive(Default)]
struct Builder {
    padstack_calls: Vec<String>,
    padstack_names: Vec<String>,
    body: Vec<String>,
    warnings: Vec<String>,
    drawing: Drawing,
}

impl Builder {
    /// Removes the assembly outline (lines, arcs, circles) from the drawing and
    /// the script.
    fn drop_assembly_graphics(&mut self) {
        let assembly = |layer: &str| layer == ASSY_TOP || layer == ASSY_BOTTOM;
        self.drawing.lines.retain(|(layer, ..)| !assembly(layer));
        self.drawing.circles.retain(|(layer, ..)| !assembly(layer));
        self.body.retain(|call| {
            !call.contains(&format!("\"{ASSY_TOP}\""))
                && !call.contains(&format!("\"{ASSY_BOTTOM}\""))
        });
    }

    fn pins_and_padstacks(&mut self, footprint: &KiFootprint) {
        let mut used_numbers: HashMap<String, usize> = HashMap::new();
        let mut skipped_vias = 0usize;

        for pad in &footprint.pads {
            let number = pad.number.trim().to_string();
            let is_mechanical = pad.pad_type == PadType::NpThroughHole;
            if number.is_empty() && !is_mechanical {
                // Unnumbered plated holes are EasyEDA vias (usually thermal vias in an
                // exposed pad). As Allegro pins they would be unconnected mechanical
                // pins inside a copper pad, so they are left out on purpose.
                skipped_vias += 1;
                continue;
            }

            let (x, y) = to_mm(pad.pos_x, pad.pos_y);
            let pin_number = if is_mechanical {
                None
            } else {
                let count = used_numbers.entry(number.clone()).or_insert(0);
                *count += 1;
                if *count == 1 {
                    Some(number.clone())
                } else {
                    let renamed = format!("{}_{}", number, count);
                    self.warnings.push(format!(
                        "pad number {number} is used more than once; extra pad renamed {renamed} \
                         (Allegro pin numbers must be unique)"
                    ));
                    Some(renamed)
                }
            };

            let Some(stack) = Padstack::for_pad(pad, &mut self.warnings) else {
                continue;
            };
            self.declare_padstack(&stack);

            let rotation = if stack.custom_polygon.is_some() {
                0.0
            } else {
                normalize_degrees(pad.rotation)
            };
            let number_arg = match &pin_number {
                Some(number) => format!("\"{}\"", skill_escape(number)),
                None => "nil".to_string(),
            };
            self.drawing
                .pads
                .push((x, y, stack.width.max(stack.height) / 2.0));
            self.body.push(format!(
                "seex_pin(\"{}\" {} {} {} {})",
                stack.name,
                number_arg,
                f(x),
                f(y),
                f(rotation)
            ));

            if let Some(points) = &stack.custom_polygon {
                let absolute: Vec<(f64, f64)> =
                    points.iter().map(|(px, py)| (x + px, y + py)).collect();
                let (copper, mask, paste) = if stack.bottom {
                    (
                        "ETCH/BOTTOM",
                        "PACKAGE GEOMETRY/SOLDERMASK_BOTTOM",
                        "PACKAGE GEOMETRY/PASTEMASK_BOTTOM",
                    )
                } else {
                    (
                        "ETCH/TOP",
                        "PACKAGE GEOMETRY/SOLDERMASK_TOP",
                        "PACKAGE GEOMETRY/PASTEMASK_TOP",
                    )
                };
                for layer in [copper, mask, paste] {
                    self.body.push(format!(
                        "seex_shape(\"{layer}\" {})",
                        points_list(&absolute)
                    ));
                }
            }
        }

        if skipped_vias > 0 {
            self.warnings.push(format!(
                "{skipped_vias} unnumbered via(s) skipped; add thermal vias to the pad in Allegro if needed"
            ));
        }
    }

    fn declare_padstack(&mut self, stack: &Padstack) {
        if self.padstack_names.contains(&stack.name) {
            return;
        }
        self.padstack_names.push(stack.name.clone());
        self.padstack_calls.push(stack.skill());
        self.padstack_calls.push(stack.check_call());
    }

    fn graphics(&mut self, footprint: &KiFootprint) {
        let mut skipped_layers: Vec<String> = Vec::new();
        let mut note_skip = |layer: &str| {
            if !skipped_layers.iter().any(|known| known == layer) {
                skipped_layers.push(layer.to_string());
            }
        };

        for line in &footprint.lines {
            match graphic_layer(&line.layer) {
                Some(layer) => self.line(layer, line),
                None if is_courtyard(&line.layer) => {}
                None => note_skip(&line.layer),
            }
        }
        for circle in &footprint.circles {
            match graphic_layer(&circle.layer) {
                Some(layer) => self.circle(layer, circle),
                None if is_courtyard(&circle.layer) => {}
                None => note_skip(&circle.layer),
            }
        }
        for arc in &footprint.arcs {
            match graphic_layer(&arc.layer) {
                Some(layer) => self.arc(layer, arc),
                None if is_courtyard(&arc.layer) => {}
                None => note_skip(&arc.layer),
            }
        }

        if !skipped_layers.is_empty() {
            self.warnings.push(format!(
                "graphics on layers without an Allegro equivalent were skipped: {}",
                skipped_layers.join(", ")
            ));
        }
        if !footprint.texts.is_empty() {
            self.warnings.push(format!(
                "{} footprint text item(s) not converted (Allegro text blocks are design specific)",
                footprint.texts.len()
            ));
        }
    }

    fn line(&mut self, layer: &str, line: &KiLine) {
        let a = to_mm(line.start_x, line.start_y);
        let b = to_mm(line.end_x, line.end_y);
        if distance(a, b) < 1e-6 {
            return;
        }
        self.drawing.lines.push((layer.to_string(), vec![a, b]));
        self.body.push(format!(
            "seex_line(\"{layer}\" {} {})",
            f(line_width(layer, line.width)),
            points_list(&[a, b])
        ));
    }

    fn circle(&mut self, layer: &str, circle: &KiCircle) {
        let center = to_mm(circle.center_x, circle.center_y);
        let edge = to_mm(circle.end_x, circle.end_y);
        let radius = distance(center, edge);
        if radius < 1e-6 {
            return;
        }
        self.drawing
            .circles
            .push((layer.to_string(), center, radius));
        self.body.push(format!(
            "seex_circle(\"{layer}\" {} {} {} {} {})",
            f(line_width_mm(layer, circle.width)),
            f(center.0),
            f(center.1),
            f(radius),
            if circle.fill { "t" } else { "nil" }
        ));
    }

    fn arc(&mut self, layer: &str, arc: &KiArc) {
        let start = to_mm(arc.start_x, arc.start_y);
        let mid = to_mm(arc.mid_x, arc.mid_y);
        let end = to_mm(arc.end_x, arc.end_y);
        let width = line_width(layer, arc.width);
        let Some((center, _)) = circumcircle(start, mid, end) else {
            self.drawing
                .lines
                .push((layer.to_string(), vec![start, mid, end]));
            self.body.push(format!(
                "seex_line(\"{layer}\" {} {})",
                f(width),
                points_list(&[start, mid, end])
            ));
            return;
        };
        // Y points up here, so a negative cross product means clockwise travel.
        let cross = (mid.0 - start.0) * (end.1 - mid.1) - (mid.1 - start.1) * (end.0 - mid.0);
        let clockwise = cross < 0.0;
        let fallback = arc_polyline(start, end, center, clockwise);
        self.drawing
            .lines
            .push((layer.to_string(), fallback.clone()));
        self.body.push(format!(
            "seex_arc(\"{layer}\" {} {} {} {} {} {} {} {} {})",
            f(width),
            f(start.0),
            f(start.1),
            f(end.0),
            f(end.1),
            f(center.0),
            f(center.1),
            if clockwise { "t" } else { "nil" },
            points_list(&fallback)
        ));
    }
}

/// Padstack definition derived from one EasyEDA pad.
struct Padstack {
    name: String,
    /// SKILL figure symbol and size (mm) of the copper pad.
    figure: &'static str,
    width: f64,
    height: f64,
    kind: PadstackKind,
    bottom: bool,
    /// Pad-relative outline (mm, Y up) for custom polygon pads.
    custom_polygon: Option<Vec<(f64, f64)>>,
}

enum PadstackKind {
    Smd,
    Plated { drill_x: f64, drill_y: f64 },
    NonPlated { drill: f64 },
}

impl Padstack {
    fn for_pad(pad: &KiPad, warnings: &mut Vec<String>) -> Option<Self> {
        let bottom = pad.layers.iter().any(|layer| layer.starts_with("B."))
            && !pad.layers.iter().any(|layer| layer.starts_with("F."));
        let mut width = px(pad.size_x).max(0.01);
        let mut height = px(pad.size_y).max(0.01);

        let kind = match pad.pad_type {
            PadType::NpThroughHole => {
                let drill = pad
                    .drill
                    .as_ref()
                    .map(|drill| px(drill.diameter))
                    .unwrap_or(width);
                PadstackKind::NonPlated { drill }
            }
            PadType::ThroughHole => {
                let Some(drill) = &pad.drill else {
                    warnings.push(format!(
                        "through-hole pad {} has no drill; skipped",
                        pad.number
                    ));
                    return None;
                };
                let drill_x = px(drill.diameter);
                let drill_y = drill.width.map(px).unwrap_or(drill_x);
                PadstackKind::Plated { drill_x, drill_y }
            }
            PadType::Smd | PadType::Connect => PadstackKind::Smd,
        };

        let mut custom_polygon = None;
        let mut figure = match pad.shape {
            PadShape::Circle => "CIRCLE",
            PadShape::Oval => "OBLONG",
            _ => "RECTANGLE",
        };
        if figure == "CIRCLE" {
            let diameter = width.max(height);
            width = diameter;
            height = diameter;
        }
        if figure == "OBLONG" && (width - height).abs() < 1e-6 {
            figure = "CIRCLE";
        }
        // Allegro rejects a RECTANGLE with equal sides ("Illegal figure size").
        if figure == "RECTANGLE" && (width - height).abs() < 1e-6 {
            figure = "SQUARE";
        }

        if pad.shape == PadShape::Custom {
            let points = pad
                .polygon
                .as_deref()
                .map(parse_kicad_polygon)
                .unwrap_or_default();
            if points.len() >= 3 {
                let (min_x, min_y, max_x, max_y) = bounds(&points);
                // A small round pad anchors the pin; the real outline is drawn as
                // copper / mask / paste shapes around it.
                let anchor = ((max_x - min_x).min(max_y - min_y) * 0.5).clamp(0.1, 1.0);
                width = anchor;
                height = anchor;
                figure = "CIRCLE";
                warnings.push(format!(
                    "pad {} is a custom polygon: emitted as a {:.2} mm anchor pad plus copper/mask/paste shapes",
                    pad.number, anchor
                ));
                custom_polygon = Some(points);
            } else {
                warnings.push(format!(
                    "pad {} is a custom polygon without usable points; using its bounding size",
                    pad.number
                ));
            }
        }

        let name = padstack_name(figure, width, height, &kind, bottom);
        Some(Self {
            name,
            figure,
            width,
            height,
            kind,
            bottom,
            custom_polygon,
        })
    }

    /// Layers (as Allegro reports them) and pad types this padstack must have.
    fn required_layers(&self) -> Vec<(&'static str, &'static str)> {
        match &self.kind {
            PadstackKind::Smd if self.bottom => vec![
                ("ETCH/BOTTOM", "REGULAR"),
                ("PIN/SOLDERMASK_BOTTOM", "REGULAR"),
                ("PIN/PASTEMASK_BOTTOM", "REGULAR"),
            ],
            PadstackKind::Smd => vec![
                ("ETCH/TOP", "REGULAR"),
                ("PIN/SOLDERMASK_TOP", "REGULAR"),
                ("PIN/PASTEMASK_TOP", "REGULAR"),
            ],
            PadstackKind::Plated { .. } => vec![
                ("ETCH/TOP", "REGULAR"),
                ("ETCH/TOP", "ANTI"),
                ("ETCH/TOP", "THERMAL"),
                ("ETCH/BOTTOM", "REGULAR"),
                ("ETCH/BOTTOM", "ANTI"),
                ("ETCH/BOTTOM", "THERMAL"),
                ("PIN/SOLDERMASK_TOP", "REGULAR"),
                ("PIN/SOLDERMASK_BOTTOM", "REGULAR"),
            ],
            PadstackKind::NonPlated { .. } => vec![
                ("PIN/SOLDERMASK_TOP", "REGULAR"),
                ("PIN/SOLDERMASK_BOTTOM", "REGULAR"),
            ],
        }
    }

    /// `seex_check_padstack(name required drilled)`: reads the padstack back and
    /// reports any required layer it lacks.
    fn check_call(&self) -> String {
        let required: Vec<String> = self
            .required_layers()
            .iter()
            .map(|(layer, kind)| format!("list(\"{layer}\" \"{kind}\")"))
            .collect();
        let drilled = if matches!(self.kind, PadstackKind::Smd) {
            "nil"
        } else {
            "t"
        };
        format!(
            "seex_check_padstack(\"{}\" list({}) {drilled})",
            self.name,
            required.join(" ")
        )
    }

    /// `seex_padstack(name drill pads fallbackDrill fallbackPads)` call.
    fn skill(&self) -> String {
        let fig = format!("'{}", self.figure);
        let fallback_fig = if self.figure == "OBLONG" {
            "'RECTANGLE".to_string()
        } else {
            fig.clone()
        };
        let (w, h) = (self.width, self.height);
        let (mw, mh) = (w + MASK_EXPANSION_MM, h + MASK_EXPANSION_MM);
        match &self.kind {
            PadstackKind::Smd => {
                let (copper, mask, paste) = if self.bottom {
                    ("BOTTOM", "SOLDERMASK_BOTTOM", "PASTEMASK_BOTTOM")
                } else {
                    ("TOP", "SOLDERMASK_TOP", "PASTEMASK_TOP")
                };
                let pads = |fig: &str| {
                    format!(
                        "list({} {} {})",
                        pad_call(copper, fig, w, h),
                        pad_call(mask, fig, mw, mh),
                        pad_call(paste, fig, w, h)
                    )
                };
                format!(
                    "seex_padstack(\"{}\" nil {} nil {})",
                    self.name,
                    pads(&fig),
                    pads(&fallback_fig)
                )
            }
            PadstackKind::Plated { drill_x, drill_y } => {
                let (aw, ah) = (w + ANTI_EXPANSION_MM, h + ANTI_EXPANSION_MM);
                let all_layers = |fig: &str, internal: bool| {
                    let mut etch = vec!["TOP"];
                    if internal {
                        etch.push("DEFAULT_INTERNAL");
                    }
                    etch.push("BOTTOM");
                    let mut calls = Vec::new();
                    for layer in etch {
                        calls.push(pad_call(layer, fig, w, h));
                        calls.push(typed_pad_call(layer, "ANTI", fig, aw, ah));
                        calls.push(typed_pad_call(layer, "THERMAL", fig, aw, ah));
                    }
                    calls.push(pad_call("SOLDERMASK_TOP", fig, mw, mh));
                    calls.push(pad_call("SOLDERMASK_BOTTOM", fig, mw, mh));
                    format!("list({})", calls.join(" "))
                };
                let is_slot = (drill_x - drill_y).abs() > 1e-6;
                let drill = if is_slot {
                    format!("seex_slot({} {} t)", f(*drill_x), f(*drill_y))
                } else {
                    format!("seex_drill({} t)", f(*drill_x))
                };
                // Fallback: round drill (smallest slot dimension) and no internal pad.
                let fallback_drill = format!("seex_drill({} t)", f(drill_x.min(*drill_y)));
                format!(
                    "seex_padstack(\"{}\" {} {} {} {})",
                    self.name,
                    drill,
                    all_layers(&fig, true),
                    fallback_drill,
                    all_layers(&fallback_fig, false)
                )
            }
            PadstackKind::NonPlated { drill } => {
                let mask = drill + MASK_EXPANSION_MM;
                let pads = format!(
                    "list({} {})",
                    pad_call("SOLDERMASK_TOP", "'CIRCLE", mask, mask),
                    pad_call("SOLDERMASK_BOTTOM", "'CIRCLE", mask, mask)
                );
                format!(
                    "seex_padstack(\"{}\" seex_drill({} nil) {pads} seex_drill({} nil) {pads})",
                    self.name,
                    f(*drill),
                    f(*drill)
                )
            }
        }
    }
}

fn pad_call(layer: &str, figure: &str, width: f64, height: f64) -> String {
    format!("seex_pad(\"{layer}\" {figure} {} {})", f(width), f(height))
}

fn typed_pad_call(layer: &str, kind: &str, figure: &str, width: f64, height: f64) -> String {
    format!(
        "seex_padt(\"{layer}\" '{kind} {figure} {} {})",
        f(width),
        f(height)
    )
}

/// Deterministic, geometry-based padstack name so identical pads share one `.pad`.
fn padstack_name(
    figure: &str,
    width: f64,
    height: f64,
    kind: &PadstackKind,
    bottom: bool,
) -> String {
    let fig = match figure {
        "CIRCLE" => "c",
        "OBLONG" => "o",
        "SQUARE" => "q",
        _ => "r",
    };
    let size = if fig == "c" || fig == "q" {
        hundredths(width)
    } else {
        format!("{}x{}", hundredths(width), hundredths(height))
    };
    let name = match kind {
        PadstackKind::Smd => format!("sx_{}{fig}{size}", if bottom { "b" } else { "s" }),
        PadstackKind::Plated { drill_x, drill_y } => {
            let drill = if (drill_x - drill_y).abs() > 1e-6 {
                format!("{}x{}", hundredths(*drill_x), hundredths(*drill_y))
            } else {
                hundredths(*drill_x)
            };
            format!("sx_t{fig}{size}d{drill}")
        }
        PadstackKind::NonPlated { drill } => format!("sx_h{}", hundredths(*drill)),
    };
    if name.len() <= MAX_PADSTACK_NAME {
        name
    } else {
        // Keep the type prefix readable and make the rest a stable short hash.
        format!("{}{:08x}", &name[..5], fnv1a(&name))
    }
}

fn hundredths(mm: f64) -> String {
    format!("{}", (mm * 100.0).round() as i64)
}

fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Points of a KiCad custom pad primitive: `(xy 0.12 -0.34)` pairs, pad-relative mm
/// with Y down. Returned with Y up.
fn parse_kicad_polygon(text: &str) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find("(xy ") {
        rest = &rest[index + 4..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let mut numbers = rest[..end].split_whitespace().map(str::parse::<f64>);
        if let (Some(Ok(x)), Some(Ok(y))) = (numbers.next(), numbers.next()) {
            points.push((x, -y));
        }
        rest = &rest[end..];
    }
    points
}

/// Half size (mm) of a square drawing area centred on the origin that holds the
/// whole footprint with room for labels. New Allegro drawings start at (0, 0), so
/// without this every negative coordinate is "outside of the extents".
fn drawing_half_extent(footprint: &KiFootprint) -> f64 {
    let mut max_abs: f64 = 0.0;
    let mut add = |x: f64, y: f64| {
        let (x, y) = to_mm(x, y);
        max_abs = max_abs.max(x.abs()).max(y.abs());
    };
    for pad in &footprint.pads {
        let half = pad.size_x.max(pad.size_y);
        add(pad.pos_x.abs() + half, pad.pos_y.abs() + half);
    }
    for line in &footprint.lines {
        add(line.start_x, line.start_y);
        add(line.end_x, line.end_y);
    }
    for circle in &footprint.circles {
        let r = ((circle.end_x - circle.center_x).powi(2)
            + (circle.end_y - circle.center_y).powi(2))
        .sqrt();
        add(circle.center_x.abs() + r, circle.center_y.abs() + r);
    }
    for arc in &footprint.arcs {
        add(arc.start_x, arc.start_y);
        add(arc.mid_x, arc.mid_y);
        add(arc.end_x, arc.end_y);
    }
    (max_abs * 2.0 + 10.0).ceil()
}

fn place_bound(footprint: &KiFootprint) -> Option<(f64, f64, f64, f64)> {
    let mut courtyard = Vec::new();
    for line in footprint
        .lines
        .iter()
        .filter(|line| is_courtyard(&line.layer))
    {
        courtyard.push(to_mm(line.start_x, line.start_y));
        courtyard.push(to_mm(line.end_x, line.end_y));
    }
    if courtyard.len() >= 2 {
        let (x1, y1, x2, y2) = bounds(&courtyard);
        if x2 - x1 > 1e-6 && y2 - y1 > 1e-6 {
            return Some((x1, y1, x2, y2));
        }
    }

    // No usable courtyard: wrap the pads.
    let mut points = Vec::new();
    for pad in &footprint.pads {
        let (x, y) = to_mm(pad.pos_x, pad.pos_y);
        let half = px(pad.size_x.max(pad.size_y)) / 2.0;
        points.push((x - half, y - half));
        points.push((x + half, y + half));
    }
    if points.is_empty() {
        return None;
    }
    let (x1, y1, x2, y2) = bounds(&points);
    Some((
        x1 - PLACE_BOUND_CLEARANCE_MM,
        y1 - PLACE_BOUND_CLEARANCE_MM,
        x2 + PLACE_BOUND_CLEARANCE_MM,
        y2 + PLACE_BOUND_CLEARANCE_MM,
    ))
}

fn is_courtyard(layer: &str) -> bool {
    matches!(layer, "F.CrtYd" | "B.CrtYd")
}

fn graphic_layer(layer: &str) -> Option<&'static str> {
    match layer {
        "F.SilkS" => Some(SILK_TOP),
        "B.SilkS" => Some(SILK_BOTTOM),
        "F.Fab" | "Dwgs.User" => Some(ASSY_TOP),
        "B.Fab" => Some(ASSY_BOTTOM),
        _ => None,
    }
}

/// Line widths in the shared model are not consistently scaled (some sources are px,
/// some already mm); accept plausible values and fall back to library defaults.
fn line_width(layer: &str, width_px: f64) -> f64 {
    sane_width(layer, px(width_px))
}

fn line_width_mm(layer: &str, width: f64) -> f64 {
    // Circle widths are stored in mm by the converter.
    sane_width(layer, width)
}

/// Silkscreen and assembly lines are drawn at the convention widths (KLC F5.1,
/// F5.2) whatever the source used; other layers keep plausible source widths.
fn sane_width(layer: &str, width_mm: f64) -> f64 {
    if layer == SILK_TOP || layer == SILK_BOTTOM {
        SILK_LINE_MM
    } else if layer == ASSY_TOP || layer == ASSY_BOTTOM {
        ASSEMBLY_LINE_MM
    } else if (0.05..=1.0).contains(&width_mm) {
        width_mm
    } else {
        0.1
    }
}

fn to_mm(x_px: f64, y_px: f64) -> (f64, f64) {
    // Footprint model: Y down. Allegro: Y up.
    (px(x_px), -px(y_px))
}

fn px(value: f64) -> f64 {
    value * PX_TO_MM
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn bounds(points: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut out = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in points {
        out.0 = out.0.min(x);
        out.1 = out.1.min(y);
        out.2 = out.2.max(x);
        out.3 = out.3.max(y);
    }
    out
}

fn circumcircle(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> Option<((f64, f64), f64)> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a.0 * a.0 + a.1 * a.1;
    let b2 = b.0 * b.0 + b.1 * b.1;
    let c2 = c.0 * c.0 + c.1 * c.1;
    let center = (
        (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d,
        (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d,
    );
    Some((center, distance(center, a)))
}

fn arc_polyline(
    start: (f64, f64),
    end: (f64, f64),
    center: (f64, f64),
    clockwise: bool,
) -> Vec<(f64, f64)> {
    let radius = distance(center, start);
    let a0 = (start.1 - center.1).atan2(start.0 - center.0);
    let a1 = (end.1 - center.1).atan2(end.0 - center.0);
    let tau = std::f64::consts::TAU;
    let sweep = if clockwise {
        -(a0 - a1).rem_euclid(tau)
    } else {
        (a1 - a0).rem_euclid(tau)
    };
    let segments = ((sweep.abs() / tau) * 32.0).ceil().max(2.0) as usize;
    (0..=segments)
        .map(|index| {
            let angle = a0 + sweep * index as f64 / segments as f64;
            (
                center.0 + radius * angle.cos(),
                center.1 + radius * angle.sin(),
            )
        })
        .collect()
}

fn normalize_degrees(angle: f64) -> f64 {
    let normalized = angle.rem_euclid(360.0);
    if (normalized - 360.0).abs() < 1e-9 {
        0.0
    } else {
        normalized
    }
}

fn points_list(points: &[(f64, f64)]) -> String {
    let inner: Vec<String> = points
        .iter()
        .map(|(x, y)| format!("list({} {})", f(*x), f(*y)))
        .collect();
    format!("list({})", inner.join(" "))
}

/// SKILL float literal (always with a decimal point, 4 decimals).
fn f(value: f64) -> String {
    let value = if value.abs() < 5e-5 { 0.0 } else { value };
    format!("{value:.4}")
}

fn skill_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Shared SKILL helpers, embedded in every footprint script so each file is
/// self-contained. All coordinates passed to them are millimetres, Y up.
const HELPERS: &str = r#"
;; ---------------------------------------------------------------- SeEx helpers
seex_scale = 1.0
seex_errors = 0

procedure( seex_units_scale()
  let( (scale units)
    scale = car(errset(axlMKSConvert(1.0 "MM") nil))
    unless( numberp(scale)
      units = car(errset(car(axlDBGetDesignUnits()) nil))
      scale = cond(
        ( !stringp(units) 1.0 )
        ( rexMatchp("^mil" units) 1.0 / 0.0254 )
        ( rexMatchp("^inch" units) 1.0 / 25.4 )
        ( rexMatchp("^cent" units) 0.1 )
        ( rexMatchp("^micro" units) 1000.0 )
        ( t 1.0 )
      )
    )
    scale
  )
)

procedure( seex_u(mm) float(mm) * seex_scale )

procedure( seex_xy(x y) list(seex_u(x) seex_u(y)) )

procedure( seex_pts(points)
  mapcar(lambda((p) seex_xy(car(p) cadr(p))) points)
)

procedure( seex_begin(name half)
  let( (dtype)
    seex_errors = 0
    seex_notes = nil
    ;; axlDesignType(nil) is "SYMBOL" for .dra drawings and "LAYOUT" for boards.
    dtype = car(errset(axlDesignType(nil) nil))
    if( stringp(dtype) && upperCase(dtype) != "SYMBOL" then
      printf("SeEx: the open design is not a symbol drawing. Use File > New > Package symbol first.\n")
      seex_errors = 1
      nil
    else
      ;; Prefer metric with 4 decimals; harmless if the call is not available.
      errset(axlDBChangeDesignUnits("millimeters" 4) nil)
      seex_scale = seex_units_scale()
      ;; New drawings span (0 0)..(w h); centre the extents on the symbol origin.
      unless( car(errset(axlDBChangeDesignExtents(
                           list(seex_xy(-half -half) seex_xy(half half))) nil))
        printf("SeEx: WARNING could not set drawing extents to +/-%g mm\n" half)
      )
      printf("SeEx: building package symbol %s (1 mm = %g design units)\n" name seex_scale)
      t
    )
  )
)

procedure( seex_pad(layer figure w h)
  make_axlPadStackPad(?layer layer ?type 'REGULAR ?figure figure
                      ?figureSize list(seex_u(w) seex_u(h)))
)

procedure( seex_padt(layer type figure w h)
  make_axlPadStackPad(?layer layer ?type type ?figure figure
                      ?figureSize list(seex_u(w) seex_u(h)))
)

;; Review notes passed back to SeEx (written after the status line).
seex_notes = nil
procedure( seex_note(text)
  seex_notes = cons(text seex_notes)
  printf("SeEx: %s\n" text)
)

;; Read a padstack back and check that every required (layer type) pad is there.
procedure( seex_has_pad(ps layer type)
  let( (found)
    found = nil
    foreach( pad ps->pads
      when( pad->layer == layer && pad->type == type && pad->figureName != "NULL"
        found = t
      )
    )
    found
  )
)

procedure( seex_check_padstack(name required drilled)
  let( (ps missing)
    ps = car(errset(axlLoadPadstack(name) nil))
    if( !ps then
      seex_note(sprintf(nil "padstack %s could not be read back for checking" name))
    else
      missing = nil
      foreach( req required
        unless( seex_has_pad(ps car(req) cadr(req))
          missing = cons(sprintf(nil "%s %s" car(req) cadr(req)) missing)
        )
      )
      when( drilled && !(numberp(ps->drillDiameter) && ps->drillDiameter > 0)
        missing = cons("drill" missing)
      )
      when( missing
        seex_note(sprintf(nil "padstack %s is missing %s" name buildString(reverse(missing) ", ")))
      )
    )
    !missing
  )
)

procedure( seex_drill(d plated)
  make_axlPadStackDrill(?drillDiameter seex_u(d)
                        ?plating if(plated 'PLATED 'NON_PLATED)
                        ?figure 'CIRCLE
                        ?figureSize list(seex_u(d) seex_u(d)))
)

procedure( seex_slot(w h plated)
  let( (drill)
    drill = car(errset(make_axlPadStackDrill(?drillDiameter seex_u(min(w h))
                                             ?holeType 'OVAL_SLOT
                                             ?slotSize list(seex_u(w) seex_u(h))
                                             ?plating if(plated 'PLATED 'NON_PLATED)
                                             ?figure 'OBLONG
                                             ?figureSize list(seex_u(w) seex_u(h))) nil))
    unless( drill
      printf("SeEx: WARNING slot %gx%g mm not supported here; using a round drill - edit the padstack\n" w h)
      seex_errors = seex_errors + 1
      drill = seex_drill(min(w h) plated)
    )
    drill
  )
)

procedure( seex_try_padstack(name drill pads)
  let( (res)
    ;; 4th argument t skips the hole-vs-pad checks (needed for NPTH mask-only stacks).
    res = errset(axlDBCreatePadStack(name drill pads t) nil)
    when( res && car(res)
      errset(axlPadstackToDisk(name) nil)
    )
    res && car(res)
  )
)

procedure( seex_padstack(name drill pads fbDrill fbPads)
  cond(
    ( seex_try_padstack(name drill pads) t )
    ( seex_try_padstack(name fbDrill fbPads)
      seex_note(sprintf(nil "padstack %s created with simplified layers/figure (no internal pad or a round drill); review it" name))
      t )
    ( car(errset(axlLoadPadstack(name) nil))
      seex_note(sprintf(nil "padstack %s could not be created; the existing one on PADPATH is used" name))
      t )
    ( t
      printf("SeEx: ERROR could not create padstack %s\n" name)
      seex_errors = seex_errors + 1
      nil )
  )
)

procedure( seex_pin(padstack number x y rot)
  let( (orient ptext res)
    ptext = nil
    when( number
      orient = make_axlTextOrientation(?textBlock "1" ?justify "center" ?rotation 0.0)
      ptext = make_axlPinText(?offset list(0.0 0.0) ?text orient)
      ptext->number = number
    )
    res = errset(axlDBCreatePin(padstack seex_xy(x y) ptext float(rot)) t)
    unless( res && car(res)
      printf("SeEx: ERROR could not place pin %L with padstack %s\n" number padstack)
      seex_errors = seex_errors + 1
    )
  )
)

procedure( seex_line(layer width points)
  errset(axlDBCreateLine(seex_pts(points) seex_u(width) layer) t)
)

procedure( seex_circle(layer width cx cy r filled)
  if( filled then
    ;; A ring as wide as the radius renders as a solid disc.
    errset(axlDBCreateCircle(list(seex_xy(cx cy) seex_u(r / 2.0)) seex_u(r) layer) t)
  else
    errset(axlDBCreateCircle(list(seex_xy(cx cy) seex_u(r)) seex_u(width) layer) t)
  )
)

procedure( seex_arc(layer width sx sy ex ey cx cy clockwise fallback)
  let( (path res)
    res = errset(
      progn(
        path = axlPathStart(list(seex_xy(sx sy)) seex_u(width))
        path = axlPathArcCenter(path seex_u(width) seex_xy(ex ey) clockwise seex_xy(cx cy))
        axlDBCreatePath(path layer)
      ) nil)
    unless( res && car(res)
      seex_line(layer width fallback)
    )
  )
)

procedure( seex_polygon_path(points)
  let( (path)
    path = axlPathStart(list(seex_xy(car(car(points)) cadr(car(points)))) 0.0)
    foreach( p cdr(points)
      path = axlPathLine(path 0.0 seex_xy(car(p) cadr(p)))
    )
    axlPathLine(path 0.0 seex_xy(car(car(points)) cadr(car(points))))
  )
)

procedure( seex_shape(layer points)
  let( (res)
    res = errset(axlDBCreateShape(seex_polygon_path(points) t layer) nil)
    unless( res && car(res)
      printf("SeEx: could not create a filled shape on %s; drawing its outline\n" layer)
      seex_line(layer 0.0 append(points list(car(points))))
    )
  )
)

procedure( seex_place_bound(layer x1 y1 x2 y2)
  seex_shape(layer list(list(x1 y1) list(x2 y1) list(x2 y2) list(x1 y2)))
)

procedure( seex_text(layer label x y block rot)
  let( (orient)
    orient = make_axlTextOrientation(?textBlock sprintf(nil "%d" block) ?justify "center"
                                     ?rotation float(rot))
    errset(axlDBCreateText(label seex_xy(x y) orient layer) t)
  )
)

;; Size a text block of this drawing (mm): character width and height and the
;; stroke ("photo") width. Boards use their own text block table.
procedure( seex_text_block(block w h stroke)
  let( (param)
    param = car(errset(axlGetParam(sprintf(nil "paramTextBlock:%d" block)) nil))
    if( param then
      errset(param->width = seex_u(w) nil)
      errset(param->height = seex_u(h) nil)
      errset(param->photoWidth = seex_u(stroke) nil)
      errset(param->charSpace = seex_u(h * 0.2) nil)
      errset(param->lineSpace = seex_u(h * 1.6) nil)
      unless( car(errset(axlSetParam(param) nil))
        printf("SeEx: could not size text block %d\n" block)
      )
    else
      printf("SeEx: text block %d not found\n" block)
    )
  )
)

;; Map a STEP model (found through STEPPATH) as the symbol's primary 3D model,
;; rotated (degrees) and then shifted (design units). The model's folder is put
;; in front of STEPPATH for this session so the mapping finds it.
procedure( seex_step(file dir rx ry rz ox oy oz)
  let( (path ok)
    path = car(errset(axlGetVariable("steppath") nil))
    when( listp(path)
      path = buildString(path " ")
    )
    unless( stringp(path) && member(dir parseString(path " "))
      errset(axlSetVariable("steppath" if(stringp(path) && path != ""
                                          strcat(dir " " path) dir)) t)
    )
    ok = car(errset(axlStepSet('primary nil file
                               list('rotation_x rx 'rotation_y ry 'rotation_z rz
                                    'offset_x ox 'offset_y oy 'offset_z oz)) t))
    unless( ok
      printf("SeEx: could not map STEP model %s\n" file)
    )
    ok
  )
)

procedure( seex_end(name)
  errset(axlVisibleUpdate(t) nil)
  if( seex_errors == 0 then
    if( boundp('seex_library_build) && seex_library_build then
      printf("SeEx: %s drawn\n" name)
    else
      printf("SeEx: %s done. File > Save to write %s.dra, File > Create Symbol for %s.psm.\n" name name name)
    )
  else
    printf("SeEx: %s finished with %d problem(s); see messages above before saving.\n" name seex_errors)
  )
  t
)

;; Save the drawing and compile the .psm; "OK" or the reason it failed.
procedure( seex_finish(name)
  let( (saved compiled)
    saved = car(errset(axlSaveDesign(?design name ?noMru t ?noConfirm t) t))
    compiled = car(errset(axlCompileSymbol(?symbol name ?silent t ?silentOnOverwrite t) t))
    cond(
      ( !saved "save failed" )
      ( !compiled "create symbol failed" )
      ( seex_errors > 0 sprintf(nil "%d problem(s) while building" seex_errors) )
      ( t "OK" )
    )
  )
)
;; ------------------------------------------------------------ end of helpers
"#;

/// The library script: loaded in Allegro PCB Editor, it builds every footprint
/// script in `dir` into `.dra`, `.psm` and `.pad` files in the same folder.
///
/// Each footprint gets a new package symbol drawing (`axlOpenDesign` creates one
/// for a `.dra` name that does not exist yet; an older build is replaced). The
/// folder is the one the export wrote to; when the library has been moved, the
/// current working directory is used instead.
pub fn library_script(library: &str, dir: &std::path::Path, footprints: &[String]) -> String {
    let dir = skill_string(&dir.to_string_lossy().replace('\\', "/"));
    let names: Vec<String> = footprints
        .iter()
        .map(|name| format!("\"{}\"", skill_string(name)))
        .collect();
    let mut script = String::new();
    let _ = writeln!(
        script,
        ";; {library}.il - builds the Allegro footprints of the SeEx library \"{library}\"\n\
         ;;\n\
         ;; In Allegro PCB Editor (with any design open), type in the command window:\n\
         ;;     skill load(\"{dir}/{library}.il\")\n\
         ;; Every footprint script in this folder becomes <name>.dra, <name>.psm and its\n\
         ;; .pad padstacks here. Then add this folder to PSMPATH and PADPATH, and the\n\
         ;; 3D model folder to STEPPATH (Setup > User Preferences > Paths > Library).\n\
         ;; Save your open design first: Allegro opens a new drawing for each footprint.\n"
    );
    let _ = writeln!(
        script,
        "procedure( seex_build_library(dir names)
  let( (home done failed status design)
    unless( isDir(dir)
      dir = getWorkingDir()
    )
    home = getWorkingDir()
    changeWorkingDir(dir)
    seex_library_build = t
    done = 0
    failed = nil
    foreach( name names
      design = strcat(dir \"/\" name \".dra\")
      when( isFile(design)
        deleteFile(design)
      )
      status = cond(
        ( !isFile(strcat(dir \"/\" name \".il\")) \"script missing\" )
        ( !car(errset(axlOpenDesign(?design design ?mode \"wf\") t)) \"could not open a new drawing\" )
        ( !car(errset(load(strcat(dir \"/\" name \".il\")) t)) \"script failed\" )
        ( t seex_finish(name) )
      )
      if( status == \"OK\" then
        done = done + 1
      else
        failed = cons(sprintf(nil \"%s (%s)\" name status) failed)
      )
    )
    seex_library_build = nil
    changeWorkingDir(home)
    printf(\"SeEx: built %d of %d footprints in %s\\n\" done length(names) dir)
    foreach( entry reverse(failed)
      printf(\"SeEx: FAILED %s\\n\" entry)
    )
    done
  )
)
"
    );
    let _ = writeln!(
        script,
        "seex_build_library(\"{dir}\" list({}))",
        names.join(" ")
    );
    script
}

/// Text for a SKILL string literal.
fn skill_string(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}
