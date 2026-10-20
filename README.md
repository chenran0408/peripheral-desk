# Peripheral Desk · 外设控制中心

Rust + Win32 原生 Windows 外设设置与电量工具。黑灰色界面、红橙强调色、两级设备页面，无浏览器运行时。

## 下载与使用

在 [最新版本](https://github.com/chenran0408/peripheral-desk/releases/latest) 下载 `PeripheralDesk-*-windows-x64.zip`，解压到可写文件夹，运行 `PeripheralDesk.exe`。无需安装驱动或管理员权限。

- 首页选择设备：在线亮色、离线暗色、接收器在线但无有效读数时显示待唤醒。
- 左键托盘图标显示设备电量；右键可打开主页、刷新或退出。
- 点右上角关闭会留在托盘。彻底退出请使用托盘菜单或首页的“退出程序”。
- 设置停止调整 1.5 秒后保存，并从设备读回核对；失败会显示提示。
- 默认每 60 秒刷新，低于 20% 时提醒。工具设置中可调整。
- 更新时退出程序，用新版文件替换旧版，保留同目录 `data` 文件夹即可。

## 已接入设备与功能

| 设备 | 电量/充电 | 基础设置 | 已移植的更多设置 |
| --- | --- | --- | --- |
| 炼狱蝰蛇 V3 专业版 | 支持 | DPI、5 档 DPI 表、回报率、休眠 | 低电量省电阈值 |
| 迈从 A5 Ultra 初代 | 支持 | DPI、6 档 DPI 表、回报率、休眠 | 按键防抖、LOD、Motion Sync、直线修正、波纹控制 |
| 迈从 G75 Pro | 支持 | 回报率、休眠、15 种灯效（含关闭） | 1～9 档亮度、1～4 档速度、Windows 键锁定、低延迟模式 |

DPI 滑块修改当前档位；点档位按钮切换。键盘亮度和速度作用于当前灯效，关闭灯效时不可调。原驱动部分灯效本身不支持变速。

当前版本是原驱动功能的第一阶段移植。改键、宏、逐键 RGB、配置文件管理及雷蛇传感器校准尚未开放，见 [移植进度](docs/FEATURES.md)。

## 已验证范围

实机：雷蛇 `1532:00B7` 接收器、A5 Ultra `3837:100C` 接收器、G75 Pro `3554:FA09` 接收器。G75 的通用接收器还会读取型号签名，匹配 G75 Pro 后才开放操作。

雷蛇有线 `1532:00B6`、G75 `41E4:2001` 为协议入口，尚未完成对应硬件验证。A5 V2/V3、A5 Pro Max 和其他 G75 版本不会按本型号强行适配。蓝牙不在本版本支持范围。

鼠标休眠时无法保证有新读数，请先移动设备再刷新。避免雷云或 M HUB 的设备页面同时与本工具通信。灰色缓存电量会标明“上次电量”。

## 编译与测试

Windows 10/11 x64，安装 Rust stable 和 Visual Studio C++ Build Tools（含 Windows SDK）。

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
powershell -File scripts/package.ps1
```

常规测试不访问设备。标记 `ignored` 的实机测试需要三台设备已连接并唤醒：其中 `live_feature_roundtrips_restore_original_values` 会临时切换设置并恢复原值，不在 CI 自动运行。

```powershell
cargo test -- --ignored --nocapture --test-threads=1
```

命令行：`--hidden` 只启动托盘；`--battery` 打开电量浮窗；`--once path.json` 导出一次状态；`--diagnose path.json` 导出 HID 元数据（不含设备路径）。

自动构建模板见 [docs/windows-build.yml](docs/windows-build.yml)。复制到 `.github/workflows/windows.yml` 可启用 Windows 构建；推送该目录需要 GitHub 凭据具有 `workflow` 权限。当前版本在本机完成构建和测试后发布。

## 扩展设备

[适配指南](docs/ADAPTER_GUIDE.md) 提供接入步骤、协议适配器契约、能力声明、测试及发布要求。核心结构：

```text
设备注册表 devices.rs → 统一刷新引擎 backend.rs → DeviceSession 适配器
                          ↓                       ↓
                     状态 model.rs           协议 codec / HID
                          ↓
                 主界面 / 托盘 / 低电提醒
```

同一协议新增型号主要修改注册表；新协议新增 `DeviceSession` 实现及工厂分支。高级设置通过元数据生成控件，无需为每个型号复制窗口。

## 来源与许可

项目代码使用 AGPL-3.0-only。产品图片和商标归各自权利人所有，用于识别设备，不随代码授予 AGPL 许可。协议研究、依赖许可和图片出处见 [SOURCES.md](SOURCES.md)、[NOTICE](NOTICE) 与 [THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES)。
