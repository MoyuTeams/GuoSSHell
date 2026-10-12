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
$tempParent = Join-Path ([System.IO.Path]::GetTempPath()) 'opencode'
if (!(Test-Path -LiteralPath $tempParent -PathType Container)) { throw '批准的临时父目录不存在' }
$resolvedParent = (Resolve-Path -LiteralPath $parent).Path
if ($resolvedParent -ne (Resolve-Path -LiteralPath $tempParent).Path) { throw '输出不在批准的临时父目录' }
$output = Join-Path $resolvedParent ([System.IO.Path]::GetFileName($output))
$created = @('bootstrap_host.obj', 'host_test.obj', 'host_test.exe')
foreach ($variant in @('good', 'missing', 'normal', 'dirty', 'duplicate')) {
    foreach ($extension in @('dll', 'obj', 'lib', 'exp')) { $created += "$variant.$extension" }
}
New-Item -ItemType Directory -Path $output | Out-Null
try {

# 编译子进程只导入现有 MSVC 环境；无安装、全局环境设置或共享 CMake 变更。
$environment = & $env:ComSpec /d /c "call `"$VcVars64`" >nul & set"
if ($LASTEXITCODE -ne 0) { throw 'MSVC 环境不可用' }
foreach ($line in $environment) {
    if ($line -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($matches[1])" -Value $matches[2] }
}
if (!(Get-Command cl.exe -ErrorAction SilentlyContinue)) { throw '现有 MSVC 编译器不可用' }
$source = Join-Path $PSScriptRoot 'test_fixture.cpp'
foreach ($variant in @('good', 'missing', 'normal', 'dirty', 'duplicate')) {
    $define = switch ($variant) {
        'missing' { '/DTEST_MISSING_EXPORT' }
        'normal' { '/DTEST_NORMAL_MODE' }
        'dirty' { '/DTEST_DIRTY_CORE' }
        default { '/DTEST_FIXTURE' }
    }
    & cl.exe /nologo /utf-8 /std:c++17 /EHsc /W4 /WX /LD $define $source "/Fe:$output\$variant.dll" "/Fo:$output\$variant.obj" /link /INCREMENTAL:NO "/IMPLIB:$output\$variant.lib"
    if ($LASTEXITCODE -ne 0) { throw "测试 DLL 编译失败：$variant" }
}
& cl.exe /nologo /utf-8 /std:c++17 /EHsc /W4 /WX (Join-Path $PSScriptRoot 'bootstrap_host.cpp') (Join-Path $PSScriptRoot 'host_test.cpp') "/Fe:$output\host_test.exe" "/Fo:$output\" /link /INCREMENTAL:NO
if ($LASTEXITCODE -ne 0) { throw '宿主测试编译失败' }
foreach ($scenario in @('off', 'good', 'missing', 'normal', 'dirty', 'duplicate', 'nonce', 'foreign')) {
    $variant = if ($scenario -in @('missing', 'normal', 'dirty')) { $scenario } else { 'good' }
    & (Join-Path $output 'host_test.exe') $scenario (Join-Path $output "$variant.dll") (Join-Path $output 'duplicate.dll')
    if ($LASTEXITCODE -ne 0) { throw "宿主测试失败：$scenario" }
}
'8 个 Windows loader/test-double 场景通过；不是 live SDK 证明。'
} finally {
    $cleanupTarget = (Resolve-Path -LiteralPath $output).Path
    if ([System.IO.Path]::GetFullPath($cleanupTarget) -ne [System.IO.Path]::GetFullPath($output)) { throw '清理目标不匹配' }
    foreach ($name in $created) {
        $artifact = Join-Path $cleanupTarget $name
        if (Test-Path -LiteralPath $artifact -PathType Leaf) { Remove-Item -LiteralPath $artifact -Force }
    }
    # 非递归删除；任何未知产物都保留并使清理失败。
    [System.IO.Directory]::Delete($cleanupTarget, $false)
}
