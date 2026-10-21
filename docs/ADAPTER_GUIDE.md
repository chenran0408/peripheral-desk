# 设备插件与适配框架

## 仓库和构建边界

主程序仓库负责 Win32 页面、托盘、统一模型、缓存、插件安装/更新、配置与诊断。[peripheral-desk-drivers](https://github.com/chenran0408/peripheral-desk-drivers) 负责设备包、独立版本、设备适配贡献与签名目录发布。新增设备优先在插件仓库提交 PR；新增通用模型语义或界面能力时协调主程序改动。

用户操作见 [安装与使用](USAGE.md)，交付边界见 [本版测试范围](VALIDATION.md)。新增品牌先看 [五品牌协议调查与参考项目核查](https://github.com/chenran0408/peripheral-desk-drivers/blob/main/docs/PROTOCOL-RESEARCH.md)，不得把上游设备列表或只读预览当作本项目实机支持。

Cargo 默认 `desktop` 只编译桌面与插件代理；`driver-runtime` 编译设备协议、键盘传输与音效运行时。二者单独构建和检查，不应使用 `--all-features` 代替验证两个产物。四款现有插件转发固定核心版本的 runtime；新设备可使用插件仓库模板实现自有 handler，不需把全部协议写回主程序。

```text
Win32 UI / 托盘
      ↓
model / controls / backend / profiles / configuration
      ↓
plugin_manager::invoke(device_key, JSON)
      ↓
受限时长的隔离进程 → 已校验的设备 DLL → codec / HID / 音频组件
```

| 层 | 主要文件 | 责任 |
| --- | --- | --- |
| 元数据 | `devices.rs`、插件 manifest | 稳定 key、严格接口匹配、图片、能力范围与选项 |
| 通用模型 | `model.rs`、`controls.rs`、`audio_lighting.rs` | 可序列化状态与动作，不传句柄 |
| 桌面代理 | `adapters/mod.rs`、`audio_fx.rs`、`controls.rs` | 按 key 转发插件请求，不内嵌具体设备协议 |
| 管理与隔离 | `plugin_manager.rs` | 签名目录、包校验、安装/回退、版本兼容与 worker |
| 协议运行时 | `plugin_entry.rs`、`adapters/`、`keyboard*.rs`、`*_runtime.rs` | 校验型号、打开通道、读写、核对与恢复 |
| 存储与配置 | `storage.rs`、`profiles.rs`、`configuration.rs` | 原子保存、未知字段保留、备份、迁移与独立应用 |
| 展示 | `ui.rs`、各专用编辑器、`paint.rs` | 元数据生成卡片/设置，结构化改键、颜色与预设页面 |

`DeviceSession` 保留 `read() -> Result<State>`、`apply(Action) -> Result<()>` 边界。`Ok` 应代表相应协议核对成功，不能只代表发送成功。无法完整读回的项目须在状态和说明中明确区分，如 G2 灯光只有开关为设备确认，其他参数是本工具的发送记录。

## 新设备接入

1. 使用长期稳定的小写字母/数字/连字符 key；同一 key 用于 manifest、缓存和请求，不能使用列表位置作为持久身份。
2. 在插件仓库模板中实现设备 handler 与 manifest，声明真实支持的能力；匹配 VID/PID、usage、报告长度等，共享接收器还必须核对型号签名。
3. 协议 codec 与传输分离，优先复用已有 HID 传输。未知字段作为原始值保存，不给它编造可写默认值。
4. 同义设置复用 `Action`/`Tuning`；映射、宏、RGB 使用 `ControlAction`/`ControlState`，不要把原始字节塞进数值设置。
5. 能力元数据描述型号可能支持的项目，实际 `State.tuning`/控件状态只返回本轮确认的可用项。固件缺失、互斥模式或未知响应均须在适配器拒写，不能只禁用 UI。
6. 补充来源、夹具、字段保留和失败恢复测试，记录实测连接方式与固件。读成功、写后读回、实际功能输出和断电保持是不同验证层次。
7. 维护者审核、构建、签名并发布包。未经签名的 PR 产物不可直接加入用户受信目录。

主程序启动时合并内置设备目录和已安装插件描述，页面按 key 恢复数据。新增设备通常不必重写卡片/托盘；未知新语义仍需要兼容的主程序和编辑器支持，不能只声明一个能力字符串就视为实现。

## ABI、目录与发布

插件是 Windows x64 用户态 DLL，ABI 1 导出 `pd_driver_abi`、`pd_driver_request`、`pd_driver_free`。请求/响应为 UTF-8 JSON，响应结构为 `{ok, value, error}`；返回缓冲区须由同一 DLL 释放。详细内存契约、请求字段与限制以插件仓库 [ABI.md](https://github.com/chenran0408/peripheral-desk-drivers/blob/main/docs/ABI.md) 为准。

现有 op 包括 `read-state`、`apply-setting`、`controls-read`、`controls-apply`、`fx-query`、`fx-worker`、`lighting-read`、`lighting-apply`。未知操作必须明确返回失败。插件固定设备 key，不能允许输入切换到其他型号。

目录按通道固定发布在 `catalog-stable` / `catalog-preview` release，文件 `catalog-<channel>.json` 配对 `.json.sig`。宿主验证内置公钥签名、包大小与 SHA-256、DLL SHA-256、manifest 一致性、ABI 和主程序版本范围；离线包也必须匹配受信目录，ZIP 不嵌入循环签名。通道独立，不通过 latest release 猜目录。

安装先解压到暂存目录并检查路径、内容和 DLL ABI，再备份配置、激活版本指针。保留前一版本用于回退；卸载移除插件而保留设备配置。插件调用在隔离进程受控执行，不能把 Rust panic 捕获视为无效内存访问或任意 native 异常的安全边界。

## 状态、写入与恢复规则

- `present` 是枚举到接口，`fresh` 是本轮收到有效读数，二者不可混同。休眠/断开时保留电量缓存，但清除可写能力，标明上次读数。
- 电量和可选设置独立处理，设置读取失败不应丢掉有效电量；未知数据不写，错误按实际阶段报告。
- Report ID、Feature/Input/Output 长度及校验分别验证；超时取消并排空 overlapped I/O，再释放缓冲。
- 写前读取完整基线，修改必要字段，保留无关位和尾部；按设备协议确认后完整读回。丢失 ACK 只有最终整块读回精确匹配时才能接受。
- 失败恢复所有已尝试项，包括可能已生效但确认失败的一项；恢复也须核对并明确报告失败。不能承诺断开、进程崩溃或超时后必然恢复。
- 已写入且核对成功、随后刷新失败时，提示实际阶段，不能误称设置已回滚。

常用设置队列按设备和字段合并，停止调整后提交。普通操作和批量配置应用在后台进行；`refresh_device` 只刷新目标设备，定时/主页刷新负责全列表。

## 已接入协议的保留边界

- A5：63 字节按键配置保留其他槽位；DPI 缩档保留未启用值并检查两连接当前档位，禁止删掉最后一个主点击。
- A5 板载宏的受限编码实现要求先取得完整单页和原名称；仅确认空页或与本工具已保存宏匹配才允许覆盖，保留原页尾部并在失败后恢复核对。本机实物页长度为 0，不满足完整备份条件，因此安全拒写，不能列为已验证板载宏。当前编码仅单次键盘宏、最多 13 事件；软件宏使用普通改键路径，不依赖该页。
- G75：型号签名通过后读 128 字节性能块；亮度/速度位域独立保留，Mac 开启时清除 Win 锁。490 字节预设色块只编辑当前允许模式的 RGB。
- G75 映射：三层实物块各 504 字节，前 96 槽对应布局，空位及尾部不改；378 字节逐键色按 R/G/B 三个 126 槽平面，未展示槽位保留。物理 Fn 和未知编码只读，板载宏未确认，不开放写入。
- 雷蛇：标准层改键须有效主键回复且目标原始标记为 0；标记 1、左键与未知槽只读，不能清零未知标记来开放写入。本机仅前进侧键（槽 5）完成本轮往返；不把其他 Razer 型号的 LOD、Hypershift 或宏命令直接用于本型号。

上述偏移只适用于已确认协议，不能成为其他型号的通用写入表。

## 配置迁移与隐私

`resident_macros.rs` 属于 desktop，不编译进设备 DLL。常驻线程拥有 `RegisterHotKey`；先注册可用 F13–F24，再持久化原绑定恢复记录，最后使用现有结构化改键写入并读回。禁止以左键、物理 Fn、未知编码或已有板载宏为触发。播放仅单次平衡键盘步骤，最多 128 步/30 秒；保留触发键不能出现在播放事件中，不并发播放，取消/异常尝试释放按键。

定义和恢复记录固定存于 `data/devices/<key>/resident-macros.json`。`disable_device(key)` 用于普通改键、配置应用、更新/卸载前恢复；正常退出必须等待 `shutdown()` 并处理错误，不能先销毁进程。失败保留 recovery，不自动启用；下次界面可手动恢复。导出只携带定义，导入保留本机 recovery，不导入外机恢复记录；批量配置不会自动启用软件宏或下发 F13–F24 触发映射。软件宏恢复确认的是结构化绑定，不代表整个固件块的字节级快照。原绑定被其他设置改动时拒绝覆盖该新值并保留记录。

三台设备已核对 F13 触发绑定及恢复；实际热键触发至 SendInput 播放尚未实测。该绑定验证不是宏播放端到端或断电保持验证，不自动向其他窗口注入测试输入。

设备配置 schema 1 按 key 归档缓存、已保存 controls/lighting/effects 与固定映射的 A5 宏、G2 EQ/灯光文件，大小限制 8 MiB。不导出 SDK、HID 路径和序列号。兼容已知 legacy/schema 0 形态，但严格检查设备身份；不理解的字段留在存档，不将其直接发送设备。

导入先准备并校验所有目标，保存原始字节备份，再原子写入；任一失败尝试恢复全部已尝试文件。对象递归保留未知字段，数组替换（包括空数组清空）。常规保存宏使用 `write_json_preserving`；EQ 库对库和预设的未知字段使用 flatten，按稳定 ID 操作。显式删除不能被通用 merge 加回来。

导入只恢复本地。“应用到设备”另行预读所有所需基线，生成已支持动作，逐项执行；失败反向恢复已尝试项。未知宏、不可完整恢复的 G2 灯光和状态快照里的普通性能值跳过并说明，不能伪装整机克隆。

匿名诊断仅记录版本、数字接口信息、连接状态与概括错误。禁止直接保存 raw error（可能包含用户名/路径）、设备串号、SDK 位置或原始宏；个人配置导出也不能作为匿名诊断。

## 音频边界

`Kind::Headset` 不含电池、DPI、回报率和休眠能力。Core Audio 区分 GAMING / CHAT / 麦克风，拒绝歧义匹配；线程所属 COM 句柄、PROPVARIANT 和字符串须正确释放。

音效经已校验的官方组件处理，不复制厂商算法或分发 DLL。`FxSpec` 的通道/属性/最大值与 EQ 偏移明确；30 Hz～16 kHz 十段 EQ 在共享模型以 20 表示 0 dB，SDK 单位为 1 dB。可选能力缺失就不返回该项，读操作不擅自启用 Windows 音频增强。

音效 JSON schema 1/设备 `hecate-g2-pro`，限制 64 KiB，拒绝错型号、未知音效参数和越界值。事务顺序为先关闭开关、写参数、最后启用，每项读回并在失败后反向恢复。命名 EQ 库仅保存十段与 EQ 开关，默认曲线是工程预设，不能称官方或声学校准。

G2 灯光开关已完成实机切换、读回与恢复。模式/RGB/速度发送入口和映射未做本轮实机写入/效果验证，也没有完整设备读回，界面仅使用本工具保存记录。恢复边界必须与实际协议一致，不声称完整原灯效可读取。

## 验证与贡献

```powershell
cargo check --locked
cargo test --locked --lib
cargo check --locked --no-default-features --features driver-runtime --lib
cargo test --locked --no-default-features --features driver-runtime --lib
```

优先少量有意义的软件测试：未知字段/尾部保留、空数组清除、导入失败恢复、敏感信息排除、错误响应拒写。CI 不操作硬件。实机按需要只读 → 有限写入并恢复 → 最终读回，避免无变化重复测试；实际范围写入 [TESTING.md](TESTING.md)。ABI 探测只证明接口和错误处理，不代表设备协议已通过。

设备协议 PR 去 [插件仓库](https://github.com/chenran0408/peripheral-desk-drivers)，主程序界面、管理与配置 PR 留本仓库。发布物禁止包含官方 SDK/驱动/安装包或反编译输出；本地 SDK 路径仅存被 Git 忽略的数据文件。
