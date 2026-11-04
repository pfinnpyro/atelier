param([ValidateSet('List', 'Sign', 'Library')][string]$Mode = 'List')
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Security

function Write-DetachedSignature($Certificate, [string]$InputPath, [string]$OutputPath) {
    if (-not $Certificate.HasPrivateKey) { throw 'The selected certificate has no private key.' }
    $now = [DateTime]::Now
    if ($now -lt $Certificate.NotBefore -or $now -ge $Certificate.NotAfter) {
        throw 'The selected certificate is outside its validity period.'
    }
    foreach ($extension in $Certificate.Extensions) {
        if ($extension -is [System.Security.Cryptography.X509Certificates.X509KeyUsageExtension] -and
            ($extension.KeyUsages -band [System.Security.Cryptography.X509Certificates.X509KeyUsageFlags]::DigitalSignature) -eq 0) {
            throw 'The selected certificate does not permit digital signatures.'
        }
    }
    $content = [System.Security.Cryptography.Pkcs.ContentInfo]::new([IO.File]::ReadAllBytes($InputPath))
    $cms = [System.Security.Cryptography.Pkcs.SignedCms]::new($content, $true)
    $signer = [System.Security.Cryptography.Pkcs.CmsSigner]::new($Certificate)
    $signer.DigestAlgorithm = [System.Security.Cryptography.Oid]::new('2.16.840.1.101.3.4.2.1')
    # Include only the signer; avoid implicit network chain retrieval during signing.
    $signer.IncludeOption = [System.Security.Cryptography.X509Certificates.X509IncludeOption]::EndCertOnly
    $cms.ComputeSignature($signer, $false)
    [IO.File]::WriteAllBytes($OutputPath, $cms.Encode())
}
function Encode-Text([string]$Text) {
    [BitConverter]::ToString([Text.Encoding]::UTF8.GetBytes($Text)).Replace('-', '')
}
if ($Mode -eq 'Library') { return }
$store = $null
try {
    $store = [System.Security.Cryptography.X509Certificates.X509Store]::new('My', 'CurrentUser')
    $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadOnly)
    if ($Mode -eq 'List') {
        foreach ($certificate in $store.Certificates) {
            if ($certificate.HasPrivateKey) {
                $fields = @($certificate.Thumbprint, (Encode-Text $certificate.Subject), (Encode-Text $certificate.Issuer), $certificate.NotAfter.ToUniversalTime().ToString('yyyy-MM-dd'))
                [Console]::Out.WriteLine(($fields -join "`t"))
            }
        }
    } else {
        $thumbprint = $env:ATELIER_NATIVE_THUMBPRINT
        if ($thumbprint -notmatch '\A[0-9A-Fa-f]{40}\z') { throw 'Invalid certificate thumbprint.' }
        $certificates = $store.Certificates.Find([System.Security.Cryptography.X509Certificates.X509FindType]::FindByThumbprint, $thumbprint, $false)
        if ($certificates.Count -ne 1) { throw 'The selected certificate is no longer uniquely available in the current user Personal store. Refresh identities.' }
        Write-DetachedSignature $certificates[0] $env:ATELIER_NATIVE_INPUT $env:ATELIER_NATIVE_OUTPUT
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
} finally {
    if ($null -ne $store) { $store.Close() }
}
