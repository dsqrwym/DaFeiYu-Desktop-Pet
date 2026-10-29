# 拉取上游素材仓库（只读参考，不修改它）。
#
# 为什么需要：动画素材（100 段 webm，约 55MB）不入库，仓库只带打包工具。
# 拉下来之后跑：
#   cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/h264 --codec h264 --crf 20
# 旧 .fishanim（可选，保留兼容/对照）：
#   cargo run -p pack-anim --release -- --src _upstream/dsh-pet/dsh-pet/assets/webm --out assets/anim
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File tools/fetch-assets.ps1

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root '_upstream\dsh-pet'

if (Test-Path $dest) {
    Write-Host "已存在，跳过：$dest"
    exit 0
}

New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null

Write-Host "正在克隆 dsh-pet（浅克隆，只取最新一次提交）..."
git clone --depth 1 https://github.com/PC2005-cloud/dsh-pet.git $dest

$webm = Join-Path $dest 'dsh-pet\assets\webm'
if (-not (Test-Path $webm)) {
    throw "克隆完成但找不到素材目录：$webm"
}

$count = (Get-ChildItem $webm -Filter *.webm | Measure-Object).Count
Write-Host "完成：$webm（$count 段 webm）"
Write-Host "下一步：cargo run -p pack-anim --release -- --src `"$webm`" --out assets/h264 --codec h264 --crf 20"
