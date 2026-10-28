namespace Atelier;

/// <summary>Why the engine refused a call.</summary>
public enum AtelierError
{
    /// <summary>An argument was out of range, such as a DPI of 0.</summary>
    InvalidArgument = 1,
    /// <summary>The page number is past the last page.</summary>
    PageOutOfRange = 2,
    /// <summary>The data could not be read as a PDF, even after repair.</summary>
    InvalidPdf = 3,
    /// <summary>The file is encrypted. Decryption comes in a later version.</summary>
    Encrypted = 4,
    /// <summary>The file uses something this version does not handle yet.</summary>
    Unsupported = 5,
    /// <summary>The buffer passed to a render call is too small.</summary>
    BufferTooSmall = 6,
    /// <summary>The bitmap would be larger than the engine allows (268 million pixels).</summary>
    TooLarge = 7,
    /// <summary>A bug in the engine. Please report it with the file.</summary>
    Internal = 99,
}

/// <summary>Thrown when the Atelier engine refuses a call.</summary>
public sealed class AtelierException : Exception
{
    internal AtelierException(AtelierError error, string message)
        : base(string.IsNullOrEmpty(message) ? error.ToString() : message)
    {
        Error = error;
    }

    /// <summary>The kind of failure.</summary>
    public AtelierError Error { get; }
}
