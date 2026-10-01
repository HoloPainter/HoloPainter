[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$IdentityName,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Publisher,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$PublisherDisplayName,

    [string]$OutputPath,

    [string]$MakeAppxPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'target'))
$msixRoot = Join-Path $targetRoot 'msix'
$targetPrefix = $targetRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar

function Get-MsixVersionFromCargoToml([string]$CargoTomlPath) {
    $cargoToml = Get-Content -LiteralPath $CargoTomlPath -Raw
    $packageSection = [regex]::Match(
        $cargoToml,
        '(?ms)^\s*\[package\]\s*$\s*(?<Body>.*?)(?=^\s*\[|\z)'
    )
    if (-not $packageSection.Success) {
        throw "Cargo.toml does not contain a [package] section: $CargoTomlPath"
    }

    $versionEntry = [regex]::Match(
        $packageSection.Groups['Body'].Value,
        '(?m)^\s*version\s*=\s*["''](?<Version>[0-9]+\.[0-9]+\.[0-9]+)["'']\s*(?:#.*)?$'
    )
    if (-not $versionEntry.Success) {
        throw "Cargo.toml package version must contain exactly three numeric components: $CargoTomlPath"
    }

    $cargoVersion = $versionEntry.Groups['Version'].Value
    $components = @($cargoVersion.Split('.') | ForEach-Object { [uint32]$_ })
    $outOfRangeComponents = @($components | Where-Object { $_ -gt 65535 })
    if ($components[0] -eq 0 -or $outOfRangeComponents.Count -ne 0) {
        throw "Cargo.toml package version cannot be represented as an MSIX version: $cargoVersion"
    }
    return "$cargoVersion.0"
}

$Version = Get-MsixVersionFromCargoToml (Join-Path $repositoryRoot 'Cargo.toml')

function Assert-PathBelowTarget([string]$Path, [string]$Purpose) {
    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not $resolved.StartsWith($targetPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to use $Purpose outside target: $resolved"
    }
    return $resolved
}

$stageRoot = Assert-PathBelowTarget (Join-Path $msixRoot 'stage') 'MSIX staging directory'
$verificationRoot = Assert-PathBelowTarget (Join-Path $msixRoot 'verify') 'MSIX verification directory'

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $msixRoot "HoloPainter3D_${Version}_x64.msix"
}
$OutputPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = Split-Path -Parent $OutputPath
if ([string]::IsNullOrWhiteSpace($outputDirectory)) {
    throw "MSIX output path has no parent directory: $OutputPath"
}

$assetsSource = Join-Path $repositoryRoot 'packaging\msix\Assets'
$requiredAssets = @(
    'StoreLogo.png',
    'Square44x44Logo.png',
    'Square150x150Logo.png'
)
if (-not (Test-Path -LiteralPath $assetsSource -PathType Container)) {
    throw "MSIX asset directory not found: $assetsSource"
}
foreach ($asset in $requiredAssets) {
    $assetPath = Join-Path $assetsSource $asset
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
        throw @"
Required MSIX asset is missing:
  $assetPath

Generate MSIX assets first:

  .\scripts\generate-msix-assets.ps1 ``
      -InputSvg .\assets\app_icon\app_icon.svg
"@
    }
}

function ConvertTo-XmlText([string]$Value) {
    return [System.Security.SecurityElement]::Escape($Value)
}

function Find-MakeAppx {
    if (-not [string]::IsNullOrWhiteSpace($MakeAppxPath)) {
        $candidate = [System.IO.Path]::GetFullPath($MakeAppxPath)
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "MakeAppx.exe was not found: $candidate"
        }
        return $candidate
    }

    $kitsBin = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $sdkDirectories = Get-ChildItem -LiteralPath $kitsBin -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match '^10\.0\.[0-9]+\.0$' } |
        Sort-Object { [version]$_.Name } -Descending
    foreach ($sdkDirectory in $sdkDirectories) {
        $candidate = Join-Path $sdkDirectory.FullName 'x64\makeappx.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }
    throw 'MakeAppx.exe was not found. Install the Windows 10/11 SDK.'
}

function Test-MsixManifestAssets([string]$ManifestPath, [string]$PackageRoot) {
    [xml]$manifestXml = Get-Content -LiteralPath $ManifestPath -Raw
    $namespaces = New-Object System.Xml.XmlNamespaceManager $manifestXml.NameTable
    $namespaces.AddNamespace('foundation', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
    $namespaces.AddNamespace('uap', 'http://schemas.microsoft.com/appx/manifest/uap/windows10')

    $logoNode = $manifestXml.SelectSingleNode(
        '/foundation:Package/foundation:Properties/foundation:Logo',
        $namespaces
    )
    $visualElements = $manifestXml.SelectSingleNode(
        '/foundation:Package/foundation:Applications/foundation:Application/uap:VisualElements',
        $namespaces
    )
    if ($null -eq $logoNode -or $null -eq $visualElements) {
        throw "Required MSIX icon declarations are missing from: $ManifestPath"
    }

    $references = @(
        @{ Name = 'Logo'; Actual = $logoNode.InnerText; Expected = 'Assets\StoreLogo.png' },
        @{ Name = 'Square44x44Logo'; Actual = $visualElements.GetAttribute('Square44x44Logo'); Expected = 'Assets\Square44x44Logo.png' },
        @{ Name = 'Square150x150Logo'; Actual = $visualElements.GetAttribute('Square150x150Logo'); Expected = 'Assets\Square150x150Logo.png' }
    )
    foreach ($reference in $references) {
        if ($reference.Actual -cne $reference.Expected) {
            throw "Unexpected $($reference.Name) in AppxManifest.xml: $($reference.Actual)"
        }
        $assetPath = Join-Path $PackageRoot $reference.Actual
        if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
            throw "AppxManifest.xml references a missing asset: $assetPath"
        }
    }
}

$makeAppx = Find-MakeAppx
New-Item -ItemType Directory -Force -Path $msixRoot | Out-Null
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $stageRoot | Out-Null

Push-Location $repositoryRoot
try {
    & cargo build --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$executable = Join-Path $targetRoot 'x86_64-pc-windows-msvc\release\holopainter.exe'
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "Release executable was not produced: $executable"
}
Copy-Item -LiteralPath $executable -Destination (Join-Path $stageRoot 'holopainter.exe')

$manifestTemplate = Get-Content -LiteralPath (Join-Path $repositoryRoot 'packaging\msix\AppxManifest.xml.in') -Raw
$manifest = $manifestTemplate.Replace('__IDENTITY_NAME__', (ConvertTo-XmlText $IdentityName))
$manifest = $manifest.Replace('__PUBLISHER__', (ConvertTo-XmlText $Publisher))
$manifest = $manifest.Replace('__PUBLISHER_DISPLAY_NAME__', (ConvertTo-XmlText $PublisherDisplayName))
$manifest = $manifest.Replace('__VERSION__', $Version)
if ($manifest.Contains('__')) {
    throw 'The generated AppxManifest.xml still contains an unresolved placeholder.'
}
$manifestPath = Join-Path $stageRoot 'AppxManifest.xml'
[System.IO.File]::WriteAllText($manifestPath, $manifest, (New-Object System.Text.UTF8Encoding $false))

$samplesSource = Join-Path $repositoryRoot 'packaging\msix\Samples'
$samplesDest   = Join-Path $stageRoot "Samples"

if (Test-Path $samplesSource) {
    New-Item `
        -ItemType Directory `
        -Path $samplesDest `
        -Force | Out-Null

    Copy-Item `
        -Path (Join-Path $samplesSource "*") `
        -Destination $samplesDest `
        -Recurse `
        -Force

    Write-Host "MSIX samples copied:"
    Write-Host "  $samplesSource"
}

$assetsDestination = Join-Path $stageRoot 'Assets'
New-Item -ItemType Directory -Force -Path $assetsDestination | Out-Null
Copy-Item -Path (Join-Path $assetsSource '*') -Destination $assetsDestination -Recurse -Force

Write-Host 'MSIX assets:'
foreach ($asset in $requiredAssets) {
    Write-Host "  $asset"
}
Test-MsixManifestAssets $manifestPath $stageRoot

& $makeAppx pack /o /h SHA256 /d $stageRoot /p $OutputPath | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "MakeAppx failed with exit code $LASTEXITCODE"
}

if (Test-Path -LiteralPath $verificationRoot) {
    Remove-Item -LiteralPath $verificationRoot -Recurse -Force
}
try {
    & $makeAppx unpack /o /p $OutputPath /d $verificationRoot | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "MakeAppx unpack verification failed with exit code $LASTEXITCODE"
    }
    Test-MsixManifestAssets (Join-Path $verificationRoot 'AppxManifest.xml') $verificationRoot
    Write-Host 'MSIX package contents verified.'
} finally {
    if (Test-Path -LiteralPath $verificationRoot) {
        Remove-Item -LiteralPath $verificationRoot -Recurse -Force
    }
}

Write-Output $OutputPath
