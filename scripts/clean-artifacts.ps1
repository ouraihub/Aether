param([Parameter(Mandatory=$true)][string]$Root, [switch]$Deep, [switch]$CheckOnly)
# Remove build output. Called by clean.bat; keep this file ASCII-only.
$ErrorActionPreference = 'Stop'
$Root = [IO.Path]::GetFullPath($Root).TrimEnd('\')

# Anything holding files under this repo makes deletion half-succeed. Only look at
# processes that belong to this repo, so other projects' cargo/node keep running.
$rootLower = $Root.ToLower()
$busy = @(Get-CimInstance Win32_Process | Where-Object {
  ($_.ExecutablePath -and $_.ExecutablePath.ToLower().StartsWith($rootLower)) -or
  ($_.Name -in @('cargo.exe', 'rustc.exe', 'node.exe') -and $_.CommandLine -and $_.CommandLine.ToLower().Contains($rootLower))
})
if ($busy.Count -gt 0) {
  $busy | ForEach-Object { Write-Host ("  busy: {0} (PID {1})" -f $_.Name, $_.ProcessId) }
  exit 2
}
if ($CheckOnly) { exit 0 }

# A data\ folder next to an example exe holds that example's user data (data_root,
# docs/10 1.1); junctions may point outside the repo. Both are never followed or deleted.
function Test-Protected([string]$Path) {
  for ($p = [IO.Path]::GetFullPath($Path); $p -and $p.Length -ge $Root.Length; $p = [IO.Path]::GetDirectoryName($p)) {
    if ([IO.Path]::GetFileName($p) -ieq 'data') { return $true }
    if ((Test-Path -LiteralPath $p) -and (((Get-Item -LiteralPath $p -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0)) { return $true }
  }
  return $false
}
function Remove-Generated([string]$Path) {
  if (!(Test-Path -LiteralPath $Path)) { return }
  if (Test-Protected $Path) { Write-Host "  kept  $Path"; return }
  $item = Get-Item -LiteralPath $Path -Force
  if (!$item.PSIsContainer) { Remove-Item -LiteralPath $Path -Force; return }
  foreach ($child in Get-ChildItem -LiteralPath $Path -Force) { Remove-Generated $child.FullName }
  if (!(Get-ChildItem -LiteralPath $Path -Force | Select-Object -First 1)) { Remove-Item -LiteralPath $Path -Force }
}
function Get-SizeMB([string]$Path) {
  if (!(Test-Path -LiteralPath $Path)) { return 0 }
  $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
  return [math]::Round(($sum / 1MB), 1)
}

$targets = @(Join-Path $Root 'target')
if ($env:CARGO_TARGET_DIR) {
  $custom = $env:CARGO_TARGET_DIR
  if (![IO.Path]::IsPathRooted($custom)) { $custom = Join-Path $Root $custom }
  # Only clean a custom target dir that lives inside this repo; a shared one belongs to other projects too.
  $custom = [IO.Path]::GetFullPath($custom)
  if ($custom.ToLower().StartsWith($rootLower + '\')) { $targets += $custom } else { Write-Host "  skipped shared CARGO_TARGET_DIR $custom" }
}
$paths = @($targets | Select-Object -Unique) + @(
  (Join-Path $Root '.tmp'),
  (Join-Path $Root 'packages\ui\.svelte-kit'),
  (Join-Path $Root 'packages\ui\dist')
)
Get-ChildItem -LiteralPath (Join-Path $Root 'examples') -Directory -ErrorAction SilentlyContinue | ForEach-Object {
  $paths += (Join-Path $_.FullName 'src-tauri\gen')
}
if ($Deep) { $paths += (Join-Path $Root 'node_modules') }

$before = 0
foreach ($p in $paths) { $before += Get-SizeMB $p }
foreach ($p in $paths) { Remove-Generated $p }
$after = 0
foreach ($p in $paths) { $after += Get-SizeMB $p }
Write-Host ("  freed {0} MB ({1} MB kept)" -f [math]::Round($before - $after, 1), $after)
