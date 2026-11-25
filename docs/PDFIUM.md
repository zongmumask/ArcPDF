# PDFium Build and Integration

## Strategy

ArcPDF uses **prebuilt PDFium core + consumer-side extension source**. This is
the same high-level strategy as ArcPDFium, without CocoaPods or XCFramework
coupling.

- The PDFium core is built infrequently per target.
- Arc extensions are compiled in Rust-side crates (`ffi/arc-pdfium-ext`, later).
- Public-style C ABI is preferred for extensions.
- B/C-layer extensions are bound to a specific `pdfium.revision` through the manifest.

## Source of truth

`pdfium/pdfium-revision.toml` records:

- PDFium URL and pinned revision.
- HarfBuzz dependency placeholder (disabled in Phase 1-A).
- Common GN args and platform-specific GN args.
- Full target matrix: macOS, Windows, Linux, Android, iOS.
- Artifact layout.

macOS targets are implemented first:

```text
macos_arm64
macos_x86_64
```

Other targets are reserved and must fail loudly rather than silently produce
wrong artifacts.

## Directory layout

```text
pdfium/
  pdfium-revision.toml
  README.md
  patches/
scripts/
  build-pdfium
artifacts/pdfium/<target>/
  include/
  libpdfium.a
  manifest.json
```

## Commands

```sh
scripts/setup-dev.sh
scripts/build-pdfium --list-targets
scripts/build-pdfium --target macos_arm64 --dry-run
scripts/build-pdfium --check-tools
scripts/build-pdfium --manifest-info
```

Phase 1-A only implements planning and validation. Real source sync, patch
application, GN generation, ninja builds and artifact collection are Phase 1-B.

## Future phases

Phase 1-B:

- gclient sync to pinned revision.
- checked-in patches with `git apply --check`.
- GN args generation by target.
- `ninja pdfium`.
- collect public headers and static lib.
- write SHA-256 manifest.

Phase 1-C:

- `ffi/arc-pdfium-sys` bindgen from the same pinned headers.
- compile/link/nm checks for extension ABI.
- first CLI render smoke test.
