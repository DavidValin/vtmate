@echo off
rem Removes the Scheduled Task install-vtmate-task.bat creates.
rem See install-vtmate-task.bat for what it does and why.

schtasks /delete /tn "vtmate" /f >nul 2>&1
echo Removed.
