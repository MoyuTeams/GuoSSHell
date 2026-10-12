param(
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,
    [ValidateSet('full', 'review-findings', 'synthetic-all')]
    [string] $Cases = 'full'
)

$ErrorActionPreference = 'Stop'
$toolset = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207'
$sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdkVersion = '10.0.22621.0'
$compiler = Join-Path $toolset 'bin\Hostx64\x64\cl.exe'
$tempParent = Join-Path ([System.IO.Path]::GetTempPath()) 'opencode'
$expected = Join-Path $tempParent 'guosshell-wavea-windows-probe-20261008'

# 只接受本探针唯一的新临时目录，不读取／复用其他构建产物。
if (!(Test-Path -LiteralPath $tempParent -PathType Container)) { throw '批准的临时父目录不存在' }
$resolvedParent = (Resolve-Path -LiteralPath $tempParent).Path
$expected = Join-Path $resolvedParent 'guosshell-wavea-windows-probe-20261008'
if ([System.IO.Path]::GetFullPath($OutputDirectory) -ne [System.IO.Path]::GetFullPath($expected)) {
    throw '输出目录不是本探针的确切临时目标'
}
if (Test-Path -LiteralPath $expected) { throw '拒绝覆盖已有临时目标' }
if (!(Test-Path -LiteralPath $compiler -PathType Leaf)) { throw '既有 MSVC 不可用；不安装' }
$includePaths = @(
    (Join-Path $toolset 'include'),
    (Join-Path $sdk "Include\$sdkVersion\ucrt"),
    (Join-Path $sdk "Include\$sdkVersion\shared"),
    (Join-Path $sdk "Include\$sdkVersion\um")
)
$libraryPaths = @(
    (Join-Path $toolset 'lib\x64'),
    (Join-Path $sdk "Lib\$sdkVersion\ucrt\x64"),
    (Join-Path $sdk "Lib\$sdkVersion\um\x64")
)
foreach ($path in ($includePaths + $libraryPaths)) {
    if (!(Test-Path -LiteralPath $path -PathType Container)) { throw '既有 SDK 目录缺失；不安装' }
}
$created = @(
    (Join-Path $expected 'window_source.obj'),
    (Join-Path $expected 'tsf_probe.obj'),
    (Join-Path $expected 'probe_main.obj'),
    (Join-Path $expected 'native_input_probe.exe')
)
$oldPath = $env:PATH
New-Item -ItemType Directory -Path $expected | Out-Null
try {
    $env:PATH = (Join-Path $toolset 'bin\Hostx64\x64') + ';' + $oldPath
    $common = @('/nologo', '/std:c++17', '/W4', '/WX', '/utf-8', '/EHsc', '/MT',
                '/DUNICODE', '/D_UNICODE', '/DNOMINMAX', '/D_HAS_EXCEPTIONS=0')
    foreach ($path in $includePaths) { $common += "/I$path" }
    $sources = @('window_source.cpp', 'tsf_probe.cpp', 'probe_main.cpp')
    for ($i = 0; $i -lt $sources.Length; ++$i) {
        & $compiler @common /c (Join-Path $PSScriptRoot $sources[$i]) "/Fo$($created[$i])"
        if ($LASTEXITCODE -ne 0) { throw "C++ 编译失败：$($sources[$i])" }
    }
    $linkArgs = @('/nologo', '/MACHINE:X64', '/INCREMENTAL:NO', '/MANIFEST:NO',
                  "/OUT:$($created[3])", $created[0], $created[1], $created[2],
                  'kernel32.lib', 'user32.lib', 'gdi32.lib', 'ole32.lib', 'uuid.lib')
    foreach ($path in $libraryPaths) { $linkArgs += "/LIBPATH:$path" }
    & (Join-Path $toolset 'bin\Hostx64\x64\link.exe') @linkArgs
    if ($LASTEXITCODE -ne 0) { throw '独立探针链接失败' }
    if ($Cases -eq 'review-findings') { & $created[3] '--review-findings' }
    elseif ($Cases -eq 'synthetic-all') { & $created[3] '--synthetic-all' }
    else { & $created[3] }
    $probeExit = $LASTEXITCODE
    if ($probeExit -eq 2) { throw '运行环境 STOP／未验证，不能计为通过' }
    if ($probeExit -ne 0) { throw '探针断言失败' }
} finally {
    $env:PATH = $oldPath
    $cleanupTarget = (Resolve-Path -LiteralPath $expected).Path
    if ([System.IO.Path]::GetFullPath($cleanupTarget) -ne [System.IO.Path]::GetFullPath($expected)) {
        throw '清理目标不匹配，停止删除'
    }
    # 无递归／通配符删除；只删除本次新目标中的四个确切产物。
    foreach ($path in $created) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Remove-Item -LiteralPath $path -Force
        }
    }
    [System.IO.Directory]::Delete($cleanupTarget, $false)
    'cleanup=exact_created_artifacts_removed'
}
