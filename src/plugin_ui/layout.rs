use super::*;

pub(super) const ROW_HEIGHT: i32 = 48;

/// All coordinates are DIPs in the area below the main window's page title.
pub(super) struct StoreLayout {
    pub width: i32,
    pub list: R,
    pub detail: R,
    pub footer: R,
    inset: i32,
}

impl StoreLayout {
    pub fn new(width: i32, height: i32, show_status: bool) -> Self {
        let inset = if width < 750 { 12 } else { 16 };
        let gutter = if width < 750 { 8 } else { 12 };
        let footer_height = if !show_status {
            48
        } else if height < 465 {
            82
        } else {
            96
        };
        let footer = R::new(0, height - footer_height, width, footer_height);
        let card_height = footer.y - 52 - gutter;
        let columns = width - gutter;
        let details_width = (columns * 39 / 100).min(440);
        let list = R::new(0, 52, columns - details_width, card_height);
        let detail = R::new(list.w + gutter, list.y, details_width, list.h);
        Self {
            width,
            list,
            detail,
            footer,
            inset,
        }
    }

    fn detail_header(&self, y: i32, height: i32) -> R {
        R::new(
            self.detail.x + self.inset,
            self.detail.y + y,
            self.detail.w - 2 * self.inset,
            height,
        )
    }

    pub fn brand(&self) -> R {
        self.detail_header(11, 20)
    }
    pub fn name(&self) -> R {
        self.detail_header(33, 28)
    }
    pub fn version(&self) -> R {
        self.detail_header(63, 20)
    }

    pub fn results(&self) -> R {
        R::new(
            self.inset,
            self.list.y + 10,
            self.list.w - 2 * self.inset,
            23,
        )
    }

    pub fn summary(&self, changed: bool) -> R {
        let right = self
            .control(if changed { RESTART_BUTTON } else { INSTALL })
            .x;
        R::new(self.inset, self.footer.y + 8, right - self.inset - 8, 32)
    }

    pub fn control(&self, id: i32) -> R {
        let pad = self.inset;
        let inner = self.list.w - pad * 2;
        let brand_width = (inner / 3).clamp(90, 208);
        let field_y = self.list.y + 59;
        let bulk_y = field_y + 42;
        let action_y = self.detail.y + self.detail.h - 46;
        match id {
            CHANNEL => R::new(72, 4, 66, 32),
            PREVIEW => R::new(141, 4, 66, 32),
            OFFLINE => R::new(self.width - 180, 4, 86, 32),
            REFRESH => R::new(self.width - 88, 4, 88, 32),
            BRAND_LABEL => R::new(pad, field_y - 19, brand_width, 18),
            SEARCH_LABEL => R::new(
                pad + brand_width + 10,
                field_y - 19,
                inner - brand_width - 10,
                18,
            ),
            BRAND => R::new(pad, field_y, brand_width, 300),
            SEARCH => R::new(
                pad + brand_width + 10,
                field_y + 3,
                inner - brand_width - 10,
                26,
            ),
            SELECT_ALL | SELECT_DETECTED | SELECT_UPDATES | SELECT_NONE => {
                let order = match id {
                    SELECT_ALL => 0,
                    SELECT_DETECTED => 1,
                    SELECT_UPDATES => 2,
                    _ => 3,
                };
                let widths = [62, 82, 82, 56];
                let unit = (inner - 3 * 5).min(widths.iter().sum::<i32>());
                let start = widths[..order].iter().sum::<i32>() * unit / 282;
                let end = widths[..=order].iter().sum::<i32>() * unit / 282;
                R::new(pad + start + order as i32 * 5, bulk_y, end - start, 28)
            }
            LIST => {
                let y = bulk_y + 36;
                R::new(
                    pad - 3,
                    y,
                    inner + 6,
                    (self.list.y + self.list.h - pad - y).max(1),
                )
            }
            DESCRIPTION => {
                let r = self.detail_header(94, 0);
                R::new(r.x + 5, r.y + 3, r.w - 10, (action_y - r.y - 14).max(1))
            }
            CHANGELOG | ROLLBACK | REMOVE => {
                let detail = self.detail;
                let inner = detail.w - pad * 2;
                let small = ((inner - 12) / 3).min(84);
                let first = (inner - 12 - small * 2).min(114);
                let (x, w) = match id {
                    CHANGELOG => (0, first),
                    ROLLBACK => (first + 6, small),
                    _ => (first + small + 12, small),
                };
                R::new(detail.x + pad + x, action_y, w, 30)
            }
            INSTALL => R::new(self.width - pad - 170, self.footer.y + 7, 170, 34),
            RESTART_BUTTON => R::new(self.width - pad - 258, self.footer.y + 7, 80, 34),
            STATUS => R::new(
                pad + 6,
                self.footer.y + 48,
                self.width - 2 * pad - 12,
                self.footer.h - 54,
            ),
            _ => unreachable!("no geometry for store control {id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_content_fits_compact_and_maximized_native_pages() {
        for (w, h) in [(616, 405), (784, 520), (1032, 620), (1600, 920)] {
            let ui = StoreLayout::new(w, h, true);
            let list = ui.control(LIST);
            let description = ui.control(DESCRIPTION);
            assert!(list.h >= ROW_HEIGHT && description.h >= 50);
            assert!(list.x + list.w <= ui.detail.x);
            assert!(description.y + description.h < ui.control(CHANGELOG).y);
            assert!(ui.control(REMOVE).x + ui.control(REMOVE).w <= w);
            assert!(ui.control(SELECT_NONE).x + ui.control(SELECT_NONE).w <= ui.list.w);
            assert!(ui.summary(true).w > 200);
            assert!(ui.control(STATUS).y + ui.control(STATUS).h <= h);
        }
    }
}
