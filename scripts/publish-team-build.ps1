# Publishes the last `npm run tauri build` to the team hub on this PC (D042): teammates' Settings > Check for updates
# offers it. Run from the repo after building:  powershell -File scripts\publish-team-build.ps1
$ErrorActionPreference = "Stop"
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot "..\src-tauri\target" }
$exe = Join-Path $target "release\papertrail-command-center.exe"
$setup = Get-ChildItem (Join-Path $target "release\bundle\nsis") -Filter "*-setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not (Test-Path $exe) -or -not $setup) { throw "Build first: npm run tauri build" }
if ($setup.LastWriteTime -lt (Get-Item $exe).LastWriteTime.AddMinutes(-10)) { throw "The installer is older than the exe; rebuild." }
$share = Join-Path $env:LOCALAPPDATA "ptcc-hub\share"
New-Item -ItemType Directory -Force $share | Out-Null
Copy-Item $setup.FullName (Join-Path $share "Papertrail Command Center setup.exe") -Force
$latest = [ordered]@{
  exe_sha256       = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
  installer_sha256 = (Get-FileHash $setup.FullName -Algorithm SHA256).Hash.ToLower()
  commit           = (git -C $PSScriptRoot rev-parse --short HEAD)
  built_at         = $setup.LastWriteTimeUtc.ToString("yyyy-MM-ddTHH:mm:ssZ")
  size             = $setup.Length
}
$latest | ConvertTo-Json | Set-Content (Join-Path $share "latest.json") -Encoding utf8
"Published build $($latest.commit) ($([math]::Round($setup.Length / 1MB, 1)) MB) to $share"
