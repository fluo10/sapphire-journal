$ErrorActionPreference = 'Stop'

$Repo = 'fluo10/archelon'
$InstallDir = Join-Path $HOME '.local\bin'

$Releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases"
$Version = ($Releases | Where-Object { $_.tag_name -like 'cli-v*' } | Select-Object -First 1).tag_name
if (-not $Version) {
    Write-Error 'Failed to fetch latest CLI version.'
    exit 1
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

function Install-Binary {
    param([string]$Binary)

    $Asset = "$Binary-windows-x86_64.exe"
    $Url = "https://github.com/$Repo/releases/download/$Version/$Asset"
    $Dest = Join-Path $InstallDir "$Binary.exe"

    Write-Host "Installing $Binary $Version to $InstallDir..."
    Invoke-WebRequest -Uri $Url -OutFile $Dest
    Write-Host "Done! $Dest installed."
}

Install-Binary 'archelon'

$UserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('PATH', "$InstallDir;$UserPath", 'User')
    Write-Host ""
    Write-Host "Added $InstallDir to your PATH. Restart your terminal to apply."
}
