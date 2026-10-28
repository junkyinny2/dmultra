# Build helper: wires up the VS Build Tools environment for cargo.
$msvc = "C:\BuildTools\VC\Tools\MSVC\14.44.35207"
$sdk = "C:\Program Files (x86)\Windows Kits\10"

$env:PATH = "$msvc\bin\Hostx64\x64;$sdk\Bin\10.0.26100.0\x64;$env:PATH"

# MSVC libs (versioned dir may not ship libs in BuildTools; link against SDK + ucrt + msvcrt from toolchain is not needed here)
$env:LIB = "$sdk\Lib\10.0.26100.0\um\x64;$sdk\Lib\10.0.26100.0\ucrt\x64"
$msvcLib = Get-ChildItem "$msvc\lib" -Directory -ErrorAction SilentlyContinue | Select-Object -First 1
if ($msvcLib) { $env:LIB = "$($msvcLib.FullName)\x64;$env:LIB" }

$env:INCLUDE = "$msvc\include;$sdk\Include\10.0.26100.0\ucrt;$sdk\Include\10.0.26100.0\um;$sdk\Include\10.0.26100.0\shared"

cargo @args
exit $LASTEXITCODE
