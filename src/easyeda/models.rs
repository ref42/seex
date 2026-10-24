use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentData {
    pub lcsc_id: String,
    pub title: String,
    #[serde(default)]
    pub designator: String,
    pub description: String,
    pub data_str: Vec<String>,
    pub bbox_x: f64, // Symbol bbox
    pub bbox_y: f64, // Symbol bbox
    pub symbol_parts: Vec<SymbolPartData>,
    pub package_detail: Vec<String>,
    pub package_bbox_x: f64, // Footprint bbox
    pub package_bbox_y: f64, // Footprint bbox
    pub model_3d: Option<Model3dInfo>,
    pub manufacturer: String,
    pub datasheet: String,
    pub jlc_id: String,
    /// EasyEDA package name (`c_para.package`), e.g. `SOT-23-5_L2.9-W1.6-P0.95-LS2.8-BR`.
    #[serde(default)]
    pub package: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolPartData {
    pub title: String,
    pub data_str: Vec<String>,
    pub bbox_x: f64,
    pub bbox_y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model3dInfo {
    pub uuid: String,
    pub title: String,
    /// Rotation (degrees) of the 3D model around the footprint's Z axis, as stored by EasyEDA.
    /// Consumers convert it into their own frame with `crate::util::model_rotation_z`.
    #[serde(default)]
    pub rotation_z: f64,
    /// Where EasyEDA places the model on the footprint (the `outline3D` node).
    #[serde(default)]
    pub placement: Option<ModelPlacement>,
}

/// EasyEDA 3D model placement, in EasyEDA canvas units (1 px = 10 mil, Y down).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPlacement {
    /// Model centre (`c_origin`).
    pub origin_x: f64,
    pub origin_y: f64,
    /// Height of the model bottom above the board (`z`).
    pub z: f64,
    /// Rotation about x, y and z in degrees (`c_rotation`).
    pub rotation: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    pub success: bool,
    pub result: Option<ApiResult>,
    pub message: Option<String>,
    pub code: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResult {
    #[serde(rename = "dataStr")]
    pub data_str: Option<serde_json::Value>,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "packageDetail")]
    pub package_detail: Option<serde_json::Value>,
    pub lcsc: Option<serde_json::Value>,
    #[serde(default)]
    pub subparts: Vec<ApiSubpart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiSubpart {
    pub title: Option<String>,
    #[serde(rename = "dataStr")]
    pub data_str: Option<serde_json::Value>,
}

// EasyEDA Symbol structures
#[derive(Debug, Clone)]
pub struct EeSymbol {
    pub name: String,
    pub prefix: String,
    pub pins: Vec<EePin>,
    pub rectangles: Vec<EeRectangle>,
    pub circles: Vec<EeCircle>,
    pub ellipses: Vec<EeEllipse>,
    pub arcs: Vec<EeArc>,
    pub polylines: Vec<EePolyline>,
    pub polygons: Vec<EePolygon>,
    pub paths: Vec<EePath>,
    pub texts: Vec<EeText>,
}

#[derive(Debug, Clone)]
pub struct EePin {
    pub number: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub rotation: i32,
    pub length: f64,
    pub name_visible: bool,
    pub number_visible: bool,
    pub electric_type: String,
    pub dot: bool,
    pub clock: bool,
}

#[derive(Debug, Clone)]
pub struct EeRectangle {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub stroke_width: f64,
    pub fill: bool,
    pub layer_id: i32,
}

#[derive(Debug, Clone)]
pub struct EeCircle {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub stroke_width: f64,
    pub fill: bool,
    pub layer_id: i32,
}

#[derive(Debug, Clone)]
pub struct EeEllipse {
    pub cx: f64,
    pub cy: f64,
    pub rx: f64,
    pub ry: f64,
    pub stroke_width: f64,
    pub fill: bool,
}

#[derive(Debug, Clone)]
pub struct EeArc {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub stroke_width: f64,
}

#[derive(Debug, Clone)]
pub struct EeFootprintArc {
    pub stroke_width: f64,
    pub layer_id: i32,
    pub path: String, // SVG path string: "M startX startY A rx ry rotation large_arc sweep endX endY"
}

#[derive(Debug, Clone)]
pub struct EePolyline {
    pub points: Vec<(f64, f64)>,
    pub stroke_width: f64,
}

#[derive(Debug, Clone)]
pub struct EePolygon {
    pub points: Vec<(f64, f64)>,
    pub stroke_width: f64,
    pub fill: bool,
}

#[derive(Debug, Clone)]
pub struct EePath {
    pub path_data: String, // SVG path string (e.g., "M 0,0 L 10,10")
    pub stroke_width: f64,
    pub fill: bool,
}

#[derive(Debug, Clone)]
pub struct EeText {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub rotation: i32,
    pub font_size: f64,
    pub stroke_width: f64,
    pub layer_id: i32,
}

// EasyEDA Footprint structures
#[derive(Debug, Clone)]
pub struct EeFootprint {
    pub name: String,
    pub pads: Vec<EePad>,
    pub tracks: Vec<EeTrack>,
    pub circles: Vec<EeCircle>,
    pub arcs: Vec<EeFootprintArc>,
    pub rectangles: Vec<EeRectangle>,
    pub texts: Vec<EeText>,
    pub holes: Vec<EeHole>,
    pub vias: Vec<EeVia>,
    pub svg_nodes: Vec<EeSvgNode>,
}

#[derive(Debug, Clone)]
pub struct EePad {
    pub number: String,
    pub shape: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub rotation: f64,
    pub hole_radius: Option<f64>,
    pub hole_length: Option<f64>, // For elliptical drills
    pub points: String,           // For polygon pads
    pub layer_id: i32,
}

#[derive(Debug, Clone)]
pub struct EeTrack {
    pub stroke_width: f64,
    pub layer_id: i32,
    pub net: String,
    pub points: String, // Space-separated coordinates: "x1 y1 x2 y2 x3 y3..."
}

#[derive(Debug, Clone)]
pub struct EeHole {
    pub x: f64,
    pub y: f64,
    pub radius: f64, // EasyEDA stores radius, not diameter
}

#[derive(Debug, Clone)]
pub struct EeVia {
    pub x: f64,
    pub y: f64,
    pub diameter: f64, // Pad outer diameter
    pub net: String,
    pub radius: f64, // Hole radius (drill = radius * 2)
}

#[derive(Debug, Clone)]
pub struct EeSvgNode {
    pub path: String,
    pub stroke_width: f64,
    pub layer: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_result_deserializes_symbol_subparts() {
        let response: ApiResponse = serde_json::from_str(
            r##"{
                "success": true,
                "result": {
                    "title": "MPU",
                    "dataStr": {"head": {"x": 400, "y": 300}, "shape": []},
                    "subparts": [
                        {
                            "title": "MPU.1",
                            "dataStr": {
                                "head": {"x": 400, "y": 300},
                                "shape": ["P~show~0~1~260~50~180~p1~0"]
                            }
                        }
                    ]
                }
            }"##,
        )
        .unwrap();

        let result = response.result.unwrap();
        assert_eq!(result.subparts.len(), 1);
        assert_eq!(result.subparts[0].title.as_deref(), Some("MPU.1"));
        assert_eq!(
            result.subparts[0]
                .data_str
                .as_ref()
                .unwrap()
                .get("shape")
                .and_then(|shape| shape.as_array())
                .map(Vec::len),
            Some(1)
        );
    }
}
