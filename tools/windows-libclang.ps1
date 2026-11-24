# bindgen (used by mozangle on the Servo edition) parses MSVC STL headers
# with libclang. The windows-2025-vs2026 image ships LLVM 20, but MSVC
# 14.52 (Visual Studio 2026 18.12) requires Clang 22 or newer (STL1000).
# Prefer a Clang 22+ already on the machine (VS LLVM first); otherwise
# install one. Sets LIBCLANG_PATH and PATH for later steps in GitHub Actions.
$ErrorActionPreference = "Stop"

function Get-ClangMajor([string]$Clang) {
    $out = & $Clang --version 2>&1 | Out-String
    if ($out -match "clang version (\d+)") {
        return [int]$Matches[1]
    }
    return 0
}

$candidates = @(
    "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Tools\Llvm\x64\bin",
    "C:\Program Files\Microsoft Visual Studio\18\Enterprise\VC\Tools\Llvm\bin",
    "C:\Program Files\LLVM\bin"
)

$selected = $null
foreach ($bin in $candidates) {
    $clang = Join-Path $bin "clang.exe"
    if (Test-Path $clang) {
        $major = Get-ClangMajor $clang
        Write-Host "found $clang (clang $major)"
        if ($major -ge 22) {
            $selected = $bin
            break
        }
    }
}

if (-not $selected) {
    Write-Host "Installing LLVM 22+ with Chocolatey (image LLVM is too old for MSVC 14.52 STL)"
    choco install llvm --yes --no-progress
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $selected = "C:\Program Files\LLVM\bin"
}

$clang = Join-Path $selected "clang.exe"
if (-not (Test-Path $clang)) {
    Write-Error "clang.exe not found in $selected"
    exit 1
}
$major = Get-ClangMajor $clang
if ($major -lt 22) {
    Write-Error "clang at $clang is version $major; need 22+"
    exit 1
}

# libclang.dll ships next to clang.exe in the official LLVM Windows layout.
$libdir = $selected
if (-not (Test-Path (Join-Path $libdir "libclang.dll"))) {
    $alt = Join-Path (Split-Path $selected -Parent) "lib"
    if (Test-Path (Join-Path $alt "libclang.dll")) {
        $libdir = $alt
    }
}

& $clang --version
Write-Host "LIBCLANG_PATH=$libdir"
if ($env:GITHUB_ENV) {
    Add-Content -Path $env:GITHUB_ENV -Value "LIBCLANG_PATH=$libdir"
}
if ($env:GITHUB_PATH) {
    Add-Content -Path $env:GITHUB_PATH -Value $selected
}
