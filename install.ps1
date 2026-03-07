$ErrorActionPreference = 'Stop'

$Repo = 'fluo10/archelon'
$InstallDir = Join-Path $HOME '.local\bin'

$Response = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -ErrorAction Ignore
$Version = $Response.Headers.Location -replace '.*/tag/', ''
if (-not $Version) {
    Write-Error 'Failed to fetch latest version.'
    exit 1
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

function Install-Binary {
    param([string]$Binary, [string]$InstallName)

    $Asset = "$Binary-windows-x86_64.exe"
    $Url = "https://github.com/$Repo/releases/download/$Version/$Asset"
    $Dest = Join-Path $InstallDir "$InstallName.exe"

    Write-Host "Installing $InstallName $Version to $InstallDir..."
    Invoke-WebRequest -Uri $Url -OutFile $Dest
    Write-Host "Done! $Dest installed."
}

Install-Binary 'archelon-cli' 'archelon'
Install-Binary 'archelon-mcp' 'archelon-mcp'

$UserPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('PATH', "$InstallDir;$UserPath", 'User')
    Write-Host ""
    Write-Host "Added $InstallDir to your PATH. Restart your terminal to apply."
}
