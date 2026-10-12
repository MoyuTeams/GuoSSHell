param(
    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory,
    [switch] $StandaloneFixture,
    [ValidateSet('all', 'safety')]
    [string] $Cases = 'all'
)

$ErrorActionPreference = 'Stop'
$toolset = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207'
$sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10'
$sdkVersion = '10.0.22621.0'
$compiler = Join-Path $toolset 'bin\Hostx64\x64\cl.exe'
$tempParent = Join-Path ([System.IO.Path]::GetTempPath()) 'opencode'
if (!(Test-Path -LiteralPath $tempParent -PathType Container)) { throw '批准的临时父目录不存在' }
$resolvedParent = (Resolve-Path -LiteralPath $tempParent).Path
$expected = Join-Path $resolvedParent 'guosshell-wavea-bridge-known-fixes-20261012'
if ([System.IO.Path]::GetFullPath($OutputDirectory) -ne [System.IO.Path]::GetFullPath($expected)) {
    throw '输出目录不是 bridge 测试的确切临时目标'
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
    if (!(Test-Path -LiteralPath $path -PathType Container)) { throw '既有 SDK 缺失；不安装' }
}
$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$fixtureDirectory = Join-Path $projectRoot 'build\native_input_sdk_fixture\debug'
$fixtureDll = Join-Path $fixtureDirectory 'hub.dll'
if ($StandaloneFixture) {
    if (!(Test-Path -LiteralPath $fixtureDirectory -PathType Container) -or
        !(Test-Path -LiteralPath $fixtureDll -PathType Leaf)) {
        throw '父受控 fixture 产物不存在；不构建或安装 Rust'
    }
}
$sources = @(
    (Join-Path $PSScriptRoot 'native_input_spike_bridge.cpp'),
    (Join-Path $PSScriptRoot 'native_input_spike_bridge_test.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike\window_source.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike\tsf_probe.cpp'),
    (Join-Path $PSScriptRoot '..\native_input_spike_sdk\bootstrap_host.cpp')
)
$created = @(
    (Join-Path $expected 'native_input_spike_bridge.obj'),
    (Join-Path $expected 'native_input_spike_bridge_test.obj'),
    (Join-Path $expected 'window_source.obj'),
    (Join-Path $expected 'tsf_probe.obj'),
    (Join-Path $expected 'bootstrap_host.obj'),
    (Join-Path $expected 'native_input_spike_bridge_test.exe')
)
$oldPath = $env:PATH
New-Item -ItemType Directory -Path $expected | Out-Null
try {
    $env:PATH = (Join-Path $toolset 'bin\Hostx64\x64') + ';' + $oldPath
    $common = @('/nologo', '/std:c++17', '/W4', '/WX', '/utf-8', '/EHsc', '/MT',
                '/DUNICODE', '/D_UNICODE', '/DNOMINMAX', '/D_HAS_EXCEPTIONS=0')
    foreach ($path in $includePaths) { $common += "/I$path" }
    for ($i = 0; $i -lt $sources.Length; ++$i) {
        & $compiler @common /c $sources[$i] "/Fo$($created[$i])"
        if ($LASTEXITCODE -ne 0) { throw "C++ 编译失败：$($sources[$i])" }
    }
    $linkArgs = @('/nologo', '/MACHINE:X64', '/INCREMENTAL:NO', '/MANIFEST:NO',
                  "/OUT:$($created[5])", $created[0], $created[1], $created[2], $created[3], $created[4],
                  'kernel32.lib', 'user32.lib', 'gdi32.lib', 'ole32.lib', 'uuid.lib')
    foreach ($path in $libraryPaths) { $linkArgs += "/LIBPATH:$path" }
    & (Join-Path $toolset 'bin\Hostx64\x64\link.exe') @linkArgs
    if ($LASTEXITCODE -ne 0) { throw 'bridge 测试链接失败' }
    "test_exec_sha256=$((Get-FileHash -LiteralPath $created[5] -Algorithm SHA256).Hash)"
    & $created[5] '--safety'
    $safetyExit = $LASTEXITCODE
    "safety_exit=$safetyExit"
    if ($safetyExit -ne 0) { throw '命令 ownership／Pending 退休回归失败' }
    if ($Cases -eq 'safety') { return }
    & $created[5] '--spy'
    $spyExit = $LASTEXITCODE
    "spy_exit=$spyExit"
    if ($spyExit -ne 0) { throw 'spy 映射／生命周期验证失败' }
    if ($StandaloneFixture) {
        $fixtureHash = (Get-FileHash -LiteralPath $fixtureDll -Algorithm SHA256).Hash
        "fixture_dll_sha256=$fixtureHash"
        & $created[5] '--standalone-fixture' $fixtureDll
        $standaloneExit = $LASTEXITCODE
        "standalone_exit=$standaloneExit"
        if ((Get-FileHash -LiteralPath $fixtureDll -Algorithm SHA256).Hash -ne $fixtureHash) {
            throw '受控 DLL 文件在验证期间变化；不作映像版本一致性结论'
        }
        if ($standaloneExit -ne 0) { throw '受控 standalone fixture 验证失败或不可用' }
    }
} finally {
    $env:PATH = $oldPath
    $cleanupTarget = (Resolve-Path -LiteralPath $expected).Path
    if ([System.IO.Path]::GetFullPath($cleanupTarget) -ne [System.IO.Path]::GetFullPath($expected)) {
        throw '清理目标不匹配，停止删除'
    }
    # 只删除本次确切六个产物；不递归、不删除 fixture DLL 或别人的输出。
    foreach ($path in $created) {
        if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force }
    }
    [System.IO.Directory]::Delete($cleanupTarget, $false)
    'cleanup=exact_created_artifacts_removed'
}
