param([string]$Out = "$env:TEMP\cs_window.png")
Add-Type -AssemblyName System.Drawing
if (-not ("PWCap" -as [type])) {
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class PWCap {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct RECT { public int L, T, R, B; }
}
'@
}
$h = [IntPtr](Get-Process hl | Where-Object { $_.MainWindowTitle -eq 'Counter-Strike' } | Select-Object -First 1).MainWindowHandle
$r = New-Object PWCap+RECT; [PWCap]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $hh = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc(); [PWCap]::PrintWindow($h, $hdc, 2) | Out-Null; $g.ReleaseHdc($hdc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
"$Out ${w}x${hh}"

