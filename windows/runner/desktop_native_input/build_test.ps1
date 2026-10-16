param(
    [Parameter(Mandatory = $true)][string]$VcVars64,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath $VcVars64 -PathType Leaf)) { throw '缺少现有 MSVC 环境入口' }
$output = [System.IO.Path]::GetFullPath($OutputDirectory)
$parent = [System.IO.Path]::GetDirectoryName($output)
if (!(Test-Path -LiteralPath $parent -PathType Container)) { throw '测试父目录不存在' }
if (Test-Path -LiteralPath $output) { throw '测试输出目录已存在，不得覆盖' }
$approvedParent = [System.IO.Path]::GetFullPath((Join-Path ([System.IO.Path]::GetTempPath()) 'opencode'))
$resolvedParent = (Resolve-Path -LiteralPath $parent).Path
if ($resolvedParent -ne (Resolve-Path -LiteralPath $approvedParent).Path) {
    throw '测试输出须为批准临时目录的全新直接子目录'
}
$output = Join-Path $resolvedParent ([System.IO.Path]::GetFileName($output))
New-Item -ItemType Directory -Path $output | Out-Null
$nativeSources = @()
$variants = @()

try {
$environment = & $env:ComSpec /d /c "call `"$VcVars64`" >nul & set"
if ($LASTEXITCODE -ne 0) { throw 'MSVC 环境不可用' }
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($matches[1])" -Value $matches[2] }
}
if (!(Get-Command cl.exe -ErrorAction SilentlyContinue)) { throw '现有 MSVC 编译器不可用' }
$runner = Split-Path -Parent $PSScriptRoot
$windows = Split-Path -Parent $runner
$wrapper = Join-Path $windows 'flutter\ephemeral\cpp_client_wrapper'
$include = Join-Path $wrapper 'include'
$flags = @('/nologo', '/utf-8', '/std:c++17', '/EHsc', '/W4', '/WX', '/MD', '/DNOMINMAX', '/DUNICODE', '/D_UNICODE')
$nativeSources = @('raw_input.cpp', 'geometry.cpp', 'host.cpp', 'pane.cpp', 'pane_channel.cpp', 'pane_input.cpp') | ForEach-Object { Join-Path $PSScriptRoot $_ }
$nativeSources += @('native_surface.cpp', 'text_store.cpp', 'layout_metadata.cpp') | ForEach-Object { Join-Path (Join-Path $runner 'native_input_surface') $_ }
$variants = @('good', 'missing_normal', 'missing_rinf', 'bad_layout', 'foreign')
foreach ($variant in $variants) {
    $directory = Join-Path $output $variant
    New-Item -ItemType Directory -Path $directory | Out-Null
    $define = switch ($variant) {
        'missing_normal' { '/DTEST_MISSING_NORMAL' }
        'missing_rinf' { '/DTEST_MISSING_RINF' }
        'bad_layout' { '/DTEST_BAD_LAYOUT' }
        'foreign' { '/DTEST_FOREIGN_BIND' }
        default { '/DTEST_GOOD' }
    }
    & cl.exe @flags /LD $define (Join-Path $PSScriptRoot 'runtime_module_test_dll.cpp') "/Fo:$directory\hub.obj" "/Fe:$directory\hub.dll" /link /INCREMENTAL:NO "/IMPLIB:$directory\hub.lib"
    if ($LASTEXITCODE -ne 0) { throw "测试替身编译失败：$variant" }
}
$good = Join-Path $output 'good'
& cl.exe @flags /LD (Join-Path $PSScriptRoot 'runtime_module_test_dll.cpp') "/Fo:$good\duplicate.obj" "/Fe:$good\duplicate.dll" /link /INCREMENTAL:NO "/IMPLIB:$good\duplicate.lib"
if ($LASTEXITCODE -ne 0) { throw '第二模块编译失败' }
$foreign = Join-Path $output 'foreign'
& cl.exe @flags /LD (Join-Path $PSScriptRoot 'runtime_module_test_dll.cpp') "/Fo:$foreign\duplicate.obj" "/Fe:$foreign\duplicate.dll" /link /INCREMENTAL:NO "/IMPLIB:$foreign\duplicate.lib"
if ($LASTEXITCODE -ne 0) { throw '转发导出目标编译失败' }
& cl.exe @flags (Join-Path $PSScriptRoot 'runtime_module.cpp') (Join-Path $PSScriptRoot 'runtime_module_test.cpp') "/Fo:$output\" "/Fe:$output\runtime_module_test.exe" /link psapi.lib /INCREMENTAL:NO
if ($LASTEXITCODE -ne 0) { throw '模块 API 测试编译失败' }
& cl.exe @flags "/I$include" "/I$windows\flutter\ephemeral" @nativeSources (Join-Path $PSScriptRoot 'runtime_module.cpp') (Join-Path $PSScriptRoot 'channel.cpp') (Join-Path $PSScriptRoot 'channel_test.cpp') (Join-Path $wrapper 'standard_codec.cc') (Join-Path $wrapper 'core_implementations.cc') "/Fo:$output\" "/Fe:$output\channel_test.exe" /link psapi.lib user32.lib gdi32.lib ole32.lib oleaut32.lib uuid.lib (Join-Path $windows 'flutter\ephemeral\flutter_windows.dll.lib') /INCREMENTAL:NO
if ($LASTEXITCODE -ne 0) { throw 'MethodChannel 测试编译失败' }
& cl.exe @flags /c "/I$include" "/I$windows" "/I$windows\flutter\ephemeral" (Join-Path $runner 'flutter_window.cpp') "/Fo:$output\flutter_window.obj"
if ($LASTEXITCODE -ne 0) { throw 'runner 接线局部编译失败' }
& cl.exe @flags /wd4100 /c "/I$include" "/I$windows" "/I$windows\flutter\ephemeral" (Join-Path $runner 'main.cpp') "/Fo:$output\main.obj"
if ($LASTEXITCODE -ne 0) { throw '正常消息泵局部编译失败' }

& (Join-Path $output 'runtime_module_test.exe') missing
if ($LASTEXITCODE -ne 0) { throw '模块缺失未拒绝' }
& (Join-Path $output 'runtime_module_test.exe') wrong_place (Join-Path $good 'hub.dll')
if ($LASTEXITCODE -ne 0) { throw '错误部署位置未拒绝' }
foreach ($variant in $variants) {
    $directory = Join-Path $output $variant
    Copy-Item -LiteralPath (Join-Path $output 'runtime_module_test.exe') -Destination (Join-Path $directory 'runtime_module_test.exe')
    & (Join-Path $directory 'runtime_module_test.exe') $variant (Join-Path $directory 'hub.dll')
    if ($LASTEXITCODE -ne 0) { throw "模块 API 场景失败：$variant" }
}
& (Join-Path $good 'runtime_module_test.exe') duplicate (Join-Path $good 'hub.dll') (Join-Path $good 'duplicate.dll')
if ($LASTEXITCODE -ne 0) { throw '重复模块未拒绝' }
$env:PATH = (Join-Path $windows 'flutter\ephemeral') + ';' + $env:PATH
& (Join-Path $output 'channel_test.exe')
if ($LASTEXITCODE -ne 0) { throw '通道缺模块场景失败' }
Copy-Item -LiteralPath (Join-Path $output 'channel_test.exe') -Destination (Join-Path $good 'channel_test.exe')
& (Join-Path $good 'channel_test.exe') (Join-Path $good 'hub.dll')
if ($LASTEXITCODE -ne 0) { throw '通道已关联模块场景失败' }
'局部 C++ 编译、8 个真实 Win32 模块 API 场景与 2 个 MethodChannel 场景通过；DLL 是测试替身，不证明正常 Rinf 活会话关联。'
} finally {
    $cleanupTarget = (Resolve-Path -LiteralPath $output).Path
    if ($cleanupTarget -cne $output) { throw '清理目标不匹配' }
    $directory = Get-Item -LiteralPath $cleanupTarget
    if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw '清理目标不能是 reparse point' }
    function Remove-BuildArtifact([string]$Target) {
        if (Test-Path -LiteralPath $Target) {
            $item = Get-Item -LiteralPath $Target
            if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw '构建产物类型不匹配' }
            Remove-Item -LiteralPath $Target
        }
    }
    $artifacts = @($nativeSources | ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_) + '.obj' })
    $artifacts += @('runtime_module.obj', 'runtime_module_test.obj', 'runtime_module_test.exe', 'channel.obj', 'channel_test.obj',
        'channel_test.exe', 'standard_codec.obj', 'core_implementations.obj', 'flutter_window.obj', 'main.obj')
    foreach ($name in $artifacts) { Remove-BuildArtifact (Join-Path $cleanupTarget $name) }
    foreach ($variant in $variants) {
        $directory = Join-Path $cleanupTarget $variant
        if (!(Test-Path -LiteralPath $directory)) { continue }
        $item = Get-Item -LiteralPath $directory
        if (!$item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw '构建子目录类型不匹配' }
        foreach ($name in @('hub.obj', 'hub.dll', 'hub.lib', 'hub.exp', 'duplicate.obj', 'duplicate.dll', 'duplicate.lib', 'duplicate.exp',
                'runtime_module_test.exe', 'channel_test.exe')) { Remove-BuildArtifact (Join-Path $directory $name) }
        [IO.Directory]::Delete($directory, $false)
    }
    [IO.Directory]::Delete($cleanupTarget, $false)
}
