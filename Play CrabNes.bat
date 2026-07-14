@echo off
title CrabNes
cd /d "%~dp0"
cargo run --release -p nes-ui -- %*
if errorlevel 1 pause
