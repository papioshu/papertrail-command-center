# Publishes the last `npm run tauri build` to the team hub on this PC (D042): teammates' Settings > Check for updates
# offers it. Run from the repo after building:  powershell -File scripts\publish-team-build.ps1
$ErrorActionPreference = "Stop"
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $PSScriptRoot "..\src-tauri\target" }
$exe = Join-Path $target "release\papertrail-command-center.exe"
$setup = Get-ChildItem (Join-Path $target "release\bundle\nsis") -Filter "*-setup.exe" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not (Test-Path $exe) -or -not $setup) { throw "Build first: npm run tauri build" }
if ($setup.LastWriteTime -lt (Get-Item $exe).LastWriteTime.AddMinutes(-10)) { throw "The installer is older than the exe; rebuild." }
# Built in another checkout (worktree) or before the last commit: refuse, or teammates get an old build under a new label.
if ($setup.LastWriteTime -lt [datetime](git -C $PSScriptRoot log -1 --format=%cI -- ../src ../src-tauri ../package.json)) { throw "The installer in $target is older than the last commit; build here first (npx tauri build)." }
# The bundler stamps the exe it packs, so the build-folder exe never matches an installed one: hash the copy this
# installer put on this PC (install it first), which is byte-for-byte what teammates get.
$installed = Join-Path $env:LOCALAPPDATA "Papertrail Command Center\papertrail-command-center.exe"
if (-not (Test-Path $installed) -or (Get-Item $installed).LastWriteTime -lt $setup.LastWriteTime.AddMinutes(-10)) { throw "Install this build on this PC first: & '$($setup.FullName)' /S" }
$share = Join-Path $env:LOCALAPPDATA "ptcc-hub\share"
New-Item -ItemType Directory -Force $share | Out-Null
Copy-Item $setup.FullName (Join-Path $share "Papertrail Command Center setup.exe") -Force
$latest = [ordered]@{
  exe_sha256       = (Get-FileHash $installed -Algorithm SHA256).Hash.ToLower()
  installer_sha256 = (Get-FileHash $setup.FullName -Algorithm SHA256).Hash.ToLower()
  commit           = (git -C $PSScriptRoot rev-parse --short HEAD)
  built_at         = $setup.LastWriteTimeUtc.ToString("yyyy-MM-ddTHH:mm:ssZ")
  size             = $setup.Length
}
# No BOM: Windows PowerShell's utf8 adds one and the hub's JSON reader then sees no build at all.
[IO.File]::WriteAllText((Join-Path $share "latest.json"), ($latest | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
"Published build $($latest.commit) ($([math]::Round($setup.Length / 1MB, 1)) MB) to $share"
