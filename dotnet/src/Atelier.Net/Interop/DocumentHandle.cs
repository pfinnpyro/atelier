using System.Runtime.InteropServices;

namespace Atelier.Interop;

/// <summary>Owns a native document and frees it exactly once, even if a render
/// on another thread is still running when the document is disposed.</summary>
internal sealed unsafe class DocumentHandle : SafeHandle
{
    public DocumentHandle() : base(IntPtr.Zero, ownsHandle: true) { }

    public override bool IsInvalid => handle == IntPtr.Zero;

    internal void Set(AtelierDocument* doc) => SetHandle((IntPtr)doc);

    protected override bool ReleaseHandle()
    {
        NativeMethods.atelier_document_free((AtelierDocument*)handle);
        return true;
    }

    /// <summary>Holds a reference for the length of a native call, so the
    /// document cannot be freed underneath it. Use with <c>using</c>.</summary>
    internal Lease Acquire()
    {
        bool added = false;
        try
        {
            DangerousAddRef(ref added);
        }
        catch (ObjectDisposedException)
        {
            throw new ObjectDisposedException(nameof(PdfDocument));
        }
        return new Lease(this, (AtelierDocument*)DangerousGetHandle());
    }

    internal readonly ref struct Lease
    {
        private readonly DocumentHandle _owner;

        internal Lease(DocumentHandle owner, AtelierDocument* ptr)
        {
            _owner = owner;
            Ptr = ptr;
        }

        public AtelierDocument* Ptr { get; }

        public void Dispose() => _owner.DangerousRelease();
    }
}
