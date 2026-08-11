@echo off
cd /d "%~dp0"
echo Log file: %LOCALAPPDATA%\RockCast\rockcast.log
set RUST_LOG=rockcast=debug
cargo run --release
