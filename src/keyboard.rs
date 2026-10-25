use crate::model::Tuning;
use crate::{
    backend::{now, Action, State},
    hid::wide,
    protocol::Result,
};
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Devices::HumanInterfaceDevice::HidD_SetNumInputBuffers,
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub struct Keyboard {
    reader: Handle,
    writer: Handle,
}
/// The A5 Ultra completes feature requests through a separate 0x13 input
/// notification. Reading the feature before E5 arrives returns a blank echo.
pub struct FeatureReady(Handle);
impl FeatureReady {
    pub fn open(path: &str) -> Result<Self> {
        unsafe {
            let h = CreateFileW(
                wide(path).as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                Ok(Self(Handle(h)))
            }
        }
    }
    pub fn wait(&self, command: u8) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < deadline {
            let mut data = [0u8; 65];
            if Keyboard::transfer(&self.0, &mut data, false)? == 65
                && data[0] == 0x13
                && data[1] == !0xe5
                && data[2] == !command
            {
                return Ok(());
            }
        }
        Err(format!("等待迈从命令 0x{command:02X} 完成通知超时"))
    }
    pub fn drain(&self) -> Result<()> {
        let mut data = [0u8; 65];
        for _ in 0..8 {
            if Keyboard::transfer_timeout(&self.0, &mut data, false, 0)? == 0 {
                return Ok(());
            }
        }
        Err("迈从输入队列持续有旧通知，无法确认本次电量查询".into())
    }
}
impl Keyboard {
    pub fn open(path: &str) -> Result<Self> {
        unsafe fn open(path: &str, access: u32) -> Result<Handle> {
            let h = CreateFileW(
                wide(path).as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                Err(format!(
                    "打开键盘通道失败：{}",
                    std::io::Error::last_os_error()
                ))
            } else {
                Ok(Handle(h))
            }
        }
        unsafe {
            let writer = open(path, GENERIC_READ | GENERIC_WRITE)?;
            let reader = open(path, GENERIC_READ)?;
            // A 490-byte color reply contains 35 reports, exceeding Windows'
            // default 32-report HID input ring. Leave room for up to 40 chunks
            // and queued notifications before starting any device request.
            if !HidD_SetNumInputBuffers(reader.0, 128) {
                return Err(format!(
                    "设置键盘接收缓冲区失败：{}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self { reader, writer })
        }
    }
    fn transfer(handle: &Handle, data: &mut [u8], write: bool) -> Result<usize> {
        Self::transfer_timeout(handle, data, write, 300)
    }
    fn transfer_timeout(
        handle: &Handle,
        data: &mut [u8],
        write: bool,
        timeout_ms: u32,
    ) -> Result<usize> {
        unsafe {
            let event = Handle(CreateEventW(null(), 1, 0, null()));
            if event.0.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = event.0;
            let mut count = 0;
            let ok = if write {
                WriteFile(
                    handle.0,
                    data.as_ptr(),
                    data.len() as u32,
                    &mut count,
                    &mut ov,
                )
            } else {
                ReadFile(
                    handle.0,
                    data.as_mut_ptr(),
                    data.len() as u32,
                    &mut count,
                    &mut ov,
                )
            };
            if ok == 0 {
                if GetLastError() != ERROR_IO_PENDING {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if WaitForSingleObject(event.0, timeout_ms) != WAIT_OBJECT_0 {
                    // Drain cancellation before freeing ov/event/buffer.
                    CancelIoEx(handle.0, &ov);
                    GetOverlappedResult(handle.0, &ov, &mut count, 1);
                    return if write {
                        Err("键盘发送查询超时".into())
                    } else {
                        Ok(0)
                    };
                }
                if GetOverlappedResult(handle.0, &ov, &mut count, 0) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
            }
            Ok(count as usize)
        }
    }
    fn send(&self, packet: &mut [u8; 20]) -> Result<()> {
        packet[19] = packet[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        if Self::transfer(&self.writer, packet, true)? != 20 {
            return Err("键盘请求未完整发送".into());
        }
        Ok(())
    }
    fn frame(&self) -> Result<Option<[u8; 20]>> {
        let mut p = [0; 20];
        let count = Self::transfer(&self.reader, &mut p, false)?;
        if count == 0 {
            return Ok(None);
        }
        if count != 20 {
            return Err("键盘报文长度异常".into());
        }
        validate(&p)?;
        Ok(Some(p))
    }
    pub fn read_block(&self, command: u8) -> Result<Vec<u8>> {
        // Battery responses do not carry a transaction number. Drain any
        // already queued 0x4A reply before asking, so a prior poll cannot be
        // mislabeled as a fresh reading. Bound the drain to avoid waiting on
        // a chatty or malformed device indefinitely.
        if command == 0x4a {
            let mut drained = false;
            for _ in 0..8 {
                if self.frame()?.is_none() {
                    drained = true;
                    break;
                }
            }
            if !drained {
                return Err("键盘输入队列持续有旧报文，无法确认电量读数新鲜度".into());
            }
        }
        let mut req = [0; 20];
        req[..3].copy_from_slice(&[0x13, command, 1]);
        for _ in 0..3 {
            self.send(&mut req)?;
            let deadline = Instant::now() + Duration::from_millis(1500);
            let mut chunks = std::collections::BTreeMap::new();
            let mut total = None;
            while Instant::now() < deadline {
                let Some(p) = self.frame()? else {
                    continue;
                };
                if p[1] != command {
                    continue;
                }
                if command == 0x4a && (p[2] != 1 || p[3] != 0 || p[4] != 2) {
                    return Err("G75 Pro 电量应答分包结构不匹配".into());
                }
                if p[2] == 0 || p[2] > 40 || p[3] >= p[2] || p[4] > 14 {
                    return Err("键盘分包字段异常".into());
                }
                if total.is_some_and(|t| t != p[2]) {
                    return Err("键盘分包总数不一致".into());
                }
                total = Some(p[2]);
                chunks.insert(p[3], p[5..5 + p[4] as usize].to_vec());
                if chunks.len() == p[2] as usize {
                    return Ok(chunks.into_values().flatten().collect());
                }
            }
        }
        Err("键盘分包不完整，请唤醒键盘后刷新".into())
    }
    pub(crate) fn read_block_layer(&self, command: u8, layer: u8) -> Result<Vec<u8>> {
        if layer > 2 {
            return Err("键盘配置层超出范围".into());
        }
        let mut request = [0; 20];
        request[..5].copy_from_slice(&[0x13, command, 1, 0, layer << 4]);
        self.read_control_request(request, false)
    }
    fn read_control_request(&self, mut request: [u8; 20], trace: bool) -> Result<Vec<u8>> {
        for _ in 0..3 {
            self.send(&mut request)?;
            let deadline = Instant::now() + Duration::from_millis(1800);
            let mut chunks = std::collections::BTreeMap::new();
            let mut total = None;
            while Instant::now() < deadline {
                let Some(packet) = self.frame()? else {
                    continue;
                };
                if packet[1] != request[1] {
                    continue;
                }
                if trace {
                    println!(
                        "G75 control header {:02x?}; payload {:02x?}",
                        &packet[..5],
                        &packet[5..19]
                    );
                }
                let length = (packet[4] & 15) as usize;
                if packet[2] == 0 || packet[2] > 40 || packet[3] >= packet[2] || length > 14 {
                    return Err("键盘控制配置分包字段异常".into());
                }
                if total.is_some_and(|count| count != packet[2]) {
                    return Err("键盘控制配置分包总数不一致".into());
                }
                total = Some(packet[2]);
                chunks.insert(packet[3], packet[5..5 + length].to_vec());
                if chunks.len() == packet[2] as usize {
                    return Ok(chunks.into_values().flatten().collect());
                }
            }
        }
        Err("键盘控制配置分包不完整".into())
    }
    pub(crate) fn write_block_layer(
        &self,
        command: u8,
        query: u8,
        layer: u8,
        desired: &[u8],
        original: &[u8],
    ) -> Result<()> {
        if layer > 2
            || desired.len() != original.len()
            || desired.is_empty()
            || desired.len().div_ceil(14) > 40
        {
            return Err("键盘控制配置结构不匹配".into());
        }
        if desired == original {
            return Ok(());
        }
        let write = |data: &[u8]| -> Result<()> {
            let total = data.len().div_ceil(14);
            for (index, block) in data.chunks(14).enumerate() {
                let mut packet = [0; 20];
                // The binding command uses a full 14-byte final slot, with
                // zero padding; its YY high nibble selects the key layer.
                let length = if query == 0x41 { 14 } else { block.len() as u8 };
                packet[..5].copy_from_slice(&[
                    0x13,
                    command,
                    total as u8,
                    index as u8,
                    (layer << 4) | length,
                ]);
                packet[5..5 + block.len()].copy_from_slice(block);
                // Sequence-indexed block writes are idempotent. Retry only a
                // missing ACK, retaining the exact layer/payload/padding.
                if let Err(error) = retry_packet_ack(|| {
                    self.send(&mut packet)?;
                    let deadline = Instant::now() + Duration::from_millis(900);
                    while Instant::now() < deadline {
                        if let Some(response) = self.frame()? {
                            if acknowledges(&response, &packet) {
                                return Ok(true);
                            }
                        }
                    }
                    Ok(false)
                }) {
                    if self
                        .read_block_layer(query, layer)
                        .is_ok_and(|read| read == data)
                    {
                        return Ok(());
                    }
                    return Err(format!("键盘控制配置第 {} 包未确认：{error}", index + 1));
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            std::thread::sleep(Duration::from_millis(150));
            if self.read_block_layer(query, layer)? != data {
                return Err("键盘控制配置完整读回不匹配".into());
            }
            Ok(())
        };
        if let Err(error) = write(desired) {
            return match write(original) {
                Ok(()) => Err(format!("{error}；已恢复原配置")),
                Err(restore) => Err(format!("{error}；恢复原配置失败：{restore}")),
            };
        }
        Ok(())
    }
    pub fn verify_model(&self) -> Result<()> {
        let signature = self.read_block(0x05)?;
        if signature.len() < 6 || signature[..6] != [3, 0, 0, 0, 1, 0x15] {
            return Err("接收器连接的设备不是已确认的 G75 Pro，已停止读写设置".into());
        }
        Ok(())
    }
    fn config(&self) -> Result<Vec<u8>> {
        let b = self.read_block(0x44)?;
        if b.len() != 128 || b[126..] != [0x5a, 0xa5] || !(1..=3).contains(&b[1]) {
            return Err("G75 Pro 配置结构不匹配，已禁止写入".into());
        }
        Ok(b)
    }
    pub fn read(&self, identity: String) -> Result<State> {
        let (battery, charging) = parse_g75_battery(&self.read_block(0x4a)?)?;
        let mut s = State {
            present: true,
            fresh: true,
            percent: Some(battery),
            battery_raw: Some(battery),
            connection_path: Some("2.4 GHz 接收器".into()),
            charging,
            updated: now(),
            identity,
            ..State::default()
        };
        match self.config() {
            Ok(c) => {
                s.rate = Some([250, 500, 1000][c[1] as usize - 1]);
                s.sleep_seconds = Some(c[24] as u16 * 30);
                s.light = Some(c[10]);
                if [0, 2].contains(&c[27]) {
                    s.tuning.insert(Tuning::MacMode, u16::from(c[27] == 2));
                }
                // The official driver disables Windows-key locking in Mac mode.
                if c[27] == 0 && c[15] <= 1 {
                    s.tuning.insert(Tuning::WinLock, c[15] as u16);
                }
                s.tuning.insert(Tuning::Latency, u16::from(c[3] == 0));
                if let Some(offset) = light_offset(c[10]) {
                    if supports_brightness(c[10]) && (1..=9).contains(&c[offset]) {
                        s.tuning.insert(Tuning::Brightness, c[offset] as u16);
                    }
                    if c[9] == 0 && supports_color(c[10]) && [0, 7].contains(&(c[offset + 1] & 15))
                    {
                        s.tuning
                            .insert(Tuning::MultiColor, u16::from(c[offset + 1] & 15 == 7));
                    }
                    if supports_speed(c[10]) && (1..=4).contains(&(c[offset + 1] >> 4)) {
                        s.tuning.insert(Tuning::Speed, (c[offset + 1] >> 4) as u16);
                    }
                }
                if color_is_available(&c) {
                    match self.colors() {
                        Ok(colors) => {
                            s.tuning
                                .insert(Tuning::LightColor, color_preset(&c, &colors));
                        }
                        Err(e) => s.settings_error = format!("灯色读取失败：{e}"),
                    }
                }
            }
            Err(e) => s.settings_error = e,
        }
        Ok(s)
    }
    fn colors(&self) -> Result<Vec<u8>> {
        let bytes = self.read_block(0x49)?;
        validate_colors(&bytes)?;
        Ok(bytes)
    }
    pub fn apply(&self, action: Action) -> Result<()> {
        let mut config = self.config()?;
        if let Action::Tune(Tuning::LightColor, value) = action {
            let original = self.colors()?;
            let mut desired = original.clone();
            edit_color(&config, &mut desired, value)?;
            return self.write_with_rollback(9, 0x49, &desired, &original);
        }
        let original = config.clone();
        match action {
            Action::Rate(v) if [250, 500, 1000].contains(&v) => {
                config[1] = [250, 500, 1000].iter().position(|x| *x == v).unwrap() as u8 + 1
            }
            Action::Sleep(v) if (v == 0 || (60..=1200).contains(&v)) && v % 30 == 0 => {
                config[24] = (v / 30) as u8
            }
            Action::Light(v)
                if [0, 1, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13, 15, 16, 17].contains(&v) =>
            {
                config[9] = 0;
                config[10] = v;
            }
            Action::Tune(key, value) => edit_tuning(&mut config, key, value)?,
            _ => return Err("G75 Pro 参数不在允许范围内".into()),
        }
        self.write_with_rollback(4, 0x44, &config, &original)
    }
    fn write_with_rollback(
        &self,
        command: u8,
        query: u8,
        desired: &[u8],
        original: &[u8],
    ) -> Result<()> {
        if let Err(error) = self.write_block(command, query, desired) {
            return match self.write_block(command, query, original) {
                Ok(()) => Err(format!("{error}；已恢复原配置")),
                Err(restore) => Err(format!("{error}；恢复原配置失败：{restore}")),
            };
        }
        Ok(())
    }
    fn write_block(&self, command: u8, query: u8, data: &[u8]) -> Result<()> {
        let total = data.len().div_ceil(14);
        if data.is_empty() || total > 40 {
            return Err("键盘配置块长度异常".into());
        }
        for (index, block) in data.chunks(14).enumerate() {
            let mut p = [0; 20];
            p[..5].copy_from_slice(&[0x13, command, total as u8, index as u8, block.len() as u8]);
            p[5..5 + block.len()].copy_from_slice(block);
            let mut ack = false;
            for _ in 0..3 {
                self.send(&mut p)?;
                let deadline = Instant::now() + Duration::from_millis(900);
                while Instant::now() < deadline {
                    let Some(r) = self.frame()? else {
                        continue;
                    };
                    if acknowledges(&r, &p) {
                        ack = true;
                        break;
                    }
                }
                if ack {
                    break;
                }
            }
            if !ack {
                // A dropped ACK is accepted only if the complete configuration
                // already equals the target, including every preserved byte.
                if self.read_block(query).is_ok_and(|read| read == data) {
                    return Ok(());
                }
                return Err(format!(
                    "键盘未确认第 {} 个配置分包，且读回未匹配",
                    index + 1
                ));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        std::thread::sleep(Duration::from_millis(150));
        if self.read_block(query)? != data {
            return Err("键盘配置读回与写入不一致".into());
        }
        Ok(())
    }
}
fn retry_packet_ack(mut attempt: impl FnMut() -> Result<bool>) -> Result<()> {
    for _ in 0..3 {
        if attempt()? {
            return Ok(());
        }
    }
    Err("三次有界发送均未收到匹配确认".into())
}
fn validate(p: &[u8; 20]) -> Result<()> {
    if p[0] != 0x13 || p[19] != p[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b)) {
        return Err("键盘报文 Report ID 或校验和错误".into());
    }
    Ok(())
}

fn parse_g75_battery(block: &[u8]) -> Result<(u8, Option<bool>)> {
    if block.len() != 2 || block[0] <= 1 || block[0] > 100 {
        return Err("G75 Pro 电量字段无效（协议占位值为 0–1）".into());
    }
    let charging = match block[1] >> 4 {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    Ok((block[0], charging))
}

fn acknowledges(response: &[u8; 20], request: &[u8; 20]) -> bool {
    // M HUB's waitForAck matches command and sequence, not an echoed payload.
    // Also require the same report/packet count and a valid checksum. The ACK
    // only advances transport; write_block still compares the entire readback.
    validate(response).is_ok() && response[..4] == request[..4]
}

fn light_offset(mode: u8) -> Option<usize> {
    Some(match mode {
        1 => 58,
        2 => 60,
        3 => 62,
        4 => 64,
        5 => 66,
        7 => 70,
        8 => 72,
        10 => 76,
        11 => 78,
        12 => 80,
        13 => 82,
        15 => 84,
        16 => 88,
        17 => 90,
        _ => return None,
    })
}
// G75 Pro's verified 490-byte color block has 21-byte effect slots.
// The leading seven-color table is a format guard, not editable preset data.
const COLOR_HEADER: [u8; 21] = [
    255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255, 0, 255, 0, 255, 255, 255, 255, 255,
];
const COLOR_PRESETS: [[u8; 3]; 8] = [
    [255, 0, 0],
    [255, 128, 0],
    [255, 255, 0],
    [0, 255, 0],
    [0, 255, 255],
    [0, 0, 255],
    [255, 0, 255],
    [255, 255, 255],
];
fn validate_colors(bytes: &[u8]) -> Result<()> {
    if bytes.len() != 490 || bytes[..21] != COLOR_HEADER {
        return Err("G75 Pro 灯色配置结构不匹配，已禁止写入".into());
    }
    Ok(())
}
fn color_is_available(config: &[u8]) -> bool {
    config[9] == 0
        && supports_color(config[10])
        && light_offset(config[10]).is_some_and(|offset| config[offset + 1] & 15 == 0)
}
fn color_preset(config: &[u8], colors: &[u8]) -> u16 {
    let offset = config[10] as usize * 21;
    COLOR_PRESETS
        .iter()
        .position(|rgb| colors[offset..offset + 3] == *rgb)
        .map_or(8, |index| index as u16)
}
fn edit_color(config: &[u8], colors: &mut [u8], value: u16) -> Result<()> {
    validate_colors(colors)?;
    if !color_is_available(config) {
        return Err("请先选择支持颜色的单色灯效".into());
    }
    let rgb = COLOR_PRESETS.get(value as usize).ok_or("灯色预设越界")?;
    let offset = config[10] as usize * 21;
    colors[offset..offset + 3].copy_from_slice(rgb);
    Ok(())
}
fn supports_brightness(mode: u8) -> bool {
    light_offset(mode).is_some() && ![15, 17].contains(&mode)
}
fn supports_speed(mode: u8) -> bool {
    supports_brightness(mode) && mode != 1
}
fn supports_color(mode: u8) -> bool {
    // M HUB disables both color controls for rainbow, pinwheel, waterfall,
    // rich-flowers and off. Other verified G75 Pro effects support 0 / 7.
    [1, 2, 4, 5, 7, 8, 10, 11, 12, 13].contains(&mode)
}
fn edit_tuning(config: &mut [u8], key: Tuning, value: u16) -> Result<()> {
    match key {
        Tuning::WinLock if value <= 1 && config[27] == 0 => config[15] = value as u8,
        Tuning::MacMode if value <= 1 => {
            config[27] = if value == 1 { 2 } else { 0 };
            if value == 1 {
                config[15] = 0;
            }
        }
        Tuning::MultiColor if value <= 1 && config[9] == 0 && supports_color(config[10]) => {
            let offset = light_offset(config[10]).ok_or("当前灯效不支持颜色切换")?;
            config[offset + 1] = (config[offset + 1] & 0xf0) | if value == 1 { 7 } else { 0 };
        }
        Tuning::Latency if value <= 1 => config[3] = if value == 1 { 0 } else { 2 },
        Tuning::Brightness | Tuning::Speed => {
            let offset = light_offset(config[10]).ok_or("请先开启一种灯效")?;
            if key == Tuning::Brightness
                && supports_brightness(config[10])
                && (1..=9).contains(&value)
            {
                config[offset] = value as u8;
            } else if key == Tuning::Speed && supports_speed(config[10]) && (1..=4).contains(&value)
            {
                config[offset + 1] = config[offset + 1] & 15 | ((value as u8) << 4);
            } else {
                return Err("灯效参数越界".into());
            }
        }
        _ => return Err("G75 Pro 不支持该参数".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_packet_ack_retries_without_overrunning_or_hiding_transport_errors() {
        let mut sends = 0;
        retry_packet_ack(|| {
            sends += 1;
            Ok(sends == 2)
        })
        .unwrap();
        assert_eq!(
            sends, 2,
            "stop immediately when the retransmission is acknowledged"
        );

        let mut sends = 0;
        let exhausted = retry_packet_ack(|| {
            sends += 1;
            Ok(false)
        });
        assert!(exhausted.is_err());
        assert_eq!(sends, 3, "a missing device cannot cause unbounded writes");

        let mut sends = 0;
        let failed = retry_packet_ack(|| {
            sends += 1;
            Err("transport disconnected".into())
        });
        assert_eq!(failed.unwrap_err(), "transport disconnected");
        assert_eq!(sends, 1, "retry ACK loss, not a failed transport");
    }
    #[test]
    #[ignore = "Read-only G75 control protocol probe; no device settings are changed"]
    fn live_g75_control_formats() {
        let model = crate::devices::REGISTRY
            .iter()
            .position(|s| s.key == "mchose-g75-pro")
            .unwrap();
        let interfaces = crate::hid::enumerate().unwrap();
        let interface = interfaces
            .iter()
            .find(|i| crate::backend::matches(i, model))
            .unwrap();
        let keyboard = Keyboard::open(&interface.path).unwrap();
        keyboard.verify_model().unwrap();
        for (command, layer) in [(0x41, 0), (0x41, 1), (0x41, 2), (0x42, 0)] {
            let mut request = [0; 20];
            request[..5].copy_from_slice(&[0x13, command, 1, 0, layer << 4]);
            let result = keyboard.read_control_request(request, true);
            match result {
                Ok(bytes) => println!(
                    "G75 command {command:02x} layer {layer}: len={} prefix={:02x?}",
                    bytes.len(),
                    &bytes[..bytes.len().min(32)]
                ),
                Err(error) => println!("G75 command {command:02x} layer {layer}: {error}"),
            }
        }
        let mut request = [0; 20];
        request[..7].copy_from_slice(&[0x13, 0x43, 1, 0, 0x10, 3, 2]);
        keyboard.send(&mut request).unwrap();
        let deadline = Instant::now() + Duration::from_millis(1800);
        let mut packets = Vec::new();
        while Instant::now() < deadline {
            if let Some(packet) = keyboard.frame().unwrap() {
                if packet[1] == 0x43 {
                    println!("G75 complete macro response {:02x?}", packet);
                    packets.push(packet);
                }
            }
        }
        println!("G75 macro response packets: {}", packets.len());
    }
    #[test]
    fn write_ack_matches_header_without_requiring_payload_echo() {
        fn checksum(mut packet: [u8; 20]) -> [u8; 20] {
            packet[19] = packet[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b));
            packet
        }
        let mut request = [0; 20];
        request[..5].copy_from_slice(&[0x13, 4, 10, 1, 14]);
        request[5..19].fill(0x5a);
        let request = checksum(request);
        assert!(acknowledges(&request, &request));
        let mut header_only = [0; 20];
        header_only[..4].copy_from_slice(&request[..4]);
        let header_only = checksum(header_only);
        assert!(acknowledges(&header_only, &request));
        for (offset, value) in [(0, 0x12), (1, 0x44), (2, 35), (3, 0)] {
            let mut stale = header_only;
            stale[offset] = value;
            assert!(!acknowledges(&checksum(stale), &request));
        }
        let mut corrupt = header_only;
        corrupt[19] ^= 1;
        assert!(!acknowledges(&corrupt, &request));
    }
    #[test]
    fn declared_keyboard_choices_match_codec_bounds() {
        for setting in crate::devices::G75_ADVANCED {
            for (value, _) in setting.choices {
                let mut config = vec![0; 128];
                config[10] = 2;
                assert!(
                    if setting.key == Tuning::LightColor {
                        let mut colors = vec![0; 490];
                        colors[..21].copy_from_slice(&COLOR_HEADER);
                        edit_color(&config, &mut colors, *value).is_ok()
                    } else {
                        edit_tuning(&mut config, setting.key, *value).is_ok()
                    },
                    "{:?} {}",
                    setting.key,
                    value
                );
            }
        }
    }
    #[test]
    fn light_tuning_preserves_color_nibble_and_other_config() {
        let mut before = vec![0x5a; 128];
        before[10] = 2;
        before[61] = 0x27;
        let mut after = before.clone();
        edit_tuning(&mut after, Tuning::Speed, 4).unwrap();
        assert_eq!(after[61], 0x47);
        after[61] = before[61];
        assert_eq!(after, before);
        assert!(edit_tuning(&mut after, Tuning::Brightness, 100).is_err());
        edit_tuning(&mut after, Tuning::Brightness, 9).unwrap();
        assert_eq!(after[60], 9);
        after[10] = 0;
        assert!(edit_tuning(&mut after, Tuning::Brightness, 9).is_err());
    }
    #[test]
    fn mac_mode_clears_only_winlock_and_rejects_locking_in_mac() {
        let mut before = vec![0x5a; 128];
        before[15] = 1;
        before[27] = 0;
        let mut after = before.clone();
        edit_tuning(&mut after, Tuning::MacMode, 1).unwrap();
        assert_eq!(after[27], 2);
        assert_eq!(after[15], 0);
        assert!(edit_tuning(&mut after, Tuning::WinLock, 1).is_err());
        let mac_snapshot = after.clone();
        assert!(edit_tuning(&mut after, Tuning::MacMode, 2).is_err());
        assert_eq!(after, mac_snapshot);
        after[15] = before[15];
        after[27] = before[27];
        assert_eq!(after, before);
        edit_tuning(&mut after, Tuning::MacMode, 0).unwrap();
        assert_eq!(after, before);
    }
    #[test]
    fn multicolor_changes_only_low_nibble_of_supported_effect() {
        for mode in [1, 2, 4, 5, 7, 8, 10, 11, 12, 13] {
            let mut before = vec![0x5a; 128];
            before[10] = mode;
            before[9] = 0;
            let offset = light_offset(mode).unwrap();
            before[offset + 1] = 0x30;
            let mut after = before.clone();
            edit_tuning(&mut after, Tuning::MultiColor, 1).unwrap();
            assert_eq!(after[offset + 1], 0x37);
            let speed = if supports_speed(mode) {
                edit_tuning(&mut after, Tuning::Speed, 4).unwrap();
                0x40
            } else {
                let snapshot = after.clone();
                assert!(edit_tuning(&mut after, Tuning::Speed, 4).is_err());
                assert_eq!(after, snapshot);
                0x30
            };
            assert_eq!(after[offset + 1], speed | 7);
            edit_tuning(&mut after, Tuning::MultiColor, 0).unwrap();
            assert_eq!(after[offset + 1], speed);
            after[offset + 1] = before[offset + 1];
            assert_eq!(after, before);
        }
        for mode in [0, 3, 15, 16, 17] {
            let mut config = vec![0x5a; 128];
            config[10] = mode;
            let before = config.clone();
            assert!(edit_tuning(&mut config, Tuning::MultiColor, 1).is_err());
            assert_eq!(config, before);
        }
    }
    #[test]
    fn color_presets_change_only_active_rgb_and_keep_unknown_colors_read_only() {
        let mut config = vec![0; 128];
        config[10] = 2;
        let mut before = vec![0x5a; 490];
        before[..21].copy_from_slice(&COLOR_HEADER);
        assert_eq!(color_preset(&config, &before), 8);
        for (preset, rgb) in COLOR_PRESETS.iter().enumerate() {
            let mut changed = before.clone();
            edit_color(&config, &mut changed, preset as u16).unwrap();
            assert_eq!(&changed[42..45], rgb);
            assert_eq!(color_preset(&config, &changed), preset as u16);
            changed[42..45].copy_from_slice(&before[42..45]);
            assert_eq!(changed, before);
        }
        let mut changed = before.clone();
        assert!(edit_color(&config, &mut changed, 8).is_err());
        assert_eq!(changed, before);
        config[61] = 7;
        assert!(edit_color(&config, &mut changed, 0).is_err());
        config[61] = 0;
        config[10] = 3;
        assert!(edit_color(&config, &mut changed, 0).is_err());
        config[10] = 2;
        changed[0] = 0;
        let bad = changed.clone();
        assert!(edit_color(&config, &mut changed, 0).is_err());
        assert_eq!(changed, bad);
    }
    #[test]
    fn fixed_animation_modes_do_not_offer_ineffective_tuning() {
        for mode in [15, 17] {
            let mut config = vec![0; 128];
            config[10] = mode;
            let before = config.clone();
            assert!(!supports_brightness(mode));
            assert!(!supports_speed(mode));
            assert!(edit_tuning(&mut config, Tuning::Brightness, 9).is_err());
            assert!(edit_tuning(&mut config, Tuning::Speed, 4).is_err());
            assert_eq!(config, before);
        }
        let mut config = vec![0; 128];
        config[10] = 1;
        assert!(edit_tuning(&mut config, Tuning::Speed, 4).is_err());
        config[10] = 16;
        assert!(edit_tuning(&mut config, Tuning::Brightness, 9).is_ok());
        assert!(edit_tuning(&mut config, Tuning::Speed, 4).is_ok());
        assert!(edit_tuning(&mut config, Tuning::MultiColor, 0).is_err());
    }
    #[test]
    #[ignore = "Temporarily changes G75 lighting/Mac/sleep and restores complete original blocks"]
    fn live_g75_new_features_restore_full_blocks() {
        fn require(actual: &[u8], expected: &[u8], operation: &str) -> Result<()> {
            if actual == expected {
                Ok(())
            } else {
                Err(format!("{operation}: full block differs"))
            }
        }
        let model = crate::devices::REGISTRY
            .iter()
            .position(|s| s.key == "mchose-g75-pro")
            .unwrap();
        let all = crate::hid::enumerate().unwrap();
        let interface = all
            .iter()
            .find(|i| crate::backend::matches(i, model))
            .unwrap();
        let keyboard = Keyboard::open(&interface.path).unwrap();
        keyboard.verify_model().unwrap();
        let original_config = keyboard.config().unwrap();
        let original_colors = keyboard.colors().unwrap();
        // Every comparison inside this closure returns an error instead of
        // panicking; both original blocks are restored before any assertion.
        let checked = (|| -> Result<()> {
            keyboard.apply(Action::Light(2))?;
            keyboard.apply(Action::Tune(Tuning::MacMode, 0))?;
            keyboard.apply(Action::Tune(Tuning::MultiColor, 0))?;
            let base = keyboard.config()?;
            let prior = color_preset(&base, &original_colors);
            let other = if prior == 0 { 1 } else { 0 };
            let mut expected_colors = original_colors.clone();
            edit_color(&base, &mut expected_colors, other)?;
            keyboard.apply(Action::Tune(Tuning::LightColor, other))?;
            require(&keyboard.colors()?, &expected_colors, "preset color")?;
            require(&keyboard.config()?, &base, "color preserved performance")?;
            keyboard.write_block(9, 0x49, &original_colors)?;
            println!("G75 color preset: {prior} -> {other}; full color block restored");
            let mut expected = base.clone();
            edit_tuning(&mut expected, Tuning::MultiColor, 1)?;
            keyboard.apply(Action::Tune(Tuning::MultiColor, 1))?;
            require(&keyboard.config()?, &expected, "multi-color")?;
            keyboard.write_block(4, 0x44, &base)?;
            println!("G75 multi-color: 0 -> 1; full config restored");
            let mut expected = base.clone();
            edit_tuning(&mut expected, Tuning::WinLock, 1)?;
            keyboard.apply(Action::Tune(Tuning::WinLock, 1))?;
            require(&keyboard.config()?, &expected, "Windows-key lock")?;
            edit_tuning(&mut expected, Tuning::MacMode, 1)?;
            keyboard.apply(Action::Tune(Tuning::MacMode, 1))?;
            require(
                &keyboard.config()?,
                &expected,
                "Mac enabled and Windows lock cleared",
            )?;
            if keyboard.apply(Action::Tune(Tuning::WinLock, 1)).is_ok() {
                return Err("Windows-key locking was incorrectly allowed in Mac mode".into());
            }
            require(
                &keyboard.config()?,
                &expected,
                "rejected lock preserved config",
            )?;
            keyboard.write_block(4, 0x44, &base)?;
            println!("G75 Mac mode and Windows-key lock: mutual exclusion confirmed; restored");
            let mut expected = base.clone();
            expected[24] = 0;
            keyboard.apply(Action::Sleep(0))?;
            require(&keyboard.config()?, &expected, "never sleep")?;
            keyboard.write_block(4, 0x44, &base)?;
            println!(
                "G75 sleep: {} -> 0; full config restored",
                base[24] as u16 * 30
            );
            Ok(())
        })();
        let config_restore = keyboard.write_block(4, 0x44, &original_config);
        let color_restore = keyboard.write_block(9, 0x49, &original_colors);
        let after_config = keyboard.config();
        let after_colors = keyboard.colors();
        println!("G75 final restore: performance {config_restore:?}, color {color_restore:?}, test {checked:?}");
        assert!(config_restore.is_ok(), "{config_restore:?}");
        assert!(color_restore.is_ok(), "{color_restore:?}");
        assert_eq!(after_config.unwrap(), original_config);
        assert_eq!(after_colors.unwrap(), original_colors);
        assert!(checked.is_ok(), "{checked:?}");
        if std::env::var_os("PERIPHERAL_DESK_G75_KEEP_AWAKE").is_some_and(|v| v == "1") {
            let mut expected = original_config.clone();
            expected[24] = 0;
            let keep = keyboard
                .apply(Action::Sleep(0))
                .and_then(|_| require(&keyboard.config()?, &expected, "temporary keep awake"));
            if keep.is_err() {
                let restore = keyboard.write_block(4, 0x44, &original_config);
                assert!(
                    restore.is_ok(),
                    "keep-awake failed and restore failed: {restore:?}"
                );
            }
            assert!(keep.is_ok(), "{keep:?}");
            println!(
                "G75 temporary keep-awake enabled; restore Action::Sleep({}) when testing finishes",
                original_config[24] as u16 * 30
            );
        }
    }
    #[test]
    #[ignore = "Requires G75 Pro; reads the color block without changing settings"]
    fn live_g75_color_format() {
        let model = crate::devices::REGISTRY
            .iter()
            .position(|s| s.key == "mchose-g75-pro")
            .unwrap();
        let all = crate::hid::enumerate().unwrap();
        let interface = all
            .iter()
            .find(|i| crate::backend::matches(i, model))
            .unwrap();
        let keyboard = Keyboard::open(&interface.path).unwrap();
        keyboard.verify_model().unwrap();
        let bytes = keyboard.read_block(0x49).unwrap();
        println!(
            "G75 color block: {} bytes; prefix {:02x?}",
            bytes.len(),
            &bytes[..bytes.len().min(42)]
        );
        if let Some(path) = std::env::var_os("PERIPHERAL_DESK_COLOR_FIXTURE") {
            std::fs::write(path, serde_json::to_vec_pretty(&bytes).unwrap()).unwrap();
        }
        assert!(!bytes.is_empty(), "empty color configuration");
    }
    #[test]
    fn g75_report_percent_is_raw_integer_and_invalid_placeholders_are_rejected() {
        let mut p = [0u8; 20];
        p[..7].copy_from_slice(&[0x13, 0x4a, 1, 0, 2, 0x24, 1]);
        p[19] = 0x85;
        assert!(validate(&p).is_ok());
        assert_eq!(parse_g75_battery(&[p[5], p[6]]).unwrap(), (36, Some(false)));
        assert_eq!(parse_g75_battery(&[100, 0x10]).unwrap(), (100, Some(true)));
        for invalid in [0, 1, 101, 255] {
            assert!(parse_g75_battery(&[invalid, 0]).is_err());
        }
        assert!(parse_g75_battery(&[36]).is_err());
    }
}
