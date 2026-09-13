@echo off
rem vtmate in the background, started at login - a per-user Scheduled Task.
rem
rem Not a Windows Service: services run outside any user's session (no
rem desktop, no audio), so they could not do the global push-to-talk
rem shortcut, the sounds, or anything this is actually for. A Scheduled Task
rem triggered "at log on" runs inside your normal desktop session, the same
rem as double-clicking vtmate yourself.
rem
rem   install:   install-vtmate-task.bat  (optionally: the full vtmate.exe path)
rem   uninstall: uninstall-vtmate-task.bat
rem   watch:     schtasks /query /tn vtmate /v /fo list
rem
rem Resolves vtmate.exe from PATH first (where.exe) - installer.sh's default
rem (no scope/prefix flags given, i.e. the plain installer command from the
rem README) installs to %LOCALAPPDATA%\Programs\vtmate\bin and adds it to
rem your User PATH, so this is normally already correct after the next
rem logon. Falls back to that same default location if PATH lookup fails.
rem A custom install prefix matches neither: pass the full path to this
rem script as its first argument if that's what you used.
rem
rem Plain --daemon, not --daemon-foreground: nothing here supervises or
rem restarts the process (the basic scheduler command below has no
rem restart-on-failure option), so vtmate is left to do what it always does
rem on --daemon - fork itself detached and let this task's own short-lived
rem launch step exit once that is confirmed, the same as running it by hand.
rem
rem See ..\..\..\README.md

setlocal

set "VTMATE_EXE=%~1"
if not defined VTMATE_EXE (
  for /f "delims=" %%I in ('where vtmate.exe 2^>nul') do if not defined VTMATE_EXE set "VTMATE_EXE=%%I"
)
if not defined VTMATE_EXE set "VTMATE_EXE=%LOCALAPPDATA%\Programs\vtmate\bin\vtmate.exe"

if not exist "%VTMATE_EXE%" (
  echo Could not find vtmate.exe at "%VTMATE_EXE%".
  echo Pass the full path as an argument to this script, e.g.:
  echo   install-vtmate-task.bat "C:\path\to\vtmate.exe"
  exit /b 1
)

schtasks /create /tn "vtmate" /tr "\"%VTMATE_EXE%\" --daemon" /sc onlogon /rl limited /f
if errorlevel 1 (
  echo Failed to create the scheduled task.
  exit /b 1
)

echo Installed: vtmate will start next time you log on.
echo To start it right now instead: schtasks /run /tn vtmate
