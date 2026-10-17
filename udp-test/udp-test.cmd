@echo off
rem YarnCat UDP test: runs udp-test.ps1 in this folder
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0udp-test.ps1"
if errorlevel 1 pause
