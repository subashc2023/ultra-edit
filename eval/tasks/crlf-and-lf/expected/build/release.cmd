@echo off
setlocal
set APP_VERSION=2.4.0
set OUT_DIR=dist\win
set SIGN_BUILD=1
echo Building %APP_VERSION% into %OUT_DIR%
call :compile "%OUT_DIR%"
exit /b %ERRORLEVEL%

:compile
if not exist %1 mkdir %1
echo compiled > %1\build.txt
exit /b 0
