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
    pub fn open(path:&str)->Result<Self> {unsafe {
        let h=CreateFileW(wide(path).as_ptr(),GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,
            null(),OPEN_EXISTING,FILE_FLAG_OVERLAPPED,null_mut());
        if h==INVALID_HANDLE_VALUE {Err(std::io::Error::last_os_error().to_string())}
        else {Ok(Self(Handle(h)))}
    }}
    pub fn wait(&self,command:u8)->Result<()> {
        let deadline=Instant::now()+Duration::from_millis(1200);
        while Instant::now()<deadline {
            let mut data=[0u8;65];
            if Keyboard::transfer(&self.0,&mut data,false)?==65 && data[0]==0x13
                && data[1]==!0xe5 && data[2]==!command {return Ok(());}
        }
        Err(format!("等待迈从命令 0x{command:02X} 完成通知超时"))
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
            Ok(Self { reader, writer })
        }
    }
    fn transfer(handle: &Handle, data: &mut [u8], write: bool) -> Result<usize> {
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
                if WaitForSingleObject(event.0, 300) != WAIT_OBJECT_0 {
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
    fn config(&self) -> Result<Vec<u8>> {
        let b = self.read_block(0x44)?;
        if b.len() != 128 || b[126..] != [0x5a, 0xa5] || !(1..=3).contains(&b[1]) {
            return Err("G75 Pro 配置结构不匹配，已禁止写入".into());
        }
        Ok(b)
    }
    pub fn read(&self, identity: String) -> Result<State> {
        let b = self.read_block(0x4a)?;
        if b.len() != 2 || b[0] > 100 {
            return Err("键盘电量字段异常".into());
        }
        let mut s = State {
            present: true,
            fresh: true,
            percent: Some(b[0]),
            charging: match b[1] >> 4 {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            updated: now(),
            identity,
            ..State::default()
        };
        match self.config() {
            Ok(c) => {
                s.rate = Some([250, 500, 1000][c[1] as usize - 1]);
                s.sleep_seconds = Some(c[24] as u16 * 30);
                s.light = Some(c[10]);
            }
            Err(e) => s.settings_error = e,
        }
        Ok(s)
    }
    pub fn apply(&self, action: Action) -> Result<()> {
        let mut config = self.config()?;
        match action {
            Action::Rate(v) if [250, 500, 1000].contains(&v) => {
                config[1] = [250, 500, 1000].iter().position(|x| *x == v).unwrap() as u8 + 1
            }
            Action::Sleep(v) if (60..=1200).contains(&v) && v % 30 == 0 => {
                config[24] = (v / 30) as u8
            }
            Action::Light(v)
                if [0, 1, 2, 3, 4, 5, 7, 8, 10, 11, 12, 13, 15, 16, 17].contains(&v) =>
            {
                config[9] = 0;
                config[10] = v;
            }
            _ => return Err("G75 Pro 参数不在允许范围内".into()),
        }
        for (index, block) in config.chunks(14).enumerate() {
            let mut p = [0; 20];
            p[..5].copy_from_slice(&[0x13, 4, 10, index as u8, block.len() as u8]);
            p[5..5 + block.len()].copy_from_slice(block);
            let mut ack = false;
            for _ in 0..3 {
            self.send(&mut p)?;
            let deadline = Instant::now() + Duration::from_millis(900);
            while Instant::now() < deadline {
                let Some(r) = self.frame()? else {
                    continue;
                };
                if r == p {
                    ack = true;
                    break;
                }
            }
            if ack {break;}
            }
            if !ack {
                return Err("键盘未确认配置分包；请刷新核对当前设置".into());
            }
        }
        std::thread::sleep(Duration::from_millis(150));
        if self.config()? != config {
            return Err("键盘配置读回与写入不一致，请刷新核对".into());
        }
        Ok(())
    }
}
fn validate(p: &[u8; 20]) -> Result<()> {
    if p[0] != 0x13 || p[19] != p[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b)) {
        return Err("键盘报文 Report ID 或校验和错误".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_g75_full_battery_fixture() {
        let mut p = [0u8; 20];
        p[..7].copy_from_slice(&[0x13, 0x4a, 1, 0, 2, 100, 1]);
        p[19] = 0xc5;
        assert!(validate(&p).is_ok());
        p[5] = 99;
        assert!(validate(&p).is_err());
    }
}
