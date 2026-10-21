//! Native GDI+ surfaces with ClearType GDI text, images and vector controls.
use crate::hid::wide;
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{Gdi::*, GdiPlus::*},
    UI::{Controls::*, WindowsAndMessaging::*},
};

pub const BG: u32 = 0xff0a0a0d;
pub const PANEL: u32 = 0xff121218;
pub const FIELD: u32 = 0xff1e1e26;
pub const BORDER: u32 = 0xff292930;
pub const TEXT: u32 = 0xfff2f2f5;
pub const MUTED: u32 = 0xffa1a1ae;
pub const DIM: u32 = 0xff737382;
pub const RED: u32 = 0xffff3859;
pub const ORANGE: u32 = 0xffff7542;
pub const GREEN: u32 = 0xff38d58a;
pub const AMBER: u32 = 0xffdcae65;

/// Retain native string storage, selection and accessibility with custom colors.
pub fn native_control_style(class: &str, style: u32) -> u32 {
    let style = style & !WS_BORDER;
    match class {
        "LISTBOX" => style | LBS_OWNERDRAWFIXED as u32 | LBS_HASSTRINGS as u32,
        "COMBOBOX" => style | CBS_OWNERDRAWFIXED as u32 | CBS_HASSTRINGS as u32,
        _ => style,
    }
}
pub unsafe fn theme_native_control(hwnd: HWND, class: &str, dpi: u32) {
    let theme = if class == "COMBOBOX" {
        "DarkMode_CFD"
    } else {
        "DarkMode_Explorer"
    };
    SetWindowTheme(hwnd, wide(theme).as_ptr(), null());
    let height = (28 * dpi.max(96) / 96) as isize;
    match class {
        "LISTBOX" => {
            SendMessageW(hwnd, LB_SETITEMHEIGHT, 0, height);
        }
        "COMBOBOX" => {
            SendMessageW(hwnd, CB_SETITEMHEIGHT, usize::MAX, height);
            SendMessageW(hwnd, CB_SETITEMHEIGHT, 0, height);
        }
        _ => {}
    }
}
pub unsafe fn measure_native_choice(item: &mut MEASUREITEMSTRUCT, dpi: u32) -> bool {
    if !matches!(item.CtlType, ODT_LISTBOX | ODT_COMBOBOX) {
        return false;
    }
    item.itemHeight = 28 * dpi.max(96) / 96;
    true
}
pub unsafe fn draw_native_choice(item: &DRAWITEMSTRUCT, dpi: u32, font: HFONT) -> bool {
    if !matches!(item.CtlType, ODT_LISTBOX | ODT_COMBOBOX) {
        return false;
    }
    let combo = item.CtlType == ODT_COMBOBOX;
    let selected = item.itemState & ODS_SELECTED != 0 && item.itemState & ODS_COMBOBOXEDIT == 0;
    let colorref = |argb: u32| ((argb >> 16) & 255) | (argb & 0xff00) | ((argb & 255) << 16);
    let background = CreateSolidBrush(colorref(if selected { BORDER } else { FIELD }));
    FillRect(item.hDC, &item.rcItem, background);
    DeleteObject(background);
    if item.itemID == u32::MAX {
        return true;
    }
    let len = SendMessageW(
        item.hwndItem,
        if combo {
            CB_GETLBTEXTLEN
        } else {
            LB_GETTEXTLEN
        },
        item.itemID as usize,
        0,
    );
    if len < 0 {
        return true;
    }
    let mut text = vec![0u16; len as usize + 1];
    SendMessageW(
        item.hwndItem,
        if combo { CB_GETLBTEXT } else { LB_GETTEXT },
        item.itemID as usize,
        text.as_mut_ptr() as isize,
    );
    let inset = (10 * dpi.max(96) / 96) as i32;
    if selected {
        let accent = CreateSolidBrush(colorref(RED));
        let stroke = (3 * dpi.max(96) / 96) as i32;
        let stripe = RECT {
            left: item.rcItem.left,
            top: item.rcItem.top + stroke,
            right: item.rcItem.left + stroke,
            bottom: item.rcItem.bottom - stroke,
        };
        FillRect(item.hDC, &stripe, accent);
        DeleteObject(accent);
    }
    let previous_font = SelectObject(item.hDC, font);
    let previous_color = SetTextColor(
        item.hDC,
        colorref(if item.itemState & ODS_DISABLED != 0 {
            DIM
        } else {
            TEXT
        }),
    );
    let previous_mode = SetBkMode(item.hDC, TRANSPARENT as i32);
    let mut rect = item.rcItem;
    rect.left += inset;
    rect.right -= inset;
    // Native scrolling/clip regions retain the full string; never truncate UTF-16
    // or hide parameter suffixes with a manually shortened label.
    DrawTextW(
        item.hDC,
        text.as_ptr(),
        len as i32,
        &mut rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    SetBkMode(item.hDC, previous_mode);
    SetTextColor(item.hDC, previous_color);
    SelectObject(item.hDC, previous_font);
    true
}

#[derive(Clone, Copy, Debug, Default)]
pub struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
impl R {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

pub struct Canvas {
    pub graphics: *mut GpGraphics,
    dpi: u32,
}
impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.graphics);
        }
    }
}
impl Canvas {
    pub unsafe fn new(dc: HDC, dpi: u32) -> Self {
        let mut graphics = null_mut();
        GdipCreateFromHDC(dc, &mut graphics);
        GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias8x8);
        GdipSetTextRenderingHint(graphics, TextRenderingHintClearTypeGridFit);
        GdipSetInterpolationMode(graphics, InterpolationModeHighQualityBicubic);
        let scale = dpi as f32 / 96.;
        GdipScaleWorldTransform(graphics, scale, scale, MatrixOrderAppend);
        SetBkMode(dc, TRANSPARENT as i32);
        Self { graphics, dpi }
    }
    pub unsafe fn rect(&self, r: R, color: u32) {
        let mut b = null_mut();
        GdipCreateSolidFill(color, &mut b);
        GdipFillRectangleI(self.graphics, b as _, r.x, r.y, r.w, r.h);
        GdipDeleteBrush(b as _);
    }
    pub unsafe fn round(&self, r: R, radius: i32, color: u32, border: Option<u32>) {
        let mut path = null_mut();
        GdipCreatePath(FillModeAlternate, &mut path);
        let d = radius * 2;
        GdipAddPathArcI(path, r.x, r.y, d, d, 180., 90.);
        GdipAddPathArcI(path, r.x + r.w - d, r.y, d, d, 270., 90.);
        GdipAddPathArcI(path, r.x + r.w - d, r.y + r.h - d, d, d, 0., 90.);
        GdipAddPathArcI(path, r.x, r.y + r.h - d, d, d, 90., 90.);
        GdipClosePathFigure(path);
        if color == RED {
            let mut b = null_mut();
            let bounds = Rect {
                X: r.x,
                Y: r.y,
                Width: r.w,
                Height: r.h,
            };
            GdipCreateLineBrushFromRectI(
                &bounds,
                RED,
                ORANGE,
                LinearGradientModeHorizontal,
                WrapModeTileFlipXY,
                &mut b,
            );
            GdipFillPath(self.graphics, b as _, path);
            GdipDeleteBrush(b as _);
        } else {
            let mut b = null_mut();
            GdipCreateSolidFill(color, &mut b);
            GdipFillPath(self.graphics, b as _, path);
            GdipDeleteBrush(b as _);
        }
        if let Some(color) = border {
            let mut p = null_mut();
            GdipCreatePen1(color, 1., UnitPixel, &mut p);
            GdipDrawPath(self.graphics, p, path);
            GdipDeletePen(p);
        }
        GdipDeletePath(path);
    }
    pub unsafe fn gradient(&self, r: R, first: u32, last: u32) {
        let mut b = null_mut();
        let gr = Rect {
            X: r.x,
            Y: r.y,
            Width: r.w,
            Height: r.h,
        };
        GdipCreateLineBrushFromRectI(
            &gr,
            first,
            last,
            LinearGradientModeHorizontal,
            WrapModeTileFlipXY,
            &mut b,
        );
        GdipFillRectangleI(self.graphics, b as _, r.x, r.y, r.w, r.h);
        GdipDeleteBrush(b as _);
    }
    pub unsafe fn ellipse(&self, r: R, color: u32) {
        let mut b = null_mut();
        GdipCreateSolidFill(color, &mut b);
        GdipFillEllipseI(self.graphics, b as _, r.x, r.y, r.w, r.h);
        GdipDeleteBrush(b as _);
    }
    pub unsafe fn line(&self, x1: i32, y1: i32, x2: i32, y2: i32, color: u32, width: f32) {
        let mut p = null_mut();
        GdipCreatePen1(color, width, UnitPixel, &mut p);
        GdipSetPenLineCap197819(p, LineCapRound, LineCapRound, DashCapRound);
        GdipDrawLineI(self.graphics, p, x1, y1, x2, y2);
        GdipDeletePen(p);
    }
    pub unsafe fn arc(&self, r: R, start: f32, sweep: f32, color: u32, width: f32) {
        let mut p = null_mut();
        GdipCreatePen1(color, width, UnitPixel, &mut p);
        GdipSetPenLineCap197819(p, LineCapRound, LineCapRound, DashCapRound);
        GdipDrawArcI(self.graphics, p, r.x, r.y, r.w, r.h, start, sweep);
        GdipDeletePen(p);
    }
    pub unsafe fn text(&self, text: &str, r: R, size: i32, weight: i32, color: u32, align: u32) {
        let mut dc = null_mut();
        if GdipGetDC(self.graphics, &mut dc) != 0 {
            return;
        }
        // GDI+ world transforms do not apply while drawing through its HDC.
        let scale = |value: i32| (value as f64 * self.dpi as f64 / 96.).round() as i32;
        let name = wide(if text.is_ascii() {
            "Segoe UI"
        } else {
            "Microsoft YaHei UI"
        });
        let font = CreateFontW(
            -scale(size).max(1),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            name.as_ptr(),
        );
        if !font.is_null() {
            let previous_font = SelectObject(dc, font);
            let previous_mode = SetBkMode(dc, TRANSPARENT as i32);
            // GDI accepts COLORREF (00BBGGRR), whereas the canvas uses ARGB.
            let rgb = ((color >> 16) & 0xff) | (color & 0xff00) | ((color & 0xff) << 16);
            let previous_color = SetTextColor(dc, rgb);
            let mut area = RECT {
                left: scale(r.x),
                top: scale(r.y),
                right: scale(r.x + r.w),
                bottom: scale(r.y + r.h),
            };
            let text = wide(text);
            DrawTextW(
                dc,
                text.as_ptr(),
                (text.len() - 1) as i32,
                &mut area,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align,
            );
            SetTextColor(dc, previous_color);
            SetBkMode(dc, previous_mode);
            SelectObject(dc, previous_font);
            DeleteObject(font);
        }
        GdipReleaseDC(self.graphics, dc);
    }
    pub unsafe fn glow(&self, cx: i32, cy: i32, radius: i32) {
        for n in (1..=24).rev() {
            let r = radius * n / 24;
            self.ellipse(R::new(cx - r, cy - r, r * 2, r * 2), 0x04ff3859);
        }
    }
    pub unsafe fn image(&self, image: *mut GpImage, r: R, active: bool) {
        if image.is_null() {
            return;
        }
        let mut w = 0;
        let mut h = 0;
        GdipGetImageWidth(image, &mut w);
        GdipGetImageHeight(image, &mut h);
        if w == 0 || h == 0 {
            return;
        }
        let ratio = (r.w as f32 / w as f32).min(r.h as f32 / h as f32);
        let dw = (w as f32 * ratio) as i32;
        let dh = (h as f32 * ratio) as i32;
        let mut attr = null_mut();
        GdipCreateImageAttributes(&mut attr);
        let alpha = if active { 1. } else { 0.24 };
        let mut matrix: ColorMatrix = zeroed();
        for n in 0..5 {
            matrix.m[n * 5 + n] = 1.;
        }
        matrix.m[18] = alpha;
        GdipSetImageAttributesColorMatrix(
            attr,
            ColorAdjustTypeBitmap,
            1,
            &matrix,
            null(),
            ColorMatrixFlagsDefault,
        );
        GdipDrawImageRectRectI(
            self.graphics,
            image,
            r.x + (r.w - dw) / 2,
            r.y + (r.h - dh) / 2,
            dw,
            dh,
            0,
            0,
            w as i32,
            h as i32,
            UnitPixel,
            attr,
            0,
            null_mut(),
        );
        GdipDisposeImageAttributes(attr);
    }
    pub unsafe fn ring(&self, r: R, percent: Option<u8>, active: bool) {
        self.arc(r, -90., 359.9, BORDER, 9.);
        if let Some(p) = percent {
            let sweep = 360. * p as f32 / 100.;
            for n in 0..60 {
                let at = n as f32 * 6.;
                if at >= sweep {
                    break;
                }
                let t = n as f32 / 60.;
                let color = blend(RED, ORANGE, t);
                self.arc(
                    r,
                    -90. + at,
                    (sweep - at).min(6.),
                    if active { color } else { DIM },
                    9.,
                );
            }
        }
        self.text(
            &percent.map(|p| p.to_string()).unwrap_or_else(|| "—".into()),
            R::new(r.x, r.y + r.h / 2 - 36, r.w, 52),
            38,
            300,
            if active { TEXT } else { DIM },
            DT_CENTER,
        );
        self.text(
            "电量 %",
            R::new(r.x, r.y + r.h / 2 + 12, r.w, 24),
            12,
            400,
            DIM,
            DT_CENTER,
        );
    }
}
fn blend(a: u32, b: u32, t: f32) -> u32 {
    let mut c = 0xff000000;
    for s in [0, 8, 16] {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        c |= ((x + (y - x) * t) as u32) << s;
    }
    c
}

pub struct Images {
    pub images: Vec<*mut GpImage>,
    token: usize,
}
impl Images {
    pub unsafe fn load() -> Self {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..zeroed()
        };
        GdiplusStartup(&mut token, &input, null_mut());
        let mut images = Vec::new();
        for spec in crate::devices::REGISTRY.iter() {
            let mut image = null_mut();
            if let Some(path) = crate::plugin_manager::artwork(spec.key) {
                let mut source = null_mut();
                if GdipLoadImageFromFile(wide(&path.to_string_lossy()).as_ptr(), &mut source) == 0 {
                    let mut width = 0;
                    let mut height = 0;
                    GdipGetImageWidth(source, &mut width);
                    GdipGetImageHeight(source, &mut height);
                    let mut bitmap = null_mut();
                    if GdipCloneBitmapAreaI(
                        0,
                        0,
                        width as i32,
                        height as i32,
                        0x26200A, /* PixelFormat32bppARGB */
                        source.cast(),
                        &mut bitmap,
                    ) == 0
                    {
                        image = bitmap.cast();
                    }
                    GdipDisposeImage(source);
                }
            }
            images.push(image);
        }
        Self { images, token }
    }
}
impl Drop for Images {
    fn drop(&mut self) {
        unsafe {
            for p in &self.images {
                if !p.is_null() {
                    GdipDisposeImage(*p);
                }
            }
            GdiplusShutdown(self.token);
        }
    }
}

pub unsafe fn tray_icon() -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let dc = CreateCompatibleDC(null_mut());
    let mut info: BITMAPINFO = zeroed();
    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = 32;
    info.bmiHeader.biHeight = -32;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let mut pixels = null_mut();
    let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0);
    if bitmap.is_null() {
        DeleteDC(dc);
        return CopyIcon(LoadIconW(null_mut(), IDI_APPLICATION));
    }
    let old = SelectObject(dc, bitmap);
    let c = Canvas::new(dc, 96);
    c.rect(R::new(0, 0, 32, 32), BG);
    c.round(R::new(5, 9, 20, 14), 4, RED, None);
    c.rect(R::new(25, 13, 3, 6), ORANGE);
    c.line(11, 16, 19, 16, TEXT, 2.);
    drop(c);
    GdiFlush();
    for p in std::slice::from_raw_parts_mut(pixels as *mut u32, 1024) {
        *p |= 0xff000000;
    }
    SelectObject(dc, old);
    let bits = [0u8; 128];
    let mask = CreateBitmap(32, 32, 1, 1, bits.as_ptr() as _);
    let icon = CreateIconIndirect(&ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: bitmap,
    });
    DeleteObject(mask);
    DeleteObject(bitmap);
    DeleteDC(dc);
    icon
}
