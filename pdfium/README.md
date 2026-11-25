# PDFium build inputs

This repository uses a **prebuilt PDFium core + consumer-side extension source**
model, without CocoaPods or XCFramework coupling.

## Layout

- `pdfium-revision.toml`: single source of truth for PDFium revision, target matrix and GN args.
- `patches/`: checked-in patches applied with `git apply --check` before build.
- `../scripts/build-pdfium`: target-aware build entry point.
- `../artifacts/pdfium/`: generated libs, headers and manifests (gitignored).

## Strategy

1. Build PDFium core infrequently for each target.
2. Compile Arc extensions in the consumer crate (`ffi/arc-pdfium-ext`, later phases).
3. Keep extension C ABI public-style and stable where possible.
4. Bind B/C-layer extensions to `pdfium.revision` through the build manifest.

## Targets

macOS is the first validated baseline:

```text
macos_arm64
macos_x86_64
```

Windows, Linux, Android and iOS are declared in the target matrix but are not
implemented in Phase 1-A.

## Commands

Phase 1-A (dry run only):

```sh
scripts/build-pdfium --target macos_arm64 --dry-run
scripts/build-pdfium --list-targets
```

Phase 1-B will add real source sync, patch application, GN generation, ninja
build and artifact collection.
