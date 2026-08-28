<#
.SYNOPSIS
  Renders the Store / MSIX logo set from src/icons/256x256.png.

  Tiles get the icon at two thirds of the tile with transparent padding
  (Microsoft's tile guidance); the app-list and taskbar sizes are full-bleed.
  Both the qualified names (scale-*, targetsize-*) and the plain names are
  written, so the manifest works with or without resources.pri.
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot "..\..\src\icons\256x256.png"),
    [string]$OutDir = (Join-Path $PSScriptRoot "Assets")
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile((Resolve-Path $Source))
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function Render([string]$Name, [int]$W, [int]$H, [double]$IconFraction) {
    $bmp = New-Object System.Drawing.Bitmap $W, $H, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $size = [int][math]::Round([math]::Min($W, $H) * $IconFraction)
    $x = [int](($W - $size) / 2); $y = [int](($H - $size) / 2)
    $g.DrawImage($src, (New-Object System.Drawing.Rectangle $x, $y, $size, $size))
    $g.Dispose()
    $bmp.Save((Join-Path $OutDir $Name), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

# name, width, height, icon fraction  -- at scale-100 and scale-200
$tiles = @(
    @("Square44x44Logo",   44,  44,  1.0),
    @("Square71x71Logo",   71,  71,  0.66),
    @("Square150x150Logo", 150, 150, 0.66),
    @("Square310x310Logo", 310, 310, 0.66),
    @("Wide310x150Logo",   310, 150, 0.66),
    @("StoreLogo",         50,  50,  1.0),
    @("SplashScreen",      620, 300, 0.5)
)
foreach ($t in $tiles) {
    $name, $w, $h, $f = $t
    Render "$name.png" $w $h $f
    Render "$name.scale-100.png" $w $h $f
    Render "$name.scale-200.png" ($w * 2) ($h * 2) $f
}
# App list / taskbar icon at exact pixel sizes, plated and unplated.
foreach ($s in 16, 24, 32, 48, 256) {
    Render "Square44x44Logo.targetsize-$s.png" $s $s 1.0
    Render "Square44x44Logo.targetsize-${s}_altform-unplated.png" $s $s 1.0
}
$src.Dispose()
Write-Host ("wrote {0} assets to {1}" -f (Get-ChildItem $OutDir -Filter *.png).Count, $OutDir)
