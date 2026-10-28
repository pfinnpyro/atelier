namespace Atelier;

/// <summary>A rendered page: pixels in a managed array.</summary>
/// <param name="Width">Width in pixels.</param>
/// <param name="Height">Height in pixels.</param>
/// <param name="Stride">Bytes per row (always <c>Width * 4</c> here).</param>
/// <param name="Format">Pixel layout.</param>
/// <param name="Pixels">The pixels, <c>Stride * Height</c> bytes, top row first.</param>
public sealed record RenderedPage(int Width, int Height, int Stride, PdfPixelFormat Format, byte[] Pixels);
