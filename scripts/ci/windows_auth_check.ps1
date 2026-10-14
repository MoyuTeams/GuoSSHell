<#
阶段三 Windows 真实目录与 Fluent SSH 认证验收。仅使用已有 Flutter、Cargo、ssh-keygen。
运行：pwsh -NoProfile -File scripts/ci/windows_auth_check.ps1
受控派生中止验证：增加 -VerifyKeygenAbort（仅此模式使用系统已有 .NET Framework 编译器）。
终端 profile 端到端：增加 -Target profile（fixture 带 GUOSH_DEMO_ENQ=1，跑 desktop_profile_effect_smoke_test）。
System OpenSSH 端到端：增加 -Target system（跑 desktop_system_ssh_smoke_test）。本轮在运行目录写 ssh 配置，
经 GUOSH_SYSTEM_SSH_CONFIG（仅 debug 构建读取）让系统 ssh 以 -F 使用它：known_hosts、身份文件均在本轮目录，
不连 agent、只用密码认证；Windows OpenSSH 不按 HOME/USERPROFILE 定位 ~/.ssh，故不用重定向主目录的方式。
本轮 fixture 与应用数据完全隔离；成功必须包含进程树退出、精确目录删除及环境恢复。
#>
[CmdletBinding()]
param(
    [ValidateSet('auth', 'profile', 'system')][string] $Target = 'auth',
    [ValidateRange(60, 3600)][int] $TimeoutSeconds = 1200,
    [switch] $FlutterChild,
    [switch] $ServerChild,
    [switch] $VerifyKeygenAbort,
    [string] $GateName,
    [string] $ServerBinary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw '此 runner 只允许在 Windows 上运行' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if (-not (Test-Path -LiteralPath (Join-Path $root 'rust/examples/demo_server.rs'))) {
    throw 'runner 必须从本仓库执行'
}

# 子进程先等待父进程纳入 Job，server/ssh-keygen 与 Flutter 宿主均不得提前派生。
if ($FlutterChild -or $ServerChild) {
    if ($FlutterChild -and $ServerChild) { throw '启动门角色不能同时指定' }
    if ($GateName -notmatch '^Local\\GuoSSHell-auth-[a-f0-9]{32}-(server|flutter)$' -or
        ($ServerChild -and -not $GateName.EndsWith('-server')) -or
        ($FlutterChild -and -not $GateName.EndsWith('-flutter'))) {
        throw '子进程缺少本轮角色启动门'
    }
    $gate = [Threading.EventWaitHandle]::OpenExisting($GateName)
    try {
        if (-not $gate.WaitOne(30000)) { throw '子进程启动所有权确认超时' }
    } finally { $gate.Dispose() }
    Push-Location -LiteralPath $root
    try {
        if ($ServerChild) {
            $runPath = $env:GUOSH_AUTH_RUN
            $runId = $env:GUOSH_AUTH_RUN_ID
            if ([string]::IsNullOrWhiteSpace($runPath) -or
                $runId -notmatch '^[a-f0-9]{32}$' -or
                $GateName -cne "Local\GuoSSHell-auth-$runId-server" -or
                [IO.Path]::GetFileName($runPath) -cne "guossh-auth-$runId" -or
                $env:GUOSH_DEMO_DATA -cne (Join-Path $runPath 'server') -or
                $env:GUOSH_DEMO_INPUT_LOG -cne (Join-Path $runPath 'received.txt') -or
                [IO.File]::ReadAllText((Join-Path $runPath '.owner')) -cne $runId -or
                $env:GUOSH_AUTH_PORT -notmatch '^\d+$' -or
                -not (Test-Path -LiteralPath $ServerBinary -PathType Leaf)) {
                throw 'server 启动前隔离参数或所有权不符'
            }
            foreach ($directory in @($runPath, $env:GUOSH_DEMO_DATA)) {
                $item = Get-Item -LiteralPath $directory -Force -ErrorAction Stop
                if (-not $item.PSIsContainer -or
                    ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
                    (Resolve-Path -LiteralPath $directory).ProviderPath -cne $directory) {
                    throw 'server 隔离目录不符，拒绝启动'
                }
            }
            $child = Start-Process -FilePath $ServerBinary -ArgumentList $env:GUOSH_AUTH_PORT `
                -WorkingDirectory $root -NoNewWindow -PassThru
            try {
                # 原子发布真正 demo_server 的 PID，不把包装 pwsh 当监听进程。
                $pidFile = Join-Path $runPath 'server.pid'
                [IO.File]::WriteAllText("$pidFile.new", "$($child.Id)")
                [IO.File]::Move("$pidFile.new", $pidFile)
                $child.WaitForExit()
                exit $child.ExitCode
            } finally { $child.Dispose() }
        }
        $testFile = @{
            auth = 'integration_test/desktop_ssh_auth_smoke_test.dart'
            profile = 'integration_test/desktop_profile_effect_smoke_test.dart'
            system = 'integration_test/desktop_system_ssh_smoke_test.dart'
        }[$Target]
        & flutter test $testFile -d windows --no-pub --reporter expanded
        exit $LASTEXITCODE
    } finally { Pop-Location }
}

foreach ($tool in @('cargo', 'flutter', 'ssh-keygen', 'pwsh')) {
    $null = Get-Command $tool -ErrorAction Stop
}
if ($Target -eq 'system') { $null = Get-Command ssh -ErrorAction Stop }

# Windows Job 的内核归属不会被 PID 重用、进程重挂或名称相同的其他 worker 混淆。
# 子孙进程自动归属，禁止 breakaway；句柄关闭仍会兜底终止本轮 Job。
if (-not ('GuoSSHellAuthJob' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;

public sealed class GuoSSHellAuthJob : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct BasicLimit {
        public long UserTime, JobTime;
        public uint Flags;
        public UIntPtr MinWorkingSet, MaxWorkingSet;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)] struct IoCounters {
        public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes;
    }
    [StructLayout(LayoutKind.Sequential)] struct ExtendedLimit {
        public BasicLimit Basic;
        public IoCounters Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential)] struct Accounting {
        public long UserTime, KernelTime, PeriodUserTime, PeriodKernelTime;
        public uint PageFaults, TotalProcesses, ActiveProcesses, TerminatedProcesses;
    }
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref ExtendedLimit info, uint length);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out Accounting info, uint length, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool QueryFullProcessImageName(IntPtr process, uint flags, System.Text.StringBuilder path, ref uint size);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool CloseHandle(IntPtr handle);
    IntPtr handle;
    public GuoSSHellAuthJob() {
        handle = CreateJobObject(IntPtr.Zero, null);
        if (handle == IntPtr.Zero) throw new Win32Exception();
        var limits = new ExtendedLimit();
        limits.Basic.Flags = 0x2000;
        if (!SetInformationJobObject(handle, 9, ref limits, (uint)Marshal.SizeOf<ExtendedLimit>())) {
            int error = Marshal.GetLastWin32Error();
            CloseHandle(handle); handle = IntPtr.Zero;
            throw new Win32Exception(error);
        }
    }
    public void Add(Process process) {
        if (!AssignProcessToJobObject(handle, process.Handle)) throw new Win32Exception();
    }
    public bool Contains(Process process) {
        bool result;
        if (!IsProcessInJob(process.Handle, handle, out result)) throw new Win32Exception();
        return result;
    }
    public string Image(Process process) {
        uint size = 32768;
        var path = new System.Text.StringBuilder((int)size);
        if (!QueryFullProcessImageName(process.Handle, 0, path, ref size)) throw new Win32Exception();
        return path.ToString();
    }
    public uint Active {
        get {
            Accounting info;
            if (!QueryInformationJobObject(handle, 1, out info, (uint)Marshal.SizeOf<Accounting>(), IntPtr.Zero)) throw new Win32Exception();
            return info.ActiveProcesses;
        }
    }
    public void Stop() {
        if (!TerminateJobObject(handle, 1)) throw new Win32Exception();
    }
    public void Dispose() {
        if (handle != IntPtr.Zero) {
            if (!CloseHandle(handle)) throw new Win32Exception();
            handle = IntPtr.Zero;
        }
    }
}
'@
}

function Assert-OwnedDirectory([string] $Path, [string] $Expected) {
    if ([string]::IsNullOrWhiteSpace($Path) -or [string]::IsNullOrWhiteSpace($Expected)) {
        throw '清理目标未明确赋值'
    }
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).ProviderPath
    if (-not $item.PSIsContainer -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $resolved -cne $Expected) {
        throw '目录所有权核验失败，拒绝启动或删除'
    }
}

function Wait-JobEmpty($Job, [int] $Seconds) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($Job.Active -ne 0) {
        if ($clock.Elapsed.TotalSeconds -ge $Seconds) { throw '本轮进程树仍未退出' }
        # 限时条件轮询，不用固定 sleep 判定 ready 或清理成功。
        $null = $poll.WaitOne(50)
    }
}

function Stop-OwnedProcess($Process, $Job) {
    if ($null -ne $Job) { $Job.Stop() }
    if ($null -ne $Process) {
        $Process.Refresh()
        # Job 分配失败时也仅终止 Start-Process 返回的本轮句柄。
        if (-not $Process.HasExited) { $Process.Kill($true) }
        if (-not $Process.WaitForExit(15000)) { throw '本轮进程 WaitForExit 超时' }
        $Process.WaitForExit()
    }
    if ($null -ne $Job) { Wait-JobEmpty $Job 15 }
}

$names = @(
    'GUOSH_HOST', 'GUOSH_PORT', 'GUOSH_USER', 'GUOSH_PASS', 'GUOSH_CMD',
    'GUOSH_DEMO_DATA', 'GUOSH_DEMO_INPUT_LOG', 'GUOSH_DEMO_ENQ', 'GUOSH_DESKTOP_TEST_DATA',
    'GUOSH_AUTH_RUN', 'GUOSH_AUTH_RUN_ID', 'GUOSH_AUTH_PORT', 'GUOSH_AUTH_FINGERPRINT',
    'GUOSH_AUTH_REAL_KEYGEN', 'GUOSH_SYSTEM_SSH_CONFIG', 'PATH'
)
$previous = @{}
foreach ($name in $names) {
    $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$run = $null
$server = $null
$serverLauncher = $null
$keygenProcess = $null
$flutter = $null
$serverJob = $null
$flutterJob = $null
$gate = $null
$serverGate = $null
$keygenParentExit = $null
$poll = [Threading.ManualResetEvent]::new($false)
$cleanupErrors = [Collections.Generic.List[string]]::new()
$failure = $null
$passed = $false
$abortTriggered = $false
try {
    $tempParent = [IO.Path]::TrimEndingDirectorySeparator(
        (Resolve-Path -LiteralPath ([IO.Path]::GetTempPath())).ProviderPath
    )
    if (-not (Test-Path -LiteralPath $tempParent -PathType Container)) {
        throw '运行时临时目录不存在'
    }
    $runId = [Guid]::NewGuid().ToString('N')
    $candidate = Join-Path $tempParent "guossh-auth-$runId"
    if (Test-Path -LiteralPath $candidate) { throw '本轮临时目录已存在，拒绝接管' }
    $run = (New-Item -ItemType Directory -Path $candidate -ErrorAction Stop).FullName
    Assert-OwnedDirectory $run $run
    [IO.File]::WriteAllText((Join-Path $run '.owner'), $runId)
    $fixture = (New-Item -ItemType Directory -Path (Join-Path $run 'server')).FullName
    $support = (New-Item -ItemType Directory -Path (Join-Path $run 'app')).FullName
    Assert-OwnedDirectory $fixture (Join-Path $run 'server')
    Assert-OwnedDirectory $support (Join-Path $run 'app')
    "[windows-auth] 本轮独占目录、fixture/app 子目录及所有权标记已创建；run=$runId"

    Push-Location -LiteralPath $root
    try {
        & cargo build --locked -p rshell-m0 --example demo_server
        if ($LASTEXITCODE -ne 0) { throw 'demo_server locked 构建失败' }
        $metadataText = & cargo metadata --locked --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata 失败' }
        $metadata = $metadataText | ConvertFrom-Json
    } finally { Pop-Location }
    $binary = Join-Path $metadata.target_directory 'debug/examples/demo_server.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw '缺少 demo_server 产物' }

    $probe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try {
        $probe.Start()
        $port = ([Net.IPEndPoint] $probe.LocalEndpoint).Port
    } finally { $probe.Stop() }
    # 不使用 AppReady 的 debug 自动连接，必须从常驻真实目录发起。
    # ENQ 探针只由 profile 目标显式开启，继承值不得影响认证 smoke。
    foreach ($name in @('GUOSH_HOST', 'GUOSH_PORT', 'GUOSH_USER', 'GUOSH_PASS', 'GUOSH_CMD', 'GUOSH_DEMO_ENQ',
            'GUOSH_SYSTEM_SSH_CONFIG')) {
        if (Test-Path -LiteralPath "Env:$name") {
            Remove-Item -LiteralPath "Env:$name" -ErrorAction Stop
        }
    }
    $values = @{
        GUOSH_DEMO_DATA = $fixture
        GUOSH_DEMO_INPUT_LOG = (Join-Path $run 'received.txt')
        GUOSH_DESKTOP_TEST_DATA = $support
        GUOSH_AUTH_RUN = $run
        GUOSH_AUTH_RUN_ID = $runId
        GUOSH_AUTH_PORT = "$port"
    }
    if ($Target -eq 'profile') { $values.GUOSH_DEMO_ENQ = '1' }
    if ($Target -eq 'system') {
        # 只在测试夹具层面隔离：配置、known_hosts、身份文件全在本轮目录；主机名别名只在此配置里
        # 定义，配置未生效时 ssh 无法解析目标，不会连到别处，也不会写用户自己的 known_hosts。
        $sshDir = (New-Item -ItemType Directory -Path (Join-Path $run 'ssh')).FullName
        Assert-OwnedDirectory $sshDir (Join-Path $run 'ssh')
        $sshPath = { param([string] $name) '"' + ((Join-Path $sshDir $name) -replace '\\', '/') + '"' }
        [IO.File]::WriteAllText((Join-Path $sshDir 'global_known_hosts'), '')
        $sshConfig = @(
            'Host guosh-system-smoke'
            '  HostName 127.0.0.1'
            "  Port $port"
            'Host *'
            "  UserKnownHostsFile $(& $sshPath 'known_hosts')"
            "  GlobalKnownHostsFile $(& $sshPath 'global_known_hosts')"
            "  IdentityFile $(& $sshPath 'no_identity')"
            '  IdentitiesOnly yes'
            '  IdentityAgent none'
            '  PubkeyAuthentication no'
            '  PreferredAuthentications password'
            '  UpdateHostKeys no'
            '  NumberOfPasswordPrompts 1'
        ) -join "`n"
        [IO.File]::WriteAllText((Join-Path $sshDir 'config'), "$sshConfig`n")
        $values.GUOSH_SYSTEM_SSH_CONFIG = Join-Path $sshDir 'config'
    }
    foreach ($entry in $values.GetEnumerator()) {
        [Environment]::SetEnvironmentVariable($entry.Key, $entry.Value, 'Process')
    }
    # demo_server 无此变量会写 HOME；启动前必须再次 fail-closed 核验本轮目录。
    if ([string]::IsNullOrWhiteSpace($env:GUOSH_DEMO_DATA) -or
        $env:GUOSH_DEMO_DATA -cne $fixture -or
        $env:GUOSH_DEMO_INPUT_LOG -cne (Join-Path $run 'received.txt')) {
        throw 'fixture 环境隔离失败，拒绝启动 server'
    }
    Assert-OwnedDirectory $run $run
    Assert-OwnedDirectory $env:GUOSH_DEMO_DATA (Join-Path $run 'server')
    if ([IO.File]::ReadAllText((Join-Path $run '.owner')) -cne $runId) { throw '所有权标记变更' }

    if ($VerifyKeygenAbort) {
        $compiler = Join-Path $env:WINDIR 'Microsoft.NET/Framework64/v4.0.30319/csc.exe'
        if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) { throw '缺少系统已有编译器，不安装软件' }
        $env:GUOSH_AUTH_REAL_KEYGEN = (Get-Command ssh-keygen -ErrorAction Stop).Source
        $shim = (New-Item -ItemType Directory -Path (Join-Path $run 'keygen-probe')).FullName
        Assert-OwnedDirectory $shim (Join-Path $run 'keygen-probe')
        & $compiler /nologo /target:exe "/out:$(Join-Path $shim 'ssh-keygen.exe')" `
            (Join-Path $PSScriptRoot 'windows_auth_keygen_probe.cs')
        if ($LASTEXITCODE -ne 0) { throw '受控派生探针编译失败' }
        $env:PATH = "$shim;$env:PATH"
        $keygenParentExit = [Threading.EventWaitHandle]::new(
            $false, [Threading.EventResetMode]::ManualReset,
            "Local\GuoSSHell-auth-$runId-keygen-parent-exit"
        )
    }
    $pwsh = (Get-Command pwsh -ErrorAction Stop).Source
    $serverGateName = "Local\GuoSSHell-auth-$runId-server"
    $serverGate = [Threading.EventWaitHandle]::new(
        $false, [Threading.EventResetMode]::ManualReset, $serverGateName
    )
    $serverJob = [GuoSSHellAuthJob]::new()
    $serverLauncher = Start-Process -FilePath $pwsh -ArgumentList @(
        '-NoProfile', '-File', "`"$PSCommandPath`"", '-ServerChild',
        '-GateName', $serverGateName, '-ServerBinary', "`"$binary`""
    ) -WorkingDirectory $root `
        -RedirectStandardOutput (Join-Path $run 'server.stdout.log') `
        -RedirectStandardError (Join-Path $run 'server.stderr.log') -NoNewWindow -PassThru
    $serverJob.Add($serverLauncher)
    if (-not $serverJob.Contains($serverLauncher)) { throw 'server 包装进程未归本轮 Job' }
    $null = $serverGate.Set()
    $pidClock = [Diagnostics.Stopwatch]::StartNew()
    $pidPath = Join-Path $run 'server.pid'
    while (-not (Test-Path -LiteralPath $pidPath -PathType Leaf)) {
        $serverLauncher.Refresh()
        if ($serverLauncher.HasExited) { throw 'server 包装进程未发布真实 PID 即退出' }
        if ($pidClock.Elapsed.TotalSeconds -ge 30) { throw '真实 server PID 发布超时' }
        $null = $serverLauncher.WaitForExit(50)
    }
    $actualPid = 0
    if (-not [int]::TryParse([IO.File]::ReadAllText($pidPath), [ref] $actualPid) -or $actualPid -le 0) {
        throw '真实 server PID 格式不符'
    }
    $server = [Diagnostics.Process]::GetProcessById($actualPid)
    $null = $server.Handle
    if ($server.Id -eq $serverLauncher.Id -or -not $serverJob.Contains($server) -or
        $serverJob.Image($server) -ine $binary) { throw '真实 server 身份或 Job 归属不符' }
    $serverPid = $server.Id
    '[windows-auth] server 执行前启动门已归 Job；真实 demo_server PID/映像/Job 已核对，不使用包装 PID'
    if ($VerifyKeygenAbort) {
        $keygenPidPath = Join-Path $run 'keygen.pid'
        $keygenClock = [Diagnostics.Stopwatch]::StartNew()
        while (-not (Test-Path -LiteralPath $keygenPidPath -PathType Leaf)) {
            $server.Refresh()
            if ($server.HasExited) { throw '首次密钥派生探针就绪前 server 退出' }
            if ($keygenClock.Elapsed.TotalSeconds -ge 30) { throw '首次密钥派生窗口等待超时' }
            $null = $server.WaitForExit(50)
        }
        $keygenPid = 0
        if (-not [int]::TryParse([IO.File]::ReadAllText($keygenPidPath), [ref] $keygenPid) -or $keygenPid -le 0) {
            throw '真实 ssh-keygen PID 格式不符'
        }
        $keygenProcess = [Diagnostics.Process]::GetProcessById($keygenPid)
        $null = $keygenProcess.Handle
        if (-not $serverJob.Contains($keygenProcess) -or $keygenProcess.HasExited -or
            $serverJob.Image($keygenProcess) -ine $env:GUOSH_AUTH_REAL_KEYGEN -or
            (Test-Path -LiteralPath (Join-Path $fixture 'hostkey'))) {
            throw '首次密钥生成窗口或真实 ssh-keygen 归属不符'
        }
        $null = $keygenParentExit.Set()
        if (-not $server.WaitForExit(15000) -or -not $serverLauncher.WaitForExit(15000)) {
            throw '受控父进程退出超时'
        }
        $serverLauncher.WaitForExit()
        if ($keygenProcess.HasExited -or $serverJob.Active -ne 1) {
            throw '未形成父进程已退出但真实密钥子进程仍存活的受控窗口'
        }
        '[windows-auth] 受控失败证据：首次真实 ssh-keygen 已派生并归 Job；父进程已退出，仅此挂起子进程仍 alive'
        $abortTriggered = $true
        throw [OperationCanceledException]::new('AUTH_SMOKE_CONTROLLED_KEYGEN_ABORT')
    }
    $readyClock = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $server.Refresh()
        if ($server.HasExited) { throw '本轮 server 提前退出；端口竞态或 fixture 启动失败' }
        $listeners = @(Get-NetTCPConnection -State Listen -ErrorAction Stop |
            Where-Object { $_.LocalPort -eq $port -and $_.LocalAddress -eq '127.0.0.1' })
        if ($listeners.Count -ne 0) {
            if ($listeners.Count -ne 1 -or $listeners[0].OwningProcess -ne $serverPid) {
                throw '端口被非本轮 server 占用，拒绝继续'
            }
            $server.Refresh()
            if ($server.HasExited) { throw 'server 监听后退出' }
            break
        }
        if ($readyClock.Elapsed.TotalSeconds -ge 30) { throw '本轮 server 监听超时' }
        $null = $server.WaitForExit(50)
    }
    $publicKey = Join-Path $fixture 'hostkey.pub'
    $keyInfo = & ssh-keygen -lf $publicKey -E sha256
    if ($LASTEXITCODE -ne 0 -or $keyInfo -notmatch ' (SHA256:[A-Za-z0-9+/]+) ') {
        throw '无法取得本轮 fixture 指纹'
    }
    $env:GUOSH_AUTH_FINGERPRINT = $Matches[1]
    '[windows-auth] locked fixture 构建成功；本轮 server PID 已确证独占 loopback 监听'

    $gateName = "Local\GuoSSHell-auth-$runId-flutter"
    $gate = [Threading.EventWaitHandle]::new($false, [Threading.EventResetMode]::ManualReset, $gateName)
    $flutterJob = [GuoSSHellAuthJob]::new()
    $flutter = Start-Process -FilePath $pwsh -ArgumentList @(
        '-NoProfile', '-File', "`"$PSCommandPath`"", '-FlutterChild', '-GateName', $gateName,
        '-Target', $Target
    ) -WorkingDirectory $root -NoNewWindow -PassThru `
        -RedirectStandardOutput (Join-Path $run 'flutter.stdout.log') `
        -RedirectStandardError (Join-Path $run 'flutter.stderr.log')
    $flutterJob.Add($flutter)
    $null = $gate.Set()
    $testClock = [Diagnostics.Stopwatch]::StartNew()
    while (-not $flutter.WaitForExit(100)) {
        $server.Refresh()
        if ($server.HasExited) { throw '测试期间本轮 server 退出' }
        if ($testClock.Elapsed.TotalSeconds -ge $TimeoutSeconds) { throw 'Flutter 定向测试超时' }
    }
    $flutter.WaitForExit()
    [IO.File]::ReadAllText((Join-Path $run 'flutter.stdout.log'))
    [IO.File]::ReadAllText((Join-Path $run 'flutter.stderr.log'))
    if ($flutter.ExitCode -ne 0) { throw 'Windows 认证定向测试失败' }
    Wait-JobEmpty $flutterJob 15
    if (Test-Path -LiteralPath $support) { throw '测试未释放并删除 SQLite support' }
    $server.Refresh()
    if ($server.HasExited) { throw '测试完成时 server 已非 alive' }
    # Start-Process 持有重定向写句柄；结束本轮 server 后再读取完整日志，finally 仍兜底。
    Stop-OwnedProcess $server $serverJob
    Stop-OwnedProcess $serverLauncher $null
    $serverLog = [IO.File]::ReadAllText((Join-Path $run 'server.stderr.log'))
    foreach ($marker in @('password accepted', 'channel_open_session', 'shell_request -> starting demo stream')) {
        if (-not $serverLog.Contains($marker)) { throw 'fixture 缺少真实认证/channel/shell 证据' }
    }
    if ($serverLog.Contains('exec_request')) { throw 'fixture 错走 exec，本轮不能算通过' }
    $received = [IO.File]::ReadAllText((Join-Path $run 'received.txt')) -replace '\s', ''
    if ($Target -eq 'profile') {
        foreach ($marker in @('pty_request term=vt220 ', 'pty_request term=xterm-256color ')) {
            if (-not $serverLog.Contains($marker)) { throw 'fixture 缺少 profile/默认 TERM 证据' }
        }
        # 非默认 profile：answerback GUOSH-AB、F5→DEL、左 Alt+x→ESC x、右 Alt+x→x；
        # 默认 profile：其 answerback（逐字节由 Dart 端核对）后 F5→CSI 15~。
        $customHex = [Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes('GUOSH-AB')).ToLowerInvariant() + '7f1b7878'
        if (-not $received.StartsWith($customHex, [StringComparison]::Ordinal) -or
            -not $received.EndsWith('1b5b31357e', [StringComparison]::Ordinal)) {
            throw 'fixture profile 输入日志不符'
        }
        '[windows-auth] fixture 证据：TERM vt220/xterm-256color；无 exec；profile 输入日志首尾匹配'
    } elseif ($Target -eq 'system') {
        $echoHex = [Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes('systemsmoke42')).ToLowerInvariant()
        if ($received -cne $echoHex) { throw 'fixture 系统 ssh 输入日志不精确匹配' }
        # 系统 ssh 只把本轮 fixture 主机密钥记进本轮 known_hosts。
        $hostKey = ([IO.File]::ReadAllText((Join-Path $fixture 'hostkey.pub')).Trim() -split '\s+')[1]
        $recorded = @([IO.File]::ReadAllLines((Join-Path $run 'ssh/known_hosts')) |
            Where-Object { $_.Trim() -ne '' })
        if ($recorded.Count -ne 1 -or -not $recorded[0].Contains("ssh-ed25519 $hostKey")) {
            throw '本轮 known_hosts 未精确记录 fixture 主机密钥'
        }
        '[windows-auth] fixture 证据：系统 ssh 密码认证 / channel / shell；无 exec；输入日志精确匹配；known_hosts 仅本轮 fixture'
    } else {
        $echoHex = [Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes('authsmokeecho123authaftermanagement456')).ToLowerInvariant()
        if ($received -cne $echoHex) { throw 'fixture 原生输入日志不精确匹配' }
        '[windows-auth] fixture 证据：password accepted / channel / shell；无 exec；输入日志精确匹配'
    }
    $passed = $true
} catch {
    $failure = $_
    "[windows-auth] 原始失败：$($failure.Exception.Message)"
    # 失败也输出实际 Flutter 日志，但不输出 fixture 私钥、密码或公钥。
    if ($null -ne $run) {
        foreach ($logName in @('flutter.stdout.log', 'flutter.stderr.log')) {
            $logPath = Join-Path $run $logName
            if (Test-Path -LiteralPath $logPath -PathType Leaf) { [IO.File]::ReadAllText($logPath) }
        }
    }
} finally {
    try { Stop-OwnedProcess $flutter $flutterJob } catch { $cleanupErrors.Add("Flutter 清理失败：$_") }
    try { Stop-OwnedProcess $server $serverJob } catch { $cleanupErrors.Add("server 清理失败：$_") }
    try { Stop-OwnedProcess $serverLauncher $null } catch { $cleanupErrors.Add("server 包装进程清理失败：$_") }
    if ($null -ne $keygenProcess) {
        try {
            if (-not $keygenProcess.WaitForExit(15000)) { throw 'Job 未终止真实 ssh-keygen' }
            $keygenProcess.WaitForExit()
            '[windows-auth] 真实 ssh-keygen 在父进程已退出后仍由 Job 终止；持有句柄 WaitForExit 已确认'
        } catch {
            $cleanupErrors.Add("ssh-keygen Job 清理核验失败：$_")
            try { Stop-OwnedProcess $keygenProcess $null } catch { $cleanupErrors.Add("ssh-keygen 句柄兜底失败：$_") }
        }
    }
    if ($cleanupErrors.Count -eq 0) { '[windows-auth] 本轮已启动的进程树均已 WaitForExit，已创建 Job active=0' }
    foreach ($resource in @($gate, $serverGate, $keygenParentExit, $flutterJob, $serverJob,
        $flutter, $server, $serverLauncher, $keygenProcess, $poll)) {
        if ($null -ne $resource) {
            try { $resource.Dispose() } catch { $cleanupErrors.Add("本轮句柄释放失败：$_") }
        }
    }
    if ($null -ne $run) {
        try {
            Assert-OwnedDirectory $run $run
            if ([IO.Path]::GetDirectoryName($run) -cne $tempParent -or
                [IO.Path]::GetFileName($run) -cne "guossh-auth-$runId" -or
                [IO.File]::ReadAllText((Join-Path $run '.owner')) -cne $runId) {
                throw '清理所有权不符'
            }
            # 仅递归删除本轮创建、刚刚 resolve 和核验过的精确目录；失败必须上报。
            Remove-Item -LiteralPath $run -Recurse -Force -ErrorAction Stop
            if (Test-Path -LiteralPath $run) { throw '本轮临时目录仍存在' }
            '[windows-auth] fixture/app/temp 所有权已复核；本轮精确目录删除成功'
        } catch { $cleanupErrors.Add("目录清理失败：$_") }
    }
    foreach ($name in $names) {
        try {
            if ($null -eq $previous[$name]) {
                if (Test-Path -LiteralPath "Env:$name") {
                    Remove-Item -LiteralPath "Env:$name" -ErrorAction Stop
                }
            } else {
                [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
            }
            if ([Environment]::GetEnvironmentVariable($name, 'Process') -cne $previous[$name]) {
                throw '环境变量恢复不匹配'
            }
        } catch { $cleanupErrors.Add("环境恢复失败：$name") }
    }
    if ($cleanupErrors.Count -eq 0) { '[windows-auth] 本 task 修改的全部环境变量已恢复' }
}
if ($cleanupErrors.Count -ne 0) { throw ($cleanupErrors -join '; ') }
if ($VerifyKeygenAbort -and $abortTriggered -and $failure.Exception -is [OperationCanceledException]) {
    '[windows-auth] KEYGEN-ABORT PASS：受控首次派生中止，已退出父进程的真实子进程无残留；目录/环境清理成功（非 SSH 成功链）'
    return
}
if ($null -ne $failure) { throw $failure }
if (-not $passed) { throw '缺少本轮通过证据' }
if ($Target -eq 'profile') {
    '[windows-auth] PROFILE PASS：设置协议建 profile→V2 会话 TERM/answerback/绑定/左右 Alt 生效，默认 profile 对照通过；清理无残留'
    return
}
if ($Target -eq 'system') {
    '[windows-auth] SYSTEM PASS：编辑器保存 System 连接→系统 ssh 在终端内首见主机密钥与密码→回显→关闭窗格；清理无残留'
    return
}
'[windows-auth] PASS：Windows 阶段三真实目录→Fluent 首见认证→Native SSH 回显→断开/返回管理；清理无残留'
