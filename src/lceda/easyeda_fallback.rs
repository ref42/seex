use serde_json::{Value, json};

use crate::easyeda::{ComponentData, EasyedaApi, FootprintImporter, SymbolImporter};
use crate::error::Result;
use crate::lceda::models::SearchItem;

const SYMBOL_PREFIX: &str = "easyeda-symbol:";
const FOOTPRINT_PREFIX: &str = "easyeda-footprint:";
const STEP_PREFIX: &str = "easyeda-step:";
const EASYEDA_PX_TO_LCEDA_UNIT: f64 = 10.0;

pub(crate) fn is_easyeda_uuid(uuid: &str) -> bool {
    uuid.starts_with(SYMBOL_PREFIX) || uuid.starts_with(FOOTPRINT_PREFIX)
}

pub(crate) fn is_easyeda_step_uuid(uuid: &str) -> bool {
    uuid.starts_with(STEP_PREFIX)
}

pub(crate) fn easyeda_step_uuid(uuid: &str) -> Option<&str> {
    uuid.strip_prefix(STEP_PREFIX)
}

pub(crate) async fn select_item_by_lcsc_id(lcsc_id: &str) -> Result<SearchItem> {
    let data = EasyedaApi::new().get_component_data(lcsc_id).await?;
    Ok(search_item_from_component(&data))
}

pub(crate) async fn component_detail(uuid: &str) -> Result<Value> {
    let Some((kind, lcsc_id)) = uuid.split_once(':') else {
        return Err(crate::error::AppError::InvalidResponse(format!(
            "invalid EasyEDA fallback uuid: {uuid}"
        )));
    };
    let data = EasyedaApi::new().get_component_data(lcsc_id).await?;
    match kind {
        "easyeda-symbol" => Ok(symbol_payload_from_component(&data)),
        "easyeda-footprint" => Ok(footprint_payload_from_component(&data)),
        _ => Err(crate::error::AppError::InvalidResponse(format!(
            "invalid EasyEDA fallback uuid: {uuid}"
        ))),
    }
}

fn search_item_from_component(data: &ComponentData) -> SearchItem {
    let model_uuid = data
        .model_3d
        .as_ref()
        .map(|model| format!("{STEP_PREFIX}{}", model.uuid));
    let model_title = data
        .model_3d
        .as_ref()
        .map(|model| model.title.clone())
        .unwrap_or_default();

    SearchItem {
        index: 1,
        display_title: data.title.clone(),
        title: data.title.clone(),
        manufacturer: data.manufacturer.clone(),
        model_uuid,
        raw: json!({
            "product_code": data.lcsc_id,
            "description": data.description,
            "symbol": {
                "uuid": format!("{SYMBOL_PREFIX}{}", data.lcsc_id),
                "display_title": data.title,
            },
            "footprint": {
                "uuid": format!("{FOOTPRINT_PREFIX}{}", data.lcsc_id),
                "display_title": data.title,
            },
            "attributes": {
                "Supplier": "LCSC",
                "Supplier Part": data.lcsc_id,
                "LCSC Part Name": data.description,
                "Manufacturer": data.manufacturer,
                "Manufacturer Part": data.title,
                "Footprint": data.title,
                "Symbol": format!("{SYMBOL_PREFIX}{}", data.lcsc_id),
                "3D Model": model_title,
            }
        }),
    }
}

fn symbol_payload_from_component(data: &ComponentData) -> Value {
    let mut rows = Vec::new();

    if !data.designator.trim().is_empty() {
        rows.push(json!([
            "ATTR",
            "root_designator",
            "",
            "Designator",
            data.designator,
            false,
            false,
            null,
            null,
            0,
            "st3",
            0
        ]));
    }

    push_symbol_rows(
        &mut rows,
        "U.1",
        &data.data_str,
        data.bbox_x,
        data.bbox_y,
        1,
    );

    let mut owner_part_id = 2;
    for part in &data.symbol_parts {
        push_symbol_rows(
            &mut rows,
            part.title.as_str(),
            &part.data_str,
            part.bbox_x,
            part.bbox_y,
            owner_part_id,
        );
        owner_part_id += 1;
    }

    json!({
        "result": {
            "display_title": data.title,
            "title": data.title,
            "description": data.description,
            "dataStr": rows_to_data_str(rows),
        }
    })
}

fn push_symbol_rows(
    rows: &mut Vec<Value>,
    part_name: &str,
    shapes: &[String],
    bbox_x: f64,
    bbox_y: f64,
    owner_part_id: i32,
) {
    let Ok(symbol) = SymbolImporter::parse(shapes) else {
        return;
    };
    let part_start = rows.len();

    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for pin in &symbol.pins {
        let pin_id = format!("p{owner_part_id}_{}", rows.len());
        let length = pin.length.max(10.0);
        let start_x = pin.x - bbox_x;
        let start_y = pin.y - bbox_y;
        let row_x = start_x;
        let row_y = start_y;
        update_bounds(&mut min_x, &mut min_y, &mut max_x, &mut max_y, row_x, row_y);
        rows.push(json!([
            "PIN",
            pin_id,
            1,
            null,
            row_x,
            row_y,
            length,
            pin.rotation,
            null,
            0,
            0,
            owner_part_id
        ]));
        rows.push(json!([
            "ATTR",
            format!("{pin_id}_name"),
            pin_id,
            "NAME",
            pin.name,
            false,
            pin.name_visible,
            row_x,
            row_y,
            0,
            "st3",
            owner_part_id
        ]));
        rows.push(json!([
            "ATTR",
            format!("{pin_id}_number"),
            pin_id,
            "NUMBER",
            pin.number,
            false,
            pin.number_visible,
            row_x,
            row_y,
            0,
            "st4",
            owner_part_id
        ]));
    }

    for rect in &symbol.rectangles {
        let x1 = rect.x - bbox_x;
        let y1 = rect.y - bbox_y;
        let x2 = x1 + rect.width;
        let y2 = y1 + rect.height;
        update_bounds(&mut min_x, &mut min_y, &mut max_x, &mut max_y, x1, y1);
        update_bounds(&mut min_x, &mut min_y, &mut max_x, &mut max_y, x2, y2);
        rows.push(json!([
            "RECT",
            format!("r{}", rows.len()),
            x1,
            y1,
            x2,
            y2,
            0,
            0,
            0,
            "st1",
            owner_part_id
        ]));
    }

    for polyline in &symbol.polylines {
        push_symbol_point_row(
            rows,
            "POLY",
            &polyline.points,
            bbox_x,
            bbox_y,
            owner_part_id,
        );
        for (x, y) in &polyline.points {
            update_bounds(
                &mut min_x,
                &mut min_y,
                &mut max_x,
                &mut max_y,
                *x - bbox_x,
                *y - bbox_y,
            );
        }
    }

    for polygon in &symbol.polygons {
        push_symbol_point_row(rows, "POLY", &polygon.points, bbox_x, bbox_y, owner_part_id);
        for (x, y) in &polygon.points {
            update_bounds(
                &mut min_x,
                &mut min_y,
                &mut max_x,
                &mut max_y,
                *x - bbox_x,
                *y - bbox_y,
            );
        }
    }

    let bbox = if min_x.is_finite() {
        json!([min_x, min_y, max_x, max_y])
    } else {
        json!([-10.0, -10.0, 10.0, 10.0])
    };
    rows.insert(part_start, json!(["PART", part_name, {"BBOX": bbox}]));
}

fn push_symbol_point_row(
    rows: &mut Vec<Value>,
    row_type: &str,
    points: &[(f64, f64)],
    bbox_x: f64,
    bbox_y: f64,
    owner_part_id: i32,
) {
    if points.len() < 2 {
        return;
    }
    let flat_points: Vec<Value> = points
        .iter()
        .flat_map(|(x, y)| [json!(x - bbox_x), json!(y - bbox_y)])
        .collect();
    rows.push(json!([
        row_type,
        format!("g{}", rows.len()),
        flat_points,
        0,
        0,
        0,
        owner_part_id
    ]));
}

fn footprint_payload_from_component(data: &ComponentData) -> Value {
    let rows = fallback_footprint_rows(data);
    let mut model_3d = Value::Null;
    if let Some(model) = &data.model_3d {
        model_3d = json!({
            "title": model.title,
            "uri": format!("{STEP_PREFIX}{}", model.uuid),
            // "<size x>,<size y>,<size z>,<rot z>,...": only the rotation is needed to place the
            // model, and it has to survive into the rebuilt lceda payload.
            "transform": format!("0,0,0,{},0,0,0,0,0", model.rotation_z),
        });
    }

    json!({
        "result": {
            "display_title": data.title,
            "title": data.title,
            "description": data.description,
            "package": data.title,
            "dataStr": rows_to_data_str(rows),
            "model_3d": model_3d,
        }
    })
}

fn fallback_footprint_rows(data: &ComponentData) -> Vec<Value> {
    let Ok(footprint) = FootprintImporter::parse(&data.package_detail) else {
        return Vec::new();
    };

    let mut rows = vec![json!(["DOCTYPE", "FOOTPRINT", "1.8"])];

    for pad in footprint.pads {
        let shape = if pad.shape.eq_ignore_ascii_case("POLYGON") {
            "POLY"
        } else {
            pad.shape.as_str()
        };
        let hole = match (pad.hole_radius, pad.hole_length) {
            (Some(radius), Some(length)) => {
                json!([
                    "SLOT",
                    scale_footprint_unit(radius * 2.0),
                    scale_footprint_unit(length)
                ])
            }
            (Some(radius), None) => json!(scale_footprint_unit(radius * 2.0)),
            _ => Value::Null,
        };
        let shape_value = if shape.eq_ignore_ascii_case("POLY") && !pad.points.trim().is_empty() {
            json!([
                "POLY",
                path_points_from_text(&pad.points, data.package_bbox_x, data.package_bbox_y,)
            ])
        } else {
            json!([
                shape,
                scale_footprint_unit(pad.width),
                scale_footprint_unit(pad.height)
            ])
        };

        rows.push(json!([
            "PAD",
            format!("pad_{}", pad.number),
            0,
            "",
            pad.layer_id,
            pad.number,
            scale_footprint_unit(pad.x - data.package_bbox_x),
            scale_footprint_unit(pad.y - data.package_bbox_y),
            pad.rotation,
            hole,
            shape_value,
            [],
            0,
            0,
            0,
            1
        ]));
    }

    for track in footprint.tracks {
        if !is_fallback_graphic_layer(track.layer_id) {
            continue;
        }
        let points =
            shifted_point_pairs_from_text(&track.points, data.package_bbox_x, data.package_bbox_y);
        if points.len() >= 2 {
            for segment in points.windows(2) {
                rows.push(json!([
                    "TRACK",
                    format!("track_{}", rows.len()),
                    0,
                    "",
                    track.layer_id,
                    scale_footprint_unit(track.stroke_width),
                    segment[0].0,
                    segment[0].1,
                    segment[1].0,
                    segment[1].1
                ]));
            }
        }
    }

    for circle in footprint.circles {
        if !is_fallback_graphic_layer(circle.layer_id) {
            continue;
        }
        rows.push(json!([
            "CIRCLE",
            format!("circle_{}", rows.len()),
            0,
            "",
            circle.layer_id,
            scale_footprint_unit(circle.stroke_width),
            scale_footprint_unit(circle.cx - data.package_bbox_x),
            scale_footprint_unit(circle.cy - data.package_bbox_y),
            scale_footprint_unit(circle.radius)
        ]));
    }

    for rect in footprint.rectangles {
        if !is_fallback_graphic_layer(rect.layer_id) {
            continue;
        }
        let x = scale_footprint_unit(rect.x - data.package_bbox_x);
        let y = scale_footprint_unit(rect.y - data.package_bbox_y);
        rows.push(json!([
            "RECT",
            format!("rect_{}", rows.len()),
            0,
            "",
            rect.layer_id,
            scale_footprint_unit(rect.stroke_width),
            x,
            y,
            x + scale_footprint_unit(rect.width),
            y + scale_footprint_unit(rect.height)
        ]));
    }

    for hole in footprint.holes {
        rows.push(json!([
            "FILL",
            format!("hole_{}", rows.len()),
            0,
            "",
            12,
            0.2,
            0,
            [
                "CIRCLE",
                scale_footprint_unit(hole.x - data.package_bbox_x),
                scale_footprint_unit(hole.y - data.package_bbox_y),
                scale_footprint_unit(hole.radius)
            ],
            0
        ]));
    }

    for via in footprint.vias {
        rows.push(json!([
            "FILL",
            format!("via_{}", rows.len()),
            0,
            "",
            12,
            0.2,
            0,
            [
                "CIRCLE",
                scale_footprint_unit(via.x - data.package_bbox_x),
                scale_footprint_unit(via.y - data.package_bbox_y),
                scale_footprint_unit(via.diameter / 2.0)
            ],
            0
        ]));
    }

    rows
}

fn is_fallback_graphic_layer(layer_id: i32) -> bool {
    matches!(layer_id, 3 | 4 | 13 | 48 | 49 | 99)
}

fn scale_footprint_unit(value: f64) -> f64 {
    value * EASYEDA_PX_TO_LCEDA_UNIT
}

fn rows_to_data_str(rows: Vec<Value>) -> String {
    rows.into_iter()
        .filter_map(|row| serde_json::to_string(&row).ok())
        .collect::<Vec<_>>()
        .join("\n")
}

fn update_bounds(
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
    x: f64,
    y: f64,
) {
    *min_x = min_x.min(x);
    *min_y = min_y.min(y);
    *max_x = max_x.max(x);
    *max_y = max_y.max(y);
}

fn shifted_point_pairs_from_text(text: &str, bbox_x: f64, bbox_y: f64) -> Vec<(f64, f64)> {
    let coords: Vec<f64> = text
        .split_whitespace()
        .filter_map(|part| part.parse::<f64>().ok())
        .collect();
    coords
        .chunks(2)
        .filter_map(|pair| {
            if pair.len() == 2 {
                Some((pair[0] - bbox_x, pair[1] - bbox_y))
            } else {
                None
            }
        })
        .map(|(x, y)| (scale_footprint_unit(x), scale_footprint_unit(y)))
        .collect()
}

fn path_points_from_text(text: &str, bbox_x: f64, bbox_y: f64) -> Vec<Value> {
    let coords: Vec<f64> = text
        .split_whitespace()
        .filter_map(|part| part.parse::<f64>().ok())
        .collect();
    let mut values = Vec::with_capacity(coords.len() + 1);
    values.push(json!("L"));
    for pair in coords.chunks(2) {
        if pair.len() == 2 {
            values.push(Value::from(scale_footprint_unit(pair[0] - bbox_x)));
            values.push(Value::from(scale_footprint_unit(pair[1] - bbox_y)));
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easyeda::{ComponentData, Model3dInfo};

    #[test]
    fn synthetic_search_item_uses_easyeda_fallback_uuids() {
        let data = sample_component_data();
        let item = search_item_from_component(&data);

        assert_eq!(item.lcsc_id().as_deref(), Some("C2849413"));
        assert_eq!(
            item.symbol_uuid().as_deref(),
            Some("easyeda-symbol:C2849413")
        );
        assert_eq!(
            item.footprint_uuid().as_deref(),
            Some("easyeda-footprint:C2849413")
        );
        assert_eq!(item.model_uuid.as_deref(), Some("easyeda-step:model-uuid"));
    }

    #[test]
    fn symbol_payload_contains_lceda_rows() {
        let data = sample_component_data();
        let payload = symbol_payload_from_component(&data);
        let data_str = payload["result"]["dataStr"].as_str().unwrap();

        assert!(data_str.contains(r#"["PART","U.1""#));
        assert!(data_str.contains(r#"["PIN""#));
        assert!(data_str.contains(r#""NUMBER","1""#));
        assert!(data_str.contains(r#""NAME","A""#));
        assert!(data_str.contains(r#""Designator","U?""#));
    }

    #[test]
    fn footprint_payload_contains_shifted_pad_row() {
        let data = sample_component_data();
        let payload = footprint_payload_from_component(&data);
        let data_str = payload["result"]["dataStr"].as_str().unwrap();
        let rows = parse_data_str_rows(data_str);
        let pad_row = rows
            .iter()
            .find(|row| row.first().and_then(Value::as_str) == Some("PAD"))
            .unwrap();
        let track_row = rows
            .iter()
            .find(|row| row.first().and_then(Value::as_str) == Some("TRACK"))
            .unwrap();

        assert!(data_str.contains(r#"["DOCTYPE","FOOTPRINT","1.8"]"#));
        assert!(data_str.contains(r#"["PAD","pad_1""#));
        assert_eq!(pad_row[6].as_f64(), Some(50.0));
        assert_eq!(pad_row[7].as_f64(), Some(150.0));
        assert_eq!(pad_row[10][1].as_f64(), Some(300.0));
        assert_eq!(pad_row[10][2].as_f64(), Some(400.0));
        assert_eq!(track_row[5].as_f64(), Some(10.0));
        assert_eq!(track_row[6].as_f64(), Some(-50.0));
        assert_eq!(track_row[7].as_f64(), Some(-50.0));
        assert_eq!(track_row[8].as_f64(), Some(50.0));
        assert_eq!(track_row[9].as_f64(), Some(-50.0));
        assert_eq!(
            payload["result"]["model_3d"]["uri"].as_str(),
            Some("easyeda-step:model-uuid")
        );
    }

    #[test]
    fn footprint_payload_skips_easyeda_manufacturing_circles() {
        let mut data = sample_component_data();
        data.package_detail = vec![
            "PAD~ELLIPSE~376.378~312.598~1.4173~1.4173~1~~A1~0~~0~rep5~0~~Y~0~-393.7008~0.1969~376.378,312.5984".to_string(),
            "CIRCLE~376.378~312.598~0.443~0.8858~100~circle-pad~0".to_string(),
            "CIRCLE~376.378~312.598~0.443~0.8858~101~circle-mask~0".to_string(),
            "CIRCLE~376.378~312.598~0.443~0.8858~12~circle-multi~0".to_string(),
            "CIRCLE~377.000~313.000~0.500~0.100~3~circle-silk~0".to_string(),
        ];
        data.package_bbox_x = 376.0;
        data.package_bbox_y = 312.0;

        let rows = fallback_footprint_rows(&data);
        let pads: Vec<_> = rows
            .iter()
            .filter(|row| {
                row.as_array()
                    .and_then(|row| row.first())
                    .and_then(Value::as_str)
                    == Some("PAD")
            })
            .collect();
        let circles: Vec<_> = rows
            .iter()
            .filter(|row| {
                row.as_array()
                    .and_then(|row| row.first())
                    .and_then(Value::as_str)
                    == Some("CIRCLE")
            })
            .collect();

        assert_eq!(pads.len(), 1);
        assert_eq!(circles.len(), 1);
        assert_eq!(circles[0][4].as_i64(), Some(3));
        assert_eq!(pads[0][10][1].as_f64(), Some(14.173));
        assert_eq!(pads[0][10][2].as_f64(), Some(14.173));
    }

    fn parse_data_str_rows(data_str: &str) -> Vec<Vec<Value>> {
        data_str
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .map(|value| value.as_array().unwrap().clone())
            .collect()
    }

    fn sample_component_data() -> ComponentData {
        ComponentData {
            lcsc_id: "C2849413".to_string(),
            title: "TEST-PART".to_string(),
            designator: "U?".to_string(),
            description: "Generated fallback component".to_string(),
            data_str: vec![
                "P~show~0~1~20~10~0~p1~0^^0~0^^M 0,0 h -10~#000^^1~0~0~0~A~start~Arial~6pt^^1~0~0~0~1~start~Arial~6pt".to_string(),
                "R~0~0~0~0~40~20~#000~1~0~none~r1~0".to_string(),
            ],
            bbox_x: 0.0,
            bbox_y: 0.0,
            symbol_parts: Vec::new(),
            package_detail: vec![
                "PAD~RECT~10~20~30~40~1~~1~0~~90".to_string(),
                "TRACK~1~3~~0 0 10 0 10 10".to_string(),
            ],
            package_bbox_x: 5.0,
            package_bbox_y: 5.0,
            model_3d: Some(Model3dInfo {
                uuid: "model-uuid".to_string(),
                title: "BODY-H1.0".to_string(),
                rotation_z: 0.0,
                placement: None,
            }),
            manufacturer: "SEEX".to_string(),
            datasheet: String::new(),
            jlc_id: String::new(),
            package: String::new(),
        }
    }
}
