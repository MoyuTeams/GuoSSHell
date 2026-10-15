<#
由父任务在 W2 源码稳定后显式运行；仅支持 Windows x64 MSVC，使用已安装工具链和离线缓存。
UpstreamRoot 必须是获授权的 shellx-wt-desktop-config 绝对路径，OutputDirectory 必须全新。
SourceFiles 是调用方批准的精确相对文件清单，至少包含根与 rshell-storage manifest 及 helper 入口。
失败不清理输出；build_recipe.json 声明本轮拥有的输出，artifact_record.json 只在成功后发布。
记录仅提供候选源码与实际产物的构建来源材料，不证明 H/C/VM 或完整 P1 验收。
构建后由 cargo rustc -- --print cfg 探测实际目标选项，不运行 helper；不替代内嵌 fingerprint。
七个嵌入字段来自本轮编译器、依赖解析及 normal 构建声明；环境与 stdio 声明须由父 launcher 另验。
点导入仅加载函数，不执行生产入口；生产入口不接受 mock 工具或替代产物。
#>
[CmdletBinding()]
param(
    [string] $UpstreamRoot,
    [string] $OutputDirectory,
    [string] $Toolchain,
    [string] $Target = 'x86_64-pc-windows-msvc',
    [string[]] $SourceFiles
)

function Stop-SelectedBuild {
    param([string] $Code, [string] $Message)
    $exception = [InvalidOperationException]::new($Message)
    $exception.Data['SelectedBuildCode'] = $Code
    throw $exception
}

function Assert-SelectedAbsolutePath {
    param([string] $Path)
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [IO.Path]::IsPathFullyQualified($Path)) {
        Stop-SelectedBuild 'InvalidPath' '必须提供文件系统绝对路径。'
    }
    return [IO.Path]::TrimEndingDirectorySeparator([IO.Path]::GetFullPath($Path))
}

function Assert-SelectedNoReparse {
    param([string] $Path)
    # 仅检查已知路径的祖先，不枚举目录；拒绝符号链接、junction 和其他重解析点。
    $current = $Path
    while ($current) {
        $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Stop-SelectedBuild 'ReparsePath' '来源和输出路径不能经过重解析点。'
        }
        $current = [IO.Path]::GetDirectoryName($current)
    }
}

function Get-SelectedFileHash {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Stop-SelectedBuild 'MissingSource' '精确清单中的文件不存在。'
    }
    Assert-SelectedNoReparse $Path
    $size = (Get-Item -LiteralPath $Path -Force -ErrorAction Stop).Length
    return [ordered]@{ sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant(); sizeBytes = [long] $size }
}

function Get-SelectedSourceSnapshot {
    param([string] $Root, [string[]] $Files)
    $rootPath = Assert-SelectedAbsolutePath $Root
    if (-not $Files -or $Files.Count -eq 0) {
        Stop-SelectedBuild 'InvalidSourceList' '必须提供获批准的精确来源文件清单。'
    }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $Files) {
        # 严格白名单同时排除旧探针、第三方目录、文档、密钥、配置及 .git。
        if ($file -cnotmatch '^(Cargo\.toml|Cargo\.lock|crates/rshell-storage/(Cargo\.toml|build\.rs|src/(?:[A-Za-z0-9_-]+/)*[A-Za-z0-9_-]+\.rs))$' -or
            @($file.Split('/') | Where-Object { $_ -in @('docs', 'secrets', 'config', 'third_party') }).Count -gt 0 -or
            -not $seen.Add($file)) {
            Stop-SelectedBuild 'InvalidSourceList' '来源清单包含不允许的路径或重复项。'
        }
    }
    foreach ($required in @('Cargo.toml', 'crates/rshell-storage/Cargo.toml')) {
        if (-not $seen.Contains($required)) {
            Stop-SelectedBuild 'InvalidSourceList' '来源清单缺少必需 manifest。'
        }
    }
    $orderedFiles = [string[]] @($Files)
    [Array]::Sort($orderedFiles, [StringComparer]::Ordinal)
    $entries = @(
        foreach ($file in $orderedFiles) {
            $hash = Get-SelectedFileHash (Join-Path $rootPath $file)
            [ordered]@{ relativePath = $file; sha256 = $hash.sha256; sizeBytes = $hash.sizeBytes }
        }
    )
    # 关联摘要按 UTF-8 的「路径 LF SHA256 LF 字节数 LF」拼接，路径禁止换行。
    $canonical = -join @($entries | ForEach-Object { "$($_.relativePath)`n$($_.sha256)`n$($_.sizeBytes)`n" })
    $digest = [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))
    return [ordered]@{
        files = $entries
        sourceFilesSha256 = [Convert]::ToHexString($digest).ToLowerInvariant()
        cargoLock = Get-SelectedFileHash (Join-Path $rootPath 'Cargo.lock')
    }
}

function ConvertFrom-SelectedRustcVersion {
    param([string] $Text)
    $fields = @{}
    foreach ($line in ($Text -split '\r?\n')) {
        if ($line -match '^([^:]+): (.+)$') {
            if ($fields.ContainsKey($Matches[1])) { Stop-SelectedBuild 'InvalidCompiler' '编译器版本字段重复。' }
            $fields[$Matches[1]] = $Matches[2]
        }
    }
    if ($fields['commit-hash'] -notmatch '^[a-f0-9]{40}$' -or
        $fields['release'] -notmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$' -or
        $fields['host'] -notmatch '^[a-z0-9_]+(?:-[a-z0-9_]+)+$' -or
        $fields['LLVM version'] -notmatch '^\d+\.\d+(?:\.\d+)?$') {
        Stop-SelectedBuild 'InvalidCompiler' '编译器版本缺少有效的完整提交、release、host 或 LLVM。'
    }
    return [ordered]@{ fullCommit = $fields['commit-hash']; release = $fields.release; host = $fields.host; llvm = $fields['LLVM version'] }
}

function ConvertFrom-SelectedTargetCfg {
    param([string] $Text)
    $values = @{}
    $features = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $flags = [Collections.Generic.List[string]]::new()
    foreach ($line in ($Text -split '\r?\n')) {
        if ($line.Length -eq 0) { continue }
        if ($line -match '^([A-Za-z_][A-Za-z0-9_]*)="([^"]*)"$') {
            $key, $value = $Matches[1], $Matches[2]
            if ($key -eq 'target_feature') { $null = $features.Add($value); continue }
            if ($values.ContainsKey($key)) {
                if ($key -in @('panic', 'target_arch', 'target_os', 'target_env', 'target_pointer_width', 'target_endian')) {
                    Stop-SelectedBuild 'InvalidTargetCfg' '目标 cfg 单值字段重复。'
                }
                $values[$key] = @($values[$key]) + @($value)
            } else { $values[$key] = $value }
        } elseif ($line -match '^[A-Za-z_][A-Za-z0-9_]*$') { $flags.Add($line) }
        else { Stop-SelectedBuild 'InvalidTargetCfg' '目标 cfg 格式无效。' }
    }
    if ($values['target_arch'] -cne 'x86_64' -or $values['target_os'] -cne 'windows' -or
        $values['target_env'] -cne 'msvc' -or $values['target_pointer_width'] -cne '64' -or
        $values['target_endian'] -cne 'little' -or $values['panic'] -notin @('abort', 'unwind')) {
        Stop-SelectedBuild 'InvalidTargetCfg' '目标 cfg 不是受支持的 Windows x64 MSVC 布局。'
    }
    $orderedFeatures = [string[]] @($features)
    [Array]::Sort($orderedFeatures, [StringComparer]::Ordinal)
    return [ordered]@{ panic = $values.panic; pointerWidth = [int] $values.target_pointer_width; endian = $values.target_endian; targetFeatures = $orderedFeatures; values = $values; flags = @($flags); raw = $Text }
}

function Get-SelectedMetadata {
    param([object] $Metadata, [string] $Root, [string[]] $Files)
    try {
        if (($Metadata.version -isnot [int] -and $Metadata.version -isnot [long]) -or
            $Metadata.version -ne 1 -or -not $Metadata.resolve.nodes) { throw 'format' }
        $helpers = @($Metadata.packages | Where-Object { $_.name -ceq 'rshell-storage' })
        if ($helpers.Count -ne 1) { throw 'helper' }
        $helper = $helpers[0]
        $manifest = Join-Path $Root 'crates/rshell-storage/Cargo.toml'
        if ([IO.Path]::GetFullPath($helper.manifest_path) -ine [IO.Path]::GetFullPath($manifest)) { throw 'manifest' }
        $targets = @($helper.targets | Where-Object { $_.name -ceq 'selected-read-helper' -and 'bin' -cin $_.kind })
        if ($targets.Count -ne 1) { throw 'target' }
        $entry = [IO.Path]::GetRelativePath($Root, $targets[0].src_path).Replace('\', '/')
        if ($entry -cnotin $Files) { throw 'entry' }
        $nodes = @{}
        foreach ($node in $Metadata.resolve.nodes) {
            if ($nodes.ContainsKey($node.id)) { throw 'duplicate' }
            $nodes[$node.id] = $node
        }
        # 从 helper 的解析节点收集可达依赖，而不是 manifest 中的 feature 声明。
        $reachable = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        $pending = [Collections.Generic.Queue[string]]::new()
        $pending.Enqueue($helper.id)
        while ($pending.Count -gt 0) {
            $id = $pending.Dequeue()
            if (-not $reachable.Add($id)) { continue }
            if (-not $nodes.ContainsKey($id)) { throw 'node' }
            foreach ($dependency in $nodes[$id].dependencies) { $pending.Enqueue($dependency) }
        }
        $dependencies = @(
            foreach ($requirement in @(@('rusqlite', '0.37'), @('libsqlite3-sys', '0.35'), @('uuid', '1.24'))) {
                $packages = @($Metadata.packages | Where-Object { $_.name -ceq $requirement[0] -and $reachable.Contains($_.id) })
                if ($packages.Count -ne 1 -or $packages[0].version -notmatch "^$([regex]::Escape($requirement[1]))\.\d+$") { throw 'version' }
                $package = $packages[0]
                $features = @($nodes[$package.id].features)
                if (@($features | Where-Object { $_ -isnot [string] -or [string]::IsNullOrWhiteSpace($_) }).Count) { throw 'features' }
                [ordered]@{ name = $package.name; version = $package.version; packageId = $package.id; source = $package.source; enabledFeatures = @($features | Sort-Object -CaseSensitive) }
            }
        )
        return [ordered]@{ helperPackageId = $helper.id; helperEntry = $entry; dependencies = $dependencies }
    } catch { Stop-SelectedBuild 'InvalidMetadata' 'Cargo metadata 的目标、依赖版本或解析 feature 无效。' }
}

function Get-SelectedEmbeddedBuildEvidenceNames {
    return @(
        'RSHELL_HELPER_BUILD_COMPILER_COMMIT',
        'RSHELL_HELPER_BUILD_COMPILER_RELEASE',
        'RSHELL_HELPER_BUILD_TARGET',
        'RSHELL_HELPER_BUILD_RESOLVED_FEATURES',
        'RSHELL_HELPER_BUILD_CFG',
        'RSHELL_HELPER_BUILD_ENVIRONMENT',
        'RSHELL_HELPER_BUILD_STDIO'
    )
}

function Assert-SelectedNormalCfg {
    param([object] $Cfg)
    if ($Cfg.flags -isnot [Array] -or $Cfg.values -isnot [Collections.IDictionary] -or
        @($Cfg.flags | Where-Object { $_ -isnot [string] -or $_ -cnotmatch '^[A-Za-z_][A-Za-z0-9_]*$' }).Count) {
        Stop-SelectedBuild 'InvalidBuildCfg' 'normal 构建需要已解析的实际 cfg。'
    }
    if ('test' -cin $Cfg.flags -or $Cfg.values.Contains('test')) {
        Stop-SelectedBuild 'NonNormalBuild' '实际 cfg 含 test 分支，拒绝 normal 来源声明及 artifact record。'
    }
}

function New-SelectedEmbeddedBuildEvidence {
    param([object] $Compiler, [object] $Metadata, [string] $BuildTarget, [object] $Cfg, [string] $BuildCfg)
    if ($BuildCfg -cne 'normal') { Stop-SelectedBuild 'NonNormalBuild' '嵌入字段仅用于明确的 normal 构建。' }
    Assert-SelectedNormalCfg $Cfg
    try {
        if ($Compiler.fullCommit -cnotmatch '^[a-f0-9]{40}$' -or $Compiler.release -cnotmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$' -or
            $BuildTarget -cne 'x86_64-pc-windows-msvc' -or $Cfg.values.target_arch -cne 'x86_64' -or
            $Cfg.values.target_os -cne 'windows' -or $Cfg.values.target_env -cne 'msvc' -or $Cfg.pointerWidth -ne 64 -or $Cfg.endian -cne 'little') { throw 'compiler-target' }
        $dependencies = @($Metadata.dependencies)
        $names = @('rusqlite', 'libsqlite3-sys', 'uuid')
        if ($dependencies.Count -ne $names.Count) { throw 'dependencies' }
        $parts = @(
            for ($index = 0; $index -lt $names.Count; $index++) {
                $dependency = $dependencies[$index]
                if ($dependency.name -cne $names[$index] -or
                    $dependency.version -cnotmatch '^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?(?:\+[A-Za-z0-9.-]+)?$' -or
                    $dependency.enabledFeatures -isnot [Array]) { throw 'dependency' }
                $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
                foreach ($feature in $dependency.enabledFeatures) {
                    if ($feature -isnot [string] -or [string]::IsNullOrWhiteSpace($feature) -or
                        $feature -match '[,;:=\s]' -or -not $seen.Add($feature)) { throw 'features' }
                }
                $features = [string[]] @($dependency.enabledFeatures)
                [Array]::Sort($features, [StringComparer]::Ordinal)
                "$($dependency.name)=$($dependency.version):$($features -join ',')"
            }
        )
        # 版本及 features 取自实际解析数据，不替换为证书或源码模型中的常量。
        return [ordered]@{
            RSHELL_HELPER_BUILD_COMPILER_COMMIT = $Compiler.fullCommit
            RSHELL_HELPER_BUILD_COMPILER_RELEASE = $Compiler.release
            RSHELL_HELPER_BUILD_TARGET = $BuildTarget
            RSHELL_HELPER_BUILD_RESOLVED_FEATURES = $parts -join ';'
            RSHELL_HELPER_BUILD_CFG = $BuildCfg
            RSHELL_HELPER_BUILD_ENVIRONMENT = 'inherited'
            RSHELL_HELPER_BUILD_STDIO = 'synchronous-pipes'
        }
    } catch { Stop-SelectedBuild 'InvalidBuildEvidence' '编译器、目标或实际依赖数据不足以生成七个嵌入字段。' }
}

function Get-SelectedBuildEnvironmentSnapshot {
    $snapshot = @{}
    $names = @('RUSTUP_AUTO_INSTALL', 'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER') + @(Get-SelectedEmbeddedBuildEvidenceNames)
    foreach ($name in $names) { $snapshot[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
    return $snapshot
}

function Set-SelectedEmbeddedBuildEnvironment {
    param([Collections.IDictionary] $Evidence)
    $names = @(Get-SelectedEmbeddedBuildEvidenceNames)
    if ($Evidence.Count -ne $names.Count) { Stop-SelectedBuild 'InvalidBuildEvidence' '嵌入环境只接受七个批准字段。' }
    foreach ($name in $names) {
        if (-not $Evidence.Contains($name) -or $Evidence[$name] -isnot [string] -or [string]::IsNullOrWhiteSpace($Evidence[$name])) {
            Stop-SelectedBuild 'InvalidBuildEvidence' '嵌入环境字段缺失或无效。'
        }
    }
    foreach ($name in $names) { [Environment]::SetEnvironmentVariable($name, $Evidence[$name], 'Process') }
}

function Restore-SelectedBuildEnvironment {
    param([Collections.IDictionary] $Snapshot)
    foreach ($name in $Snapshot.Keys) {
        # PowerShell 向 .NET string 参数传 $null 会变为空串；显式 null 才能恢复变量不存在。
        if ($null -eq $Snapshot[$name]) {
            [Environment]::SetEnvironmentVariable($name, [NullString]::Value, 'Process')
        } else { [Environment]::SetEnvironmentVariable($name, $Snapshot[$name], 'Process') }
    }
}

function Invoke-SelectedTool {
    param([string] $Executable, [string[]] $Arguments, [string] $Directory, [string] $Step)
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.WorkingDirectory = $Directory
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        $null = $process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        $text = $stdout.GetAwaiter().GetResult()
        $null = $stderr.GetAwaiter().GetResult()
        # 不转发第三方 stderr 或参数，以免回显路径、源码片段或凭据。
        if ($process.ExitCode -ne 0) { Stop-SelectedBuild 'ToolFailed' "$Step 失败，退出码 $($process.ExitCode)；已生成来源材料保留。" }
        return $text.TrimEnd()
    } finally { $process.Dispose() }
}

function Write-SelectedJson {
    param([string] $Path, [object] $Value)
    $json = ConvertTo-Json -InputObject $Value -Depth 32
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes("$json`n")
        $stream.Write($bytes, 0, $bytes.Length)
    } finally { $stream.Dispose() }
}

function Get-SelectedCargoArguments {
    param([string] $InstalledToolchain, [string] $BuildTarget, [string] $Root, [string] $Output)
    $outputPath = Assert-SelectedAbsolutePath $Output
    # build-dir 支持模板；本轮输出只允许字面路径，不能展开到共享缓存。
    if ($outputPath.IndexOfAny([char[]] '{}') -ge 0) { Stop-SelectedBuild 'InvalidPath' '输出路径不能包含 Cargo 路径模板字符。' }
    $cargoDirectory = Join-Path $outputPath 'cargo'
    $buildDirectory = Join-Path $outputPath 'build'
    # JSON 字符串使用 TOML 基本字符串共有的转义，保留路径中的反斜杠、空格及 Unicode。
    $buildDirectoryValue = ConvertTo-Json -InputObject $buildDirectory -Compress
    # --config 优先于环境和用户配置；空环境值在部分 .NET 版本等同删除。
    $config = @('--config', 'build.rustc-wrapper=""', '--config', 'build.rustc-workspace-wrapper=""', '--config', "build.build-dir=$buildDirectoryValue")
    return [ordered]@{
        cargoDirectory = $cargoDirectory
        build = @('run', $InstalledToolchain, 'cargo', 'build', '--locked', '--offline', '-p', 'rshell-storage', '--bin', 'selected-read-helper', '--target', $BuildTarget, '--target-dir', $cargoDirectory, '--message-format', 'json') + $config
        metadata = @('run', $InstalledToolchain, 'cargo', 'metadata', '--format-version', '1', '--locked', '--offline', '--filter-platform', $BuildTarget, '--manifest-path', (Join-Path $Root 'crates/rshell-storage/Cargo.toml')) + $config
        cfg = @('run', $InstalledToolchain, 'cargo', 'rustc', '--locked', '--offline', '-p', 'rshell-storage', '--bin', 'selected-read-helper', '--target', $BuildTarget, '--target-dir', $cargoDirectory) + $config + @('--', '--print', 'cfg')
    }
}

function Get-SelectedBuiltArtifact {
    param([string] $Messages, [object] $Metadata, [string] $CargoDirectory)
    try {
        $events = @($Messages -split '\r?\n' | Where-Object { $_.Length -gt 0 } | ForEach-Object { ConvertFrom-Json -InputObject $_ -ErrorAction Stop })
        $finished = @($events | Where-Object { $_.reason -ceq 'build-finished' })
        if ($finished.Count -ne 1 -or $finished[0].success -isnot [bool] -or -not $finished[0].success) { throw 'finished' }
        $artifacts = @($events | Where-Object { $_.reason -ceq 'compiler-artifact' })
        $helper = @($artifacts | Where-Object { $_.package_id -ceq $Metadata.helperPackageId -and $_.target.name -ceq 'selected-read-helper' -and 'bin' -cin $_.target.kind -and $_.profile.test -is [bool] -and -not $_.profile.test })
        if ($helper.Count -ne 1 -or -not $helper[0].executable) { throw 'helper' }
        if ($helper[0].fresh -isnot [bool] -or $helper[0].fresh) { throw 'fresh' }
        $executable = Assert-SelectedAbsolutePath $helper[0].executable
        $expected = Join-Path $CargoDirectory 'x86_64-pc-windows-msvc/debug/selected-read-helper.exe'
        if ($executable -ine [IO.Path]::GetFullPath($expected)) { throw 'path' }
        foreach ($dependency in $Metadata.dependencies) {
            $compiled = @($artifacts | Where-Object { $_.package_id -ceq $dependency.packageId -and 'lib' -cin $_.target.kind })
            if ($compiled.Count -ne 1) { throw 'dependency' }
            # metadata 的工作区 feature 合并可能比所选 binary 宽；不把这个差异认证为实际构建。
            $actual = @($compiled[0].features | Sort-Object -CaseSensitive)
            if (($actual -join "`n") -cne ($dependency.enabledFeatures -join "`n")) { throw 'features' }
        }
        $hash = Get-SelectedFileHash $executable
        if ($hash.sizeBytes -le 0) { throw 'empty' }
        return [ordered]@{ path = $executable; sha256 = $hash.sha256; sizeBytes = $hash.sizeBytes; packageId = $helper[0].package_id; features = @($helper[0].features); profile = $helper[0].profile }
    } catch { Stop-SelectedBuild 'InvalidArtifact' '构建消息、实际依赖 feature 或本轮产物不一致，拒绝发布 artifact record。' }
}

function Invoke-SelectedHelperBuild {
    param([string] $Root, [string] $Output, [string] $InstalledToolchain, [string] $BuildTarget, [string[]] $Files)
    $rootPath = Assert-SelectedAbsolutePath $Root
    $outputPath = Assert-SelectedAbsolutePath $Output
    if ($BuildTarget -cne 'x86_64-pc-windows-msvc' -or $InstalledToolchain -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$') {
        Stop-SelectedBuild 'InvalidArgument' '必须显式指定已安装工具链，目标仅支持 Windows x64 MSVC。'
    }
    if (-not $IsWindows -or [IO.Path]::GetFileName($rootPath) -cne 'shellx-wt-desktop-config' -or
        -not (Test-Path -LiteralPath $rootPath -PathType Container)) {
        Stop-SelectedBuild 'InvalidRoot' '需要 Windows 和获授权的上游工作树。'
    }
    $parent = [IO.Path]::GetDirectoryName($outputPath)
    if (-not $parent -or -not (Test-Path -LiteralPath $parent -PathType Container) -or (Test-Path -LiteralPath $outputPath)) {
        Stop-SelectedBuild 'OutputNotFresh' '输出父目录必须存在，输出目标必须全新。'
    }
    if ($outputPath.StartsWith("$rootPath$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) {
        Stop-SelectedBuild 'InvalidPath' '输出必须位于上游工作树之外。'
    }
    Assert-SelectedNoReparse $rootPath
    Assert-SelectedNoReparse $parent
    $snapshot = Get-SelectedSourceSnapshot $rootPath $Files
    $rustup = @(Get-Command rustup -CommandType Application -ErrorAction Stop)[0].Source
    $git = @(Get-Command git -CommandType Application -ErrorAction Stop)[0].Source
    $previousEnvironment = Get-SelectedBuildEnvironmentSnapshot
    try {
        [Environment]::SetEnvironmentVariable('RUSTUP_AUTO_INSTALL', '0', 'Process')
        $installed = Invoke-SelectedTool $rustup @('toolchain', 'list') $rootPath '工具链检查'
        $names = @($installed -split '\r?\n' | ForEach-Object { ($_ -split '\s+')[0] })
        if ($InstalledToolchain -cnotin $names) { Stop-SelectedBuild 'ToolchainNotInstalled' '必须使用 rustup 列出的精确已安装工具链名称。' }
        $installedTargets = Invoke-SelectedTool $rustup @('target', 'list', '--installed', '--toolchain', $InstalledToolchain) $rootPath '目标检查'
        if ($BuildTarget -cnotin ($installedTargets -split '\r?\n')) { Stop-SelectedBuild 'TargetNotInstalled' '目标未安装；本脚本不安装组件。' }
        $compilerPathText = Invoke-SelectedTool $rustup @('which', '--toolchain', $InstalledToolchain, 'rustc') $rootPath '编译器定位'
        $compilerPath = Assert-SelectedAbsolutePath $compilerPathText
        if (-not (Test-Path -LiteralPath $compilerPath -PathType Leaf)) { Stop-SelectedBuild 'InvalidCompiler' '已安装编译器文件不存在。' }
        # 固定 Cargo 使用同一实际编译器，禁用可替换编译器/参数的包装器；不修改用户配置。
        [Environment]::SetEnvironmentVariable('RUSTC', $compilerPath, 'Process')
        [Environment]::SetEnvironmentVariable('RUSTC_WRAPPER', '', 'Process')
        [Environment]::SetEnvironmentVariable('RUSTC_WORKSPACE_WRAPPER', '', 'Process')
        $gitRoot = Invoke-SelectedTool $git @('-C', $rootPath, 'rev-parse', '--show-toplevel') $rootPath 'Git 根检查'
        if ((Assert-SelectedAbsolutePath $gitRoot) -ine $rootPath) { Stop-SelectedBuild 'InvalidRoot' '上游必须是独立 Git 工作树根。' }
        $head = Invoke-SelectedTool $git @('-C', $rootPath, 'rev-parse', 'HEAD') $rootPath 'Git 基线检查'
        if ($head -notmatch '^[a-f0-9]{40}$') { Stop-SelectedBuild 'InvalidRoot' 'Git 基线提交无效。' }
        $cargoArguments = Get-SelectedCargoArguments $InstalledToolchain $BuildTarget $rootPath $outputPath
        $null = New-Item -ItemType Directory -Path $outputPath -ErrorAction Stop
        $cargoDirectory = $cargoArguments.cargoDirectory
        $buildArguments = $cargoArguments.build
        $recipe = [ordered]@{
            schemaVersion = 1
            purpose = '仅构建来源材料；不证明 H/C/VM 或完整 P1 验收。'
            upstreamRoot = $rootPath
            actualGitHeadBaseline = $head
            sourceAssociation = '候选源码由精确清单摘要关联；HEAD 不代表未提交 helper 已交付，清单不证明全部依赖源码覆盖。'
            sources = $snapshot
            toolchain = $InstalledToolchain
            target = $BuildTarget
            workingDirectory = $rootPath
            executable = $rustup
            arguments = $buildArguments
            environment = @{ RUSTUP_AUTO_INSTALL = '0'; RUSTC = $compilerPath; RUSTC_WRAPPER = ''; RUSTC_WORKSPACE_WRAPPER = '' }
            ownedOutputs = @('build_recipe.json', 'cargo', 'build', 'artifact_record.json')
        }
        $versionText = Invoke-SelectedTool $rustup @('run', $InstalledToolchain, 'rustc', '-vV') $rootPath '编译器检查'
        $compiler = ConvertFrom-SelectedRustcVersion $versionText
        $cfgText = Invoke-SelectedTool $rustup @('run', $InstalledToolchain, 'rustc', '--print', 'cfg', '--target', $BuildTarget) $rootPath '目标 cfg 检查'
        $defaultCfg = ConvertFrom-SelectedTargetCfg $cfgText
        $metadataArgs = $cargoArguments.metadata
        $metadataText = Invoke-SelectedTool $rustup $metadataArgs $rootPath '依赖解析'
        try { $metadataObject = ConvertFrom-Json -InputObject $metadataText -ErrorAction Stop }
        catch { Stop-SelectedBuild 'InvalidMetadata' 'Cargo metadata 不是有效 JSON。' }
        $resolved = Get-SelectedMetadata $metadataObject $rootPath $Files
        $beforeBuild = Get-SelectedSourceSnapshot $rootPath $Files
        if ($beforeBuild.sourceFilesSha256 -cne $snapshot.sourceFilesSha256 -or $beforeBuild.cargoLock.sha256 -cne $snapshot.cargoLock.sha256) {
            Stop-SelectedBuild 'SourceChanged' '来源在依赖解析期间变化；需要稳定源码及全新输出。'
        }
        $embeddedBuildEvidence = New-SelectedEmbeddedBuildEvidence $compiler $resolved $BuildTarget $defaultCfg 'normal'
        $declarationLimits = [ordered]@{
            embedding = '七字段是传入本轮构建的嵌入参数，不声称已从 exe 读取；父须将 helper 返回的 fingerprint 与 postbuild 来源记录关联。'
            normal = '仅在实际 Cargo cfg 不含 test 且 compiler-artifact 是非 test normal bin 时发布 record。'
            runtime = 'inherited 与 synchronous-pipes 仅为 binary 声明的既有受信调用方前件；父 launcher 须另验 stdinherit 与三路 piped，不代表 runtime 已测。'
            compilerFlags = '仅记录现有 RUSTFLAGS、CARGO_ENCODED_RUSTFLAGS 与实际目标 cfg；不是完整实际 rustc argv，Cargo 配置和 flags 仍须与 source model 匹配。'
            association = '不嵌入自身 binary SHA256；postbuild record 用实际 binary SHA256 关联来源声明。父须 beforeSpawn 验证 record hash 并独立核验 caller runtime class。'
        }
        $inheritedCompilerFlags = [ordered]@{
            RUSTFLAGS = [Environment]::GetEnvironmentVariable('RUSTFLAGS', 'Process')
            CARGO_ENCODED_RUSTFLAGS = [Environment]::GetEnvironmentVariable('CARGO_ENCODED_RUSTFLAGS', 'Process')
        }
        $recipe['embeddedBuildEvidence'] = $embeddedBuildEvidence
        $recipe['embeddedBuildEvidenceDeclarationLimits'] = $declarationLimits
        $recipe['inheritedCompilerFlags'] = $inheritedCompilerFlags
        Set-SelectedEmbeddedBuildEnvironment $embeddedBuildEvidence
        Write-SelectedJson (Join-Path $outputPath 'build_recipe.json') $recipe
        $messages = Invoke-SelectedTool $rustup $buildArguments $rootPath '离线构建'
        # cargo rustc 沿用相同 profile、目标及 Cargo/环境选项，仅打印 helper 的 cfg，不执行 exe。
        $cfgArguments = $cargoArguments.cfg
        $actualCfgText = Invoke-SelectedTool $rustup $cfgArguments $rootPath '实际 helper cfg 检查'
        $actualCfg = ConvertFrom-SelectedTargetCfg $actualCfgText
        Assert-SelectedNormalCfg $actualCfg
        $artifact = Get-SelectedBuiltArtifact $messages $resolved $cargoDirectory
        $afterBuild = Get-SelectedSourceSnapshot $rootPath $Files
        $afterHead = Invoke-SelectedTool $git @('-C', $rootPath, 'rev-parse', 'HEAD') $rootPath 'Git 基线复核'
        if ($afterBuild.sourceFilesSha256 -cne $snapshot.sourceFilesSha256 -or $afterBuild.cargoLock.sha256 -cne $snapshot.cargoLock.sha256 -or $afterHead -cne $head) {
            Stop-SelectedBuild 'SourceChanged' '来源或 Git 基线在构建期间变化，拒绝发布 artifact record。'
        }
        $record = [ordered]@{
            schemaVersion = 1
            purpose = $recipe.purpose
            recipeSha256 = (Get-SelectedFileHash (Join-Path $outputPath 'build_recipe.json')).sha256
            upstreamRoot = $rootPath
            actualGitHeadBaseline = $head
            sources = $snapshot
            compiler = $compiler
            compilerPath = $compilerPath
            compilerVerboseVersion = $versionText
            target = $BuildTarget
            toolchainDefaultTargetCfg = $defaultCfg
            actualTargetCfg = $actualCfg
            cfgArguments = $cfgArguments
            cfgScope = '同一 Cargo 目标与选项的 cargo rustc -- --print cfg；仍须由 helper 内嵌 fingerprint 与 SDK 接口关联，不能作为 H/C/VM 验收。'
            dependencies = $resolved.dependencies
            dependencyFeatureEvidence = 'resolve.nodes 的启用 features，并与本轮 compiler-artifact 的实际库 features 核对。'
            embeddedBuildEvidence = $embeddedBuildEvidence
            embeddedBuildEvidenceDeclarationLimits = $declarationLimits
            inheritedCompilerFlags = $inheritedCompilerFlags
            artifact = $artifact
        }
        Write-SelectedJson (Join-Path $outputPath 'artifact_record.json') $record
        return $record
    } finally { Restore-SelectedBuildEnvironment $previousEnvironment }
}

if ($MyInvocation.InvocationName -ne '.') {
    try {
        $null = Invoke-SelectedHelperBuild -Root $UpstreamRoot -Output $OutputDirectory -InstalledToolchain $Toolchain -BuildTarget $Target -Files $SourceFiles
        '本机构建来源记录已生成；不代表 H/C/VM 或完整 P1 验收。'
    } catch {
        if ($_.Exception.Data.Contains('SelectedBuildCode')) { throw }
        Stop-SelectedBuild 'BuildFailed' '构建未完成；不转发底层错误中的本机路径或参数，已生成输出保留供本机审计。'
    }
}
