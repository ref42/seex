//! Placing EasyEDA STEP models on Allegro footprints.
//!
//! EasyEDA keeps a model in its own coordinates and records where it sits on the
//! footprint: the model's centre (`c_origin`), its height above the board (`z`) and a
//! rotation (`c_rotation`). EasyEDA centres the model's outline on `c_origin` and
//! rests its bottom on the board, so the Allegro mapping is: rotate by `c_rotation`,
//! then shift the rotated centre onto `c_origin` and the bottom onto `z`.

use crate::cadence::allegro::StepTransform;
use crate::easyeda::{ComponentData, ModelPlacement};
use crate::kicad::KiFootprint;

/// EasyEDA canvas unit (10 mil) in millimetres.
const PX_MM: f64 = 0.254;

/// Bounding box (min, max) of a STEP model's vertices, in the file's units (mm for
/// EasyEDA models). Falls back to all points when the file has no vertex list.
pub fn step_bounds(data: &[u8]) -> Option<([f64; 3], [f64; 3])> {
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
/// `package_origin` (EasyEDA canvas units).
pub fn allegro_transform(
    placement: Option<&ModelPlacement>,
    package_origin: (f64, f64),
    bounds: Option<([f64; 3], [f64; 3])>,
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
    let offset = match bounds {
        // A model tilted about x or y: keep EasyEDA's numbers, the bounding box of
        // the unrotated model says nothing useful about the result.
        Some((min, max)) if rotation[0] == 0.0 && rotation[1] == 0.0 => {
            let (cx, cy) = ((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
            let (sin, cos) = rotation[2].to_radians().sin_cos();
            let (rx, ry) = (cx * cos - cy * sin, cx * sin + cy * cos);
            [target_x - rx, target_y - ry, bottom - min[2]]
        }
        _ => [target_x, target_y, bottom],
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
    ipc: Option<(f64, (f64, f64))>,
    data: Option<&[u8]>,
    notes: &mut Vec<String>,
) -> StepTransform {
    let Some((turn, (sx, sy))) = ipc else {
        return easyeda_model_transform(component, easyeda_footprint, data, notes);
    };
    // The calculated pattern is centred on the package, so the model's outline is
    // centred on the origin; EasyEDA only decides how it is turned and its height.
    // EasyEDA's notes about where it put the model do not apply here.
    let mut easyeda_notes = Vec::new();
    let transform = easyeda_model_transform(component, easyeda_footprint, data, &mut easyeda_notes);
    if transform.rotation[0] == 0.0 && transform.rotation[1] == 0.0 {
        if let Some((min, max)) = data.and_then(step_bounds) {
            let rz = (transform.rotation[2] + turn).rem_euclid(360.0);
            let (cx, cy) = ((min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0);
            let (sin, cos) = rz.to_radians().sin_cos();
            return StepTransform {
                rotation: [0.0, 0.0, rz],
                offset: [
                    round4(-(cx * cos - cy * sin)) + 0.0,
                    round4(-(cx * sin + cy * cos)) + 0.0,
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
    StepTransform {
        rotation: [
            transform.rotation[0],
            transform.rotation[1],
            (transform.rotation[2] + turn).rem_euclid(360.0),
        ],
        offset: [
            round4(ox * cos - oy * sin + sx) + 0.0,
            round4(ox * sin + oy * cos + sy) + 0.0,
            oz,
        ],
    }
}

/// Allegro mapping for EasyEDA's own model on EasyEDA's footprint.
fn easyeda_model_transform(
    component: &ComponentData,
    footprint: &KiFootprint,
    data: Option<&[u8]>,
    notes: &mut Vec<String>,
) -> StepTransform {
    let bounds = data.and_then(step_bounds);
    if bounds.is_none() {
        notes.push("3D: could not read the model outline; placed at the origin".to_string());
    }
    let package_origin = (component.package_bbox_x, component.package_bbox_y);
    let mut placement = component.model_3d.as_ref().and_then(|m| m.placement);
    // EasyEDA data sometimes places a model beside its pads. A two-pad part is
    // symmetric about its pads, so centre it there; otherwise keep EasyEDA's
    // placement and say so.
    if let (Some(p), Some((x0, y0, x1, y1))) = (placement.as_mut(), pad_extent_mm(footprint)) {
        let pads = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let centre = model_centre_mm(p, package_origin);
        let off = ((centre.0 - pads.0).powi(2) + (centre.1 - pads.1).powi(2)).sqrt();
        // Farther from the pads than the pads reach (plus a body's overhang): the
        // placement is plainly wrong (seen 1.1 m off), so centre the model.
        let reach = (x1 - x0).hypot(y1 - y0) / 2.0 + 3.0;
        if off > reach || off > 0.25 && footprint.pads.len() == 2 {
            p.origin_x = package_origin.0 + pads.0 / PX_MM;
            p.origin_y = package_origin.1 - pads.1 / PX_MM;
            notes.push(format!(
                "3D: EasyEDA puts the model {off:.2} mm off its pads; centred it on the pads"
            ));
        } else if off > 0.25 && footprint.pads.len() > 2 {
            notes.push(format!(
                "3D: EasyEDA puts the model {off:.2} mm from the pad centre; check it in the 3D view"
            ));
        }
    }
    allegro_transform(placement.as_ref(), package_origin, bounds)
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

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: &str = "ISO-10303-21;\nDATA;\n#1=CARTESIAN_POINT('',(-1.01,-0.41,-0.71));\n#2=CARTESIAN_POINT('',(0.59,0.39,0.));\n#3=CARTESIAN_POINT('Axis',(50.,50.,50.));\n#4=VERTEX_POINT('',#1);\n#5=VERTEX_POINT('',#2);\nENDSEC;\n";

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
        let t = allegro_transform(
            Some(&placement),
            (4000.0, 3000.0),
            step_bounds(STEP.as_bytes()),
        );
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
        let bounds = Some(([-0.8, -0.4, -0.4], [0.8, 0.4, 0.4]));
        let t = allegro_transform(Some(&placement), (4000.0, 3000.0), bounds);
        assert_eq!(t.offset, [-0.497, -0.127, 0.4]);
        assert_eq!(t.rotation, [0.0; 3]);
        assert_eq!(
            allegro_transform(None, (0.0, 0.0), None),
            StepTransform::default()
        );
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
}
