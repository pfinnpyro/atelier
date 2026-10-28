using Atelier.Interop;

namespace Atelier;

/// <summary>One page of a <see cref="PdfDocument"/>.</summary>
public sealed unsafe class PdfPage
{
    private readonly PdfDocument _document;

    internal PdfPage(PdfDocument document, int index, double width, double height)
    {
        _document = document;
        Index = index;
        Width = width;
        Height = height;
    }

    /// <summary>Page number, starting at 0.</summary>
    public int Index { get; }

    /// <summary>Visible width in points (1/72 inch), after crop box and rotation.</summary>
    public double Width { get; }

    /// <summary>Visible height in points (1/72 inch), after crop box and rotation.</summary>
    public double Height { get; }

    /// <summary>The bitmap size, in pixels, that rendering at <paramref name="dpi"/> produces.</summary>
    /// <exception cref="AtelierException">The DPI is out of range or the bitmap would be too large.</exception>
    public (int Width, int Height) GetPixelSize(double dpi)
    {
        uint w = 0, h = 0;
        using (var doc = _document.Acquire())
        {
            Native.Check(NativeMethods.atelier_page_pixel_size(doc.Ptr, (uint)Index, dpi / 72.0, &w, &h));
        }
        return ((int)w, (int)h);
    }

    /// <summary>Renders the page to a new pixel array.</summary>
    /// <exception cref="AtelierException">The engine could not render the page.</exception>
    public RenderedPage Render(RenderOptions? options = null)
    {
        options ??= RenderOptions.Default;
        var (width, height) = GetPixelSize(options.Dpi);
        int stride = width * 4;
        byte[] pixels = GC.AllocateUninitializedArray<byte>(checked(stride * height));
        RenderInto(pixels, stride, options);
        return new RenderedPage(width, height, stride, options.Format, pixels);
    }

    /// <summary>
    /// Renders the page into an existing buffer, such as a pooled array. The
    /// buffer must hold <c>stride * height</c> bytes for the size from
    /// <see cref="GetPixelSize"/>, and <paramref name="stride"/> must be at
    /// least <c>width * 4</c>. Bytes past each row's pixels are left alone.
    /// </summary>
    /// <exception cref="AtelierException">The buffer is too small or the engine could not render the page.</exception>
    public void RenderInto(Span<byte> buffer, int stride, RenderOptions? options = null)
    {
        fixed (byte* p = buffer)
        {
            RenderInto((IntPtr)p, buffer.Length, stride, options);
        }
    }

    /// <summary>
    /// Renders the page into native memory, such as WPF's
    /// <c>WriteableBitmap.BackBuffer</c> (with <c>Format = Bgra8Premultiplied</c>
    /// and <c>stride = BackBufferStride</c>).
    /// </summary>
    /// <exception cref="AtelierException">The buffer is too small or the engine could not render the page.</exception>
    public void RenderInto(IntPtr buffer, int length, int stride, RenderOptions? options = null)
    {
        options ??= RenderOptions.Default;
        ArgumentOutOfRangeException.ThrowIfNegative(length);
        ArgumentOutOfRangeException.ThrowIfNegative(stride);
        using var doc = _document.Acquire();
        Native.Check(NativeMethods.atelier_render_page(
            doc.Ptr,
            (uint)Index,
            options.Scale,
            options.BackgroundArgb,
            (int)options.Format,
            (byte*)buffer,
            (nuint)stride,
            (nuint)length));
    }
}
