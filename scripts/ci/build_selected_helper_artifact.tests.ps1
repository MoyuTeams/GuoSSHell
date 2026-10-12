<#
独立 fixture 测试：解析、清单、摘要和拒绝边界；仅调用 rustup toolchain list 验证失败时环境恢复。
不调用 Cargo/rustc/git、不构建或运行 helper。
运行：pwsh -NoProfile -File scripts/ci/build_selected_helper_artifact.tests.ps1
临时目录必须已有；仅创建并逐一清理本测试声明的文件与空目录，不递归删除。
#>
[CmdletBinding()]
param([string] $FixtureParent = (Join-Path ([IO.Path]::GetTempPath()) 'opencode'))

$ErrorActionPreference = 'Stop'
$script:assertions = 0
$script:ownedFiles = [Collections.Generic.List[string]]::new()
$script:ownedDirectories = [Collections.Generic.List[string]]::new()

function Assert-Fixture {
    param([bool] $Condition, [string] $Name)
    if (-not $Condition) { throw "测试失败：$Name" }
    $script:assertions++
}

function Assert-FixtureError {
    param([string] $Code, [scriptblock] $Action)
    try { $null = & $Action }
    catch {
        $actualCode = $_.Exception.Data['SelectedBuildCode']
        $exceptionType = $_.Exception.GetType().Name
        Assert-Fixture ($actualCode -ceq $Code) "错误分类 $Code，实际 $actualCode / $exceptionType / $($_.FullyQualifiedErrorId)"
        return
    }
    throw "测试失败：未拒绝 $Code"
}

function New-FixtureDirectory {
    param([string] $Path)
    $parent = [IO.Path]::GetDirectoryName($Path)
    if (-not (Test-Path -LiteralPath $parent -PathType Container) -or (Test-Path -LiteralPath $Path)) {
        throw 'fixture 父目录不存在或目标已存在。'
    }
    $null = New-Item -ItemType Directory -Path $Path
    $script:ownedDirectories.Add($Path)
}

function New-FixtureFile {
    param([string] $Path, [string] $Text)
    if (-not (Test-Path -LiteralPath ([IO.Path]::GetDirectoryName($Path)) -PathType Container)) {
        throw 'fixture 文件父目录不存在。'
    }
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew)
    $script:ownedFiles.Add($Path)
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
        $stream.Write($bytes, 0, $bytes.Length)
    } finally { $stream.Dispose() }
}

function Get-FixtureCargoConfig {
    param([string[]] $Arguments)
    $configuration = @{}
    for ($index = 0; $index -lt $Arguments.Count; $index++) {
        if ($Arguments[$index] -ceq '--') { break }
        if ($Arguments[$index] -ceq '--config') {
            $key, $value = $Arguments[++$index] -split '=', 2
            if ($configuration.ContainsKey($key)) { throw 'fixture 配置参数重复。' }
            # 生产参数限定为 TOML/JSON 共有的基本字符串；解析实际参数，不读取脚本文案。
            $configuration[$key] = ConvertFrom-Json -InputObject $value
        }
    }
    return $configuration
}

function ConvertTo-FixtureArtifactMessages {
    param([object[]] $Events)
    return (@($Events | ForEach-Object { ConvertTo-Json -InputObject $_ -Depth 12 -Compress }) -join "`n")
}

$buildScript = Join-Path $PSScriptRoot 'build_selected_helper_artifact.ps1'
foreach ($path in @($buildScript, $PSCommandPath)) {
    $tokens = $null
    $parseErrors = $null
    $null = [Management.Automation.Language.Parser]::ParseFile($path, [ref] $tokens, [ref] $parseErrors)
    Assert-Fixture ($parseErrors.Count -eq 0) 'PowerShell 语法'
}
. $buildScript
Set-StrictMode -Version Latest

$parentPath = Assert-SelectedAbsolutePath $FixtureParent
if (-not (Test-Path -LiteralPath $parentPath -PathType Container)) { throw 'fixture 父目录必须已存在。' }
Assert-SelectedNoReparse $parentPath
$fixture = Join-Path $parentPath "selected-helper-script-test-$([Guid]::NewGuid().ToString('N'))"
try {
    New-FixtureDirectory $fixture
    $root = Join-Path $fixture 'shellx-wt-desktop-config'
    New-FixtureDirectory $root
    foreach ($relative in @('crates', 'crates/rshell-storage', 'crates/rshell-storage/src', 'crates/rshell-storage/src/bin')) {
        New-FixtureDirectory (Join-Path $root $relative)
    }
    $files = @('Cargo.toml', 'crates/rshell-storage/Cargo.toml', 'crates/rshell-storage/src/bin/selected_read_helper.rs')
    foreach ($relative in @($files) + @('Cargo.lock')) { New-FixtureFile (Join-Path $root $relative) 'abc' }

    $snapshot = Get-SelectedSourceSnapshot $root $files
    $abcSha256 = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
    Assert-Fixture ($snapshot.files.Count -eq 3) '精确清单数量'
    Assert-Fixture ($snapshot.files[0].sha256 -ceq $abcSha256) '真实文件 SHA256'
    Assert-Fixture ($snapshot.files[0].sizeBytes -is [long] -and $snapshot.files[0].sizeBytes -eq 3) '文件字节数'
    Assert-Fixture ($snapshot.cargoLock.sha256 -ceq $abcSha256) '锁文件摘要'
    $reverse = @($files[2], $files[1], $files[0])
    Assert-Fixture ((Get-SelectedSourceSnapshot $root $reverse).sourceFilesSha256 -ceq $snapshot.sourceFilesSha256) '清单顺序不影响关联摘要'
    $stream = [IO.File]::Open((Join-Path $root $files[2]), [IO.FileMode]::Append)
    try { $stream.WriteByte(100) } finally { $stream.Dispose() }
    Assert-Fixture ((Get-SelectedSourceSnapshot $root $files).sourceFilesSha256 -cne $snapshot.sourceFilesSha256) '源码内容变化影响关联摘要'

    foreach ($bad in @('../Cargo.toml', '/Cargo.toml', 'crates/rshell-storage/src/../../secret.rs', 'crates/rshell-storage/src//main.rs',
        'third_party/lib/src/lib.rs', 'native/hub/src/native_input_spike/main.rs', 'docs/plan.md', '.git/config', '.env',
        'crates/rshell-storage/src/secrets/key.rs', 'crates/rshell-storage/src/config/token.rs', 'crates/rshell-storage/src/bin/key.pem',
        (Join-Path $fixture 'outside.rs'))) {
        Assert-FixtureError 'InvalidSourceList' { Get-SelectedSourceSnapshot $root (@($files) + @($bad)) }
    }
    Assert-FixtureError 'InvalidSourceList' { Get-SelectedSourceSnapshot $root (@($files) + @('Cargo.toml')) }
    Assert-FixtureError 'InvalidSourceList' { Get-SelectedSourceSnapshot $root @($files[2]) }
    Assert-FixtureError 'MissingSource' { Get-SelectedSourceSnapshot $root (@($files) + @('crates/rshell-storage/src/missing.rs')) }
    Assert-FixtureError 'InvalidPath' { Assert-SelectedAbsolutePath 'relative' }

    $version = @(
        'rustc 1.90.0 (012345678 2025-09-14)',
        'binary: rustc',
        'commit-hash: 0123456789abcdef0123456789abcdef01234567',
        'commit-date: 2025-09-14',
        'host: x86_64-pc-windows-msvc',
        'release: 1.90.0',
        'LLVM version: 20.1.8'
    ) -join "`n"
    $compiler = ConvertFrom-SelectedRustcVersion $version
    Assert-Fixture ($compiler.fullCommit.Length -eq 40 -and $compiler.release -ceq '1.90.0' -and $compiler.llvm -ceq '20.1.8') '完整 compiler 字段'
    Assert-FixtureError 'InvalidCompiler' { ConvertFrom-SelectedRustcVersion ($version.Replace('0123456789abcdef0123456789abcdef01234567', 'unknown')) }
    Assert-FixtureError 'InvalidCompiler' { ConvertFrom-SelectedRustcVersion ($version + "`nrelease: 1.91.0") }
    Assert-FixtureError 'InvalidCompiler' { ConvertFrom-SelectedRustcVersion ($version.Replace('20.1.8', 'unknown')) }

    $cfgText = @('debug_assertions', 'panic="unwind"', 'target_arch="x86_64"', 'target_os="windows"', 'target_env="msvc"',
        'target_pointer_width="64"', 'target_endian="little"', 'target_feature="sse2"', 'target_feature="fxsr"',
        'target_has_atomic="8"', 'target_has_atomic="64"') -join "`n"
    $cfg = ConvertFrom-SelectedTargetCfg $cfgText
    Assert-Fixture ($cfg.pointerWidth -is [int] -and $cfg.pointerWidth -eq 64 -and $cfg.endian -ceq 'little') '真实数值布局'
    Assert-Fixture (($cfg.targetFeatures -join ',') -ceq 'fxsr,sse2' -and $cfg.values.target_has_atomic.Count -eq 2) '多值 cfg'
    Assert-Fixture ((ConvertFrom-SelectedTargetCfg ($cfgText.Replace('panic="unwind"', 'panic="abort"'))).panic -ceq 'abort') 'abort cfg'
    Assert-Fixture ((ConvertFrom-SelectedTargetCfg ($cfgText + "`nselected_p1")).flags -contains 'selected_p1') '合法自定义 cfg 标识符'
    Assert-FixtureError 'InvalidTargetCfg' { ConvertFrom-SelectedTargetCfg ($cfgText.Replace('width="64"', 'width="invalid"')) }
    Assert-FixtureError 'InvalidTargetCfg' { ConvertFrom-SelectedTargetCfg ($cfgText.Replace('env="msvc"', 'env="gnu"')) }
    Assert-FixtureError 'InvalidTargetCfg' { ConvertFrom-SelectedTargetCfg ($cfgText + "`npanic=`"abort`"") }

    # 所有 metadata 都是本文件对象 fixture；不替换生产入口或工具调用。
    $metadata = [pscustomobject]@{
        version = 1
        packages = @(
            [pscustomobject]@{ name = 'rshell-storage'; id = 'fixture-helper'; manifest_path = Join-Path $root $files[1]; targets = @([pscustomobject]@{ name = 'selected-read-helper'; kind = @('bin'); src_path = Join-Path $root $files[2] }) },
            [pscustomobject]@{ name = 'rusqlite'; version = '0.37.0'; id = 'fixture-rusqlite'; source = 'registry+fixture'; features = @{ unused = @() } },
            [pscustomobject]@{ name = 'libsqlite3-sys'; version = '0.35.0'; id = 'fixture-sqlite'; source = 'registry+fixture' },
            [pscustomobject]@{ name = 'uuid'; version = '1.24.0'; id = 'fixture-uuid'; source = 'registry+fixture' }
        )
        resolve = [pscustomobject]@{
            nodes = @(
                [pscustomobject]@{ id = 'fixture-helper'; dependencies = @('fixture-rusqlite', 'fixture-uuid'); features = @() },
                [pscustomobject]@{ id = 'fixture-rusqlite'; dependencies = @('fixture-sqlite'); features = @('bundled') },
                [pscustomobject]@{ id = 'fixture-sqlite'; dependencies = @(); features = @('bundled', 'default') },
                [pscustomobject]@{ id = 'fixture-uuid'; dependencies = @(); features = @('std', 'v4') }
            )
        }
    }
    $resolved = Get-SelectedMetadata $metadata $root $files
    Assert-Fixture ($resolved.dependencies.Count -eq 3) '三项可达真实版本依赖'
    Assert-Fixture (($resolved.dependencies[0].enabledFeatures -join ',') -ceq 'bundled') 'features 来自 resolve 节点'
    $metadata.version = '1'
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata $metadata $root $files }
    $metadata.version = 1
    $metadata.packages[1].version = '0.36.0'
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata $metadata $root $files }
    $metadata.packages[1].version = '0.37.0'
    $metadata.resolve.nodes[0].dependencies = @('fixture-rusqlite')
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata $metadata $root $files }
    $metadata.resolve.nodes[0].dependencies = @('fixture-rusqlite', 'fixture-uuid')
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata $metadata $root @($files[0], $files[1]) }
    $metadata.resolve.nodes[3].features = @('std', 4)
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata $metadata $root $files }
    $metadata.resolve.nodes[3].features = @('std', 'v4')
    Assert-FixtureError 'InvalidMetadata' { Get-SelectedMetadata ([pscustomobject]@{ version = 1; resolve = $null }) $root $files }

    $buildEvidence = New-SelectedEmbeddedBuildEvidence $compiler $resolved 'x86_64-pc-windows-msvc' $cfg 'normal'
    Assert-Fixture ($buildEvidence.Count -eq 7) '嵌入参数只有七个批准字段'
    Assert-Fixture ($buildEvidence.RSHELL_HELPER_BUILD_COMPILER_COMMIT -ceq $compiler.fullCommit -and
        $buildEvidence.RSHELL_HELPER_BUILD_COMPILER_RELEASE -ceq $compiler.release) 'compiler 字段取实际解析值'
    Assert-Fixture ($buildEvidence.RSHELL_HELPER_BUILD_TARGET -ceq 'x86_64-pc-windows-msvc' -and $buildEvidence.RSHELL_HELPER_BUILD_CFG -ceq 'normal') '目标及 normal 构建声明'
    Assert-Fixture ($buildEvidence.RSHELL_HELPER_BUILD_ENVIRONMENT -ceq 'inherited' -and
        $buildEvidence.RSHELL_HELPER_BUILD_STDIO -ceq 'synchronous-pipes') '既有受信调用方前件声明'
    $expectedFeatureSequence = "rusqlite=$($resolved.dependencies[0].version):bundled;libsqlite3-sys=$($resolved.dependencies[1].version):bundled,default;uuid=$($resolved.dependencies[2].version):std,v4"
    Assert-Fixture ($buildEvidence.RSHELL_HELPER_BUILD_RESOLVED_FEATURES -ceq $expectedFeatureSequence) '实际版本及启用 features 序列'

    $variantMetadata = ConvertFrom-Json (ConvertTo-Json -InputObject $resolved -Depth 12)
    $variantMetadata.dependencies[0].version = '9.88.77'
    $variantMetadata.dependencies[0].enabledFeatures = @('zeta', 'alpha')
    $variant = New-SelectedEmbeddedBuildEvidence $compiler $variantMetadata 'x86_64-pc-windows-msvc' $cfg 'normal'
    Assert-Fixture ($variant.RSHELL_HELPER_BUILD_RESOLVED_FEATURES.StartsWith('rusqlite=9.88.77:alpha,zeta;')) '序列保留实际未知版本并按 ordinal 排 features'
    $changedCompiler = ConvertFrom-SelectedRustcVersion ($version.Replace($compiler.fullCommit, ('a' * 40)).Replace($compiler.release, '2.77.8-nightly'))
    $variant = New-SelectedEmbeddedBuildEvidence $changedCompiler $resolved 'x86_64-pc-windows-msvc' $cfg 'normal'
    Assert-Fixture ($variant.RSHELL_HELPER_BUILD_COMPILER_COMMIT -ceq $changedCompiler.fullCommit -and
        $variant.RSHELL_HELPER_BUILD_COMPILER_RELEASE -ceq $changedCompiler.release) '不把别次 compiler 当作本次实际值'
    $variantMetadata.dependencies[0].enabledFeatures = @()
    $variant = New-SelectedEmbeddedBuildEvidence $compiler $variantMetadata 'x86_64-pc-windows-msvc' $cfg 'normal'
    Assert-Fixture ($variant.RSHELL_HELPER_BUILD_RESOLVED_FEATURES.StartsWith('rusqlite=9.88.77:;')) '合法空启用 feature 集'
    foreach ($invalidFeatures in @(@('alpha', 'alpha'), @('alpha;injected'), @(''), @('alpha', 4), 'alpha')) {
        $variantMetadata.dependencies[0].enabledFeatures = $invalidFeatures
        Assert-FixtureError 'InvalidBuildEvidence' { New-SelectedEmbeddedBuildEvidence $compiler $variantMetadata 'x86_64-pc-windows-msvc' $cfg 'normal' }
    }
    $variantMetadata.dependencies[0].enabledFeatures = $null
    Assert-FixtureError 'InvalidBuildEvidence' { New-SelectedEmbeddedBuildEvidence $compiler $variantMetadata 'x86_64-pc-windows-msvc' $cfg 'normal' }
    $variantMetadata.dependencies[0].PSObject.Properties.Remove('enabledFeatures')
    Assert-FixtureError 'InvalidBuildEvidence' { New-SelectedEmbeddedBuildEvidence $compiler $variantMetadata 'x86_64-pc-windows-msvc' $cfg 'normal' }
    Assert-FixtureError 'InvalidBuildEvidence' { New-SelectedEmbeddedBuildEvidence ([ordered]@{ release = $compiler.release }) $resolved 'x86_64-pc-windows-msvc' $cfg 'normal' }
    Assert-FixtureError 'InvalidBuildEvidence' { New-SelectedEmbeddedBuildEvidence $compiler $resolved 'aarch64-pc-windows-msvc' $cfg 'normal' }
    foreach ($invalidMarker in @($null, 'test', 'Normal', 'normal ')) {
        Assert-FixtureError 'NonNormalBuild' { New-SelectedEmbeddedBuildEvidence $compiler $resolved 'x86_64-pc-windows-msvc' $cfg $invalidMarker }
    }
    $testCfg = ConvertFrom-SelectedTargetCfg ($cfgText + "`ntest")
    Assert-FixtureError 'NonNormalBuild' { Assert-SelectedNormalCfg $testCfg }
    Assert-FixtureError 'NonNormalBuild' { New-SelectedEmbeddedBuildEvidence $compiler $resolved 'x86_64-pc-windows-msvc' $testCfg 'normal' }
    $valuedTestCfg = ConvertFrom-SelectedTargetCfg ($cfgText + "`ntest=`"fixture`"")
    Assert-FixtureError 'NonNormalBuild' { Assert-SelectedNormalCfg $valuedTestCfg }
    $invalidEnvironment = @{}
    foreach ($name in $buildEvidence.Keys) { $invalidEnvironment[$name] = $buildEvidence[$name] }
    $invalidEnvironment['UNAPPROVED_FIXTURE_FIELD'] = 'fixture'
    Assert-FixtureError 'InvalidBuildEvidence' { Set-SelectedEmbeddedBuildEnvironment $invalidEnvironment }
    $invalidEnvironment.Remove('UNAPPROVED_FIXTURE_FIELD')
    $invalidEnvironment['RSHELL_HELPER_BUILD_CFG'] = ''
    Assert-FixtureError 'InvalidBuildEvidence' { Set-SelectedEmbeddedBuildEnvironment $invalidEnvironment }
    $invalidEnvironment.Remove('RSHELL_HELPER_BUILD_CFG')
    Assert-FixtureError 'InvalidBuildEvidence' { Set-SelectedEmbeddedBuildEnvironment $invalidEnvironment }

    # 仅在本测试进程中执行与生产相同的七字段 setter/finally restorer，不替换任何工具或产物。
    $originalBuildEnvironment = Get-SelectedBuildEnvironmentSnapshot
    try {
        $evidenceNames = @(Get-SelectedEmbeddedBuildEvidenceNames)
        for ($index = 0; $index -lt $evidenceNames.Count; $index++) {
            if (($index % 3) -eq 1) {
                [Environment]::SetEnvironmentVariable($evidenceNames[$index], [NullString]::Value, 'Process')
            } elseif (($index % 3) -eq 2) {
                [Environment]::SetEnvironmentVariable($evidenceNames[$index], '', 'Process')
            } else { [Environment]::SetEnvironmentVariable($evidenceNames[$index], "fixture-prior-$index", 'Process') }
        }
        $beforeFault = Get-SelectedBuildEnvironmentSnapshot
        $priorPresence = @{}
        foreach ($name in $evidenceNames) { $priorPresence[$name] = Test-Path -LiteralPath "Env:$name" }
        $beforeFlags = @{
            RUSTFLAGS = [Environment]::GetEnvironmentVariable('RUSTFLAGS', 'Process')
            CARGO_ENCODED_RUSTFLAGS = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
        }
        $observedFault = $false
        try {
            try {
                Set-SelectedEmbeddedBuildEnvironment $buildEvidence
                foreach ($name in $evidenceNames) {
                    Assert-Fixture ([Environment]::GetEnvironmentVariable($name, 'Process') -ceq $buildEvidence[$name]) '七字段赋值来自本次参数而非原环境'
                }
                $fault = [InvalidOperationException]::new('fixture')
                $fault.Data['FixtureFault'] = $true
                throw $fault
            } finally { Restore-SelectedBuildEnvironment $beforeFault }
        } catch {
            if (-not $_.Exception.Data.Contains('FixtureFault')) { throw }
            $observedFault = $true
        }
        Assert-Fixture $observedFault '赋值后受控 fault 已执行'
        foreach ($name in $beforeFault.Keys) {
            Assert-Fixture ([Environment]::GetEnvironmentVariable($name, 'Process') -ceq $beforeFault[$name]) 'fault 后原环境值恢复'
        }
        foreach ($name in $evidenceNames) {
            Assert-Fixture ((Test-Path -LiteralPath "Env:$name") -eq $priorPresence[$name]) 'fault 后 present/unset 状态恢复'
        }
        foreach ($name in $beforeFlags.Keys) {
            Assert-Fixture ([Environment]::GetEnvironmentVariable($name, 'Process') -ceq $beforeFlags[$name]) '不清除或标准化已有 compiler flags'
        }
    } finally { Restore-SelectedBuildEnvironment $originalBuildEnvironment }

    $output = Join-Path $fixture 'output'
    Assert-FixtureError 'InvalidArgument' { Invoke-SelectedHelperBuild $root $output 'stable' 'aarch64-pc-windows-msvc' $files }
    Assert-FixtureError 'InvalidArgument' { Invoke-SelectedHelperBuild $root $output '--install' 'x86_64-pc-windows-msvc' $files }
    New-FixtureDirectory $output
    Assert-FixtureError 'OutputNotFresh' { Invoke-SelectedHelperBuild $root $output 'stable' 'x86_64-pc-windows-msvc' $files }
    Assert-FixtureError 'OutputNotFresh' { Invoke-SelectedHelperBuild $root (Join-Path $fixture 'missing-parent/output') 'stable' 'x86_64-pc-windows-msvc' $files }

    $quotedOutput = Join-Path $fixture '参数 空格=字面'
    $cargoArguments = Get-SelectedCargoArguments 'fixture-toolchain' 'x86_64-pc-windows-msvc' $root $quotedOutput
    $expectedBuildDirectory = Join-Path $quotedOutput 'build'
    foreach ($operation in @('build', 'metadata', 'cfg')) {
        $configuration = Get-FixtureCargoConfig $cargoArguments[$operation]
        Assert-Fixture ($configuration.Count -eq 3) "$operation 使用相同限定配置字段"
        Assert-Fixture ($configuration['build.build-dir'] -ceq $expectedBuildDirectory) "$operation 路径转义正确且位于本轮输出"
        Assert-Fixture ($configuration['build.rustc-wrapper'] -ceq '' -and $configuration['build.rustc-workspace-wrapper'] -ceq '') "$operation 禁用包装器配置"
    }
    Assert-FixtureError 'InvalidPath' { Get-SelectedCargoArguments 'fixture-toolchain' 'x86_64-pc-windows-msvc' $root (Join-Path $fixture '{cargo-cache-home}') }

    $savedAutoInstall = [Environment]::GetEnvironmentVariable('RUSTUP_AUTO_INSTALL', 'Process')
    try {
        foreach ($priorValue in @($null, 'fixture-sentinel')) {
            if ($null -eq $priorValue) {
                [Environment]::SetEnvironmentVariable('RUSTUP_AUTO_INSTALL', [NullString]::Value, 'Process')
            } else { [Environment]::SetEnvironmentVariable('RUSTUP_AUTO_INSTALL', $priorValue, 'Process') }
            $absentToolchain = "fixture-not-installed-$([Guid]::NewGuid().ToString('N'))"
            Assert-FixtureError 'ToolchainNotInstalled' { Invoke-SelectedHelperBuild $root (Join-Path $fixture 'uncreated-output') $absentToolchain 'x86_64-pc-windows-msvc' $files }
            Assert-Fixture ([Environment]::GetEnvironmentVariable('RUSTUP_AUTO_INSTALL', 'Process') -ceq $priorValue) '失败时恢复原进程环境'
        }
    } finally {
        if ($null -eq $savedAutoInstall) {
            [Environment]::SetEnvironmentVariable('RUSTUP_AUTO_INSTALL', [NullString]::Value, 'Process')
        } else { [Environment]::SetEnvironmentVariable('RUSTUP_AUTO_INSTALL', $savedAutoInstall, 'Process') }
    }

    $jsonPath = Join-Path $output 'fixture.json'
    Write-SelectedJson $jsonPath @{ count = 3; sizeBytes = [long] 7; embeddedBuildEvidence = $buildEvidence }
    $script:ownedFiles.Add($jsonPath)
    $json = ConvertFrom-Json ([IO.File]::ReadAllText($jsonPath))
    Assert-Fixture ($json.count -eq 3 -and $json.sizeBytes -eq 7) 'JSON 数值保持数值'
    Assert-Fixture ($json.embeddedBuildEvidence.RSHELL_HELPER_BUILD_COMPILER_COMMIT -ceq $compiler.fullCommit -and
        $json.embeddedBuildEvidence.RSHELL_HELPER_BUILD_RESOLVED_FEATURES -ceq $expectedFeatureSequence) 'JSON 嵌入参数保留实际来源值与格式'
    $refusedOverwrite = $false
    try { Write-SelectedJson $jsonPath @{ count = 4 } } catch [IO.IOException] { $refusedOverwrite = $true }
    Assert-Fixture $refusedOverwrite '来源输出不可覆盖'
    Assert-Fixture ((ConvertFrom-Json ([IO.File]::ReadAllText($jsonPath))).count -eq 3) '拒绝覆盖保留原始数据'
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact '{"reason":"build-finished","success":true}' $resolved (Join-Path $output 'cargo') }
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact '{"reason":"build-finished","success":false}' $resolved (Join-Path $output 'cargo') }
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact 'invalid-json' $resolved (Join-Path $output 'cargo') }

    # 只供 parser/hash 的普通字节 fixture，不是实际 helper，也不发布生产 artifact record。
    foreach ($relative in @('cargo', 'cargo/x86_64-pc-windows-msvc', 'cargo/x86_64-pc-windows-msvc/debug')) {
        New-FixtureDirectory (Join-Path $output $relative)
    }
    $cargoDirectory = Join-Path $output 'cargo'
    $fixtureBinary = Join-Path $cargoDirectory 'x86_64-pc-windows-msvc/debug/selected-read-helper.exe'
    New-FixtureFile $fixtureBinary 'abc'
    $helperEvent = [ordered]@{
        reason = 'compiler-artifact'
        package_id = $resolved.helperPackageId
        target = [ordered]@{ name = 'selected-read-helper'; kind = @('bin') }
        profile = [ordered]@{ test = $false; opt_level = '0'; debug_assertions = $true }
        executable = $fixtureBinary
        features = @()
        fresh = $false
    }
    $events = @($helperEvent) + @(
        foreach ($dependency in $resolved.dependencies) {
            [ordered]@{ reason = 'compiler-artifact'; package_id = $dependency.packageId; target = @{ name = $dependency.name; kind = @('lib') }; features = $dependency.enabledFeatures; fresh = $false }
        }
    ) + @([ordered]@{ reason = 'build-finished'; success = $true })
    $accepted = Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory
    Assert-Fixture ($accepted.sha256 -ceq $abcSha256) '本轮文件实际 SHA256'
    Assert-Fixture ($accepted.sizeBytes -is [long] -and $accepted.sizeBytes -eq 3) '本轮文件实际字节数'
    Assert-Fixture ($accepted.path -ceq $fixtureBinary -and $accepted.profile.test -ceq $false) '匹配依赖 features 的 normal bin 接纳'

    foreach ($invalidFresh in @($true, $null, 0, 1, 'false')) {
        $helperEvent.fresh = $invalidFresh
        Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    }
    $helperEvent.Remove('fresh')
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    $helperEvent['fresh'] = $false

    foreach ($invalidTest in @($true, $null, 0, 'false')) {
        $helperEvent.profile.test = $invalidTest
        Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    }
    $helperEvent.profile.Remove('test')
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    $helperEvent.profile['test'] = $false
    $helperEvent.target.kind = @('test')
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    $helperEvent.target.kind = @('bin')

    $externalBinary = Join-Path $fixture 'external-helper.exe'
    New-FixtureFile $externalBinary 'abc'
    foreach ($relative in @('old-cargo', 'old-cargo/x86_64-pc-windows-msvc', 'old-cargo/x86_64-pc-windows-msvc/debug')) {
        New-FixtureDirectory (Join-Path $fixture $relative)
    }
    $oldBinary = Join-Path $fixture 'old-cargo/x86_64-pc-windows-msvc/debug/selected-read-helper.exe'
    New-FixtureFile $oldBinary 'abc'
    foreach ($unownedBinary in @($externalBinary, $oldBinary)) {
        $helperEvent.executable = $unownedBinary
        Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    }
    $helperEvent.executable = $fixtureBinary
    $events[1].features = @('unexpected-enabled-feature')
    Assert-FixtureError 'InvalidArtifact' { Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory }
    $events[1].features = $resolved.dependencies[0].enabledFeatures
    $accepted = Get-SelectedBuiltArtifact (ConvertTo-FixtureArtifactMessages $events) $resolved $cargoDirectory
    Assert-Fixture ($accepted.sha256 -ceq $abcSha256 -and $accepted.sizeBytes -eq 3) '合法 boolean 与依赖 features 接纳'
    "通过 $script:assertions 项断言；未运行实际 Rust 构建或 helper。"
} finally {
    # fail-closed：只删除本次逐项登记、位于唯一 fixture 内且没有重解析点的文件与空目录。
    foreach ($file in $script:ownedFiles) {
        if (-not $file.StartsWith("$fixture$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝清理非 fixture 文件。' }
        Assert-SelectedNoReparse $file
        Remove-Item -LiteralPath $file -ErrorAction Stop
    }
    for ($index = $script:ownedDirectories.Count - 1; $index -ge 0; $index--) {
        $directory = $script:ownedDirectories[$index]
        if ($directory -cne $fixture -and -not $directory.StartsWith("$fixture$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) { throw '拒绝清理非 fixture 目录。' }
        Assert-SelectedNoReparse $directory
        [IO.Directory]::Delete($directory, $false)
    }
}
