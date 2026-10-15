param(
  [Parameter(Mandatory = $true)][string]$OutputRoot,
  [switch]$Cleanup,
  [switch]$LibraryOnly
)
$ErrorActionPreference = 'Stop'
$parent = Join-Path ([IO.Path]::GetTempPath()) 'opencode'
if (-not (Test-Path -LiteralPath $parent)) { throw '临时父目录不存在' }
$resolvedParent = [IO.Path]::GetFullPath($parent)
$resolved = [IO.Path]::GetFullPath($OutputRoot)
if ([IO.Path]::GetDirectoryName($resolved) -ne $resolvedParent -or
    [IO.Path]::GetFileName($resolved) -notmatch '^guossh-c-native-[a-f0-9]{32}$') {
  throw '只允许本任务命名的直接临时子目录'
}
$artifacts = @('text_store.obj', 'layout_metadata.obj', 'native_surface.obj',
               'native_fixture.obj', 'native_fixture.exe', 'native_fixture.pdb',
               'native_fixture.ilk', 'vc140.pdb', 'vc140.idb')
if ($Cleanup) {
  $directory = Get-Item -LiteralPath $resolved -Force
  if (-not $directory.PSIsContainer -or
      ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw '清理目标必须是实际目录，不能是文件或 reparse point'
  }
  foreach ($name in $artifacts) {
    $target = Join-Path $resolved $name
    if (Test-Path -LiteralPath $target) {
      $item = Get-Item -LiteralPath $target -Force
      if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw '构建产物必须是普通文件'
      }
    }
  }
  foreach ($name in $artifacts) {
    $target = Join-Path $resolved $name
    if (Test-Path -LiteralPath $target) { Remove-Item -LiteralPath $target }
  }
  $directory = Get-Item -LiteralPath $resolved -Force
  if (-not $directory.PSIsContainer -or
      ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw '最终清理目录类型已变化'
  }
  # false 禁止递归，也不会触发 PowerShell 的非空目录确认提示。
  [IO.Directory]::Delete($resolved, $false)
  return
}
if (Test-Path -LiteralPath $resolved) { throw '拒绝覆盖已有构建目录' }
$null = New-Item -ItemType Directory -Path $resolved
$vc = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207'
$sdk = 'C:\Program Files (x86)\Windows Kits\10'
$cl = Join-Path $vc 'bin\Hostx64\x64\cl.exe'
if (-not (Test-Path -LiteralPath $cl)) { throw '既有 MSVC 不可用；不安装工具' }
$include = @("/I$vc\include", "/I$sdk\Include\10.0.22621.0\ucrt",
             "/I$sdk\Include\10.0.22621.0\shared", "/I$sdk\Include\10.0.22621.0\um",
             "/I$sdk\Include\10.0.22621.0\winrt")
$sources = @('text_store', 'layout_metadata', 'native_surface')
$fixtureDefines = @()
if (-not $LibraryOnly) {
  $sources += 'native_fixture'
  $fixtureDefines += '/DGUOSSH_NATIVE_FIXTURE'
}
foreach ($source in $sources) {
  & $cl /nologo /c /std:c++17 /EHsc /utf-8 /W4 /WX /Z7 /DUNICODE /D_UNICODE /DNOMINMAX @fixtureDefines @include "/Fo$resolved\$source.obj" "$PSScriptRoot\$source.cpp"
  if ($LASTEXITCODE -ne 0) { throw "编译失败：$source" }
}
if ($LibraryOnly) { return }
$objects = $sources | ForEach-Object { "$resolved\$_.obj" }
& $cl /nologo @objects "/Fe$resolved\native_fixture.exe" /link /DEBUG /INCREMENTAL:NO "/PDB:$resolved\native_fixture.pdb" "/LIBPATH:$vc\lib\x64" "/LIBPATH:$sdk\Lib\10.0.22621.0\um\x64" "/LIBPATH:$sdk\Lib\10.0.22621.0\ucrt\x64" user32.lib ole32.lib oleaut32.lib uuid.lib
if ($LASTEXITCODE -ne 0) { throw '链接失败' }
& "$resolved\native_fixture.exe"
if ($LASTEXITCODE -ne 0) { throw "fixture 失败：$LASTEXITCODE" }
