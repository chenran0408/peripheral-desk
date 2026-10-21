# Sources and attribution

## Protocol research

- [OpenMouse mouse-protocol](https://github.com/OpenMouse-Project/mouse-protocol), AGPL-3.0, revision `bac031567b28f2a55c616fc525b03f2d5fa452cc`.
- [mchosebar](https://github.com/kuz-1/mchosebar), GPL-3.0, revision `fc18f6952ce167fda6a6aea60af6fd8d47223194`.
- [razer-battery-report](https://github.com/xzeldon/razer-battery-report), MIT, revision `4abf90c280a35fe9c451c17589bedaf237204ec9`.
- [openrazer-win mouse controls](https://github.com/GavinViney/openrazer-win/blob/472a4faab5fa72339839f7f0d222c689d2886dd5/src/openrazer_win/mouse.py), GPL-3.0-or-later, revision `472a4faab5fa72339839f7f0d222c689d2886dd5`. Used as a protocol-layout reference alongside OpenMouse for class `0x02` button commands `0x8C` / `0x0C`. The upstream observations concern other models; DeathAdder V3 Pro firmware responses are checked independently, and unknown mappings remain opaque. The Python application is not bundled.
- [MCHOSE HUB](https://www.mchose.com.cn/), public driver JavaScript protocol schemas inspected in October 2026. The first-generation A5 performance schema was checked for DPI stage count, sensor rotation, esports mode and the no-sleep value. The G75 Pro module was checked for the 128-byte performance block, Mac/Windows-key-lock rules, per-effect control limits and the 490-byte color block (`0x49` read / `0x09` write). The local device confirmed the color-block format; full-block readback and restoration were verified. Local implementation does not bundle the driver or its application code.
- [Razer DeathAdder V3 Pro manual](https://dl.razerzone.com/master-guides/RazerSynapse3/DEATHADDERV3PRO-00000182-en.pdf).
- [G75 Pro manual](https://file.maicong.cn/uploads/25/01/20250113.pdf).

The MCHOSE HUB bundle inspected for this round was `purify.es-BgEB9_OY.js`, SHA256 `9f8c8cb5c560583adf581a0c82c2b08fbe7a479d4d47e0ca378874f96587d32d`. It is vendor software, not an open-source dependency. Only independently implemented wire encodings are distributed. First-generation A5 button and macro schemas (`Fst`, `Gst`, `nJ`) informed six-button bindings and the experimental single-page macro codec; the tested firmware returns unconfirmed length-zero macro pages, so macro writes are refused. G75 ordinary/Fn bindings and planar per-key RGB (`0x41` / `0x42`) preserve all 126 slots, including hidden matrix slots; performance (`0x44`) and color-block restoration are verified separately. No A5 Pro Max macro commands are used for the first-generation A5.

Workflow inspiration: [quiet-battery](https://github.com/TheWebDog/quiet-battery) (PolyForm Noncommercial 1.0.0) and [2.4G-battery-viewer](https://github.com/yszyszysz233/2.4G-battery-viewer). Their source is not bundled into this executable. Device protocols are implemented in Rust.

## Product images

Images remain the property of their respective manufacturers and are excluded from the code's AGPL grant. They are used to identify supported products, with no endorsement implied.

- `assets/deathadder.png`: Razer official CDN, [DeathAdder V3 Pro black](https://assets3.razerzone.com/LZufqyVl2jCUqKmH66_tKX2eRck=/300x300/https%3A%2F%2Fmedias-p1.phoenix.razer.com%2Fsys-master-phoenix-images-container%2Fhc9%2Fh51%2F9449963946014%2Fdeathadder-v3-pro-black-v2-500x500.png).
- `assets/a5.png`: MCHOSE official HUB [shared A5 illustration](https://cdn.mchose.com.cn/configCenter/assets/img/mouse/A5V2Ultra_white.png). HUB also assigns this image to first-generation `A5 ultra`; the image filename does not determine the supported protocol generation.
- `assets/g75.png`: MCHOSE official HUB [G75 Pro illustration](https://cdn.mchose.com.cn/configCenter/assets/img/keyboard/G75Pro_whiteLine.png).

The user-supplied UI reference is not distributed in this repository.

## HECATE G2 Pro

- 用户提供的官方 HECATE 1.1.44 安装包，仅用于本地兼容性研究；不在仓库或发布包中分发。
- [漫步者官方驱动下载](https://www.edifier.com/cn/driver.html)。
- [G2 Pro 官方产品页](https://cnweb.edifier.com/product/product-684.html)。
- `assets/g2pro.png`：官方安装包中的设备缩略图，仅用于识别设备，归 EDIFIER / HECATE 所有。
- Windows 原生音量使用 Microsoft Core Audio 的 [IAudioEndpointVolume](https://learn.microsoft.com/windows/win32/api/endpointvolume/nn-endpointvolume-iaudioendpointvolume)。
- 音效桥接调用用户本机已安装的 C-Media SDK（1.0.8.0 x64）。未分发厂商 DLL、反编译代码或原界面资源。
- 音效 JSON 配置、参数范围校验、导入顺序与失败恢复由本项目实现；不附带官方音效预设或声学算法。
- G2 Pro 灯光语义来自上述官方程序的 `LedEndpoint_G2Pro`：设置入口 `0x140375540`、严格开关回复处理 `0x140376740`。分析的 HECATE.exe SHA256 为 `a1bd42c16dd46c6a86975b70ce79e658c035d2488ec09913185182d4a6086e3f`，不分发厂商程序或反编译源码。只确认了开关读→切换→恢复；模式、RGB 与速度没有在本轮写入验证。
- [HIDAPI Windows transport](https://github.com/libusb/hidapi/blob/852cc68b8e0e4c7eba0815ef992951cd8b75c55b/windows/hid.c#L1171)，固定提交 `852cc68b8e0e4c7eba0815ef992951cd8b75c55b`。其 [许可说明](https://github.com/libusb/hidapi/blob/852cc68b8e0e4c7eba0815ef992951cd8b75c55b/LICENSE.txt) 为 GPLv3 / BSD-style / 原 HIDAPI 许可可选。这里只参考 Windows `HidD_SetFeature` 必须按描述符长度补齐的语义，自行实现 Rust 传输，未复制或分发 HIDAPI 代码或 DLL。
- 更完整的灯光接口依据与实测边界见独立驱动仓库的 [G2 Pro 灯光文档](https://github.com/chenran0408/peripheral-desk-drivers/blob/main/docs/G2_LIGHTING.md)。

- [Microsoft：HID 丢报文与输入环形队列](https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/troubleshooting-hid-reports)、[HidD_SetNumInputBuffers](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/hidsdi/nf-hidsdi-hidd_setnuminputbuffers)：默认队列32份报告，G75灯色回复35包，读取通道预设128份容量。

## Rust dependency license inventory

`THIRD_PARTY_LICENSES/` retains unmodified license texts from the local Cargo registry sources, including the new ZIP/DEFLATE dependency chain. `MANIFEST.json` records each package version, license expression, registry checksum and license-file SHA256. The inventory covers the entire Cargo.lock registry-package set, including optional, build and target-specific dependencies; it is a superset of what each executable or device DLL links. Existing license files are retained when dependencies change.

Maintain the inventory with Python 3.11+ and `scripts/collect-licenses.py` after `cargo fetch --locked`, using the same CARGO_HOME as the build. The script verifies source texts against Cargo's checksum records or the cached `.crate` archive and refuses to replace a different license text. The separate driver repository carries this same complete core inventory as an explicit attribution superset; it does not imply that ZIP handling is linked into device DLLs.

安装器简体中文翻译：kira-96/Inno-Setup-Chinese-Simplified-Translation，固定提交 `1ff90acc4ed4aee82b1cda43253243deee3daed4`，MIT；原文件与授权在 installer/Languages，分发授权见 THIRD_PARTY_LICENSES/inno-chinese-translation。
