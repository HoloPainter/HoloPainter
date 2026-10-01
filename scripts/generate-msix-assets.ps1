param(
    [Parameter(Mandatory = $true)]
    [string]$InputSvg,

    [string]$OutputDir = "packaging/msix/Assets",

    [string]$InkscapePath = ""
)

$ErrorActionPreference = "Stop"

# ------------------------------------------------------------
# Resolve input SVG
# ------------------------------------------------------------

$InputSvg = (Resolve-Path $InputSvg).Path

if (-not (Test-Path $InputSvg)) {
    throw "SVG file not found: $InputSvg"
}

# ------------------------------------------------------------
# Find Inkscape
# ------------------------------------------------------------

if ([string]::IsNullOrWhiteSpace($InkscapePath)) {
    $command = Get-Command "inkscape" -ErrorAction SilentlyContinue

    if ($command) {
        $InkscapePath = $command.Source
    }
    else {
        $candidates = @(
            "$env:ProgramFiles\Inkscape\bin\inkscape.exe",
            "${env:ProgramFiles(x86)}\Inkscape\bin\inkscape.exe"
        )

        foreach ($candidate in $candidates) {
            if ($candidate -and (Test-Path $candidate)) {
                $InkscapePath = $candidate
                break
            }
        }
    }
}

if ([string]::IsNullOrWhiteSpace($InkscapePath) -or
    -not (Test-Path $InkscapePath)) {
    throw @"
Inkscape was not found.

Install Inkscape or specify its path:

  .\scripts\generate-msix-assets.ps1 `
      -InputSvg path\to\holopainter.svg `
      -InkscapePath "C:\Program Files\Inkscape\bin\inkscape.exe"
"@
}

Write-Host "Inkscape: $InkscapePath"
Write-Host "Input SVG: $InputSvg"

# ------------------------------------------------------------
# Prepare output directory
# ------------------------------------------------------------

New-Item `
    -ItemType Directory `
    -Path $OutputDir `
    -Force | Out-Null

$OutputDir = (Resolve-Path $OutputDir).Path

# ------------------------------------------------------------
# SVG -> PNG helper
# ------------------------------------------------------------

function Export-SvgPng {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Width,

        [Parameter(Mandatory = $true)]
        [int]$Height,

        [Parameter(Mandatory = $true)]
        [string]$OutputFile
    )

    $outputPath = Join-Path $OutputDir $OutputFile

    Write-Host ("  {0,-48} {1}x{2}" -f $OutputFile, $Width, $Height)

    # Remove stale output so success cannot be confused with an old file.
    Remove-Item $outputPath -Force -ErrorAction SilentlyContinue

    $arguments = @(
        "--export-type=png"
        "--export-filename=`"$outputPath`""
        "--export-width=$Width"
        "--export-height=$Height"
        "--export-area-page"
        "`"$InputSvg`""
    )

    $process = Start-Process `
        -FilePath $InkscapePath `
        -ArgumentList $arguments `
        -Wait `
        -PassThru `
        -NoNewWindow

    if ($process.ExitCode -ne 0) {
        throw @"
Inkscape failed while generating:
  $OutputFile

Exit code:
  $($process.ExitCode)

Command:
  "$InkscapePath" $($arguments -join ' ')
"@
    }

    if (-not (Test-Path $outputPath)) {
        throw @"
Inkscape exited successfully, but the output file was not generated:
  $outputPath
"@
    }
}
# ------------------------------------------------------------
# MSIX scale assets
#
# Windows scale factors:
#   100%  125%  150%  200%  400%
# ------------------------------------------------------------

$scales = @(100, 125, 150, 200, 400)

$assets = @(
    @{
        Name   = "Square44x44Logo"
        Width  = 44
        Height = 44
    },
    @{
        Name   = "Square150x150Logo"
        Width  = 150
        Height = 150
    },
    @{
        Name   = "StoreLogo"
        Width  = 50
        Height = 50
    }
)

Write-Host ""
Write-Host "Generating MSIX scale assets..."

foreach ($asset in $assets) {
    foreach ($scale in $scales) {
        $width = [int][Math]::Round(
            $asset.Width * $scale / 100.0
        )

        $height = [int][Math]::Round(
            $asset.Height * $scale / 100.0
        )

        $fileName = "{0}.scale-{1}.png" -f `
            $asset.Name,
            $scale

        Export-SvgPng `
            -Width $width `
            -Height $height `
            -OutputFile $fileName
    }
}

# ------------------------------------------------------------
# Base / fallback files
#
# These are useful for a simple manifest such as:
#
#   Square44x44Logo="Assets\Square44x44Logo.png"
# ------------------------------------------------------------

Write-Host ""
Write-Host "Generating base assets..."

Export-SvgPng `
    -Width 44 `
    -Height 44 `
    -OutputFile "Square44x44Logo.png"

Export-SvgPng `
    -Width 150 `
    -Height 150 `
    -OutputFile "Square150x150Logo.png"

Export-SvgPng `
    -Width 50 `
    -Height 50 `
    -OutputFile "StoreLogo.png"

# ------------------------------------------------------------
# Target-size variants
#
# Used by Windows in places such as Start/Search/taskbar-like
# shell surfaces. Generate them from the same master SVG.
# ------------------------------------------------------------

$targetSizes = @(16, 20, 24, 30, 32, 36, 40, 44, 48, 60, 64, 72, 80, 96, 256)

Write-Host ""
Write-Host "Generating target-size assets..."

foreach ($size in $targetSizes) {
    Export-SvgPng `
        -Width $size `
        -Height $size `
        -OutputFile "Square44x44Logo.targetsize-$size.png"
}

Write-Host ""
Write-Host "MSIX assets generated successfully:"
Write-Host "  $OutputDir"