//! Placement of the STEP models that ship with an EasyEDA footprint.
//!
//! LCEDA authors every STEP model in its own local frame, and that frame does not always match the
//! footprint it belongs to. Two things are wrong in the models we have seen:
//!
//! * the underside is not on the board plane, so the part sinks into the board or floats above it.
//!   `C2904739` (SOP-16) and `C9864` (ESOP-8) are centred on Z and sink ~0.8 mm;
//! * the bounding box is not centred on the model's own origin, so the part misses its pads
//!   sideways. `C347421` (TO-263-5) is drawn from its lead tips, which leaves it 7 mm off the pad
//!   pattern, and its leads never reach the copper.
//!
//! Most parts (for example `C128601`, `C190794`, `C668205`) are already correct and are returned
//! untouched.
//!
//! Neither target format can be trusted to fix this on its own: KiCad and Altium each have a model
//! offset field, but they disagree about its sign, and an offset only applies where the consumer
//! keeps the STEP as a separate model reference. A STEP file has no origin to move, so the
//! correction is applied to the geometry itself: the part is centred on its origin on X and Y and
//! its underside is moved onto Z = 0. Embedded bodies, exported `.step` files and both CAD tools
//! then agree without depending on any tool's own convention.
//!
//! Both measurements are robust by construction: the underside is a low percentile of the vertex
//! heights and the centre is computed from percentile trimmed extents, so stray solids and thin
//! fins (LCEDA models carry a few) cannot drag either one. `C347421` hides a solid 12.9 mm below
//! the package, `C37593` (MSOP-10) has a 10 vertex fin 0.55 mm below its lead feet, and `C9864` has
//! an appendage 2.5 mm past the end of the body; none of them may decide the placement.
//!
//! Call [`place_step_on_board`] before writing or embedding a downloaded STEP model.
//!
//! Before that, every target hands the model to a [`ModelCleanup`], which takes out the logo
//! EasyEDA puts on many of its models (see `crate::step_logo`) and keeps count of what it did,
//! so the export can say which models still carry the logo.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};
use std::ops::Range;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::step_logo::{LogoOutcome, strip_logo};

/// Share of the lowest vertices ignored when reading the underside of a part.
const UNDERSIDE_TRIM: f64 = 0.02;
/// Share of the outermost vertices ignored on each side when measuring the body extents.
const EXTENT_TRIM: f64 = 0.02;
/// Smaller corrections are measurement noise rather than a misplaced model.
const MIN_LIFT_MM: f64 = 0.05;
/// Larger corrections come from stray geometry (a hidden solid far below the part), not placement.
const MAX_LIFT_MM: f64 = 2.0;
/// Sideways corrections below this are within the noise of the two bounding boxes.
const MIN_CENTRE_OFFSET_MM: f64 = 0.5;
/// Below this vertex count there is not enough geometry to measure anything.
const MIN_VERTICES: usize = 16;

const CARTESIAN_POINT: &[u8] = b"CARTESIAN_POINT";

#[derive(Debug, Clone)]
struct ModelPoint {
    coordinates: [f64; 3],
    /// Spans of the X, Y and Z literals in the STEP source, used to rewrite them.
    spans: [Range<usize>; 3],
}

impl ModelPoint {
    fn z(&self) -> f64 {
        self.coordinates[2]
    }
}

/// Move a downloaded STEP model onto the board plane and centre it on its own origin.
///
/// Returns the input unchanged when the model is already correct, when it carries too little
/// geometry to measure, or when a measured correction is implausible (see the constants above).
/// Most models need no correction, so the result borrows the input rather than copying a
/// model-sized buffer for every part in a batch.
pub fn place_step_on_board(step_data: &[u8]) -> Cow<'_, [u8]> {
    let Some((points, shift)) = placement_for(step_data) else {
        return Cow::Borrowed(step_data);
    };
    let mut placed = Vec::with_capacity(step_data.len());
    write_shifted(step_data, &points, shift, &mut placed).expect("writing into memory cannot fail");
    Cow::Owned(placed)
}

/// Writes the model with its placement applied.
///
/// A batch can hand the corrected model straight to a file instead of building the
/// whole corrected text in memory first, which is another model-sized buffer per part
/// in flight.
pub fn write_placed_step<W: Write>(step_data: &[u8], writer: &mut W) -> io::Result<()> {
    match placement_for(step_data) {
        Some((points, shift)) => write_shifted(step_data, &points, shift, writer),
        None => writer.write_all(step_data),
    }
}

/// The correction a model needs, or `None` when it is already correct.
fn placement_for(step_data: &[u8]) -> Option<(Vec<ModelPoint>, [f64; 3])> {
    let points = parse_model_points(step_data);
    if points.len() < MIN_VERTICES {
        return None;
    }

    let lift = underside_lift_mm(&points);
    let [shift_x, shift_y] = centring_offset_mm(&points);
    if lift == 0.0 && shift_x == 0.0 && shift_y == 0.0 {
        return None;
    }

    Some((points, [shift_x, shift_y, lift]))
}

fn write_shifted<W: Write>(
    step_data: &[u8],
    points: &[ModelPoint],
    shift: [f64; 3],
    writer: &mut W,
) -> io::Result<()> {
    let mut cursor = 0usize;
    for point in points {
        for axis in 0..3 {
            let span = &point.spans[axis];
            writer.write_all(&step_data[cursor..span.start])?;
            writer.write_all(format_step_real(point.coordinates[axis] + shift[axis]).as_bytes())?;
            cursor = span.end;
        }
    }
    writer.write_all(&step_data[cursor..])
}

/// How far a model has to move up to rest on the board plane, in millimetres.
fn underside_lift_mm(points: &[ModelPoint]) -> f64 {
    if points.len() < MIN_VERTICES {
        return 0.0;
    }

    let mut heights = points.iter().map(ModelPoint::z).collect::<Vec<_>>();
    let underside = percentile(&mut heights, UNDERSIDE_TRIM);
    let lift = round_micrometres(-underside);

    if (MIN_LIFT_MM..=MAX_LIFT_MM).contains(&lift) {
        lift
    } else {
        0.0
    }
}

/// Sideways offset that centres the part on the origin of its own frame, in millimetres.
fn centring_offset_mm(points: &[ModelPoint]) -> [f64; 2] {
    // Geometry far below the part is a stray solid, and it must not drag the centre with it.
    let body = points
        .iter()
        .filter(|point| point.z() >= -MAX_LIFT_MM)
        .collect::<Vec<_>>();
    if body.len() < MIN_VERTICES {
        return [0.0, 0.0];
    }

    let mut offsets = [0.0; 2];
    for axis in 0..2 {
        let mut coordinates = body
            .iter()
            .map(|point| point.coordinates[axis])
            .collect::<Vec<_>>();
        let low = percentile(&mut coordinates, EXTENT_TRIM);
        let high = percentile(&mut coordinates, 1.0 - EXTENT_TRIM);
        offsets[axis] = centring_axis((low + high) / 2.0);
    }

    offsets
}

fn centring_axis(centre: f64) -> f64 {
    let offset = round_micrometres(-centre);

    if offset.abs() >= MIN_CENTRE_OFFSET_MM {
        offset
    } else {
        0.0
    }
}

/// Value at `fraction` of the sorted sample, using the nearest rank.
fn percentile(values: &mut [f64], fraction: f64) -> f64 {
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let last = values.len() - 1;
    let index = (fraction * last as f64).round().clamp(0.0, last as f64) as usize;
    values[index]
}

fn round_micrometres(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// Every `CARTESIAN_POINT` with three coordinates, in file order.
fn parse_model_points(step_data: &[u8]) -> Vec<ModelPoint> {
    let mut points = Vec::new();
    let mut search_from = 0usize;

    while let Some(found) = find_from(step_data, CARTESIAN_POINT, search_from) {
        search_from = found + CARTESIAN_POINT.len();
        let Some(coordinates) = cartesian_point_coordinates(step_data, search_from) else {
            continue;
        };
        let values = coordinate_values(step_data, coordinates);
        if values.len() != 3 {
            // Parameter-space points carry two coordinates and are not part of the 3D body.
            continue;
        }

        points.push(ModelPoint {
            coordinates: [values[0].0, values[1].0, values[2].0],
            spans: [
                values[0].1.clone(),
                values[1].1.clone(),
                values[2].1.clone(),
            ],
        });
    }

    points
}

/// Span of the `( x, y, z )` group that follows a `CARTESIAN_POINT` label.
fn cartesian_point_coordinates(step_data: &[u8], from: usize) -> Option<Range<usize>> {
    // The record reads `CARTESIAN_POINT ( 'NONE', ( x, y, z ) )`: the first parenthesis opens the
    // parameter list, the second one the coordinates.
    let mut cursor = from;
    for _ in 0..2 {
        let open = step_data[cursor..].iter().position(|byte| *byte == b'(')?;
        cursor += open + 1;
    }
    let close = step_data[cursor..].iter().position(|byte| *byte == b')')?;
    Some(cursor..cursor + close)
}

/// Values and absolute spans of the comma separated coordinates in a point group.
fn coordinate_values(step_data: &[u8], span: Range<usize>) -> Vec<(f64, Range<usize>)> {
    let mut values = Vec::new();
    let mut start = span.start;

    for index in span.clone() {
        if step_data[index] == b',' {
            push_coordinate(step_data, start..index, &mut values);
            start = index + 1;
        }
    }
    push_coordinate(step_data, start..span.end, &mut values);

    values
}

fn push_coordinate(step_data: &[u8], span: Range<usize>, values: &mut Vec<(f64, Range<usize>)>) {
    let text = &step_data[span.clone()];
    let leading = text
        .iter()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    let trailing = text
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_whitespace())
        .count();
    if leading + trailing >= text.len() {
        return;
    }

    let trimmed = span.start + leading..span.end - trailing;
    let Some(value) = std::str::from_utf8(&step_data[trimmed.clone()])
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
    else {
        return;
    };

    values.push((value, trimmed));
}

/// STEP requires a decimal point in every real; a fixed nine decimals is both valid and plenty.
fn format_step_real(value: f64) -> String {
    format!("{value:.9}")
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// What happens to the downloaded models of one export before they are written or embedded.
///
/// EasyEDA puts its logo on many of its STEP models. Removing it is on by default for every
/// target: the model is rebuilt without the logo. When that cannot be done without breaking the
/// model, the original model is exported and its part is remembered, so the run can tell the user
/// exactly which models still show the logo.
pub struct ModelCleanup {
    remove_logo: bool,
    removed: AtomicUsize,
    with_logo: Mutex<Vec<String>>,
}

impl Default for ModelCleanup {
    fn default() -> Self {
        Self::new(true)
    }
}

impl fmt::Debug for ModelCleanup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelCleanup")
            .field("remove_logo", &self.remove_logo)
            .field("removed", &self.removed())
            .field("with_logo", &self.parts_with_logo())
            .finish()
    }
}

impl ModelCleanup {
    pub fn new(remove_logo: bool) -> Self {
        Self {
            remove_logo,
            removed: AtomicUsize::new(0),
            with_logo: Mutex::new(Vec::new()),
        }
    }

    /// The model of `part` as it should be exported: without the logo when that is asked for and
    /// can be done, otherwise unchanged.
    pub fn clean<'a>(&self, part: &str, step_data: &'a [u8]) -> Cow<'a, [u8]> {
        if !self.remove_logo {
            return Cow::Borrowed(step_data);
        }
        match strip_logo(step_data) {
            (Some(model), LogoOutcome::Removed(removal)) => {
                log::debug!(
                    "{part}: removed the 3D model logo ({} pieces, {} faces)",
                    removal.pieces,
                    removal.faces
                );
                self.removed.fetch_add(1, Ordering::Relaxed);
                Cow::Owned(model)
            }
            (_, outcome) => {
                if outcome.logo_remains() {
                    log::warn!("{part}: the 3D model logo could not be removed ({outcome:?})");
                    if let Ok(mut parts) = self.with_logo.lock()
                        && !parts.iter().any(|known| known == part)
                    {
                        parts.push(part.to_string());
                    }
                }
                Cow::Borrowed(step_data)
            }
        }
    }

    /// Models rebuilt without the logo so far.
    pub fn removed(&self) -> usize {
        self.removed.load(Ordering::Relaxed)
    }

    /// Parts whose exported model still carries the logo, sorted.
    pub fn parts_with_logo(&self) -> Vec<String> {
        let mut parts = self
            .with_logo
            .lock()
            .map(|parts| parts.clone())
            .unwrap_or_default();
        parts.sort();
        parts
    }

    /// Lines for the export summary.
    pub fn summary_lines(&self) -> Vec<String> {
        if !self.remove_logo {
            return vec!["3D logo removal: OFF (models keep the EasyEDA logo)".to_string()];
        }
        let with_logo = self.parts_with_logo();
        let mut lines = vec![format!(
            "3D logo removal: ON | removed {} | kept with logo {}",
            self.removed(),
            with_logo.len()
        )];
        if !with_logo.is_empty() {
            lines.push(format!("{LOGO_KEPT_PREFIX} {}", with_logo.join(", ")));
        }
        lines
    }
}

/// Starts the summary line that names the parts whose model still carries the logo.
pub const LOGO_KEPT_PREFIX: &str = "Models with logo:";

#[cfg(test)]
mod tests {
    use super::{
        LOGO_KEPT_PREFIX, ModelCleanup, parse_model_points, place_step_on_board, underside_lift_mm,
    };
    use std::borrow::Cow;

    #[test]
    fn logo_removal_reports_the_models_that_keep_their_logo() {
        let plain = step_with(&[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]);
        let unreadable = b"not a STEP file".to_vec();

        let models = ModelCleanup::default();
        assert!(matches!(models.clean("C1", &plain), Cow::Borrowed(_)));
        assert!(matches!(models.clean("C2", &unreadable), Cow::Borrowed(_)));
        assert!(matches!(models.clean("C2", &unreadable), Cow::Borrowed(_)));
        assert_eq!(models.removed(), 0);
        assert_eq!(models.parts_with_logo(), ["C2"]);
        assert_eq!(
            models.summary_lines(),
            [
                "3D logo removal: ON | removed 0 | kept with logo 1".to_string(),
                format!("{LOGO_KEPT_PREFIX} C2"),
            ]
        );

        let off = ModelCleanup::new(false);
        assert!(matches!(off.clean("C2", &unreadable), Cow::Borrowed(_)));
        assert!(off.parts_with_logo().is_empty());
        assert!(off.summary_lines()[0].contains("OFF"));
    }

    fn step_with(points: &[[f64; 3]]) -> Vec<u8> {
        let mut text = String::from("ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n");
        for (index, [x, y, z]) in points.iter().enumerate() {
            text.push_str(&format!(
                "#{index} = CARTESIAN_POINT ( 'NONE',  ( {x}, {y}, {z} ) ) ;\n"
            ));
        }
        text.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        text.into_bytes()
    }

    /// A part lying flat on the board: `size` wide, `thickness` tall, centred on the origin.
    fn flat_part(count: usize, size: f64, thickness: f64) -> Vec<[f64; 3]> {
        (0..count)
            .map(|index| {
                let t = index as f64 / count as f64;
                let x = -size / 2.0 + size * t;
                let y = -size / 2.0 + size * (t * 3.0).fract();
                [x, y, thickness * (t * 7.0).fract()]
            })
            .collect()
    }

    fn placed_points(step_data: &[u8]) -> Vec<[f64; 3]> {
        parse_model_points(step_data)
            .into_iter()
            .map(|point| point.coordinates)
            .collect()
    }

    fn lift_of(points: &[[f64; 3]]) -> f64 {
        let step_data = step_with(points);
        underside_lift_mm(&parse_model_points(&step_data))
    }

    /// Trimmed centre of one axis, i.e. what the placement is based on.
    fn trimmed_centre(points: &[[f64; 3]], axis: usize) -> f64 {
        let mut values = points.iter().map(|point| point[axis]).collect::<Vec<_>>();
        values.sort_by(|left, right| left.partial_cmp(right).unwrap());
        let last = values.len() - 1;
        let low = values[(0.02 * last as f64).round() as usize];
        let high = values[(0.98 * last as f64).round() as usize];
        (low + high) / 2.0
    }

    #[test]
    fn leaves_a_model_that_already_rests_on_the_board_untouched() {
        let points = flat_part(120, 5.0, 1.5);
        let step_data = step_with(&points);

        assert_eq!(lift_of(&points), 0.0);
        assert!(
            place_step_on_board(&step_data) == step_data,
            "a correct model must be returned byte for byte"
        );
    }

    #[test]
    fn lifts_a_model_whose_underside_sits_below_the_board() {
        // A flat underside 0.8 mm below the origin, like the centred LCEDA models.
        let mut points = vec![[-1.0, -1.0, -0.8]; 60];
        points.extend(flat_part(120, 3.0, 1.4));
        let step_data = step_with(&points);

        let shifted = placed_points(&place_step_on_board(&step_data));

        assert!((lift_of(&points) - 0.8).abs() < 1e-6);
        assert!(shifted[0][2] >= -0.001, "underside still sunk");
        assert!((shifted[60][2] - (points[60][2] + 0.8)).abs() < 0.001);
        assert_eq!(shifted.len(), points.len());
    }

    #[test]
    fn measures_the_underside_from_the_bulk_of_the_model() {
        // Two lone vertices 0.07 mm below the real underside must not drive the lift.
        let mut points = vec![[0.0, 0.0, -0.932]; 2];
        points.extend((0..144).map(|index| [0.0, 0.0, -0.86 + index as f64 * 1e-6]));
        points.extend(flat_part(500, 4.0, 1.0));

        assert!((lift_of(&points) - 0.86).abs() < 1e-3);
    }

    #[test]
    fn ignores_a_thin_fin_below_the_lead_feet() {
        // C37593 (MSOP-10): ten vertices form a fin 0.55 mm below the feet, which rest on Z = 0.
        let mut points = (0..10)
            .map(|index| [0.0, -2.5 + index as f64 * 0.55, -0.55])
            .collect::<Vec<_>>();
        points.push([-1.5, 0.0, 0.0]);
        points.extend(flat_part(103, 1.5, 0.0));
        points.extend(flat_part(600, 3.0, 1.1));

        assert_eq!(lift_of(&points), 0.0);
    }

    #[test]
    fn ignores_a_stray_solid_far_below_the_part() {
        let mut points = flat_part(400, 5.0, 1.0);
        points.extend((0..22).map(|_| [0.0, 0.0, -12.924]));

        assert_eq!(lift_of(&points), 0.0);
    }

    #[test]
    fn ignores_corrections_below_the_measurement_threshold() {
        let mut points = vec![[0.0, 0.0, -0.01]; 40];
        points.extend(flat_part(60, 3.0, 0.5));

        assert_eq!(lift_of(&points), 0.0);
    }

    #[test]
    fn centres_a_model_drawn_from_one_of_its_corners() {
        // C347421 (TO-263-5) starts at its lead tips: x [-5.8, 5.8], y [0, 14.2].
        let points = (0..400)
            .map(|index| {
                let t = index as f64 / 399.0;
                [
                    -5.8 + 11.6 * t,
                    14.2 * (t * 5.0).fract(),
                    4.5 * (t * 3.0).fract(),
                ]
            })
            .collect::<Vec<_>>();
        let step_data = step_with(&points);

        let shifted = placed_points(&place_step_on_board(&step_data));

        assert!(
            trimmed_centre(&points, 1) > 6.9,
            "test data must start off centre"
        );
        assert!(
            trimmed_centre(&shifted, 0).abs() < 0.02,
            "x moved although it was centred: {}",
            trimmed_centre(&shifted, 0)
        );
        assert!(
            trimmed_centre(&shifted, 1).abs() < 0.02,
            "model still off centre: {}",
            trimmed_centre(&shifted, 1)
        );
    }

    #[test]
    fn ignores_a_sparse_appendage_when_centring() {
        // C9864 (ESOP-8) carries an appendage 2.5 mm past the end of its body: 30 of its 3373
        // vertices.
        let mut points = flat_part(3000, 4.8, 1.5);
        points.extend((0..30).map(|index| [0.0, 2.4 + index as f64 * 0.08, 0.5]));
        let step_data = step_with(&points);

        assert!(
            place_step_on_board(&step_data) == step_data,
            "a sparse appendage must not move the model"
        );
    }

    #[test]
    fn ignores_models_without_enough_geometry() {
        assert_eq!(lift_of(&[[0.0, 0.0, -0.5]; 4]), 0.0);

        let step_data = b"ISO-10303-21;\nENDSEC;\n".to_vec();
        assert!(place_step_on_board(&step_data) == step_data);
    }

    #[test]
    fn only_rewrites_three_dimensional_points() {
        let mut points = flat_part(200, 2.0, 1.0);
        points.extend((0..20).map(|_| [0.0, 0.0, -0.83]));
        let step_data = step_with(&points);

        let shifted = placed_points(&place_step_on_board(&step_data));

        assert_eq!(shifted.len(), points.len());
        assert!(
            shifted[200][2].abs() < 1e-6,
            "the sunken vertices were not lifted: {}",
            shifted[200][2]
        );

        let two_dimensional = b"CARTESIAN_POINT ( 'NONE',  ( 0.5, 0.5 ) ) ;".to_vec();
        let mut mixed = step_data;
        mixed.extend_from_slice(&two_dimensional);
        let placed = place_step_on_board(&mixed);
        assert!(
            placed.ends_with(&two_dimensional),
            "a parameter space point must be left alone"
        );
    }

    #[test]
    fn keeps_the_vertex_count_and_the_entity_layout() {
        let mut points = vec![[0.0, 0.0, -0.62]; 100];
        points.extend(flat_part(150, 4.0, 1.2));
        let step_data = step_with(&points);

        let shifted = place_step_on_board(&step_data);

        assert_eq!(
            shifted
                .windows(b"CARTESIAN_POINT".len())
                .filter(|window| *window == b"CARTESIAN_POINT")
                .count(),
            points.len()
        );
        assert!(shifted.ends_with(b"ENDSEC;\nEND-ISO-10303-21;\n"));
    }

    #[test]
    fn parses_points_with_tight_spacing_and_exponents() {
        let points = parse_model_points(
            b"#7=CARTESIAN_POINT('NONE',(1.5E-01,-2,3.25E+00));#8=CARTESIAN_POINT('NONE',(0,0,-8.6E-01));",
        );

        assert_eq!(points.len(), 2);
        assert_eq!(points[0].coordinates, [0.15, -2.0, 3.25]);
        assert_eq!(points[1].coordinates, [0.0, 0.0, -0.86]);
    }

    #[test]
    fn rewrites_the_three_coordinate_literals() {
        let step_data = b"#1 = CARTESIAN_POINT ( 'NONE',  ( 1.25, -3.5, -0.9 ) ) ;".to_vec();
        let points = parse_model_points(&step_data);

        assert_eq!(points.len(), 1);
        assert_eq!(&step_data[points[0].spans[0].clone()], b"1.25");
        assert_eq!(&step_data[points[0].spans[1].clone()], b"-3.5");
        assert_eq!(&step_data[points[0].spans[2].clone()], b"-0.9");
    }
}
