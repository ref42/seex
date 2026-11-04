use serde_json::Value;

use super::pcblib::{
    CoordPoint, LAYER_BOTTOM, LAYER_BOTTOM_OVERLAY, LAYER_BOTTOM_SOLDER, LAYER_MECHANICAL_1,
    LAYER_MECHANICAL_2, LAYER_MECHANICAL_5, LAYER_MECHANICAL_6, LAYER_MECHANICAL_9, LAYER_MULTI,
    LAYER_TOP, LAYER_TOP_OVERLAY, LAYER_TOP_SOLDER, ModelData, PAD_HOLE_ROUND, PAD_HOLE_SLOT,
    PAD_HOLE_SQUARE, PAD_SHAPE_OCTAGONAL, PAD_SHAPE_RECTANGULAR, PAD_SHAPE_ROUND,
    PAD_SHAPE_ROUNDED_RECTANGLE, PcbArc, PcbComponent, PcbComponentBody, PcbExtendedPrimitiveInfo,
    PcbLibrary, PcbModel, PcbPad, PcbRegion, PcbTrack, stable_guid,
};
use crate::error::{AppError, Result};
use crate::util::{altium_model_rotation_z, nested_string, sanitize_filename, value_to_string};

const FOOTPRINT_UNIT_TO_MM: f64 = 0.0254;
const RAW_PER_MIL: f64 = 10_000.0;
const DEFAULT_GRAPHIC_WIDTH_MM: f64 = 0.05;
const CIRCLE_SEGMENTS: usize = 32;
const PATH_ARC_SEGMENT_DEGREES: f64 = 10.0;
const ROUNDED_RECT_CORNER_SEGMENTS: usize = 8;
const DEFAULT_CORNER_RADIUS_PERCENTAGE: u8 = 25;
const MIN_COMPONENT_BODY_HEIGHT_MM: f64 = 0.2;
const CUSTOM_PAD_HOTSPOT_UNITS: f64 = 2.3792;
const DEFAULT_PAD_SOLDER_MASK_EXPANSION_MIL: f64 = 1.969;

/// Builds a library from a footprint payload and a model's bytes.
///
/// The export path spills the model before building (see
/// `build_pcblib_from_payload_with_model`), so a batch never holds a model per part;
/// this entry point is for callers that already have the bytes in hand.
#[cfg_attr(not(test), allow(dead_code))]
pub fn build_pcblib_from_payload(
    payload: &Value,
    component_name: &str,
    step_bytes: Option<&[u8]>,
) -> Result<PcbLibrary> {
    let step_data = step_bytes.map(|bytes| {
        // LCEDA does not always author a model with its underside on the board plane, so
        // the embedded geometry is moved there (see `crate::step_model`).
        ModelData::spill_copy(crate::step_model::place_step_on_board(bytes).as_ref())
    });
    build_pcblib_from_payload_with_model(payload, component_name, step_data)
}

/// The same library, for a caller that already placed the model and spilled it.
pub fn build_pcblib_from_payload_with_model(
    payload: &Value,
    component_name: &str,
    step_data: Option<ModelData>,
) -> Result<PcbLibrary> {
    let has_step = step_data.is_some();
    let rows = parse_easyeda_rows(payload)?;
    let model_3d = parse_footprint_3d_model(payload).or_else(|| {
        has_step.then(|| Model3dRaw {
            title: component_name.to_string(),
            uri: format!("embedded-step:{component_name}"),
            height_mm: None,
            rotation_x: 0.0,
            rotation_y: 0.0,
            rotation_z: 0.0,
        })
    });
    let mut pads = Vec::new();
    let mut overlay_polys = Vec::new();
    let mut overlay_circles = Vec::new();
    let mut overlay_arcs = Vec::new();
    let mut overlay_regions = Vec::new();
    let mut multilayer_fill_circles = Vec::new();
    let mut bounds = Bounds::default();
    let mut body_bounds = Bounds::default();
    let mut fallback_designator = 1usize;

    for row in &rows {
        let Some(row_type) = row.first().and_then(Value::as_str) else {
            continue;
        };
        match row_type.trim().to_ascii_uppercase().as_str() {
            "PAD" => {
                let layer_code = row_i32(row, 4, 1);
                let mut designator = row_string(row, 5);
                if designator.trim().is_empty() {
                    designator = fallback_designator.to_string();
                    fallback_designator += 1;
                }
                let mut x = row_f64(row, 6, 0.0);
                let mut y = row_f64(row, 7, 0.0);
                let mut rotation = row_f64(row, 8, f64::NAN);
                if rotation.is_nan() {
                    rotation = row_f64(row, 14, 0.0);
                }
                let mut hole = row_f64(row, 9, 0.0);
                let mut hole_slot = hole;
                let mut hole_shape = "ROUND".to_string();
                if let Some(Value::Array(hole_array)) = row.get(9) {
                    hole_shape =
                        value_string(hole_array.first()).unwrap_or_else(|| "ROUND".to_string());
                    let first_dimension = value_f64(hole_array.get(1)).unwrap_or(0.0);
                    let second_dimension = value_f64(hole_array.get(2)).unwrap_or(first_dimension);
                    if hole_shape.trim().eq_ignore_ascii_case("SLOT") {
                        hole = first_dimension.min(second_dimension);
                        hole_slot = first_dimension.max(second_dimension);
                    } else {
                        hole = first_dimension;
                        hole_slot = second_dimension;
                    }
                }
                let mut width: f64 = 10.0;
                let mut height: f64 = 10.0;
                let mut shape = "ROUND".to_string();
                let mut polygon_points = None;
                let mut custom_polygon = false;
                if let Some(Value::Array(shape_array)) = row.get(10) {
                    shape =
                        value_string(shape_array.first()).unwrap_or_else(|| "ROUND".to_string());
                    if shape.eq_ignore_ascii_case("POLY") {
                        if let Some(poly_shape) = shape_array.get(1) {
                            let poly_raw_points = parse_path_raw_points(poly_shape);
                            if let Some(poly_bounds) = Bounds::from_raw_points(&poly_raw_points) {
                                custom_polygon = poly_raw_points.len() >= 3;
                                width = width.max(poly_bounds.max_x - poly_bounds.min_x);
                                height = height.max(poly_bounds.max_y - poly_bounds.min_y);
                                if let Some(rect) = axis_aligned_rect_from_points(&poly_raw_points)
                                {
                                    shape = "RECT".to_string();
                                    x = rect.center_x;
                                    y = rect.center_y;
                                    width = rect.width;
                                    height = rect.height;
                                } else if poly_raw_points.len() >= 3 {
                                    polygon_points = Some(poly_raw_points);
                                }
                            }
                        }
                    } else {
                        width = value_f64(shape_array.get(1)).unwrap_or(width);
                        height = value_f64(shape_array.get(2)).unwrap_or(width);
                    }
                }
                if width <= 0.0 {
                    width = 10.0;
                }
                if height <= 0.0 {
                    height = width;
                }
                pads.push(PadRaw {
                    designator,
                    x,
                    y,
                    width,
                    height,
                    hole: hole.max(0.0),
                    hole_slot: hole_slot.max(hole),
                    hole_shape,
                    rotation,
                    layer_code,
                    shape,
                    polygon_points,
                    custom_polygon,
                    custom_mask_expansion_units: value_f64(row.get(17))
                        .filter(|value| *value > 0.0)
                        .or_else(|| value_f64(row.get(18)).filter(|value| *value > 0.0)),
                });
                bounds.update_span(
                    x - width / 2.0,
                    x + width / 2.0,
                    y - height / 2.0,
                    y + height / 2.0,
                );
            }
            "POLY" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let Some(shape_value) = row.get(6) else {
                    continue;
                };
                if let Some(circle) = try_parse_circle_shape(shape_value) {
                    if is_component_body_layer(layer_code) {
                        body_bounds.update_span(
                            circle.cx - circle.radius,
                            circle.cx + circle.radius,
                            circle.cy - circle.radius,
                            circle.cy + circle.radius,
                        );
                        continue;
                    }
                    if !is_overlay_layer(layer_code) {
                        continue;
                    }
                    bounds.update_span(
                        circle.cx - circle.radius,
                        circle.cx + circle.radius,
                        circle.cy - circle.radius,
                        circle.cy + circle.radius,
                    );
                    overlay_circles.push(CircleRaw {
                        layer_code,
                        width: stroke,
                        cx: circle.cx,
                        cy: circle.cy,
                        radius: circle.radius,
                    });
                    continue;
                }
                let raw_points = parse_path_raw_points(shape_value);
                if raw_points.len() < 2 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_from_raw_points(&raw_points);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_from_raw_points(&raw_points);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: raw_points.into_iter().map(raw_point_to_coord).collect(),
                });
            }
            "FILL" => {
                let layer_code = row_i32(row, 4, -1);
                let Some(shape_value) = row.get(7) else {
                    continue;
                };
                if layer_code == 12 {
                    for shape in fill_shape_values(shape_value) {
                        if let Some(circle) = try_parse_circle_shape(shape) {
                            bounds.update_span(
                                circle.cx - circle.radius,
                                circle.cx + circle.radius,
                                circle.cy - circle.radius,
                                circle.cy + circle.radius,
                            );
                            multilayer_fill_circles.push(CircleRaw {
                                layer_code,
                                width: row_f64(row, 5, 0.0),
                                cx: circle.cx,
                                cy: circle.cy,
                                radius: circle.radius,
                            });
                            continue;
                        }
                        let raw_points = parse_path_raw_points(shape);
                        if raw_points.len() < 3 {
                            continue;
                        }
                        bounds.update_from_raw_points(&raw_points);
                        let mut points: Vec<CoordPoint> =
                            raw_points.into_iter().map(raw_point_to_coord).collect();
                        if points.first() != points.last() {
                            if let Some(first) = points.first().copied() {
                                points.push(first);
                            }
                        }
                        overlay_regions.push(RegionRaw { layer_code, points });
                    }
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    for shape in fill_shape_values(shape_value) {
                        if let Some(circle) = try_parse_circle_shape(shape) {
                            body_bounds.update_span(
                                circle.cx - circle.radius,
                                circle.cx + circle.radius,
                                circle.cy - circle.radius,
                                circle.cy + circle.radius,
                            );
                            continue;
                        }
                        let raw_points = parse_path_raw_points(shape);
                        if raw_points.len() < 3 {
                            continue;
                        }
                        body_bounds.update_from_raw_points(&raw_points);
                    }
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                for shape in fill_shape_values(shape_value) {
                    if let Some(circle) = try_parse_circle_shape(shape) {
                        bounds.update_span(
                            circle.cx - circle.radius,
                            circle.cx + circle.radius,
                            circle.cy - circle.radius,
                            circle.cy + circle.radius,
                        );
                        overlay_regions.push(RegionRaw {
                            layer_code,
                            points: circle_region(circle.cx, circle.cy, circle.radius),
                        });
                        continue;
                    }
                    let raw_points = parse_path_raw_points(shape);
                    if raw_points.len() < 3 {
                        continue;
                    }
                    bounds.update_from_raw_points(&raw_points);
                    let mut points: Vec<CoordPoint> =
                        raw_points.into_iter().map(raw_point_to_coord).collect();
                    if points.first() != points.last() {
                        if let Some(first) = points.first().copied() {
                            points.push(first);
                        }
                    }
                    overlay_regions.push(RegionRaw { layer_code, points });
                }
            }
            "TRACK" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x1 = row_f64(row, 6, 0.0);
                let y1 = row_f64(row, 7, 0.0);
                let x2 = row_f64(row, 8, 0.0);
                let y2 = row_f64(row, 9, 0.0);
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x1, x2, y1, y2);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x1, x2, y1, y2);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: vec![coord_from_easy_units(x1, y1), coord_from_easy_units(x2, y2)],
                });
            }
            "RECT" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x1 = row_f64(row, 6, 0.0);
                let y1 = row_f64(row, 7, 0.0);
                let x2 = row_f64(row, 8, x1);
                let y2 = row_f64(row, 9, y1);
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x1, x2, y1, y2);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x1, x2, y1, y2);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: vec![
                        coord_from_easy_units(x1, y1),
                        coord_from_easy_units(x2, y1),
                        coord_from_easy_units(x2, y2),
                        coord_from_easy_units(x1, y2),
                        coord_from_easy_units(x1, y1),
                    ],
                });
            }
            "CIRCLE" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x = row_f64(row, 6, 0.0);
                let y = row_f64(row, 7, 0.0);
                let radius = row_f64(row, 8, 0.0).abs();
                if radius <= 0.000_001 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                overlay_circles.push(CircleRaw {
                    layer_code,
                    width: stroke,
                    cx: x,
                    cy: y,
                    radius,
                });
            }
            "ARC" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x = row_f64(row, 6, 0.0);
                let y = row_f64(row, 7, 0.0);
                let radius = row_f64(row, 8, 0.0).abs();
                if radius <= 0.000_001 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                overlay_arcs.push(ArcRaw {
                    layer_code,
                    width: stroke,
                    cx: x,
                    cy: y,
                    radius,
                    start_angle: normalize_angle(row_f64(row, 9, 0.0)),
                    end_angle: normalize_angle(row_f64(row, 10, 0.0)),
                });
            }
            _ => {}
        }
    }

    let document_circles: Vec<CircleRaw> = overlay_circles
        .iter()
        .copied()
        .filter(|circle| circle.layer_code == 13)
        .collect();
    let component_height_mm =
        resolve_component_height_mm(payload, component_name, model_3d.as_ref());
    let mut component = PcbComponent {
        name: component_name.to_string(),
        description: resolve_footprint_description(payload, component_name),
        height_raw: raw_from_mm(component_height_mm),
        pads: Vec::new(),
        arcs: Vec::new(),
        tracks: Vec::new(),
        regions: Vec::new(),
        bodies: Vec::new(),
        extended_primitive_information: Vec::new(),
    };

    let mut pad_shape_regions = Vec::new();
    let mut custom_copper_regions = Vec::new();
    let mut custom_mask_regions = Vec::new();
    // Keep the owning pad and its copper-region mask metadata in agreement.
    let mut custom_mask_expansions = Vec::new();

    for pad_raw in pads {
        let hole_mm = easy_units_to_mm(pad_raw.hole);
        let is_custom_poly =
            hole_mm <= 0.000_001 && pad_raw.custom_polygon && pad_raw.polygon_points.is_some();
        // A linked solder-layer contour overrides expansion with an explicit
        // mask shape. Native custom pads must derive their opening from copper.
        let needs_custom_mask = hole_mm <= 0.000_001 && pad_raw.custom_polygon && !is_custom_poly;
        let solder_expansion_mils = if is_custom_poly {
            pad_raw
                .custom_mask_expansion_units
                .unwrap_or(DEFAULT_PAD_SOLDER_MASK_EXPANSION_MIL)
        } else {
            DEFAULT_PAD_SOLDER_MASK_EXPANSION_MIL
        };
        let solder_expansion_raw = raw_from_mils(solder_expansion_mils);
        let (anchor, diameter) = if is_custom_poly {
            custom_pad_anchor(
                pad_raw.polygon_points.as_deref().unwrap(),
                RawPoint {
                    x: pad_raw.x,
                    y: pad_raw.y,
                },
            )
        } else {
            (
                RawPoint {
                    x: pad_raw.x,
                    y: pad_raw.y,
                },
                0.0,
            )
        };
        let (shape, width, height, rotation) = if is_custom_poly {
            (PAD_SHAPE_ROUND, diameter, diameter, 0.0)
        } else {
            (
                map_pad_shape(&pad_raw.shape, pad_raw.width, pad_raw.height),
                pad_raw.width,
                pad_raw.height,
                normalize_angle(pad_raw.rotation),
            )
        };
        push_pad(
            &mut component,
            &pad_raw,
            anchor.x,
            anchor.y,
            width,
            height,
            rotation,
            shape,
            solder_expansion_raw,
        );

        if let Some(region_outline) = pad_outline_region(&pad_raw) {
            pad_shape_regions.push(PcbRegion {
                layer: LAYER_MECHANICAL_9,
                outline: region_outline.clone(),
                kind: 0,
                net: None,
                unique_id: None,
                name: Some(" ".to_string()),
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                additional_params: pad_shape_region_params("MECHANICAL9"),
            });
            if is_custom_poly {
                let copper_layer = map_pad_layer(pad_raw.layer_code, hole_mm);
                let copper_layer_name = if copper_layer == LAYER_BOTTOM {
                    "BOTTOM"
                } else if copper_layer == LAYER_MULTI {
                    "MULTILAYER"
                } else {
                    "TOP"
                };
                custom_copper_regions.push(PcbRegion {
                    layer: copper_layer,
                    outline: region_outline.clone(),
                    kind: 0,
                    net: None,
                    unique_id: None,
                    name: Some(" ".to_string()),
                    is_locked: false,
                    is_tenting_top: false,
                    is_tenting_bottom: false,
                    is_keepout: false,
                    additional_params: custom_pad_region_params(
                        copper_layer_name,
                        component.pads.len(),
                        &region_outline,
                    ),
                });
                // The region belongs to the preceding pad through its native PADINDEX.
                // Mask expansion metadata still references the copper region primitive.
                custom_mask_expansions.push(solder_expansion_raw);
            }
            if needs_custom_mask {
                for (mask_layer, v7_layer_name) in mask_region_layers(pad_raw.layer_code, hole_mm) {
                    custom_mask_regions.push(PcbRegion {
                        layer: mask_layer,
                        outline: region_outline.clone(),
                        kind: 0,
                        net: None,
                        unique_id: None,
                        name: Some(" ".to_string()),
                        is_locked: false,
                        is_tenting_top: false,
                        is_tenting_bottom: false,
                        is_keepout: false,
                        additional_params: pad_shape_region_params(v7_layer_name),
                    });
                }
            }
        }
    }

    for circle in multilayer_fill_circles {
        if has_matching_circle_marker(circle, &document_circles) {
            push_alignment_hole(&mut component, circle);
        } else {
            component.regions.push(PcbRegion {
                layer: map_graphic_layer(circle.layer_code),
                outline: circle_region(circle.cx, circle.cy, circle.radius),
                kind: 0,
                net: None,
                unique_id: None,
                name: None,
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                additional_params: Vec::new(),
            });
        }
    }

    for poly in overlay_polys {
        let width_raw = resolve_graphic_width_raw(poly.width);
        for points in poly.points.windows(2) {
            component.tracks.push(PcbTrack {
                layer: map_graphic_layer(poly.layer_code),
                start: points[0],
                end: points[1],
                width_raw,
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                net_index: 0,
                component_index: 0,
            });
        }
    }
    for circle in overlay_circles {
        component.arcs.push(PcbArc {
            layer: map_graphic_layer(circle.layer_code),
            center: coord_from_easy_units(circle.cx, circle.cy),
            radius_raw: raw_from_easy_units(circle.radius),
            start_angle: 0.0,
            end_angle: 360.0,
            width_raw: resolve_graphic_width_raw(circle.width),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
    }
    for arc in overlay_arcs {
        component.arcs.push(PcbArc {
            layer: map_graphic_layer(arc.layer_code),
            center: coord_from_easy_units(arc.cx, arc.cy),
            radius_raw: raw_from_easy_units(arc.radius),
            start_angle: arc.start_angle,
            end_angle: arc.end_angle,
            width_raw: resolve_graphic_width_raw(arc.width),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
    }
    for region in overlay_regions {
        if region.points.len() >= 3 {
            component.regions.push(PcbRegion {
                layer: map_graphic_layer(region.layer_code),
                outline: region.points,
                kind: 0,
                net: None,
                unique_id: None,
                name: None,
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                additional_params: Vec::new(),
            });
        }
    }
    let custom_copper_region_start = component.pads.len()
        + component.tracks.len()
        + component.arcs.len()
        + component.regions.len()
        + pad_shape_regions.len();
    component.regions.extend(pad_shape_regions);
    component.regions.extend(custom_copper_regions);
    component.regions.extend(custom_mask_regions);
    for (offset, expansion_raw) in custom_mask_expansions.into_iter().enumerate() {
        component
            .extended_primitive_information
            .push(PcbExtendedPrimitiveInfo {
                // Apply expansion to the full copper contour, using the same
                // value and precision as the owning pad's manual expansion.
                primitive_index: custom_copper_region_start + offset,
                object_name: "Region".to_string(),
                params: vec![
                    ("TYPE".to_string(), "Mask".to_string()),
                    ("SOLDERMASKEXPANSIONMODE".to_string(), "Manual".to_string()),
                    (
                        "SOLDERMASKEXPANSION_MANUAL".to_string(),
                        format!("{:.4}mil", f64::from(expansion_raw) / RAW_PER_MIL),
                    ),
                    ("PASTEMASKEXPANSIONMODE".to_string(), "None".to_string()),
                ],
            });
    }

    let mut library = PcbLibrary::default();
    let body_extents = body_bounds.finish().or_else(|| bounds.finish());
    if let (Some(model), Some(step_data), Some(extents)) =
        (model_3d.as_ref(), step_data, body_extents)
    {
        let model_id = stable_guid(&format!("{}|{}", component_name, model.uri));
        let model_name = choose_step_model_name(component_name, &model.title);
        component.bodies.push(PcbComponentBody {
            layer_name: "MECHANICAL1".to_string(),
            name: "__LCEDA_BODY__".to_string(),
            kind: 0,
            subpoly_index: -1,
            union_index: 0,
            arc_resolution_raw: raw_from_mils(0.5),
            is_shape_based: false,
            cavity_height_raw: 0,
            standoff_height_raw: 0,
            overall_height_raw: raw_from_mm(component_height_mm.max(MIN_COMPONENT_BODY_HEIGHT_MM)),
            body_color_3d: 0x808080,
            body_opacity_3d: 1.0,
            body_projection: 0,
            model_id: model_id.clone(),
            model_embed: true,
            model_2d_location: CoordPoint::new(0, 0),
            model_2d_rotation: 0.0,
            model_3d_rot_x: 0.0,
            model_3d_rot_y: 0.0,
            // Without this the embedded STEP lands 90 degrees off on every footprint whose
            // EasyEDA model is stored rotated (see `altium_model_rotation_z`).
            model_3d_rot_z: altium_model_rotation_z(model.rotation_z),
            model_3d_dz_raw: 0,
            model_checksum: 0,
            model_name: model_name.clone(),
            model_type: 1,
            model_source: "Undefined".to_string(),
            identifier: None,
            texture: String::new(),
            outline: component_body_outline(extents),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
        library.models.push(PcbModel {
            id: model_id,
            name: model_name,
            is_embedded: true,
            model_source: "Undefined".to_string(),
            rotation_x: 0.0,
            rotation_y: 0.0,
            // Altium keeps the model table entry and the body's MODEL.3D.ROTZ in sync.
            rotation_z: altium_model_rotation_z(model.rotation_z),
            dz_raw: 0,
            checksum: 0,
            step_data,
        });
    }
    library.components.push(component);
    Ok(library)
}

fn push_pad(
    component: &mut PcbComponent,
    pad_raw: &PadRaw,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    rotation: f64,
    shape: u8,
    solder_mask_expansion_raw: i32,
) {
    let hole_mm = easy_units_to_mm(pad_raw.hole);
    let layer = map_pad_layer(pad_raw.layer_code, hole_mm);
    let hole_type = map_pad_hole_type(&pad_raw.hole_shape);
    let hole_slot_length_raw = raw_from_mm(easy_units_to_mm(pad_raw.hole_slot));
    let hole_rotation = if hole_type == PAD_HOLE_SLOT {
        slot_hole_rotation(rotation, width, height)
    } else {
        rotation
    };
    component.pads.push(PcbPad {
        designator: pad_raw.designator.clone(),
        location: coord_from_easy_units(x, y),
        size_top: coord_from_easy_units(width, height),
        size_middle: coord_from_easy_units(width, height),
        size_bottom: coord_from_easy_units(width, height),
        hole_size_raw: raw_from_mm(hole_mm),
        shape_top: shape,
        shape_middle: shape,
        shape_bottom: shape,
        rotation,
        is_plated: true,
        layer,
        is_locked: false,
        is_tenting_top: false,
        is_tenting_bottom: false,
        is_keepout: false,
        mode: 0,
        power_plane_connect_style: 0,
        relief_air_gap_raw: 0,
        relief_conductor_width_raw: raw_from_mils(10.0),
        relief_entries: 4,
        power_plane_clearance_raw: raw_from_mils(10.0),
        power_plane_relief_expansion_raw: raw_from_mils(20.0),
        paste_mask_expansion_raw: 0,
        solder_mask_expansion_raw,
        drill_type: 0,
        jumper_id: 0,
        hole_type,
        hole_slot_length_raw,
        hole_rotation,
        corner_radius_percentage: DEFAULT_CORNER_RADIUS_PERCENTAGE,
    });
}

fn parse_easyeda_rows(payload: &Value) -> Result<Vec<Vec<Value>>> {
    let data_str = nested_string(payload, &["result", "dataStr"])
        .or_else(|| nested_string(payload, &["dataStr"]))
        .ok_or_else(|| AppError::InvalidResponse("footprint payload has no dataStr".to_string()))?;
    let mut rows = Vec::new();
    for (line_index, line) in data_str.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row_value: Value = serde_json::from_str(trimmed).map_err(|err| {
            AppError::InvalidResponse(format!(
                "invalid EasyEDA dataStr row {}: {err}",
                line_index + 1
            ))
        })?;
        if let Value::Array(row) = row_value {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn parse_footprint_3d_model(payload: &Value) -> Option<Model3dRaw> {
    let model = payload.get("result")?.get("model_3d")?;
    let uri = model.get("uri")?.as_str()?.trim();
    if uri.is_empty() {
        return None;
    }
    let title = model
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let transform = model
        .get("transform")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut result = Model3dRaw {
        title,
        uri: uri.to_string(),
        height_mm: try_parse_height_from_model_title(
            model
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        rotation_x: 0.0,
        rotation_y: 0.0,
        rotation_z: 0.0,
    };
    let parts: Vec<&str> = transform
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    // The transform is "<size x>,<size y>,<size z>,<rot z>,<rot y>,<rot x>,<offset x>,<offset y>,...";
    // the Z rotation is the one EasyEDA applies when it draws the model on the footprint (it matches
    // the footprint's `c_rotation`), so it has to become the 3D body's rotation.
    if parts.len() >= 6 {
        result.rotation_z = parts[3].parse().unwrap_or(0.0);
        result.rotation_y = parts[4].parse().unwrap_or(0.0);
        result.rotation_x = parts[5].parse().unwrap_or(0.0);
    }
    Some(result)
}

fn resolve_component_height_mm(
    payload: &Value,
    component_name: &str,
    model_3d: Option<&Model3dRaw>,
) -> f64 {
    if let Some(height) = model_3d
        .and_then(|model| model.height_mm)
        .filter(|height| *height > 0.000_001)
    {
        return height;
    }

    for candidate in [
        nested_string(payload, &["result", "display_title"]),
        nested_string(payload, &["result", "title"]),
        nested_string(payload, &["result", "package"]),
        Some(component_name.to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(height) = try_parse_height_from_model_title(&candidate) {
            return height;
        }
        if let Some(height) = guess_package_family_height_mm(&candidate) {
            return height;
        }
    }

    1.0
}

fn resolve_footprint_description(payload: &Value, component_name: &str) -> String {
    let preferred = nested_string(payload, &["result", "description"])
        .or_else(|| nested_string(payload, &["description"]))
        .and_then(|text| normalize_footprint_description(&text));

    preferred
        .or_else(|| nested_string(payload, &["result", "display_title"]))
        .or_else(|| nested_string(payload, &["display_title"]))
        .or_else(|| nested_string(payload, &["result", "package"]))
        .or_else(|| nested_string(payload, &["package"]))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| format!("Generated from EasyEDA footprint ({component_name})"))
}

fn normalize_footprint_description(text: &str) -> Option<String> {
    let parts: Vec<String> = text
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn try_parse_height_from_model_title(title: &str) -> Option<f64> {
    let normalized = title.trim().to_ascii_uppercase();
    let mut index = normalized.find("-H").or_else(|| normalized.find("_H"))? + 2;
    let start = index;
    let bytes = normalized.as_bytes();
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
        index += 1;
    }
    (index > start)
        .then(|| normalized[start..index].parse().ok())
        .flatten()
}

fn guess_package_family_height_mm(text: &str) -> Option<f64> {
    let normalized = text.trim().to_ascii_uppercase();
    if normalized.contains("QFN") || normalized.contains("DFN") || normalized.contains("LGA") {
        Some(1.0)
    } else if normalized.contains("BGA") {
        Some(1.2)
    } else if normalized.contains("QFP")
        || normalized.contains("TQFP")
        || normalized.contains("LQFP")
    {
        Some(1.4)
    } else if normalized.contains("SOIC")
        || normalized.contains("SOP")
        || normalized.contains("SSOP")
        || normalized.contains("TSSOP")
        || normalized.contains("MSOP")
    {
        Some(1.6)
    } else if normalized.contains("SOT") {
        Some(1.6)
    } else if normalized.contains("DIP") {
        Some(4.0)
    } else {
        None
    }
}

fn component_body_outline(extents: Extents) -> Vec<CoordPoint> {
    vec![
        coord_from_easy_units(extents.min_x, extents.min_y),
        coord_from_easy_units(extents.max_x, extents.min_y),
        coord_from_easy_units(extents.max_x, extents.max_y),
        coord_from_easy_units(extents.min_x, extents.max_y),
    ]
}

fn choose_step_model_name(component_name: &str, title: &str) -> String {
    let base = if title.trim().is_empty() {
        component_name
    } else {
        title.trim()
    };
    let mut sanitized = sanitize_filename(base);
    if !sanitized.to_ascii_lowercase().ends_with(".step")
        && !sanitized.to_ascii_lowercase().ends_with(".stp")
    {
        sanitized.push_str(".step");
    }
    sanitized
}

fn is_overlay_layer(layer_code: i32) -> bool {
    matches!(layer_code, 3 | 4 | 12 | 13 | 49)
}

fn is_component_body_layer(layer_code: i32) -> bool {
    matches!(layer_code, 48 | 99)
}

fn map_graphic_layer(layer_code: i32) -> u8 {
    match layer_code {
        1 => LAYER_TOP,
        2 => LAYER_BOTTOM,
        3 => LAYER_TOP_OVERLAY,
        4 => LAYER_BOTTOM_OVERLAY,
        5 => super::pcblib::LAYER_TOP_SOLDER,
        6 => super::pcblib::LAYER_BOTTOM_SOLDER,
        7 => super::pcblib::LAYER_TOP_PASTE,
        8 => super::pcblib::LAYER_BOTTOM_PASTE,
        11 | 48 => LAYER_MECHANICAL_1,
        13 => LAYER_MECHANICAL_2,
        49 => LAYER_TOP_OVERLAY,
        50 => LAYER_MECHANICAL_5,
        51 => LAYER_MECHANICAL_6,
        12 => LAYER_MULTI,
        _ => LAYER_TOP_OVERLAY,
    }
}

fn map_pad_layer(layer_code: i32, hole_mm: f64) -> u8 {
    if layer_code == 12 || hole_mm > 0.000_001 {
        LAYER_MULTI
    } else if layer_code == 2 {
        LAYER_BOTTOM
    } else {
        LAYER_TOP
    }
}

fn map_pad_hole_type(name: &str) -> u8 {
    let upper = name.trim().to_ascii_uppercase();
    if upper.contains("SLOT") {
        PAD_HOLE_SLOT
    } else if upper.contains("SQUARE") || upper.contains("RECT") {
        PAD_HOLE_SQUARE
    } else {
        PAD_HOLE_ROUND
    }
}

fn map_pad_shape(name: &str, width: f64, height: f64) -> u8 {
    let upper = name.trim().to_ascii_uppercase();
    if upper.contains("RECT") {
        PAD_SHAPE_ROUNDED_RECTANGLE
    } else if upper.contains("POLY") {
        PAD_SHAPE_RECTANGULAR
    } else if upper.contains("OCT") {
        PAD_SHAPE_OCTAGONAL
    } else if upper.contains("OVAL") {
        PAD_SHAPE_ROUNDED_RECTANGLE
    } else if (width - height).abs() < 0.000_001 {
        PAD_SHAPE_ROUND
    } else {
        PAD_SHAPE_ROUNDED_RECTANGLE
    }
}

fn easy_units_to_mm(value: f64) -> f64 {
    value * FOOTPRINT_UNIT_TO_MM
}
fn raw_from_easy_units(value: f64) -> i32 {
    (value * RAW_PER_MIL)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
fn raw_from_mils(value: f64) -> i32 {
    (value * RAW_PER_MIL)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
fn raw_from_mm(value: f64) -> i32 {
    raw_from_mils(value / FOOTPRINT_UNIT_TO_MM)
}
fn coord_from_easy_units(x: f64, y: f64) -> CoordPoint {
    CoordPoint::new(raw_from_easy_units(x), raw_from_easy_units(y))
}
fn raw_point_to_coord(point: RawPoint) -> CoordPoint {
    coord_from_easy_units(point.x, point.y)
}
fn resolve_graphic_width_raw(width: f64) -> i32 {
    let raw = raw_from_easy_units(width);
    if raw != 0 {
        raw
    } else {
        raw_from_mm(DEFAULT_GRAPHIC_WIDTH_MM)
    }
}
fn normalize_angle(value: f64) -> f64 {
    let mut angle = value % 360.0;
    if angle < 0.0 {
        angle += 360.0;
    }
    angle
}

fn push_alignment_hole(component: &mut PcbComponent, circle: CircleRaw) {
    let diameter = circle.radius * 2.0;
    let size = coord_from_easy_units(diameter, diameter);
    component.pads.push(PcbPad {
        designator: String::new(),
        location: coord_from_easy_units(circle.cx, circle.cy),
        size_top: size,
        size_middle: size,
        size_bottom: size,
        hole_size_raw: raw_from_easy_units(diameter),
        shape_top: PAD_SHAPE_ROUND,
        shape_middle: PAD_SHAPE_ROUND,
        shape_bottom: PAD_SHAPE_ROUND,
        rotation: 0.0,
        is_plated: false,
        layer: LAYER_MULTI,
        is_locked: false,
        is_tenting_top: false,
        is_tenting_bottom: false,
        is_keepout: false,
        mode: 0,
        power_plane_connect_style: 0,
        relief_air_gap_raw: 0,
        relief_conductor_width_raw: raw_from_mils(10.0),
        relief_entries: 4,
        power_plane_clearance_raw: raw_from_mils(10.0),
        power_plane_relief_expansion_raw: raw_from_mils(20.0),
        paste_mask_expansion_raw: 0,
        solder_mask_expansion_raw: 0,
        drill_type: 0,
        jumper_id: 0,
        hole_type: PAD_HOLE_ROUND,
        hole_slot_length_raw: raw_from_easy_units(diameter),
        hole_rotation: 0.0,
        corner_radius_percentage: DEFAULT_CORNER_RADIUS_PERCENTAGE,
    });

    push_alignment_circle_tracks(component, LAYER_TOP_OVERLAY, circle);
    push_alignment_circle_tracks(component, LAYER_MECHANICAL_1, circle);
}

fn push_alignment_circle_tracks(component: &mut PcbComponent, layer: u8, circle: CircleRaw) {
    let points = circle_region(circle.cx, circle.cy, circle.radius);
    if points.len() < 2 {
        return;
    }
    let width_raw =
        resolve_graphic_width_raw(circle.width).max(raw_from_mm(DEFAULT_GRAPHIC_WIDTH_MM));
    for index in 0..points.len() {
        component.tracks.push(PcbTrack {
            layer,
            start: points[index],
            end: points[(index + 1) % points.len()],
            width_raw,
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
            net_index: 0,
            component_index: 0,
        });
    }
}

fn slot_hole_rotation(pad_rotation: f64, pad_width: f64, pad_height: f64) -> f64 {
    let long_axis_offset = if pad_height > pad_width { 90.0 } else { 0.0 };
    normalize_angle(pad_rotation + long_axis_offset)
}

fn row_f64(row: &[Value], index: usize, default: f64) -> f64 {
    value_f64(row.get(index)).unwrap_or(default)
}
fn row_i32(row: &[Value], index: usize, default: i32) -> i32 {
    value_f64(row.get(index))
        .map(|value| value.round() as i32)
        .unwrap_or(default)
}
fn row_string(row: &[Value], index: usize) -> String {
    value_string(row.get(index)).unwrap_or_default()
}
fn value_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}
fn value_string(value: Option<&Value>) -> Option<String> {
    value.and_then(value_to_string)
}

fn try_parse_circle_shape(shape: &Value) -> Option<CircleShape> {
    let array = shape.as_array()?;
    if array.len() < 4 {
        return None;
    }
    if !value_string(array.first())
        .unwrap_or_default()
        .eq_ignore_ascii_case("CIRCLE")
    {
        return None;
    }
    let radius = value_f64(array.get(3))?.abs();
    (radius > 0.000_001).then(|| CircleShape {
        cx: value_f64(array.get(1)).unwrap_or(0.0),
        cy: value_f64(array.get(2)).unwrap_or(0.0),
        radius,
    })
}

fn fill_shape_values(shape_value: &Value) -> Vec<&Value> {
    let Some(array) = shape_value.as_array() else {
        return Vec::new();
    };
    if array.first().and_then(Value::as_array).is_some() {
        array.iter().collect()
    } else {
        vec![shape_value]
    }
}

fn has_matching_circle_marker(circle: CircleRaw, markers: &[CircleRaw]) -> bool {
    markers.iter().any(|marker| {
        (marker.cx - circle.cx).abs() < 0.001
            && (marker.cy - circle.cy).abs() < 0.001
            && marker.radius > 0.0
            && marker.radius <= circle.radius
    })
}

fn add_raw_point(points: &mut Vec<RawPoint>, x: f64, y: f64) {
    if let Some(last) = points.last() {
        if (last.x - x).abs() < 1e-9 && (last.y - y).abs() < 1e-9 {
            return;
        }
    }
    points.push(RawPoint { x, y });
}

fn add_axis_aligned_rect_points(
    points: &mut Vec<RawPoint>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let x2 = x + width;
    let y2 = y - height;
    let left = x.min(x2);
    let right = x.max(x2);
    let top = y.max(y2);
    let bottom = y.min(y2);
    let radius = radius
        .abs()
        .min((right - left).abs() / 2.0)
        .min((top - bottom).abs() / 2.0);

    if radius <= 1e-9 {
        add_raw_point(points, left, top);
        add_raw_point(points, right, top);
        add_raw_point(points, right, bottom);
        add_raw_point(points, left, bottom);
        add_raw_point(points, left, top);
        return;
    }

    const CORNER_SEGMENTS: usize = 6;
    add_raw_point(points, left + radius, top);
    add_raw_point(points, right - radius, top);
    append_corner_arc(
        points,
        right - radius,
        top - radius,
        radius,
        90.0,
        0.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, right, bottom + radius);
    append_corner_arc(
        points,
        right - radius,
        bottom + radius,
        radius,
        0.0,
        -90.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left + radius, bottom);
    append_corner_arc(
        points,
        left + radius,
        bottom + radius,
        radius,
        -90.0,
        -180.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left, top - radius);
    append_corner_arc(
        points,
        left + radius,
        top - radius,
        radius,
        180.0,
        90.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left + radius, top);
}

fn append_corner_arc(
    points: &mut Vec<RawPoint>,
    center_x: f64,
    center_y: f64,
    radius: f64,
    start_degrees: f64,
    end_degrees: f64,
    segments: usize,
) {
    for step in 1..=segments {
        let t = step as f64 / segments as f64;
        let angle = (start_degrees + (end_degrees - start_degrees) * t).to_radians();
        add_raw_point(
            points,
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        );
    }
}

fn append_path_arc_points(points: &mut Vec<RawPoint>, sweep_degrees: f64, end_x: f64, end_y: f64) {
    let Some(start) = points.last().copied() else {
        add_raw_point(points, end_x, end_y);
        return;
    };

    let sweep_abs = sweep_degrees.abs();
    let dx = end_x - start.x;
    let dy = end_y - start.y;
    let chord = dx.hypot(dy);
    if chord <= 1e-9 || sweep_abs <= 1e-9 {
        add_raw_point(points, end_x, end_y);
        return;
    }

    let half_sweep = sweep_abs.to_radians() / 2.0;
    let sin_half = half_sweep.sin();
    let tan_half = half_sweep.tan();
    if sin_half.abs() <= 1e-9 || tan_half.abs() <= 1e-9 {
        add_raw_point(points, end_x, end_y);
        return;
    }

    let radius = chord / (2.0 * sin_half.abs());
    let center_distance = chord / (2.0 * tan_half.abs());
    let mid_x = (start.x + end_x) / 2.0;
    let mid_y = (start.y + end_y) / 2.0;
    let unit_x = dx / chord;
    let unit_y = dy / chord;
    let left_x = -unit_y;
    let left_y = unit_x;
    let direction = if sweep_degrees >= 0.0 { 1.0 } else { -1.0 };
    let center_x = mid_x + left_x * center_distance * direction;
    let center_y = mid_y + left_y * center_distance * direction;
    let start_angle = (start.y - center_y).atan2(start.x - center_x);
    let segments = ((sweep_abs / PATH_ARC_SEGMENT_DEGREES).ceil() as usize).max(1);

    for step in 1..=segments {
        let t = step as f64 / segments as f64;
        let angle = start_angle + sweep_degrees.to_radians() * t;
        add_raw_point(
            points,
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        );
    }

    if let Some(last) = points.last_mut() {
        last.x = end_x;
        last.y = end_y;
    }
}

fn parse_path_raw_points(shape: &Value) -> Vec<RawPoint> {
    let Some(array) = shape.as_array() else {
        return Vec::new();
    };
    if value_string(array.first())
        .unwrap_or_default()
        .eq_ignore_ascii_case("CIRCLE")
    {
        return Vec::new();
    }
    let mut points = Vec::new();
    let mut i = 0usize;
    while i < array.len() {
        if let Some(token) = array[i].as_str() {
            let command = token.trim().to_ascii_uppercase();
            i += 1;
            if command == "L" {
                while i + 1 < array.len() {
                    let Some(x) = value_f64(array.get(i)) else {
                        break;
                    };
                    let Some(y) = value_f64(array.get(i + 1)) else {
                        break;
                    };
                    add_raw_point(&mut points, x, y);
                    i += 2;
                }
            } else if command == "R" {
                let Some(x) = value_f64(array.get(i)) else {
                    continue;
                };
                let Some(y) = value_f64(array.get(i + 1)) else {
                    continue;
                };
                let Some(width) = value_f64(array.get(i + 2)) else {
                    continue;
                };
                let Some(height) = value_f64(array.get(i + 3)) else {
                    continue;
                };
                let radius = value_f64(array.get(i + 4)).unwrap_or(0.0);
                add_axis_aligned_rect_points(&mut points, x, y, width, height, radius);
                i += 5;
            } else if command == "ARC" || command == "A" {
                if i + 2 < array.len() {
                    if let (Some(sweep), Some(x), Some(y)) = (
                        value_f64(array.get(i)),
                        value_f64(array.get(i + 1)),
                        value_f64(array.get(i + 2)),
                    ) {
                        append_path_arc_points(&mut points, sweep, x, y);
                        i += 3;
                    }
                }
            }
            continue;
        }
        if i + 1 < array.len() {
            if let (Some(x), Some(y)) = (value_f64(array.get(i)), value_f64(array.get(i + 1))) {
                add_raw_point(&mut points, x, y);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    points
}

fn circle_region(cx: f64, cy: f64, radius: f64) -> Vec<CoordPoint> {
    (0..CIRCLE_SEGMENTS)
        .map(|index| {
            let angle = (2.0 * std::f64::consts::PI * index as f64) / CIRCLE_SEGMENTS as f64;
            coord_from_easy_units(cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

#[derive(Debug, Clone)]
struct PadRaw {
    designator: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    hole: f64,
    hole_slot: f64,
    hole_shape: String,
    rotation: f64,
    layer_code: i32,
    shape: String,
    polygon_points: Option<Vec<RawPoint>>,
    custom_polygon: bool,
    custom_mask_expansion_units: Option<f64>,
}
#[derive(Debug, Clone)]
struct PolyRaw {
    layer_code: i32,
    width: f64,
    points: Vec<CoordPoint>,
}
#[derive(Debug, Clone, Copy)]
struct CircleRaw {
    layer_code: i32,
    width: f64,
    cx: f64,
    cy: f64,
    radius: f64,
}
#[derive(Debug, Clone, Copy)]
struct ArcRaw {
    layer_code: i32,
    width: f64,
    cx: f64,
    cy: f64,
    radius: f64,
    start_angle: f64,
    end_angle: f64,
}
#[derive(Debug, Clone)]
struct RegionRaw {
    layer_code: i32,
    points: Vec<CoordPoint>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct RawPoint {
    x: f64,
    y: f64,
}
#[derive(Debug, Clone, Copy)]
struct CircleShape {
    cx: f64,
    cy: f64,
    radius: f64,
}
#[derive(Debug, Clone, Copy)]
struct AxisAlignedRect {
    center_x: f64,
    center_y: f64,
    width: f64,
    height: f64,
}
#[derive(Debug, Clone)]
struct Model3dRaw {
    title: String,
    uri: String,
    height_mm: Option<f64>,
    rotation_x: f64,
    rotation_y: f64,
    rotation_z: f64,
}
#[derive(Debug, Clone, Copy)]
struct Extents {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

#[derive(Debug, Default, Clone, Copy)]
struct Bounds {
    min_x: Option<f64>,
    max_x: Option<f64>,
    min_y: Option<f64>,
    max_y: Option<f64>,
}
impl Bounds {
    fn update_span(&mut self, x1: f64, x2: f64, y1: f64, y2: f64) {
        self.update_x(x1.min(x2), x1.max(x2));
        self.update_y(y1.min(y2), y1.max(y2));
    }
    fn update_from_raw_points(&mut self, points: &[RawPoint]) {
        for point in points {
            self.update_span(point.x, point.x, point.y, point.y);
        }
    }
    fn update_x(&mut self, min: f64, max: f64) {
        self.min_x = Some(self.min_x.map_or(min, |value| value.min(min)));
        self.max_x = Some(self.max_x.map_or(max, |value| value.max(max)));
    }
    fn update_y(&mut self, min: f64, max: f64) {
        self.min_y = Some(self.min_y.map_or(min, |value| value.min(min)));
        self.max_y = Some(self.max_y.map_or(max, |value| value.max(max)));
    }
    fn finish(self) -> Option<Extents> {
        Some(Extents {
            min_x: self.min_x?,
            max_x: self.max_x?,
            min_y: self.min_y?,
            max_y: self.max_y?,
        })
    }
    fn from_raw_points(points: &[RawPoint]) -> Option<Extents> {
        let mut bounds = Bounds::default();
        bounds.update_from_raw_points(points);
        bounds.finish()
    }
}

/// A polygon's electrical pad also supplies Altium's on-screen designator.
/// The former fixed 0.06 mm hotspot made that label effectively invisible.
/// Fit a round pad inside the copper, leaving the exact outline and masks to
/// their existing regions. Do not add silkscreen/copper text as a fake label.
fn custom_pad_anchor(points: &[RawPoint], original: RawPoint) -> (RawPoint, f64) {
    let Some(bounds) = Bounds::from_raw_points(points) else {
        return (original, CUSTOM_PAD_HOTSPOT_UNITS);
    };
    let clearance = |point: RawPoint| -> f64 {
        if !inside_polygon(point, points) {
            return 0.0;
        }
        points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
            .map(|(&a, &b)| distance_to_segment(point, a, b))
            .fold(f64::INFINITY, f64::min)
    };
    let mut center = original;
    let mut radius = clearance(original);
    // Include the original anchor and a grid across the entire outline so a
    // narrow terminal neck does not constrain the label in a wider pad body.
    for iy in 0..=16 {
        for ix in 0..=16 {
            let point = RawPoint {
                x: bounds.min_x + (bounds.max_x - bounds.min_x) * ix as f64 / 16.0,
                y: bounds.min_y + (bounds.max_y - bounds.min_y) * iy as f64 / 16.0,
            };
            let candidate = clearance(point);
            if candidate > radius {
                center = point;
                radius = candidate;
            }
        }
    }
    if radius.is_finite() && radius > 0.001 {
        // Leave clearance for integer coordinate rounding in the PcbLib writer.
        (center, radius * 2.0 * 0.98)
    } else {
        (original, CUSTOM_PAD_HOTSPOT_UNITS)
    }
}

fn inside_polygon(point: RawPoint, points: &[RawPoint]) -> bool {
    let mut inside = false;
    for (&a, &b) in points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
    {
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

fn distance_to_segment(point: RawPoint, a: RawPoint, b: RawPoint) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 0.0 {
        (((point.x - a.x) * dx + (point.y - a.y) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point.x - a.x - t * dx).hypot(point.y - a.y - t * dy)
}

fn pad_outline_region(pad: &PadRaw) -> Option<Vec<CoordPoint>> {
    if map_pad_shape(&pad.shape, pad.width, pad.height) == PAD_SHAPE_ROUNDED_RECTANGLE {
        return rounded_rect_pad_outline(
            pad.x,
            pad.y,
            pad.width,
            pad.height,
            pad.rotation,
            DEFAULT_CORNER_RADIUS_PERCENTAGE,
        );
    }
    if let Some(points) = &pad.polygon_points {
        let mut outline: Vec<CoordPoint> = points.iter().copied().map(raw_point_to_coord).collect();
        if outline.first() != outline.last() {
            if let Some(first) = outline.first().copied() {
                outline.push(first);
            }
        }
        return (outline.len() >= 4).then_some(outline);
    }
    rectangular_pad_outline(pad.x, pad.y, pad.width, pad.height, pad.rotation)
}

fn axis_aligned_rect_from_points(points: &[RawPoint]) -> Option<AxisAlignedRect> {
    let mut unique = Vec::new();
    for point in points {
        if unique
            .iter()
            .any(|existing| same_raw_point(*existing, *point))
        {
            continue;
        }
        unique.push(*point);
    }
    if unique.len() != 4 {
        return None;
    }

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for point in &unique {
        push_unique_f64(&mut xs, point.x);
        push_unique_f64(&mut ys, point.y);
    }
    if xs.len() != 2 || ys.len() != 2 {
        return None;
    }
    xs.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

    for x in &xs {
        for y in &ys {
            if !unique
                .iter()
                .any(|point| same_f64(point.x, *x) && same_f64(point.y, *y))
            {
                return None;
            }
        }
    }

    let width = xs[1] - xs[0];
    let height = ys[1] - ys[0];
    (width > 0.000_001 && height > 0.000_001).then_some(AxisAlignedRect {
        center_x: (xs[0] + xs[1]) / 2.0,
        center_y: (ys[0] + ys[1]) / 2.0,
        width,
        height,
    })
}

fn push_unique_f64(values: &mut Vec<f64>, value: f64) {
    if values.iter().any(|existing| same_f64(*existing, value)) {
        return;
    }
    values.push(value);
}

fn same_raw_point(left: RawPoint, right: RawPoint) -> bool {
    same_f64(left.x, right.x) && same_f64(left.y, right.y)
}

fn same_f64(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-6
}

fn rectangular_pad_outline(
    center_x: f64,
    center_y: f64,
    width: f64,
    height: f64,
    rotation_degrees: f64,
) -> Option<Vec<CoordPoint>> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let half_w = width / 2.0;
    let half_h = height / 2.0;
    let angle = normalize_angle(rotation_degrees).to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let corners = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    let mut outline = Vec::with_capacity(5);
    for (dx, dy) in corners {
        let x = center_x + dx * cos - dy * sin;
        let y = center_y + dx * sin + dy * cos;
        outline.push(coord_from_easy_units(x, y));
    }
    outline.push(outline[0]);
    Some(outline)
}

fn rounded_rect_pad_outline(
    center_x: f64,
    center_y: f64,
    width: f64,
    height: f64,
    rotation_degrees: f64,
    corner_radius_percentage: u8,
) -> Option<Vec<CoordPoint>> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let half_w = width / 2.0;
    let half_h = height / 2.0;
    let max_radius = half_w.min(half_h);
    let radius =
        (width.min(height) * f64::from(corner_radius_percentage) / 200.0).clamp(0.0, max_radius);
    if radius <= 1e-9 {
        return rectangular_pad_outline(center_x, center_y, width, height, rotation_degrees);
    }

    let angle = normalize_angle(rotation_degrees).to_radians();
    let (sin, cos) = angle.sin_cos();
    let mut outline = Vec::with_capacity(ROUNDED_RECT_CORNER_SEGMENTS * 4 + 5);

    let mut push_local = |local_x: f64, local_y: f64| {
        let x = center_x + local_x * cos - local_y * sin;
        let y = center_y + local_x * sin + local_y * cos;
        let point = coord_from_easy_units(x, y);
        if outline.last().copied() != Some(point) {
            outline.push(point);
        }
    };

    push_local(-half_w + radius, -half_h);
    push_local(half_w - radius, -half_h);
    push_rounded_corner(
        &mut push_local,
        half_w - radius,
        -half_h + radius,
        radius,
        -90.0,
        0.0,
    );
    push_local(half_w, half_h - radius);
    push_rounded_corner(
        &mut push_local,
        half_w - radius,
        half_h - radius,
        radius,
        0.0,
        90.0,
    );
    push_local(-half_w + radius, half_h);
    push_rounded_corner(
        &mut push_local,
        -half_w + radius,
        half_h - radius,
        radius,
        90.0,
        180.0,
    );
    push_local(-half_w, -half_h + radius);
    push_rounded_corner(
        &mut push_local,
        -half_w + radius,
        -half_h + radius,
        radius,
        180.0,
        270.0,
    );

    if let Some(first) = outline.first().copied() {
        outline.push(first);
    }
    Some(outline)
}

fn push_rounded_corner(
    push_local: &mut impl FnMut(f64, f64),
    center_x: f64,
    center_y: f64,
    radius: f64,
    start_degrees: f64,
    end_degrees: f64,
) {
    for step in 1..=ROUNDED_RECT_CORNER_SEGMENTS {
        let t = step as f64 / ROUNDED_RECT_CORNER_SEGMENTS as f64;
        let angle = (start_degrees + (end_degrees - start_degrees) * t).to_radians();
        push_local(
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        );
    }
}

fn pad_shape_region_params(v7_layer: &str) -> Vec<(String, String)> {
    vec![
        ("V7_LAYER".to_string(), v7_layer.to_string()),
        ("SUBPOLYINDEX".to_string(), "-1".to_string()),
        ("UNIONINDEX".to_string(), "0".to_string()),
        ("ARCRESOLUTION".to_string(), "0.5mil".to_string()),
        ("ISSHAPEBASED".to_string(), "FALSE".to_string()),
        ("CAVITYHEIGHT".to_string(), "0mil".to_string()),
    ]
}

/// Native PcbLib custom-pad regions reference the 1-based pad record ordinal,
/// not the designator or the index among all primitives. The editable contour
/// accompanies the binary region outline; coordinates are absolute mils.
fn custom_pad_region_params(
    v7_layer: &str,
    pad_index: usize,
    outline: &[CoordPoint],
) -> Vec<(String, String)> {
    let mut params = pad_shape_region_params(v7_layer);
    params
        .iter_mut()
        .find(|(key, _)| key == "ISSHAPEBASED")
        .unwrap()
        .1 = "TRUE".to_string();
    params.push(("KIND".to_string(), "0".to_string()));
    params.push(("PADINDEX".to_string(), pad_index.to_string()));
    // Altium closes the editable contour implicitly. Keep the original binary
    // outline unchanged, including any closing point supplied by the source.
    let vertices = if outline.len() > 1 && outline.first() == outline.last() {
        &outline[..outline.len() - 1]
    } else {
        outline
    };
    params.push((
        "MAINCONTOURVERTEXCOUNT".to_string(),
        vertices.len().to_string(),
    ));
    for (index, point) in vertices.iter().enumerate() {
        // Curves have already been tessellated by the source path parser.
        // Four decimal places preserve the writer's 1/10000 mil resolution.
        for (key, value) in [
            ("KIND", "0".to_string()),
            ("VX", format!("{:.4}mil", f64::from(point.x) / RAW_PER_MIL)),
            ("VY", format!("{:.4}mil", f64::from(point.y) / RAW_PER_MIL)),
            ("CX", "0mil".to_string()),
            ("CY", "0mil".to_string()),
            ("SA", "0.00000000000000E+0000".to_string()),
            ("EA", "0.00000000000000E+0000".to_string()),
            ("R", "0mil".to_string()),
        ] {
            params.push((format!("{key}{index}"), value));
        }
    }
    params
}

fn mask_region_layers(layer_code: i32, hole_mm: f64) -> Vec<(u8, &'static str)> {
    if layer_code == 2 {
        vec![(LAYER_BOTTOM_SOLDER, "BOTTOMSOLDER")]
    } else if layer_code == 12 || hole_mm > 0.000_001 {
        vec![
            (LAYER_TOP_SOLDER, "TOPSOLDER"),
            (LAYER_BOTTOM_SOLDER, "BOTTOMSOLDER"),
        ]
    } else {
        vec![(LAYER_TOP_SOLDER, "TOPSOLDER")]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_CORNER_RADIUS_PERCENTAGE, ROUNDED_RECT_CORNER_SEGMENTS, RawPoint,
        build_pcblib_from_payload, mask_region_layers, normalize_footprint_description,
        parse_path_raw_points, rectangular_pad_outline,
    };
    use crate::altium::pcblib::{
        LAYER_BOTTOM_SOLDER, LAYER_MECHANICAL_2, LAYER_MECHANICAL_9, LAYER_MULTI, LAYER_TOP,
        LAYER_TOP_OVERLAY, LAYER_TOP_SOLDER, PAD_HOLE_ROUND, PAD_HOLE_SLOT, PAD_SHAPE_ROUND,
    };
    use serde_json::json;

    #[test]
    fn builds_pcblib_primitives_from_easyeda_footprint() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",10,20,90,null,["OVAL",30,40],[],0,0,0,1]
["TRACK","t1",0,"",3,6,0,0,10,0]
["CIRCLE","c1",0,"",3,2,5,5,2]
["FILL","f1",0,"",49,0.2,0,[[0,0,"L",10,0,10,10,0,10,0,0]],0]"#, "model_3d": {"title":"BODY-H1.2", "uri":"modeluuid", "transform":"0,0,0,1,2,3"}}});
        let library =
            build_pcblib_from_payload(&payload, "TEST", Some(b"ISO-10303-21;END-ISO-10303-21;"))
                .unwrap();
        let component = &library.components[0];
        assert_eq!(component.pads.len(), 1);
        assert_eq!(component.tracks.len(), 1);
        assert_eq!(component.arcs.len(), 1);
        assert_eq!(component.regions.len(), 2);
        assert!(
            component
                .regions
                .iter()
                .any(|region| region.layer == LAYER_MECHANICAL_9)
        );
        assert_eq!(component.bodies.len(), 1);
        assert_eq!(library.models.len(), 1);
    }

    #[test]
    fn applies_easyeda_model_rotation_to_the_component_body() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]"#,
            "model_3d": {"title":"BODY-H1.2", "uri":"modeluuid", "transform":"236.22,192.913,0,90,0,0,0,0,0"}}});
        let library =
            build_pcblib_from_payload(&payload, "SOP-8", Some(b"ISO-10303-21;END-ISO-10303-21;"))
                .unwrap();
        let body = &library.components[0].bodies[0];

        // EasyEDA stores 90 degrees; Altium's footprint frame is the Y mirror of EasyEDA's, so the
        // body keeps that angle (KiCad, whose frame matches EasyEDA's, gets 270 instead).
        assert_eq!(body.model_3d_rot_z, 90.0);
        assert_eq!(library.models[0].rotation_z, 90.0);
    }

    #[test]
    fn keeps_unrotated_models_unrotated() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]"#,
            "model_3d": {"title":"BODY-H1.2", "uri":"modeluuid", "transform":"192.913,236.22,0,0,0,0,0,0,0"}}});
        let library =
            build_pcblib_from_payload(&payload, "SOIC-8", Some(b"ISO-10303-21;END-ISO-10303-21;"))
                .unwrap();
        let body = &library.components[0].bodies[0];

        assert_eq!(body.model_3d_rot_z, 0.0);
    }

    #[test]
    fn places_a_real_rotated_model_on_its_pads() {
        // C668205 (SOP-8_L5.0-W4.0) stores its model at 90 degrees. Rendering the exported KiCad
        // footprint with `kicad-cli pcb render` shows the leads only reaching the pads with a 270
        // degree rotation in KiCad's frame, which is the 90 degrees Altium's mirrored frame needs.
        let payload: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/lceda_footprint_c668205.json"
        ))
        .unwrap();
        let library =
            build_pcblib_from_payload(&payload, "SOP-8", Some(b"ISO-10303-21;END-ISO-10303-21;"))
                .unwrap();
        let body = &library.components[0].bodies[0];

        assert_eq!(body.model_3d_rot_z, 90.0);
        assert_eq!(library.models[0].rotation_z, 90.0);
    }

    #[test]
    fn binds_custom_pad_mask_expansion_to_the_copper_region() {
        let payload: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/lceda_footprint_c29073231.json"
        ))
        .unwrap();
        let library =
            build_pcblib_from_payload(&payload, "QFN-19_L5.0-W3.0_CD4583GQVE", None).unwrap();
        let component = &library.components[0];

        // C29073231 has five irregular (polygon) pads: 1, 6, 7, 9 and 15.
        let copper_regions: Vec<&crate::altium::pcblib::PcbRegion> = component
            .regions
            .iter()
            .filter(|region| region.layer == LAYER_TOP)
            .collect();
        let mask_regions: Vec<&crate::altium::pcblib::PcbRegion> = component
            .regions
            .iter()
            .filter(|region| region.layer == LAYER_TOP_SOLDER)
            .collect();
        assert_eq!(copper_regions.len(), 5);
        assert!(
            mask_regions.is_empty(),
            "explicit mask shapes would override expansion"
        );

        let region_primitive_start =
            component.pads.len() + component.tracks.len() + component.arcs.len();
        assert_eq!(
            component.extended_primitive_information.len(),
            copper_regions.len()
        );
        for info in &component.extended_primitive_information {
            let region = &component.regions[info.primitive_index - region_primitive_start];
            assert_eq!(
                region.layer, LAYER_TOP,
                "the mask entry must reference the pad copper region, not its mask region"
            );
            assert_eq!(info.object_name, "Region");
            assert!(
                info.params
                    .contains(&("TYPE".to_string(), "Mask".to_string()))
            );
            assert!(info.params.contains(&(
                "SOLDERMASKEXPANSION_MANUAL".to_string(),
                "1.9685mil".to_string()
            )));
            assert!(
                info.params
                    .contains(&("PASTEMASKEXPANSIONMODE".to_string(), "None".to_string()))
            );
            let pad_index: usize = region
                .additional_params
                .iter()
                .find(|(key, _)| key == "PADINDEX")
                .unwrap()
                .1
                .parse()
                .unwrap();
            assert_eq!(
                component.pads[pad_index - 1].solder_mask_expansion_raw,
                19_685
            );
        }
    }

    #[test]
    fn custom_pad_mask_regions_use_solder_layers() {
        assert_eq!(
            mask_region_layers(1, 0.0),
            vec![(LAYER_TOP_SOLDER, "TOPSOLDER")]
        );
        assert_eq!(
            mask_region_layers(2, 0.0),
            vec![(LAYER_BOTTOM_SOLDER, "BOTTOMSOLDER")]
        );
        assert_eq!(
            mask_region_layers(12, 0.0),
            vec![
                (LAYER_TOP_SOLDER, "TOPSOLDER"),
                (LAYER_BOTTOM_SOLDER, "BOTTOMSOLDER")
            ]
        );
    }

    #[test]
    fn skips_body_when_step_model_is_missing() {
        let payload = json!({"result": {"display_title": "QFN-60_L7.0-W7.0-P0.40-TL-EP3.4", "dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]
["POLY","body",0,"",48,2,[-120,120,"L",120,120,120,-120,-120,-120,-120,120],0]"#}});
        let library = build_pcblib_from_payload(&payload, "RP2350A_C42411118", None).unwrap();
        let component = &library.components[0];
        assert_eq!(library.models.len(), 0);
        assert_eq!(component.bodies.len(), 0);
    }

    #[test]
    fn embeds_the_step_model_resting_on_the_board_plane() {
        // A model centred on Z, like C2904739 (SOP-16) and C9864 (ESOP-8), whose geometry sinks
        // about 0.8 mm into the board when it is embedded untouched.
        let mut step = String::from(
            "ISO-10303-21;\nDATA;\n#900 = CARTESIAN_POINT ( 'NONE',  ( 0.0, 0.0, 1.2 ) ) ;\n",
        );
        for index in 0..40 {
            step.push_str(&format!(
                "#{index} = CARTESIAN_POINT ( 'NONE',  ( 0.0, 0.0, -0.83 ) ) ;\n"
            ));
        }
        step.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]"#,
            "model_3d": {"title":"BODY-H1.2", "uri":"modeluuid", "transform":"192.913,236.22,0,0,0,0,0,0,0"}}});

        let library = build_pcblib_from_payload(&payload, "ESOP-8", Some(step.as_bytes())).unwrap();
        let embedded = library.models[0].step_data.read().unwrap().into_owned();
        let embedded = String::from_utf8(embedded).unwrap();

        assert!(!embedded.contains("-0.83"));
        assert!(embedded.contains("0.000000000"));
        assert!(embedded.contains("2.030000000"));
    }

    #[test]
    fn embeds_resolved_step_without_payload_model_metadata() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]"#}});
        let library = build_pcblib_from_payload(
            &payload,
            "MODEL_FALLBACK",
            Some(b"ISO-10303-21;END-ISO-10303-21;"),
        )
        .unwrap();

        assert_eq!(library.models.len(), 1);
        assert_eq!(library.components[0].bodies.len(), 1);
        assert_eq!(library.models[0].name, "MODEL_FALLBACK.step");
    }

    #[test]
    fn builds_rotated_rectangular_outline_regions() {
        let outline = rectangular_pad_outline(-57.09, 19.38, 11.811, 39.37, 90.0).unwrap();
        assert_eq!(outline.len(), 5);
        assert_eq!(outline.first(), outline.last());
    }

    #[test]
    fn exports_poly_pad_with_native_copper_and_generated_mask() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"20",-39.37,61.39,0,null,["POLY",[-45.315,76.467,"L",-45.315,51.985,-37.945,44.948,-33.474,44.863,-33.419,76.471,-45.315,76.467]],[],0.003,-0.003,90,1,0,1.9689999999999999,1.9689999999999999,0,0,0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "UFQFPN-20_L3.0-W3.0-P0.50-TL", None).unwrap();
        let component = &library.components[0];
        assert_eq!(component.pads.len(), 1);
        assert_eq!(component.pads[0].shape_top, PAD_SHAPE_ROUND);
        assert_eq!(component.regions.len(), 2);
        assert!(
            component
                .regions
                .iter()
                .any(|region| region.layer == LAYER_MECHANICAL_9 && region.outline.len() == 6)
        );
        assert!(
            component
                .regions
                .iter()
                .any(|region| region.layer == LAYER_TOP)
        );
        assert_eq!(component.extended_primitive_information.len(), 1);
        assert_eq!(
            component.extended_primitive_information[0].primitive_index,
            component.pads.len() + component.tracks.len() + component.arcs.len() + 1
        );
    }

    #[test]
    fn exports_rectangular_poly_pad_as_rounded_rectangular_pad() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e8",0,"",1,"1",-28,0.2,0,null,["POLY",[-40.005,9.698,"L",-16.005,9.698,-16.005,-9.302,-40.005,-9.302,-40.005,9.698]],[],-0.005,-0.003,0,1,0,null,null,null,null,0]
["PAD","e9",0,"",1,"2",28,-0.2,0,null,["POLY",[15.995,9.304,"L",39.995,9.304,39.995,-9.696,15.995,-9.696,15.995,9.304]],[],-0.005,0.004,0,1,0,null,null,null,null,0]"#}});
        let library = build_pcblib_from_payload(&payload, "SOD-323", None).unwrap();
        let component = &library.components[0];

        assert_eq!(component.pads.len(), 2);
        assert!(
            component
                .pads
                .iter()
                .all(|pad| pad.shape_top == crate::altium::pcblib::PAD_SHAPE_ROUNDED_RECTANGLE)
        );
        assert!(
            component
                .pads
                .iter()
                .all(|pad| pad.corner_radius_percentage == DEFAULT_CORNER_RADIUS_PERCENTAGE)
        );
        assert_eq!(component.pads[0].size_top.x, 240_000);
        assert_eq!(component.pads[0].size_top.y, 190_000);
        assert_eq!(component.pads[0].location.x, -280_050);
        assert_eq!(component.pads[0].location.y, 1_980);
        assert_eq!(component.regions.len(), 4);
        assert!(
            component
                .regions
                .iter()
                .filter(|region| region.layer == LAYER_MECHANICAL_9)
                .all(|region| region.outline.len() > 5)
        );
        assert!(
            component
                .regions
                .iter()
                .filter(|region| region.layer == LAYER_MECHANICAL_9)
                .all(|region| region.outline.len() == ROUNDED_RECT_CORNER_SEGMENTS * 4 + 6)
        );
        // These polygon pads are plain rectangles, so they become real rounded rectangle
        // pads that already carry the solder mask expansion themselves: no copper region
        // (and therefore no mask entry) is needed for them.
        assert_eq!(component.extended_primitive_information.len(), 0);
        assert!(
            component
                .regions
                .iter()
                .filter(|region| region.layer == LAYER_TOP_SOLDER)
                .count()
                == 2
        );
    }

    #[test]
    fn maps_easyeda_slot_pad_to_altium_slot_drill() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e56",0,"",12,"13",-170.275,71.855,0,["SLOT",59.055,23.622],["OVAL",43.307,78.74],[],0.002,-0.003,90,1,0,1.9689999999999999,1.9689999999999999,0,0,0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "USB-C-SMD_TYPE-C-16PIN-2MD-073", None).unwrap();
        let pad = &library.components[0].pads[0];

        assert_eq!(pad.hole_type, PAD_HOLE_SLOT);
        assert_eq!(pad.hole_size_raw, 236_220);
        assert_eq!(pad.hole_slot_length_raw, 590_550);
        assert_eq!(pad.hole_rotation, 90.0);
    }

    #[test]
    fn maps_marked_multilayer_fill_circle_to_unplated_alignment_hole() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["POLY","e2",0,"",13,9.843,["CIRCLE",-113.775,51.375,4.92],0]
["FILL","e54",0,"",12,0.2,0,["CIRCLE",-113.775,51.375,13.78],0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "USB-C-SMD_TYPE-C-16PIN-2MD-073", None).unwrap();
        let component = &library.components[0];
        let pad = &component.pads[0];

        assert_eq!(component.pads.len(), 1);
        assert_eq!(pad.layer, LAYER_MULTI);
        assert!(!pad.is_plated);
        assert_eq!(pad.hole_type, PAD_HOLE_ROUND);
        assert_eq!(pad.hole_size_raw, 275_600);
        assert_eq!(pad.size_top.x, 275_600);
        assert_eq!(component.arcs.len(), 1);
        assert!(
            component
                .tracks
                .iter()
                .filter(|track| track.layer == LAYER_TOP_OVERLAY)
                .count()
                >= 32
        );
    }

    #[test]
    fn maps_unmarked_nested_multilayer_fill_circle_to_multilayer_region() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["FILL","e55",0,"",12,0.2,0,[["CIRCLE",113.785,51.375,13.78]],0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "USB-C-SMD_TYPE-C-16PIN-2MD-073", None).unwrap();
        let component = &library.components[0];

        assert_eq!(component.pads.len(), 0);
        assert_eq!(component.regions.len(), 1);
        assert!(
            component
                .regions
                .iter()
                .any(|region| region.layer == LAYER_MULTI && region.outline.len() == 32)
        );
    }

    #[test]
    fn maps_document_circle_to_mechanical_arc() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["POLY","e2",0,"",13,9.843,["CIRCLE",-113.775,51.375,4.92],0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "USB-C-SMD_TYPE-C-16PIN-2MD-073", None).unwrap();
        let component = &library.components[0];
        let arc = &component.arcs[0];

        assert_eq!(component.arcs.len(), 1);
        assert_eq!(arc.layer, LAYER_MECHANICAL_2);
        assert_eq!(arc.radius_raw, 49_200);
        assert_eq!(arc.width_raw, 98_430);
    }

    #[test]
    fn c2765186_exports_two_alignment_hole_markers() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["POLY","e2",0,"",13,9.843,["CIRCLE",-113.775,51.375,4.92],0]
["POLY","e3",0,"",13,9.843,["CIRCLE",113.785,51.375,4.92],0]
["FILL","e54",0,"",12,0.2,0,["CIRCLE",-113.775,51.375,13.78],0]
["FILL","e55",0,"",12,0.2,0,["CIRCLE",113.785,51.375,13.78],0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "USB-C-SMD_TYPE-C-16PIN-2MD-073", None).unwrap();
        let component = &library.components[0];
        let alignment_pads: Vec<_> = component
            .pads
            .iter()
            .filter(|pad| {
                pad.layer == LAYER_MULTI && !pad.is_plated && pad.hole_size_raw == 275_600
            })
            .collect();
        let alignment_arcs: Vec<_> = component
            .tracks
            .iter()
            .filter(|track| {
                track.layer == LAYER_TOP_OVERLAY
                    && (track.start.x + 1_137_750).abs() < 140_000
                    && (track.start.y - 513_750).abs() < 140_000
            })
            .collect();

        assert_eq!(alignment_pads.len(), 2);
        assert_eq!(alignment_arcs.len(), 32);
        assert!(
            alignment_pads
                .iter()
                .any(|pad| pad.location.x == -1_137_750 && pad.location.y == 513_750)
        );
        assert!(
            alignment_pads
                .iter()
                .any(|pad| pad.location.x == 1_137_850 && pad.location.y == 513_750)
        );
    }

    #[test]
    fn normalizes_semicolon_footprint_description() {
        assert_eq!(
            normalize_footprint_description(";UFQFPN-20(3x3);UFQFPN-20;UFQFPN-20_L3.0-W3.0-P0.50"),
            Some("UFQFPN-20(3x3); UFQFPN-20; UFQFPN-20_L3.0-W3.0-P0.50".to_string())
        );
    }

    #[test]
    fn parses_rectangular_r_path_command() {
        let shape = json!(["R", -100.001, 35, 200, 70, 0]);
        let points = parse_path_raw_points(&shape);
        assert_eq!(points.len(), 5);
        assert_eq!(
            points,
            vec![
                RawPoint {
                    x: -100.001,
                    y: 35.0
                },
                RawPoint { x: 99.999, y: 35.0 },
                RawPoint {
                    x: 99.999,
                    y: -35.0
                },
                RawPoint {
                    x: -100.001,
                    y: -35.0
                },
                RawPoint {
                    x: -100.001,
                    y: 35.0
                },
            ]
        );
    }

    #[test]
    fn expands_easyeda_path_arc_commands() {
        let shape = json!([0, 0, "ARC", 180, 10, 0]);
        let points = parse_path_raw_points(&shape);

        assert!(points.len() > 3);
        assert_eq!(points.first(), Some(&RawPoint { x: 0.0, y: 0.0 }));
        assert_eq!(points.last(), Some(&RawPoint { x: 10.0, y: 0.0 }));
        assert!(
            points
                .iter()
                .skip(1)
                .take(points.len().saturating_sub(2))
                .any(|point| point.y.abs() > 1.0)
        );
    }

    #[test]
    fn expands_c41430892_top_overlay_arc_instead_of_chord() {
        let shape = json!([285.274, 177.846, "ARC", 90, 2.486, 460.634]);
        let points = parse_path_raw_points(&shape);

        assert!(points.len() > 3);
        assert_eq!(
            points.first(),
            Some(&RawPoint {
                x: 285.274,
                y: 177.846
            })
        );
        assert_eq!(
            points.last(),
            Some(&RawPoint {
                x: 2.486,
                y: 460.634
            })
        );
        assert!(
            points
                .iter()
                .skip(1)
                .take(points.len().saturating_sub(2))
                .any(|point| point.x > 2.486 && point.x < 285.274 && point.y > 177.846)
        );
    }
}

#[cfg(test)]
mod custom_pad_tests {
    use super::*;
    use serde_json::json;

    fn sot89_like_payload() -> Value {
        // A narrow lead joins a wide irregular body, with the upstream pad
        // anchor in the lead. The label must remain a real pad designator.
        let polygon = json!([
            -40, -8, "L", -10, -8, 0, -25, 50, -25, 65, -10, 65, 10, 50, 25, 0, 25, -10, 8, -40, 8,
            -40, -8
        ]);
        let rows = [
            json!([
                "PAD",
                "p1",
                0,
                "",
                1,
                "1",
                -30,
                40,
                0,
                null,
                ["RECT", 24, 10],
                []
            ]),
            json!([
                "PAD",
                "p2",
                0,
                "",
                1,
                "2",
                -30,
                0,
                0,
                null,
                ["POLY", polygon],
                []
            ]),
            json!([
                "PAD",
                "p3",
                0,
                "",
                1,
                "3",
                -30,
                -40,
                0,
                null,
                ["RECT", 24, 10],
                []
            ]),
        ];
        json!({"result":{"dataStr":rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")}})
    }

    // Decode the exported binary independently of the in-memory region model.
    // This checks the actual ownership and editable geometry Altium receives.
    fn assert_native_custom_regions(
        data: &[u8],
        expected_owners: &[(&str, u8)],
        expected_solder_expansion: i32,
    ) {
        let u32_at = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
        let end_block = |offset| offset + 4 + (u32_at(offset) & 0x00ff_ffff) as usize;
        let mut offset = end_block(0);
        let mut pads = Vec::new();
        let mut pad_masks = Vec::new();
        let mut owners = Vec::new();
        while offset < data.len() {
            let kind = data[offset];
            offset += 1;
            if kind == 2 {
                let length = usize::from(data[offset + 4]);
                pads.push(std::str::from_utf8(&data[offset + 5..offset + 5 + length]).unwrap());
                let mut geometry = offset;
                for _ in 0..4 {
                    geometry = end_block(geometry);
                }
                let payload = geometry + 4;
                pad_masks.push((
                    i32::from_le_bytes(data[payload + 90..payload + 94].try_into().unwrap()),
                    data[payload + 102],
                ));
            } else if kind == 11 {
                let payload = offset + 4;
                let layer = data[payload];
                assert!(
                    ![LAYER_TOP_SOLDER, LAYER_BOTTOM_SOLDER].contains(&layer),
                    "an explicit solder contour would override the pad's manual expansion"
                );
                let params_start = payload + 22;
                let params_end = params_start + u32_at(payload + 18) as usize;
                let text = std::str::from_utf8(&data[params_start..params_end]).unwrap();
                let params: std::collections::HashMap<_, _> = text
                    .trim_end_matches('\0')
                    .split('|')
                    .filter_map(|pair| pair.split_once('='))
                    .collect();
                if let Some(index) = params.get("PADINDEX") {
                    let index: usize = index.parse().unwrap();
                    assert!(index > 0 && index <= pads.len());
                    owners.push((pads[index - 1], layer));
                    assert_eq!(
                        pad_masks[index - 1],
                        (expected_solder_expansion, 2),
                        "native custom pads must store manual solder-mask expansion"
                    );
                    assert_eq!(params["ISSHAPEBASED"], "TRUE");
                    assert!(
                        !params.contains_key("NET"),
                        "a footprint must inherit its board net"
                    );
                    let count: usize = params["MAINCONTOURVERTEXCOUNT"].parse().unwrap();
                    let binary_count = u32_at(params_end) as usize;
                    assert!(count >= 3 && (binary_count == count || binary_count == count + 1));
                    for vertex in 0..count {
                        assert_eq!(params[format!("KIND{vertex}").as_str()], "0");
                        for (axis, delta) in [("X", 0), ("Y", 8)] {
                            let pos = params_end + 4 + vertex * 16 + delta;
                            let binary = f64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
                            let token = params[format!("V{axis}{vertex}").as_str()];
                            let mils: f64 = token.strip_suffix("mil").unwrap().parse().unwrap();
                            assert!((mils * 10_000.0 - binary).abs() < 0.001);
                        }
                    }
                } else {
                    assert!(!params.contains_key("MAINCONTOURVERTEXCOUNT"));
                }
            }
            for _ in 0..if kind == 2 { 6 } else { 1 } {
                offset = end_block(offset);
            }
        }
        assert_eq!(owners, expected_owners);
    }

    #[test]
    fn irregular_pad_has_readable_real_designator_and_unchanged_copper() {
        let library =
            build_pcblib_from_payload(&sot89_like_payload(), "SOT89-label-regression", None)
                .unwrap();
        let component = &library.components[0];
        assert_eq!(
            component
                .pads
                .iter()
                .map(|p| p.designator.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
        let pad = &component.pads[1];
        assert_eq!(pad.shape_top, PAD_SHAPE_ROUND);
        assert!(pad.size_top.x > raw_from_easy_units(30.0));
        assert!(
            pad.location.x > 0,
            "move out of the narrow lead into the wide copper body"
        );
        let copper = component
            .regions
            .iter()
            .find(|r| r.layer == LAYER_TOP)
            .unwrap();
        assert_eq!(copper.outline[0], coord_from_easy_units(-40.0, -8.0));
        assert_eq!(copper.outline.len(), 11);
        assert!(
            component
                .regions
                .iter()
                .all(|r| r.layer != LAYER_TOP_SOLDER)
        );
        assert_eq!(pad.solder_mask_expansion_raw, 19_690);
        assert_eq!(
            pad.solder_mask_expansion_raw,
            component.pads[0].solder_mask_expansion_raw
        );
        assert_eq!(component.extended_primitive_information.len(), 1);
        assert!(
            component.extended_primitive_information[0]
                .params
                .contains(&(
                    "SOLDERMASKEXPANSION_MANUAL".to_string(),
                    "1.9690mil".to_string()
                ))
        );
        assert_eq!(
            component.pads[0].size_top,
            coord_from_easy_units(24.0, 10.0)
        );
        // Confirm the number is present in the actual serialized pad records.
        let records = crate::altium::merge::pcblib_records_from_library(&library).unwrap();
        let data = &records.components[0].data;
        assert_native_custom_regions(data, &[("2", LAYER_TOP)], 19_690);
        let skip_block = |offset: usize| {
            offset
                + 4
                + (u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) & 0x00ff_ffff)
                    as usize
        };
        let mut offset = skip_block(0); // component name
        for expected in [b'1', b'2', b'3'] {
            assert_eq!(data[offset], 2); // Altium pad primitive
            offset += 1;
            assert_eq!(data[offset + 4], 1); // Pascal string length
            assert_eq!(data[offset + 5], expected);
            for _ in 0..6 {
                offset = skip_block(offset);
            }
        }
    }

    #[test]
    fn custom_pad_ownership_uses_record_order_on_the_bottom_layer() {
        let payload = sot89_like_payload();
        let rows: Vec<Value> = payload["result"]["dataStr"]
            .as_str()
            .unwrap()
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        // The third pad record has an alphanumeric designator. Neither the
        // designator nor its copper-region ordinal can stand in for PADINDEX.
        let mut rows = [rows[2].clone(), rows[0].clone(), rows[1].clone()];
        rows[2][4] = json!(2);
        rows[2][5] = json!("TAB");
        rows[2].as_array_mut().unwrap().resize(19, Value::Null);
        rows[2][17] = json!(3.25);
        let payload = json!({"result":{"dataStr":rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")}});
        let library = build_pcblib_from_payload(&payload, "BOTTOM-TAB", None).unwrap();
        assert!(
            library.components[0].extended_primitive_information[0]
                .params
                .contains(&(
                    "SOLDERMASKEXPANSION_MANUAL".to_string(),
                    "3.2500mil".to_string()
                ))
        );
        let records = crate::altium::merge::pcblib_records_from_library(&library).unwrap();
        assert_native_custom_regions(
            &records.components[0].data,
            &[("TAB", LAYER_BOTTOM)],
            32_500,
        );
    }

    #[test]
    fn real_footprint_keeps_each_irregular_pad_separate() {
        let payload = serde_json::from_str(include_str!(
            "../../tests/fixtures/lceda_footprint_c29073231.json"
        ))
        .unwrap();
        let library = build_pcblib_from_payload(&payload, "QFN-custom-pads", None).unwrap();
        let records = crate::altium::merge::pcblib_records_from_library(&library).unwrap();
        let expected = ["1", "15", "7", "9", "6"].map(|name| (name, LAYER_TOP));
        assert_native_custom_regions(&records.components[0].data, &expected, 19_685);
    }

    #[test]
    fn anchors_fit_inside_concave_and_rotated_polygons() {
        let outline = [
            (-40., -8.),
            (-10., -8.),
            (0., -25.),
            (50., -25.),
            (65., -10.),
            (65., 10.),
            (50., 25.),
            (0., 25.),
            (-10., 8.),
            (-40., 8.),
        ];
        for degrees in [0.0_f64, 37.0, 90.0, 180.0] {
            let angle = degrees.to_radians();
            let transform = |x: f64, y: f64| RawPoint {
                x: 123.0 + x * angle.cos() - y * angle.sin(),
                y: -74.0 + x * angle.sin() + y * angle.cos(),
            };
            let points: Vec<_> = outline.iter().map(|&(x, y)| transform(x, y)).collect();
            let (center, diameter) = custom_pad_anchor(&points, transform(-30., 0.));
            assert!(diameter > 30.0);
            assert!(inside_polygon(center, &points));
            for (&a, &b) in points
                .iter()
                .zip(points.iter().cycle().skip(1))
                .take(points.len())
            {
                assert!(distance_to_segment(center, a, b) > diameter / 2.0);
            }
        }
    }
}
