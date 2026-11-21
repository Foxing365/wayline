[CmdletBinding(DefaultParameterSetName = 'Install', SupportsShouldProcess)]
param(
    [Parameter(Mandatory, ParameterSetName = 'Install')] [string] $PackagePath,
    [Parameter(Mandatory, ParameterSetName = 'Install')] [string] $InstallDirectory,
    [Parameter(Mandatory, ParameterSetName = 'Remove')] [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'
$name = 'Wayline.ExplorerMenu'

if ($Uninstall) {
    Get-AppxPackage -Name $name | Where-Object Name -eq $name | ForEach-Object {
        if ($PSCmdlet.ShouldProcess($_.PackageFullName, 'Remove Wayline Explorer command')) {
            Remove-AppxPackage -Package $_.PackageFullName
        }
    }
    return
}

if ([environment]::OSVersion.Version.Build -lt 22000) {
    throw 'The first-level Explorer menu requires Windows 11. The desktop app still has its classic menu and Send To entry.'
}
$external = Get-Item -LiteralPath $InstallDirectory
if (-not $external.PSIsContainer -or $external.PSProvider.Name -ne 'FileSystem') {
    throw 'The external location must be the installed Wayline directory.'
}
foreach ($file in @('wayline-desktop.exe', 'wayline-explorer.dll', 'Assets\Wayline.png')) {
    if (-not (Test-Path -LiteralPath (Join-Path $external.FullName $file) -PathType Leaf)) {
        throw "$file is missing from the installed Wayline directory. Install the package files first."
    }
}
$package = Get-Item -LiteralPath $PackagePath
if ($package.PSIsContainer -or $package.Extension -ne '.msix') {
    throw 'Select the signed Wayline.ExplorerMenu.msix file.'
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::OpenRead($package.FullName)
try {
    $metadata = $archive.GetEntry('AppxManifest.xml')
    if (-not $metadata -or -not $archive.GetEntry('AppxSignature.p7x')) {
        throw 'The Explorer identity MSIX must be signed before registration.'
    }
    $reader = [IO.StreamReader]::new($metadata.Open())
    try { [xml] $manifest = $reader.ReadToEnd() } finally { $reader.Dispose() }
    if ($manifest.Package.Identity.Name -ne $name) { throw 'This MSIX is not the Wayline Explorer identity.' }
} finally {
    $archive.Dispose()
}

# Register the new version directly so a failed upgrade leaves its predecessor usable.
if ($PSCmdlet.ShouldProcess($name, 'Register Wayline Explorer command for the current user')) {
    Add-AppxPackage -Path $package.FullName -ExternalLocation $external.FullName
    Get-AppxPackage -Name $name | Where-Object Name -eq $name
}
