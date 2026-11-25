//! PDFium engine: owns process-global init and raw handle lifetime.
//!
//! Phase 1-C is a single-threaded minimal engine used by the CLI. The future
//! GPUI app will wrap the same operations in a process-level actor.

use arc_pdf_core::{ContentGeneration, PageId};
use arc_pdfium_sys as sys;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Once;

static INIT: Once = Once::new();

#[derive(Debug)]
pub struct EngineError(pub String);

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EngineError {}

pub struct PdfiumRuntime;

impl PdfiumRuntime {
    pub fn init() -> Self {
        INIT.call_once(|| unsafe {
            sys::FPDF_InitLibrary();
        });
        Self
    }

    pub fn open_document(&self, path: &Path) -> Result<Document, EngineError> {
        let bytes = std::fs::read(path)
            .map_err(|e| EngineError(format!("read PDF failed: {e}")))?;
        let handle = unsafe {
            sys::FPDF_LoadMemDocument(
                bytes.as_ptr() as *const std::os::raw::c_void,
                bytes.len() as std::os::raw::c_int,
                std::ptr::null(),
            )
        };
        let handle = NonNull::new(handle)
            .ok_or_else(|| EngineError("FPDF_LoadMemDocument failed".into()))?;
        Ok(Document {
            _bytes: bytes,
            handle,
            generation: ContentGeneration::default(),
        })
    }
}

pub struct Document {
    _bytes: Vec<u8>,
    handle: NonNull<sys::fpdf_document_t__>,
    generation: ContentGeneration,
}

impl Document {
    pub fn generation(&self) -> ContentGeneration {
        self.generation
    }

    pub fn page_count(&self) -> usize {
        unsafe { sys::FPDF_GetPageCount(self.handle.as_ptr()) as usize }
    }

    pub fn page_size(&self, page: PageId) -> Result<(f32, f32), EngineError> {
        let mut width = 0.0f64;
        let mut height = 0.0f64;
        let ok = unsafe {
            sys::FPDF_GetPageSizeByIndex(
                self.handle.as_ptr(),
                page.0 as std::os::raw::c_int,
                &mut width,
                &mut height,
            )
        };
        if ok == 0 {
            return Err(EngineError(format!("page size failed for page {}", page.0)));
        }
        Ok((width as f32, height as f32))
    }

    pub fn render_page(&self, page: PageId, scale: f32) -> Result<Raster, EngineError> {
        let (width_pt, height_pt) = self.page_size(page)?;
        let width = (width_pt * scale).ceil() as usize;
        let height = (height_pt * scale).ceil() as usize;
        if width == 0 || height == 0 {
            return Err(EngineError("empty render target".into()));
        }

        let mut pixels = vec![0u8; width * height * 4];
        let stride = (width * 4) as std::os::raw::c_int;

        let bitmap = unsafe {
            sys::FPDFBitmap_CreateEx(
                width as std::os::raw::c_int,
                height as std::os::raw::c_int,
                sys::FPDFBitmap_BGRA as std::os::raw::c_int,
                pixels.as_mut_ptr() as *mut std::os::raw::c_void,
                stride,
            )
        };
        if bitmap.is_null() {
            return Err(EngineError("FPDFBitmap_CreateEx failed".into()));
        }
        let _bitmap_guard = BitmapGuard(bitmap);

        unsafe {
            sys::FPDFBitmap_FillRect(
                bitmap,
                0,
                0,
                width as std::os::raw::c_int,
                height as std::os::raw::c_int,
                0xFFFFFFFF,
            );
        }

        let page_handle = unsafe {
            sys::FPDF_LoadPage(self.handle.as_ptr(), page.0 as std::os::raw::c_int)
        };
        if page_handle.is_null() {
            return Err(EngineError(format!("FPDF_LoadPage failed for page {}", page.0)));
        }
        let _page_guard = PageGuard(page_handle);

        let matrix = sys::FS_MATRIX {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            e: 0.0,
            f: 0.0,
        };
        let clip = sys::FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: width as f32,
            bottom: height as f32,
        };
        unsafe {
            sys::FPDF_RenderPageBitmapWithMatrix(
                bitmap,
                page_handle,
                &matrix,
                &clip,
                sys::FPDF_REVERSE_BYTE_ORDER as std::os::raw::c_int,
            );
        }

        // PDFium writes BGRA; convert to RGBA for the image crate.
        for chunk in pixels.as_chunks_mut::<4>().0 {
            chunk.swap(0, 2);
        }

        Ok(Raster {
            width,
            height,
            stride,
            pixels,
        })
    }
}

pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub stride: std::os::raw::c_int,
    pub pixels: Vec<u8>,
}

struct BitmapGuard(sys::FPDF_BITMAP);
impl Drop for BitmapGuard {
    fn drop(&mut self) {
        unsafe { sys::FPDFBitmap_Destroy(self.0) }
    }
}

struct PageGuard(sys::FPDF_PAGE);
impl Drop for PageGuard {
    fn drop(&mut self) {
        unsafe { sys::FPDF_ClosePage(self.0) }
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { sys::FPDF_CloseDocument(self.handle.as_ptr()) }
    }
}
