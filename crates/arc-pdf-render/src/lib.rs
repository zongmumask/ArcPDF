//! GPUI-free preview/tile scheduling and cache policy.
//!
//! This crate owns the strategy only: it knows page geometry, viewport, tile
//! keys, cancellation and cache budgets. It does not call PDFium and does not
//! create GPU resources.

use arc_pdf_core::{ContentGeneration, DocumentId, PageId, Rect, Size};
use std::collections::HashMap;

pub const TILE_SIZE_PT: f32 = 128.0;
pub const PREFETCH_VERTICAL_FACTOR: f32 = 0.8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub visible: Rect,
}

impl Viewport {
    pub fn new(visible: Rect) -> Self {
        Self { visible }
    }

    /// The existing Swift implementation expands only vertically, by 0.8 of
    /// the visible height on each side (`DocumentView.preloadVerticalMargin`).
    pub fn prefetch(&self) -> Rect {
        let margin = self.visible.size.height * PREFETCH_VERTICAL_FACTOR;
        Rect::new(
            self.visible.origin.x,
            self.visible.origin.y - margin,
            self.visible.size.width,
            self.visible.size.height + margin * 2.0,
        )
    }

    pub fn is_visible(&self, rect: Rect) -> bool {
        self.visible.intersects(rect)
    }

    pub fn is_in_prefetch(&self, rect: Rect) -> bool {
        self.prefetch().intersects(rect)
    }
}

/// Discrete pixel scale. Continuous zoom must be quantized before it enters a
/// cache key; otherwise every zoom frame invalidates all tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScaleBucket(pub i16);

impl ScaleBucket {
    /// Quarter-octave buckets: bucket = round(log2(scale) * 4).
    pub fn from_scale(scale: f32) -> Self {
        let s = scale.max(1e-6);
        let bucket = (s.log2() * 4.0).round() as i16;
        Self(bucket)
    }

    pub fn scale(self) -> f32 {
        2.0f32.powf(self.0 as f32 / 4.0)
    }

    pub fn lower(self, steps: i16) -> Self {
        Self(self.0 - steps)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileCoord {
    pub col: i32,
    pub row: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderRole {
    Preview,
    Tile,
    Thumbnail,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RenderFlags {
    pub annotations: bool,
    pub forms: bool,
    pub night: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub document: DocumentId,
    pub page: PageId,
    pub role: RenderRole,
    pub tile: Option<TileCoord>,
    pub bucket: ScaleBucket,
    pub generation: ContentGeneration,
    pub flags: RenderFlags,
}

impl RenderKey {
    pub fn preview(document: DocumentId, page: PageId, bucket: ScaleBucket,
                   generation: ContentGeneration, flags: RenderFlags) -> Self {
        Self { document, page, role: RenderRole::Preview, tile: None, bucket, generation, flags }
    }

    pub fn tile(document: DocumentId, page: PageId, tile: TileCoord, bucket: ScaleBucket,
                generation: ContentGeneration, flags: RenderFlags) -> Self {
        Self { document, page, role: RenderRole::Tile, tile: Some(tile), bucket, generation, flags }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Thumbnail,
    Prefetch,
    Preview,
    VisibleTile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestToken(pub u64);

#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    pub key: RenderKey,
    pub rect_pt: Rect,
    pub pixel_size: Size,
    pub priority: Priority,
    pub token: RequestToken,
}

pub struct TileGrid;

impl TileGrid {
    /// 128pt cells clipped to the page rectangle.
    pub fn visible_tiles(page_rect: Rect, viewport: &Viewport) -> Vec<(TileCoord, Rect)> {
        let visible = viewport.visible;
        let first_col = (visible.origin.x / TILE_SIZE_PT).floor() as i32;
        let last_col = ((visible.origin.x + visible.size.width - 1.0) / TILE_SIZE_PT).floor() as i32;
        let first_row = (visible.origin.y / TILE_SIZE_PT).floor() as i32;
        let last_row = ((visible.origin.y + visible.size.height - 1.0) / TILE_SIZE_PT).floor() as i32;

        let mut result = Vec::new();
        for row in first_row..=last_row {
            for col in first_col..=last_col {
                let cell = Rect::new(
                    col as f32 * TILE_SIZE_PT,
                    row as f32 * TILE_SIZE_PT,
                    TILE_SIZE_PT,
                    TILE_SIZE_PT,
                );
                if let Some(clipped) = intersect(page_rect, cell) {
                    result.push((TileCoord { col, row }, clipped));
                }
            }
        }
        result
    }

    pub fn pixel_size(rect: Rect, bucket: ScaleBucket) -> Size {
        let scale = bucket.scale();
        Size::new(
            (rect.size.width * scale).ceil(),
            (rect.size.height * scale).ceil(),
        )
    }
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let min_x = a.origin.x.max(b.origin.x);
    let min_y = a.origin.y.max(b.origin.y);
    let max_x = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let max_y = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    if max_x <= min_x || max_y <= min_y {
        None
    } else {
        Some(Rect::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulerDecision {
    Submit,
    Duplicate,
    Stale,
}

/// Tracks in-flight requests and rejects stale generation results.
#[derive(Default)]
pub struct RenderScheduler {
    next_token: u64,
    in_flight: HashMap<RenderKey, RequestToken>,
}

impl RenderScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&mut self, key: RenderKey) -> SchedulerDecision {
        if self.in_flight.contains_key(&key) {
            return SchedulerDecision::Duplicate;
        }
        self.next_token += 1;
        let token = RequestToken(self.next_token);
        self.in_flight.insert(key, token);
        SchedulerDecision::Submit
    }

    pub fn token(&self, key: &RenderKey) -> Option<RequestToken> {
        self.in_flight.get(key).copied()
    }

    pub fn finish(&mut self, key: &RenderKey, token: RequestToken) -> SchedulerDecision {
        if self.in_flight.get(key).copied() != Some(token) {
            return SchedulerDecision::Stale;
        }
        self.in_flight.remove(key);
        SchedulerDecision::Submit
    }

    pub fn cancel(&mut self, key: &RenderKey) {
        self.in_flight.remove(key);
    }

    pub fn cancel_generation(&mut self, generation: ContentGeneration) {
        self.in_flight.retain(|key, _| key.generation != generation);
    }

    pub fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

#[derive(Clone, Copy, Debug)]
struct CacheEntry {
    bytes: usize,
    last_used: u64,
}

/// Simple byte-budgeted LRU. Stores accounting only; raster bytes live in the
/// caller (engine/UI bridge) in this minimal phase.
pub struct RenderCache {
    budget: usize,
    used: usize,
    clock: u64,
    entries: HashMap<RenderKey, CacheEntry>,
}

impl RenderCache {
    pub fn new(budget: usize) -> Self {
        Self { budget, used: 0, clock: 0, entries: HashMap::new() }
    }

    pub fn contains(&self, key: &RenderKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert(&mut self, key: RenderKey, bytes: usize) {
        self.clock += 1;
        if let Some(old) = self.entries.remove(&key) {
            self.used = self.used.saturating_sub(old.bytes);
        }
        self.entries.insert(key, CacheEntry { bytes, last_used: self.clock });
        self.used += bytes;
        self.evict_to_budget();
    }

    pub fn touch(&mut self, key: &RenderKey) {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = self.clock;
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn evict_to_budget(&mut self) {
        while self.used > self.budget {
            let oldest = self.entries.iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| *key);
            let Some(key) = oldest else { break };
            if let Some(entry) = self.entries.remove(&key) {
                self.used = self.used.saturating_sub(entry.bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_page() -> (DocumentId, PageId, ContentGeneration) {
        (DocumentId(1), PageId(0), ContentGeneration(7))
    }

    #[test]
    fn prefetch_expands_vertically_only() {
        let viewport = Viewport::new(Rect::new(0.0, 0.0, 100.0, 100.0));
        let prefetch = viewport.prefetch();

        assert_eq!(prefetch.origin.x, 0.0);
        assert_eq!(prefetch.size.width, 100.0);
        assert_eq!(prefetch.origin.y, -80.0);
        assert_eq!(prefetch.size.height, 260.0);

        let page = Rect::new(0.0, 170.0, 100.0, 100.0);
        assert!(!viewport.is_visible(page));
        assert!(viewport.is_in_prefetch(page));
    }

    #[test]
    fn scale_bucket_is_quantized() {
        assert_eq!(ScaleBucket::from_scale(1.0), ScaleBucket(0));
        assert_eq!(ScaleBucket::from_scale(1.19), ScaleBucket(1));
        assert_eq!(ScaleBucket::from_scale(2.0), ScaleBucket(4));
    }

    #[test]
    fn tile_grid_clips_to_page_and_uses_ceil() {
        let viewport = Viewport::new(Rect::new(0.0, 0.0, 130.0, 130.0));
        let tiles = TileGrid::visible_tiles(Rect::new(0.0, 0.0, 200.0, 200.0), &viewport);
        assert_eq!(tiles.len(), 4);

        let size = TileGrid::pixel_size(Rect::new(0.0, 0.0, 100.0, 100.0), ScaleBucket::from_scale(2.0));
        assert_eq!(size.width, 200.0);
        assert_eq!(size.height, 200.0);
    }

    #[test]
    fn scheduler_deduplicates_and_rejects_stale() {
        let (doc, page, generation) = doc_page();
        let key = RenderKey::tile(doc, page, TileCoord { col: 0, row: 0 },
                                  ScaleBucket(4), generation, RenderFlags::default());
        let mut scheduler = RenderScheduler::new();

        assert_eq!(scheduler.request(key), SchedulerDecision::Submit);
        let token = scheduler.token(&key).unwrap();
        assert_eq!(scheduler.request(key), SchedulerDecision::Duplicate);
        assert_eq!(scheduler.finish(&key, token), SchedulerDecision::Submit);
        assert_eq!(scheduler.finish(&key, token), SchedulerDecision::Stale);
        assert_eq!(scheduler.in_flight_count(), 0);
    }

    #[test]
    fn generation_change_drops_old_requests() {
        let (doc, page, generation) = doc_page();
        let old_key = RenderKey::preview(doc, page, ScaleBucket(4), generation, RenderFlags::default());
        let new_key = RenderKey::preview(doc, page, ScaleBucket(4), generation.next(), RenderFlags::default());
        let mut scheduler = RenderScheduler::new();

        assert_eq!(scheduler.request(old_key), SchedulerDecision::Submit);
        assert_eq!(scheduler.request(new_key), SchedulerDecision::Submit);
        scheduler.cancel_generation(generation);

        assert_eq!(scheduler.in_flight_count(), 1);
        assert!(scheduler.token(&new_key).is_some());
    }

    #[test]
    fn cache_evicts_lru_by_bytes() {
        let (doc, page, generation) = doc_page();
        let mut cache = RenderCache::new(100);
        let a = RenderKey::preview(doc, page, ScaleBucket(4), generation, RenderFlags::default());
        let flags = RenderFlags { night: true, ..Default::default() };
        let b = RenderKey::preview(doc, page, ScaleBucket(4), generation, flags);

        cache.insert(a, 60);
        cache.insert(b, 60);
        assert_eq!(cache.len(), 1);
        assert!(!cache.contains(&a));
        assert!(cache.contains(&b));
        assert!(cache.used_bytes() <= 100);
    }
}
