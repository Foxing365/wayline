[CmdletBinding(DefaultParameterSetName = 'Signed')]
param(
    [Parameter(Mandatory)] [string] $OutputDirectory,
    [Parameter(Mandatory)] [string] $SdkBin,
    [string] $Compiler = 'cl.exe',
    [version] $PackageVersion = '1.0.0.0',
    [Parameter(Mandatory, ParameterSetName = 'Signed')]
    [ValidatePattern('^[a-fA-F0-9]{40}$')] [string] $SigningThumbprint,
    [Parameter(Mandatory, ParameterSetName = 'Unsigned')] [switch] $Unsigned,
    [Parameter(ParameterSetName = 'Unsigned')] [string] $Publisher = 'CN=Wayline'
)

$ErrorActionPreference = 'Stop'

if (-not [System.Environment]::Is64BitOperatingSystem) {
    throw 'The Wayline Explorer command targets Windows 11 x64.'
}
if ($PackageVersion.Revision -lt 0 -or $PackageVersion.Major -lt 1 -or
    ($PackageVersion.ToString().Split('.') | Where-Object { [int]$_ -gt 65535 })) {
    throw 'MSIX package version must have four components in 0–65535 and a nonzero major version.'
}

$compilerPath = (Get-Command -Name $Compiler -CommandType Application).Source
$makeAppx = (Get-Item -LiteralPath (Join-Path $SdkBin 'MakeAppx.exe')).FullName
if (-not $Unsigned) {
    $signTool = (Get-Item -LiteralPath (Join-Path $SdkBin 'SignTool.exe')).FullName
    $signer = Get-Item -LiteralPath "Cert:\CurrentUser\My\$SigningThumbprint"
    if (-not $signer.HasPrivateKey) { throw 'The signing certificate needs a private key.' }
    $Publisher = $signer.Subject
}

$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$dll = Join-Path $output 'wayline-explorer.dll'
$package = Join-Path $output 'Wayline.ExplorerMenu.msix'
$externalAssets = Join-Path $output 'Assets'
$externalLogo = Join-Path $externalAssets 'Wayline.png'
New-Item -Path $externalAssets -ItemType Directory -Force | Out-Null

$source = Join-Path $PSScriptRoot 'command.cpp'
$definition = Join-Path $PSScriptRoot 'command.def'
$name = [IO.Path]::GetFileNameWithoutExtension($compilerPath)
if ($name -in @('cl', 'clang-cl')) {
    & $compilerPath /nologo /std:c++17 /utf-8 /EHsc /O2 /MT /LD /DUNICODE /D_UNICODE /W4 /WX `
        "/Fo$(Join-Path $output 'wayline-explorer.obj')" $source shlwapi.lib ole32.lib uuid.lib `
        /link /MACHINE:X64 "/DEF:$definition" "/OUT:$dll" "/IMPLIB:$(Join-Path $output 'wayline-explorer.lib')"
} else {
    & $compilerPath -std=c++17 -shared -static -s -O2 -DUNICODE -D_UNICODE -Wall -Wextra -Werror `
        $source $definition -o $dll -lshlwapi -lole32 -luuid
}
if ($LASTEXITCODE -ne 0) { throw 'Could not build the Wayline Explorer DLL.' }

# Read the PE machine field so an x86 toolchain cannot produce a mislabeled x64 package.
$stream = [IO.File]::OpenRead($dll)
try {
    $reader = [IO.BinaryReader]::new($stream)
    if ($reader.ReadUInt16() -ne 0x5a4d) { throw 'The Explorer DLL is not a PE file.' }
    $stream.Position = 0x3c
    $stream.Position = $reader.ReadUInt32()
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'The Explorer DLL must target x64. Select an x64 C++ compiler.'
    }
} finally {
    $stream.Dispose()
}

$stage = Join-Path $output ('sparse-' + [guid]::NewGuid().ToString('N'))
$assetDirectory = Join-Path $stage 'Assets'
$manifestPath = Join-Path $stage 'AppxManifest.xml'
$logoPath = Join-Path $assetDirectory 'Wayline.png'
New-Item -Path $assetDirectory -ItemType Directory | Out-Null
try {
    [xml] $manifest = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw
    $manifest.Package.Identity.Publisher = $Publisher
    $manifest.Package.Identity.Version = $PackageVersion.ToString()
    $manifest.Save($manifestPath)
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot '..\desktop\assets\icon.png') -Destination $externalLogo
    Copy-Item -LiteralPath $externalLogo -Destination $logoPath

    # The referenced EXE and DLL live beside each other at the external install path.
    & $makeAppx pack /o /nv /d $stage /p $package
    if ($LASTEXITCODE -ne 0) { throw 'Could not pack the Explorer identity MSIX.' }
} finally {
    foreach ($file in @($manifestPath, $logoPath)) {
        if (Test-Path -LiteralPath $file -PathType Leaf) { Remove-Item -LiteralPath $file }
    }
    foreach ($directory in @($assetDirectory, $stage)) {
        if (-not (Get-ChildItem -LiteralPath $directory -Force)) { Remove-Item -LiteralPath $directory }
    }
}
if (-not $Unsigned) {
    & $signTool sign /fd SHA256 /sha1 $SigningThumbprint $package
    if ($LASTEXITCODE -ne 0) { throw 'Could not sign the Explorer identity MSIX.' }
}

Get-FileHash -Algorithm SHA256 -LiteralPath $dll, $package, $externalLogo
