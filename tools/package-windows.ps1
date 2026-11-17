# Package the Windows Servo build: the exe, the VC++ runtime DLLs it imports,
# and a short note. Servo's resources are compiled into the binary
# (servo feature "bundled"), so there is no resources directory to copy.
# ASCII only. Windows PowerShell reads a UTF-8 script without a BOM as
# the system code page, and a multibyte dash was being parsed as a quote.
$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..")
$exe = Join-Path $repo "target\release\mithu.exe"
if (-not (Test-Path $exe)) {
    throw "missing $exe. Build with: cargo build -p mithu --release"
}
$out = Join-Path $repo "target\release\mithu-windows"
if (Test-Path $out) {
    Remove-Item -Recurse -Force $out
}
New-Item -ItemType Directory -Path $out | Out-Null
Copy-Item $exe (Join-Path $out "mithu.exe")
$dlls = @("VCRUNTIME140.dll", "VCRUNTIME140_1.dll", "MSVCP140.dll")
$sys = Join-Path $env:SystemRoot "System32"
foreach ($dll in $dlls) {
    $src = Join-Path $sys $dll
    if (-not (Test-Path $src)) {
        throw "missing runtime $src"
    }
    Copy-Item $src (Join-Path $out $dll)
}
$note = @(
    "Mithu for Windows."
    ""
    "mithu.exe contains the Servo resources it needs. The three DLL files are the"
    "Visual C++ runtime the binary is linked against. Keep them in this folder."
    ""
    "If the window closes immediately, the error is in"
    "%LOCALAPPDATA%\Mithu\mithu.log"
) -join "`r`n"
Set-Content -Encoding ascii -Path (Join-Path $out "README.txt") -Value $note
$zip = Join-Path $repo "target\release\mithu-windows-x64.zip"
if (Test-Path $zip) {
    Remove-Item -Force $zip
}
Compress-Archive -Path (Join-Path $out "*") -DestinationPath $zip
Write-Output $zip
