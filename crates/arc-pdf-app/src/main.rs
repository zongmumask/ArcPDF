use arc_pdf_ui::{LibraryWindow, PdfWindow};
use gpui::*;
use std::path::PathBuf;

fn main() {
    let app = gpui_platform::application();
    let initial_pdf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .filter(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")));

    app.run(move |cx: &mut App| {
        if let Some(path) = initial_pdf.clone() {
            open_pdf_window(path, cx);
            return;
        }
        open_library(cx);
    });
}

fn open_library(cx: &mut App) {
    cx.open_window(WindowOptions::default(), |_window, cx| {
        cx.new(LibraryWindow::new)
    })
    .unwrap();
}

fn open_pdf_window(path: PathBuf, cx: &mut App) {
    cx.open_window(WindowOptions::default(), |_window, cx| {
        cx.new(|cx| PdfWindow::new(path.clone(), cx))
    })
    .unwrap();
}
