$ErrorActionPreference = 'Stop'
$buildProcess = [System.Diagnostics.Process]::GetCurrentProcess()
$previousAffinity = $buildProcess.ProcessorAffinity
$previousBuildJobs = $env:CARGO_BUILD_JOBS
$env:CARGO_BUILD_JOBS = '2'
$buildProcess.ProcessorAffinity = [IntPtr]$(if ([Environment]::ProcessorCount -gt 1) { 3 } else { 1 })
Push-Location -LiteralPath $PSScriptRoot
try {
    Write-Host 'Bundling TraceTuner in Release mode...'
    cargo xtask bundle trace_tuner --release --features gui
    if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed; existing bin artifacts were preserved.' }
    $artifacts = @('TraceTuner.vst3', 'TraceTuner.clap')
    foreach ($artifact in $artifacts) {
        if (-not (Test-Path -LiteralPath (Join-Path 'target/bundled' $artifact))) {
            throw "Missing bundle: $artifact; existing bin artifacts were preserved."
        }
    }
    $stamp = (Get-Date -Format 'yyyyMMdd_HHmmss_fff') + '_' + [guid]::NewGuid().ToString('N').Substring(0, 8)
    $snapshot = Join-Path 'tmp' "release_$stamp"
    New-Item -ItemType Directory -Path $snapshot -Force | Out-Null
    foreach ($artifact in $artifacts) {
        Copy-Item -LiteralPath (Join-Path 'target/bundled' $artifact) -Destination $snapshot -Recurse -Force
    }
    if (Test-Path -LiteralPath 'bin') {
        Copy-Item -LiteralPath 'bin' -Destination (Join-Path 'tmp' "before_release_$stamp") -Recurse
    }
    New-Item -ItemType Directory -Path 'bin' -Force | Out-Null
    foreach ($artifact in $artifacts) {
        $source = Join-Path $snapshot $artifact
        $destination = Join-Path 'bin' $artifact
        if (Test-Path -LiteralPath $source -PathType Container) {
            New-Item -ItemType Directory -Path $destination -Force | Out-Null
            Get-ChildItem -LiteralPath $source -Force | Copy-Item -Destination $destination -Recurse -Force
        } else {
            Copy-Item -LiteralPath $source -Destination $destination -Force
        }
    }
    Write-Host "Build complete: bin/ (snapshot: $snapshot)"
} finally {
    Pop-Location
    $buildProcess.ProcessorAffinity = $previousAffinity
    $env:CARGO_BUILD_JOBS = $previousBuildJobs
}
