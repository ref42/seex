//! Placing EasyEDA STEP models on Allegro footprints.
//!
//! EasyEDA keeps a model in its own coordinates and records where it sits on the
//! footprint: the model's centre (`c_origin`), its height above the board (`z`) and a
//! rotation (`c_rotation`). EasyEDA centres the model's outline on `c_origin` and
//! rests its bottom on the board, so the Allegro mapping is: rotate by `c_rotation`,
//! then shift the rotated centre onto `c_origin` and the bottom onto `z`.
//!
//! The outline is the model's real extent: round bodies (capacitors, buzzers, LED
//! domes, coax connectors) are bounded by circles and surfaces whose extremes are
//! not vertices, so the outline comes from the tessellated surfaces, not from the
//! vertex list. It is taken after the rotation, tilted models included.

use crate::cadence::allegro::StepTransform;
use crate::easyeda::{ComponentData, ModelPlacement};
use crate::kicad::KiFootprint;

/// EasyEDA canvas unit (10 mil) in millimetres.
const PX_MM: f64 = 0.254;

/// Points that bound a STEP model, in the file's units (mm for EasyEDA models):
/// its tessellated surfaces, or its vertices when it cannot be tessellated.
pub fn model_points(data: &[u8]) -> Option<Vec<[f64; 3]>> {
    if let Some(mesh) = crate::cadence::step_mesh::tessellate(data) {
        if !mesh.positions.is_empty() {
            return Some(
                mesh.positions
                    .iter()
                    .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
                    .collect(),
            );
        }
    }
    let (min, max) = vertex_bounds(data)?;
    Some(
        (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                ]
            })
            .collect(),
    )
}

/// Bounding box (min, max) of a STEP model's real extent ([`model_points`]).
#[cfg(test)]
pub fn step_bounds(data: &[u8]) -> Option<([f64; 3], [f64; 3])> {
    bounds_of(&model_points(data)?, [0.0; 3])
}

/// `point` turned by `rotation` (degrees about x, then y, then z), the way
/// Allegro and the preview apply a STEP mapping.
pub fn rotate(point: [f64; 3], rotation: [f64; 3]) -> [f64; 3] {
    let [rx, ry, rz] = rotation.map(f64::to_radians);
    let [x, y, z] = point;
    let (sx, cx) = rx.sin_cos();
    let (sy, cy) = ry.sin_cos();
    let (sz, cz) = rz.sin_cos();
    let (y, z) = (y * cx - z * sx, y * sx + z * cx);
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    let (x, y) = (x * cz - y * sz, x * sz + y * cz);
    [x, y, z]
}

/// Bounding box of `points` turned by `rotation`.
pub fn bounds_of(points: &[[f64; 3]], rotation: [f64; 3]) -> Option<([f64; 3], [f64; 3])> {
    if points.is_empty() {
        return None;
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &point in points {
        let point = rotate(point, rotation);
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    Some((min, max))
}

/// Bounding box of a STEP model's vertices. Falls back to all points when the
/// file has no vertex list.
fn vertex_bounds(data: &[u8]) -> Option<([f64; 3], [f64; 3])> {
    let text = String::from_utf8_lossy(data);
    let mut points: std::collections::HashMap<&str, [f64; 3]> = std::collections::HashMap::new();
    let mut vertices: Vec<&str> = Vec::new();
    for statement in text.split(';') {
        let statement = statement.trim();
        let Some(rest) = statement.strip_prefix('#') else {
            continue;
        };
        let Some((id, body)) = rest.split_once('=') else {
            continue;
        };
        let id = id.trim();
        let body = body.trim();
        if let Some(args) = body.strip_prefix("CARTESIAN_POINT") {
            if let Some(point) = parse_point(args) {
                points.insert(id, point);
            }
        } else if let Some(args) = body.strip_prefix("VERTEX_POINT") {
            if let Some(reference) = args.rsplit('#').next() {
                let reference = reference.trim_end_matches(|c: char| c == ')' || c.is_whitespace());
                vertices.push(reference);
            }
        }
    }
    let selected: Vec<[f64; 3]> = if vertices.is_empty() {
        points.values().copied().collect()
    } else {
        vertices
            .iter()
            .filter_map(|id| points.get(id).copied())
            .collect()
    };
    if selected.is_empty() {
        return None;
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for point in selected {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    Some((min, max))
}

fn parse_point(args: &str) -> Option<[f64; 3]> {
    let start = args.rfind('(')?;
    let end = args[start..].find(')')? + start;
    let values: Vec<f64> = args[start + 1..end]
        .split(',')
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .collect();
    match values.as_slice() {
        [x, y, z] => Some([*x, *y, *z]),
        [x, y] => Some([*x, *y, 0.0]),
        _ => None,
    }
}

/// Allegro STEP mapping for an EasyEDA model on the footprint whose origin is
/// `package_origin` (EasyEDA canvas units). `points` bound the model
/// ([`model_points`]).
pub fn allegro_transform(
    placement: Option<&ModelPlacement>,
    package_origin: (f64, f64),
    points: Option<&[[f64; 3]]>,
) -> StepTransform {
    let (target_x, target_y, bottom, rotation) = match placement {
        Some(p) => (
            (p.origin_x - package_origin.0) * PX_MM,
            // EasyEDA's y axis points down, Allegro's up.
            -(p.origin_y - package_origin.1) * PX_MM,
            p.z * PX_MM,
            p.rotation.map(normalize_degrees),
        ),
        None => (0.0, 0.0, 0.0, [0.0; 3]),
    };
    // The turned model's outline: its centre goes onto c_origin, its bottom onto z.
    let offset = match points.and_then(|points| bounds_of(points, rotation)) {
        Some((min, max)) => {
            let (cx, cy) = ((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
            [target_x - cx, target_y - cy, bottom - min[2]]
        }
        None => [target_x, target_y, bottom],
    };
    StepTransform {
        rotation,
        offset: offset.map(|v| round4(v) + 0.0),
    }
}

/// Where EasyEDA puts the model centre on the footprint (mm, Allegro axes).
pub fn model_centre_mm(placement: &ModelPlacement, package_origin: (f64, f64)) -> (f64, f64) {
    (
        (placement.origin_x - package_origin.0) * PX_MM,
        -(placement.origin_y - package_origin.1) * PX_MM,
    )
}

fn normalize_degrees(angle: f64) -> f64 {
    let a = angle.rem_euclid(360.0);
    if (a - 360.0).abs() < 1e-9 {
        0.0
    } else {
        round4(a)
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Allegro mapping of EasyEDA's model on the exported footprint: EasyEDA's
/// placement on its own footprint, turned and shifted with the footprint when it
/// was redrawn as an IPC-7352 land pattern (`ipc`: turn in degrees and shift in mm,
/// `ipc = R(turn) * easyeda + shift`). `data` is the model the export writes.
pub fn model_on_footprint(
    component: &ComponentData,
    easyeda_footprint: &KiFootprint,
    ipc_alignment: Option<(f64, (f64, f64))>,
    data: Option<&[u8]>,
    notes: &mut Vec<String>,
) -> StepTransform {
    let Some((turn, (sx, sy))) = ipc_alignment else {
        return easyeda_model_transform(component, easyeda_footprint, data, notes);
    };
    // The calculated pattern is centred on the package, so the model's outline is
    // centred on the origin; EasyEDA only decides how it is turned and its height.
    // EasyEDA's notes about where it put the model do not apply here.
    let mut easyeda_notes = Vec::new();
    let points = data.and_then(model_points);
    let transform = easyeda_model_transform_with(
        component,
        easyeda_footprint,
        points.as_deref(),
        &mut easyeda_notes,
    );
    if transform.rotation[0] == 0.0 && transform.rotation[1] == 0.0 {
        let rz = (transform.rotation[2] + turn).rem_euclid(360.0);
        if let Some((min, max)) = points.as_deref().and_then(|p| bounds_of(p, [0.0, 0.0, rz])) {
            let round = |v: f64| (v * 10_000.0).round() / 10_000.0 + 0.0;
            return StepTransform {
                rotation: [0.0, 0.0, rz],
                offset: [
                    round(-(min[0] + max[0]) / 2.0),
                    round(-(min[1] + max[1]) / 2.0),
                    transform.offset[2],
                ],
            };
        }
    }
    notes.extend(easyeda_notes);
    if transform.rotation[0] != 0.0 || transform.rotation[1] != 0.0 {
        notes.push("3D: EasyEDA tilts this model; check it in the 3D view".to_string());
    }
    let (sin, cos) = turn.to_radians().sin_cos();
    let [ox, oy, oz] = transform.offset;
    let round = |v: f64| (v * 10_000.0).round() / 10_000.0 + 0.0;
    StepTransform {
        rotation: [
            transform.rotation[0],
            transform.rotation[1],
            (transform.rotation[2] + turn).rem_euclid(360.0),
        ],
        offset: [
            round(ox * cos - oy * sin + sx),
            round(ox * sin + oy * cos + sy),
            oz,
        ],
    }
}

/// Allegro mapping for EasyEDA's own model (`data` is the STEP file the export
/// writes), with the same corrections the export applies.
fn easyeda_model_transform(
    component: &ComponentData,
    ki_footprint: &KiFootprint,
    data: Option<&[u8]>,
    notes: &mut Vec<String>,
) -> StepTransform {
    let points = data.and_then(model_points);
    easyeda_model_transform_with(component, ki_footprint, points.as_deref(), notes)
}

/// [`easyeda_model_transform`] for a model whose outline points are known.
fn easyeda_model_transform_with(
    component: &ComponentData,
    ki_footprint: &KiFootprint,
    points: Option<&[[f64; 3]]>,
    notes: &mut Vec<String>,
) -> StepTransform {
    if points.is_none() {
        notes.push("3D: could not read the model outline; placed at the origin".to_string());
    }
    let package_origin = (component.package_bbox_x, component.package_bbox_y);
    let mut placement = component.model_3d.as_ref().and_then(|m| m.placement);
    // EasyEDA data sometimes puts a model far beside its pads (seen 1.1 m off):
    // a model that does not even reach its pads is plainly misplaced, so it
    // starts from the pad centre. A model that overlaps them stays where EasyEDA
    // put it, however far its centre is: bodies are often not centred on their
    // pads (a battery holder, a right-angle BNC whose barrel runs 24 mm past
    // its pins, C709676), and the model's own leads are checked below.
    if let (Some(p), Some(pads)) = (placement.as_mut(), pad_centre_mm(ki_footprint)) {
        let centre = model_centre_mm(p, package_origin);
        let off = ((centre.0 - pads.0).powi(2) + (centre.1 - pads.1).powi(2)).sqrt();
        let extent = pad_extent_mm(ki_footprint);
        let half = points
            .and_then(|points| bounds_of(points, p.rotation))
            .map(|(lo, hi)| ((hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0));
        let misplaced = match (extent, half) {
            (Some((x0, y0, x1, y1)), Some((hx, hy))) => {
                let margin = 1.0;
                centre.0 + hx < x0 - margin
                    || centre.0 - hx > x1 + margin
                    || centre.1 + hy < y0 - margin
                    || centre.1 - hy > y1 + margin
            }
            _ => {
                let reach = extent
                    .map(|(x0, y0, x1, y1)| ((x1 - x0).hypot(y1 - y0)) / 2.0 + 3.0)
                    .unwrap_or(5.0);
                off > reach
            }
        };
        if misplaced {
            p.origin_x = package_origin.0 + pads.0 / PX_MM;
            p.origin_y = package_origin.1 - pads.1 / PX_MM;
            notes.push(format!(
                "3D: EasyEDA puts the model {off:.2} mm away from its pads, beyond the part; centred it on the pads"
            ));
        }
    }
    let mut transform = allegro_transform(placement.as_ref(), package_origin, points);
    if let Some(points) = points {
        settle_on_footprint(&mut transform, points, ki_footprint, notes);
    }
    transform
}

/// Checks EasyEDA's placement against the model itself and the pads:
///
/// * a model left wholly under the board (EasyEDA's height for C18188954 sinks a
///   TO-92 12 mm) gets its body set on the board;
/// * a through-hole model's leads must sit in their holes: when most holes see
///   their lead off by the same amount (C141882's D-sub pins by 0.16 mm) the
///   model is moved by it; a placement whose leads are already in the holes is
///   left alone even if some other part pokes out (C7095263's shield tabs);
/// * a surface-mount model's contacts (its bottom layer: balls, lands, leads)
///   must sit on the pads. When they span the pads but leave them (C253882's
///   balls by 1.2 mm, C3224283's terminals by 2.7 mm), the model is moved onto
///   the pads.
fn settle_on_footprint(
    transform: &mut StepTransform,
    points: &[[f64; 3]],
    footprint: &KiFootprint,
    notes: &mut Vec<String>,
) {
    let place = |t: &StepTransform| -> Vec<[f64; 3]> {
        points
            .iter()
            .map(|&p| {
                let q = crate::cadence::step_place::rotate(p, t.rotation);
                [q[0] + t.offset[0], q[1] + t.offset[1], q[2] + t.offset[2]]
            })
            .collect()
    };
    let round = |v: f64| (v * 10_000.0).round() / 10_000.0 + 0.0;
    let mut placed = place(transform);
    let Some((min, max)) = bounds_of(&placed, [0.0; 3]) else {
        return;
    };
    // A model whose top barely clears the board has its body in or under it
    // (EasyEDA's height for C5632434 puts a leaded MLCC 5 mm down): its body is
    // set on the board too.
    let sunk = max[2] < 1.0 && max[2] < 0.2 * (max[2] - min[2]);
    if max[2] < 0.05 || sunk {
        let seat = seat_height(&placed).unwrap_or(max[2]);
        transform.offset[2] = round(transform.offset[2] - seat);
        notes.push(if max[2] < 0.05 {
            format!(
                "3D: EasyEDA puts the whole model {:.2} mm under the board; its body was set on the board",
                -max[2]
            )
        } else {
            format!(
                "3D: EasyEDA puts the model's body {:.2} mm into the board; it was set on the board",
                -seat
            )
        });
        placed = place(transform);
    }

    // Pad centres, and the pads' outline (centres plus their rotated sizes).
    let Some(centres) = pad_extent_mm(footprint) else {
        return;
    };
    let mut outline = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for pad in &footprint.pads {
        let (x, y) = (pad.pos_x * PX_MM, -pad.pos_y * PX_MM);
        let (sin, cos) = pad.rotation.to_radians().sin_cos();
        let (w, h) = (pad.size_x * PX_MM, pad.size_y * PX_MM);
        let hw = (w * cos.abs() + h * sin.abs()) / 2.0;
        let hh = (w * sin.abs() + h * cos.abs()) / 2.0;
        outline.0 = outline.0.min(x - hw);
        outline.1 = outline.1.min(y - hh);
        outline.2 = outline.2.max(x + hw);
        outline.3 = outline.3.max(y + hh);
    }
    let Some((zmin, zmax)) = bounds_of(&placed, [0.0; 3]).map(|(lo, hi)| (lo[2], hi[2])) else {
        return;
    };
    let through_hole = zmin < -0.1 && zmax > 0.1;
    if through_hole {
        if let Some(lift) = lift_out_of_holes(&placed, footprint) {
            transform.offset[2] = round(transform.offset[2] + lift);
            notes.push(format!(
                "3D: the model's lead bends sat in the board in EasyEDA's data; raised it {lift:.2} mm so the straight leads go through the holes"
            ));
            placed = place(transform);
        }
        let leads: Vec<[f64; 3]> = placed.iter().copied().filter(|p| p[2] < -0.05).collect();
        if let Some(shift) = leads_off_holes(&leads, footprint) {
            let distance = shift.0.hypot(shift.1);
            transform.offset[0] = round(transform.offset[0] + shift.0);
            transform.offset[1] = round(transform.offset[1] + shift.1);
            notes.push(format!(
                "3D: the model's leads were {distance:.2} mm off the holes in EasyEDA's data; moved into the holes"
            ));
        }
        return;
    }
    // Surface mount: what touches the board (a thin slab, so a body sitting
    // just above its terminals is not counted as a contact).
    let contacts: Vec<&[f64; 3]> = placed.iter().filter(|p| p[2] <= zmin + 0.02).collect();
    if contacts.is_empty() {
        return;
    }
    let (mut cx0, mut cy0, mut cx1, mut cy1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &contacts {
        cx0 = cx0.min(p[0]);
        cy0 = cy0.min(p[1]);
        cx1 = cx1.max(p[0]);
        cy1 = cy1.max(p[1]);
    }
    // The contacts belong to these pads if, on each axis, they span at least
    // the pad centres and no more than the pads themselves.
    let fits =
        |contact: f64, centre: f64, pads: f64| contact >= centre - 0.3 && contact <= pads + 0.5;
    let matches = fits(cx1 - cx0, centres.2 - centres.0, outline.2 - outline.0)
        && fits(cy1 - cy0, centres.3 - centres.1, outline.3 - outline.1);
    if !matches {
        return;
    }
    // Only move a model whose contacts leave the pads; contacts that already
    // land on them stay where EasyEDA put them.
    let overhang = (outline.0 - cx0)
        .max(cx1 - outline.2)
        .max(outline.1 - cy0)
        .max(cy1 - outline.3);
    let snug = (cx1 - cx0 - (outline.2 - outline.0))
        .max(cy1 - cy0 - (outline.3 - outline.1))
        .max(0.0)
        / 2.0;
    if overhang - snug <= 0.05 {
        return;
    }
    let shift = (
        (outline.0 + outline.2) / 2.0 - (cx0 + cx1) / 2.0,
        (outline.1 + outline.3) / 2.0 - (cy0 + cy1) / 2.0,
    );
    let distance = shift.0.hypot(shift.1);
    if distance > 0.05 && distance <= 3.0 {
        transform.offset[0] = round(transform.offset[0] + shift.0);
        transform.offset[1] = round(transform.offset[1] + shift.1);
        notes.push(format!(
            "3D: the model's contacts were {distance:.2} mm off the pads in EasyEDA's data; moved onto the pads"
        ));
    }
}

/// Drilled pads as (x, y, hole radius, reach): reach is half the distance to
/// the next hole (at most 1.5 mm), the room a lead has around its hole.
fn drilled_holes(footprint: &KiFootprint) -> Vec<(f64, f64, f64, f64)> {
    let holes: Vec<(f64, f64, f64)> = footprint
        .pads
        .iter()
        .filter_map(|pad| {
            let drill = pad.drill.as_ref()?;
            let radius = drill.diameter.max(drill.width.unwrap_or(0.0)) * PX_MM / 2.0;
            (radius > 0.0).then(|| (pad.pos_x * PX_MM, -pad.pos_y * PX_MM, radius))
        })
        .collect();
    holes
        .iter()
        .enumerate()
        .map(|(index, &(x, y, radius))| {
            let next = holes
                .iter()
                .enumerate()
                .filter(|&(other, _)| other != index)
                .map(|(_, &(ox, oy, _))| (ox - x).hypot(oy - y))
                .fold(f64::MAX, f64::min);
            (x, y, radius, (next / 2.0).min(1.5).max(radius))
        })
        .collect()
}

/// How far to raise a through-hole model whose lead bends (the kinks that hold
/// a disc thermistor or varistor off the board) EasyEDA put in the board, so
/// that where the leads cross the board they are inside their holes
/// (C49241632's kinks were 1.3 mm down in the board). The leads' ends must be
/// in the holes and the body clear of the board; models whose leads already
/// cross the board in their holes, or whose body sits on it, are left alone.
fn lift_out_of_holes(placed: &[[f64; 3]], footprint: &KiFootprint) -> Option<f64> {
    let holes = drilled_holes(footprint);
    if holes.is_empty() {
        return None;
    }
    let lowest = placed.iter().map(|p| p[2]).fold(f64::MAX, f64::min);
    // The body (the first wide level) must be off the board.
    let seat = seat_height(placed)?;
    if seat <= 0.2 {
        return None;
    }
    let near = |p: &[f64; 3], &(x, y, _, reach): &(f64, f64, f64, f64)| {
        (p[0] - x).hypot(p[1] - y) <= reach
    };
    // Every hole must hold the end of a lead.
    for hole in &holes {
        let ends: Vec<&[f64; 3]> = placed
            .iter()
            .filter(|p| p[2] <= lowest + 0.5 && near(p, hole))
            .collect();
        if ends.is_empty() {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &ends {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        if ((x0 + x1) / 2.0 - hole.0).hypot((y0 + y1) / 2.0 - hole.1) > hole.2 {
            return None;
        }
    }
    // Below the board, the leads must be inside their holes: the lowest point
    // around a hole but outside it is where the bends start, and it has to
    // come up to the board.
    let bends = placed
        .iter()
        .filter(|p| {
            holes
                .iter()
                .any(|hole| near(p, hole) && (p[0] - hole.0).hypot(p[1] - hole.1) > hole.2)
        })
        .map(|p| p[2])
        .fold(f64::MAX, f64::min);
    if bends >= -0.05 {
        return None;
    }
    let lift = ((-bends + 0.05) * 100.0).round() / 100.0;
    // Still through the board, by no more than a bend's height.
    (lift <= 3.0 && lowest + lift <= -0.5).then_some(lift)
}

/// How far a through-hole model's leads sit from its holes, as the shift that
/// puts them in: for every drilled pad, the middle of the lead points around it
/// (within half the distance to the next hole) compared with the hole. Only a
/// shift most holes agree on counts, so a lead that is off-centre by design (a
/// shield tab in a slot, a split board lock) neither causes nor blocks one.
fn leads_off_holes(leads: &[[f64; 3]], footprint: &KiFootprint) -> Option<(f64, f64)> {
    let holes = drilled_holes(footprint);
    let mut offsets = Vec::new();
    for &(x, y, _, reach) in &holes {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in leads.iter().filter(|p| (p[0] - x).hypot(p[1] - y) <= reach) {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        if x1 >= x0 {
            offsets.push(((x0 + x1) / 2.0 - x, (y0 + y1) / 2.0 - y));
        }
    }
    if offsets.is_empty() {
        return None;
    }
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let mid = values.len() / 2;
        if values.len().is_multiple_of(2) {
            (values[mid - 1] + values[mid]) / 2.0
        } else {
            values[mid]
        }
    };
    let mx = median(offsets.iter().map(|o| o.0).collect());
    let my = median(offsets.iter().map(|o| o.1).collect());
    let agree = offsets
        .iter()
        .filter(|o| (o.0 - mx).hypot(o.1 - my) <= 0.1)
        .count();
    let needed = (0.6 * holes.len() as f64).max(2.0);
    (agree as f64 >= needed && mx.hypot(my) > 0.05).then_some((-mx, -my))
}

/// Height of a model's seating plane: the lowest level whose cross-section is
/// at least 40% of the widest one (leads are narrow, the body is not).
fn seat_height(points: &[[f64; 3]]) -> Option<f64> {
    let (min, max) = bounds_of(points, [0.0; 3])?;
    const SLICES: usize = 40;
    let step = (max[2] - min[2]) / SLICES as f64;
    if step <= 0.0 {
        return Some(min[2]);
    }
    let areas: Vec<f64> = (0..SLICES)
        .map(|i| {
            let (z0, z1) = (min[2] + step * i as f64, min[2] + step * (i + 1) as f64);
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in points.iter().filter(|p| p[2] >= z0 && p[2] <= z1) {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            if x1 < x0 { 0.0 } else { (x1 - x0) * (y1 - y0) }
        })
        .collect();
    let widest = areas.iter().copied().fold(0.0, f64::max);
    let first = areas.iter().position(|&a| a >= 0.4 * widest)?;
    Some(min[2] + step * first as f64)
}

/// (min x, min y, max x, max y) of the pad centres in mm, Allegro axes (Y up).
fn pad_extent_mm(footprint: &KiFootprint) -> Option<(f64, f64, f64, f64)> {
    let mut extent: Option<(f64, f64, f64, f64)> = None;
    for pad in &footprint.pads {
        let (x, y) = (pad.pos_x * PX_MM, -pad.pos_y * PX_MM);
        extent = Some(match extent {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    extent
}

/// Centre of the pad centres in mm, Allegro axes.
fn pad_centre_mm(footprint: &KiFootprint) -> Option<(f64, f64)> {
    let (x0, y0, x1, y1) = pad_extent_mm(footprint)?;
    Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: &str = "ISO-10303-21;\nDATA;\n#1=CARTESIAN_POINT('',(-1.01,-0.41,-0.71));\n#2=CARTESIAN_POINT('',(0.59,0.39,0.));\n#3=CARTESIAN_POINT('Axis',(50.,50.,50.));\n#4=VERTEX_POINT('',#1);\n#5=VERTEX_POINT('',#2);\nENDSEC;\n";

    fn corners(min: [f64; 3], max: [f64; 3]) -> Vec<[f64; 3]> {
        (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                ]
            })
            .collect()
    }

    #[test]
    fn bounds_use_vertices_only() {
        let (min, max) = step_bounds(STEP.as_bytes()).unwrap();
        assert_eq!(min, [-1.01, -0.41, -0.71]);
        assert_eq!(max, [0.59, 0.39, 0.0]);
    }

    #[test]
    fn rotated_off_centre_model_lands_on_its_origin() {
        // ESD5Z5.0 (C475685): model centre x = -0.21, rotated 180 degrees, origin on
        // the package origin, bottom 0.71 below the model origin.
        let placement = ModelPlacement {
            origin_x: 4000.0,
            origin_y: 3000.0,
            z: 0.0,
            rotation: [0.0, 0.0, 180.0],
        };
        let points = model_points(STEP.as_bytes()).unwrap();
        let t = allegro_transform(Some(&placement), (4000.0, 3000.0), Some(&points));
        assert_eq!(t.rotation, [0.0, 0.0, 180.0]);
        assert_eq!(t.offset, [-0.21, -0.01, 0.71]);
    }

    #[test]
    fn origin_offset_is_converted_and_y_flipped() {
        // CL10A475 (C19666): c_origin 3998.0435,3000.5 on a package at 4000,3000.
        let placement = ModelPlacement {
            origin_x: 3998.0435,
            origin_y: 3000.5,
            z: 0.0,
            rotation: [0.0; 3],
        };
        let points = corners([-0.8, -0.4, -0.4], [0.8, 0.4, 0.4]);
        let t = allegro_transform(Some(&placement), (4000.0, 3000.0), Some(&points));
        assert_eq!(t.offset, [-0.497, -0.127, 0.4]);
        assert_eq!(t.rotation, [0.0; 3]);
        assert_eq!(
            allegro_transform(None, (0.0, 0.0), None),
            StepTransform::default()
        );
    }

    #[test]
    fn tilted_model_rests_on_the_board_centred_on_its_origin() {
        // A TO-92 drawn lying down (leads along +y), stood up by EasyEDA's 90
        // degree turn about x: its leads must end up below, the body above z.
        let placement = ModelPlacement {
            origin_x: 0.0,
            origin_y: 0.0,
            z: 0.0,
            rotation: [90.0, 0.0, 0.0],
        };
        let points = corners([-2.0, 0.0, -0.8], [2.0, 18.0, 0.8]);
        let t = allegro_transform(Some(&placement), (0.0, 0.0), Some(&points));
        let placed: Vec<[f64; 3]> = points
            .iter()
            .map(|&p| {
                let q = rotate(p, t.rotation);
                [q[0] + t.offset[0], q[1] + t.offset[1], q[2] + t.offset[2]]
            })
            .collect();
        let (min, max) = bounds_of(&placed, [0.0; 3]).unwrap();
        assert!(min[2].abs() < 1e-6, "bottom at {}", min[2]);
        assert!(((min[0] + max[0]) / 2.0).abs() < 1e-6);
        assert!(((min[1] + max[1]) / 2.0).abs() < 1e-6);
        assert!(max[2] > 17.9);
    }

    #[test]
    fn negative_angles_are_normalised() {
        let placement = ModelPlacement {
            origin_x: 0.0,
            origin_y: 0.0,
            z: 0.0,
            rotation: [0.0, 0.0, -90.0],
        };
        let t = allegro_transform(Some(&placement), (0.0, 0.0), None);
        assert_eq!(t.rotation, [0.0, 0.0, 270.0]);
    }

    #[test]
    fn seat_is_where_the_body_starts_above_the_leads() {
        // Two thin leads from z -5 to -2, a 4 x 2 mm body from -2 to 1.
        let mut points = Vec::new();
        for z in [-5.0, -4.0, -3.0, -2.5] {
            points.push([-1.27, 0.0, z]);
            points.push([-1.0, 0.3, z]);
            points.push([1.27, 0.0, z]);
            points.push([1.0, 0.3, z]);
        }
        for z in [-2.0, -1.0, 0.0, 1.0] {
            points.push([-2.0, -1.0, z]);
            points.push([2.0, 1.0, z]);
        }
        let seat = seat_height(&points).unwrap();
        assert!((seat + 2.0).abs() < 0.2, "seat {seat}");
    }

    fn hole_pads(holes: &[(f64, f64, f64)]) -> KiFootprint {
        let pads = holes
            .iter()
            .enumerate()
            .map(|(i, &(x, y, d))| crate::kicad::KiPad {
                number: (i + 1).to_string(),
                pad_type: crate::kicad::PadType::ThroughHole,
                shape: crate::kicad::PadShape::Circle,
                pos_x: x / 0.254,
                pos_y: -y / 0.254,
                size_x: (d + 0.5) / 0.254,
                size_y: (d + 0.5) / 0.254,
                rotation: 0.0,
                layers: vec!["*.Cu".into()],
                drill: Some(crate::kicad::Drill {
                    diameter: d / 0.254,
                    width: None,
                    offset_x: 0.0,
                    offset_y: 0.0,
                }),
                polygon: None,
            })
            .collect();
        KiFootprint {
            name: "t".into(),
            pads,
            tracks: vec![],
            circles: vec![],
            arcs: vec![],
            texts: vec![],
            lines: vec![],
            model_3d: None,
        }
    }

    /// A square lead's corners around (x, y), below the board.
    fn lead(points: &mut Vec<[f64; 3]>, x: f64, y: f64) {
        for (dx, dy) in [(-0.3, -0.3), (0.3, -0.3), (-0.3, 0.3), (0.3, 0.3)] {
            points.push([x + dx, y + dy, -1.0]);
            points.push([x + dx, y + dy, -3.0]);
        }
    }

    #[test]
    fn leads_off_their_holes_are_moved_in() {
        // Nine D-sub pins 0.16 mm off, and two board locks whose split legs sit
        // off-centre in their big holes.
        let mut holes: Vec<(f64, f64, f64)> = (0..5)
            .map(|i| (-1.42, -5.52 + 2.76 * i as f64, 1.1))
            .collect();
        holes.extend((0..4).map(|i| (1.42, -4.14 + 2.76 * i as f64, 1.1)));
        holes.extend([(0.0, -12.5, 3.2), (0.0, 12.5, 3.2)]);
        let mut points = Vec::new();
        for &(x, y, d) in &holes {
            if d < 2.0 {
                lead(&mut points, x - 0.16, y);
            } else {
                lead(&mut points, x - 0.16, y - 0.5);
            }
        }
        let shift = leads_off_holes(&points, &hole_pads(&holes)).expect("a shift");
        assert!(
            (shift.0 - 0.16).abs() < 1e-6 && shift.1.abs() < 1e-6,
            "{shift:?}"
        );
    }

    #[test]
    fn kinked_leads_are_raised_out_of_the_board() {
        // A disc thermistor: leads through holes 5 mm apart, kinked 1.75 mm
        // sideways from z -1.4 to 0, the disc from 0.4 up.
        let holes = [(-2.5, 0.0, 0.9), (2.5, 0.0, 0.9)];
        let mut points = Vec::new();
        for &(x, _, _) in &holes {
            for z in [-5.0, -3.0, -1.5] {
                for (dx, dy) in [(-0.25, -0.25), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)] {
                    points.push([x + dx, dy, z]);
                }
            }
            for (z, y) in [
                (-1.4, 0.6),
                (-1.0, 1.0),
                (-0.6, 1.75),
                (-0.2, 1.0),
                (0.0, 0.3),
            ] {
                points.push([x - 0.25, y, z]);
                points.push([x + 0.25, y, z]);
            }
        }
        for z in [0.4, 2.0, 5.0, 8.0, 10.0] {
            for (x, y) in [(-4.5, -3.0), (4.5, -3.0), (-4.5, 3.0), (4.5, 3.0)] {
                points.push([x, y, z]);
            }
        }
        let lift = lift_out_of_holes(&points, &hole_pads(&holes)).expect("a lift");
        assert!(lift > 1.4 && lift < 1.8, "lift {lift}");
        // The same leads under a body that sits on the board stay as they are.
        for (x, y) in [(-4.5, -3.0), (4.5, -3.0), (-4.5, 3.0), (4.5, 3.0)] {
            points.push([x, y, 0.0]);
        }
        assert_eq!(lift_out_of_holes(&points, &hole_pads(&holes)), None);
    }

    #[test]
    fn leads_in_their_holes_stay_put() {
        // A USB-C receptacle: sixteen pins in their holes, four shield tabs off
        // the middle of their slots by design.
        let mut holes: Vec<(f64, f64, f64)> = Vec::new();
        for row in [0.83, 2.18] {
            holes.extend((0..8).map(|i| (-2.98 + 0.85 * i as f64, row, 0.4)));
        }
        holes.extend([
            (-4.33, 1.2, 0.6),
            (4.33, 1.2, 0.6),
            (-4.33, -2.18, 0.6),
            (4.33, -2.18, 0.6),
        ]);
        let mut points = Vec::new();
        for &(x, y, d) in &holes {
            let off = if d > 0.5 { 0.7 } else { 0.02 };
            for (dx, dy) in [(-0.1, -0.07), (0.1, -0.07), (-0.1, 0.07), (0.1, 0.07)] {
                points.push([x + dx, y + off + dy, -1.0]);
            }
        }
        assert_eq!(leads_off_holes(&points, &hole_pads(&holes)), None);
    }
}
