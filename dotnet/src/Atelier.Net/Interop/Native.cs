using System.Text;

namespace Atelier.Interop;

internal static unsafe class Native
{
    /// <summary>Throws an <see cref="AtelierException"/> for any status but Ok,
    /// with the engine's message for this thread.</summary>
    internal static void Check(AtelierStatus status)
    {
        if (status == AtelierStatus.Ok)
        {
            return;
        }
        throw new AtelierException((AtelierError)(int)status, LastError());
    }

    internal static string LastError()
    {
        int len = checked((int)NativeMethods.atelier_last_error(null, 0));
        if (len == 0)
        {
            return string.Empty;
        }
        byte[] buf = new byte[len + 1];
        fixed (byte* p = buf)
        {
            NativeMethods.atelier_last_error(p, (nuint)buf.Length);
        }
        return Encoding.UTF8.GetString(buf, 0, len);
    }
}
