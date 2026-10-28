# Atelier

A native PDF viewer and editor, written from the ground up in Rust. No webview:
the PDF engine (parsing, decompression, fonts, rendering, saving) is our own code,
and the app draws pages straight to the GPU.

Windows is the first target. Linux and macOS come later.

## Status

Phase 1 of the roadmap: reading the file structure.

| Crate | What it does |
| --- | --- |
| `atelier-filters` | Our own Flate (inflate), LZW, ASCII85, ASCIIHex and RunLength decoders, plus PNG and TIFF predictors |
| `atelier-syntax` | Lexer, object model, xref tables, xref streams, object streams, incremental updates, broken-file repair, page tree |
| `atelier-cli` | The `atelier` command for inspecting files |

Damaged files are read the way other viewers read them: a wrong `/Length`, wrong
object offsets, a broken `startxref` or a missing xref table are repaired by
scanning the file, and truncated Flate data keeps whatever decoded.

## Build and test

```
cargo test --workspace
cargo run --release -p atelier-cli -- info some.pdf
```

## The `atelier` command

```
atelier info <file.pdf>                  version, structure and page count
atelier pages <file.pdf>                 one line per page: boxes and rotation
atelier objects <file.pdf>               every object number and where it lives
atelier obj <file.pdf> <num> [gen]       print one object
atelier stream <file.pdf> <num> [gen]    write a stream's decoded bytes to stdout
atelier content <file.pdf> <page>        write a page's decoded content (page from 1)
```

## Roadmap

1. Foundation and test harness
2. Parse files (this phase)
3. Paths and images: content stream interpreter and our own rasterizer
4. Fonts and text
5. Viewer app: winit, wgpu, egui chrome, tiled rendering
6. Full fidelity: shadings, transparency, ICC, JBIG2, JPX, encryption
7. Saving: incremental updates and full rewrites
8. Annotations and drawing
9. Forms
