# 在外设盒使用 M HUB 模板

在「设备调节」选择对应的接收器与型号，打开「本地模板」。保存当前设备、导入、重命名和删除在本地完成。点选左侧模板可查看内容，点击「应用所选模板」后才写入当前选中的实物和板载槽。设备离线时仍可管理文件。适用的鼠标和磁轴键盘可点「导入旧库」从这台电脑的本地 M HUB 复制已有模板。相同内容只导入一次，同名的不同模板会分别保存。

| 设备 | 文件与适用范围 |
| --- | --- |
| 迈从 0x4D 鼠标 | 17 款型号均可导入、导出原版 M HUB 的单型号 JSON，也能把当前设置直接保存成该格式。适用于 A5 V3 的 Pro、Ultra、Ultra+(3955)，A7 V3 的标准版、Pro、Pro+、Ultra、V3-X、Magnetic，以及 K5 Pro、Ultra、V2，K7 V2 Pro、Pro+、Ultra、Ultra+ 和 R7 Ultra。每个模板只应用于相匹配的型号。 |
| G75 Pro、初代 A5 Ultra | 继续使用外设盒模板。初代 A5 Ultra 支持原版六键模板和可读的休眠、消抖、抬升高度、传感器修正、电竞模式与旋转设置。USB/无线 DPI 档表、回报率及原版宏随导入文件保存。 |
| 迈从磁轴键盘 | 具有磁轴设置插件的型号支持对应板载配置快照，也可导入 M HUB 导出的 KeyboardProfile v3 JSON。导入时按 USB 厂商与产品标识核对具体布局。原版文件的灯光、宏及自定义参数随文件保留。 |
| V9 Pro 2、R9 | 本地快照包括设备可读的三套板载 EQ 及当前槽。 |

0x4D 鼠标模板包含按键、启用和备用 DPI 档、独立 X/Y、USB 与无线回报率、休眠、消抖、旋转和型号支持的传感器选项。K5 Pro、K5 Ultra 依原版固件门槛读取自适应省电设置。导入文件中的未知字段随文件保留，实际写入使用当前实物的型号参数与板载槽。普通原版文件中的宏按键通常只包含名称引用，能与设备上原槽的完整名称对应时可恢复。外设盒导出的文件也保存可读宏的动作，供另一台同型号设备应用。

协议依据为随包 M HUB `main-DIt5vAcV.js` 的 `readMouseAllInfo`、`setMouseAllInfo`、`mouseKeyToObj`、`dpiToObj` 与模板导出函数。0x4D 鼠标此次使用原版数据布局、软件样本和插件接口验证，尚无本批逐款实机模板切换记录。[原厂前端](https://www.mchose.com.cn/assets/main-DIt5vAcV.js) · [型号及接收器记录](https://github.com/chenran0408/peripheral-desk-drivers/blob/main/docs/MCHOSE-EDITORS.md)
