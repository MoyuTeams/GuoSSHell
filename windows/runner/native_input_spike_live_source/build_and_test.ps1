param(
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'
$toolset = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207'
$sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdkVersion = '10.0.22621.0'
$compiler = Join-Path $toolset 'bin\Hostx64\x64\cl.exe'
$tempParent = Join-Path ([System.IO.Path]::GetTempPath()) 'opencode'
if (!(Test-Path -LiteralPath $tempParent -PathType Container)) { throw '批准的临时父目录不存在' }
$expected = Join-Path (Resolve-Path -LiteralPath $tempParent).Path 'guosshell-wavea-live-source-20261012'
if ([System.IO.Path]::GetFullPath($OutputDirectory) -ne [System.IO.Path]::GetFullPath($expected)) {
    throw '输出不是本组件的确切临时目标'
}
if (Test-Path -LiteralPath $expected) { throw '拒绝覆盖已有目标' }
if (!(Test-Path -LiteralPath $compiler -PathType Leaf)) { throw '既有 MSVC 不可用；不安装' }
$includes = @(
    (Join-Path $toolset 'include'),
    (Join-Path $sdk "Include\$sdkVersion\ucrt"),
    (Join-Path $sdk "Include\$sdkVersion\shared"),
    (Join-Path $sdk "Include\$sdkVersion\um")
)
$libraries = @(
    (Join-Path $toolset 'lib\x64'),
    (Join-Path $sdk "Lib\$sdkVersion\ucrt\x64"),
    (Join-Path $sdk "Lib\$sdkVersion\um\x64")
)
foreach ($path in ($includes + $libraries)) {
    if (!(Test-Path -LiteralPath $path -PathType Container)) { throw '既有 SDK 不可用；不安装' }
}
$sources = @(
    (Join-Path $PSScriptRoot 'source_fixture_host.cpp'),
    (Join-Path $PSScriptRoot 'component_test.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike_bridge\native_input_spike_bridge.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike\window_source.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike\tsf_probe.cpp')
)
# 资源登记只有本次确切目标，不生成／清理 DLL、SDK、Rust 或共享构建文件。
$created = @(
    (Join-Path $expected 'source_fixture_host.obj'),
    (Join-Path $expected 'component_test.obj'),
    (Join-Path $expected 'native_input_spike_bridge.obj'),
    (Join-Path $expected 'window_source.obj'),
    (Join-Path $expected 'tsf_probe.obj'),
    (Join-Path $expected 'source_fixture_host_production.obj'),
    (Join-Path $expected 'component_test.exe')
)
New-Item -ItemType Directory -Path $expected | Out-Null
try {
    $common = @('/nologo', '/std:c++17', '/W4', '/WX', '/utf-8', '/EHsc', '/MT',
                '/DUNICODE', '/D_UNICODE', '/DNOMINMAX', '/D_HAS_EXCEPTIONS=0')
    foreach ($path in $includes) { $common += "/I$path" }
    # 先核验真实父注入版本：没有 LayerSpy 工厂或 probe-only HWND／COM getter。
    & $compiler @common /c $sources[0] "/Fo$($created[5])"
    if ($LASTEXITCODE -ne 0) { throw '父注入组件编译失败' }
    for ($i = 0; $i -lt $sources.Length; ++$i) {
        & $compiler @common '/DGUOSSH_LIVE_SOURCE_LAYER_TEST' /c $sources[$i] "/Fo$($created[$i])"
        if ($LASTEXITCODE -ne 0) { throw "组件测试编译失败：$($sources[$i])" }
    }
    $linkArgs = @('/nologo', '/MACHINE:X64', '/INCREMENTAL:NO', '/MANIFEST:NO',
        "/OUT:$($created[6])", $created[0], $created[1], $created[2], $created[3], $created[4],
        'kernel32.lib', 'user32.lib', 'gdi32.lib', 'ole32.lib', 'uuid.lib')
    foreach ($path in $libraries) { $linkArgs += "/LIBPATH:$path" }
    & (Join-Path $toolset 'bin\Hostx64\x64\link.exe') @linkArgs
    if ($LASTEXITCODE -ne 0) { throw '组件测试链接失败' }
    foreach ($case in @('--gate', '--numbers', '--reject', '--ledger', '--pending', '--receiver')) {
        & $created[6] $case
        if ($LASTEXITCODE -ne 0) { throw "组件测试失败：$case" }
    }
} finally {
    $cleanupTarget = (Resolve-Path -LiteralPath $expected).Path
    if ([System.IO.Path]::GetFullPath($cleanupTarget) -ne [System.IO.Path]::GetFullPath($expected)) {
        throw '清理目标不匹配，停止删除'
    }
    foreach ($path in $created) {
        if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force }
    }
    [System.IO.Directory]::Delete($cleanupTarget, $false)
    'cleanup=exact_created_artifacts_removed'
}
