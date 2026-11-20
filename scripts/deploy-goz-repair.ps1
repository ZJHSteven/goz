<#
.SYNOPSIS
  在管理员授权下，安全试运行基于官方v0.7.1的goz硬链接修复版。

.DESCRIPTION
  只操作已安装的 Windows 服务 goz 与 C:\Program Files\goz 下的两个EXE。
  不更改USN日志、不删除用户文件、不更改服务账号/启动模式/注册表配置。
  从编译产物复制二进制文件前后均验证固定SHA256；原版保存在
  Program Files 下受系统ACL保护的回滚目录。
  服务启动且 C/D 卷恢复 Live 前，均不宣告升级完成。
  失败时优先恢复原有两个可执行文件并重启服务。

.EXAMPLE
  pwsh -NoProfile -File .\scripts\deploy-goz-repair.ps1
  pwsh -NoProfile -File .\scripts\deploy-goz-repair.ps1 -Apply
  pwsh -NoProfile -File .\scripts\deploy-goz-repair.ps1 -Rollback
#>
param(
    [switch]$Apply,
    [switch]$Rollback
)

$ErrorActionPreference = 'Stop'
if ($Apply -and $Rollback) {
    throw '不能同时部署和回滚'
}

$root = 'D:\Workspace\goz-v071-hardlink-repair'
$sourceDir = Join-Path $root 'target\release'
$installDir = 'C:\Program Files\goz'
$backupDir = Join-Path $installDir 'rollback-20261120-v055'
$expected = @{
    'gozd.exe' = '1E36094B45EA80A79315593072736787A1946F3DC81F552719003B3CC4FD617A'
    'goz.exe'  = '7DDB23E6EDA426AAF0FBF5BF7E5D9CFD9E54C1778852991155067F663B40CD94'
}

function Assert-Hash([string]$path, [string]$expectedHash) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "文件不存在: $path"
    }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    if ($actual -ne $expectedHash) {
        throw "SHA256不符，拒绝安装: $path"
    }
}

function Stop-GoZ {
    if ((Get-Service -Name goz).Status -ne 'Stopped') {
        Stop-Service -Name goz -Force
        (Get-Service -Name goz).WaitForStatus('Stopped', [TimeSpan]::FromSeconds(60))
    }
}

function Start-GoZ {
    if ((Get-Service -Name goz).Status -ne 'Running') {
        Start-Service -Name goz
        (Get-Service -Name goz).WaitForStatus('Running', [TimeSpan]::FromSeconds(45))
    }
}

function Wait-ForIndexes {
    $client = Join-Path $installDir 'goz.exe'
    # 新启动的gozd须重建数百万条MFT，不能只以SCM的Running作为完成。
    $deadline = (Get-Date).AddMinutes(3)
    do {
        $status = @(& $client --status 2>&1)
        $exit = $LASTEXITCODE
        $text = $status -join "`n"
        $cLive = $text -match '(?m)^  C:\\.*phase=live'
        $dLive = $text -match '(?m)^  D:\\.*phase=live'
        if ($exit -eq 0 -and $cLive -and $dLive) {
            Write-Output 'INDEXES_LIVE'
            $status | Select-String 'phase=|hard-link|drift:|daemon:' | ForEach-Object { $_.Line }
            return
        }
        Start-Sleep -Seconds 5
    } while ((Get-Date) -lt $deadline)
    throw '等待3分钟后C/D卷仍未全部恢复Live'
}

function Restore-Original {
    $backedUpDaemon = Join-Path $backupDir 'gozd.exe'
    $backedUpClient = Join-Path $backupDir 'goz.exe'
    if (-not (Test-Path $backedUpDaemon) -or -not (Test-Path $backedUpClient)) {
        throw "缺少完整备份，禁止执行不完整回滚：$backupDir"
    }
    $manifest = Get-Content -LiteralPath (Join-Path $backupDir 'manifest.json') -Raw |
        ConvertFrom-Json
    Assert-Hash $backedUpDaemon $manifest.goZdSha256
    Assert-Hash $backedUpClient $manifest.goZSha256
    Stop-GoZ
    Copy-Item -LiteralPath $backedUpDaemon -Destination (Join-Path $installDir 'gozd.exe') -Force
    Copy-Item -LiteralPath $backedUpClient -Destination (Join-Path $installDir 'goz.exe') -Force
    Start-GoZ
    Wait-ForIndexes
    Write-Output 'ROLLED_BACK_TO_ORIGINAL'
}

if ($Rollback) {
    Restore-Original
    exit 0
}

foreach ($name in @('gozd.exe', 'goz.exe')) {
    Assert-Hash (Join-Path $sourceDir $name) $expected[$name]
}
$service = Get-CimInstance Win32_Service -Filter "Name='goz'"
if (-not $service -or $service.PathName -ne
    'C:\Program Files\goz\gozd.exe run --service') {
    throw '现有goz服务路径与预期不符，拒绝更改'
}
Write-Output "SOURCE_VERIFIED: $sourceDir"
Write-Output "SERVICE: $($service.State) $($service.StartMode) $($service.PathName)"
Write-Output "BACKUP: $backupDir"

if (-not $Apply) {
    Write-Output 'PLAN_ONLY: 所有文件与服务均未更改'
    exit 0
}
if (Test-Path -LiteralPath $backupDir) {
    throw '备份目录已经存在：不自动覆盖，也不重复升级'
}

# 先备份正在使用的旧文件，再停止服务。备份留在管理员保护目录，
# 并记录原始哈希，使以后回滚无需信任用户可写的工作区内容。
New-Item -ItemType Directory -Path $backupDir -Force | Out-Null
$oldHashes = @{}
foreach ($name in @('gozd.exe', 'goz.exe')) {
    $from = Join-Path $installDir $name
    $to = Join-Path $backupDir $name
    $oldHashes[$name] = (Get-FileHash -LiteralPath $from -Algorithm SHA256).Hash
    Copy-Item -LiteralPath $from -Destination $to
    Assert-Hash $to $oldHashes[$name]
}
@{
    goZdSha256 = $oldHashes['gozd.exe']
    goZSha256 = $oldHashes['goz.exe']
    backupCreatedAt = (Get-Date).ToString('o')
} | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $backupDir 'manifest.json') -Encoding utf8

try {
    Stop-GoZ
    foreach ($name in @('gozd.exe', 'goz.exe')) {
        Copy-Item -LiteralPath (Join-Path $sourceDir $name) -Destination (Join-Path $installDir $name) -Force
        Assert-Hash (Join-Path $installDir $name) $expected[$name]
    }
    Start-GoZ
    Wait-ForIndexes
    Write-Output 'UPGRADE_COMPLETED: original backup preserved for rollback'
} catch {
    $errorMessage = $_.Exception.Message
    Write-Warning "升级检查失败：$errorMessage；尝试恢复旧版"
    Restore-Original
    throw "已执行旧版回滚，升级未完成：$errorMessage"
}
