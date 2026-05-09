#!/usr/bin/env pwsh

[CmdletBinding()]
param (
   [string[]]$Dirs
)

if ($Dirs.Count -eq 0) {
   $Dirs = Get-ChildItem -Directory modules
}

$jobs = @()

foreach ($Dir in $Dirs) {
   Write-Host "Building $Dir"
   $jobs += Start-Process -FilePath "deno" -ArgumentList "run -A jsr:@veryboringhwl/creator@0.0.2 build -i $Dir -o $Dir -c classmap.json" -NoNewWindow -PassThru
}

$jobs | Wait-Process

Write-Host "Done"
