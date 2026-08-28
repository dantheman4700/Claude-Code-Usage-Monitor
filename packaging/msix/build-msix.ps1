<#
.SYNOPSIS
  Packs dist\headroom.exe into an MSIX.

.DESCRIPTION
  Stages the executable, the manifest and the Assets folder, indexes the
  scale/targetsize assets with makepri, and packs with makeappx. The result
  is unsigned unless -DevSign or -Pfx is given; Partner Center signs Store
  submissions itself.

  Tools come from the Windows SDK if it is installed, or from the folder in
  $env:MSIX_TOOLS (the bin\<ver>\x64 folder of the
  Microsoft.Windows.SDK.BuildTools NuGet package works).

.EXAMPLE
  .\build-msix.ps1                       # unsigned, for Partner Center
  .\build-msix.ps1 -DevSign              # self-signed, for a local sideload test
  .\build-msix.ps1 -IdentityName "12345Publisher.Headroom" -Publisher "CN=ABCDEF12-..." -PublisherDisplayName "Danny Lamphere"
#>
param(
    [string]$Exe = (Join-Path $PSScriptRoot "..\..\dist\headroom.exe"),
    [string]$Version,
    [string]$IdentityName,
    [string]$Publisher,
    [string]$PublisherDisplayName,
    [string]$OutDir = (Join-Path $PSScriptRoot "out"),
    [switch]$DevSign,
    [string]$Pfx,
    [string]$PfxPassword
)
$ErrorActionPreference = "Stop"

function Find-Tool([string]$Name) {
    if ($env:MSIX_TOOLS) {
        $candidate = Join-Path $env:MSIX_TOOLS $Name
        if (Test-Path $candidate) { return $candidate }
    }
    $kit = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\$Name" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($kit) { return $kit.FullName }
    throw "$Name was not found. Install the Windows SDK, or set MSIX_TOOLS to a folder containing it."
}

$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
if (-not $Version) {
    $cargo = Get-Content (Join-Path $repo "Cargo.toml") -Raw
    if ($cargo -match '(?m)^version\s*=\s*"([^"]+)"') { $Version = $Matches[1] } else { throw "version not found in Cargo.toml" }
}
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must be major.minor.patch (got $Version)" }
$packageVersion = "$Version.0"

if (-not (Test-Path $Exe)) { throw "Executable not found: $Exe (build it first)" }
$assets = Join-Path $PSScriptRoot "Assets"
if (-not (Test-Path (Join-Path $assets "Square150x150Logo.png"))) {
    & (Join-Path $PSScriptRoot "make-assets.ps1")
}

$makeappx = Find-Tool "makeappx.exe"
$makepri = Find-Tool "makepri.exe"

$staging = Join-Path $env:TEMP ("headroom-msix-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $staging | Out-Null
try {
    Copy-Item $Exe (Join-Path $staging "headroom.exe")
    Copy-Item $assets (Join-Path $staging "Assets") -Recurse

    [xml]$manifest = Get-Content (Join-Path $PSScriptRoot "AppxManifest.xml")
    $manifest.Package.Identity.Version = $packageVersion
    if ($IdentityName) { $manifest.Package.Identity.Name = $IdentityName }
    if ($Publisher) { $manifest.Package.Identity.Publisher = $Publisher }
    if ($PublisherDisplayName) { $manifest.Package.Properties.PublisherDisplayName = $PublisherDisplayName }
    $manifest.Save((Join-Path $staging "AppxManifest.xml"))

    # Index the qualified assets so the shell picks the right scale.
    $priConfig = Join-Path $staging "priconfig.xml"
    & $makepri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o | Out-Null
    & $makepri new /pr $staging /cf $priConfig /of (Join-Path $staging "resources.pri") /mn (Join-Path $staging "AppxManifest.xml") /o | Out-Null
    Remove-Item $priConfig

    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $package = Join-Path $OutDir ("Headroom_{0}_x64.msix" -f $packageVersion)
    if (Test-Path $package) { Remove-Item $package }
    & $makeappx pack /d $staging /p $package /o
    if ($LASTEXITCODE -ne 0) { throw "makeappx failed with exit code $LASTEXITCODE" }

    if ($DevSign -or $Pfx) {
        $signtool = Find-Tool "signtool.exe"
        if ($DevSign) {
            $subject = $manifest.Package.Identity.Publisher
            $cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq $subject -and $_.FriendlyName -eq "Headroom dev signing" } | Select-Object -First 1
            if (-not $cert) {
                $cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature `
                    -FriendlyName "Headroom dev signing" -CertStoreLocation "Cert:\CurrentUser\My" `
                    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
            }
            $Pfx = Join-Path $OutDir "headroom-dev.pfx"
            $PfxPassword = "headroom"
            $secure = ConvertTo-SecureString $PfxPassword -AsPlainText -Force
            Export-PfxCertificate -Cert $cert -FilePath $Pfx -Password $secure | Out-Null
            Export-Certificate -Cert $cert -FilePath (Join-Path $OutDir "headroom-dev.cer") | Out-Null
        }
        & $signtool sign /fd SHA256 /a /f $Pfx /p $PfxPassword $package
        if ($LASTEXITCODE -ne 0) { throw "signtool failed with exit code $LASTEXITCODE" }
        Write-Host ""
        Write-Host "Signed with a development certificate. To sideload once (as Administrator):"
        Write-Host "  Import-Certificate -FilePath `"$(Join-Path $OutDir 'headroom-dev.cer')`" -CertStoreLocation Cert:\LocalMachine\TrustedPeople"
        Write-Host "  Add-AppxPackage `"$package`""
    }
    Write-Host ""
    Write-Host "Package: $package ($([math]::Round((Get-Item $package).Length / 1MB, 2)) MB)"
}
finally {
    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
}
