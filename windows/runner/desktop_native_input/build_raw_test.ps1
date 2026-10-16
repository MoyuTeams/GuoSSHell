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
try {
    $environment = & $env:ComSpec /d /c "call `"$VcVars64`" >nul & set"
    if ($LASTEXITCODE -ne 0) { throw 'MSVC 环境不可用' }
    foreach ($line in $environment) {
        if ($line -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($matches[1])" -Value $matches[2] }
    }
    $surface = Join-Path (Split-Path -Parent $PSScriptRoot) 'native_input_surface'
    $sources = @((Join-Path $PSScriptRoot 'raw_input.cpp'), (Join-Path $PSScriptRoot 'raw_input_test.cpp'), (Join-Path $PSScriptRoot 'layout_probe_test.cpp'),
        (Join-Path $surface 'native_surface.cpp'), (Join-Path $surface 'text_store.cpp'), (Join-Path $surface 'layout_metadata.cpp'))
    & cl.exe /nologo /utf-8 /std:c++17 /EHsc /W4 /WX /MD /DNOMINMAX @sources "/Fo:$output\" "/Fe:$output\raw_input_test.exe" /link user32.lib ole32.lib oleaut32.lib uuid.lib /INCREMENTAL:NO
    if ($LASTEXITCODE -ne 0) { throw 'normal raw/TSF 测试编译失败' }
    & (Join-Path $output 'raw_input_test.exe')
    if ($LASTEXITCODE -ne 0) { throw 'normal raw/TSF 测试失败' }
} finally {
    $resolved = (Resolve-Path -LiteralPath $output).Path
    if ($resolved -cne $output) { throw '清理目标不匹配' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
