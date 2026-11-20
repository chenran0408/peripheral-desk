# 为型号插件添加可点选的原生设备图

已提供 `bindings` 的型号可在受签名目录中的 `device` 对象声明 `control_surface`。外设盒将插件的 `device.png` 画在原生编辑器中，按设备回复开放键位、宏和逐键灯。键盘可以用实物图覆盖每个键帽，鼠标可以将侧键和底部按键画成带名称的引线标注。初代 A5 Ultra 和 G75 Pro 的已发布旧插件有内置布局。型号没有布局时，已有的键位列表仍可使用。

## 从一张正视图片开始

建立与图片对应的逻辑坐标系。例如下列片段使用 700 × 305 的画布；图像放在中间，左侧有一个侧键标签。标签和图上的物理按钮选择同一个控制槽。

```json
{
  "control_surface": {
    "size": [700, 305],
    "art": [270, 0, 160, 305],
    "layers": ["普通"],
    "keys": [
      {
        "id": 3,
        "label": "前进侧键",
        "rect": [90, 117, 124, 32],
        "anchor": [278, 115],
        "hit_rect": [268, 97, 17, 32]
      }
    ]
  }
}
```

- `size` 是画布的宽和高。图片按比例放入 `art` 指定的区域，默认填入整个画布。
- `rect`、`art`、`hit_rect` 都按 `[左, 上, 宽, 高]` 表示。键盘用 `rect` 框住实际键帽。鼠标可另用 `hit_rect` 框住照片上的按钮，`anchor` 为引线在图片上的终点。底部按键通过标注选择。
- `id` 对应 `controls-read` 返回的 `bindings[].control` 和 `colors[].control`。直接使用协议槽号，保留矩阵空洞。每个真实控制只声明一次。
- `layers` 按协议层号从零开始排列。同一控制的每层动作独立，从设备读回。已保留或无法解释的绑定显示为只读。
- `controls-apply` 接收有明确槽号和层号的 `Bind` 或 `Colors`。拖动源键到目标键，只复制当前层读到的可编辑动作。写入处理器须读取设备当前值、保留其他槽和未知字段，并据真实应答返回结果。
- `key-colors` 开放逐键着色入口。宏编辑器根据本次读取的 `macro_supported` 和可读宏判断是否开放，支持动作范围和容量由插件提供。

完整鼠标和机械键盘数据可对照 [初代 A5 Ultra](../src/controls_ui/layouts/a5-ultra.json) 和 [G75 Pro](../src/controls_ui/layouts/g75-pro.json)。G75 Pro 示例使用随单一安装包交付的 M HUB 键盘图。其他插件从本包 `device.png` 取得素材，图片的来源及许可放在该型号的贡献记录里。

## 一次定向验收

将热区编号与协议按键表逐项对应；从每个热点中心命中正确槽。键盘至少验证一次 Fn 层拖放。连续提交两项修改后立即返回「电量」，确认两项完成通知，再开编辑器读取板载结果，同时核对未编辑层保持原值。可先使用模拟设备复用前端交互检查，具体型号有变更的关键写入再做定向实机验证。


插件可以声明 `control-sections`，支持在 `controls-read` 和 `controls-apply` 中接收可选字段 `control_section`。普通层为 `{"layer":0}`，Fn、Fn2 分别为 1、2，灯光为 `"colors"`。返回相同的 `ControlState` 类型，只填入本次已真实读回的区域。旧宿主省略字段时返回全量结果，旧插件在新宿主中沿用全量请求。分层只改变查询范围，写入仍须以目标设备的当前块为基线，并核对结果。

[按迈从型号查原生、内置网页与设备限制](https://github.com/chenran0408/peripheral-desk-drivers/blob/main/docs/MCHOSE-EDITORS.md)

声明 `macro-section` 的型号，普通键位页只读当前层。打开板载宏页时发送 `"control_section":"macros"`，读取基本层引用和板载定义。可写入的动作和模式以本次回复为准。17 款迈从 0x4D 鼠标可在 `binding_types` 分别声明 `dpi-up` 和 `dpi-down`。
