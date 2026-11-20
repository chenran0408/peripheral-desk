use crate::hid::wide;
use std::{
    io,
    mem::zeroed,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

pub(super) fn open_handle(path: &str, access: u32, flags: u32) -> io::Result<OwnedHandle> {
    unsafe {
        let h = CreateFileW(
            wide(path).as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(OwnedHandle::from_raw_handle(h))
        }
    }
}

pub(super) fn event() -> io::Result<OwnedHandle> {
    unsafe {
        let h = CreateEventW(null(), 1, 0, null());
        if h.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(OwnedHandle::from_raw_handle(h))
        }
    }
}

pub(super) enum Transfer {
    Input,
    Output,
    GetFeature,
    SetFeature,
}

pub(super) fn transfer(
    h: &OwnedHandle,
    stop: &OwnedHandle,
    op: Transfer,
    wire: &mut [u8],
) -> io::Result<usize> {
    // WDK hidclass.h: HID_IN_CTL_CODE(100) and HID_OUT_CTL_CODE(100).
    const SET_FEATURE: u32 = (0x0b << 16) | (100 << 2) | 1;
    const GET_FEATURE: u32 = (0x0b << 16) | (100 << 2) | 2;
    let event = event()?;
    let h = h.as_raw_handle();
    let len = wire.len() as u32;
    unsafe {
        if WaitForSingleObject(stop.as_raw_handle(), 0) == WAIT_OBJECT_0 {
            return Err(io::Error::from_raw_os_error(ERROR_OPERATION_ABORTED as i32));
        }
        let mut ov: OVERLAPPED = zeroed();
        ov.hEvent = event.as_raw_handle();
        let mut count = 0;
        let started = match op {
            Transfer::Input => ReadFile(h, wire.as_mut_ptr(), len, &mut count, &mut ov),
            Transfer::Output => WriteFile(h, wire.as_ptr(), len, &mut count, &mut ov),
            Transfer::GetFeature => DeviceIoControl(
                h,
                GET_FEATURE,
                null(),
                0,
                wire.as_mut_ptr().cast(),
                len,
                &mut count,
                &mut ov,
            ),
            Transfer::SetFeature => DeviceIoControl(
                h,
                SET_FEATURE,
                wire.as_ptr().cast(),
                len,
                null_mut(),
                0,
                &mut count,
                &mut ov,
            ),
        };
        if started == 0 {
            if GetLastError() != ERROR_IO_PENDING {
                return Err(io::Error::last_os_error());
            }
            let timeout = if matches!(op, Transfer::Input) {
                INFINITE
            } else {
                3000
            };
            let events = [stop.as_raw_handle(), ov.hEvent];
            let wait = WaitForMultipleObjects(2, events.as_ptr(), 0, timeout);
            if wait != WAIT_OBJECT_0 + 1 {
                let error = match wait {
                    WAIT_OBJECT_0 => io::Error::from_raw_os_error(ERROR_OPERATION_ABORTED as i32),
                    WAIT_TIMEOUT => io::Error::new(io::ErrorKind::TimedOut, "设备报告超时"),
                    _ => io::Error::last_os_error(),
                };
                // The kernel owns the buffer and OVERLAPPED until cancellation completes.
                CancelIoEx(h, &ov);
                GetOverlappedResult(h, &ov, &mut count, 1);
                return Err(error);
            }
        }
        if GetOverlappedResult(h, &ov, &mut count, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(count as usize)
    }
}
