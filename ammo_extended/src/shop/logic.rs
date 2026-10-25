//! Platform-independent shop entry selection, safety checks, and grid layout.

use crate::Ammo;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;

#[derive(Debug)]
pub(super) struct Entry {
    pub(super) item: CString,
    pub(super) title: String,
    pub(super) price: i32,
}

pub(super) fn entries(vanilla: &[Ammo], configured: &[Ammo]) -> Vec<Entry> {
    let vanilla: HashSet<_> = vanilla.iter().map(|a| a.item_name.get_string()).collect();
    let mut occurrences = HashMap::new();
    for ammo in configured {
        *occurrences
            .entry(ammo.item_name.get_string())
            .or_insert(0usize) += 1;
    }

    configured
        .iter()
        .enumerate()
        .filter_map(|(index, ammo)| {
            let name = ammo.item_name.get_string();
            if vanilla.contains(name) {
                return None;
            }

            // Native inventory treats index zero as undefined; lookup picks the first duplicate.
            if index == 0 || name.is_empty() || occurrences[name] != 1 || ammo.shop_price <= 0 {
                log::warn!("Not adding invalid/ambiguous custom shop ammo {name:?} (index {index}, price {})", ammo.shop_price);
                return None;
            }

            let item = match CString::new(name) {
                Ok(item) => item,
                Err(_) => {
                    log::warn!("Not adding custom shop ammo with a NUL in its identifier");
                    return None;
                }
            };

            let title = format!("{} {}", ammo.milimeterage.get_string(), ammo.display_name.get_string());
            let title = if title.trim().is_empty() { name } else { title.trim() };
            Some(Entry { item, title: plain_title(title), price: ammo.shop_price })
        })
        .collect()
}

fn plain_title(title: &str) -> String {
    // Native labels support rich-text commands. Config text here must stay literal and single-line.
    title
        .chars()
        .take(32)
        .map(|c| match c {
            '{' => '(',
            '}' => ')',
            '@' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// Keep native i32 price arithmetic and f32 slider quantities in their exact, positive range.
pub(super) fn can_offer(stock: i32, owned: i32, price: i32, cash: i32) -> bool {
    stock >= 0
        && owned >= 0
        && cash >= 0
        && price > 0
        && owned
            .checked_mul(price)
            .and_then(|proceeds| cash.checked_add(proceeds))
            .is_some()
        && stock.checked_add(owned).is_some_and(|total| {
            total > 0 && total <= 16_777_215 && total.checked_mul(price).is_some()
        })
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Grid {
    rows: usize,
    columns: usize,
    width: f32,
    height: f32,
}

impl Grid {
    pub(super) const CELL_WIDTH: f32 = 280.0;
    pub(super) const CELL_HEIGHT: f32 = 36.0;
    const MARGIN: f32 = 16.0;
    const GAP: f32 = 8.0;
    /// Layout space reserved for the native exit control, in addition to the outer margins.
    const EXIT_AREA_HEIGHT: f32 = 64.0;

    pub(super) fn new(width: i32, height: i32) -> Option<Self> {
        let width = width as f32;
        let height = height as f32;
        let usable_width = width - 2.0 * Self::MARGIN;
        let usable_height = height - 2.0 * Self::MARGIN - Self::EXIT_AREA_HEIGHT;

        if usable_width < Self::CELL_WIDTH || usable_height < Self::CELL_HEIGHT {
            return None;
        }

        let rows = ((usable_height + Self::GAP) / (Self::CELL_HEIGHT + Self::GAP)).floor() as usize;
        let columns =
            ((usable_width + Self::GAP) / (Self::CELL_WIDTH + Self::GAP)).floor() as usize;

        Some(Self {
            rows,
            columns,
            width,
            height,
        })
    }

    pub(super) fn position(self, index: usize) -> Option<(f32, f32)> {
        if index >= self.rows * self.columns {
            return None;
        }

        Some((
            -self.width * 0.5
                + Self::MARGIN
                + Self::CELL_WIDTH * 0.5
                + (index / self.rows) as f32 * (Self::CELL_WIDTH + Self::GAP),
            -self.height * 0.5
                + Self::MARGIN
                + Self::CELL_HEIGHT * 0.5
                + (index % self.rows) as f32 * (Self::CELL_HEIGHT + Self::GAP),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<Ammo> {
        let mut values: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../ammo_extended_1_163.json")).unwrap();
        for value in &mut values {
            value["padding_cch"] = 0.into();
        }
        serde_json::from_value(values.into()).unwrap()
    }

    fn custom(name: &str, price: i32) -> Ammo {
        let mut ammo = fixture().remove(1);
        ammo.item_name = name.into();
        ammo.display_name = "Custom AP".into();
        ammo.shop_price = price;
        ammo
    }

    #[test]
    fn entries_only_include_unique_custom_names_in_config_order() {
        let vanilla = fixture();
        let mut configured = fixture();
        configured.push(custom("CUSTOM_B", 50));
        configured.push(custom("CUSTOM_A", 75));
        let result = entries(&vanilla, &configured);
        assert_eq!(
            result
                .iter()
                .map(|e| e.item.to_str().unwrap())
                .collect::<Vec<_>>(),
            ["CUSTOM_B", "CUSTOM_A"]
        );
        assert_eq!(result[0].title, "37mm Custom AP");
        assert_eq!(result[0].price, 50);
    }

    #[test]
    fn entries_reject_undefined_duplicate_empty_nul_and_nonpositive_prices() {
        let vanilla = fixture();
        let configured = vec![
            custom("AT_INDEX_ZERO", 1),
            custom("DUP", 1),
            custom("DUP", 2),
            custom("", 1),
            custom("BAD\0NAME", 1),
            custom("FREE", 0),
            custom("NEGATIVE", -1),
        ];
        assert!(entries(&vanilla, &configured).is_empty());
    }

    #[test]
    fn empty_display_name_falls_back_to_the_item_identifier() {
        let mut configured = fixture();
        let mut ammo = custom("CUSTOM_AMMO", 5);
        ammo.display_name = "".into();
        ammo.milimeterage = "".into();
        configured.push(ammo);
        assert_eq!(entries(&fixture(), &configured)[0].title, "CUSTOM_AMMO");
    }

    #[test]
    fn grid_starts_top_left_and_fills_down_before_next_column() {
        let grid = Grid::new(1920, 1080).unwrap();
        let first = grid.position(0).unwrap();
        let second = grid.position(1).unwrap();
        assert_eq!(first, (-804.0, -506.0));
        assert_eq!(second, (first.0, first.1 + 44.0));
        assert_eq!(
            grid.position(grid.rows).unwrap(),
            (first.0 + 288.0, first.1)
        );
        assert!(grid.position(grid.rows * grid.columns).is_none());
    }

    #[test]
    fn grid_rejects_invalid_sizes_and_keeps_cells_inside_the_viewport() {
        assert!(Grid::new(0, 1080).is_none());
        assert!(Grid::new(1920, -1).is_none());
        assert!(Grid::new(20_000, 20_000).is_some());
        for (width, height) in [(320, 240), (1280, 720), (1920, 1080), (3840, 2160)] {
            let grid = Grid::new(width, height).unwrap();
            for i in 0..grid.rows * grid.columns {
                let (x, y) = grid.position(i).unwrap();
                assert!(x - Grid::CELL_WIDTH * 0.5 >= -(width as f32) * 0.5);
                assert!(x + Grid::CELL_WIDTH * 0.5 <= width as f32 * 0.5);
                assert!(y - Grid::CELL_HEIGHT * 0.5 >= -(height as f32) * 0.5);
                assert!(
                    y + Grid::CELL_HEIGHT * 0.5 <= height as f32 * 0.5 - Grid::EXIT_AREA_HEIGHT
                );
            }
        }
    }

    #[test]
    fn grid_minimum_size_is_derived_from_one_cell_and_reserved_space() {
        let width = (Grid::CELL_WIDTH + 2.0 * Grid::MARGIN) as i32;
        let height = (Grid::CELL_HEIGHT + 2.0 * Grid::MARGIN + Grid::EXIT_AREA_HEIGHT) as i32;

        let grid = Grid::new(width, height).unwrap();
        assert_eq!((grid.rows, grid.columns), (1, 1));
        assert!(Grid::new(width - 1, height).is_none());
        assert!(Grid::new(width, height - 1).is_none());
    }

    #[test]
    fn offer_checks_allow_selling_out_of_stock_ammo_but_reject_unsafe_totals() {
        assert!(can_offer(10, 0, 5, 100));
        assert!(can_offer(0, 10, 5, 100));
        assert!(!can_offer(0, 1, 2_000_000_000, 200_000_000));
        assert!(!can_offer(1, 0, 1, -1));
        for args in [
            (0, 0, 5),
            (-1, 2, 5),
            (1, -1, 5),
            (1, 0, 0),
            (1, 0, -5),
            (i32::MAX, 1, 1),
            (16_777_216, 0, 1),
            (10, 0, i32::MAX),
        ] {
            assert!(!can_offer(args.0, args.1, args.2, 100));
        }
    }

    #[test]
    fn labels_are_literal_single_line_and_bounded() {
        assert_eq!(plain_title("Test@{ammo}\nname\0"), "Test (ammo) name ");
        assert_eq!(plain_title(&"é".repeat(100)).chars().count(), 32);
    }
}
