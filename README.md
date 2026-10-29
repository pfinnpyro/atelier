# Atelier

A native PDF viewer and editor, written from the ground up in Rust. No webview:
the PDF engine (parsing, decompression, fonts, rendering, saving) is our own code,
and the app draws pages straight to the GPU.

Windows is the first target. Linux and macOS come later.

## Status

Phases 1 to 5 are done. Pages render with paths, strokes, clipping, colour,
uncompressed, Flate and JPEG images, and text in every PDF font type, and the
text of a page can be extracted with each glyph's position. Shadings and
patterns are not drawn yet.

The viewer opens a file (from the Open button, Ctrl+O, by dropping it on the
window, or on the command line), scrolls through all pages, zooms, selects and
copies text, and searches. Pages are drawn by our renderer in 1024-pixel tiles
on background threads; the window only puts finished tiles on screen, so
moving or resizing it never touches the document.

Fonts a file does not embed are drawn with the system's metric-compatible
fonts: Arial, Times New Roman and Courier New on Windows and macOS, and
Liberation (or DejaVu) on Linux. Chinese, Japanese and Korean text uses the
system's CJK fonts for its script (Yu Gothic, Microsoft YaHei, Malgun Gothic
and the like on Windows; Noto CJK, IPA or WenQuanYi on Linux). Set
`ATELIER_FONT_DIR` to add a folder to search.

Adobe's predefined CJK CMaps are built in, compiled from Adobe's
cmap-resources (BSD-3-Clause; see
`crates/atelier-render/src/font/data/ADOBE-CMAPS-LICENSE.txt`).

| Crate | What it does |
| --- | --- |
| `atelier-filters` | Our own Flate (inflate and a compressor), LZW, ASCII85, ASCIIHex and RunLength, PNG and TIFF predictors, and image decoders: JPEG (baseline and progressive), CCITT fax, JBIG2 and JPEG 2000 |
| `atelier-color` | Colour management: ICC profiles (matrix/TRC and LUT based), CalGray, CalRGB and Lab, and press-like DeviceCMYK |
| `atelier-syntax` | Lexer, object model, xref tables, xref streams, object streams, incremental updates, broken-file repair, page tree, content stream parsing |
| `atelier-font` | Our font readers: TrueType and OpenType, CFF (including CID-keyed) and Type 1 outlines, character maps and metrics, plus the standard encodings, the Adobe Glyph List and the 14 standard fonts' widths |
| `atelier-raster` | Our rasterizer: exact-area anti-aliased fills (non-zero and even-odd), strokes with joins, caps and dashes, clip masks, image drawing |
| `atelier-render` | Content stream interpreter, graphics state, colour spaces, Form XObjects, images, PDF fonts (simple, CID, Type 3, CMaps including Adobe's predefined CJK ones, ToUnicode), text drawing and extraction |
| `atelier-view` | Viewer logic without a window: page layout, the tile grid and cache, text selection and search |
| `atelier-app` | The viewer window (`atelier-viewer`), built on eframe (winit, wgpu and egui) |
| `atelier-cli` | The `atelier` command for inspecting and rendering files |

Damaged files are read the way other viewers read them: a wrong `/Length`, wrong
object offsets, a broken `startxref` or a missing xref table are repaired by
scanning the file, and truncated Flate data keeps whatever decoded.

## Build and test

```
cargo test --workspace
cargo run --release -p atelier-app -- some.pdf
cargo run --release -p atelier-cli -- info some.pdf
```

The viewer needs Rust 1.95 or newer; the library crates build with 1.85.

## Releases and updates

Pushing a version tag publishes a release for Windows through
`.github/workflows/release.yml`:

1. Set `version` in the workspace `Cargo.toml`, for example `0.2.0`, and merge.
2. Tag that commit and push the tag: `git tag v0.2.0 && git push origin v0.2.0`.
   A tag with a dash, such as `v0.2.0-beta.1`, is published as a pre-release
   on the **preview** channel.

The release carries the installer (`atelier-setup-windows-x64.exe`), the
program on its own (`atelier-windows-x64.exe`), the command-line tool, a zip,
and `SHA256SUMS.txt`. Atelier checks GitHub for a newer release when it starts
(Settings → Updates), and About → Check for updates downloads it, checks it
against `SHA256SUMS.txt`, and installs it: through the installer for an
installed copy, or by replacing the program file for a portable one.

## Organizing pages

In the sidebar's page list:

- Click a page to go to it; Ctrl+click and Shift+click pick several.
- Drag pages to move them (in the one-column view).
- Right-click for: rotate, save the pages as a new file, remove them, and add
  the pages of another PDF before this page or at the end. **+ Add pages**
  adds a file's pages at the end, which is how files are merged.
- Delete removes the picked pages while the pointer is over the list.

Changes are saved like any other. Once pages are removed, saving writes the
file out whole without them, so their content is gone from the file's bytes
and not only from its page list. Saving pages as a new file leaves out
everything the other pages used, including the form and bookmarks.

## Viewer keys

| Key | Does |
| --- | --- |
| Ctrl+O | Open a file |
| Ctrl+wheel, Ctrl+= and Ctrl+−, Ctrl+0 | Zoom around the pointer, zoom in and out, 100% |
| Page Up, Page Down, Space, Home, End, arrows | Move through the document |
| Drag | Select text; Ctrl+C copies it, Esc clears it |
| Ctrl+F, Enter, Shift+Enter, F3 | Search, next match, previous match |

## The `atelier` command

```
atelier info <file.pdf>                  version, structure and page count
atelier pages <file.pdf>                 one line per page: boxes and rotation
atelier objects <file.pdf>               every object number and where it lives
atelier obj <file.pdf> <num> [gen]       print one object
atelier stream <file.pdf> <num> [gen]    write a stream's decoded bytes to stdout
atelier content <file.pdf> <page>        write a page's decoded content (page from 1)
atelier render <file.pdf> <page|all> [--dpi N] [-o out.png]
                                         render pages to PNG
atelier text <file.pdf> [page|all] [--glyphs]
                                         print the text of pages; --glyphs prints each
                                         glyph with its box
atelier arrange <file.pdf> <pages> -o <out.pdf>
                                         keep the pages listed, in that order (3,1-2)
atelier extract <file.pdf> <pages> -o <out.pdf>
                                         save the pages listed as a new file
atelier merge <file.pdf> <more.pdf...> -o <out.pdf>
                                         add the other files' pages at the end
```

## Comparing against a reference renderer

`tools/make_samples.py DIR` writes small test PDFs that exercise fills, strokes,
clipping, colour, images, JPEG images, forms and text in every font type. `tools/refdiff.py FILE...` renders them with
Atelier and with poppler's `pdftoppm` and prints how many pixels differ
(`--keep DIR` also writes side-by-side images). `tools/fontcheck.py FONT`
compares every glyph outline with fontTools'. `tools/gen_font_data.py`
regenerates the font tables in `atelier-font/src/data`.

## Roadmap

1. Foundation and test harness
2. Parse files
3. Paths and images: content stream interpreter and our own rasterizer
4. Fonts and text
5. Viewer app: winit, wgpu, egui chrome, tiled rendering
6. Full fidelity: shadings, transparency, ICC, JBIG2, JPX, encryption
7. Saving: incremental updates and full rewrites
8. Annotations and drawing
9. Forms
