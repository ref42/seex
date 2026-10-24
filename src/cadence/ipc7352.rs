//! IPC-7352 land patterns, density level N (nominal), for the standard SMD families.
//!
//! IPC-7352 (2023) replaced IPC-7352. For surface-mount parts it keeps IPC-7352's
//! land equations, zero orientations and naming convention, so the land is
//! calculated from the component's dimensions (minimum/maximum lead span `L`,
//! terminal length `T` and width `W`):
//!
//! ```text
//! Zmax = Lmin + 2 Jt + sqrt(CL² + F² + P²)
//! Gmin = Smax - 2 Jh - sqrt(CS² + F² + P²)     (S toe-to-heel, RMS tolerance)
//! Xmax = Wmin + 2 Js + sqrt(CW² + F² + P²)
//! ```
//!
//! Z and X are rounded up and G down to the round-off (0.005 mm for chips narrower
//! than 0.5 mm, as IPC-7352 asks); the courtyard is the lands and body plus the
//! courtyard excess, on a 0.05 mm grid. The pattern is built in zero orientation
//! (pin 1 upper left, chips and diodes with pin 1 / the cathode on the left), its
//! origin at the package centre, and named with the convention
//! (`RESC1005X40N`, `SOT95P280X145-5N`, ...).
//!
//! EasyEDA's own land pattern is used to check the result: its pads must line up
//! with the calculated ones after a quarter turn, which also carries EasyEDA's pin
//! numbers over. Anything that does not fit is reported and keeps EasyEDA's lands.

use crate::kicad::{FootprintKiCircle as KiCircle, KiFootprint, KiLine, KiPad, PadShape, PadType};

/// Board fabrication tolerance F and placement tolerance P (mm).
pub const FAB_TOLERANCE: f64 = 0.05;
pub const PLACEMENT_TOLERANCE: f64 = 0.025;
/// Silkscreen line width and minimum gap to copper (mm).
const SILK_WIDTH: f64 = 0.12;
const SILK_TO_COPPER: f64 = 0.20;
const ASSEMBLY_WIDTH: f64 = 0.10;
const COURTYARD_WIDTH: f64 = 0.05;
const COURTYARD_GRID: f64 = 0.05;
/// Minimum copper gap between an exposed pad and the terminal lands.
const EP_TO_PAD: f64 = 0.20;
/// EasyEDA pads must land within this distance of the calculated ones.
const MATCH_TOLERANCE: f64 = 0.6;
const PX: f64 = 0.254;

/// Solder-joint goals and courtyard of one IPC-7352 table, density level N.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goals {
    pub toe: f64,
    pub heel: f64,
    pub side: f64,
    pub courtyard: f64,
    pub round_off: f64,
    pub table: &'static str,
}

/// Rectangular and square-end chips, 1608 (0603) and larger.
pub const CHIP: Goals = Goals {
    toe: 0.35,
    heel: 0.0,
    side: 0.0,
    courtyard: 0.25,
    round_off: 0.05,
    table: "chip >= 1608",
};
/// Chips narrower than 0.5 mm (0201, 01005): IPC-7352 rounds their lands to
/// 0.005 mm.
pub const CHIP_TINY: Goals = Goals {
    round_off: 0.005,
    table: "chip, width < 0.5",
    ..CHIP_SMALL
};
/// Chips smaller than 1608 (0603).
pub const CHIP_SMALL: Goals = Goals {
    toe: 0.20,
    heel: 0.0,
    side: 0.0,
    courtyard: 0.15,
    round_off: 0.01,
    table: "chip < 1608",
};
/// Gull-wing leads, pitch above 0.625 mm.
pub const GULL_WING: Goals = Goals {
    toe: 0.35,
    heel: 0.35,
    side: 0.03,
    courtyard: 0.25,
    round_off: 0.05,
    table: "gull-wing, pitch > 0.625",
};
/// Gull-wing leads, pitch 0.625 mm and below.
pub const GULL_WING_FINE: Goals = Goals {
    toe: 0.35,
    heel: 0.35,
    side: -0.02,
    courtyard: 0.25,
    round_off: 0.05,
    table: "gull-wing, pitch <= 0.625",
};
/// Molded bodies with inward L-bend terminals (DO-214 SMA/SMB/SMC).
pub const MOLDED: Goals = Goals {
    toe: 0.15,
    heel: 0.50,
    side: -0.05,
    courtyard: 0.25,
    round_off: 0.05,
    table: "molded body, inward L-bend",
};
/// Flat leads (SODFL / SOTFL: SOD-523, SOD-923). IPC-7352 gives no separate
/// table; the toe is the nominal lead thickness rounded up (0.20 mm, as PCB
/// Libraries' calculator does) and heel and side are the chip goals (Table 3-5).
/// The QFN goals used before (side -0.04 mm) made 0.30 mm wide lands for a
/// 0.30 mm lead, narrower than NXP's and onsemi's SOD-523 footprints.
pub const FLAT_LEAD: Goals = Goals {
    toe: 0.20,
    heel: 0.0,
    side: 0.0,
    courtyard: 0.25,
    round_off: 0.05,
    table: "flat lead",
};
/// Flat no-lead terminations (QFN, SON).
pub const NO_LEAD: Goals = Goals {
    toe: 0.30,
    heel: 0.0,
    side: -0.04,
    courtyard: 0.25,
    round_off: 0.05,
    table: "flat no-lead",
};

/// A dimension with its tolerance (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dim {
    pub min: f64,
    pub max: f64,
}

impl Dim {
    pub const fn new(min: f64, max: f64) -> Self {
        Self { min, max }
    }

    fn around(nominal: f64, tolerance: f64) -> Self {
        Self::new(nominal - tolerance, nominal + tolerance)
    }

    pub fn nominal(&self) -> f64 {
        (self.min + self.max) / 2.0
    }

    fn tolerance(&self) -> f64 {
        self.max - self.min
    }
}

/// Where the terminals are, in IPC-7352 zero orientation.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    /// Two terminals on the X axis, pin 1 on the left.
    TwoTerminal,
    /// Columns on the left and right: Y of each left pin (top first) and right pin
    /// (bottom first), counter-clockwise numbering.
    Dual { left: Vec<f64>, right: Vec<f64> },
    /// `per_side` pins on each of the four sides, pin 1 at the top of the left side.
    Quad { per_side: usize },
}

/// Everything the calculation needs about a package.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// IPC-7352 family prefix (`RESC`, `SOT`, `SOIC`, `QFN`, ...).
    pub prefix: &'static str,
    pub layout: Layout,
    pub pitch: f64,
    /// Toe-to-toe span across the lands (chips and no-lead: body length).
    pub span: Dim,
    pub terminal: Dim,
    pub width: Dim,
    /// Width of the tab lead (SOT-223), which takes the right-hand column.
    pub tab: Option<Dim>,
    /// Nominal body size along X and Y in zero orientation.
    pub body: (f64, f64),
    pub height: f64,
    pub exposed_pad: Option<(f64, f64)>,
    pub goals: Goals,
    /// The two terminals are polarised (cathode = pin 1).
    pub polarized: bool,
    /// Dimensions came from typical JEDEC/EIA values rather than the part.
    pub source: &'static str,
}

impl Package {
    pub fn pins(&self) -> usize {
        match &self.layout {
            Layout::TwoTerminal => 2,
            Layout::Dual { left, right } => left.len() + right.len(),
            Layout::Quad { per_side } => per_side * 4,
        }
    }

    /// IPC-7352 land pattern name, density level N.
    pub fn ipc_name(&self) -> String {
        let h = hundredths(self.height);
        let n = self.pins() + usize::from(self.exposed_pad.is_some());
        match self.prefix {
            "SOD" | "SODFL" | "DIOM" | "RESC" | "CAPC" | "INDC" | "LEDC" | "DIOC" | "FUSC" => {
                let (a, b) = if matches!(self.prefix, "SOD" | "SODFL" | "DIOM") {
                    (self.span.nominal(), self.body.1)
                } else {
                    (self.body.0, self.body.1)
                };
                format!("{}{}{}X{h}N", self.prefix, tenths(a), tenths(b))
            }
            "QFP" => format!(
                "QFP{}P{}X{}X{h}-{n}N",
                hundredths(self.pitch),
                hundredths(self.span.nominal()),
                hundredths(self.span.nominal())
            ),
            "QFN" | "SON" => format!(
                "{}{}P{}X{}X{h}-{n}N",
                self.prefix,
                hundredths(self.pitch),
                hundredths(self.body.0),
                hundredths(self.body.1)
            ),
            _ => format!(
                "{}{}P{}X{h}-{n}N",
                self.prefix,
                hundredths(self.pitch),
                hundredths(self.span.nominal())
            ),
        }
    }
}

/// `1.6` -> `16`, `0.8` -> `08`, `1.25` -> `12` (two digits of 0.1 mm).
fn tenths(mm: f64) -> String {
    format!("{:02}", ((mm * 10.0) + 1e-6).floor() as i64)
}

/// `0.95` -> `95`, `2.80` -> `280`.
fn hundredths(mm: f64) -> String {
    format!("{}", (mm * 100.0).round() as i64)
}

/// Land dimensions of one row of terminals (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lands {
    pub z: f64,
    pub g: f64,
    pub x: f64,
}

impl Lands {
    /// Land length along the span.
    pub fn length(&self) -> f64 {
        (self.z - self.g) / 2.0
    }

    /// Land centre distance from the package centre.
    pub fn centre(&self) -> f64 {
        (self.z + self.g) / 4.0
    }
}

/// IPC-7352 land equations.
pub fn lands(span: Dim, terminal: Dim, width: Dim, goals: &Goals) -> Lands {
    let ft = FAB_TOLERANCE.powi(2) + PLACEMENT_TOLERANCE.powi(2);
    let z = span.min + 2.0 * goals.toe + (span.tolerance().powi(2) + ft).sqrt();
    // Toe-to-heel distance with an RMS tolerance.
    let s_min = span.min - 2.0 * terminal.max;
    let s_max = span.max - 2.0 * terminal.min;
    let s_tol = s_max - s_min;
    let s_tol_rms = (span.tolerance().powi(2) + 2.0 * terminal.tolerance().powi(2)).sqrt();
    let s_max_rms = s_max - (s_tol - s_tol_rms) / 2.0;
    let g = s_max_rms - 2.0 * goals.heel - (s_tol_rms.powi(2) + ft).sqrt();
    let x = width.min + 2.0 * goals.side + (width.tolerance().powi(2) + ft).sqrt();
    let r = goals.round_off;
    Lands {
        z: round_up(z, r),
        g: round_down(g, r).max(0.0),
        x: round_up(x, r),
    }
}

fn round_up(v: f64, step: f64) -> f64 {
    ((v / step) - 1e-9).ceil() * step
}

fn round_down(v: f64, step: f64) -> f64 {
    ((v / step) + 1e-9).floor() * step
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0 + 0.0
}

// ---------------------------------------------------------------------------
// Packages

/// EIA chip sizes: code, body length and width with tolerance, terminal length.
const CHIP_SIZES: [(&str, f64, f64, f64, f64, Dim); 9] = [
    ("0201", 0.60, 0.03, 0.30, 0.03, Dim::new(0.10, 0.20)),
    ("0402", 1.00, 0.05, 0.50, 0.05, Dim::new(0.10, 0.30)),
    ("0603", 1.60, 0.10, 0.80, 0.10, Dim::new(0.10, 0.50)),
    ("0805", 2.00, 0.10, 1.25, 0.10, Dim::new(0.20, 0.75)),
    ("1206", 3.20, 0.15, 1.60, 0.15, Dim::new(0.25, 0.75)),
    ("1210", 3.20, 0.15, 2.50, 0.15, Dim::new(0.25, 0.75)),
    ("1812", 4.50, 0.30, 3.20, 0.20, Dim::new(0.25, 0.85)),
    ("2010", 5.00, 0.10, 2.50, 0.15, Dim::new(0.35, 0.85)),
    ("2512", 6.35, 0.10, 3.20, 0.15, Dim::new(0.35, 0.85)),
];

/// Maximum height by chip kind and size (resistor, capacitor, other).
fn chip_height(prefix: &str, code: &str) -> f64 {
    let (r, c, other) = match code {
        "0201" => (0.26, 0.33, 0.33),
        "0402" => (0.40, 0.55, 0.55),
        "0603" => (0.55, 0.90, 0.80),
        "0805" => (0.70, 1.45, 1.00),
        "1206" => (0.70, 1.80, 1.20),
        "1210" => (0.70, 2.70, 1.20),
        "1812" => (0.70, 2.70, 2.00),
        _ => (0.70, 2.70, 1.50),
    };
    match prefix {
        "RESC" | "FUSC" => r,
        "CAPC" => c,
        _ => other,
    }
}

/// IPC chip prefix for an LCSC reference prefix.
fn chip_prefix(reference: &str, package_prefix: &str) -> &'static str {
    match reference {
        "R" | "RN" => "RESC",
        "C" => "CAPC",
        "L" | "FB" => "INDC",
        "LED" => "LEDC",
        "D" => "DIOC",
        "F" => "FUSC",
        _ => match package_prefix {
            "R" => "RESC",
            "C" => "CAPC",
            "L" | "FB" => "INDC",
            "LED" => "LEDC",
            "D" => "DIOC",
            "F" => "FUSC",
            _ => "RESC",
        },
    }
}

fn chip_code(l: f64, w: f64) -> Option<&'static str> {
    CHIP_SIZES
        .iter()
        .find(|s| (l - s.1).abs() <= 0.12 && (w - s.3).abs() <= 0.12)
        .map(|s| s.0)
}

fn chip(code: &str, prefix: &'static str) -> Option<Package> {
    let &(_, l, lt, w, wt, t) = CHIP_SIZES.iter().find(|s| s.0 == code)?;
    let small = l < 1.5;
    Some(Package {
        prefix,
        layout: Layout::TwoTerminal,
        pitch: 0.0,
        span: Dim::around(l, lt),
        terminal: t,
        width: Dim::around(w, wt),
        body: (l, w),
        height: chip_height(prefix, code),
        tab: None,
        exposed_pad: None,
        goals: if w < 0.5 {
            CHIP_TINY
        } else if small {
            CHIP_SMALL
        } else {
            CHIP
        },
        polarized: matches!(prefix, "LEDC" | "DIOC"),
        source: "EIA chip dimensions",
    })
}

/// Parameters EasyEDA puts in its package names: `_L2.9-W1.6-P0.95-LS2.8`.
#[derive(Debug, Default, Clone, Copy)]
struct NameDims {
    l: Option<f64>,
    w: Option<f64>,
    p: Option<f64>,
    ls: Option<f64>,
    h: Option<f64>,
    ep: Option<(f64, f64)>,
    has_ep: bool,
}

fn name_dims(rest: &str) -> NameDims {
    let mut d = NameDims::default();
    for token in rest.split(['-', '_']) {
        let num = |s: &str| s.parse::<f64>().ok();
        if let Some(v) = token.strip_prefix("LS").and_then(num) {
            d.ls.get_or_insert(v);
        } else if let Some(v) = token.strip_prefix("EP") {
            d.has_ep = true;
            let mut parts = v.split('X').filter_map(num);
            if let Some(a) = parts.next() {
                d.ep = Some((a, parts.next().unwrap_or(a)));
            }
        } else if let Some(v) = token.strip_prefix('L').and_then(num) {
            d.l.get_or_insert(v);
        } else if let Some(v) = token.strip_prefix('W').and_then(num) {
            d.w.get_or_insert(v);
        } else if let Some(v) = token.strip_prefix('P').and_then(num) {
            d.p.get_or_insert(v);
        } else if let Some(v) = token.strip_prefix('H').and_then(num) {
            d.h.get_or_insert(v);
        }
    }
    d
}

/// Gull-wing lead width by pitch (JEDEC).
fn gull_lead_width(pitch: f64) -> Dim {
    if pitch >= 1.2 {
        Dim::new(0.31, 0.51)
    } else if pitch >= 0.75 {
        Dim::new(0.30, 0.45)
    } else if pitch >= 0.6 {
        Dim::new(0.22, 0.38)
    } else if pitch >= 0.45 {
        Dim::new(0.17, 0.27)
    } else {
        Dim::new(0.13, 0.23)
    }
}

/// No-lead terminal width by pitch (JEDEC MO-220 / MO-229).
fn no_lead_width(pitch: f64) -> Dim {
    if pitch >= 0.75 {
        Dim::new(0.28, 0.38)
    } else if pitch >= 0.6 {
        Dim::new(0.25, 0.35)
    } else if pitch >= 0.45 {
        Dim::new(0.18, 0.30)
    } else {
        Dim::new(0.15, 0.25)
    }
}

fn gull_goals(pitch: f64) -> Goals {
    if pitch > 0.625 {
        GULL_WING
    } else {
        GULL_WING_FINE
    }
}

/// Y positions of `n` pins at `pitch`, top first, centred on 0.
fn column(n: usize, pitch: f64) -> Vec<f64> {
    (0..n)
        .map(|i| round2(((n as f64 - 1.0) / 2.0 - i as f64) * pitch))
        .collect()
}

fn dual(n: usize, pitch: f64) -> Layout {
    let left = column(n / 2, pitch);
    let mut right = left.clone();
    right.reverse();
    Layout::Dual { left, right }
}

/// Recognise the package from EasyEDA's package name. `reference` is the part's
/// reference prefix (R, C, LED, ...), which picks the chip kind.
pub fn package_from_name(package: &str, reference: &str) -> Result<Package, String> {
    let mut p = parse_package(package, reference)?;
    // Bidirectional TVS diodes (EasyEDA's "-BI") have no cathode: no cathode
    // mark on the footprint.
    let bidirectional = package
        .to_ascii_uppercase()
        .split(['-', '_'])
        .any(|token| token == "BI");
    if bidirectional && p.polarized {
        p.polarized = false;
    }
    Ok(p)
}

fn parse_package(package: &str, reference: &str) -> Result<Package, String> {
    let upper = package.trim().to_ascii_uppercase();
    if upper.is_empty() {
        return Err("EasyEDA names no package".into());
    }
    let (base, rest) = upper.split_once('_').unwrap_or((upper.as_str(), ""));
    let dims = name_dims(rest);
    let head = base.split('-').next().unwrap_or(base);

    // Chips: R0402, C0603, LED0603-RD, or LED-SMD_L1.6-W0.8.
    let letters: String = head
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let digits = &head[letters.len()..];
    if !letters.is_empty()
        && letters.len() <= 3
        && digits.len() == 4
        && digits.chars().all(|c| c.is_ascii_digit())
    {
        if let Some(p) = chip(digits, chip_prefix(reference, &letters)) {
            return Ok(p);
        }
        return Err(format!("chip size {digits} is not in the EIA table"));
    }
    if base.ends_with("-SMD") {
        if let (Some(l), Some(w)) = (dims.l, dims.w) {
            if let Some(code) = chip_code(l, w) {
                let letters = base.trim_end_matches("-SMD");
                if let Some(p) = chip(code, chip_prefix(reference, letters)) {
                    return Ok(p);
                }
            }
        }
    }

    let mut parts = base.split('-');
    let family = parts.next().unwrap_or_default();
    let size = parts.next().unwrap_or_default();
    let count: Option<usize> = parts.next().and_then(|v| v.parse().ok());
    let pins_in_name: Option<usize> = size.parse().ok();

    match family {
        "SOT" | "SC" => sot(size, count, &dims),
        "SOD" => sod(size),
        "SMA" | "SMB" | "SMC" | "DO" => do214(family, size, reference),
        "SO" | "SOIC" | "SOP" | "TSSOP" | "MSOP" | "SSOP" | "TSOP" | "HTSSOP" | "ESOP" | "HSOP"
        | "VSSOP" | "QSOP" => {
            let n = pins_in_name.ok_or("no pin count")?;
            let pitch = dims.p.ok_or("no pitch in the package name")?;
            let span_nom = dims.ls.ok_or("no lead span in the package name")?;
            let (mut body_l, mut body_w) = (
                dims.l.ok_or("no body length")?,
                dims.w.ok_or("no body width")?,
            );
            if n % 2 != 0 {
                return Err(format!("{n} pins cannot sit in two rows"));
            }
            let exposed_pad = match (
                dims.ep,
                dims.has_ep || family.starts_with('H') || family == "ESOP",
            ) {
                (Some(ep), _) => Some(ep),
                (None, true) => return Err("exposed pad size missing from the package name".into()),
                (None, false) => None,
            };
            let soic = (pitch - 1.27).abs() < 0.01 && family != "SSOP";
            // One footprint per JEDEC package: EasyEDA's body sizes vary a little
            // from part to part (4.9 x 3.9, 5.0 x 4.0 for the same SOIC-8).
            let span_nom = if soic {
                if body_w < 5.0 {
                    body_w = 3.90;
                    body_l = match n {
                        8 => 4.90,
                        14 => 8.65,
                        16 => 9.90,
                        _ => body_l,
                    };
                    6.00
                } else {
                    body_w = 7.50;
                    body_l = match n {
                        16 => 10.30,
                        18 => 11.55,
                        20 => 12.80,
                        24 => 15.40,
                        28 => 17.90,
                        _ => body_l,
                    };
                    10.30
                }
            } else {
                span_nom
            };
            let height = dims.h.unwrap_or(match family {
                "TSSOP" | "TSOP" => 1.20,
                "MSOP" => 1.10,
                "SSOP" => 2.00,
                _ if body_w > 5.0 => 2.65,
                _ => 1.75,
            });
            Ok(Package {
                prefix: if soic { "SOIC" } else { "SOP" },
                layout: dual(n, pitch),
                pitch,
                span: Dim::around(span_nom, 0.20),
                terminal: if soic {
                    Dim::new(0.40, 1.27)
                } else {
                    Dim::new(0.45, 0.75)
                },
                width: if matches!(family, "TSSOP" | "TSOP") {
                    Dim::new(0.19, 0.30)
                } else {
                    gull_lead_width(pitch)
                },
                body: (body_w, body_l),
                height,
                tab: None,
                exposed_pad,
                goals: gull_goals(pitch),
                polarized: false,
                source: "package name with JEDEC lead tolerances",
            })
        }
        "QFP" | "LQFP" | "TQFP" => {
            let n = pins_in_name.ok_or("no pin count")?;
            let pitch = dims.p.ok_or("no pitch in the package name")?;
            let span_nom = dims.ls.ok_or("no lead span in the package name")?;
            let (l, w) = (
                dims.l.ok_or("no body length")?,
                dims.w.ok_or("no body width")?,
            );
            if (l - w).abs() > 0.01 || n % 4 != 0 {
                return Err("rectangular QFPs are not calculated yet".into());
            }
            Ok(Package {
                prefix: "QFP",
                layout: Layout::Quad { per_side: n / 4 },
                pitch,
                span: Dim::around(span_nom, 0.20),
                terminal: Dim::new(0.45, 0.75),
                width: gull_lead_width(pitch),
                body: (w, l),
                height: dims.h.unwrap_or(match family {
                    "TQFP" => 1.20,
                    "LQFP" => 1.60,
                    _ => 2.45,
                }),
                tab: None,
                exposed_pad: None,
                goals: gull_goals(pitch),
                polarized: false,
                source: "package name with JEDEC lead tolerances",
            })
        }
        "QFN" | "VQFN" | "WQFN" | "UQFN" | "TQFN" | "XQFN" | "HVQFN" | "LQFN" | "UFQFPN"
        | "VFQFPN" | "WFQFN" | "DFN" | "SON" | "WSON" | "VSON" | "UDFN" | "WDFN" | "TDFN"
        | "VDFN" | "XDFN" => {
            let n = pins_in_name.ok_or("no pin count")?;
            let pitch = dims.p.ok_or("no pitch in the package name")?;
            let (l, w) = (
                dims.l.ok_or("no body length")?,
                dims.w.ok_or("no body width")?,
            );
            // UFQFPN / VFQFPN are ST's names for quad flat no-lead packages.
            let quad = family.ends_with("QFN") || family.ends_with("QFPN");
            let height = dims.h.unwrap_or(match family.chars().next() {
                Some('W') => 0.80,
                Some('U') => 0.60,
                Some('X') => 0.40,
                _ => 1.00,
            });
            let ep = match (dims.ep, dims.has_ep) {
                (Some(ep), _) => Some(ep),
                (None, true) => return Err("exposed pad size missing from the package name".into()),
                (None, false) => None,
            };
            if quad {
                if (l - w).abs() > 0.01 || n % 4 != 0 {
                    return Err("rectangular QFNs are not calculated yet".into());
                }
                Ok(Package {
                    prefix: "QFN",
                    layout: Layout::Quad { per_side: n / 4 },
                    pitch,
                    span: Dim::around(l, 0.10),
                    terminal: Dim::new(0.30, 0.50),
                    width: no_lead_width(pitch),
                    body: (w, l),
                    height,
                    tab: None,
                    exposed_pad: ep,
                    goals: NO_LEAD,
                    polarized: false,
                    source: "package name with JEDEC MO-220 tolerances",
                })
            } else {
                if n % 2 != 0 {
                    return Err(format!("{n} pins cannot sit in two rows"));
                }
                // Terminals along the longer side; the span is the shorter side.
                let (along, across) = if l >= w { (l, w) } else { (w, l) };
                Ok(Package {
                    prefix: "SON",
                    layout: dual(n, pitch),
                    pitch,
                    span: Dim::around(across, 0.10),
                    terminal: Dim::new(0.30, 0.50),
                    width: no_lead_width(pitch),
                    body: (across, along),
                    height,
                    tab: None,
                    exposed_pad: ep,
                    goals: NO_LEAD,
                    polarized: false,
                    source: "package name with JEDEC MO-229 tolerances",
                })
            }
        }
        _ => Err(format!(
            "{family} packages are not in the IPC-7352 calculator"
        )),
    }
}

/// SOT-23 (3/5/6 pins) and SC-70 / SOT-323 / SOT-353 / SOT-363 (JEDEC TO-236,
/// MO-178, MO-203).
fn sot(size: &str, count: Option<usize>, dims: &NameDims) -> Result<Package, String> {
    if size == "223" && matches!(count, None | Some(3) | Some(4)) {
        // JEDEC TO-261AA: three leads and the tab opposite.
        return Ok(Package {
            prefix: "SOT",
            layout: Layout::Dual {
                left: vec![2.30, 0.0, -2.30],
                right: vec![0.0],
            },
            pitch: 2.30,
            span: Dim::new(6.70, 7.30),
            terminal: Dim::new(0.75, 1.10),
            width: Dim::new(0.66, 0.84),
            tab: Some(Dim::new(2.90, 3.18)),
            body: (3.50, 6.50),
            height: 1.80,
            exposed_pad: None,
            goals: GULL_WING,
            polarized: false,
            source: "JEDEC TO-261AA dimensions",
        });
    }
    let (pins, small) = match (size, count) {
        ("23", None) | ("23", Some(3)) => (3, false),
        ("23", Some(n)) if n == 5 || n == 6 => (n, false),
        ("323", None) | ("70", Some(3)) => (3, true),
        ("353", None) | ("70", Some(5)) => (5, true),
        ("363", None) | ("70", Some(6)) => (6, true),
        _ => return Err(format!("SOT-{size} is not in the IPC-7352 calculator")),
    };
    let pitch = if small { 0.65 } else { 0.95 };
    let p = pitch;
    let layout = match pins {
        3 => Layout::Dual {
            left: vec![p, -p],
            right: vec![0.0],
        },
        5 => Layout::Dual {
            left: vec![p, 0.0, -p],
            right: vec![-p, p],
        },
        _ => Layout::Dual {
            left: vec![p, 0.0, -p],
            right: vec![-p, 0.0, p],
        },
    };
    // Three-lead SOT-23 comes narrow (JEDEC TO-236, 1.3 mm body, 2.4 mm span) and
    // wide (MO-178, the SOT-23-5 body: 1.6 mm, 2.8 mm span); the package name says
    // which.
    let wide = dims.ls.is_some_and(|ls| ls >= 2.7) || dims.w.is_some_and(|w| w >= 1.5);
    let (span, terminal, width, body, height) = match (small, pins) {
        (false, 3) if !wide => (
            Dim::new(2.10, 2.64),
            Dim::new(0.30, 0.55),
            Dim::new(0.30, 0.50),
            (1.30, 2.92),
            1.12,
        ),
        (false, _) => (
            Dim::new(2.60, 3.00),
            Dim::new(0.30, 0.60),
            Dim::new(0.30, 0.50),
            (1.60, 2.90),
            1.45,
        ),
        (true, _) => (
            Dim::new(2.00, 2.20),
            Dim::new(0.26, 0.46),
            Dim::new(0.15, 0.30),
            (1.25, 2.00),
            1.10,
        ),
    };
    Ok(Package {
        prefix: "SOT",
        layout,
        pitch,
        span,
        terminal,
        width,
        body,
        height,
        tab: None,
        exposed_pad: None,
        goals: gull_goals(pitch),
        polarized: false,
        source: "JEDEC TO-236 / MO-178 / MO-203 dimensions",
    })
}

/// DO-214 molded diodes: SMA (DO-214AC), SMB (DO-214AA), SMC (DO-214AB).
fn do214(family: &str, size: &str, reference: &str) -> Result<Package, String> {
    let kind = match (family, size) {
        ("SMA", _) | ("DO", "214AC") => "SMA",
        ("SMB", _) | ("DO", "214AA") => "SMB",
        ("SMC", _) | ("DO", "214AB") => "SMC",
        _ => {
            return Err(format!("{family}-{size} is not in the IPC-7352 calculator"));
        }
    };
    let _ = reference;
    let (span, width, body, height) = match kind {
        "SMA" => (
            Dim::new(4.90, 5.50),
            Dim::new(1.27, 1.63),
            (4.30, 2.60),
            2.44,
        ),
        "SMB" => (
            Dim::new(5.10, 5.70),
            Dim::new(1.96, 2.21),
            (4.30, 3.60),
            2.44,
        ),
        _ => (
            Dim::new(7.70, 8.10),
            Dim::new(2.90, 3.20),
            (6.85, 5.90),
            2.62,
        ),
    };
    Ok(Package {
        prefix: "DIOM",
        layout: Layout::TwoTerminal,
        pitch: 0.0,
        span,
        terminal: Dim::new(0.76, 1.52),
        width,
        tab: None,
        body,
        height,
        exposed_pad: None,
        goals: MOLDED,
        polarized: true,
        source: "JEDEC DO-214 dimensions",
    })
}

/// SOD-123, SOD-323 (gull-wing) and SOD-523, SOD-923 (flat lead).
fn sod(size: &str) -> Result<Package, String> {
    let (span, terminal, width, body, height, flat) = match size {
        "123" => (
            Dim::new(3.55, 3.85),
            Dim::new(0.25, 0.55),
            Dim::new(0.45, 0.65),
            (2.70, 1.60),
            1.35,
            false,
        ),
        "323" => (
            Dim::new(2.30, 2.70),
            Dim::new(0.20, 0.40),
            Dim::new(0.25, 0.40),
            (1.70, 1.25),
            1.10,
            false,
        ),
        // onsemi case 502 / NXP SOD523: lead length 0.15-0.25 mm.
        "523" => (
            Dim::new(1.50, 1.70),
            Dim::new(0.15, 0.25),
            Dim::new(0.25, 0.35),
            (1.20, 0.80),
            0.70,
            true,
        ),
        "923" => (
            Dim::new(0.95, 1.05),
            Dim::new(0.15, 0.25),
            Dim::new(0.15, 0.25),
            (0.80, 0.60),
            0.45,
            true,
        ),
        _ => return Err(format!("SOD-{size} is not in the IPC-7352 calculator")),
    };
    Ok(Package {
        prefix: if flat { "SODFL" } else { "SOD" },
        layout: Layout::TwoTerminal,
        pitch: 0.0,
        span,
        terminal,
        width,
        body,
        height,
        tab: None,
        exposed_pad: None,
        goals: if flat { FLAT_LEAD } else { GULL_WING },
        polarized: true,
        source: "JEDEC/JEITA SOD dimensions",
    })
}

// ---------------------------------------------------------------------------
// Geometry

/// One calculated land (mm, Y up, zero orientation).
#[derive(Debug, Clone, PartialEq)]
pub struct Land {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Land {
    fn left(&self) -> f64 {
        self.x - self.w / 2.0
    }
    fn right(&self) -> f64 {
        self.x + self.w / 2.0
    }
    fn bottom(&self) -> f64 {
        self.y - self.h / 2.0
    }
    fn top(&self) -> f64 {
        self.y + self.h / 2.0
    }
}

/// The lands in standard numbering order (pin 1 first, exposed pad last).
pub fn layout_lands(package: &Package) -> (Vec<Land>, Lands) {
    let mut row = lands(
        package.span,
        package.terminal,
        package.width,
        &package.goals,
    );
    // Neighbouring lands keep a copper gap.
    if package.pitch > 0.0 {
        let max_x = round_down(package.pitch - 0.1, 0.01);
        row.x = row.x.min(max_x);
    }
    // Pull the heel back from an exposed pad.
    if let Some((ex, ey)) = package.exposed_pad {
        let min_g = ex.max(ey) + 2.0 * EP_TO_PAD;
        if row.g < min_g {
            row.g = round_up(min_g, 0.01);
        }
    }
    let (len, c) = (round2(row.length()), round2(row.centre()));
    let tab_x = package
        .tab
        .map(|tab| lands(package.span, package.terminal, tab, &package.goals).x)
        .unwrap_or(row.x);
    let mut out = Vec::new();
    match &package.layout {
        Layout::TwoTerminal => {
            out.push(Land {
                x: -c,
                y: 0.0,
                w: len,
                h: row.x,
            });
            out.push(Land {
                x: c,
                y: 0.0,
                w: len,
                h: row.x,
            });
        }
        Layout::Dual { left, right } => {
            for &y in left {
                out.push(Land {
                    x: -c,
                    y,
                    w: len,
                    h: row.x,
                });
            }
            for &y in right {
                out.push(Land {
                    x: c,
                    y,
                    w: len,
                    h: tab_x,
                });
            }
        }
        Layout::Quad { per_side } => {
            let col = column(*per_side, package.pitch);
            for &y in &col {
                out.push(Land {
                    x: -c,
                    y,
                    w: len,
                    h: row.x,
                });
            }
            for &y in &col {
                out.push(Land {
                    x: -y,
                    y: -c,
                    w: row.x,
                    h: len,
                });
            }
            for &y in &col {
                out.push(Land {
                    x: c,
                    y: -y,
                    w: len,
                    h: row.x,
                });
            }
            for &y in &col {
                out.push(Land {
                    x: y,
                    y: c,
                    w: row.x,
                    h: len,
                });
            }
        }
    }
    if let Some((ex, ey)) = package.exposed_pad {
        out.push(Land {
            x: 0.0,
            y: 0.0,
            w: ex,
            h: ey,
        });
    }
    (out, row)
}

/// A finished IPC-7352 land pattern.
#[derive(Debug, Clone)]
pub struct LandPattern {
    pub name: String,
    /// Footprint in the converter's units (EasyEDA px, Y down), origin at the
    /// package centre.
    pub footprint: KiFootprint,
    /// Turn (degrees, counter-clockwise) and shift (mm) from EasyEDA's footprint to
    /// this one: `ipc = R(turn) * easyeda + shift`.
    pub turn: f64,
    pub shift: (f64, f64),
    pub notes: Vec<String>,
}

/// Calculate the IPC-7352 land pattern for `package_name` and line it up with
/// EasyEDA's footprint `easyeda` to carry its pin numbers over. `first_pin` is the
/// pad that goes where IPC puts pin 1 (the cathode of diodes and LEDs).
pub fn land_pattern(
    package_name: &str,
    reference: &str,
    easyeda: &KiFootprint,
    first_pin: &str,
) -> Result<LandPattern, String> {
    let package = package_from_name(package_name, reference)?;
    let (lands, row) = layout_lands(&package);
    if lands.iter().any(|l| l.w <= 0.0 || l.h <= 0.0) {
        return Err("the calculated lands have no size".into());
    }
    if row.g <= 0.0 {
        return Err("the calculated lands overlap".into());
    }

    let theirs = easyeda_pads(easyeda)?;
    let ours: Vec<(f64, f64)> = lands.iter().map(|l| (l.x, l.y)).collect();
    let alignment = align(&ours, &theirs, first_pin)?;
    let mut notes = Vec::new();
    if !alignment.pin1_ipc {
        notes.push(format!(
            "IPC-7352: EasyEDA's pin {first_pin} is not at an IPC pin-1 position for this package; numbering kept from EasyEDA"
        ));
    }

    notes.extend(check_fit(
        package_name,
        &package,
        &lands,
        easyeda,
        &alignment,
    )?);

    let name = package.ipc_name();
    // Silkscreen: the simplified IPC-7352 drawing of the body (see
    // `build_footprint`), not EasyEDA's, whose outlines are drawn round its own
    // lands and come out up to twice the size of the part.
    let footprint = build_footprint(&name, &package, &lands, &alignment.numbers);
    Ok(LandPattern {
        name,
        footprint,
        turn: alignment.turn,
        shift: alignment.shift,
        notes,
    })
}

/// How far the calculated pattern may stray from the part EasyEDA describes.
/// A pad centre may sit this far (mm) from the manufacturer's...
const FIT_CENTRE: f64 = 0.35;
/// ...or this fraction of the manufacturer's land length, whichever is larger.
const FIT_CENTRE_RATIO: f64 = 0.45;
/// Lands must overlap the manufacturer's by at least this fraction of the smaller.
const FIT_OVERLAP: f64 = 0.35;
/// A land may not be narrower (across its pin) than this fraction of the
/// manufacturer's: narrower means the calculation assumed a thinner lead.
const FIT_WIDTH: f64 = 0.7;

/// Check that the calculated pattern is the part's real package: the package
/// name's own dimensions (body, lead span) must agree with the dimensions the
/// calculation used, and every land must sit on the manufacturer's land for the
/// same pin. A part whose package differs from the JEDEC/EIA one assumed (a
/// non-standard lead span, a wider body, a different pad layout) is refused, and
/// the manufacturer's lands are used instead.
fn check_fit(
    package_name: &str,
    package: &Package,
    lands: &[Land],
    easyeda: &KiFootprint,
    alignment: &Alignment,
) -> Result<Vec<String>, String> {
    let upper = package_name.trim().to_ascii_uppercase();
    let rest = upper.split_once('_').map(|(_, r)| r).unwrap_or("");
    let dims = name_dims(rest);
    if let Some(ls) = dims.ls {
        let (lo, hi) = (package.span.min - 0.15, package.span.max + 0.15);
        if ls < lo || ls > hi {
            return Err(format!(
                "the package name gives a {ls:.2} mm lead span, outside the {:.2}-{:.2} mm of the JEDEC/EIA outline the calculation uses",
                package.span.min, package.span.max
            ));
        }
    }
    if let (Some(l), Some(w)) = (dims.l, dims.w) {
        let named = (l.min(w), l.max(w));
        let used = (
            package.body.0.min(package.body.1),
            package.body.0.max(package.body.1),
        );
        if (named.0 - used.0).abs() > 0.3 || (named.1 - used.1).abs() > 0.3 {
            return Err(format!(
                "the package name gives a {:.2} x {:.2} mm body, the JEDEC/EIA outline {:.2} x {:.2} mm",
                named.1, named.0, used.1, used.0
            ));
        }
    }

    // Manufacturer's lands in the calculated pattern's frame.
    let q = (alignment.turn / 90.0).round() as usize % 4;
    let mut theirs: Vec<(String, [f64; 4])> = Vec::new();
    for pad in &easyeda.pads {
        let number = pad.number.trim();
        if number.is_empty() || matches!(pad.pad_type, PadType::NpThroughHole) {
            continue;
        }
        let (w, h) = (pad.size_x * PX, pad.size_y * PX);
        let (sin, cos) = pad.rotation.to_radians().sin_cos();
        let (hw, hh) = (
            (w * cos.abs() + h * sin.abs()) / 2.0,
            (w * sin.abs() + h * cos.abs()) / 2.0,
        );
        let (x, y) = turn_point(q, pad.pos_x * PX, -pad.pos_y * PX);
        let (x, y) = (x + alignment.shift.0, y + alignment.shift.1);
        let (hw, hh) = if q % 2 == 1 { (hh, hw) } else { (hw, hh) };
        let rect = [x - hw, y - hh, x + hw, y + hh];
        match theirs.iter_mut().find(|t| t.0 == number) {
            Some(t) => {
                t.1 = [
                    t.1[0].min(rect[0]),
                    t.1[1].min(rect[1]),
                    t.1[2].max(rect[2]),
                    t.1[3].max(rect[3]),
                ];
            }
            None => theirs.push((number.to_string(), rect)),
        }
    }
    let mut worst: Option<(f64, String)> = None;
    for (land, number) in lands.iter().zip(&alignment.numbers) {
        let Some((_, r)) = theirs.iter().find(|t| &t.0 == number) else {
            continue;
        };
        let ours = [land.left(), land.bottom(), land.right(), land.top()];
        let centre = |b: &[f64; 4]| ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let (a, b) = (centre(&ours), centre(r));
        let distance = (a.0 - b.0).hypot(a.1 - b.1);
        let area = |b: &[f64; 4]| ((b[2] - b[0]) * (b[3] - b[1])).max(1e-9);
        let inter = ((ours[2].min(r[2]) - ours[0].max(r[0])).max(0.0))
            * ((ours[3].min(r[3]) - ours[1].max(r[1])).max(0.0));
        let overlap = inter / area(&ours).min(area(r));
        let their_length = (r[2] - r[0]).max(r[3] - r[1]);
        let allowed = FIT_CENTRE.max(FIT_CENTRE_RATIO * their_length);
        // Across the pin: perpendicular to the direction from the package centre
        // (none for a centre pad).
        let centre_pad = land.x.abs() < 1e-6 && land.y.abs() < 1e-6;
        let left_or_right = match package.layout {
            Layout::Quad { .. } => land.x.abs() >= land.y.abs(),
            _ => true,
        };
        let across = if centre_pad {
            None
        } else if left_or_right {
            Some((land.h, r[3] - r[1]))
        } else {
            Some((land.w, r[2] - r[0]))
        };
        // Where IPC-7352 asks for a side fillet (side goal >= 0), and for every
        // two-terminal part but the molded-body ones, the land is at least as wide
        // as the widest lead.
        let full_width = package.goals.side >= 0.0
            || (matches!(package.layout, Layout::TwoTerminal) && package.goals != MOLDED);
        if let (Some((ours, _)), true) = (across, full_width) {
            if ours + 1e-9 < package.width.max {
                return Err(format!(
                    "pad {number} would be {ours:.2} mm wide, narrower than the {:.2} mm lead",
                    package.width.max
                ));
            }
        }
        let problem = if distance > allowed {
            Some(format!(
                "pad {number} sits {distance:.2} mm from the manufacturer's land (allowed {allowed:.2} mm)"
            ))
        } else if overlap < FIT_OVERLAP {
            Some(format!(
                "pad {number} overlaps the manufacturer's land by only {:.0}%",
                overlap * 100.0
            ))
        } else if let Some((ours, theirs)) = across.filter(|(o, t)| *o < FIT_WIDTH * t) {
            Some(format!(
                "pad {number} is {ours:.2} mm wide, the manufacturer's {theirs:.2} mm"
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(format!(
                "{problem}: the part's package does not match the JEDEC/EIA outline the calculation assumes"
            ));
        }
        let score = distance / allowed;
        if worst.as_ref().is_none_or(|w| score > w.0) {
            let width = across
                .map(|(o, t)| format!(", width {o:.2} mm vs {t:.2} mm"))
                .unwrap_or_default();
            worst = Some((
                score,
                format!("{distance:.2} mm, {:.0}% overlap{width}", overlap * 100.0),
            ));
        }
    }
    Ok(worst
        .map(|(_, text)| {
            vec![format!(
                "IPC-7352 lands checked against the manufacturer's: largest pad offset {text}"
            )]
        })
        .unwrap_or_default())
}

/// EasyEDA's numbered SMD pads (mm, Y up); pads sharing a number become one.
fn easyeda_pads(footprint: &KiFootprint) -> Result<Vec<(String, f64, f64)>, String> {
    let mut groups: Vec<(String, f64, f64, f64, f64)> = Vec::new();
    for pad in &footprint.pads {
        match pad.pad_type {
            PadType::NpThroughHole => continue,
            PadType::ThroughHole => {
                if pad.number.trim().is_empty() {
                    continue;
                }
                return Err("the part has through-hole pins (IPC-7251, not calculated)".into());
            }
            _ => {}
        }
        let number = pad.number.trim();
        if number.is_empty() {
            continue;
        }
        let (x, y) = (pad.pos_x * PX, -pad.pos_y * PX);
        match groups.iter_mut().find(|g| g.0 == number) {
            Some(g) => {
                g.1 = g.1.min(x);
                g.2 = g.2.min(y);
                g.3 = g.3.max(x);
                g.4 = g.4.max(y);
            }
            None => groups.push((number.to_string(), x, y, x, y)),
        }
    }
    Ok(groups
        .into_iter()
        .map(|(n, x0, y0, x1, y1)| (n, (x0 + x1) / 2.0, (y0 + y1) / 2.0))
        .collect())
}

struct Alignment {
    numbers: Vec<String>,
    turn: f64,
    shift: (f64, f64),
    pin1_ipc: bool,
}

fn bbox_centre(points: impl Iterator<Item = (f64, f64)>) -> (f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
}

fn turn_point(q: usize, x: f64, y: f64) -> (f64, f64) {
    match q % 4 {
        0 => (x, y),
        1 => (-y, x),
        2 => (-x, -y),
        _ => (y, -x),
    }
}

/// IPC-7352 polarity marking on the silkscreen: a bar beside the cathode of a
/// polarized two-terminal part, a 0.3 mm dot beside pin 1 of anything else
/// with more pins.
fn polarity_mark(package: &Package, lands: &[Land]) -> (Vec<KiLine>, Vec<KiCircle>) {
    let to_px = |mm: f64| mm / PX;
    let (mut lines, mut circles) = (Vec::new(), Vec::new());
    let Some(first) = lands.first() else {
        return (lines, circles);
    };
    let gap = SILK_TO_COPPER + SILK_WIDTH / 2.0;
    if package.layout == Layout::TwoTerminal {
        if package.polarized {
            let x = first.left() - gap;
            lines.push(KiLine {
                start_x: to_px(x),
                start_y: to_px(-first.bottom()),
                end_x: to_px(x),
                end_y: to_px(-first.top()),
                width: to_px(SILK_WIDTH),
                layer: "F.SilkS".into(),
            });
        }
    } else {
        let r = 0.15;
        let (cx, cy) = (first.left() - SILK_TO_COPPER - r, first.y);
        circles.push(KiCircle {
            center_x: to_px(cx),
            center_y: to_px(-cy),
            end_x: to_px(cx + r),
            end_y: to_px(-cy),
            // The converter stores circle widths in mm.
            width: SILK_WIDTH,
            layer: "F.SilkS".into(),
            fill: true,
        });
    }
    (lines, circles)
}

/// Find the quarter turn that puts EasyEDA's pads on the calculated ones.
fn align(
    ours: &[(f64, f64)],
    theirs: &[(String, f64, f64)],
    first: &str,
) -> Result<Alignment, String> {
    if ours.len() != theirs.len() {
        return Err(format!(
            "EasyEDA's footprint has {} numbered pads, the IPC-7352 pattern {}",
            theirs.len(),
            ours.len()
        ));
    }
    let c_ours = bbox_centre(ours.iter().copied());
    let c_theirs = bbox_centre(theirs.iter().map(|p| (p.1, p.2)));
    let mut fits: Vec<Alignment> = Vec::new();
    for q in 0..4 {
        let (tcx, tcy) = turn_point(q, c_theirs.0, c_theirs.1);
        let shift = (c_ours.0 - tcx, c_ours.1 - tcy);
        let mut numbers = vec![String::new(); ours.len()];
        let mut ok = true;
        for (number, x, y) in theirs {
            let (rx, ry) = turn_point(q, *x, *y);
            let (px, py) = (rx + shift.0, ry + shift.1);
            let nearest = ours
                .iter()
                .enumerate()
                .map(|(i, o)| (i, ((o.0 - px).powi(2) + (o.1 - py).powi(2)).sqrt()))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match nearest {
                Some((i, d)) if d <= MATCH_TOLERANCE && numbers[i].is_empty() => {
                    numbers[i] = number.clone();
                }
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            fits.push(Alignment {
                pin1_ipc: numbers.first().is_some_and(|n| n == first),
                numbers,
                turn: q as f64 * 90.0,
                shift: (round4(shift.0), round4(shift.1)),
            });
        }
    }
    if fits.is_empty() {
        return Err(
            "EasyEDA's pads do not line up with the IPC-7352 pattern (different package?)".into(),
        );
    }
    let best = fits.iter().position(|f| f.pin1_ipc).unwrap_or(0);
    Ok(fits.swap_remove(best))
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0 + 0.0
}

/// Lands, silkscreen, assembly outline and courtyard as a converter footprint.
fn build_footprint(
    name: &str,
    package: &Package,
    lands: &[Land],
    numbers: &[String],
) -> KiFootprint {
    let to_px = |mm: f64| mm / PX;
    let mut fp = KiFootprint {
        name: name.to_string(),
        pads: Vec::new(),
        tracks: Vec::new(),
        circles: Vec::new(),
        arcs: Vec::new(),
        texts: Vec::new(),
        lines: Vec::new(),
        model_3d: None,
    };
    for (land, number) in lands.iter().zip(numbers) {
        fp.pads.push(KiPad {
            number: number.clone(),
            pad_type: PadType::Smd,
            shape: PadShape::Rect,
            pos_x: to_px(land.x),
            pos_y: to_px(-land.y),
            size_x: to_px(land.w),
            size_y: to_px(land.h),
            rotation: 0.0,
            layers: vec!["F.Cu".into(), "F.Paste".into(), "F.Mask".into()],
            drill: None,
            polygon: None,
        });
    }
    let mut lines: Vec<KiLine> = Vec::new();
    let mut line = |layer: &str, width: f64, a: (f64, f64), b: (f64, f64)| {
        lines.push(KiLine {
            start_x: to_px(a.0),
            start_y: to_px(-a.1),
            end_x: to_px(b.0),
            end_y: to_px(-b.1),
            width: to_px(width),
            layer: layer.to_string(),
        });
    };

    let (bx, by) = (package.body.0 / 2.0, package.body.1 / 2.0);
    let two_terminal = package.layout == Layout::TwoTerminal;

    // Assembly: the body, pin 1 corner chamfered; a cathode band on diodes.
    if two_terminal {
        rect_lines(&mut line, "F.Fab", ASSEMBLY_WIDTH, -bx, -by, bx, by);
        if package.polarized {
            let band = -bx + package.body.0 * 0.2;
            line("F.Fab", ASSEMBLY_WIDTH, (band, -by), (band, by));
        }
    } else {
        let chamfer = (package.body.0.min(package.body.1) * 0.25).min(1.0);
        let corners = [
            (-bx + chamfer, by),
            (bx, by),
            (bx, -by),
            (-bx, -by),
            (-bx, by - chamfer),
            (-bx + chamfer, by),
        ];
        for pair in corners.windows(2) {
            line("F.Fab", ASSEMBLY_WIDTH, pair[0], pair[1]);
        }
    }

    // Silkscreen: the body outline, kept clear of the copper.
    let keep_out: Vec<(f64, f64, f64, f64)> = lands
        .iter()
        .map(|l| {
            let m = SILK_TO_COPPER + SILK_WIDTH / 2.0;
            (l.left() - m, l.bottom() - m, l.right() + m, l.top() + m)
        })
        .collect();
    let sx = bx + SILK_WIDTH / 2.0;
    let sy = by + SILK_WIDTH / 2.0;
    let outline = [
        ((-sx, sy), (sx, sy)),
        ((sx, sy), (sx, -sy)),
        ((sx, -sy), (-sx, -sy)),
        ((-sx, -sy), (-sx, sy)),
    ];
    let mut outline_drawn = false;
    for (a, b) in outline {
        for (p, q) in clip_segment(a, b, &keep_out) {
            line("F.SilkS", SILK_WIDTH, p, q);
            outline_drawn = true;
        }
    }
    // Small chips (0402, 0603, ...) leave no room for the body outline between
    // the lands. The part still gets a mark on the board: the body's two long
    // sides, as long as the body, moved out just far enough to clear the copper.
    let (lx0, mut ly0, lx1, mut ly1) = (-bx, -sy, bx, sy);
    if !outline_drawn {
        for l in lands {
            ly0 = ly0.min(l.bottom() - SILK_TO_COPPER - SILK_WIDTH / 2.0);
            ly1 = ly1.max(l.top() + SILK_TO_COPPER + SILK_WIDTH / 2.0);
        }
        line("F.SilkS", SILK_WIDTH, (lx0, ly1), (lx1, ly1));
        line("F.SilkS", SILK_WIDTH, (lx0, ly0), (lx1, ly0));
    }
    // Pin 1 mark: a bar beside a diode's cathode, a dot beside pin 1 otherwise.
    let (mark_lines, mark_circles) = polarity_mark(package, lands);
    for l in &mark_lines {
        let (a, b) = (
            (l.start_x * PX, -l.start_y * PX),
            (l.end_x * PX, -l.end_y * PX),
        );
        line("F.SilkS", SILK_WIDTH, a, b);
    }
    fp.circles.extend(mark_circles);

    // Courtyard: lands and body plus the courtyard excess, on the courtyard grid,
    // and never inside the silkscreen drawn outside the lands.
    let (mut x0, mut y0, mut x1, mut y1) = (-bx, -by, bx, by);
    for l in lands {
        x0 = x0.min(l.left());
        y0 = y0.min(l.bottom());
        x1 = x1.max(l.right());
        y1 = y1.max(l.top());
    }
    let e = package.goals.courtyard;
    let (x0, y0) = (
        round_down(x0 - e, COURTYARD_GRID),
        round_down(y0 - e, COURTYARD_GRID),
    );
    let (x1, y1) = (
        round_up(x1 + e, COURTYARD_GRID),
        round_up(y1 + e, COURTYARD_GRID),
    );
    let silk = SILK_WIDTH / 2.0;
    let (x0, y0) = (
        x0.min(round_down(lx0 - silk, COURTYARD_GRID)),
        y0.min(round_down(ly0 - silk, COURTYARD_GRID)),
    );
    let (x1, y1) = (
        x1.max(round_up(lx1 + silk, COURTYARD_GRID)),
        y1.max(round_up(ly1 + silk, COURTYARD_GRID)),
    );
    rect_lines(&mut line, "F.CrtYd", COURTYARD_WIDTH, x0, y0, x1, y1);
    fp.lines = lines;
    fp
}

fn rect_lines(
    line: &mut impl FnMut(&str, f64, (f64, f64), (f64, f64)),
    layer: &str,
    width: f64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
) {
    line(layer, width, (x0, y1), (x1, y1));
    line(layer, width, (x1, y1), (x1, y0));
    line(layer, width, (x1, y0), (x0, y0));
    line(layer, width, (x0, y0), (x0, y1));
}

/// Parts of the axis-aligned segment `a`-`b` outside every keep-out box, dropping
/// pieces shorter than 0.2 mm.
fn clip_segment(
    a: (f64, f64),
    b: (f64, f64),
    keep_out: &[(f64, f64, f64, f64)],
) -> Vec<((f64, f64), (f64, f64))> {
    let horizontal = (a.1 - b.1).abs() < 1e-9;
    let (fixed, mut lo, mut hi) = if horizontal {
        (a.1, a.0.min(b.0), a.0.max(b.0))
    } else {
        (a.0, a.1.min(b.1), a.1.max(b.1))
    };
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    let mut pieces = vec![(lo, hi)];
    for &(x0, y0, x1, y1) in keep_out {
        let (f0, f1, c0, c1) = if horizontal {
            (y0, y1, x0, x1)
        } else {
            (x0, x1, y0, y1)
        };
        if fixed <= f0 || fixed >= f1 {
            continue;
        }
        pieces = pieces
            .into_iter()
            .flat_map(|(s, e)| {
                let mut out = Vec::new();
                if c0 > s {
                    out.push((s, e.min(c0)));
                }
                if c1 < e {
                    out.push((s.max(c1), e));
                }
                out
            })
            .filter(|(s, e)| e > s)
            .collect();
    }
    pieces
        .into_iter()
        .filter(|(s, e)| e - s >= 0.2)
        .map(|(s, e)| {
            if horizontal {
                ((s, fixed), (e, fixed))
            } else {
                ((fixed, s), (fixed, e))
            }
        })
        .collect()
}

/// IPC-7352 rules for a land pattern seex cannot calculate (connectors,
/// through-hole and other non-standard parts): the manufacturer's lands from
/// EasyEDA stay, and the rest is brought in line: a courtyard of lands and body
/// plus 0.25 mm on the 0.05 mm grid, 0.12 mm silkscreen kept 0.2 mm off the
/// copper, an assembly outline, and plated holes with at least a 0.25 mm annular
/// ring. Returns the footprint and what was changed.
pub fn conform(easyeda: &KiFootprint) -> (KiFootprint, Vec<String>) {
    let mut fp = easyeda.clone();
    let mut changes = Vec::new();
    let mm = |px: f64| px * PX;
    let to_px = |v: f64| v / PX;

    // Plated holes: at least a 0.25 mm annular ring, if the neighbours allow it.
    const RING: f64 = 0.25;
    let boxes: Vec<(f64, f64, f64, f64)> = fp.pads.iter().map(pad_box).collect();
    let mut widened = 0;
    for i in 0..fp.pads.len() {
        let pad = &fp.pads[i];
        if pad.pad_type != PadType::ThroughHole {
            continue;
        }
        let Some(drill) = &pad.drill else { continue };
        // The ring is checked along each axis of the pad, so a slot in an oval
        // pad keeps the oval: only the side that is too thin grows.
        let hole_x = mm(drill.diameter);
        let hole_y = mm(drill.width.unwrap_or(drill.diameter));
        let need_x = round_up(hole_x + 2.0 * RING, 0.05);
        let need_y = round_up(hole_y + 2.0 * RING, 0.05);
        let (w, h) = (mm(pad.size_x), mm(pad.size_y));
        let grow_x = (need_x - w).max(0.0);
        let grow_y = (need_y - h).max(0.0);
        if grow_x < 1e-6 && grow_y < 1e-6 {
            continue;
        }
        // Pad rotated by 90 or 270 degrees: its x side lies along the board's y.
        let quarter = ((pad.rotation.rem_euclid(180.0)) - 90.0).abs() < 45.0;
        let (gx, gy) = if quarter {
            (grow_y, grow_x)
        } else {
            (grow_x, grow_y)
        };
        let (x0, y0, x1, y1) = boxes[i];
        let grown = (x0 - gx / 2.0, y0 - gy / 2.0, x1 + gx / 2.0, y1 + gy / 2.0);
        let clear = boxes
            .iter()
            .enumerate()
            .all(|(j, b)| j == i || gap_boxes(grown, *b) >= 0.2);
        if !clear {
            continue;
        }
        let pad = &mut fp.pads[i];
        pad.size_x = to_px(w + grow_x);
        pad.size_y = to_px(h + grow_y);
        widened += 1;
    }
    if widened > 0 {
        changes.push(format!(
            "{widened} plated pad(s) widened to a {RING:.2} mm annular ring"
        ));
    }

    // Silkscreen: 0.12 mm, clear of the copper.
    let keep_out: Vec<(f64, f64, f64, f64)> = fp
        .pads
        .iter()
        .map(|p| {
            let (x0, y0, x1, y1) = pad_box(p);
            let m = SILK_TO_COPPER + SILK_WIDTH / 2.0;
            (x0 - m, y0 - m, x1 + m, y1 + m)
        })
        .collect();
    let mut lines = Vec::new();
    let mut clipped = 0;
    let mut outline: Vec<(f64, f64)> = Vec::new();
    for line in fp.lines.drain(..) {
        let a = (mm(line.start_x), -mm(line.start_y));
        let b = (mm(line.end_x), -mm(line.end_y));
        match line.layer.as_str() {
            "F.CrtYd" | "B.CrtYd" => {}
            "F.SilkS" | "B.SilkS" => {
                outline.extend([a, b]);
                let pieces = clip_any(a, b, &keep_out);
                if pieces.len() != 1 || pieces[0] != (a, b) {
                    clipped += 1;
                }
                for (p, q) in pieces {
                    lines.push(KiLine {
                        start_x: to_px(p.0),
                        start_y: to_px(-p.1),
                        end_x: to_px(q.0),
                        end_y: to_px(-q.1),
                        width: to_px(SILK_WIDTH),
                        layer: line.layer.clone(),
                    });
                }
            }
            _ => {
                if line.layer.ends_with(".Fab") {
                    outline.extend([a, b]);
                }
                lines.push(line);
            }
        }
    }
    // Silkscreen arcs become clipped segments too.
    for arc in std::mem::take(&mut fp.arcs) {
        if !arc.layer.ends_with("SilkS") {
            fp.arcs.push(arc);
            continue;
        }
        let points = arc_points(&arc);
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            outline.extend([a, b]);
            for (p, q) in clip_any(a, b, &keep_out) {
                lines.push(KiLine {
                    start_x: to_px(p.0),
                    start_y: to_px(-p.1),
                    end_x: to_px(q.0),
                    end_y: to_px(-q.1),
                    width: to_px(SILK_WIDTH),
                    layer: arc.layer.clone(),
                });
            }
        }
        clipped += 1;
    }
    // Silkscreen circles: kept where they clear the copper. A circle drawn round
    // a pad (a test point's ring) that the pad reaches into grows just enough to
    // clear it, so the mark stays the manufacturer's.
    let copper: Vec<(f64, f64, f64, f64)> = fp.pads.iter().map(pad_box).collect();
    let mut circles = Vec::new();
    for mut c in std::mem::take(&mut fp.circles) {
        if !c.layer.ends_with("SilkS") {
            circles.push(c);
            continue;
        }
        let (cx, cy) = (mm(c.center_x), -mm(c.center_y));
        let r = ((mm(c.end_x) - mm(c.center_x)).powi(2) + (mm(c.end_y) - mm(c.center_y)).powi(2))
            .sqrt();
        // Circle widths are stored in mm by the converter.
        let half = if c.fill { 0.0 } else { SILK_WIDTH / 2.0 };
        let clear = |r: f64| {
            copper.iter().all(|b| {
                let near = (cx - cx.clamp(b.0, b.2)).hypot(cy - cy.clamp(b.1, b.3));
                let far = (cx - b.0)
                    .abs()
                    .max((cx - b.2).abs())
                    .hypot((cy - b.1).abs().max((cy - b.3).abs()));
                near >= r + half + SILK_TO_COPPER - 1e-9 || r - half >= far + SILK_TO_COPPER - 1e-9
            })
        };
        if clear(r) {
            if !c.fill {
                c.width = SILK_WIDTH;
            }
            circles.push(c);
            continue;
        }
        clipped += 1;
        // Rings round the pads under their centre.
        let around: Vec<&(f64, f64, f64, f64)> = boxes
            .iter()
            .filter(|b| cx > b.0 && cx < b.2 && cy > b.1 && cy < b.3)
            .collect();
        if c.fill || around.is_empty() {
            continue;
        }
        let needed = around
            .iter()
            .map(|b| {
                (cx - b.0)
                    .abs()
                    .max((cx - b.2).abs())
                    .hypot((cy - b.1).abs().max((cy - b.3).abs()))
            })
            .fold(0.0, f64::max)
            + SILK_TO_COPPER
            + half;
        if needed > r * 2.0 || !clear(needed) {
            continue;
        }
        c.end_x = to_px(cx + needed);
        c.end_y = to_px(-cy);
        c.width = SILK_WIDTH;
        circles.push(c);
    }
    fp.circles = circles;
    if clipped > 0 {
        changes.push(format!(
            "silkscreen set to {SILK_WIDTH:.2} mm and kept {SILK_TO_COPPER:.2} mm off the copper"
        ));
    }

    // Body outline: EasyEDA's assembly or silkscreen drawing; else the lands.
    let mut bx = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in &outline {
        bx = (bx.0.min(x), bx.1.min(y), bx.2.max(x), bx.3.max(y));
    }
    let has_assembly = lines.iter().any(|l| l.layer == "F.Fab");
    if !has_assembly && bx.0 < bx.2 {
        let corners = [
            (bx.0, bx.3),
            (bx.2, bx.3),
            (bx.2, bx.1),
            (bx.0, bx.1),
            (bx.0, bx.3),
        ];
        for pair in corners.windows(2) {
            lines.push(KiLine {
                start_x: to_px(pair[0].0),
                start_y: to_px(-pair[0].1),
                end_x: to_px(pair[1].0),
                end_y: to_px(-pair[1].1),
                width: to_px(ASSEMBLY_WIDTH),
                layer: "F.Fab".into(),
            });
        }
        changes.push("assembly outline added".into());
    }

    // A footprint without any silkscreen (test points, pads-only parts) gets an
    // outline around its lands and body, clear of the copper, so the part is
    // marked on the board.
    let has_silk = lines.iter().any(|l| l.layer.ends_with("SilkS"))
        || fp.circles.iter().any(|c| c.layer.ends_with("SilkS"));
    let mut cy = bx;
    if !has_silk && !keep_out.is_empty() {
        let mut sb = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for b in keep_out.iter().copied() {
            sb = (sb.0.min(b.0), sb.1.min(b.1), sb.2.max(b.2), sb.3.max(b.3));
        }
        if bx.0 < bx.2 {
            let m = SILK_WIDTH / 2.0;
            sb = (
                sb.0.min(bx.0 - m),
                sb.1.min(bx.1 - m),
                sb.2.max(bx.2 + m),
                sb.3.max(bx.3 + m),
            );
        }
        // Just outside the keep-out boxes, which count their own edges as inside.
        let sb = (sb.0 - 1e-6, sb.1 - 1e-6, sb.2 + 1e-6, sb.3 + 1e-6);
        let corners = [
            (sb.0, sb.3),
            (sb.2, sb.3),
            (sb.2, sb.1),
            (sb.0, sb.1),
            (sb.0, sb.3),
        ];
        for pair in corners.windows(2) {
            for (p, q) in clip_any(pair[0], pair[1], &keep_out) {
                lines.push(KiLine {
                    start_x: to_px(p.0),
                    start_y: to_px(-p.1),
                    end_x: to_px(q.0),
                    end_y: to_px(-q.1),
                    width: to_px(SILK_WIDTH),
                    layer: "F.SilkS".into(),
                });
            }
        }
        let m = SILK_WIDTH / 2.0;
        cy = (sb.0 - m, sb.1 - m, sb.2 + m, sb.3 + m);
        if bx.0 < bx.2 {
            cy = (
                cy.0.min(bx.0),
                cy.1.min(bx.1),
                cy.2.max(bx.2),
                cy.3.max(bx.3),
            );
        }
        changes.push("silkscreen outline added around the lands".into());
    }

    // Courtyard: lands and body plus the courtyard excess, on the 0.05 mm grid,
    // and round the silkscreen circles.
    for b in &boxes {
        cy = (cy.0.min(b.0), cy.1.min(b.1), cy.2.max(b.2), cy.3.max(b.3));
    }
    for c in fp.circles.iter().filter(|c| c.layer.ends_with("SilkS")) {
        let (x, y) = (mm(c.center_x), -mm(c.center_y));
        let r = (mm(c.end_x) - mm(c.center_x)).hypot(mm(c.end_y) - mm(c.center_y)) + c.width / 2.0;
        cy = (
            cy.0.min(x - r),
            cy.1.min(y - r),
            cy.2.max(x + r),
            cy.3.max(y + r),
        );
    }
    if cy.0 < cy.2 {
        let e = GULL_WING.courtyard;
        let (x0, y0) = (
            round_down(cy.0 - e, COURTYARD_GRID),
            round_down(cy.1 - e, COURTYARD_GRID),
        );
        let (x1, y1) = (
            round_up(cy.2 + e, COURTYARD_GRID),
            round_up(cy.3 + e, COURTYARD_GRID),
        );
        let corners = [(x0, y1), (x1, y1), (x1, y0), (x0, y0), (x0, y1)];
        for pair in corners.windows(2) {
            lines.push(KiLine {
                start_x: to_px(pair[0].0),
                start_y: to_px(-pair[0].1),
                end_x: to_px(pair[1].0),
                end_y: to_px(-pair[1].1),
                width: to_px(COURTYARD_WIDTH),
                layer: "F.CrtYd".into(),
            });
        }
        changes.push("courtyard: lands and body + 0.25 mm on the 0.05 mm grid".into());
    }
    fp.lines = lines;
    (fp, changes)
}

/// Copper extent of a pad (mm, Y up), any rotation.
fn pad_box(pad: &KiPad) -> (f64, f64, f64, f64) {
    let (x, y) = (pad.pos_x * PX, -pad.pos_y * PX);
    let (w, h) = (pad.size_x * PX / 2.0, pad.size_y * PX / 2.0);
    let (s, c) = pad.rotation.to_radians().sin_cos();
    let (ex, ey) = ((w * c).abs() + (h * s).abs(), (w * s).abs() + (h * c).abs());
    let mut b = (x - ex, y - ey, x + ex, y + ey);
    if let Some(points) = pad.polygon.as_deref() {
        for (px, py) in parse_points(points) {
            b = (
                b.0.min(x + px),
                b.1.min(y + py),
                b.2.max(x + px),
                b.3.max(y + py),
            );
        }
    }
    b
}

/// Pad-relative polygon points (mm, Y up) from KiCad's `(xy x y)` list.
fn parse_points(text: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for chunk in text.split("(xy").skip(1) {
        let nums: Vec<f64> = chunk
            .split(|c: char| c.is_whitespace() || c == ')')
            .filter_map(|t| t.parse().ok())
            .collect();
        if let [x, y, ..] = nums.as_slice() {
            out.push((*x, -*y));
        }
    }
    out
}

fn gap_boxes(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let dx = (a.0 - b.2).max(b.0 - a.2).max(0.0);
    let dy = (a.1 - b.3).max(b.1 - a.3).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// Points along a footprint arc (mm, Y up).
fn arc_points(arc: &crate::kicad::FootprintKiArc) -> Vec<(f64, f64)> {
    let p = |x: f64, y: f64| (x * PX, -y * PX);
    let (a, m, b) = (
        p(arc.start_x, arc.start_y),
        p(arc.mid_x, arc.mid_y),
        p(arc.end_x, arc.end_y),
    );
    let d = 2.0 * (a.0 * (m.1 - b.1) + m.0 * (b.1 - a.1) + b.0 * (a.1 - m.1));
    if d.abs() < 1e-12 {
        return vec![a, m, b];
    }
    let sq = |q: (f64, f64)| q.0 * q.0 + q.1 * q.1;
    let ux = (sq(a) * (m.1 - b.1) + sq(m) * (b.1 - a.1) + sq(b) * (a.1 - m.1)) / d;
    let uy = (sq(a) * (b.0 - m.0) + sq(m) * (a.0 - b.0) + sq(b) * (m.0 - a.0)) / d;
    let r = ((a.0 - ux).powi(2) + (a.1 - uy).powi(2)).sqrt();
    let ang = |q: (f64, f64)| (q.1 - uy).atan2(q.0 - ux);
    let (a0, am, a1) = (ang(a), ang(m), ang(b));
    let tau = std::f64::consts::TAU;
    let ccw = (am - a0).rem_euclid(tau) < (a1 - a0).rem_euclid(tau);
    let sweep = if ccw {
        (a1 - a0).rem_euclid(tau)
    } else {
        -(a0 - a1).rem_euclid(tau)
    };
    (0..=16)
        .map(|i| {
            let t = a0 + sweep * i as f64 / 16.0;
            (ux + r * t.cos(), uy + r * t.sin())
        })
        .collect()
}

/// Parts of the segment `a`-`b` outside every box (any direction), dropping
/// pieces shorter than 0.1 mm.
fn clip_any(
    a: (f64, f64),
    b: (f64, f64),
    keep_out: &[(f64, f64, f64, f64)],
) -> Vec<((f64, f64), (f64, f64))> {
    let mut pieces = vec![(0.0f64, 1.0f64)];
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    for &(x0, y0, x1, y1) in keep_out {
        // Liang-Barsky: the parameter interval of the segment inside the box.
        let mut t0: f64 = 0.0;
        let mut t1: f64 = 1.0;
        let mut inside = true;
        for (p, q) in [
            (-dx, a.0 - x0),
            (dx, x1 - a.0),
            (-dy, a.1 - y0),
            (dy, y1 - a.1),
        ] {
            if p.abs() < 1e-12 {
                if q < 0.0 {
                    inside = false;
                    break;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
        if !inside || t0 >= t1 {
            continue;
        }
        pieces = pieces
            .into_iter()
            .flat_map(|(s, e)| {
                let mut out = Vec::new();
                if t0 > s {
                    out.push((s, e.min(t0)));
                }
                if t1 < e {
                    out.push((s.max(t1), e));
                }
                out
            })
            .filter(|(s, e)| e > s)
            .collect();
    }
    let len = (dx * dx + dy * dy).sqrt();
    pieces
        .into_iter()
        .filter(|(s, e)| (e - s) * len >= 0.1)
        .map(|(s, e)| ((a.0 + dx * s, a.1 + dy * s), (a.0 + dx * e, a.1 + dy * e)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(number: &str, x_mm: f64, y_mm: f64) -> KiPad {
        KiPad {
            number: number.into(),
            pad_type: PadType::Smd,
            shape: PadShape::Rect,
            pos_x: x_mm / PX,
            pos_y: -y_mm / PX,
            size_x: 0.5 / PX,
            size_y: 0.5 / PX,
            rotation: 0.0,
            layers: vec!["F.Cu".into()],
            drill: None,
            polygon: None,
        }
    }

    fn footprint(pads: Vec<KiPad>) -> KiFootprint {
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

    #[test]
    fn names_follow_the_ipc_convention() {
        let name = |pkg: &str, r: &str| package_from_name(pkg, r).unwrap().ipc_name();
        assert_eq!(name("R0402", "R"), "RESC1005X40N");
        assert_eq!(name("C0603", "C"), "CAPC1608X90N");
        assert_eq!(name("LED0603-RD", "LED"), "LEDC1608X80N");
        assert_eq!(name("LED-SMD_L1.6-W0.8-R-RD", "LED"), "LEDC1608X80N");
        assert_eq!(
            name("SOT-23-5_L2.9-W1.6-P0.95-LS2.8-BR", "U"),
            "SOT95P280X145-5N"
        );
        // Narrow (TO-236) and wide (MO-178) three-lead SOT-23.
        assert_eq!(
            name("SOT-23-3_L2.9-W1.3-P1.90-LS2.4-BR", "Q"),
            "SOT95P237X112-3N"
        );
        assert_eq!(
            name("SOT-23-3_L2.9-W1.6-P1.90-LS2.8-BR", "U"),
            "SOT95P280X145-3N"
        );
        assert_eq!(
            name("SOT-23-3_L2.9-W1.3-P1.90-LS2.4-BR", "Q"),
            "SOT95P237X112-3N"
        );
        assert_eq!(
            name("SOT-323_L2.0-W1.3-P1.30-LS2.1-BR", "Q"),
            "SOT65P210X110-3N"
        );
        assert_eq!(name("SOD-323_L1.8-W1.3-LS2.5-RD", "D"), "SOD2512X110N");
        assert_eq!(name("SOD-523_L1.2-W0.8-LS1.6-RD", "D"), "SODFL1608X70N");
        // SOD-523 flat leads: Z 2.15 as in NXP's footprint, lands 0.55 x 0.40.
        let p = package_from_name("SOD-523_L1.2-W0.8-LS1.6-RD", "D").unwrap();
        let l = lands(p.span, p.terminal, p.width, &p.goals);
        assert_eq!((round2(l.z), round2(l.g), round2(l.x)), (2.15, 1.05, 0.40));
        assert_eq!(
            name("SOIC-8_L4.9-W3.9-P1.27-LS6.0-BL", "U"),
            "SOIC127P600X175-8N"
        );
        assert_eq!(
            name("TSSOP-20_L6.5-W4.4-P0.65-LS6.4-BL", "U"),
            "SOP65P640X120-20N"
        );
        assert_eq!(
            name("SOT-223-3_L6.5-W3.4-P2.30-LS7.0-BR", "U"),
            "SOT230P700X180-4N"
        );
        // EasyEDA's SOIC-8 variants share one pattern.
        assert_eq!(
            name("SOP-8_L5.0-W4.0-P1.27-LS6.0-BL", "U"),
            "SOIC127P600X175-8N"
        );
        assert_eq!(
            name("SO-8_L4.9-W3.9-P1.27-LS6.0-BL", "U"),
            "SOIC127P600X175-8N"
        );
        assert_eq!(
            package_from_name("SOP-8_L5.0-W4.0-P1.27-LS6.0-BL", "U").unwrap(),
            package_from_name("SOIC-8_L4.9-W3.9-P1.27-LS6.1-BL", "U").unwrap()
        );
        assert_eq!(name("SMA_L4.3-W2.6-LS5.2-RD", "D"), "DIOM5226X244N");
        assert_eq!(name("SMC_L6.9-W5.9-LS7.9-BI", "D"), "DIOM7959X262N");
        assert_eq!(
            name("ESOP-8_L4.9-W3.9-P1.27-LS6.0-BL-EP3.3X2.4", "U"),
            "SOIC127P600X175-9N"
        );
        assert_eq!(
            name("LQFP-48_L7.0-W7.0-P0.50-LS9.0-BL", "U"),
            "QFP50P900X900X160-48N"
        );
        assert_eq!(
            name("QFN-56_L7.0-W7.0-P0.40-BL-EP4.0", "U"),
            "QFN40P700X700X100-57N"
        );
        // ST's UFQFPN is a QFN.
        assert_eq!(
            name("UFQFPN-28_L4.0-W4.0-P0.50-BL", "U"),
            "QFN50P400X400X60-28N"
        );
        assert!(package_from_name("USB-C-SMD_TYPE-C-31-M-12", "USBC").is_err());
    }

    #[test]
    fn chip_lands_follow_the_equations() {
        let p = package_from_name("R0603", "R").unwrap();
        let row = lands(p.span, p.terminal, p.width, &p.goals);
        // Zmax = 1.50 + 0.70 + sqrt(0.2^2 + 0.05^2 + 0.025^2) = 2.408 -> 2.45
        assert!((row.z - 2.45).abs() < 1e-9, "{row:?}");
        // Xmax = 0.70 + sqrt(0.2^2 + 0.05^2 + 0.025^2) = 0.908 -> 0.95
        assert!((row.x - 0.95).abs() < 1e-9, "{row:?}");
        assert!(row.g > 0.0 && row.g < 1.0, "{row:?}");
        let small = package_from_name("R0402", "R").unwrap();
        assert_eq!(small.goals, CHIP_SMALL);
    }

    #[test]
    fn sot23_5_is_turned_to_pin_one_upper_left() {
        // EasyEDA draws SOT-23-5 with pins 1-3 along the bottom.
        let easy = footprint(vec![
            pad("1", -0.95, -1.15),
            pad("2", 0.0, -1.15),
            pad("3", 0.95, -1.15),
            pad("4", 0.95, 1.15),
            pad("5", -0.95, 1.15),
        ]);
        let lp = land_pattern("SOT-23-5_L2.9-W1.6-P0.95-LS2.8-BR", "U", &easy, "1").unwrap();
        assert_eq!(lp.name, "SOT95P280X145-5N");
        let first = &lp.footprint.pads[0];
        assert_eq!(first.number, "1");
        assert!(
            first.pos_x < 0.0 && first.pos_y < 0.0,
            "pin 1 upper left (Y down)"
        );
        let numbers: Vec<&str> = lp
            .footprint
            .pads
            .iter()
            .map(|p| p.number.as_str())
            .collect();
        assert_eq!(numbers, ["1", "2", "3", "4", "5"]);
        assert!(
            lp.notes
                .iter()
                .all(|n| n.starts_with("IPC-7352 lands checked")),
            "{:?}",
            lp.notes
        );
        // EasyEDA's pin 1 (bottom left) turns to the upper left: a quarter turn clockwise.
        assert_eq!(lp.turn, 270.0);
        assert!(lp.footprint.lines.iter().any(|l| l.layer == "F.CrtYd"));
    }

    #[test]
    fn led_cathode_goes_left() {
        // Pin 2 is the cathode and EasyEDA has it on the left already.
        let easy = footprint(vec![pad("1", 0.8, 0.0), pad("2", -0.8, 0.0)]);
        let lp = land_pattern("LED0603-RD", "LED", &easy, "2").unwrap();
        assert_eq!(lp.footprint.pads[0].number, "2");
        assert!(lp.footprint.pads[0].pos_x < 0.0);
        assert_eq!(lp.turn, 0.0);
    }

    #[test]
    fn small_chips_are_marked_on_the_silkscreen() {
        // 0402 and 0603 leave no room between the lands: a line above and one
        // below the lands, clear of the copper and inside the courtyard.
        for (package, x, half_body) in [("R0402", 0.48, 0.5), ("C0603", 0.78, 0.8)] {
            let easy = footprint(vec![pad("1", -x, 0.0), pad("2", x, 0.0)]);
            let reference = &package[..1];
            let lp = land_pattern(package, reference, &easy, "1").unwrap();
            let silk: Vec<&KiLine> = lp
                .footprint
                .lines
                .iter()
                .filter(|l| l.layer == "F.SilkS")
                .collect();
            assert_eq!(silk.len(), 2, "{package}");
            // As long as the body, not the land pattern.
            for l in &silk {
                assert!((l.start_x * PX).abs() <= half_body + 1e-9, "{package}");
                assert!((l.end_x * PX).abs() <= half_body + 1e-9, "{package}");
            }
            let lands: Vec<(f64, f64, f64, f64)> = lp.footprint.pads.iter().map(pad_box).collect();
            let top = lands.iter().map(|b| b.3).fold(f64::MIN, f64::max);
            for l in &silk {
                let y = (-l.start_y * PX).abs();
                assert!(
                    y - SILK_WIDTH / 2.0 >= top + SILK_TO_COPPER - 1e-9,
                    "{package}: {y}"
                );
            }
            let courtyard = lp
                .footprint
                .lines
                .iter()
                .filter(|l| l.layer == "F.CrtYd")
                .map(|l| (-l.start_y * PX).abs())
                .fold(f64::MIN, f64::max);
            let silk_top = (-silk[0].start_y * PX).abs() + SILK_WIDTH / 2.0;
            assert!(courtyard >= silk_top - 1e-9, "{package}");
        }
    }

    #[test]
    fn test_point_ring_grows_round_its_pad() {
        // C2826547: a 2.2 mm square pad with a silkscreen ring inside its corners.
        let mut lug = pad("1", 0.0, 0.0);
        lug.size_x = 2.2 / PX;
        lug.size_y = 2.2 / PX;
        let mut easy = footprint(vec![lug]);
        easy.circles.push(KiCircle {
            center_x: 0.0,
            center_y: 0.0,
            end_x: 1.3 / PX,
            end_y: 0.0,
            width: 0.15,
            layer: "F.SilkS".into(),
            fill: false,
        });
        let (fp, _) = conform(&easy);
        let rings: Vec<&KiCircle> = fp.circles.iter().filter(|c| c.layer == "F.SilkS").collect();
        assert_eq!(rings.len(), 1);
        let r = rings[0].end_x * PX;
        assert!(
            (r - (1.1f64.hypot(1.1) + SILK_TO_COPPER + SILK_WIDTH / 2.0)).abs() < 1e-6,
            "{r}"
        );
        // The ring is the mark: no outline is added round the pad.
        assert!(!fp.lines.iter().any(|l| l.layer == "F.SilkS"));
    }

    #[test]
    fn pads_only_footprints_get_a_silkscreen_outline() {
        let mut lug = pad("1", 0.0, 0.0);
        lug.size_x = 3.0 / PX;
        lug.size_y = 1.8 / PX;
        let (fp, changes) = conform(&footprint(vec![lug]));
        assert!(
            changes.iter().any(|c| c.contains("silkscreen outline")),
            "{changes:?}"
        );
        let silk: Vec<&KiLine> = fp.lines.iter().filter(|l| l.layer == "F.SilkS").collect();
        assert_eq!(silk.len(), 4);
        // 1.5 mm half width + 0.20 mm clearance + half the 0.12 mm line.
        let x = silk
            .iter()
            .map(|l| (l.start_x * PX).abs())
            .fold(f64::MIN, f64::max);
        assert!((x - 1.76).abs() < 1e-3, "{x}");
    }

    #[test]
    fn qfn_exposed_pad_keeps_its_number_and_clearance() {
        let p = package_from_name("QFN-16_L3.0-W3.0-P0.50-BL-EP1.7", "U").unwrap();
        let (lands, row) = layout_lands(&p);
        assert_eq!(lands.len(), 17);
        assert!(row.g / 2.0 >= 1.7 / 2.0 + EP_TO_PAD - 1e-9);
        let mut pads: Vec<KiPad> = lands[..16]
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut p = pad(&(i + 1).to_string(), l.x, l.y);
                p.size_x = l.w / PX;
                p.size_y = l.h / PX;
                p
            })
            .collect();
        // EasyEDA's exposed pad split into four paste windows with one number.
        for (dx, dy) in [(-0.4, -0.4), (0.4, -0.4), (-0.4, 0.4), (0.4, 0.4)] {
            pads.push(pad("17", dx, dy));
        }
        let lp = land_pattern(
            "QFN-16_L3.0-W3.0-P0.50-BL-EP1.7",
            "U",
            &footprint(pads),
            "1",
        )
        .unwrap();
        assert_eq!(lp.footprint.pads.len(), 17);
        assert_eq!(lp.footprint.pads[16].number, "17");
        assert_eq!(lp.name, "QFN50P300X300X100-17N");
    }

    #[test]
    fn two_terminal_lands_cover_the_lead() {
        // Every two-terminal family but the molded bodies gets lands at least as
        // wide as the widest lead (the SOD-523 lands once came out narrower).
        for (name, r) in [
            ("R0201", "R"),
            ("R0402", "R"),
            ("R0603", "R"),
            ("R0805", "R"),
            ("R1206", "R"),
            ("C0402", "C"),
            ("C0603", "C"),
            ("LED0603-RD", "LED"),
            ("SOD-123", "D"),
            ("SOD-323", "D"),
            ("SOD-523", "D"),
            ("SOD-923", "D"),
        ] {
            let p = package_from_name(name, r).unwrap();
            let row = lands(p.span, p.terminal, p.width, &p.goals);
            assert!(
                row.x + 1e-9 >= p.width.max,
                "{name}: {row:?} for lead {:?}",
                p.width
            );
        }
    }

    #[test]
    fn parts_off_the_jedec_outline_are_refused() {
        // A SOD-523 named with a 2.2 mm lead span is not the JEDEC part.
        let easy = footprint(vec![pad("1", -0.8, 0.0), pad("2", 0.8, 0.0)]);
        let err = land_pattern("SOD-523_L1.2-W0.8-LS2.2-RD", "D", &easy, "1").unwrap_err();
        assert!(err.contains("lead span"), "{err}");
        // Manufacturer lands far outside the calculated ones: a different package.
        let wide = footprint(vec![pad("1", -1.3, 0.0), pad("2", 1.3, 0.0)]);
        let err = land_pattern("SOD-523_L1.2-W0.8-LS1.6-RD", "D", &wide, "1").unwrap_err();
        assert!(err.contains("manufacturer's land"), "{err}");
        // Manufacturer's lands much wider than the calculated ones: the part's leads
        // are wider than the outline assumes.
        let mut fat = footprint(vec![pad("1", -0.8, 0.0), pad("2", 0.8, 0.0)]);
        for p in &mut fat.pads {
            p.size_y = 0.9 / PX;
        }
        let err = land_pattern("SOD-523_L1.2-W0.8-LS1.6-RD", "D", &fat, "1").unwrap_err();
        assert!(err.contains("wide"), "{err}");
        // The goals used before for SOD-523 (QFN, side -0.04 mm) gave lands narrower
        // than the lead: refused.
        let mut p = package_from_name("SOD-523_L1.2-W0.8-LS1.6-RD", "D").unwrap();
        p.goals = NO_LEAD;
        let row = lands(p.span, p.terminal, p.width, &p.goals);
        assert!(row.x < p.width.max, "{row:?}");
        // The real part: lands on the manufacturer's.
        let lp = land_pattern("SOD-523_L1.2-W0.8-LS1.6-RD", "D", &easy, "1").unwrap();
        assert_eq!(lp.name, "SODFL1608X70N");
    }

    #[test]
    fn mismatched_footprints_are_refused() {
        let easy = footprint(vec![
            pad("1", -1.0, 0.0),
            pad("2", 1.0, 0.0),
            pad("3", 0.0, 1.0),
        ]);
        assert!(land_pattern("R0603", "R", &easy, "1").is_err());
    }

    #[test]
    fn conform_adds_courtyard_and_clears_silkscreen() {
        let mut through = pad("1", -2.54, 0.0);
        through.pad_type = PadType::ThroughHole;
        through.size_x = 1.0 / PX;
        through.size_y = 1.0 / PX;
        through.drill = Some(crate::kicad::Drill {
            diameter: 0.8 / PX,
            width: None,
            offset_x: 0.0,
            offset_y: 0.0,
        });
        let mut easy = footprint(vec![through, pad("2", 2.54, 0.0)]);
        easy.lines.push(KiLine {
            start_x: -4.0 / PX,
            start_y: 0.0,
            end_x: 4.0 / PX,
            end_y: 0.0,
            width: 0.25 / PX,
            layer: "F.SilkS".into(),
        });
        let (fp, changes) = conform(&easy);
        assert!(changes.iter().any(|c| c.contains("annular")), "{changes:?}");
        assert!((fp.pads[0].size_x * PX - 1.30).abs() < 1e-9);
        let silk: Vec<&KiLine> = fp.lines.iter().filter(|l| l.layer == "F.SilkS").collect();
        assert_eq!(silk.len(), 3, "split around both pads");
        assert!(
            silk.iter()
                .all(|l| (l.width * PX - SILK_WIDTH).abs() < 1e-9)
        );
        assert_eq!(fp.lines.iter().filter(|l| l.layer == "F.CrtYd").count(), 4);
        assert_eq!(fp.lines.iter().filter(|l| l.layer == "F.Fab").count(), 4);
        // Diagonal silkscreen is clipped too.
        assert_eq!(
            clip_any((-1.0, -1.0), (1.0, 1.0), &[(-0.5, -0.5, 0.5, 0.5)]).len(),
            2
        );
    }

    #[test]
    fn slotted_oval_pads_stay_oval() {
        // USB-C shell lug: 1.0 x 2.1 mm oval pad, 0.6 x 1.7 mm slot.
        let lug = |rotation: f64| {
            let mut p = pad("1", 0.0, 0.0);
            p.pad_type = PadType::ThroughHole;
            p.shape = PadShape::Oval;
            p.size_x = 1.0 / PX;
            p.size_y = 2.1 / PX;
            p.rotation = rotation;
            p.drill = Some(crate::kicad::Drill {
                diameter: 0.6 / PX,
                width: Some(1.7 / PX),
                offset_x: 0.0,
                offset_y: 0.0,
            });
            p
        };
        for rotation in [0.0, 90.0] {
            let (fp, changes) = conform(&footprint(vec![lug(rotation)]));
            assert!(changes.iter().any(|c| c.contains("annular")), "{changes:?}");
            assert!(
                (fp.pads[0].size_x * PX - 1.10).abs() < 1e-9,
                "{}",
                fp.pads[0].size_x * PX
            );
            assert!(
                (fp.pads[0].size_y * PX - 2.20).abs() < 1e-9,
                "{}",
                fp.pads[0].size_y * PX
            );
            assert_eq!(fp.pads[0].shape, PadShape::Oval);
        }
        // A slot that already has its ring is left alone.
        let mut ok = lug(0.0);
        ok.size_x = 1.2 / PX;
        ok.size_y = 2.3 / PX;
        let (fp, _) = conform(&footprint(vec![ok]));
        assert!(
            (fp.pads[0].size_x * PX - 1.2).abs() < 1e-9
                && (fp.pads[0].size_y * PX - 2.3).abs() < 1e-9
        );
    }

    #[test]
    fn silkscreen_stays_off_the_copper() {
        let keep = [(-1.0, -0.5, -0.4, 0.5), (0.4, -0.5, 1.0, 0.5)];
        let pieces = clip_segment((-1.5, 0.3), (1.5, 0.3), &keep);
        assert_eq!(pieces.len(), 3);
        assert!(pieces.iter().all(|((a, _), (b, _))| b - a >= 0.2));
        assert!(clip_segment((-0.9, 0.0), (-0.5, 0.0), &keep).is_empty());
    }
}
