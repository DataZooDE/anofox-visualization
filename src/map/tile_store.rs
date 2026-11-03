//! The tiles a page has fetched for its `::MAPTILES` maps (#760): handed in
//! by the browser ([`register`], from the server's `/api/tiles`) and read by
//! the renderer through the render context's `"basemap"`. Like the boundary
//! registry ([`super::boundaries`]) it is process-wide; it is bounded
//! ([`MAX_TILES`], [`MAX_BYTES`]), oldest out first.
// Only the wasm page hands tiles in; a native build reads none.
#![cfg_attr(not(feature = "wasm"), allow(dead_code))]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

/// A tile body above this is refused (an ordinary 256 px tile is 5–50 KB).
pub const MAX_TILE_BYTES: usize = 512 * 1024;
/// The deepest level any XYZ source serves.
pub const MAX_ZOOM: u32 = 19;
pub const MAX_TILES: usize = 2000;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

type Key = (u32, u32, u32);

#[derive(Default)]
struct Store {
    tiles: HashMap<Key, Arc<Vec<u8>>>,
    order: VecDeque<Key>,
    bytes: usize,
}

fn store() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// The two image types a tile may be, from its first bytes.
pub fn image_type(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// Whether `z/x/y` is a tile of a source that goes to `max_zoom`.
pub fn valid_tile(z: u32, x: u32, y: u32, max_zoom: u32) -> bool {
    z <= max_zoom && z <= MAX_ZOOM && (x as u64) < (1u64 << z) && (y as u64) < (1u64 << z)
}

/// Keep tile `z/x/y`; `false` (nothing kept) for a tile outside the grid or
/// bytes that aren't a PNG / JPEG of at most [`MAX_TILE_BYTES`].
pub fn register(z: u32, x: u32, y: u32, bytes: &[u8]) -> bool {
    if !valid_tile(z, x, y, MAX_ZOOM) || bytes.len() > MAX_TILE_BYTES || image_type(bytes).is_none()
    {
        return false;
    }
    let mut s = store().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(old) = s.tiles.insert((z, x, y), Arc::new(bytes.to_vec())) {
        s.bytes -= old.len();
        s.order.retain(|k| *k != (z, x, y));
    }
    s.bytes += bytes.len();
    s.order.push_back((z, x, y));
    while s.order.len() > MAX_TILES || s.bytes > MAX_BYTES {
        let Some(k) = s.order.pop_front() else { break };
        if let Some(b) = s.tiles.remove(&k) {
            s.bytes -= b.len();
        }
    }
    true
}

pub fn get(z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
    let s = store().lock().unwrap_or_else(|p| p.into_inner());
    s.tiles.get(&(z, x, y)).map(|b| b.as_ref().clone())
}

pub fn has(z: u32, x: u32, y: u32) -> bool {
    let s = store().lock().unwrap_or_else(|p| p.into_inner());
    s.tiles.contains_key(&(z, x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1];

    #[test]
    fn only_images_in_the_grid_are_kept_and_the_store_is_bounded() {
        assert!(register(19, 7, 7, PNG));
        assert_eq!(get(19, 7, 7).as_deref(), Some(PNG));
        assert!(!register(19, 7, 8, b"<svg onload=alert(1)>"));
        assert!(!register(20, 0, 0, PNG));
        assert!(!register(2, 4, 0, PNG));
        assert!(!register(18, 0, 0, &vec![0x89; MAX_TILE_BYTES + 1]));
        assert!(!has(19, 7, 8));
        // bounded, oldest out first (one test: the store is process-wide)
        for i in 0..(MAX_TILES as u32 + 10) {
            assert!(register(17, i, 1, PNG));
        }
        let s = store().lock().unwrap();
        assert!(s.tiles.len() <= MAX_TILES);
        assert_eq!(s.order.len(), s.tiles.len());
        drop(s);
        assert!(!has(17, 0, 1), "the oldest went first");
        // the render context's basemap reads this store
        let b = crate::RenderOptions::from_primary_arg(
            r#"{"basemap": {"attribution": "a", "max_zoom": 3}}"#,
        )
        .unwrap()
        .basemap
        .unwrap();
        assert!(register(3, 2, 1, PNG));
        assert_eq!((b.tiles)(3, 2, 1).as_deref(), Some(PNG));
        assert!(has(17, MAX_TILES as u32 + 9, 1));
    }

    #[test]
    fn the_render_context_basemap_reads_the_store() {
        let o = crate::RenderOptions::from_primary_arg(
            r#"{"basemap": {"attribution": "© A\u0007", "max_zoom": 3}}"#,
        )
        .unwrap();
        let b = o.basemap.expect("a basemap");
        assert_eq!((b.attribution.as_str(), b.max_zoom), ("© A", 3));
        for bad in [
            r#"{"basemap": {"attribution": "a"}}"#,
            r#"{"basemap": {"attribution": "a", "max_zoom": 20}}"#,
            r#"{"basemap": {"max_zoom": 3}}"#,
            r#"{"basemap": "on"}"#,
        ] {
            let o = crate::RenderOptions::from_primary_arg(bad).unwrap();
            assert!(o.basemap.is_none(), "{bad}");
        }
    }
}
