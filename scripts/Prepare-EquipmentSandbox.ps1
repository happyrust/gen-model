$ErrorActionPreference='Stop'
$repo=Split-Path -Parent $PSScriptRoot
$run=Join-Path $repo 'output/rocksdb-8000-20260905'
$projects=Join-Path $run 'projects'
New-Item -ItemType Directory $projects -Force | Out-Null
$destination=Join-Path $projects 'AvevaMarineSample'
if (!(Test-Path $destination)) {
    Copy-Item -LiteralPath 'D:/AVEVA/Projects/E3D3.1/AvevaMarineSample' -Destination $projects -Recurse
}
foreach ($name in @('AvevaCatalogue','ZDJ')) {
    $link=Join-Path $projects $name
    if (!(Test-Path $link)) { New-Item -ItemType Junction -Path $link -Target ('D:/AVEVA/Projects/E3D3.1/'+$name) | Out-Null }
}
$path=Join-Path $run 'DbOption.toml'
$text=[IO.File]::ReadAllText($path)
$text=[regex]::Replace($text,'(?m)^project_path\s*=.*$',('project_path = "'+($projects -replace '\\','/')+'"'))
[IO.File]::WriteAllText($path,$text)
Write-Output $destination
