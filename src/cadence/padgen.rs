//! Parametric padstacks and simple footprints for Allegro.
//!
//! The pad generator builds what the EasyEDA conversion never needs to: padstacks
//! from a set of parameters (land shape and size, drill or slot, plating, drill
//! tolerance, inner-layer pads, anti-pads and thermal reliefs, solder mask and paste
//! on either side), and three footprints built around one of them: mounting
//! holes, test points and fiducials.
//!
//! Names follow the IPC-7351B / IPC-7352 padstack naming convention (published by
//! PCB Libraries): `c150h90` is a 1.50 mm round land on a 0.90 mm plated hole,
//! `r145_60` a 1.45 x 0.60 mm SMD rectangle, `v50h25` a via, `c100hn150` a
//! non-plated hole, `s300r25` a square with 0.25 mm rounded corners, `m0` a tented
//! side, `p0` no paste, `z140` a 1.40 mm inner-layer land, `x` an opposite-side
//! land. Two modifiers the convention leaves open are SeEx's own: `hr` for a
//! rectangular slot and `ht<plus>_<minus>` for a drill tolerance.
//!
//! Like the rest of the Cadence export, the generator writes SKILL, into the
//! library's `_allegro` folder: one script per footprint, and one per padstack
//! under `padstacks`. The library script builds them with the exported
//! footprints when it is loaded in Allegro PCB Editor.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use crate::cadence::allegro::{
    ASSEMBLY_TEXT_BLOCK, HELPERS, PLACE_BOUND_TOP, REFDES_GAP_MM, SILK_LINE_MM, SILK_TEXT_BLOCK,
    SILK_TOP, assembly_refdes_layout, f, fnv1a, silk_refdes_height, silk_stroke, skill_string,
};
use crate::cadence::workflow::CadenceLayout;
use crate::error::AppError;

/// Allegro accepts longer padstack and symbol names, but older releases and many
/// library tools stop at 31 characters.
pub const MAX_NAME: usize = 31;
/// Solder mask opening per side when nothing else is asked for (0.05 mm, as the
/// EasyEDA export uses).
pub const DEFAULT_MASK_PER_SIDE: f64 = 0.05;
/// Anti-pad and thermal relief: this much larger than the inner land per side.
pub const DEFAULT_PLANE_CLEARANCE: f64 = 0.25;
/// Clearance between copper and the silkscreen circle of a mounting hole.
const SILK_CLEARANCE: f64 = 0.2;
/// The place bound sits this far outside the outermost geometry.
const BOUND_CLEARANCE: f64 = 0.25;

// ---------------------------------------------------------------------------
// Specification

/// Land outline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Round,
    Square,
    Rect,
    Oblong,
    RoundRect,
    ChamferRect,
}

impl Shape {
    pub const ALL: [Shape; 6] = [
        Shape::Round,
        Shape::Square,
        Shape::Rect,
        Shape::Oblong,
        Shape::RoundRect,
        Shape::ChamferRect,
    ];

    /// Whether the shape has a second dimension.
    pub fn has_height(self) -> bool {
        !matches!(self, Shape::Round | Shape::Square)
    }

    pub fn has_corners(self) -> bool {
        matches!(self, Shape::RoundRect | Shape::ChamferRect)
    }

    pub fn key(self) -> &'static str {
        match self {
            Shape::Round => "round",
            Shape::Square => "square",
            Shape::Rect => "rect",
            Shape::Oblong => "oblong",
            Shape::RoundRect => "roundrect",
            Shape::ChamferRect => "chamfer",
        }
    }
}

/// A copper (or mask, or paste) land, in millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Land {
    pub shape: Shape,
    pub width: f64,
    /// Ignored for round and square lands.
    pub height: f64,
    /// Corner radius or chamfer leg (rounded / chamfered rectangles).
    pub corner: f64,
    /// Which corners are rounded or cut: upper left, upper right, lower right,
    /// lower left.
    pub corners: [bool; 4],
}

impl Land {
    pub fn round(diameter: f64) -> Self {
        Self {
            shape: Shape::Round,
            width: diameter,
            height: diameter,
            corner: 0.0,
            corners: [true; 4],
        }
    }

    pub fn rect(width: f64, height: f64) -> Self {
        Self {
            shape: Shape::Rect,
            width,
            height,
            corner: 0.0,
            corners: [true; 4],
        }
    }

    pub fn with_shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    /// The same land with a sensible shape for its numbers: equal sides make a
    /// circle or a square, a zero corner a plain rectangle.
    pub fn normalized(&self) -> Land {
        let mut land = *self;
        if !land.shape.has_height() {
            land.height = land.width;
        }
        if land.shape.has_corners() {
            land.corner = land.corner.clamp(0.0, land.width.min(land.height) / 2.0);
            if land.corner <= 0.0 || !land.corners.iter().any(|c| *c) {
                land.shape = Shape::Rect;
                land.corner = 0.0;
            }
        } else {
            land.corner = 0.0;
        }
        if land.shape == Shape::Oblong && same(land.width, land.height) {
            land.shape = Shape::Round;
        }
        land
    }

    pub fn is_square(&self) -> bool {
        same(self.width, self.height)
    }

    /// Allegro figure name.
    pub fn figure(&self) -> &'static str {
        let land = self.normalized();
        match land.shape {
            Shape::Round => "CIRCLE",
            Shape::Square => "SQUARE",
            // Allegro rejects a RECTANGLE with equal sides.
            Shape::Rect if land.is_square() => "SQUARE",
            Shape::Rect => "RECTANGLE",
            Shape::Oblong => "OBLONG",
            Shape::RoundRect => "ROUNDED_RECTANGLE",
            Shape::ChamferRect => "CHAMFERED_RECTANGLE",
        }
    }

    /// Figure used when Allegro refuses the corner treatment.
    fn plain_figure(&self) -> &'static str {
        match self.figure() {
            "ROUNDED_RECTANGLE" | "CHAMFERED_RECTANGLE" if self.is_square() => "SQUARE",
            "ROUNDED_RECTANGLE" | "CHAMFERED_RECTANGLE" => "RECTANGLE",
            other => other,
        }
    }

    /// The land with its outline moved out by `per_side` (negative: in). Radii
    /// stay concentric; a 45-degree chamfer's legs grow by (2 - sqrt 2) of it.
    pub fn grown(&self, per_side: f64) -> Land {
        let land = self.normalized();
        let mut out = land;
        out.width = (land.width + 2.0 * per_side).max(0.01);
        out.height = (land.height + 2.0 * per_side).max(0.01);
        out.corner = match land.shape {
            Shape::RoundRect => (land.corner + per_side).max(0.0),
            Shape::ChamferRect => {
                (land.corner + per_side * (2.0 - std::f64::consts::SQRT_2)).max(0.0)
            }
            _ => 0.0,
        };
        out.normalized()
    }

    /// Corner string Allegro takes (`"UL UR LR LL"`), or `None`.
    fn allegro_corners(&self) -> Option<String> {
        let land = self.normalized();
        if !land.shape.has_corners() {
            return None;
        }
        let names = ["UL", "UR", "LR", "LL"];
        let chosen: Vec<&str> = names
            .iter()
            .zip(land.corners)
            .filter(|(_, on)| *on)
            .map(|(name, _)| *name)
            .collect();
        Some(chosen.join(" "))
    }

    /// Shape letter of the IPC name (`c`, `s`, `r`, `b`); rounded and chamfered
    /// lands are rectangles or squares with a corner modifier.
    fn ipc_letter(&self) -> &'static str {
        let land = self.normalized();
        match land.shape {
            Shape::Round => "c",
            Shape::Square => "s",
            Shape::Oblong => "b",
            Shape::Rect | Shape::RoundRect | Shape::ChamferRect if land.is_square() => "s",
            Shape::Rect | Shape::RoundRect | Shape::ChamferRect => "r",
        }
    }

    /// Size of the IPC name: `150` for round and square, `145_60` otherwise.
    fn ipc_size(&self) -> String {
        let land = self.normalized();
        if land.shape == Shape::Round || land.is_square() {
            hundredths(land.width)
        } else {
            format!("{}_{}", hundredths(land.width), hundredths(land.height))
        }
    }

    /// Corner modifier: `r25` (all corners rounded 0.25), `cul50` (upper left
    /// chamfered 0.50), `rulur25` (both upper corners).
    fn ipc_corner(&self) -> String {
        let land = self.normalized();
        let letter = match land.shape {
            Shape::RoundRect => "r",
            Shape::ChamferRect => "c",
            _ => return String::new(),
        };
        let which = if land.corners.iter().all(|c| *c) {
            String::new()
        } else {
            ["ul", "ur", "br", "bl"]
                .iter()
                .zip(land.corners)
                .filter(|(_, on)| *on)
                .map(|(code, _)| *code)
                .collect()
        };
        format!("{letter}{which}{}", hundredths(land.corner))
    }

    /// `c150`, `r145_60`, `s300r25`, `r200_100cul50`.
    pub fn ipc(&self) -> String {
        format!(
            "{}{}{}",
            self.ipc_letter(),
            self.ipc_size(),
            self.ipc_corner()
        )
    }

    fn ipc_same_shape(&self, other: &Land) -> bool {
        let (a, b) = (self.normalized(), other.normalized());
        a.ipc_letter() == b.ipc_letter() && a.ipc_corner() == b.ipc_corner()
    }

    /// `seex_gpad(layer type figure w h corner corners)` call.
    fn skill(&self, layer: &str, kind: &str) -> String {
        let land = self.normalized();
        let corners = land
            .allegro_corners()
            .map(|c| format!("\"{c}\""))
            .unwrap_or_else(|| "nil".to_string());
        format!(
            "seex_gpad(\"{layer}\" '{kind} '{} {} {} {} {corners})",
            land.figure(),
            f(land.width),
            f(land.height),
            f(land.corner)
        )
    }

    /// Like [`Land::skill`] without corner treatment, for the fallback stack.
    fn skill_plain(&self, layer: &str, kind: &str) -> String {
        let land = self.normalized();
        format!(
            "seex_gpad(\"{layer}\" '{kind} '{} {} {} 0.0 nil)",
            land.plain_figure(),
            f(land.width),
            f(land.height)
        )
    }

    /// Bounding half sizes.
    fn half(&self) -> (f64, f64) {
        let land = self.normalized();
        (land.width / 2.0, land.height / 2.0)
    }
}

/// Drilled hole.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hole {
    Round(f64),
    Slot {
        width: f64,
        height: f64,
        /// Routed with square ends instead of round ones.
        rectangular: bool,
    },
}

impl Hole {
    pub fn size(&self) -> (f64, f64) {
        match *self {
            Hole::Round(d) => (d, d),
            Hole::Slot { width, height, .. } => (width, height),
        }
    }

    fn is_slot(&self) -> bool {
        matches!(self, Hole::Slot { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drill {
    pub hole: Hole,
    pub plated: bool,
    /// Finished-hole tolerance, mm (both positive numbers).
    pub tol_plus: f64,
    pub tol_minus: f64,
}

impl Drill {
    pub fn plated(diameter: f64) -> Self {
        Self {
            hole: Hole::Round(diameter),
            plated: true,
            tol_plus: 0.0,
            tol_minus: 0.0,
        }
    }

    pub fn non_plated(diameter: f64) -> Self {
        Self {
            plated: false,
            ..Self::plated(diameter)
        }
    }

    /// `h90`, `hn150`, `h300_100`, `hr300_100`, then `ht5_5` for a tolerance.
    fn ipc(&self) -> String {
        let mut text = String::from(if self.plated { "h" } else { "hn" });
        match self.hole {
            Hole::Round(d) => text.push_str(&hundredths(d)),
            Hole::Slot {
                width,
                height,
                rectangular,
            } => {
                if rectangular {
                    text.push('r');
                }
                let _ = write!(text, "{}_{}", hundredths(width), hundredths(height));
            }
        }
        if self.tol_plus > 0.0 || self.tol_minus > 0.0 {
            if same(self.tol_plus, self.tol_minus) {
                let _ = write!(text, "ht{}", hundredths(self.tol_plus));
            } else {
                let _ = write!(
                    text,
                    "ht{}_{}",
                    hundredths(self.tol_plus),
                    hundredths(self.tol_minus)
                );
            }
        }
        text
    }

    /// `seex_gdrill(kind w h plated tolPlus tolMinus)` call.
    fn skill(&self) -> String {
        let (kind, (w, h)) = match self.hole {
            Hole::Round(_) => ("ROUND", self.hole.size()),
            Hole::Slot {
                rectangular: true, ..
            } => ("RECT", self.hole.size()),
            Hole::Slot { .. } => ("OVAL", self.hole.size()),
        };
        format!(
            "seex_gdrill('{kind} {} {} {} {} {})",
            f(w),
            f(h),
            if self.plated { "t" } else { "nil" },
            f(self.tol_plus.abs()),
            f(self.tol_minus.abs())
        )
    }

    /// The same hole as a round drill (the smaller slot side), for the fallback.
    fn skill_round(&self) -> String {
        let (w, h) = self.hole.size();
        format!(
            "seex_gdrill('ROUND {} {} {} 0.0 0.0)",
            f(w.min(h)),
            f(w.min(h)),
            if self.plated { "t" } else { "nil" }
        )
    }
}

/// Inner-layer land of a drilled padstack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InnerPad {
    /// The outer land.
    Same,
    /// No land on inner layers (only the anti-pad / thermal relief).
    None,
    Custom(Land),
}

/// A named inner layer with its own land (`None`: no land there).
#[derive(Debug, Clone, PartialEq)]
pub struct LayerPad {
    pub layer: String,
    pub land: Option<Land>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InnerLayers {
    /// Land on every inner layer (Allegro's `DEFAULT_INTERNAL`).
    pub pad: InnerPad,
    /// Anti-pad: this much larger than the inner land per side.
    pub anti_clearance: f64,
    /// Thermal relief: this much larger than the inner land per side; `None`
    /// leaves the thermal pad out.
    pub thermal_clearance: Option<f64>,
    /// Layers that differ from the default, by their cross-section name.
    pub layers: Vec<LayerPad>,
}

impl Default for InnerLayers {
    fn default() -> Self {
        Self {
            pad: InnerPad::Same,
            anti_clearance: DEFAULT_PLANE_CLEARANCE,
            thermal_clearance: Some(DEFAULT_PLANE_CLEARANCE),
            layers: Vec::new(),
        }
    }
}

/// Solder mask and paste on one side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideFinish {
    /// Mask opening per side over the land; negative makes the opening smaller
    /// (mask-defined pads). `None`: tented, no opening.
    pub mask: Option<f64>,
    /// Paste per side over the land; negative is a reduction. `None`: no paste.
    pub paste: Option<f64>,
}

impl SideFinish {
    pub const SMD: SideFinish = SideFinish {
        mask: Some(DEFAULT_MASK_PER_SIDE),
        paste: Some(0.0),
    };
    pub const THROUGH: SideFinish = SideFinish {
        mask: Some(DEFAULT_MASK_PER_SIDE),
        paste: None,
    };
    #[cfg(test)]
    pub const TENTED: SideFinish = SideFinish {
        mask: None,
        paste: None,
    };
}

/// Everything that decides a padstack.
#[derive(Debug, Clone, PartialEq)]
pub struct PadstackSpec {
    /// Land on the top side (or on the SMD side).
    pub land: Land,
    /// Through-hole only: a different land on the bottom side.
    pub bottom_land: Option<Land>,
    /// `None` for an SMD pad.
    pub drill: Option<Drill>,
    /// SMD pad on the bottom side.
    pub smd_bottom: bool,
    /// `false` for a bare non-plated hole without any copper.
    pub copper: bool,
    pub inner: InnerLayers,
    pub top: SideFinish,
    pub bottom: SideFinish,
    /// A via: named `v…` (round) or `vs…` (square).
    pub via: bool,
}

impl PadstackSpec {
    pub fn smd(land: Land) -> Self {
        Self {
            land,
            bottom_land: None,
            drill: None,
            smd_bottom: false,
            copper: true,
            inner: InnerLayers::default(),
            top: SideFinish::SMD,
            bottom: SideFinish::SMD,
            via: false,
        }
    }

    pub fn through(land: Land, drill: Drill) -> Self {
        Self {
            drill: Some(drill),
            top: SideFinish::THROUGH,
            bottom: SideFinish::THROUGH,
            ..Self::smd(land)
        }
    }

    /// A non-plated hole without copper.
    pub fn bare_hole(diameter: f64) -> Self {
        Self {
            copper: false,
            ..Self::through(Land::round(diameter), Drill::non_plated(diameter))
        }
    }

    pub fn is_smd(&self) -> bool {
        self.drill.is_none()
    }

    fn has_copper(&self) -> bool {
        self.copper || self.drill.is_none_or(|d| d.plated)
    }

    /// Land on the bottom side of a drilled padstack.
    fn bottom_land(&self) -> Land {
        self.bottom_land.unwrap_or(self.land)
    }

    fn inner_land(&self) -> Option<Land> {
        match self.inner.pad {
            InnerPad::Same => Some(self.land),
            InnerPad::None => None,
            InnerPad::Custom(land) => Some(land),
        }
    }

    /// What anti-pads and thermals grow from on a layer: its land, or the hole.
    fn plane_base(&self, land: Option<Land>) -> Land {
        land.unwrap_or_else(|| {
            let (w, h) = self.drill.map(|d| d.hole.size()).unwrap_or((0.0, 0.0));
            match self.drill.map(|d| d.hole) {
                Some(Hole::Slot {
                    rectangular: true, ..
                }) => Land::rect(w, h),
                _ if same(w, h) => Land::round(w),
                _ => Land::rect(w, h).with_shape(Shape::Oblong),
            }
        })
    }

    /// Problems that make the parameters unusable.
    pub fn validate(&self) -> Result<(), String> {
        let positive = |value: f64, what: &str| -> Result<(), String> {
            if value.is_finite() && value > 0.0 {
                Ok(())
            } else {
                Err(format!("{what} must be larger than 0"))
            }
        };
        if self.has_copper() {
            positive(self.land.width, "Land width")?;
            if self.land.shape.has_height() {
                positive(self.land.height, "Land height")?;
            }
        }
        if let Some(drill) = &self.drill {
            let (w, h) = drill.hole.size();
            positive(w, "Hole size")?;
            positive(h, "Hole size")?;
            if drill.plated || self.copper {
                let land = self.land.normalized();
                let fits = |land: &Land| land.width > w && land.height > h;
                if !fits(&land) || self.bottom_land.is_some_and(|l| !fits(&l.normalized())) {
                    return Err("The land must be larger than the hole".to_string());
                }
            }
            if drill.tol_plus < 0.0 || drill.tol_minus < 0.0 {
                return Err("Drill tolerances are given as positive numbers".to_string());
            }
        }
        for layer in &self.inner.layers {
            let name = layer.layer.trim();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!(
                    "Inner layer name \"{}\" may use letters, digits, '_' and '-' only",
                    layer.layer
                ));
            }
            // Allegro gives every inner layer the default land; one layer cannot
            // drop it.
            if layer.land.is_none() && self.inner.pad != InnerPad::None {
                return Err(format!(
                    "\"{}=none\" needs \"No land\" as the inner-layer land: Allegro puts the default land on every inner layer. Choose \"No land\" and list the layers that need one.",
                    layer.layer.trim()
                ));
            }
        }
        Ok(())
    }

    /// The IPC-7352 style name (lower case, as Allegro stores it), shortened
    /// with a hash beyond [`MAX_NAME`] characters.
    pub fn name(&self) -> String {
        let full = self.ipc_name();
        if full.len() <= MAX_NAME {
            full
        } else {
            let keep = MAX_NAME - 9;
            let head = full[..keep].trim_end_matches(['_', '-']);
            format!("{head}_{:08x}", fnv1a(&full))
        }
    }

    /// The name without the length limit.
    pub fn ipc_name(&self) -> String {
        let land = self.land.normalized();
        let mut name = String::new();
        if self.is_smd() && self.smd_bottom {
            name.push('x');
        }
        if self.has_copper() {
            if self.via && land.shape == Shape::Round {
                let _ = write!(name, "v{}", land.ipc_size());
            } else if self.via && land.ipc_letter() == "s" {
                let _ = write!(name, "vs{}{}", land.ipc_size(), land.ipc_corner());
            } else {
                name.push_str(&land.ipc());
            }
        }
        if let Some(drill) = &self.drill {
            name.push_str(&drill.ipc());
            if self.has_copper() {
                match self.inner.pad {
                    InnerPad::Same => {}
                    InnerPad::None => name.push_str("z0"),
                    InnerPad::Custom(inner) => {
                        let inner = inner.normalized();
                        if inner.ipc_same_shape(&land) {
                            let _ = write!(name, "z{}", inner.ipc_size());
                        } else {
                            let _ = write!(name, "z{}", inner.ipc());
                        }
                    }
                }
                if let Some(bottom) = self.bottom_land {
                    if bottom.normalized() != land {
                        let _ = write!(name, "x{}", bottom.ipc());
                    }
                }
                match self.inner.thermal_clearance {
                    None => name.push_str("t0"),
                    Some(t) if !same(t, DEFAULT_PLANE_CLEARANCE) => {
                        let base = self.plane_base(self.inner_land());
                        let _ = write!(name, "t{}", base.grown(t).ipc_size());
                    }
                    Some(_) => {}
                }
            }
            if !same(self.inner.anti_clearance, DEFAULT_PLANE_CLEARANCE) {
                let base = self.plane_base(self.has_copper().then(|| self.inner_land()).flatten());
                let _ = write!(
                    name,
                    "y{}",
                    base.grown(self.inner.anti_clearance).ipc_size()
                );
            }
            if !self.inner.layers.is_empty() {
                let key: Vec<String> = self
                    .inner
                    .layers
                    .iter()
                    .map(|l| {
                        format!(
                            "{}={}",
                            l.layer.to_ascii_uppercase(),
                            l.land.map(|land| land.ipc()).unwrap_or_default()
                        )
                    })
                    .collect();
                let _ = write!(name, "l{:04x}", fnv1a(&key.join(";")) & 0xffff);
            }
        }
        // Mask and paste: written only when they differ from the defaults.
        let sides: Vec<(SideFinish, SideFinish, Land, &str)> = if self.is_smd() {
            let side = if self.smd_bottom {
                self.bottom
            } else {
                self.top
            };
            vec![(side, SideFinish::SMD, land, "")]
        } else {
            let base = if self.has_copper() {
                land
            } else {
                self.plane_base(None)
            };
            let bottom_base = if self.has_copper() {
                self.bottom_land().normalized()
            } else {
                base
            };
            let mut sides = vec![(self.top, SideFinish::THROUGH, base, "")];
            if self.bottom != self.top || bottom_base != base {
                sides.push((self.bottom, SideFinish::THROUGH, bottom_base, "x"));
            }
            sides
        };
        for (side, default, base, tag) in &sides {
            if side.mask != default.mask || (!tag.is_empty() && side.mask != self.top.mask) {
                match side.mask {
                    None => {
                        let _ = write!(name, "m{tag}0");
                    }
                    Some(e) => {
                        let _ = write!(name, "m{tag}{}", base.grown(e).ipc_size());
                    }
                }
            }
        }
        for (side, default, base, tag) in &sides {
            if side.paste != default.paste || (!tag.is_empty() && side.paste != self.top.paste) {
                match side.paste {
                    None => {
                        let _ = write!(name, "p{tag}0");
                    }
                    Some(e) => {
                        let _ = write!(name, "p{tag}{}", base.grown(e).ipc_size());
                    }
                }
            }
        }
        name.to_ascii_lowercase()
    }

    /// Layers and pad types the padstack must have once built, as Allegro
    /// reports them for a two-layer drawing.
    pub fn required_layers(&self) -> Vec<(&'static str, &'static str)> {
        let mut layers = Vec::new();
        if self.is_smd() {
            let (etch, mask, paste) = if self.smd_bottom {
                (
                    "ETCH/BOTTOM",
                    "PIN/SOLDERMASK_BOTTOM",
                    "PIN/PASTEMASK_BOTTOM",
                )
            } else {
                ("ETCH/TOP", "PIN/SOLDERMASK_TOP", "PIN/PASTEMASK_TOP")
            };
            let side = if self.smd_bottom {
                self.bottom
            } else {
                self.top
            };
            layers.push((etch, "REGULAR"));
            if side.mask.is_some() {
                layers.push((mask, "REGULAR"));
            }
            if side.paste.is_some() {
                layers.push((paste, "REGULAR"));
            }
            return layers;
        }
        if self.has_copper() {
            layers.push(("ETCH/TOP", "REGULAR"));
            layers.push(("ETCH/BOTTOM", "REGULAR"));
        }
        if self.top.mask.is_some() {
            layers.push(("PIN/SOLDERMASK_TOP", "REGULAR"));
        }
        if self.bottom.mask.is_some() {
            layers.push(("PIN/SOLDERMASK_BOTTOM", "REGULAR"));
        }
        if self.has_copper() && self.top.paste.is_some() {
            layers.push(("PIN/PASTEMASK_TOP", "REGULAR"));
        }
        if self.has_copper() && self.bottom.paste.is_some() {
            layers.push(("PIN/PASTEMASK_BOTTOM", "REGULAR"));
        }
        layers
    }

    /// Pad calls of the stack. `plain` drops corner treatments and the inner
    /// layers, for the fallback stack.
    fn pad_calls(&self, plain: bool) -> Vec<String> {
        let pad = |land: &Land, layer: &str, kind: &str| {
            if plain {
                land.skill_plain(layer, kind)
            } else {
                land.skill(layer, kind)
            }
        };
        let mut calls = Vec::new();
        let land = self.land.normalized();
        if self.is_smd() {
            let (etch, mask, paste, side) = if self.smd_bottom {
                (
                    "BOTTOM",
                    "SOLDERMASK_BOTTOM",
                    "PASTEMASK_BOTTOM",
                    self.bottom,
                )
            } else {
                ("TOP", "SOLDERMASK_TOP", "PASTEMASK_TOP", self.top)
            };
            calls.push(pad(&land, etch, "REGULAR"));
            if let Some(e) = side.mask {
                calls.push(pad(&land.grown(e), mask, "REGULAR"));
            }
            if let Some(e) = side.paste {
                calls.push(pad(&land.grown(e), paste, "REGULAR"));
            }
            return calls;
        }

        let copper = self.has_copper();
        let anti = self.inner.anti_clearance;
        let thermal = self.inner.thermal_clearance;
        // Outer layers: land, anti-pad and thermal relief (for planes on them).
        let outer = [("TOP", land), ("BOTTOM", self.bottom_land().normalized())];
        for (layer, outer_land) in outer {
            let base = self.plane_base(copper.then_some(outer_land));
            if copper {
                calls.push(pad(&outer_land, layer, "REGULAR"));
            }
            calls.push(pad(&base.grown(anti), layer, "ANTI"));
            if copper {
                if let Some(t) = thermal {
                    calls.push(pad(&base.grown(t), layer, "THERMAL"));
                }
            }
        }
        if !plain {
            let inner_layers = std::iter::once(("DEFAULT_INTERNAL".to_string(), self.inner_land()))
                .chain(
                    self.inner
                        .layers
                        .iter()
                        .map(|l| (l.layer.trim().to_ascii_uppercase(), l.land)),
                );
            for (layer, inner_land) in inner_layers {
                let inner_land = if copper { inner_land } else { None };
                let base = self.plane_base(inner_land);
                if let Some(inner_land) = inner_land {
                    calls.push(pad(&inner_land.normalized(), &layer, "REGULAR"));
                }
                calls.push(pad(&base.grown(anti), &layer, "ANTI"));
                if copper {
                    if let Some(t) = thermal {
                        calls.push(pad(&base.grown(t), &layer, "THERMAL"));
                    }
                }
            }
        }
        // Mask and paste. A bare hole's mask grows from the hole.
        let mask_base = |land: Land| if copper { land } else { self.plane_base(None) };
        for (side, layer_mask, layer_paste, side_land) in [
            (self.top, "SOLDERMASK_TOP", "PASTEMASK_TOP", land),
            (
                self.bottom,
                "SOLDERMASK_BOTTOM",
                "PASTEMASK_BOTTOM",
                self.bottom_land().normalized(),
            ),
        ] {
            if let Some(e) = side.mask {
                calls.push(pad(&mask_base(side_land).grown(e), layer_mask, "REGULAR"));
            }
            if copper {
                if let Some(e) = side.paste {
                    calls.push(pad(&side_land.grown(e), layer_paste, "REGULAR"));
                }
            }
        }
        calls
    }

    /// SKILL that creates the padstack, writes it to disk and checks it.
    pub fn skill(&self) -> String {
        let name = self.name();
        let (drill, fallback_drill) = match &self.drill {
            Some(drill) => (drill.skill(), drill.skill_round()),
            None => ("nil".to_string(), "nil".to_string()),
        };
        let mut text = String::new();
        let _ = writeln!(
            text,
            "seex_padstack(\"{name}\" {drill}\n    list({})\n    {fallback_drill}\n    list({}))",
            self.pad_calls(false).join("\n         "),
            self.pad_calls(true).join("\n         ")
        );
        let required: Vec<String> = self
            .required_layers()
            .iter()
            .map(|(layer, kind)| format!("list(\"{layer}\" \"{kind}\")"))
            .collect();
        let _ = writeln!(
            text,
            "seex_check_padstack(\"{name}\" list({}) {})",
            required.join(" "),
            if self.drill.is_some() { "t" } else { "nil" }
        );
        let land = self.land.normalized();
        if self.has_copper() && land.shape.has_corners() {
            let layer = if self.is_smd() && self.smd_bottom {
                "ETCH/BOTTOM"
            } else {
                "ETCH/TOP"
            };
            let _ = writeln!(
                text,
                "seex_check_figure(\"{name}\" \"{layer}\" \"{}\")",
                land.figure()
            );
        }
        if let Some(drill) = &self.drill {
            if drill.hole.is_slot() {
                let (w, h) = drill.hole.size();
                let _ = writeln!(
                    text,
                    "seex_check_slot(\"{name}\" {} {})",
                    f(w.max(h)),
                    f(w.min(h))
                );
            }
        }
        text
    }

    /// Inner-layer names the drawing needs before the padstack can be made.
    pub fn named_layers(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .inner
            .layers
            .iter()
            .map(|l| l.layer.trim().to_ascii_uppercase())
            .collect();
        names.dedup();
        names
    }

    /// Radius of a circle around everything the padstack puts on a board.
    fn reach(&self) -> f64 {
        let mut reach: f64 = 0.0;
        let mut add = |land: &Land| {
            let land = land.normalized();
            let (w, h) = land.half();
            let r = match land.shape {
                Shape::Round => w,
                Shape::Oblong => w.max(h),
                _ => (w * w + h * h).sqrt(),
            };
            reach = reach.max(r);
        };
        if self.has_copper() {
            add(&self.land);
            add(&self.bottom_land());
        }
        if self.drill.is_some() {
            add(&self.plane_base(None));
        }
        let mask_base = if self.has_copper() {
            self.land
        } else {
            self.plane_base(None)
        };
        for side in [self.top, self.bottom] {
            if let Some(e) = side.mask {
                add(&mask_base.grown(e));
            }
        }
        reach
    }
}

// ---------------------------------------------------------------------------
// Footprints

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FootprintKind {
    MountingHole,
    TestPoint,
    Fiducial,
}

impl FootprintKind {
    fn prefix(self) -> &'static str {
        match self {
            FootprintKind::MountingHole => "mh_",
            FootprintKind::TestPoint => "tp_",
            FootprintKind::Fiducial => "fid_",
        }
    }

    pub fn refdes(self) -> &'static str {
        match self {
            FootprintKind::MountingHole => "MH",
            FootprintKind::TestPoint => "TP",
            FootprintKind::Fiducial => "FID",
        }
    }
}

/// A footprint around one generated padstack.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintSpec {
    pub kind: FootprintKind,
    pub pad: PadstackSpec,
    /// Route and via keepout circle diameter around the pad; `None` for none.
    pub keepout: Option<f64>,
    /// Silkscreen circle around the copper.
    pub silk: bool,
}

/// Screw sizes for mounting holes: (name, plated hole, land), after the KiCad
/// library's MountingHole footprints (ISO 273 medium clearance holes).
pub const MOUNTING_PRESETS: [(&str, f64, f64); 6] = [
    ("M2", 2.2, 4.0),
    ("M2.5", 2.7, 5.0),
    ("M3", 3.2, 6.0),
    ("M4", 4.3, 8.0),
    ("M5", 5.3, 9.0),
    ("M6", 6.4, 11.0),
];

/// A finished footprint script.
#[derive(Debug, Clone)]
pub struct GeneratedFootprint {
    pub name: String,
    pub padstack: String,
    pub script: String,
}

impl FootprintSpec {
    pub fn validate(&self) -> Result<(), String> {
        self.pad.validate()?;
        if let Some(k) = self.keepout {
            if !(k.is_finite() && k > 0.0) {
                return Err("Keepout diameter must be larger than 0".to_string());
            }
        }
        Ok(())
    }

    /// `mh_c600h320`, `tp_c100`, `fid_c100m200k200`.
    pub fn name(&self) -> String {
        let mut full = format!("{}{}", self.kind.prefix(), self.pad.ipc_name());
        if let Some(k) = self.keepout {
            let _ = write!(full, "k{}", hundredths(k));
        }
        if full.len() <= MAX_NAME {
            full
        } else {
            let keep = MAX_NAME - 9;
            let head = full[..keep].trim_end_matches(['_', '-']);
            format!("{head}_{:08x}", fnv1a(&full))
        }
    }

    fn pin_number(&self) -> Option<String> {
        match self.kind {
            FootprintKind::Fiducial => None,
            // A bare hole connects nothing.
            FootprintKind::MountingHole if !self.pad.has_copper() => None,
            _ => Some("1".to_string()),
        }
    }

    /// Keepout layers: through every layer for drilled pads, the pad's side
    /// for SMD ones.
    fn keepout_layers(&self) -> Vec<&'static str> {
        if self.pad.is_smd() {
            if self.pad.smd_bottom {
                vec!["ROUTE KEEPOUT/BOTTOM", "VIA KEEPOUT/BOTTOM"]
            } else {
                vec!["ROUTE KEEPOUT/TOP", "VIA KEEPOUT/TOP"]
            }
        } else {
            vec!["ROUTE KEEPOUT/ALL", "VIA KEEPOUT/ALL"]
        }
    }

    pub fn build(&self) -> GeneratedFootprint {
        let name = self.name();
        let padstack = self.pad.name();
        let reach = self.pad.reach();
        let mut outer = reach;

        let mut body: Vec<String> = Vec::new();
        for layer in self.pad.named_layers() {
            body.push(format!("seex_inner_layer(\"{layer}\")"));
        }
        body.push(self.pad.skill());
        let number = self
            .pin_number()
            .map(|n| format!("\"{n}\""))
            .unwrap_or_else(|| "nil".to_string());
        body.push(format!("seex_pin(\"{padstack}\" {number} 0.0 0.0 0.0)"));

        if let Some(k) = self.keepout {
            let r = k / 2.0;
            outer = outer.max(r);
            for layer in self.keepout_layers() {
                body.push(format!("seex_disc(\"{layer}\" 0.0 0.0 {})", f(r)));
            }
        }
        let silk = self.silk && self.kind != FootprintKind::Fiducial;
        if silk {
            let r = reach + SILK_CLEARANCE + SILK_LINE_MM / 2.0;
            outer = outer.max(r + SILK_LINE_MM / 2.0);
            body.push(format!(
                "seex_circle(\"{SILK_TOP}\" {} 0.0 0.0 {} nil)",
                f(SILK_LINE_MM),
                f(r)
            ));
        }
        let b = outer + BOUND_CLEARANCE;
        let bound = (-b, -b, b, b);
        body.push(format!(
            "seex_place_bound(\"{PLACE_BOUND_TOP}\" {} {} {} {})",
            f(-b),
            f(-b),
            f(b),
            f(b)
        ));

        let refdes = format!("{}*", self.kind.refdes());
        let silk_height = silk_refdes_height(Some(bound), &refdes);
        body.push(format!(
            "seex_text_block({SILK_TEXT_BLOCK} {} {} {})",
            f(silk_height),
            f(silk_height),
            f(silk_stroke(silk_height))
        ));
        let (a_height, a_rot, a_x, a_y) = assembly_refdes_layout(Some(bound), &refdes);
        body.push(format!(
            "seex_text_block({ASSEMBLY_TEXT_BLOCK} {} {} {})",
            f(a_height),
            f(a_height),
            f(a_height * 0.15)
        ));
        if self.kind != FootprintKind::Fiducial {
            let y = b + REFDES_GAP_MM;
            body.push(format!(
                "seex_text(\"REF DES/SILKSCREEN_TOP\" \"{refdes}\" 0.0 {} {SILK_TEXT_BLOCK} 0.0)",
                f(y)
            ));
        }
        // Allegro refuses a symbol without a reference designator.
        body.push(format!(
            "seex_text(\"REF DES/ASSEMBLY_TOP\" \"{refdes}\" {} {} {ASSEMBLY_TEXT_BLOCK} {})",
            f(a_x),
            f(a_y),
            f(a_rot)
        ));

        let mut script = String::new();
        let _ = writeln!(
            script,
            ";; {name}.il - Allegro package symbol generated by the SeEx pad generator\n\
             ;; Padstack {padstack}; see the padstack generator notes in SeEx."
        );
        script.push_str(HELPERS);
        script.push_str(GEN_HELPERS);
        let _ = writeln!(
            script,
            "\nwhen( seex_begin(\"{name}\" {})",
            f((b + 3.0).max(5.0))
        );
        for line in &body {
            for (i, part) in line.lines().enumerate() {
                let indent = if i == 0 { "  " } else { "    " };
                let _ = writeln!(script, "{indent}{part}");
            }
        }
        let _ = writeln!(script, "  seex_end(\"{name}\")\n)");

        GeneratedFootprint {
            name,
            padstack,
            script,
        }
    }
}

/// SKILL that makes one padstack and writes its `.pad` file into the working
/// folder. The library script runs it in a scratch drawing; loaded by hand, it
/// needs a package symbol drawing (File > New > Package symbol).
///
/// Each padstack gets a drawing of its own: a padstack made while the drawing
/// has inner layers stores those layers one by one, so one that names an inner
/// layer would otherwise change the next.
pub fn padstack_script(spec: &PadstackSpec) -> String {
    let name = spec.name();
    let mut script = String::new();
    let _ = writeln!(
        script,
        ";; {name}.il - Allegro padstack generated by the SeEx pad generator\n\
         ;; Built by the library script. To build it alone, open a package symbol\n\
         ;; drawing in the folder the .pad file should go to, then\n\
         ;;     skill load(\"<this file>\")"
    );
    script.push_str(HELPERS);
    script.push_str(GEN_HELPERS);
    let _ = writeln!(script, "\nwhen( seex_begin(\"{name}\" 10.0)");
    for layer in spec.named_layers() {
        let _ = writeln!(script, "  seex_inner_layer(\"{}\")", skill_string(&layer));
    }
    for (i, line) in spec.skill().lines().enumerate() {
        let indent = if i == 0 { "  " } else { "    " };
        let _ = writeln!(script, "{indent}{line}");
    }
    let _ = writeln!(
        script,
        "  unless( boundp('seex_library_build) && seex_library_build\n    seex_padstack_done(\"{name}\")\n  )\n)"
    );
    script
}

/// Extra SKILL helpers of the generator (after the footprint helpers).
const GEN_HELPERS: &str = r#"
;; ------------------------------------------------------- SeEx pad generator
;; A pad of any figure; rounded and chamfered rectangles take a corner size and
;; the corners as Allegro names them ("UL UR LR LL").
procedure( seex_gpad(layer type figure w h corner corners)
  if( corners && corner > 0 then
    make_axlPadStackPad(?layer layer ?type type ?figure figure
                        ?figureSize list(seex_u(w) seex_u(h))
                        ?radius seex_u(corner) ?corners corners)
  else
    make_axlPadStackPad(?layer layer ?type type ?figure figure
                        ?figureSize list(seex_u(w) seex_u(h)))
  )
)

;; A drill: kind 'ROUND, 'OVAL (slot) or 'RECT (slot); tolerances in mm.
;; Slots get no ?drillDiameter, which would override ?slotSize.
procedure( seex_gdrill(kind w h plated tp tm)
  let( (plating tol drill)
    plating = if(plated 'PLATED 'NON_PLATED)
    tol = list(seex_u(tp) seex_u(tm))
    drill = car(errset(
      case( kind
        ( ROUND make_axlPadStackDrill(?drillDiameter seex_u(w) ?plating plating
                                      ?figure 'CIRCLE ?figureSize list(seex_u(w) seex_u(w))
                                      ?holeTolerance tol) )
        ( OVAL make_axlPadStackDrill(?holeType 'OVAL_SLOT ?slotSize list(seex_u(w) seex_u(h))
                                     ?plating plating ?holeTolerance tol ?holeToleranceY tol) )
        ( RECT make_axlPadStackDrill(?holeType 'RECTANGLE_SLOT ?slotSize list(seex_u(w) seex_u(h))
                                     ?plating plating ?holeTolerance tol ?holeToleranceY tol) )
      ) nil))
    unless( drill
      seex_note(sprintf(nil "drill %L %gx%g mm could not be made; a round drill is used" kind w h))
      drill = seex_drill(min(w h) plated)
    )
    drill
  )
)

;; Add an inner conductor layer to the drawing so a padstack can name it.
procedure( seex_inner_layer(name)
  unless( car(errset(axlIsLayer(strcat("ETCH/" name)) nil))
    errset(axlLayerCreateCrossSection("BOTTOM" "DIELECTRIC" "FR-4") nil)
    unless( car(errset(axlLayerCreateCrossSection("BOTTOM" "CONDUCTOR" "COPPER" name) nil))
      seex_note(sprintf(nil "inner layer %s could not be added; its pad is left out" name))
    )
  )
)

;; A filled circle (keepouts, place bounds), with a polygon if arcs fail.
procedure( seex_disc(layer cx cy r)
  let( (res path)
    res = errset(
      progn(
        path = axlPathStart(list(seex_xy(cx + r cy)) 0.0)
        path = axlPathArcCenter(path 0.0 seex_xy(cx - r cy) nil seex_xy(cx cy))
        path = axlPathArcCenter(path 0.0 seex_xy(cx + r cy) nil seex_xy(cx cy))
        axlDBCreateShape(path t layer)
      ) nil)
    unless( res && car(res)
      seex_shape(layer
        let( (pts) pts = nil
          for( i 0 47
            pts = cons(list(cx + r * cos(i * 3.14159265 / 24.0) cy + r * sin(i * 3.14159265 / 24.0)) pts))
          reverse(pts)))
    )
  )
)

;; Check the figure of a pad on one layer.
procedure( seex_check_figure(name layer figure)
  let( (ps found)
    ps = car(errset(axlLoadPadstack(name) nil))
    found = nil
    when( ps
      foreach( pad ps->pads
        when( pad->layer == layer && pad->type == "REGULAR" found = pad->figureName )
      )
    )
    unless( found == figure
      seex_note(sprintf(nil "padstack %s: %s is %L instead of %s" name layer found figure))
    )
  )
)

;; Check that a slot kept its size (long side, short side; mm).
procedure( seex_check_slot(name long short)
  let( (ps w h)
    ps = car(errset(axlLoadPadstack(name) nil))
    when( ps
      w = ps->drillSizeWidth
      h = ps->drillSizeHeight
      unless( numberp(w) && numberp(h)
              && abs(max(w h) - seex_u(long)) < seex_u(0.005)
              && abs(min(w h) - seex_u(short)) < seex_u(0.005)
        seex_note(sprintf(nil "padstack %s: slot is %L x %L instead of %g x %g mm" name w h long short))
      )
    )
  )
)

;; "OK" or the reason a padstack script failed.
procedure( seex_padstack_done(name)
  let( (status)
    status = cond(
      ( !car(errset(axlLoadPadstack(name) nil)) "padstack not created" )
      ( seex_errors > 0 sprintf(nil "%d problem(s) while building" seex_errors) )
      ( t "OK" )
    )
    printf("SeEx: padstack %s -> %s\n" name status)
    status
  )
)
;; --------------------------------------------------- end of the pad generator
"#;

// ---------------------------------------------------------------------------
// Writing the scripts

/// What to generate.
#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub padstacks: Vec<PadstackSpec>,
    pub footprints: Vec<FootprintSpec>,
    /// Replace scripts that are already in the library.
    pub overwrite: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GenerateReport {
    pub written: Vec<PathBuf>,
    /// Scripts left alone because the library has them already.
    pub kept: Vec<PathBuf>,
}

impl GenerateReport {
    pub fn summary(&self) -> String {
        let mut text = String::new();
        for path in &self.written {
            let _ = writeln!(text, "Written: {}", path.display());
        }
        for path in &self.kept {
            let _ = writeln!(text, "Already in the library (kept): {}", path.display());
        }
        text
    }
}

/// Write the scripts of `request` into the library's Allegro folder and
/// refresh its library script, which builds them with the other footprints.
pub fn generate(
    request: &GenerateRequest,
    layout: &CadenceLayout,
) -> crate::error::Result<GenerateReport> {
    for spec in &request.padstacks {
        spec.validate().map_err(AppError::Other)?;
    }
    for spec in &request.footprints {
        spec.validate().map_err(AppError::Other)?;
    }
    let mut report = GenerateReport::default();
    let mut place = |path: PathBuf, script: &str| -> crate::error::Result<()> {
        if path.exists() && !request.overwrite {
            report.kept.push(path);
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, script)?;
        report.written.push(path);
        Ok(())
    };
    for spec in &request.padstacks {
        let path = layout.padstack_dir().join(format!("{}.il", spec.name()));
        place(path, &padstack_script(spec))?;
    }
    for spec in &request.footprints {
        let footprint = spec.build();
        let path = layout.allegro_dir.join(format!("{}.il", footprint.name));
        place(path, &footprint.script)?;
    }
    layout.write_library_script()?;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Helpers

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// 1.5 -> "150" (hundredths of a millimetre, as IPC names write sizes).
fn hundredths(mm: f64) -> String {
    format!("{}", (mm * 100.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rounded(w: f64, h: f64, r: f64) -> Land {
        Land {
            shape: Shape::RoundRect,
            width: w,
            height: h,
            corner: r,
            corners: [true; 4],
        }
    }

    #[test]
    fn names_follow_the_ipc_convention() {
        let th = PadstackSpec::through(Land::round(1.5), Drill::plated(0.9));
        assert_eq!(th.name(), "c150h90");
        let smd = PadstackSpec::smd(Land::rect(1.45, 0.6));
        assert_eq!(smd.name(), "r145_60");
        let mut via = PadstackSpec::through(Land::round(0.5), Drill::plated(0.25));
        via.via = true;
        via.top = SideFinish::TENTED;
        via.bottom = SideFinish::TENTED;
        assert_eq!(via.name(), "v50h25m0");
        let npth = PadstackSpec::through(Land::round(1.0), Drill::non_plated(1.5));
        assert_eq!(npth.ipc_name(), "c100hn150");
        assert_eq!(PadstackSpec::bare_hole(3.2).name(), "hn320");
        let square = PadstackSpec::smd(rounded(3.0, 3.0, 0.25));
        assert_eq!(square.name(), "s300r25");
        let mut chamfer = PadstackSpec::smd(Land {
            shape: Shape::ChamferRect,
            corner: 0.5,
            corners: [true, false, false, false],
            ..Land::rect(1.0, 2.0)
        });
        assert_eq!(chamfer.name(), "r100_200cul50");
        chamfer.smd_bottom = true;
        assert_eq!(chamfer.name(), "xr100_200cul50");
        let oblong = PadstackSpec::through(
            Land::rect(4.0, 2.0).with_shape(Shape::Oblong),
            Drill {
                hole: Hole::Slot {
                    width: 3.0,
                    height: 1.0,
                    rectangular: false,
                },
                ..Drill::plated(1.0)
            },
        );
        assert_eq!(oblong.name(), "b400_200h300_100");
    }

    #[test]
    fn names_carry_what_differs_from_the_defaults() {
        let mut spec = PadstackSpec::through(Land::round(1.5), Drill::plated(0.9));
        spec.top.mask = Some(0.075);
        spec.bottom.mask = Some(0.075);
        assert_eq!(spec.name(), "c150h90m165");
        spec.inner.pad = InnerPad::Custom(Land::round(1.4));
        assert_eq!(spec.name(), "c150h90z140m165");
        spec.inner.pad = InnerPad::Custom(Land::rect(1.4, 1.4));
        assert_eq!(spec.name(), "c150h90zs140m165");
        spec.inner.pad = InnerPad::Same;
        spec.drill = Some(Drill {
            tol_plus: 0.05,
            tol_minus: 0.05,
            ..Drill::plated(0.9)
        });
        assert_eq!(spec.name(), "c150h90ht5m165");
        let mut paste = PadstackSpec::smd(Land::round(0.4));
        paste.top.paste = None;
        assert_eq!(paste.name(), "c40p0");
        paste.top.paste = Some(-0.05);
        assert_eq!(paste.name(), "c40p30");
        let mut tent_bottom = PadstackSpec::through(Land::round(0.6), Drill::plated(0.3));
        tent_bottom.bottom.mask = None;
        assert_eq!(tent_bottom.name(), "c60h30mx0");
    }

    #[test]
    fn long_names_are_shortened_with_a_hash() {
        let mut spec = PadstackSpec::through(
            Land {
                corners: [true, false, true, false],
                ..rounded(12.34, 5.67, 0.89)
            },
            Drill {
                hole: Hole::Slot {
                    width: 3.21,
                    height: 1.23,
                    rectangular: true,
                },
                tol_plus: 0.08,
                tol_minus: 0.03,
                plated: true,
            },
        );
        spec.inner.anti_clearance = 0.4;
        let full = spec.ipc_name();
        assert!(full.len() > MAX_NAME, "{full}");
        let name = spec.name();
        assert!(name.len() <= MAX_NAME, "{name}");
        let (head, hash) = name.rsplit_once('_').unwrap();
        assert!(full.starts_with(head));
        assert_eq!(hash.len(), 8);
    }

    #[test]
    fn rounded_and_chamfered_pads_use_allegro_corner_names() {
        let spec = PadstackSpec::smd(Land {
            corners: [true, true, false, false],
            ..rounded(1.6, 0.9, 0.2)
        });
        let skill = spec.skill();
        assert!(
            skill.contains(
                "seex_gpad(\"TOP\" 'REGULAR 'ROUNDED_RECTANGLE 1.6000 0.9000 0.2000 \"UL UR\")"
            ),
            "{skill}"
        );
        // Mask grows the radius with the outline.
        assert!(
            skill.contains("'ROUNDED_RECTANGLE 1.7000 1.0000 0.2500 \"UL UR\""),
            "{skill}"
        );
        // The fallback stack drops the corners.
        assert!(
            skill.contains("'RECTANGLE 1.6000 0.9000 0.0 nil"),
            "{skill}"
        );
        assert_eq!(spec.name(), "r160_90rulur20");
        assert!(
            skill.contains(
                "seex_check_figure(\"r160_90rulur20\" \"ETCH/TOP\" \"ROUNDED_RECTANGLE\")"
            )
        );
    }

    #[test]
    fn through_holes_get_inner_anti_and_thermal_pads() {
        let spec = PadstackSpec::through(Land::round(1.8), Drill::plated(1.0));
        let skill = spec.skill();
        for needle in [
            "seex_gdrill('ROUND 1.0000 1.0000 t 0.0000 0.0000)",
            "seex_gpad(\"DEFAULT_INTERNAL\" 'REGULAR 'CIRCLE 1.8000 1.8000",
            "seex_gpad(\"DEFAULT_INTERNAL\" 'ANTI 'CIRCLE 2.3000 2.3000",
            "seex_gpad(\"DEFAULT_INTERNAL\" 'THERMAL 'CIRCLE 2.3000 2.3000",
            "seex_gpad(\"SOLDERMASK_TOP\" 'REGULAR 'CIRCLE 1.9000 1.9000",
            "seex_gpad(\"SOLDERMASK_BOTTOM\" 'REGULAR 'CIRCLE 1.9000 1.9000",
        ] {
            assert!(skill.contains(needle), "{needle}\n{skill}");
        }
        assert!(!skill.contains("PASTEMASK"));
        // Inner land removed: anti-pad around the hole.
        let mut no_inner = spec.clone();
        no_inner.inner.pad = InnerPad::None;
        let skill = no_inner.skill();
        assert!(!skill.contains("\"DEFAULT_INTERNAL\" 'REGULAR"));
        assert!(skill.contains("seex_gpad(\"DEFAULT_INTERNAL\" 'ANTI 'CIRCLE 1.5000 1.5000"));
    }

    #[test]
    fn slots_and_tolerances_reach_the_drill() {
        let spec = PadstackSpec::through(
            Land::rect(2.6, 1.4).with_shape(Shape::Oblong),
            Drill {
                hole: Hole::Slot {
                    width: 2.0,
                    height: 0.8,
                    rectangular: true,
                },
                plated: true,
                tol_plus: 0.08,
                tol_minus: 0.05,
            },
        );
        let skill = spec.skill();
        assert!(
            skill.contains("seex_gdrill('RECT 2.0000 0.8000 t 0.0800 0.0500)"),
            "{skill}"
        );
        assert!(
            skill.contains("seex_gdrill('ROUND 0.8000 0.8000 t 0.0 0.0)"),
            "{skill}"
        );
        assert!(
            skill.contains("seex_check_slot(\"b260_140hr200_80ht8_5\" 2.0000 0.8000)"),
            "{skill}"
        );
    }

    #[test]
    fn per_layer_pads_add_their_layers() {
        let mut spec = PadstackSpec::through(Land::round(1.0), Drill::plated(0.5));
        spec.inner.layers = vec![
            LayerPad {
                layer: "gnd".to_string(),
                land: None,
            },
            LayerPad {
                layer: "PWR".to_string(),
                land: Some(Land::round(0.8)),
            },
        ];
        assert!(
            spec.validate().is_err(),
            "none needs No land as the default"
        );
        spec.inner.pad = InnerPad::None;
        assert!(spec.validate().is_ok());
        let script = padstack_script(&spec);
        assert!(script.contains("seex_inner_layer(\"GND\")"));
        assert!(script.contains("seex_inner_layer(\"PWR\")"));
        assert!(script.contains("seex_gpad(\"PWR\" 'REGULAR 'CIRCLE 0.8000"));
        assert!(!script.contains("seex_gpad(\"GND\" 'REGULAR"));
        assert!(script.contains("seex_gpad(\"GND\" 'ANTI 'CIRCLE 1.0000"));
        assert!(spec.name().starts_with("c100h50z0l"), "{}", spec.name());
        let mut bad = spec.clone();
        bad.inner.layers[0].layer = "my layer".to_string();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn land_must_cover_the_hole() {
        let spec = PadstackSpec::through(Land::round(0.8), Drill::plated(0.9));
        assert!(spec.validate().is_err());
        assert!(PadstackSpec::bare_hole(3.2).validate().is_ok());
    }

    #[test]
    fn mounting_hole_footprint() {
        let (_, hole, land) = MOUNTING_PRESETS[2];
        let spec = FootprintSpec {
            kind: FootprintKind::MountingHole,
            pad: PadstackSpec::through(Land::round(land), Drill::plated(hole)),
            keepout: Some(7.0),
            silk: true,
        };
        let fp = spec.build();
        assert_eq!(fp.name, "mh_c600h320k700");
        assert_eq!(fp.padstack, "c600h320");
        assert!(
            fp.script
                .contains("seex_pin(\"c600h320\" \"1\" 0.0 0.0 0.0)")
        );
        assert!(
            fp.script
                .contains("seex_disc(\"ROUTE KEEPOUT/ALL\" 0.0 0.0 3.5000)")
        );
        assert!(
            fp.script
                .contains("seex_disc(\"VIA KEEPOUT/ALL\" 0.0 0.0 3.5000)")
        );
        assert!(fp.script.contains("PACKAGE GEOMETRY/SILKSCREEN_TOP"));
        assert!(fp.script.contains("REF DES/ASSEMBLY_TOP\" \"MH*\""));
        // A bare hole is a mechanical pin.
        let bare = FootprintSpec {
            pad: PadstackSpec::bare_hole(3.2),
            ..spec
        };
        assert!(bare.build().script.contains("seex_pin(\"hn320\" nil"));
    }

    #[test]
    fn fiducial_footprint() {
        let mut pad = PadstackSpec::smd(Land::round(1.0));
        pad.top.mask = Some(0.5);
        pad.top.paste = None;
        let spec = FootprintSpec {
            kind: FootprintKind::Fiducial,
            pad,
            keepout: Some(2.0),
            silk: true,
        };
        let fp = spec.build();
        assert_eq!(fp.name, "fid_c100m200p0k200");
        assert!(fp.script.contains("seex_pin(\"c100m200p0\" nil"));
        assert!(fp.script.contains("ROUTE KEEPOUT/TOP"));
        assert!(!fp.script.contains("seex_circle(\""));
        assert!(!fp.script.contains("PASTEMASK_TOP"));
        assert!(!fp.script.contains("REF DES/SILKSCREEN_TOP"));
    }

    #[test]
    fn grown_rounded_land_keeps_concentric_corners() {
        let land = rounded(1.0, 0.6, 0.1).grown(0.05);
        assert!((land.width - 1.1).abs() < 1e-9);
        assert!((land.corner - 0.15).abs() < 1e-9);
        let shrunk = rounded(1.0, 0.6, 0.1).grown(-0.2);
        assert_eq!(shrunk.shape, Shape::Rect);
    }
}

#[cfg(test)]
mod library {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("seex_padgen_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn balanced(script: &str) -> bool {
        let code: String = script
            .lines()
            .filter(|line| !line.trim_start().starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n");
        code.matches('(').count() == code.matches(')').count()
    }

    #[test]
    fn padstack_script_builds_one_padstack() {
        let spec = PadstackSpec::through(Land::round(1.5), Drill::plated(0.9));
        let script = padstack_script(&spec);
        assert!(script.contains("when( seex_begin(\"c150h90\" 10.0)"));
        assert!(script.contains("seex_padstack(\"c150h90\""));
        assert!(script.contains("seex_padstack_done(\"c150h90\")"));
        assert!(balanced(&script));
        assert!(balanced(
            &FootprintSpec {
                kind: FootprintKind::MountingHole,
                pad: spec,
                keepout: Some(7.0),
                silk: true,
            }
            .build()
            .script
        ));
    }

    #[test]
    fn scripts_go_into_the_library_folder() {
        let out = temp_dir("library");
        let layout = CadenceLayout::new(&out, "Parts");
        let request = GenerateRequest {
            padstacks: vec![PadstackSpec::smd(Land::rect(1.45, 0.6))],
            footprints: vec![FootprintSpec {
                kind: FootprintKind::MountingHole,
                pad: PadstackSpec::through(Land::round(6.0), Drill::plated(3.2)),
                keepout: Some(7.0),
                silk: true,
            }],
            overwrite: false,
        };
        let report = generate(&request, &layout).unwrap();
        let padstack = layout.padstack_dir().join("r145_60.il");
        let footprint = layout.allegro_dir.join("mh_c600h320k700.il");
        assert_eq!(report.written, vec![padstack.clone(), footprint.clone()]);
        assert!(padstack.is_file() && footprint.is_file());
        let library = fs::read_to_string(layout.library_script()).unwrap();
        assert!(library.contains("list(\"mh_c600h320k700\") list(\"r145_60\"))"));

        // Without overwrite, what is there stays.
        fs::write(&footprint, "kept").unwrap();
        let again = generate(&request, &layout).unwrap();
        assert!(again.written.is_empty());
        assert_eq!(again.kept.len(), 2);
        assert_eq!(fs::read_to_string(&footprint).unwrap(), "kept");
        let replaced = generate(
            &GenerateRequest {
                overwrite: true,
                ..request
            },
            &layout,
        )
        .unwrap();
        assert_eq!(replaced.written.len(), 2);
        assert_ne!(fs::read_to_string(&footprint).unwrap(), "kept");
        let _ = fs::remove_dir_all(&out);
    }
}
