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

**SeEx** 是一个批量导出立创商城元件为 Altium Designer（AD）、KiCad、Cadence（OrCAD Capture / Allegro）可用格式的桌面工具。

## ✨ 功能特性

### KiCad

- 导出 KiCad 可用的 `Symbol`、`Footprint`、`3D` 模型格式。
- 支持批量导出，也支持单独导出。

### Altium Designer（AD）

- 单独导出器件的 `schlib`、`pcblib`（`pcblib` 已经内嵌了 `step` 模型）。
- 支持批量导出。
- 支持把多个器件导出到同一个 `schlib`、`pcblib`，即合并导出。

### Cadence（OrCAD Capture / Allegro）

- 导出 `Capture` 原理图符号：一个 `Library XML` 文件，在 `Capture` 中通过 `File > Import > Library XML` 转换为 `.olb`。
- 导出 `Allegro` 封装：每个封装一个 `SKILL` 脚本，外加一个库脚本；在 `Allegro PCB Editor` 的命令窗口执行 `skill load("<库>_allegro/<库>.il")`，即在同一目录生成 `.dra`、`.psm` 和 `.pad`。
- 导出 `3D` 模型（`step`），封装中已按立创EDA的位置映射好模型。
- 标准封装（贴片阻容、`SOT`、`SOD`、`SOIC`、`QFN` 等）按 `IPC-7352` 计算焊盘并核对厂家焊盘；其余封装保留厂家焊盘，按 `IPC-7352` 规则生成丝印、装配层和占位区。
- `seex` 本身不调用任何 `Cadence` 程序，导入由用户在 `Cadence` 中完成。

### 共同特性

- 均支持选择中英文元数据导出。
- 均支持多线程下载。
- 均支持向已现存的经由 `seex` 导出的库里添加新的器件/元件。
- 均支持自定义原理图的描边和填充颜色。
- 均支持单独导出 `3D` 模型。
- 默认去除 `3D` 模型上的立创EDA标志（水印），重建模型；无法安全去除时保留原模型，并在导出结果中列出这些元件。

## 🔄 工作流

<div align="center">
  <img src="imgs/workflow.png" alt="npnp/seex 工作流：立创商城编号 → seex / npnp → KiCad、Altium、Cadence" width="900"/>
</div>

## 📸 界面预览

<table>
  <summary>SeEx 界面预览</summary>

  <table>
    <tr>
      <td><img src="imgs/seex_layout/zh/1.png" width="600" /></td>
      <td><img src="imgs/seex_layout/zh/2.png" width="600" /></td>
      <td><img src="imgs/seex_layout/zh/3.png" width="600" /></td>
    </tr>
    <tr>
      <td><img src="imgs/seex_layout/zh/4.png" width="600" /></td>
      <td><img src="imgs/seex_layout/zh/5.png" width="600" /></td>
    </tr>
  </table>
</table>

### 导出示例

<table>
  <summary>KiCad 符号与封装示例</summary>

  <table>
    <tr>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_1.png" alt="KiCad 符号与封装示例 1" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_2.png" alt="KiCad 符号与封装示例 2" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_3.png" alt="KiCad 符号与封装示例 3" width="420" /></td>
    </tr>
    <tr>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_4.png" alt="KiCad 符号与封装示例 4" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_5.png" alt="KiCad 符号与封装示例 5" width="420" /></td>
      <td><img src="imgs/showcase/kicad/sch_pcb/sch_pcb_6.png" alt="KiCad 符号与封装示例 6" width="420" /></td>
    </tr>
  </table>
</table>

<table>
  <summary>KiCad 3D 模型示例</summary>

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
  <summary>Altium SCH + 3D 模型库示例</summary>

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
  <summary>Altium PCB 示例</summary>

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

## 🖥️ 支持的 EDA 版本

> [!NOTE]
> 这些版本仅为目前所知，不排除 `seex` 支持更多的版本。如果您无法在更低版本的 `EDA` 软件中打开 `seex` 导出的库，可以在 `issue` 里提出，会考虑适配。

- `KiCad 9.0` ~ `Latest`
- `Altium Designer 23.x` ~ `Latest`
- `Cadence SPB 26.1`（OrCAD X Capture / Allegro X）；`Allegro` 封装脚本需要支持 `SKILL` 的版本（`Allegro PCB Designer` 或 `OrCAD PCB Designer Professional`）

## 🚀 安装与使用

### 下载

从 [Releases](https://github.com/ref42/seex/releases) 下载对应平台的压缩包，解压后直接运行 `seex.exe`，无需安装。

### 从源码构建

```bash
git clone https://github.com/ref42/seex.git
cd seex
cargo build --release
```

可执行文件位于 `target/release/seex.exe`。

### 基本流程

1. 在 `监听` 页复制立创商城的器件编号（如 `C2040`），或手动导入 ID 文件。
2. 进入 `导出` 页，选择 `Altium`、`KiCad` 或 `Cadence` 目标与导出范围。
3. 点击 `导出库`，完成后到输出目录取用生成的库文件。

## 📖 项目历程

整个项目的开发历程（按照时间线）：

- `nlbn（Nice Library Bring now，谐音“拿来吧你”）`
  - 现已停止维护
- `npnp（Normalize Pin Net Pad，谐音“牛批牛批”）`
  - 会持续维护（与 `seex` 互为姐妹项目）
- `seex（seek and Export）`
  - 会持续维护（与 `npnp` 互为姐妹项目）

部分吐槽：

- 此项目还未发布之时，市面上几乎没有此类工具，不知为何最近多了许多相似的项目，这是一件好事，起码大家都有事做。但是这样一个工具实在不值得换一换皮就拿到某平台进行售卖，真的太没品了。基础工具存在的意义就是让每个开发者都能使用，而不是开动脑筋搞经济，真的太没品了。🤣

## 🧩 姐妹项目

- [npnp](https://github.com/ref42/npnp) 是 `seex` 的 `cli` 版本，更适合 `AI`/`Agent` 直接调用。
  - 你甚至可以让 `AI` 帮你分析某个工程，然后调用 `npnp` 导出所需要的元件。这样就不必再手动搜索挑选器件，只需要核验把关即可。
  - 如果你有待导出元件的客编号，那就更好了，直接喂给 `npnp` 导出即可。
  - 你也可以将 `npnp` 作为 `CI` 工具。

## 🗺️ 路线图

- 支持更多的 `EDA` 工具，类似 `pads` 等。
- 继续完善现在的工作流，修复更多隐藏的 `bug`。
- 持续优化交互功能，提升用户使用体验，优化图标，尝试新的设计语言。

## 🤝 贡献

- 具有软件设计经验，能持续维护软件。
- 审美在线，非常期待得到您的宝贵建议。
- “设计师”可以帮助我们重新设计软件的图标，界面 Layout 等。
- 如果你的 `Token` 花不完，同时也对本项目感兴趣，大可进行尝试，本项目不排斥使用 `AI`，只需确保提交 `PR` 的时候，所有作者为物理世界存在的活人即可。
- 并不一定需要贡献代码，所有能帮助 `seex` 和 `npnp` 变得更好用的建议都会被考虑，并会在文末致谢。

## 🙏 致谢

以下排名不分先后，仅按照软件开发时的时间线进行排序。

### 参考

- [`easyeda2kicad`](https://github.com/uPesy/easyeda2kicad.py)@[uPesy](https://github.com/uPesy)
  - 虽然只支持 `KiCad`，但也给了我一部分灵感，这是一个很优秀的工具，市面上 `KiCad` 相关的 `easyeda` 导入插件，绝大多数都是在这个工具的基础上进行开发的。
- `IPC-7352(2023)` https://dokumen.pub/qdownload/ipc-7352-generic-guideline-for-land-pattern-design.html
  - 用于 `3D` 模型重建。

### 致谢

- 菊猫大佬
  - 赠送给我一部分 `TOKEN`，虽然也自费购买了一部分，但是最后算下来也属于半买半送了，那个时候 `opus 4.6` 还很好用，猛蹬 `opus 4.6`，实现了很多功能，打下了好的基础。
- `emoji` 老弟@[z217-cpu](https://github.com/z217-cpu)
  - 帮忙测试了多平台兼容性，从项目伊始直至现在都一直在帮我测试，非常感谢！
- `🐏🐏`@[Cloud-Lightning](https://github.com/Cloud-Lightning)
  - 曾在多个项目里使用 `seex`，并且给了我非常多的实用建议，非常感谢！
- `c大佬`@[lins-code54](https://github.com/lins-code54)
  - 为软件提供了导出 `Cadence` 的支持；同时还提供了 `3D` 模型的重建功能，`3D` 模型不再附带云纹或者文字 `Logo`；非常感谢！
- 微信 `1` 群和 `2` 群的所有群友
  - 并没有反感我在该群里发布 `seex` 和 `npnp` 的更新事宜，相反却十分捧场，非常感谢！
- 所有用户
  - 如果没有你们的下载与 `star`，可能这个项目会被我搁置，非常感谢！

## 📄 许可证

本项目采用自定义的 **SeEx Non-Commercial Share-Alike License 1.0**：允许学习、修改与再分发，但**禁止任何形式的商业使用**，且修改后的作品需以相同许可共享。完整条款请参阅 [LICENSE](LICENSE)。

> [!NOTE]
> 项目源码虽已完全公开，但请务必遵循 [LICENSE](LICENSE) 进行使用。

## 版权声明

`Altium`、`Altium Designer` 是 Altium Limited 的商标；`Cadence`、`OrCAD`、`Allegro` 是 Cadence Design Systems, Inc. 的商标；`立创商城`、`立创EDA`、`嘉立创` 是深圳嘉立创科技集团股份有限公司的商标。`KiCad` 是 KiCad 项目及其开发者社区的名称与标识，代码以 GPL 发布。本项目与上述公司及项目均无隶属或关联关系，相关名称仅用于说明兼容性。
