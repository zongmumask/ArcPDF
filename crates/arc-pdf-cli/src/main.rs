use arc_pdf_core::PageId;
use arc_pdf_engine::PdfiumRuntime;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or("usage: arc-pdf-cli render --pdf <path> --page <n> --scale <s> --out <path>")?;
    if command != "render" {
        return Err(format!("unknown command: {command}").into());
    }

    let mut pdf: Option<PathBuf> = None;
    let mut page = 0u32;
    let mut scale = 2.0f32;
    let mut out: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--pdf" => pdf = Some(args.next().ok_or("--pdf requires a path")?.into()),
            "--page" => page = args.next().ok_or("--page requires a number")?.parse()?,
            "--scale" => scale = args.next().ok_or("--scale requires a number")?.parse()?,
            "--out" => out = Some(args.next().ok_or("--out requires a path")?.into()),
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    let pdf = pdf.ok_or("missing --pdf")?;
    let out = out.ok_or("missing --out")?;

    let runtime = PdfiumRuntime::init();
    let doc = runtime.open_document(&pdf)?;
    if page as usize >= doc.page_count() {
        return Err(format!("page {page} out of range (pages={})", doc.page_count()).into());
    }
    let raster = doc.render_page(PageId(page), scale)?;
    image::save_buffer(
        &out,
        &raster.pixels,
        raster.width as u32,
        raster.height as u32,
        image::ColorType::Rgba8,
    )?;
    println!(
        "rendered page {} at scale {} -> {} ({}x{})",
        page, scale, out.display(), raster.width, raster.height
    );
    Ok(())
}
