namespace Atelier.Tests;

public class PdfDocumentTests
{
    [Fact]
    public void ReportsEngineVersion() => Assert.Matches(@"^\d+\.\d+\.\d+", PdfDocument.EngineVersion);

    [Fact]
    public void ReadsDocumentFacts()
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        Assert.Equal(2, doc.PageCount);
        Assert.Equal("1.7", doc.Version);
        Assert.False(doc.IsEncrypted);
        Assert.False(doc.WasRepaired);
    }

    [Fact]
    public void PageSizesAccountForRotation()
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        Assert.Equal((100.0, 50.0), (doc.Pages[0].Width, doc.Pages[0].Height));
        Assert.Equal((100.0, 200.0), (doc.Pages[1].Width, doc.Pages[1].Height));
        Assert.Equal((200, 100), doc.GetPage(0).GetPixelSize(144));
    }

    [Theory]
    [InlineData(PdfPixelFormat.Rgba8, new byte[] { 255, 0, 0, 255 })]
    [InlineData(PdfPixelFormat.Rgba8Premultiplied, new byte[] { 255, 0, 0, 255 })]
    [InlineData(PdfPixelFormat.Bgra8Premultiplied, new byte[] { 0, 0, 255, 255 })]
    public void RendersPixels(PdfPixelFormat format, byte[] red)
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        var page = doc.GetPage(0).Render(new RenderOptions { Dpi = 72, Format = format });
        Assert.Equal((100, 50, 400), (page.Width, page.Height, page.Stride));
        Assert.Equal(red, Pixel(page, 10, 10));
        Assert.Equal(new byte[] { 255, 255, 255, 255 }, Pixel(page, 60, 40));
    }

    [Fact]
    public void RendersIntoPaddedBuffer()
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        const int stride = 416;
        var buffer = new byte[stride * 50];
        doc.GetPage(0).RenderInto(buffer, stride, new RenderOptions { Dpi = 72, BackgroundArgb = 0 });
        Assert.Equal(0, buffer[40 * stride + 60 * 4 + 3]);
        Assert.Equal(255, buffer[10 * stride + 10 * 4 + 3]);
    }

    [Fact]
    public void RendersPagesInParallel()
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        var sizes = Enumerable.Range(0, 16).AsParallel()
            .Select(i => doc.GetPage(i % 2).Render().Width)
            .ToArray();
        Assert.All(sizes, w => Assert.True(w > 0));
    }

    [Fact]
    public void RejectsNonPdf()
    {
        var ex = Assert.Throws<AtelierException>(() => PdfDocument.Open("hello"u8));
        Assert.NotEqual(AtelierError.Internal, ex.Error);
        Assert.NotEmpty(ex.Message);
        Assert.Throws<AtelierException>(() => PdfDocument.Open(ReadOnlySpan<byte>.Empty));
    }

    [Fact]
    public void ReportsBadArguments()
    {
        using var doc = PdfDocument.Open(SamplePdf.TwoPages());
        var page = doc.GetPage(0);
        Assert.Equal(AtelierError.InvalidArgument,
            Assert.Throws<AtelierException>(() => page.Render(new RenderOptions { Dpi = 0 })).Error);
        Assert.Equal(AtelierError.BufferTooSmall,
            Assert.Throws<AtelierException>(() => page.RenderInto(new byte[10], 400)).Error);
        Assert.Throws<ArgumentOutOfRangeException>(() => doc.GetPage(2));
    }

    [Fact]
    public void ThrowsAfterDispose()
    {
        var doc = PdfDocument.Open(SamplePdf.TwoPages());
        var page = doc.GetPage(0);
        doc.Dispose();
        doc.Dispose();
        Assert.Throws<ObjectDisposedException>(() => page.Render());
    }

    [Fact]
    public void OpensStreamsAndFiles()
    {
        using (var doc = PdfDocument.Open(new MemoryStream(SamplePdf.TwoPages())))
        {
            Assert.Equal(2, doc.PageCount);
        }
        string path = Path.GetTempFileName();
        try
        {
            File.WriteAllBytes(path, SamplePdf.TwoPages());
            using var doc = PdfDocument.Open(path);
            Assert.Equal(2, doc.PageCount);
        }
        finally
        {
            File.Delete(path);
        }
    }

    private static byte[] Pixel(RenderedPage page, int x, int y) =>
        page.Pixels.AsSpan(y * page.Stride + x * 4, 4).ToArray();
}
