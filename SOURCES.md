# Sources and attribution

## Protocol research

- [OpenMouse mouse-protocol](https://github.com/OpenMouse-Project/mouse-protocol), AGPL-3.0, revision `bac031567b28f2a55c616fc525b03f2d5fa452cc`.
- [mchosebar](https://github.com/kuz-1/mchosebar), GPL-3.0, revision `fc18f6952ce167fda6a6aea60af6fd8d47223194`.
- [razer-battery-report](https://github.com/xzeldon/razer-battery-report), MIT, revision `4abf90c280a35fe9c451c17589bedaf237204ec9`.
- [MCHOSE HUB](https://www.mchose.com.cn/), public driver JavaScript protocol schemas inspected in October 2026. Local implementation does not bundle the driver or its application code.
- [Razer DeathAdder V3 Pro manual](https://dl.razerzone.com/master-guides/RazerSynapse3/DEATHADDERV3PRO-00000182-en.pdf).
- [G75 Pro manual](https://file.maicong.cn/uploads/25/01/20250113.pdf).

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
- Windows 原生音量使用 Microsoft Core Audio 的 IAudioEndpointVolume。
- 音效桥接调用用户本机已安装的 C-Media SDK（1.0.8.0 x64）。未分发厂商 DLL、反编译代码或原界面资源。
