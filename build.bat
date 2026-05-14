@echo off
setlocal

echo Bundling TraceTuner in Release mode...
if exist "target\bundled\TraceTuner.vst3" rmdir /S /Q "target\bundled\TraceTuner.vst3"
if exist "target\bundled\TraceTuner.clap" del /F /Q "target\bundled\TraceTuner.clap"
if exist "target\bundled\TraceTuner.vst3" (
    echo Error: target\bundled\TraceTuner.vst3 could not be removed.
    exit /b 1
)
if exist "target\bundled\TraceTuner.clap" (
    echo Error: target\bundled\TraceTuner.clap could not be removed.
    exit /b 1
)
cargo xtask bundle trace_tuner --release --features gui
if errorlevel 1 exit /b %errorlevel%

if not exist "bin" mkdir "bin"
if not exist "tmp" mkdir "tmp"

echo Deploying bundled VST3 and CLAP...
if exist "bin\TraceTuner.vst3" rmdir /S /Q "bin\TraceTuner.vst3"
if exist "bin\TraceTuner.clap" del /F /Q "bin\TraceTuner.clap"
if exist "bin\TraceTuner.vst3" (
    echo Error: bin\TraceTuner.vst3 could not be removed. Close any host using it and retry.
    exit /b 1
)
if exist "bin\TraceTuner.clap" (
    echo Error: bin\TraceTuner.clap could not be removed. Close any host using it and retry.
    exit /b 1
)
xcopy "target\bundled\TraceTuner.vst3" "bin\TraceTuner.vst3\" /E /I /Y >nul
if errorlevel 1 exit /b %errorlevel%
copy "target\bundled\TraceTuner.clap" "bin\TraceTuner.clap" /Y
if errorlevel 1 exit /b %errorlevel%

echo Build complete! Plugins are located in the bin/ directory.

echo Creating timestamped release in tmp/...
for /f %%i in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd_HHmmss"') do set dt=%%i
set dir=tmp\release_%dt%
if not exist %dir% mkdir %dir%
if exist "%dir%\TraceTuner.vst3" rmdir /S /Q "%dir%\TraceTuner.vst3"
xcopy "target\bundled\TraceTuner.vst3" "%dir%\TraceTuner.vst3\" /E /I /Y >nul
if errorlevel 1 exit /b %errorlevel%
copy "target\bundled\TraceTuner.clap" "%dir%\TraceTuner.clap" /Y >nul
if errorlevel 1 exit /b %errorlevel%
echo Snapshot saved to %dir%
