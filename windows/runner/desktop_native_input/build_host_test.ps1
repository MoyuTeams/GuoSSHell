param(
    [Parameter(Mandatory = $true)][string]$VcVars64,
    [Parameter(Mandatory = $true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath $VcVars64 -PathType Leaf)) { throw '缺少现有 MSVC 环境入口' }
$output = [IO.Path]::GetFullPath($OutputDirectory)
$parent = [IO.Path]::GetDirectoryName($output)
$approved = Join-Path ([IO.Path]::GetTempPath()) 'opencode'
if (!(Test-Path -LiteralPath $parent -PathType Container)) { throw '测试父目录不存在' }
if ((Resolve-Path -LiteralPath $parent).Path -ne (Resolve-Path -LiteralPath $approved).Path) { throw '只允许批准临时目录的直接子目录' }
if (Test-Path -LiteralPath $output) { throw '不得覆盖已有测试目录' }
$output = Join-Path (Resolve-Path -LiteralPath $parent).Path ([IO.Path]::GetFileName($output))
$null = New-Item -ItemType Directory -Path $output
$sources = @()
try {
    $environment = & $env:ComSpec /d /c "call `"$VcVars64`" >nul & set"
    if ($LASTEXITCODE -ne 0) { throw 'MSVC 环境不可用' }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($matches[1])" -Value $matches[2] }
    }
    $runner = Split-Path -Parent $PSScriptRoot
    $windows = Split-Path -Parent $runner
    $ephemeral = Join-Path $windows 'flutter\ephemeral'
    $wrapper = Join-Path $ephemeral 'cpp_client_wrapper'
    $sources = @('raw_input.cpp', 'geometry.cpp', 'host.cpp', 'pane.cpp', 'pane_channel.cpp', 'pane_input.cpp', 'runtime_module.cpp', 'channel.cpp', 'host_test.cpp') | ForEach-Object { Join-Path $PSScriptRoot $_ }
    $sources += @('native_surface.cpp', 'text_store.cpp', 'layout_metadata.cpp') | ForEach-Object { Join-Path (Join-Path $runner 'native_input_surface') $_ }
    & cl.exe /nologo /utf-8 /std:c++17 /EHsc /W4 /WX /MD /DNOMINMAX /DUNICODE /D_UNICODE "/I$wrapper\include" "/I$ephemeral" @sources (Join-Path $wrapper 'standard_codec.cc') (Join-Path $wrapper 'core_implementations.cc') "/Fo:$output\" "/Fe:$output\host_test.exe" /link user32.lib gdi32.lib ole32.lib oleaut32.lib uuid.lib psapi.lib (Join-Path $ephemeral 'flutter_windows.dll.lib') /INCREMENTAL:NO
    if ($LASTEXITCODE -ne 0) { throw 'normal host 测试编译失败' }
    $env:PATH = $ephemeral + ';' + $env:PATH
    & (Join-Path $output 'host_test.exe')
    if ($LASTEXITCODE -ne 0) { throw 'normal host 测试失败' }
} finally {
    $resolved = (Resolve-Path -LiteralPath $output).Path
    if ($resolved -cne $output) { throw '清理目标不匹配' }
    $directory = Get-Item -LiteralPath $resolved
    if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw '清理目标不能是 reparse point' }
    $artifacts = @($sources | ForEach-Object { [IO.Path]::GetFileNameWithoutExtension($_) + '.obj' })
    $artifacts += @('standard_codec.obj', 'core_implementations.obj', 'host_test.exe')
    foreach ($name in $artifacts) {
        $target = Join-Path $resolved $name
        if (Test-Path -LiteralPath $target) {
            $item = Get-Item -LiteralPath $target
            if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw '构建产物类型不匹配' }
            Remove-Item -LiteralPath $target
        }
    }
    [IO.Directory]::Delete($resolved, $false)
}
