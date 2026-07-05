#Requires -Version 5.1

$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$TauriRoot = Join-Path $ProjectRoot "src-tauri"
$DebugExe = Join-Path $TauriRoot "target\debug\openclaw-launcher.exe"

$Passed = 0
$Failed = 0
$SpawnedProcess = $null

function Write-Section {
    param([string]$Name)
    Write-Host ""
    Write-Host "== $Name ==" -ForegroundColor Cyan
}

function Test-Step {
    param(
        [string]$Name,
        [scriptblock]$Body
    )

    Write-Host "[TEST] $Name ... " -NoNewline
    try {
        & $Body
        Write-Host "PASS" -ForegroundColor Green
        $script:Passed++
    } catch {
        Write-Host "FAIL" -ForegroundColor Red
        Write-Host "       $($_.Exception.Message)" -ForegroundColor Red
        $script:Failed++
    }
}

function Invoke-Checked {
    param(
        [string]$Name,
        [string]$Command,
        [string]$WorkingDirectory
    )

    Write-Host "[RUN ] $Name" -ForegroundColor Gray
    Push-Location $WorkingDirectory
    try {
        powershell -NoProfile -ExecutionPolicy Bypass -Command $Command
        if ($LASTEXITCODE -ne 0) {
            throw "$Name failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class WindowProbe {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

    [DllImport("user32.dll")]
    public static extern bool IsWindowVisible(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder lpString, int nMaxCount);

    [DllImport("user32.dll")]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder lpClassName, int nMaxCount);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);

    [DllImport("user32.dll")]
    public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    public class WindowInfo {
        public IntPtr Handle;
        public int Pid;
        public string Title;
        public string ClassName;
        public bool Visible;
    }

    public static List<WindowInfo> AllWindows() {
        var windows = new List<WindowInfo>();
        EnumWindows((hWnd, lParam) => {
            uint pid;
            GetWindowThreadProcessId(hWnd, out pid);

            var title = new StringBuilder(512);
            GetWindowText(hWnd, title, title.Capacity);

            var className = new StringBuilder(256);
            GetClassName(hWnd, className, className.Capacity);

            windows.Add(new WindowInfo {
                Handle = hWnd,
                Pid = (int)pid,
                Title = title.ToString(),
                ClassName = className.ToString(),
                Visible = IsWindowVisible(hWnd)
            });
            return true;
        }, IntPtr.Zero);
        return windows;
    }
}
"@ -ErrorAction SilentlyContinue

function Get-VisibleConsoleWindows {
    [WindowProbe]::AllWindows() |
        Where-Object { $_.Visible -and $_.ClassName -eq "ConsoleWindowClass" }
}

function Get-AppWindows {
    param([int]$ProcessId)
    [WindowProbe]::AllWindows() |
        Where-Object { $_.Pid -eq $ProcessId -and $_.Visible }
}

function Get-OwnedWindows {
    param([int]$ProcessId)
    [WindowProbe]::AllWindows() |
        Where-Object { $_.Pid -eq $ProcessId }
}

function Wait-ForAppWindow {
    param(
        [int]$ProcessId,
        [int]$TimeoutSeconds = 15
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $windows = @(Get-AppWindows -ProcessId $ProcessId)
        if ($windows.Count -gt 0) {
            return $windows
        }
        Start-Sleep -Milliseconds 250
    }
    return @()
}

function Wait-ForExit {
    param(
        [System.Diagnostics.Process]$Process,
        [int]$TimeoutSeconds = 8
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $Process.Refresh()
        if ($Process.HasExited) {
            return $true
        }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

Write-Section "Build"
Test-Step "frontend build succeeds" {
    Invoke-Checked -Name "npm run build" -Command "npm run build" -WorkingDirectory $ProjectRoot
}

Test-Step "tauri rust build succeeds" {
    Invoke-Checked -Name "cargo build" -Command "cargo build" -WorkingDirectory $TauriRoot
}

Test-Step "debug executable exists" {
    if (-not (Test-Path $DebugExe)) {
        throw "Missing executable: $DebugExe"
    }
}

Test-Step "backend uses CREATE_NO_WINDOW" {
    $lib = Get-Content (Join-Path $TauriRoot "src\lib.rs") -Raw
    if ($lib -notmatch "CREATE_NO_WINDOW") {
        throw "CREATE_NO_WINDOW guard not found in backend"
    }
}

Write-Section "Runtime"
Get-Process openclaw-launcher -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

$baselineConsoles = @(Get-VisibleConsoleWindows | ForEach-Object { $_.Handle.ToInt64() })
$unexpectedConsoles = New-Object System.Collections.Generic.List[string]

Test-Step "app starts" {
    $script:SpawnedProcess = Start-Process -FilePath $DebugExe -WorkingDirectory $ProjectRoot -PassThru
    Start-Sleep -Milliseconds 500
    $script:SpawnedProcess.Refresh()
    if ($script:SpawnedProcess.HasExited) {
        throw "Process exited immediately with code $($script:SpawnedProcess.ExitCode)"
    }
}

Test-Step "main window becomes visible" {
    $windows = @(Wait-ForAppWindow -ProcessId $script:SpawnedProcess.Id)
    if ($windows.Count -eq 0) {
        throw "No visible app window for PID $($script:SpawnedProcess.Id)"
    }
}

Test-Step "no extra black console window appears" {
    $deadline = (Get-Date).AddSeconds(12)
    while ((Get-Date) -lt $deadline) {
        $current = @(Get-VisibleConsoleWindows)
        foreach ($window in $current) {
            $handle = $window.Handle.ToInt64()
            if ($baselineConsoles -notcontains $handle) {
                $unexpectedConsoles.Add("pid=$($window.Pid) title='$($window.Title)' handle=$handle")
            }
        }
        Start-Sleep -Milliseconds 100
    }

    if ($unexpectedConsoles.Count -gt 0) {
        throw "Unexpected console windows: $($unexpectedConsoles -join '; ')"
    }
}

Test-Step "window close exits process" {
    $script:SpawnedProcess.Refresh()
    if ($script:SpawnedProcess.HasExited) {
        return
    }

    $closed = $script:SpawnedProcess.CloseMainWindow()
    if (-not $closed) {
        $windows = @(Get-OwnedWindows -ProcessId $script:SpawnedProcess.Id)
        if ($windows.Count -eq 0) {
            throw "No app window to close"
        }

        foreach ($window in $windows) {
            [WindowProbe]::SendMessage($window.Handle, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
        }
    }

    if (-not (Wait-ForExit -Process $script:SpawnedProcess)) {
        $script:SpawnedProcess.Kill()
        throw "Process did not exit after WM_CLOSE"
    }
}

if ($SpawnedProcess -and -not $SpawnedProcess.HasExited) {
    $SpawnedProcess.Kill()
}

Write-Section "Result"
Write-Host "$Passed passed, $Failed failed"

if ($Failed -gt 0) {
    exit 1
}
exit 0
