# SeEx (Seek and Export)

<div align="center">
  <img src="assets/icons/seex.svg" alt="SeEx Logo" width="200"/>
</div>

<div align="center">

[![release](https://img.shields.io/github/v/release/ref42/seex?display_name=tag)](https://github.com/ref42/seex/releases) [![license](https://img.shields.io/badge/license-Non--Commercial-blue)](LICENSE) [![CI](https://img.shields.io/github/actions/workflow/status/ref42/seex/ci.yml)](https://github.com/ref42/seex/actions/workflows/ci.yml) [![downloads](https://img.shields.io/github/downloads/ref42/seex/total)](https://github.com/ref42/seex/releases)

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust&logoColor=white) ![GPUI](https://img.shields.io/badge/UI-gpui--kit-8b5cf6) ![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20Linux%20%7C%20macOS-0078D6?logo=windows&logoColor=white) ![tokio](https://img.shields.io/badge/Async-tokio-14b8a6) ![reqwest](https://img.shields.io/badge/HTTP-reqwest-2ea44f)

</div>

<p align="center">
  <a href="./README.en.md">English</a> | <a href="./README.md">简体中文</a>
</p>

**SeEx** is a desktop tool that batch-exports LCSC components into formats that Altium Designer (AD), KiCad and Cadence (OrCAD Capture / Allegro) can use.

## ✨ Features

### KiCad

- Export `Symbol`, `Footprint` and `3D` model formats that KiCad can use.
- Export in batches, or one at a time.

### Altium Designer (AD)

- Export a component's `schlib` and `pcblib` on its own (`pcblib` already embeds the `step` model).
- Export in batches as well.
- Export several components into one `schlib`/`pcblib`, i.e. a merged export.

### Cadence (OrCAD Capture / Allegro)

- Export `Capture` schematic symbols: one `Library XML` file, which `Capture` converts to an `.olb` with `File > Import > Library XML`.
- Export `Allegro` footprints: one `SKILL` script per footprint plus one library script; run `skill load("<library>_allegro/<library>.il")` in the `Allegro PCB Editor` command window to generate the `.dra`, `.psm` and `.pad` files in the same folder.
- Export `3D` models (`step`), with the model already mapped into the footprint at the position EasyEDA uses.
- Standard packages (chip resistors and capacitors, `SOT`, `SOD`, `SOIC`, `QFN`, ...) get lands calculated per `IPC-7352` and checked against the manufacturer's; other packages keep the manufacturer's lands, with silkscreen, assembly layer and place bound generated per `IPC-7352`.
- `seex` itself calls no Cadence program; the import is done by you inside `Cadence`.

### Shared

- All targets support exporting metadata in Chinese or English.
- All targets support multi-threaded downloads.
- All targets support adding new components to an existing library previously exported by `seex`.
- All targets support a custom schematic stroke and fill colour.
- All targets support exporting `3D` models on their own.
- The LCEDA logo (watermark) is removed from `3D` models by default and the model rebuilt; when it cannot be removed safely the original model is kept, and those components are listed in the export result.

## 🔄 Workflow

<div align="center">
  <img src="imgs/workflow.png" alt="npnp/seex workflow: LCSC part numbers → seex / npnp → KiCad, Altium, Cadence" width="900"/>
</div>

## 📸 Screenshots

<table>
  <summary>SeEx GUI</summary>

  <table>
    <tr>
      <td><img src="imgs/seex_layout/en/1.png" width="600" /></td>
      <td><img src="imgs/seex_layout/en/2.png" width="600" /></td>
      <td><img src="imgs/seex_layout/en/3.png" width="600" /></td>
    </tr>
    <tr>
      <td><img src="imgs/seex_layout/en/4.png" width="600" /></td>
      <td><img src="imgs/seex_layout/en/5.png" width="600" /></td>
    </tr>
  </table>
</table>

### Export examples

<table>
  <summary>KiCad symbol and footprint examples</summary>

  <table>
    <tr>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_1.png" alt="KiCad symbol and footprint example 1" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_2.png" alt="KiCad symbol and footprint example 2" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_3.png" alt="KiCad symbol and footprint example 3" width="420" /></td>
    </tr>
    <tr>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_4.png" alt="KiCad symbol and footprint example 4" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_5.png" alt="KiCad symbol and footprint example 5" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_6.png" alt="KiCad symbol and footprint example 6" width="420" /></td>
    </tr>
  </table>
</table>

<table>
  <summary>KiCad 3D model examples</summary>

  <table>
    <tr>
      <td><img src="imgs/showcase/kicad/3d_models/SOT-89-3.png" alt="SOT-89-3" width="420" /></td>
      <td><img src="imgs/showcase/kicad/3d_models/TSSOP-20.png" alt="TSSOP-20" width="420" /></td>
      <td><img src="imgs/showcase/kicad/3d_models/QFN-80.png" alt="QFN-80" width="420" /></td>
    </tr>
    <tr>
      <td><img src="imgs/showcase/kicad/3d_models/SOIC-8.png" alt="SOIC-8" width="420" /></td>
      <td><img src="imgs/showcase/kicad/3d_models/SOIC-14.png" alt="SOIC-14" width="420" /></td>
      <td><img src="imgs/showcase/kicad/3d_models/LQFP-48.png" alt="LQFP-48" width="420" /></td>
    </tr>
  </table>
</table>

<table>
  <summary>Altium SCH + 3D model examples</summary>

  <table>
    <tr>
      <td><img src="imgs/showcase/ad/sch_3d/2N2222AU.png" alt="2N2222AU" width="420" /></td>
      <td><img src="imgs/showcase/ad/sch_3d/CH32V003F4P6.png" alt="CH32V003F4P6" width="420" /></td>
      <td><img src="imgs/showcase/ad/sch_3d/ESP32-S31NRV16.png" alt="ESP32-S31NRV16" width="420" /></td>
    </tr>
    <tr>
      <td><img src="imgs/showcase/ad/sch_3d/NE555DR.png" alt="NE555DR" width="420" /></td>
      <td><img src="imgs/showcase/ad/sch_3d/SN74HC04DR.png" alt="SN74HC04DR" width="420" /></td>
      <td><img src="imgs/showcase/ad/sch_3d/STM32F103C8T6.png" alt="STM32F103C8T6" width="420" /></td>
    </tr>
  </table>
</table>

<table>
  <summary>Altium PCB examples</summary>

  <table>
    <tr>
      <td><img src="imgs/showcase/ad/pcd/sot89-3.png" alt="sot89-3" width="420" /></td>
      <td><img src="imgs/showcase/ad/pcd/tssop-20.png" alt="tssop-20" width="420" /></td>
      <td><img src="imgs/showcase/ad/pcd/qfn-80.png" alt="qfn-80" width="420" /></td>
    </tr>
    <tr>
      <td><img src="imgs/showcase/ad/pcd/soic-8.png" alt="soic-8" width="420" /></td>
      <td><img src="imgs/showcase/ad/pcd/soic-14.png" alt="soic-14" width="420" /></td>
      <td><img src="imgs/showcase/ad/pcd/lqfp-48.png" alt="lqfp-48" width="420" /></td>
    </tr>
  </table>
</table>

## 🖥️ Supported EDA Versions

> [!NOTE]
> These are only the versions known so far; SeEx may well support more. If a library exported by `seex` cannot be opened in an older `EDA` release, please open an `issue` and compatibility will be considered.

- `KiCad 9.0` ~ `Latest`
- `Altium Designer 23.x` ~ `Latest`
- `Cadence SPB 26.1` (OrCAD X Capture / Allegro X); the `Allegro` footprint scripts need a release that supports `SKILL` (`Allegro PCB Designer` or `OrCAD PCB Designer Professional`)

## 🚀 Install and Use

### Download

Download the archive for your platform from [Releases](https://github.com/ref42/seex/releases), unpack it and run `seex.exe` — no installation needed.

### Build from source

```bash
git clone https://github.com/ref42/seex.git
cd seex
cargo build --release
```

The executable is written to `target/release/seex.exe`.

### Basic flow

1. On the `Monitor` page, copy a part number from the LCSC store (e.g. `C2040`), or import an ID file.
2. Open the `Export` page and choose the `Altium`, `KiCad` or `Cadence` target and the export scope.
3. Click `Export library`, then pick up the generated library files from the output folder.

## 📖 Project History

The project history, in chronological order:

- `nlbn` (Nice Library Bring now, a pun on "拿来吧你")
  - no longer maintained
- `npnp` (Normalize Pin Net Pad, a pun on "牛批牛批")
  - still maintained (a sister project of `seex`)
- `seex` (Seek and Export)
  - still maintained (a sister project of `npnp`)

A little grumbling:

- When this project had not been released yet there was almost nothing like it on the market; for some reason there are many similar projects lately, which is a good thing — at least everyone has something to do. But a tool like this really is not worth reskinning and putting up for sale on some platform; that is genuinely tasteless. The point of a basic tool is that every developer can use it, not to get clever about making money. Really tasteless. 🤣

## 🧩 Sister Project

- [npnp](https://github.com/ref42/npnp) is the `cli` version of `seex`, and is better suited to being called directly by `AI`/agents.
  - You can even have `AI` analyse a project for you, and then call `npnp` to export the components it needs. That way you no longer have to search and pick parts by hand — you only have to verify the result.
  - If you already have the part numbers to export, even better: feed them straight to `npnp`.
  - You can also use `npnp` as a `CI` tool.

## 🗺️ Roadmap

- Support more `EDA` tools, such as `pads`.
- Keep improving the current workflow and fix more hidden `bugs`.
- Keep improving the interaction and the user experience, refine the icons, and try new design languages.

## 🤝 Contributing

- Has software design experience and can help maintain the software over time.
- Has good taste; your suggestions would be very welcome.
- "Designers" who can help redesign the software's icon, the interface layout, and so on.
- If you have tokens to spare and are interested in this project, feel free to give it a go: the project does not reject `AI`, it only asks that every author on a submitted `PR` is a real living person.
- Contributing code is not required — every suggestion that makes `seex` and `npnp` better will be considered, and will be thanked at the end of this file.

## 🙏 Acknowledgements

The entries are in no particular order; they follow the development timeline.

### References

- [`easyeda2kicad`](https://github.com/uPesy/easyeda2kicad.py)@[uPesy](https://github.com/uPesy)
  - Although it only supports `KiCad`, it gave me some of the inspiration. It is an excellent tool, and the vast majority of EasyEDA import plugins for `KiCad` on the market are built on top of it.
- `IPC-7352(2023)` https://dokumen.pub/qdownload/ipc-7352-generic-guideline-for-land-pattern-design.html
  - Used for rebuilding the `3D` models.

### Thanks

- 菊猫
  - Gave me part of my `TOKEN`. I also paid for some of it myself, but in the end it worked out as roughly half bought and half donated. `opus 4.6` was still very good at the time, and I leaned on it heavily to implement a lot of features and lay a good foundation.
- emoji老弟@[z217-cpu](https://github.com/z217-cpu)
  - Helped me test multi-platform compatibility from the very start of the project until now; thank you very much!
- 🐏🐏@[Cloud-Lightning](https://github.com/Cloud-Lightning)
  - Has used `seex` in several projects and gave me a great deal of practical advice; thank you very much!
- c大佬@[lins-code54](https://github.com/lins-code54)
  - Provided the `Cadence` export support, and the `3D` model rebuild feature so that models no longer carry the cloud-pattern or text `Logo`; thank you very much!
- Everyone in WeChat groups `1` and `2`
  - They did not mind me posting `seex` and `npnp` updates in those groups; on the contrary, they were very supportive. Thank you very much!
- All users
  - Without your downloads and `star`s this project might have been shelved; thank you very much!

## 📄 License

This project uses the custom **SeEx Non-Commercial Share-Alike License 1.0**: studying, modifying and redistributing are allowed, **commercial use of any kind is not**, and modified works must be shared under the same license. The full terms are in [LICENSE](LICENSE).

> [!NOTE]
> Although the source code is fully public, please follow the [LICENSE](LICENSE) when using this project.

## Copyright Notice

`Altium` and `Altium Designer` are trademarks of Altium Limited; `Cadence`, `OrCAD` and `Allegro` are trademarks of Cadence Design Systems, Inc.; `LCSC`, `LCEDA` and `JLC` are trademarks of Shenzhen JLC Technology Group Co., Ltd. `KiCad` is the name and mark of the KiCad project and its developer community, released under the GPL. This project is not affiliated with or endorsed by any of these companies or projects; the names are used only to describe compatibility.
