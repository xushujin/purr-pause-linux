# 胖猫暂停一下（PurrPause） 完全清理脚本 (Windows)
# 以管理员身份运行 PowerShell 执行此脚本

Write-Host "=== 胖猫暂停一下（PurrPause） 清理工具 (Windows) ===" -ForegroundColor Cyan
Write-Host ""

# 卸载程序（NSIS 安装的）
$uninstaller = "$env:LOCALAPPDATA\Programs\purr-pause\Uninstall purr-pause.exe"
if (Test-Path $uninstaller) {
    Write-Host "正在卸载 purr-pause..."
    Start-Process -FilePath $uninstaller -ArgumentList "/S" -Wait
    Write-Host "  已卸载"
} else {
    Write-Host "  未检测到已安装的程序"
}

Write-Host ""
Write-Host "正在清理配置文件和数据..."

# AppData 配置目录
$appData = "$env:APPDATA\purr-pause"
if (Test-Path $appData) {
    Remove-Item -Recurse -Force $appData
    Write-Host "  已删除: $appData"
}

# Local AppData
$localAppData = "$env:LOCALAPPDATA\purr-pause"
if (Test-Path $localAppData) {
    Remove-Item -Recurse -Force $localAppData
    Write-Host "  已删除: $localAppData"
}

# 安装目录
$installDir = "$env:LOCALAPPDATA\Programs\purr-pause"
if (Test-Path $installDir) {
    Remove-Item -Recurse -Force $installDir
    Write-Host "  已删除: $installDir"
}

# 辅助标记文件
$markFile = "$env:LOCALAPPDATA\.purr-pause-mark"
if (Test-Path $markFile) {
    Remove-Item -Force $markFile
    Write-Host "  已删除: $markFile"
}

# 注册表自启动项
$regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$regValue = Get-ItemProperty -Path $regPath -Name "purr-pause" -ErrorAction SilentlyContinue
if ($regValue) {
    Remove-ItemProperty -Path $regPath -Name "purr-pause"
    Write-Host "  已移除注册表自启动项"
}

Write-Host ""
Write-Host "清理完成！" -ForegroundColor Green
