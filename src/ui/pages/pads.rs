//! The pad generator, a panel of the Export page for the Cadence target:
//! padstacks, mounting holes, test points and fiducials built from parameters
//! and written as SKILL scripts into the library's Allegro folder, where the
//! library script builds them with the exported footprints.

use std::collections::HashMap;

use gpui_kit::component::input::Input;
use gpui_kit::*;

use crate::cadence::padgen::{
    Drill, FootprintKind, FootprintSpec, GenerateRequest, Hole, InnerPad, Land, LayerPad,
    MOUNTING_PRESETS, PadstackSpec, Shape, SideFinish, generate,
};
use crate::cadence::workflow::CadenceLayout;
use crate::ui::app::{InputSlot, SeexUi};
use crate::ui::{components, locale, theme};

const BODY: f32 = 20.0;
const SMALL: f32 = 14.0;
const RUN_WIDTH: f32 = 390.0;
const LABEL_WIDTH: f32 = 230.0;

// ---------------------------------------------------------------------------
// Form

/// What the page generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Padstack,
    MountingHole,
    TestPoint,
    Fiducial,
}

/// The kind of padstack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    Smd,
    Through,
    Via,
    /// A non-plated hole without copper.
    Hole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoleKind {
    Round,
    Oval,
    Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InnerMode {
    Same,
    None,
    Custom,
}

/// The number boxes of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadField {
    Width,
    Height,
    Corner,
    Hole,
    SlotWidth,
    TolPlus,
    TolMinus,
    InnerWidth,
    InnerHeight,
    Anti,
    Thermal,
    MaskTop,
    MaskBottom,
    PasteTop,
    PasteBottom,
    Keepout,
    Layers,
}

impl PadField {
    pub fn placeholder(self) -> &'static str {
        match self {
            PadField::Keepout => locale::t("none", "无"),
            PadField::Layers => locale::t("e.g. GND=none, PWR=0.8", "例如 GND=none, PWR=0.8"),
            _ => "mm",
        }
    }
}

/// Everything the form holds.
#[derive(Debug, Clone)]
pub struct PadForm {
    pub output: Output,
    pub kind: PadKind,
    pub shape: Shape,
    pub corners: [bool; 4],
    pub hole: HoleKind,
    pub plated: bool,
    pub smd_bottom: bool,
    pub inner: InnerMode,
    pub thermal: bool,
    pub mask_top: bool,
    pub mask_bottom: bool,
    pub paste_top: bool,
    pub paste_bottom: bool,
    pub silk: bool,
    pub overwrite: bool,
    pub values: HashMap<PadField, String>,
}

impl Default for PadForm {
    fn default() -> Self {
        let values = [
            (PadField::Width, "1.6"),
            (PadField::Height, "1.0"),
            (PadField::Corner, "0.15"),
            (PadField::Hole, "1.0"),
            (PadField::SlotWidth, "0.6"),
            (PadField::TolPlus, "0"),
            (PadField::TolMinus, "0"),
            (PadField::InnerWidth, "1.4"),
            (PadField::InnerHeight, "1.0"),
            (PadField::Anti, "0.25"),
            (PadField::Thermal, "0.25"),
            (PadField::MaskTop, "0.05"),
            (PadField::MaskBottom, "0.05"),
            (PadField::PasteTop, "0"),
            (PadField::PasteBottom, "0"),
            (PadField::Keepout, ""),
            (PadField::Layers, ""),
        ]
        .into_iter()
        .map(|(field, value)| (field, value.to_string()))
        .collect();
        Self {
            output: Output::Padstack,
            kind: PadKind::Through,
            shape: Shape::Round,
            corners: [true; 4],
            hole: HoleKind::Round,
            plated: true,
            smd_bottom: false,
            inner: InnerMode::Same,
            thermal: true,
            mask_top: true,
            mask_bottom: true,
            paste_top: false,
            paste_bottom: false,
            silk: true,
            overwrite: false,
            values,
        }
    }
}

impl PadForm {
    pub fn value(&self, field: PadField) -> String {
        self.values.get(&field).cloned().unwrap_or_default()
    }

    /// Set a box; returns the field so the page can refresh what it shows.
    fn set(&mut self, field: PadField, value: &str) -> PadField {
        self.values.insert(field, value.to_string());
        field
    }

    fn number(&self, field: PadField, what: &str) -> Result<f64, String> {
        let text = self.value(field).trim().replace(',', ".");
        text.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{what}: \"{}\" is not a number", self.value(field).trim()))
    }

    fn optional(&self, field: PadField, what: &str) -> Result<Option<f64>, String> {
        if self.value(field).trim().is_empty() {
            Ok(None)
        } else {
            self.number(field, what).map(Some)
        }
    }

    /// Switch the pad kind, with the finish that usually goes with it.
    /// Returns the fields whose boxes changed.
    pub fn set_kind(&mut self, kind: PadKind) -> Vec<PadField> {
        self.kind = kind;
        match kind {
            PadKind::Smd => {
                self.mask_top = true;
                self.mask_bottom = true;
                self.paste_top = true;
                self.paste_bottom = true;
            }
            PadKind::Through => {
                self.plated = true;
                self.mask_top = true;
                self.mask_bottom = true;
                self.paste_top = false;
                self.paste_bottom = false;
            }
            PadKind::Via => {
                self.plated = true;
                self.mask_top = false;
                self.mask_bottom = false;
                self.paste_top = false;
                self.paste_bottom = false;
                self.shape = Shape::Round;
                self.hole = HoleKind::Round;
                return vec![
                    self.set(PadField::Width, "0.6"),
                    self.set(PadField::Hole, "0.3"),
                ];
            }
            PadKind::Hole => {
                self.plated = false;
                self.mask_top = true;
                self.mask_bottom = true;
                self.paste_top = false;
                self.paste_bottom = false;
            }
        }
        Vec::new()
    }

    /// Switch what is generated and load its usual parameters. Returns the
    /// fields whose boxes changed.
    pub fn set_output(&mut self, output: Output) -> Vec<PadField> {
        self.output = output;
        let mut changed = Vec::new();
        match output {
            Output::Padstack => {}
            Output::MountingHole => {
                changed.extend(self.set_kind(PadKind::Through));
                self.shape = Shape::Round;
                self.hole = HoleKind::Round;
                self.silk = true;
                changed.extend(self.apply_preset(2));
                changed.push(self.set(PadField::Keepout, ""));
            }
            Output::TestPoint => {
                changed.extend(self.set_kind(PadKind::Smd));
                self.shape = Shape::Round;
                self.paste_top = false;
                self.paste_bottom = false;
                self.silk = false;
                changed.push(self.set(PadField::Width, "1.0"));
                changed.push(self.set(PadField::MaskTop, "0.05"));
                changed.push(self.set(PadField::MaskBottom, "0.05"));
                changed.push(self.set(PadField::Keepout, ""));
            }
            Output::Fiducial => {
                changed.extend(self.set_kind(PadKind::Smd));
                self.shape = Shape::Round;
                self.paste_top = false;
                self.paste_bottom = false;
                self.silk = false;
                changed.push(self.set(PadField::Width, "1.0"));
                changed.push(self.set(PadField::MaskTop, "0.5"));
                changed.push(self.set(PadField::MaskBottom, "0.5"));
                changed.push(self.set(PadField::Keepout, "2.0"));
            }
        }
        changed
    }

    /// Load a mounting-hole size from [`MOUNTING_PRESETS`].
    pub fn apply_preset(&mut self, index: usize) -> Vec<PadField> {
        let Some((_, hole, land)) = MOUNTING_PRESETS.get(index) else {
            return Vec::new();
        };
        vec![
            self.set(PadField::Hole, &format!("{hole}")),
            self.set(PadField::Width, &format!("{land}")),
            self.set(PadField::InnerWidth, &format!("{land}")),
        ]
    }

    /// Which pad kinds the current output allows.
    pub fn kinds(&self) -> &'static [PadKind] {
        match self.output {
            Output::Padstack => &[PadKind::Smd, PadKind::Through, PadKind::Via, PadKind::Hole],
            Output::MountingHole => &[PadKind::Through, PadKind::Hole],
            Output::TestPoint => &[PadKind::Smd, PadKind::Through],
            Output::Fiducial => &[PadKind::Smd],
        }
    }

    fn land(&self, width: f64, height: f64) -> Result<Land, String> {
        let corner = if self.shape.has_corners() {
            self.number(PadField::Corner, "Corner")?
        } else {
            0.0
        };
        Ok(Land {
            shape: self.shape,
            width,
            height: if self.shape.has_height() {
                height
            } else {
                width
            },
            corner,
            corners: self.corners,
        })
    }

    fn drill(&self) -> Result<Drill, String> {
        let length = self.number(PadField::Hole, "Hole")?;
        let hole = match self.hole {
            HoleKind::Round => Hole::Round(length),
            HoleKind::Oval | HoleKind::Rect => Hole::Slot {
                width: length,
                height: self.number(PadField::SlotWidth, "Slot width")?,
                rectangular: self.hole == HoleKind::Rect,
            },
        };
        Ok(Drill {
            hole,
            plated: match self.kind {
                PadKind::Via => true,
                PadKind::Hole => false,
                _ => self.plated,
            },
            tol_plus: self.number(PadField::TolPlus, "Tolerance +")?.abs(),
            tol_minus: self.number(PadField::TolMinus, "Tolerance -")?.abs(),
        })
    }

    /// `GND=none, PWR=0.8, SIG3=1.2x0.8` -> per-layer lands, in the outer
    /// land's shape.
    fn layer_pads(&self, outer: &Land) -> Result<Vec<LayerPad>, String> {
        let text = self.value(PadField::Layers);
        let mut pads = Vec::new();
        for entry in text
            .split([',', ';', '\n'])
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            let (layer, size) = entry
                .split_once(['=', ':'])
                .ok_or_else(|| format!("Per-layer pads: write \"LAYER=size\", not \"{entry}\""))?;
            let size = size.trim().to_ascii_lowercase();
            let land = if matches!(size.as_str(), "none" | "0" | "-" | "") {
                None
            } else {
                let (w, h) = match size.split_once(['x', '*']) {
                    Some((w, h)) => (w.trim().to_string(), h.trim().to_string()),
                    None => (size.clone(), size.clone()),
                };
                let parse = |v: &str| {
                    v.replace(',', ".")
                        .parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .ok_or_else(|| format!("Per-layer pads: \"{entry}\" has no usable size"))
                };
                let (w, h) = (parse(&w)?, parse(&h)?);
                let mut land = *outer;
                land.width = w;
                land.height = if outer.shape.has_height() { h } else { w };
                Some(land)
            };
            pads.push(LayerPad {
                layer: layer.trim().to_string(),
                land,
            });
        }
        Ok(pads)
    }

    /// The padstack the form describes.
    pub fn padstack(&self) -> Result<PadstackSpec, String> {
        let land = self.land(
            self.number(PadField::Width, "Width")?,
            if self.shape.has_height() {
                self.number(PadField::Height, "Height")?
            } else {
                0.0
            },
        )?;
        let finish =
            |mask: bool, mask_field, paste: bool, paste_field| -> Result<SideFinish, String> {
                Ok(SideFinish {
                    mask: if mask {
                        Some(self.number(mask_field, "Mask")?)
                    } else {
                        None
                    },
                    paste: if paste {
                        Some(self.number(paste_field, "Paste")?)
                    } else {
                        None
                    },
                })
            };
        let top = finish(
            self.mask_top,
            PadField::MaskTop,
            self.paste_top,
            PadField::PasteTop,
        )?;
        let bottom = finish(
            self.mask_bottom,
            PadField::MaskBottom,
            self.paste_bottom,
            PadField::PasteBottom,
        )?;
        let mut spec = match self.kind {
            PadKind::Smd => {
                let mut spec = PadstackSpec::smd(land);
                spec.smd_bottom = self.smd_bottom;
                // The pad's own side takes the top boxes.
                spec.top = top;
                spec.bottom = top;
                return spec.validate().map(|_| spec);
            }
            PadKind::Through | PadKind::Via => PadstackSpec::through(land, self.drill()?),
            PadKind::Hole => {
                let drill = self.drill()?;
                let (w, h) = drill.hole.size();
                let mut spec = PadstackSpec::bare_hole(w.min(h));
                spec.drill = Some(drill);
                spec
            }
        };
        spec.via = self.kind == PadKind::Via;
        spec.top = SideFinish { paste: None, ..top };
        spec.bottom = SideFinish {
            paste: None,
            ..bottom
        };
        if self.kind != PadKind::Hole {
            spec.top.paste = top.paste;
            spec.bottom.paste = bottom.paste;
        }
        spec.inner.pad = match self.inner {
            InnerMode::Same => InnerPad::Same,
            InnerMode::None => InnerPad::None,
            InnerMode::Custom => InnerPad::Custom(self.land(
                self.number(PadField::InnerWidth, "Inner width")?,
                if self.shape.has_height() {
                    self.number(PadField::InnerHeight, "Inner height")?
                } else {
                    0.0
                },
            )?),
        };
        spec.inner.anti_clearance = self.number(PadField::Anti, "Anti-pad clearance")?;
        spec.inner.thermal_clearance = if self.thermal {
            Some(self.number(PadField::Thermal, "Thermal clearance")?)
        } else {
            None
        };
        spec.inner.layers = self.layer_pads(&land)?;
        spec.validate().map(|_| spec)
    }

    /// The footprint the form describes, when it describes one.
    pub fn footprint(&self) -> Result<Option<FootprintSpec>, String> {
        let kind = match self.output {
            Output::Padstack => return Ok(None),
            Output::MountingHole => FootprintKind::MountingHole,
            Output::TestPoint => FootprintKind::TestPoint,
            Output::Fiducial => FootprintKind::Fiducial,
        };
        let spec = FootprintSpec {
            kind,
            pad: self.padstack()?,
            keepout: self.optional(PadField::Keepout, "Keepout")?,
            silk: self.silk,
        };
        spec.validate().map(|_| Some(spec))
    }

    /// Footprint and padstack names of what the form describes (the footprint
    /// name is empty for a padstack alone).
    pub fn names(&self) -> Result<(String, String), String> {
        if let Some(footprint) = self.footprint()? {
            let built = footprint.build();
            return Ok((built.name, built.padstack));
        }
        Ok((String::new(), self.padstack()?.name()))
    }
}

// ---------------------------------------------------------------------------
// Panel state

/// Everything the pad generator keeps between frames.
pub struct PadsUi {
    pub form: PadForm,
    /// Shown in place of the failed-ID list on the Export page.
    pub open: bool,
    pub result: Option<Result<String, String>>,
    /// Boxes whose value changed in code, to be written into their inputs.
    pub refresh: Vec<PadField>,
}

impl PadsUi {
    pub fn new() -> Self {
        Self {
            form: PadForm::default(),
            open: false,
            result: None,
            refresh: Vec::new(),
        }
    }
}

impl Default for PadsUi {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Drawing the panel

fn title(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(BODY))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::text_color())
        .child(text.into())
}

fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(SMALL))
        .text_color(theme::muted())
        .child(text.into())
}

/// A compact toggle chip.
fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (background, text) = if selected {
        (theme::primary(), theme::white())
    } else {
        (theme::field_bg(), theme::text_color())
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(theme::RADIUS_SM))
        .bg(background)
        .border(px(1.0))
        .border_color(theme::border())
        .text_size(px(SMALL))
        .text_color(text)
        .cursor_pointer()
        .hover(move |style| {
            if selected {
                style
            } else {
                style.bg(theme::widget_hover_bg())
            }
        })
        .child(label.into())
        .on_click(on_click)
}

fn chips_row() -> Div {
    div().flex().flex_row().flex_wrap().gap(px(6.0))
}

fn group(heading: &'static str) -> Div {
    div().flex().flex_col().gap(px(8.0)).child(title(heading))
}

impl SeexUi {
    /// The bottom section of the Export page while the pad generator is open.
    pub(crate) fn pads_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .w_full()
            .child(
                div()
                    .text_size(px(BODY))
                    .text_color(theme::text_color())
                    .child(locale::t("Pad generator", "焊盘生成器")),
            )
            .child(div().flex_1())
            .child(components::button(
                "close-pad-generator",
                locale::t("Failed IDs", "失败 ID"),
                true,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.pads.open = false;
                    cx.notify();
                }),
            ));
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .size_full()
            .child(header)
            .child(muted(locale::t(
                "Padstacks, mounting holes, test points and fiducials for the Allegro library.",
                "为 Allegro 库生成焊盘栈、安装孔、测试点和基准点。",
            )))
            .child(self.pads_panel(window, cx));
        components::section_fill(body).into_any_element()
    }

    /// The pad generator panel: the form, and beside it what it makes and where
    /// it goes.
    pub(crate) fn pads_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Values set by presets reach their boxes here, where a window exists.
        for field in std::mem::take(&mut self.pads.refresh) {
            let value = self.pads.form.value(field);
            let input = self.input(InputSlot::Pad(field), window, cx);
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        div()
            .flex()
            .flex_row()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(16.0))
            .child(self.pads_form(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(RUN_WIDTH))
                    .flex_shrink_0()
                    .h_full()
                    .child(self.pads_run(cx)),
            )
            .into_any_element()
    }

    fn pad_box(
        &mut self,
        field: PadField,
        label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let input = self.input(InputSlot::Pad(field), window, cx);
        let width = if field == PadField::Layers {
            220.0
        } else {
            110.0
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .w(px(LABEL_WIDTH))
                    .flex_shrink_0()
                    .text_size(px(SMALL))
                    .text_color(theme::text_color())
                    .child(label),
            )
            .child(div().w(px(width)).child(Input::new(&input).h(px(30.0))))
    }

    fn pads_form(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let form = self.pads.form.clone();
        let mut body = div()
            .id("pads-form-scroll")
            .flex()
            .flex_col()
            .gap(px(16.0))
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_y_scroll();

        // What to make.
        let mut outputs = chips_row();
        for (output, label) in [
            (Output::Padstack, locale::t("Padstack", "焊盘栈")),
            (Output::MountingHole, locale::t("Mounting hole", "安装孔")),
            (Output::TestPoint, locale::t("Test point", "测试点")),
            (Output::Fiducial, locale::t("Fiducial", "基准点")),
        ] {
            outputs = outputs.child(chip(
                SharedString::from(format!("pads-output-{output:?}")),
                label,
                form.output == output,
                cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    let changed = this.pads.form.set_output(output);
                    this.pads.refresh.extend(changed);
                    cx.notify();
                }),
            ));
        }
        let mut kinds = chips_row();
        for kind in form.kinds() {
            let kind = *kind;
            let label = match kind {
                PadKind::Smd => "SMD",
                PadKind::Through => locale::t("Through-hole", "通孔"),
                PadKind::Via => locale::t("Via", "过孔"),
                PadKind::Hole => locale::t("Non-plated hole", "非金属化孔"),
            };
            kinds = kinds.child(chip(
                SharedString::from(format!("pads-kind-{kind:?}")),
                label,
                form.kind == kind,
                cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    let changed = this.pads.form.set_kind(kind);
                    this.pads.refresh.extend(changed);
                    cx.notify();
                }),
            ));
        }
        let mut make = group(locale::t("Make", "生成")).child(outputs).child(kinds);
        if form.output == Output::MountingHole {
            let mut presets = chips_row();
            for (index, (name, hole, land)) in MOUNTING_PRESETS.iter().enumerate() {
                let selected = form.value(PadField::Hole) == format!("{hole}")
                    && form.value(PadField::Width) == format!("{land}");
                presets = presets.child(chip(
                    SharedString::from(format!("pads-preset-{index}")),
                    *name,
                    selected,
                    cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                        let changed = this.pads.form.apply_preset(index);
                        this.pads.refresh.extend(changed);
                        cx.notify();
                    }),
                ));
            }
            make = make.child(presets);
        }
        if form.kind == PadKind::Smd {
            make = make.child(chips_row().child(chip(
                "pads-smd-bottom",
                locale::t("Bottom side", "底层"),
                form.smd_bottom,
                cx.listener(|this, _event: &ClickEvent, _window, cx| {
                    this.pads.form.smd_bottom = !this.pads.form.smd_bottom;
                    cx.notify();
                }),
            )));
        }
        body = body.child(make);

        // Land.
        if form.kind != PadKind::Hole {
            let mut shapes = chips_row();
            for shape in Shape::ALL {
                let label = match shape {
                    Shape::Round => locale::t("Round", "圆形"),
                    Shape::Square => locale::t("Square", "方形"),
                    Shape::Rect => locale::t("Rectangle", "矩形"),
                    Shape::Oblong => locale::t("Oblong", "长圆形"),
                    Shape::RoundRect => locale::t("Rounded rect", "圆角矩形"),
                    Shape::ChamferRect => locale::t("Chamfered rect", "倒角矩形"),
                };
                shapes = shapes.child(chip(
                    SharedString::from(format!("pads-shape-{}", shape.key())),
                    label,
                    form.shape == shape,
                    cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                        this.pads.form.shape = shape;
                        cx.notify();
                    }),
                ));
            }
            let mut land = group(locale::t("Land", "焊盘"))
                .child(shapes)
                .child(self.pad_box(
                    PadField::Width,
                    if form.shape.has_height() {
                        locale::t("Width (mm)", "宽度 (mm)")
                    } else {
                        locale::t("Size / diameter (mm)", "尺寸/直径 (mm)")
                    },
                    window,
                    cx,
                ));
            if form.shape.has_height() {
                land = land.child(self.pad_box(
                    PadField::Height,
                    locale::t("Height (mm)", "高度 (mm)"),
                    window,
                    cx,
                ));
            }
            if form.shape.has_corners() {
                land = land.child(self.pad_box(
                    PadField::Corner,
                    if form.shape == Shape::RoundRect {
                        locale::t("Corner radius (mm)", "圆角半径 (mm)")
                    } else {
                        locale::t("Chamfer (mm)", "倒角 (mm)")
                    },
                    window,
                    cx,
                ));
                let mut corners = chips_row();
                for (index, label) in [
                    locale::t("Upper left", "左上"),
                    locale::t("Upper right", "右上"),
                    locale::t("Lower right", "右下"),
                    locale::t("Lower left", "左下"),
                ]
                .into_iter()
                .enumerate()
                {
                    corners = corners.child(chip(
                        SharedString::from(format!("pads-corner-{index}")),
                        label,
                        form.corners[index],
                        cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                            this.pads.form.corners[index] = !this.pads.form.corners[index];
                            cx.notify();
                        }),
                    ));
                }
                land = land.child(corners);
            }
            body = body.child(land);
        }

        // Hole.
        if form.kind != PadKind::Smd {
            let mut holes = chips_row();
            for (hole, label) in [
                (HoleKind::Round, locale::t("Round drill", "圆孔")),
                (HoleKind::Oval, locale::t("Oval slot", "椭圆槽")),
                (HoleKind::Rect, locale::t("Rectangular slot", "矩形槽")),
            ] {
                holes = holes.child(chip(
                    SharedString::from(format!("pads-hole-{hole:?}")),
                    label,
                    form.hole == hole,
                    cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                        this.pads.form.hole = hole;
                        cx.notify();
                    }),
                ));
            }
            if form.kind == PadKind::Through {
                holes = holes.child(chip(
                    "pads-plated",
                    locale::t("Plated", "金属化"),
                    form.plated,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.pads.form.plated = !this.pads.form.plated;
                        cx.notify();
                    }),
                ));
            }
            let mut hole = group(locale::t("Hole", "钻孔"))
                .child(holes)
                .child(self.pad_box(
                    PadField::Hole,
                    if form.hole == HoleKind::Round {
                        locale::t("Finished hole (mm)", "成品孔径 (mm)")
                    } else {
                        locale::t("Slot length (mm)", "槽长 (mm)")
                    },
                    window,
                    cx,
                ));
            if form.hole != HoleKind::Round {
                hole = hole.child(self.pad_box(
                    PadField::SlotWidth,
                    locale::t("Slot width (mm)", "槽宽 (mm)"),
                    window,
                    cx,
                ));
            }
            hole = hole
                .child(self.pad_box(
                    PadField::TolPlus,
                    locale::t("Tolerance + (mm)", "公差 + (mm)"),
                    window,
                    cx,
                ))
                .child(self.pad_box(
                    PadField::TolMinus,
                    locale::t("Tolerance − (mm)", "公差 − (mm)"),
                    window,
                    cx,
                ));
            body = body.child(hole);

            // Inner layers.
            let mut inner = group(locale::t("Inner layers", "内层"));
            if form.kind != PadKind::Hole {
                let mut modes = chips_row();
                for (mode, label) in [
                    (InnerMode::Same, locale::t("Land as outer", "同外层焊盘")),
                    (InnerMode::Custom, locale::t("Own land", "自定义焊盘")),
                    (InnerMode::None, locale::t("No land", "无焊盘")),
                ] {
                    modes = modes.child(chip(
                        SharedString::from(format!("pads-inner-{mode:?}")),
                        label,
                        form.inner == mode,
                        cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                            this.pads.form.inner = mode;
                            cx.notify();
                        }),
                    ));
                }
                modes = modes.child(chip(
                    "pads-thermal",
                    locale::t("Thermal relief", "热焊盘"),
                    form.thermal,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.pads.form.thermal = !this.pads.form.thermal;
                        cx.notify();
                    }),
                ));
                inner = inner.child(modes);
                if form.inner == InnerMode::Custom {
                    inner = inner.child(self.pad_box(
                        PadField::InnerWidth,
                        locale::t("Inner width / diameter (mm)", "内层宽度/直径 (mm)"),
                        window,
                        cx,
                    ));
                    if form.shape.has_height() {
                        inner = inner.child(self.pad_box(
                            PadField::InnerHeight,
                            locale::t("Inner height (mm)", "内层高度 (mm)"),
                            window,
                            cx,
                        ));
                    }
                }
            }
            inner = inner.child(self.pad_box(
                PadField::Anti,
                locale::t("Anti-pad clearance per side", "反焊盘单边间距"),
                window,
                cx,
            ));
            if form.kind != PadKind::Hole && form.thermal {
                inner = inner.child(self.pad_box(
                    PadField::Thermal,
                    locale::t("Thermal relief per side", "热焊盘单边外扩"),
                    window,
                    cx,
                ));
            }
            if form.output == Output::Padstack && form.kind != PadKind::Hole {
                inner = inner
                    .child(self.pad_box(
                        PadField::Layers,
                        locale::t("Per-layer lands", "分层焊盘"),
                        window,
                        cx,
                    ))
                    .child(muted(locale::t(
                        "LAYER=size (or WxH) by cross-section name. With \"No land\" chosen above, only the layers listed get a land.",
                        "按叠层名填写 层=尺寸（或 宽x高）。上面选\"无焊盘\"时，只有列出的层有焊盘。",
                    )));
            }
            body = body.child(inner);
        }

        // Mask and paste.
        let smd = form.kind == PadKind::Smd;
        let mut finish = group(locale::t("Solder mask and paste", "阻焊与钢网"));
        let sides: &[(bool, &'static str)] = if smd {
            &[(true, "")]
        } else {
            &[(true, "top"), (false, "bottom")]
        };
        for &(top, _) in sides {
            let (mask_on, paste_on, mask_field, paste_field) = if top {
                (
                    form.mask_top,
                    form.paste_top,
                    PadField::MaskTop,
                    PadField::PasteTop,
                )
            } else {
                (
                    form.mask_bottom,
                    form.paste_bottom,
                    PadField::MaskBottom,
                    PadField::PasteBottom,
                )
            };
            let side_label = match (smd, top) {
                (true, _) => locale::t("Pad side", "焊盘面"),
                (false, true) => locale::t("Top", "顶层"),
                (false, false) => locale::t("Bottom", "底层"),
            };
            let mut toggles = chips_row().child(muted(side_label)).child(chip(
                SharedString::from(format!("pads-mask-{top}")),
                locale::t("Mask opening", "阻焊开窗"),
                mask_on,
                cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    let form = &mut this.pads.form;
                    if top {
                        form.mask_top = !form.mask_top;
                    } else {
                        form.mask_bottom = !form.mask_bottom;
                    }
                    cx.notify();
                }),
            ));
            if form.kind != PadKind::Hole {
                toggles = toggles.child(chip(
                    SharedString::from(format!("pads-paste-{top}")),
                    locale::t("Paste", "钢网"),
                    paste_on,
                    cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                        let form = &mut this.pads.form;
                        if top {
                            form.paste_top = !form.paste_top;
                        } else {
                            form.paste_bottom = !form.paste_bottom;
                        }
                        cx.notify();
                    }),
                ));
            }
            finish = finish.child(toggles);
            if mask_on {
                finish = finish.child(self.pad_box(
                    mask_field,
                    locale::t("Mask per side (− shrinks)", "阻焊单边外扩（负值内缩）"),
                    window,
                    cx,
                ));
            } else {
                finish = finish.child(muted(locale::t(
                    "Tented: no opening on this side.",
                    "盖油：此面不开窗。",
                )));
            }
            if paste_on && form.kind != PadKind::Hole {
                finish = finish.child(self.pad_box(
                    paste_field,
                    locale::t("Paste per side (− shrinks)", "钢网单边外扩（负值内缩）"),
                    window,
                    cx,
                ));
            }
        }
        body = body.child(finish);

        // Footprint extras.
        if form.output != Output::Padstack {
            let mut extras = group(locale::t("Footprint", "封装")).child(self.pad_box(
                PadField::Keepout,
                locale::t("Keepout diameter (mm)", "禁布区直径 (mm)"),
                window,
                cx,
            ));
            if form.output != Output::Fiducial {
                extras = extras.child(chips_row().child(chip(
                    "pads-silk",
                    locale::t("Silkscreen circle", "丝印圆"),
                    form.silk,
                    cx.listener(|this, _event: &ClickEvent, _window, cx| {
                        this.pads.form.silk = !this.pads.form.silk;
                        cx.notify();
                    }),
                )));
            }
            body = body.child(extras);
        }

        body.into_any_element()
    }

    /// The library the scripts go into: the Export page's folder and name.
    fn pads_layout(&self) -> Option<CadenceLayout> {
        let state = self.state.lock().ok()?;
        let output = state.export_output_path.trim();
        let name = state.export_library_name.trim();
        (!output.is_empty() && !name.is_empty())
            .then(|| CadenceLayout::new(std::path::Path::new(output), name))
    }

    fn pads_run(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let layout = self.pads_layout();
        let form = self.pads.form.clone();
        let names = form.names();

        let mut body = div().flex().flex_col().gap(px(10.0)).w_full();
        match &names {
            Ok((footprint, padstack)) => {
                if footprint.is_empty() {
                    body = body.child(title(padstack.clone()));
                } else {
                    body = body.child(title(footprint.clone())).child(muted(format!(
                        "{} {padstack}",
                        locale::t("Padstack", "焊盘栈")
                    )));
                }
                body = body.child(muted(locale::t(
                    "Named after the IPC-7352 padstack convention.",
                    "按 IPC-7352 焊盘栈命名规则自动命名。",
                )));
            }
            Err(err) => {
                body = body.child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(theme::danger())
                        .child(err.clone()),
                );
            }
        }
        match &layout {
            Some(layout) => {
                body = body
                    .child(muted(format!(
                        "{} {}",
                        locale::t("Written to:", "写入："),
                        layout.allegro_dir.display()
                    )))
                    .child(muted(locale::t(
                        "Loading the library script in Allegro PCB Editor builds them with the exported footprints.",
                        "在 Allegro PCB Editor 中加载库脚本时，会与导出的封装一起生成。",
                    )));
            }
            None => {
                body = body.child(muted(locale::t(
                    "Set the export directory and library name first.",
                    "请先设置导出目录和库名称。",
                )));
            }
        }
        body = body.child(chips_row().child(chip(
            "pads-overwrite",
            locale::t("Replace existing scripts", "覆盖已有脚本"),
            form.overwrite,
            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                this.pads.form.overwrite = !this.pads.form.overwrite;
                cx.notify();
            }),
        )));
        body = body.child(div().child(components::button(
            "pads-generate",
            locale::t("Generate", "生成"),
            names.is_ok() && layout.is_some(),
            cx.listener(|this, _event: &ClickEvent, _window, cx| {
                this.generate_pads();
                cx.notify();
            }),
        )));
        match &self.pads.result {
            Some(Ok(summary)) => {
                body = body
                    .child(components::chip(
                        locale::t("Done", "完成"),
                        theme::success(),
                    ))
                    .child(
                        div()
                            .w_full()
                            .p(px(8.0))
                            .rounded(px(theme::RADIUS_SM))
                            .bg(theme::field_bg())
                            .font_family(self.mono_family.clone())
                            .text_size(px(13.0))
                            .text_color(theme::text_color())
                            .child(summary.clone()),
                    );
            }
            Some(Err(err)) => {
                body = body.child(
                    div()
                        .text_size(px(SMALL))
                        .text_color(theme::danger())
                        .child(err.clone()),
                );
            }
            None => {}
        }
        div()
            .id("pads-run-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(body)
            .into_any_element()
    }

    fn generate_pads(&mut self) {
        let Some(layout) = self.pads_layout() else {
            return;
        };
        let form = &self.pads.form;
        let request = match form.footprint() {
            Ok(Some(footprint)) => Ok((Vec::new(), vec![footprint])),
            Ok(None) => form.padstack().map(|spec| (vec![spec], Vec::new())),
            Err(err) => Err(err),
        }
        .map(|(padstacks, footprints)| GenerateRequest {
            padstacks,
            footprints,
            overwrite: form.overwrite,
        });
        self.pads.result = Some(request.and_then(|request| {
            generate(&request, &layout)
                .map(|report| report.summary())
                .map_err(|err| err.to_string())
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{HoleKind, InnerMode, Output, PadField, PadForm, PadKind};
    use crate::cadence::padgen::Shape;

    #[test]
    fn default_form_is_a_plated_through_hole() {
        let form = PadForm::default();
        let spec = form.padstack().unwrap();
        assert_eq!(spec.name(), "c160h100");
    }

    #[test]
    fn presets_fill_the_form() {
        let mut form = PadForm::default();
        form.set_output(Output::MountingHole);
        let footprint = form.footprint().unwrap().unwrap();
        assert_eq!(footprint.name(), "mh_c600h320");
        form.set_output(Output::Fiducial);
        let footprint = form.footprint().unwrap().unwrap();
        assert_eq!(footprint.name(), "fid_c100m200p0k200");
        form.set_output(Output::TestPoint);
        assert_eq!(form.footprint().unwrap().unwrap().name(), "tp_c100p0");
    }

    #[test]
    fn via_is_tented_by_default() {
        let mut form = PadForm::default();
        form.set_kind(PadKind::Via);
        assert_eq!(form.padstack().unwrap().name(), "v60h30m0");
    }

    #[test]
    fn per_layer_lands_parse() {
        let mut form = PadForm::default();
        form.values
            .insert(PadField::Layers, "GND=none; PWR = 1.2".to_string());
        // "none" on one layer needs no default inner land.
        assert!(form.padstack().unwrap_err().contains("No land"));
        form.inner = InnerMode::None;
        let spec = form.padstack().unwrap();
        assert_eq!(spec.inner.layers.len(), 2);
        assert_eq!(spec.inner.layers[0].land, None);
        assert_eq!(spec.inner.layers[1].land.unwrap().width, 1.2);
        form.values.insert(PadField::Layers, "GND".to_string());
        assert!(form.padstack().is_err());
    }

    #[test]
    fn bad_numbers_are_reported() {
        let mut form = PadForm::default();
        form.values.insert(PadField::Width, "abc".to_string());
        assert!(form.padstack().unwrap_err().contains("Width"));
        form.values.insert(PadField::Width, "0.8".to_string());
        assert!(form.padstack().is_err(), "land smaller than the hole");
    }

    #[test]
    fn slots_and_rounded_lands() {
        let mut form = PadForm::default();
        form.shape = Shape::RoundRect;
        form.hole = HoleKind::Oval;
        form.values.insert(PadField::Width, "2.6".to_string());
        form.values.insert(PadField::Height, "1.4".to_string());
        form.values.insert(PadField::Hole, "2.0".to_string());
        form.values.insert(PadField::SlotWidth, "0.8".to_string());
        assert_eq!(form.padstack().unwrap().name(), "r260_140r15h200_80");
    }
}
