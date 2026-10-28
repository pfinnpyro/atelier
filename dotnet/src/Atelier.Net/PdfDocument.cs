using Atelier.Interop;

namespace Atelier;

/// <summary>
/// An open PDF file. Dispose it when done to free the native memory it holds.
/// One document can be used from several threads at once, for example to
/// render different pages in parallel.
/// </summary>
public sealed unsafe class PdfDocument : IDisposable
{
    private readonly DocumentHandle _handle;
    private readonly PdfPage[] _pages;

    private PdfDocument(DocumentHandle handle)
    {
        _handle = handle;
        AtelierDocumentInfo info = default;
        using (var doc = _handle.Acquire())
        {
            Native.Check(NativeMethods.atelier_document_info(doc.Ptr, &info));
        }
        PageCount = checked((int)info.page_count);
        IsEncrypted = info.encrypted != 0;
        WasRepaired = info.repaired != 0;
        Version = info.version == 0 ? string.Empty : $"{info.version / 10}.{info.version % 10}";
        _pages = new PdfPage[PageCount];
    }

    /// <summary>The version of the native Atelier engine.</summary>
    public static string EngineVersion =>
        System.Runtime.InteropServices.Marshal.PtrToStringUTF8((IntPtr)NativeMethods.atelier_version()) ?? string.Empty;

    /// <summary>Opens a PDF from a file.</summary>
    /// <exception cref="AtelierException">The file is not a readable PDF.</exception>
    public static PdfDocument Open(string path) => Open(File.ReadAllBytes(path));

    /// <summary>Opens a PDF from a stream, reading it to the end.</summary>
    /// <exception cref="AtelierException">The data is not a readable PDF.</exception>
    public static PdfDocument Open(Stream stream)
    {
        ArgumentNullException.ThrowIfNull(stream);
        using var copy = new MemoryStream();
        stream.CopyTo(copy);
        return Open(copy.GetBuffer().AsSpan(0, (int)copy.Length));
    }

    /// <summary>Opens a PDF from bytes in memory. The bytes are copied, so the
    /// span may be reused once this returns.</summary>
    /// <exception cref="AtelierException">The data is not a readable PDF.</exception>
    public static PdfDocument Open(ReadOnlySpan<byte> data)
    {
        var handle = new DocumentHandle();
        AtelierDocument* doc = null;
        AtelierStatus status;
        fixed (byte* p = data)
        {
            // A non-null pointer even for an empty span, so the engine reports
            // "not a PDF" rather than a null argument.
            byte empty = 0;
            status = NativeMethods.atelier_document_open(p == null ? &empty : p, (nuint)data.Length, &doc);
        }
        handle.Set(doc);
        try
        {
            Native.Check(status);
            return new PdfDocument(handle);
        }
        catch
        {
            handle.Dispose();
            throw;
        }
    }

    /// <summary>Number of pages.</summary>
    public int PageCount { get; }

    /// <summary>PDF version from the file header, such as "1.7". Empty when the header is missing.</summary>
    public string Version { get; }

    /// <summary>True when the file is encrypted. Encrypted files do not render yet.</summary>
    public bool IsEncrypted { get; }

    /// <summary>True when broken cross-reference data was rebuilt by scanning the file.</summary>
    public bool WasRepaired { get; }

    /// <summary>The pages in order.</summary>
    public IReadOnlyList<PdfPage> Pages => Enumerable.Range(0, PageCount).Select(GetPage).ToArray();

    /// <summary>A page by number, starting at 0.</summary>
    /// <exception cref="ArgumentOutOfRangeException">No such page.</exception>
    public PdfPage GetPage(int index)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(index);
        ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual(index, PageCount);
        lock (_pages)
        {
            if (_pages[index] is { } page)
            {
                return page;
            }
            double w = 0, h = 0;
            using (var doc = _handle.Acquire())
            {
                Native.Check(NativeMethods.atelier_page_size(doc.Ptr, (uint)index, &w, &h));
            }
            return _pages[index] = new PdfPage(this, index, w, h);
        }
    }

    internal DocumentHandle.Lease Acquire() => _handle.Acquire();

    /// <summary>Frees the native document. Renders already running finish first.</summary>
    public void Dispose() => _handle.Dispose();
}
