$ErrorActionPreference = "Stop"
$Version = if ($env:DUOLA_AGENTCOST_VERSION) { $env:DUOLA_AGENTCOST_VERSION } else { "0.1.0" }
$Base = if ($env:DUOLA_AGENTCOST_RELEASE_BASE_URL) { $env:DUOLA_AGENTCOST_RELEASE_BASE_URL } else { "https://agentcost.manyaitool.com/downloads" }
$Name = "duola-agentcost-v$Version-x86_64-pc-windows-msvc"
$Temp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Force $Temp | Out-Null
try {
  Invoke-WebRequest "$Base/$Name.zip" -OutFile "$Temp/$Name.zip"
  Invoke-WebRequest "$Base/$Name.zip.sha256" -OutFile "$Temp/$Name.zip.sha256"
  $expected = ((Get-Content "$Temp/$Name.zip.sha256" -Raw) -split '\s+')[0].ToLower()
  $actual = (Get-FileHash "$Temp/$Name.zip" -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw "安装包校验失败" }
  Expand-Archive "$Temp/$Name.zip" -DestinationPath $Temp -Force
  $BinDir = Join-Path $env:LOCALAPPDATA "DuoLA\AgentCost\bin"
  New-Item -ItemType Directory -Force $BinDir | Out-Null
  Copy-Item "$Temp\$Name\duola-agentcost.exe" "$BinDir\duola-agentcost.exe" -Force
  & "$BinDir\duola-agentcost.exe" setup --non-interactive
  Write-Host "安装完成：duola-agentcost launch codex --open-dashboard"
} finally {
  Remove-Item $Temp -Recurse -Force -ErrorAction SilentlyContinue
}
