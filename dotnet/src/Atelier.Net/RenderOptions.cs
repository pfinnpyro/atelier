namespace Atelier;

/// <summary>How rendered pixels are laid out. All formats are 4 bytes per
/// pixel, rows from top to bottom.</summary>
public enum PdfPixelFormat
{
    /// <summary>R, G, B, A with straight alpha. For PNG encoders and most image libraries.</summary>
    Rgba8 = 0,
    /// <summary>R, G, B, A with premultiplied alpha.</summary>
    Rgba8Premultiplied = 1,
    /// <summary>B, G, R, A with premultiplied alpha. Matches WPF <c>PixelFormats.Pbgra32</c>,
    /// GDI+ <c>Format32bppPArgb</c>, WinUI, Direct2D and SkiaSharp <c>Bgra8888</c> premultiplied.</summary>
    Bgra8Premultiplied = 2,
}

/// <summary>Settings for rendering a page.</summary>
public sealed record RenderOptions
{
    /// <summary>Resolution in dots per inch. 72 draws one pixel per PDF point.
    /// Must be above 0 and at most 4608.</summary>
    public double Dpi { get; init; } = 150;

    /// <summary>Pixel layout of the output.</summary>
    public PdfPixelFormat Format { get; init; } = PdfPixelFormat.Bgra8Premultiplied;

    /// <summary>Colour behind the page as 0xAARRGGBB, straight alpha.
    /// The default is opaque white; use 0 for a transparent background.</summary>
    public uint BackgroundArgb { get; init; } = 0xFFFFFFFF;

    /// <summary>The defaults: 150 dpi, BGRA premultiplied, white background.</summary>
    public static RenderOptions Default { get; } = new();

    internal double Scale => Dpi / 72.0;
}
