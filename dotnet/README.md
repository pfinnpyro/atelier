# Atelier.Net

.NET bindings for the [Atelier](https://github.com/pfinnpyro/atelier) PDF engine.
The engine is native Rust code, shipped inside the package as `atelier.dll`
(Windows x64 for now). There is no server or webview: calls go straight into
the engine in your process.

## Use it

```csharp
using Atelier;

using var doc = PdfDocument.Open("file.pdf");
Console.WriteLine($"{doc.PageCount} pages, PDF {doc.Version}");

// BGRA premultiplied at 150 dpi on white, unless you say otherwise.
RenderedPage img = doc.GetPage(0).Render(new RenderOptions { Dpi = 150 });
// img.Width, img.Height, img.Stride, img.Pixels
```

WPF: render straight into a `WriteableBitmap`.

```csharp
var page = doc.GetPage(0);
var (w, h) = page.GetPixelSize(96);
var bmp = new WriteableBitmap(w, h, 96, 96, PixelFormats.Pbgra32, null);
bmp.Lock();
page.RenderInto(bmp.BackBuffer, bmp.BackBufferStride * h, bmp.BackBufferStride,
    new RenderOptions { Dpi = 96, Format = PdfPixelFormat.Bgra8Premultiplied });
bmp.AddDirtyRect(new Int32Rect(0, 0, w, h));
bmp.Unlock();
```

- Pages are numbered from 0. Sizes are in points (1/72 inch) after crop box
  and rotation.
- A `PdfDocument` can be shared across threads, for example to render pages in
  parallel. Dispose it to free native memory.
- Failures throw `AtelierException`, whose `Error` says why (not a PDF,
  encrypted, page out of range, and so on).

What renders today is what the engine renders today: paths, strokes, clipping,
colour and images. Text arrives with the engine's fonts phase, with no change
to this API.

## Build it

Needs Rust (stable) and the .NET 8 SDK or newer.

```
dotnet/build-native.ps1          # Windows: builds atelier.dll into dotnet/native/win-x64
dotnet/build-native.sh           # Linux/macOS: same, for this machine
dotnet test dotnet
dotnet pack dotnet/src/Atelier.Net -c Release -o out
```

The package takes every native build found under `dotnet/native/<rid>/`, so a
Linux or macOS library can be added next to the Windows one later.

## How it fits together

- `crates/atelier-ffi` is a small C interface over the engine (open, info,
  page size, render). It is the only crate allowed to use `unsafe`.
- Its `build.rs` uses [csbindgen](https://github.com/Cysharp/csbindgen) to
  write `src/Atelier.Net/Interop/NativeMethods.g.cs`, so the C# declarations
  always match the Rust side. Do not edit that file by hand.
- `src/Atelier.Net` wraps those calls in `PdfDocument` and `PdfPage`.
