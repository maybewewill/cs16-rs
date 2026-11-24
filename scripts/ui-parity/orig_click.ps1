param([int]$X, [int]$Y, [string]$Out, [int]$WaitMs = 1500)
# Clicks at client coords (X, Y) of the original hl.exe window, then captures it.
if (-not ("OC" -as [type])) {
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class OC { [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f,int dx,int dy,uint d,UIntPtr e);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
public struct RECT { public int L,T,R,B; } }
'@
}
$h = (Get-Process hl | Where-Object { $_.MainWindowTitle -eq 'Counter-Strike' } | Select-Object -First 1).MainWindowHandle
$r = New-Object OC+RECT; [OC]::GetWindowRect($h, [ref]$r) | Out-Null
[OC]::SetForegroundWindow($h) | Out-Null; Start-Sleep -Milliseconds 300
if ($X -ge 0) {
  [OC]::SetCursorPos($r.L + 8 + $X, $r.T + 31 + $Y) | Out-Null; Start-Sleep -Milliseconds 250
  [OC]::mouse_event(2,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 80; [OC]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
  Start-Sleep -Milliseconds $WaitMs
}
& (Join-Path $PSScriptRoot "capture_original.ps1") $Out
