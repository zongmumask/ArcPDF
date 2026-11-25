//! GPUI-free, PDFium-free document core.
//!
//! This crate owns identifiers, geometry and generation semantics that are
//! shared by the engine, render scheduler and UI layers. It must remain
//! platform-independent and easy to test without a window.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Vec2,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Vec2::new(x, y),
            size: Size::new(width, height),
        }
    }

    pub fn intersects(self, other: Self) -> bool {
        let self_max_x = self.origin.x + self.size.width;
        let self_max_y = self.origin.y + self.size.height;
        let other_max_x = other.origin.x + other.size.width;
        let other_max_y = other.origin.y + other.size.height;

        self.origin.x < other_max_x
            && other.origin.x < self_max_x
            && self.origin.y < other_max_y
            && other.origin.y < self_max_y
    }
}

/// Geometry of a PDF page in PDF user space points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub width_pt: f32,
    pub height_pt: f32,
    pub crop_offset: Vec2,
    pub rotation: u8,
}

impl PageGeometry {
    pub const fn new(width_pt: f32, height_pt: f32) -> Self {
        Self {
            width_pt,
            height_pt,
            crop_offset: Vec2::new(0.0, 0.0),
            rotation: 0,
        }
    }

    pub fn rect(self) -> Rect {
        Rect::new(
            self.crop_offset.x,
            self.crop_offset.y,
            self.width_pt,
            self.height_pt,
        )
    }
}

/// Incremented whenever state that affects rendering changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentGeneration(pub u64);

impl ContentGeneration {
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_generation_advances() {
        assert_eq!(ContentGeneration(0).next(), ContentGeneration(1));
    }

    #[test]
    fn rect_intersection_uses_extended_region_semantics() {
        let visible = Rect::new(0.0, 0.0, 100.0, 100.0);
        let extended = Rect::new(0.0, -80.0, 100.0, 260.0);
        let page_above = Rect::new(0.0, 170.0, 100.0, 100.0);

        assert!(!page_above.intersects(visible));
        assert!(page_above.intersects(extended));
    }
}
