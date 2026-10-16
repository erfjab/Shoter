# Installs or updates Shoter on Windows:
#   irm https://raw.githubusercontent.com/erfjab/Shoter/master/install.ps1 | iex
# Only the program file is replaced. Saved shots in Pictures\Shoter are never touched.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$dir = "$env:LOCALAPPDATA\Programs\Shoter"
$exe = "$dir\shoter.exe"
$tmp = Join-Path $env:TEMP "shoter-install-$PID"
$update = Test-Path $exe

# Download and unpack first, so a failed download leaves the current install as it is.
Write-Host $(if ($update) { 'Updating Shoter...' } else { 'Installing Shoter...' })
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Invoke-WebRequest 'https://github.com/erfjab/Shoter/releases/latest/download/shoter-windows-x64.zip' -OutFile "$tmp\shoter.zip"
    Expand-Archive "$tmp\shoter.zip" "$tmp\new" -Force
    if (-not (Test-Path "$tmp\new\shoter.exe")) { throw 'The download did not contain shoter.exe.' }

    # Close the running copy and wait until the file is free.
    Get-Process shoter -ErrorAction SilentlyContinue | Stop-Process -Force
    Get-Process shoter -ErrorAction SilentlyContinue | Wait-Process -Timeout 10

    # Swap the program file, putting the old one back if anything goes wrong.
    New-Item -ItemType Directory -Force $dir | Out-Null
    if ($update) { Move-Item $exe "$exe.old" -Force }
    try {
        Copy-Item "$tmp\new\shoter.exe" $exe
    } catch {
        if ($update) { Move-Item "$exe.old" $exe -Force }
        throw
    }
    Remove-Item "$exe.old" -ErrorAction SilentlyContinue
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

# Start menu entry, and start with Windows so PrtSc always works.
$shell = New-Object -ComObject WScript.Shell
foreach ($folder in "$env:APPDATA\Microsoft\Windows\Start Menu\Programs", [Environment]::GetFolderPath('Startup')) {
    $lnk = $shell.CreateShortcut("$folder\Shoter.lnk")
    $lnk.TargetPath = $exe
    $lnk.Save()
}

Start-Process $exe
if ($update) { Write-Host 'Shoter updated.' }
else { Write-Host "Shoter installed to $dir. Press PrtSc to capture." }
