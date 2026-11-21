use super::layout::{StoreLayout, ROW_HEIGHT};
use super::*;

impl Manager {
    unsafe fn geometry(&self, hwnd: HWND) -> StoreLayout {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client);
        let dip = |pixels| (pixels * 96 + self.dpi as i32 / 2) / self.dpi as i32;
        StoreLayout::new(
            dip(client.right),
            dip(client.bottom),
            !self.status.is_empty(),
        )
    }

    pub(super) unsafe fn reflow(&self, hwnd: HWND) {
        let ui = self.geometry(hwnd);
        for child in &self.children {
            let r = ui.control(child.id);
            SetWindowPos(
                child.hwnd,
                null_mut(),
                self.px(r.x),
                self.px(r.y),
                self.px(r.w.max(1)),
                self.px(r.h.max(1)),
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
        let status = self.child(STATUS);
        if !status.is_null() {
            ShowWindow(
                status,
                if self.status.is_empty() {
                    SW_HIDE
                } else {
                    SW_SHOWNA
                },
            );
            fit_text_scrollbar(status);
        }
        fit_text_scrollbar(self.child(DESCRIPTION));
        let list = self.child(LIST);
        if !list.is_null()
            && SendMessageW(list, LB_GETITEMHEIGHT, 0, 0) != self.px(ROW_HEIGHT) as isize
        {
            SendMessageW(list, LB_SETITEMHEIGHT, 0, self.px(ROW_HEIGHT) as isize);
        }
        if !list.is_null() {
            InvalidateRect(list, null(), 0);
        }
        InvalidateRect(hwnd, null(), 0);
    }

    pub(super) unsafe fn layout(&mut self, hwnd: HWND) {
        if self.children.is_empty() {
            let ui = self.geometry(hwnd);
            let button = BS_OWNERDRAW as u32;
            let read_only = (ES_MULTILINE | ES_READONLY) as u32 | WS_VSCROLL;
            for (id, class, label, style) in [
                (CHANNEL, "BUTTON", "稳定版", button),
                (PREVIEW, "BUTTON", "预览版", button),
                (OFFLINE, "BUTTON", "离线安装", button),
                (REFRESH, "BUTTON", "检查更新", button),
                (BRAND_LABEL, "STATIC", "厂商(&B)", 0),
                (BRAND, "COMBOBOX", "", CBS_DROPDOWNLIST as u32 | WS_VSCROLL),
                (SEARCH_LABEL, "STATIC", "搜索(&F)", 0),
                (SEARCH, "EDIT", "", ES_AUTOHSCROLL as u32),
                (SELECT_ALL, "BUTTON", "全选", button),
                (SELECT_DETECTED, "BUTTON", "选已识别", button),
                (SELECT_UPDATES, "BUTTON", "选可更新", button),
                (SELECT_NONE, "BUTTON", "清空", button),
                (
                    LIST,
                    "LISTBOX",
                    "",
                    (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_MULTIPLESEL) as u32 | WS_VSCROLL,
                ),
                (DESCRIPTION, "EDIT", "", read_only),
                (CHANGELOG, "BUTTON", "更新说明", button),
                (ROLLBACK, "BUTTON", "回退", button),
                (REMOVE, "BUTTON", "卸载", button),
                (RESTART_BUTTON, "BUTTON", "重启生效", button),
                (INSTALL, "BUTTON", "安装所选插件", button),
                (STATUS, "EDIT", "", read_only),
            ] {
                self.add(hwnd, id, class, label, ui.control(id), style);
            }
            for id in [SEARCH, DESCRIPTION, STATUS] {
                SendMessageW(
                    self.child(id),
                    EM_SETMARGINS,
                    (EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize,
                    (self.px(7) * 0x10001) as isize,
                );
            }
            SendMessageW(
                self.child(SEARCH),
                EM_SETCUEBANNER,
                1,
                wide("型号、厂商或协议").as_ptr() as isize,
            );
            if !self.browser.query.is_empty() {
                self.text(SEARCH, &self.browser.query.clone());
            }
            for child in &self.children {
                if child.id != RESTART_BUTTON {
                    ShowWindow(child.hwnd, SW_SHOWNA);
                }
            }
        }
        self.browser.refresh_brands(self.inventory.as_ref());
        let brands = self.child(BRAND);
        SendMessageW(brands, CB_RESETCONTENT, 0, 0);
        SendMessageW(brands, CB_ADDSTRING, 0, wide("全部厂商").as_ptr() as isize);
        for name in &self.browser.brands {
            SendMessageW(brands, CB_ADDSTRING, 0, wide(name).as_ptr() as isize);
        }
        let choice = self
            .browser
            .brands
            .iter()
            .position(|b| Some(b) == self.browser.brand.as_ref())
            .map_or(0, |n| n + 1);
        SendMessageW(brands, CB_SETCURSEL, choice, 0);
        self.text(STATUS, &self.status);
        self.populate_list();
        self.selection(hwnd);
        self.reflow(hwnd);
    }

    pub(super) unsafe fn paint(&self, hwnd: HWND) {
        let mut ps = zeroed();
        let dest = BeginPaint(hwnd, &mut ps);
        let mut area = RECT::default();
        GetClientRect(hwnd, &mut area);
        if area.right <= 0 || area.bottom <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }
        let dc = CreateCompatibleDC(dest);
        let bitmap = CreateCompatibleBitmap(dest, area.right, area.bottom);
        let old = SelectObject(dc, bitmap);
        SetDCBrushColor(dc, colorref(palette().bg));
        FillRect(dc, &area, GetStockObject(DC_BRUSH));
        {
            let c = Canvas::new(dc, self.dpi);
            let ui = self.geometry(hwnd);
            let p = palette();
            c.text("更新通道", R::new(2, 4, 65, 32), 13, 500, p.muted, DT_LEFT);
            for card in [ui.list, ui.detail, ui.footer] {
                c.round(card, RADIUS_LG, p.panel, None);
            }
            let results = if self.inventory.is_some() {
                format!("设备插件   {}", self.browser.visible.len())
            } else {
                "设备插件".into()
            };
            c.text(&results, ui.results(), 16, 600, p.text, DT_LEFT);
            let edit = ui.control(SEARCH);
            c.round(
                R::new(edit.x - 1, edit.y - 3, edit.w + 2, edit.h + 6),
                6,
                p.field,
                Some(if GetFocus() == self.child(SEARCH) {
                    p.accent
                } else {
                    p.border
                }),
            );
            if let Some(row) = self.selected() {
                let flag = match row.recommendation {
                    Some(Recommendation::Clear) => Some(("已识别", p.green)),
                    Some(Recommendation::Ambiguous) => Some(("需核对型号", p.amber)),
                    None if !row.catalog_available => Some(("仅本机安装", p.muted)),
                    None => None,
                };
                let mut brand = ui.brand();
                if let Some((label, color)) = flag {
                    c.text(label, brand, 12, 500, color, DT_RIGHT);
                    brand.w -= 94;
                }
                c.text(
                    row.brand(),
                    brand,
                    12,
                    500,
                    p.muted,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                c.text(
                    row.name,
                    ui.name(),
                    19,
                    600,
                    p.text,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                let version = if row.catalog_available {
                    format!("{}  ·  {}", row.version, size(row.download_bytes))
                } else {
                    self.installation().label().to_owned()
                };
                c.text(
                    &version,
                    ui.version(),
                    12,
                    400,
                    p.muted,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
            } else {
                c.text("型号详情", ui.name(), 18, 600, p.text, DT_LEFT);
            }
            let detail = ui.control(DESCRIPTION);
            c.round(
                R::new(detail.x - 5, detail.y - 4, detail.w + 10, detail.h + 8),
                RADIUS_MD,
                p.field,
                None,
            );

            let targets = self.targets();
            let visible = self.inventory.as_ref().map_or(0, |inventory| {
                self.browser
                    .visible
                    .iter()
                    .filter(|&&i| {
                        inventory
                            .row(i)
                            .is_some_and(|r| self.checked.contains(r.id))
                    })
                    .count()
            });
            let hidden = targets.len().saturating_sub(visible);
            let summary = if targets.is_empty() {
                "勾选型号，可一次安装多个插件".to_owned()
            } else {
                let bytes = size(targets.iter().map(|t| t.bytes).sum());
                if hidden == 0 {
                    format!("已选 {} 项   ·   下载 {bytes}", targets.len())
                } else {
                    format!(
                        "已选 {} 项（筛选外 {hidden} 项）  ·  {bytes}",
                        targets.len()
                    )
                }
            };
            c.text(
                &summary,
                ui.summary(self.changed),
                13,
                500,
                p.text,
                DT_LEFT | DT_END_ELLIPSIS,
            );
            if !self.status.is_empty() {
                let status = ui.control(STATUS);
                c.round(
                    R::new(status.x - 6, status.y - 2, status.w + 12, status.h + 4),
                    6,
                    p.field,
                    Some(if self.error { p.error } else { p.border }),
                );
            }
        }
        BitBlt(dest, 0, 0, area.right, area.bottom, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
}

// Draw the closed face within the original native combo box. Its drop-down
// list, keyboard search and accessibility continue to use the Windows control.
pub(super) unsafe extern "system" fn brand_proc(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(brand_proc), 1);
            DefSubclassProc(hwnd, msg, w, l)
        }
        WM_PAINT | WM_PRINTCLIENT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = if msg == WM_PAINT {
                BeginPaint(hwnd, &mut ps)
            } else {
                w as HDC
            };
            if !dc.is_null() {
                let dpi = GetDpiForWindow(hwnd).max(96);
                let mut client = RECT::default();
                GetClientRect(hwnd, &mut client);
                let width = client.right * 96 / dpi as i32;
                let height = client.bottom * 96 / dpi as i32;
                let p = palette();
                SetDCBrushColor(dc, colorref(p.panel));
                FillRect(dc, &client, GetStockObject(DC_BRUSH));
                let c = Canvas::new(dc, dpi);
                let open = SendMessageW(hwnd, CB_GETDROPPEDSTATE, 0, 0) != 0;
                let enabled = IsWindowEnabled(hwnd) != 0;
                let stroke = if GetFocus() == hwnd || open {
                    p.accent
                } else {
                    p.border
                };
                c.round(
                    R::new(0, 0, width - 1, height - 1),
                    6,
                    p.field,
                    Some(stroke),
                );
                let label = crate::configuration_ui::text(hwnd);
                let color = if enabled { p.text } else { p.dim };
                c.text(
                    &label,
                    R::new(9, 0, width - 37, height),
                    14,
                    400,
                    color,
                    DT_LEFT | DT_END_ELLIPSIS,
                );
                let x = width - 16;
                let y = height / 2;
                let dy = if open { -2 } else { 2 };
                c.line(x - 4, y - dy, x, y + dy, p.muted, 1.4);
                c.line(x, y + dy, x + 4, y - dy, p.muted, 1.4);
            }
            if msg == WM_PAINT {
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS | CB_SHOWDROPDOWN | CB_SETCURSEL => {
            let result = DefSubclassProc(hwnd, msg, w, l);
            InvalidateRect(hwnd, null(), 0);
            result
        }
        _ => DefSubclassProc(hwnd, msg, w, l),
    }
}

pub(super) unsafe fn fit_text_scrollbar(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    let mut rect = RECT::default();
    SendMessageW(hwnd, EM_GETRECT, 0, (&mut rect as *mut RECT) as isize);
    let dc = GetDC(hwnd);
    let font = SendMessageW(hwnd, WM_GETFONT, 0, 0) as HFONT;
    let old = SelectObject(dc, font);
    let mut metrics = TEXTMETRICW::default();
    GetTextMetricsW(dc, &mut metrics);
    SelectObject(dc, old);
    ReleaseDC(hwnd, dc);
    let lines = SendMessageW(hwnd, EM_GETLINECOUNT, 0, 0) as i32;
    let visible = (rect.bottom - rect.top) / metrics.tmHeight.max(1);
    ShowScrollBar(hwnd, SB_VERT, i32::from(lines > visible && lines > 1));
}
