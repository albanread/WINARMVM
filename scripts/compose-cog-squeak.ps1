# compose-squeak-full.ps1 -- build the FULL 7-row Squeak race file.
# ASCII ONLY in this script: Windows PowerShell 5.1 reads BOM-less UTF-8 as
# ANSI, so any smart punctuation becomes mojibake INSIDE string literals and
# cascades into parser errors. And the OUTPUT must be BOM-free: Squeak's
# chunk reader chokes on a BOM at byte zero, which is exactly what
# Set-Content -Encoding utf8 emits on PS 5.1 -- hence WriteAllText with an
# explicit UTF8Encoding(false) below.
#
# Layout of mst2st.py --assemble output (measured, not assumed): the embedded
# Pharo harness has its driver STRIPPED, so the only reliable seam is the
# first translated class-def -- RBObject, richards' root, always first.
param(
    [string]$Assembled = "cog-all-pharo.st",
    [string]$Harness   = "bench-squeak.st",
    [string]$Out       = "img\bench-squeak-full.st"
)
$ErrorActionPreference = 'Stop'
$all = Get-Content $Assembled -Raw

$i = $all.IndexOf("subclass: #RBObject")
if ($i -lt 0) { throw "no RBObject class-def found - translator output changed shape" }
$i = $all.LastIndexOf("`n", $i) + 1
$rest = $all.Substring($i)

# Strip the macro tail's own driver doIts; we supply a trapped one.
$rest = $rest -replace "(?s)CogBench runEverything\.!\s*Smalltalk exitSuccess\.!\s*$", ""

# Pharo class-defs -> Squeak: package: becomes poolDictionaries: + category:.
$rest = $rest -replace "`tpackage: 'CogBench'!", "`tpoolDictionaries: ''`n`tcategory: 'CogBench'!"

# Our proven Squeak harness, minus its own runAll/quit driver tail.
$h = Get-Content $Harness -Raw
$h = $h -replace "(?s)CogBench runAll\.!\s*Smalltalk snapshot: false andQuit: true\.!\s*$", ""

$driver = @'

[ CogBench runEverything ]
	on: Error
	do: [:e | CogBench log: 'ERROR ', e class name, ': ', e messageText printString ].!
CogBench flushLog.!
Smalltalk snapshot: false andQuit: true.!
'@
$text = $h + $rest + $driver
[System.IO.File]::WriteAllText((Join-Path (Get-Location) $Out), $text, (New-Object System.Text.UTF8Encoding($false)))
"composed -> $Out ($([math]::Round((Get-Item $Out).Length/1KB,1)) KB)"
