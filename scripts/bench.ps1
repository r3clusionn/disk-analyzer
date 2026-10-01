# Wall-clock comparison of dux, dust and GNU du on one or more trees.
#   ./scripts/bench.ps1 -Roots C:\Projects,C:\Windows -Runs 5 -DuRoots C:\Projects
# dux and dust: one warm-up run, then -Runs timed runs, median reported. GNU du is far slower on
# Windows, so it gets one timed run (after the other tools have warmed the file cache) and only on
# the trees named in -DuRoots. Output goes to NUL, and each tool prints only the grand total, so
# what is timed is the scan, not the rendering.
param(
  [Parameter(Mandatory)] [string[]] $Roots,
  [int] $Runs = 5,
  [string[]] $DuRoots = @(),
  [string] $Dux = (Join-Path $PSScriptRoot '..\target\release\dux.exe'),
  [string] $Dust = 'dust',
  [string] $Du = 'C:\Program Files\Git\usr\bin\du.exe'
)

function Measure-Cmd([string] $cmdline, [int] $n, [bool] $warm) {
  if ($warm) { cmd /c "$cmdline > NUL 2>&1" | Out-Null }
  $t = 1..$n | ForEach-Object {
    $sw = [Diagnostics.Stopwatch]::StartNew(); cmd /c "$cmdline > NUL 2>&1" | Out-Null; $sw.Stop(); $sw.Elapsed.TotalMilliseconds
  } | Sort-Object
  [math]::Round($t[[int][math]::Floor($n / 2)])
}

Write-Host ('{0,-22} {1,10} {2,18} {3,10} {4,12}' -f 'tree', 'dux', 'dux no-hard-links', 'dust', 'GNU du')
foreach ($r in $Roots) {
  $q = '"' + $r + '"'
  $a = Measure-Cmd "`"$Dux`" -d 0 $q" $Runs $true
  $b = Measure-Cmd "`"$Dux`" -d 0 --no-hard-links $q" $Runs $true
  $c = Measure-Cmd "$Dust -d 0 $q" $Runs $true
  $d = if ($DuRoots -contains $r) { '{0} ms' -f (Measure-Cmd "`"$Du`" -s $q" 1 $false) } else { 'not run' }
  Write-Host ('{0,-22} {1,7} ms {2,15} ms {3,7} ms {4,12}' -f $r, $a, $b, $c, $d)
}
