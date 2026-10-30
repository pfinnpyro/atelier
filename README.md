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
| `atelier-script` | The form-script sandbox: a JavaScript engine (Boa) with only the form's values and Acrobat's form helpers, run in a helper process |
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

## Tabs and recent files

Each file opens in its own tab, and keeps its place, zoom, search and picked
pages while another tab is in front. Opening a file that is already open
brings its tab forward. The ⏷ beside Open lists the last 12 files, which also
show on the start screen; a file that has gone is taken off the list when
picked. Closing the window asks about each tab with unsaved changes in turn.

## Marking up

The **Mark** menu:

- **Highlight**: select text, then pick a colour and *Highlight the selected
  text* (or right-click the selection). Each line gets its own box, and the
  text shows through.
- **Add a comment**: click where it goes and type. Pointing at a comment, or
  at any annotation with text (including ones already in the file), shows
  its text.
- **Draw**: drag on a page; pick an ink colour and width in the bar along
  the bottom. Enter adds the lines drawn so far, Esc puts the pen down.
- **Sign…**: draw your signature once on the pad, then click where it goes.
  It is kept in `signature.txt` beside the settings, on this computer only,
  for next time; *Draw a new signature…* replaces it. This is a drawn
  signature, not a cryptographic digital signature.

Ctrl+Z takes back the newest mark, and right-clicking a mark added since the
last save removes it. Marks are saved as standard Highlight, Text and Ink
annotations, each with its own appearance, so other PDF apps show them too.

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

## Form scripts

Many forms carry JavaScript that checks what is typed and works out totals.
Atelier blocks all of it until you allow it. When a file has form scripts,
a notice says so; open the inspector (F12), go to **Actions**, read the
scripts and tick the ones to allow, or use *Allow all*. Allowing lasts
while the file is open; the next time it opens, its scripts are blocked
again.

Allowed scripts run in a separate helper process with an empty environment,
started for each change and stopped after two seconds per script. Inside it,
a script sees only this form's field values and a small set of Acrobat
objects (`getField`, `event`, `app.alert`, `util.printf` and the `AF`
number, percent, range and special-format helpers). It cannot reach files,
the network, the app or the computer. What comes back is a list of new field
values and which fields to show or hide, which the viewer checks and
applies like any other edit.

Keystroke and validate scripts run when a value is committed, then every
calculation runs in the form's calculation order, each calculated value
passing its own validate script. Formatting scripts (how a value looks, such
as 1,234.50) are not run yet, and nothing else in the file (open actions,
links that launch programs, print scripts) ever runs.

## Viewer keys

| Key | Does |
| --- | --- |
| Ctrl+O | Open a file, in a new tab |
| Ctrl+Tab, Ctrl+Shift+Tab | Next and previous tab |
| Ctrl+W | Close the tab (asks first when it has unsaved changes) |
| Ctrl+Z | Take back the newest mark |
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
