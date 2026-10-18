<#
.SYNOPSIS
  Prompt Gallery 实机 UI 验证：启动 release 版应用，用合成鼠标执行一段操作序列，逐帧抓客户区截图。

.DESCRIPTION
  前置：cargo build --release（需要 target/release/prompt-gallery.exe 与 seed.exe）。
  合成输入经 SetCursorPos + mouse_event 发出，会真实移动光标，需要交互式桌面，不能进 CI。
  前置条件：目标窗口必须处于前台且**不被其它窗口遮挡**，否则第一次按下会被 Windows 用于激活窗口、事件到不了应用（实测表现为抓图帧间完全无差异）。
  截图用 PrintWindow(hwnd, hdc, 2)（PW_RENDERFULLCONTENT）抓窗口自身像素并裁出客户区，因此截图本身不受遮挡/前台影响；flag 1 对该 wgpu 窗口无效（整幅纯白）。
  坐标是客户区像素（系统 125% 缩放下 1pt = 1.25px）。
  流程：清空数据目录 -> seed 造数据 -> 起应用 -> 等主窗口（客户端面积 > 300000px）-> 提到前台 -> 执行场景 -> 抓图到 OutDir。

.PARAMETER Preset
  内置场景：drag / click / fixed / scrollup / map / lock。未显式传 -Seed / -SeedImage / -Data 时用预设值。

.PARAMETER Scene
  自定义步骤，元素格式：
    shot:<名字>    抓图到 <OutDir>/<名字>.png
    down:<x>:<y>   光标移到客户区 (x,y)，稍停后按下左键
    move:<x>:<y>   光标移到客户区 (x,y)
    up             松开左键
    sleep:<毫秒>   等待
    wheel:<格数>   滚轮（正数向上、负数向下，1 格 = 120）

.EXAMPLE
  pwsh -NoProfile -ExecutionPolicy Bypass -File tools/uicheck.ps1 -Preset drag

.EXAMPLE
  # 自定义场景要用直接调用（-File 传数组/逗号串会把元素错绑到别的参数上）：
  & 'tools/uicheck.ps1' -Seed 4 -Scene @('shot:idle','down:120:119','sleep:700','shot:armed','move:120:260','sleep:300','shot:moved','up','sleep:400','shot:released')
#>
param(
  [string]$Preset,
  [string[]]$Scene,
  [string]$Data,
  [int]$Seed = -1,
  [string]$SeedImage,
  [string]$OutDir,
  [switch]$KeepData
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$exe = Join-Path $root 'target/release/prompt-gallery.exe'
$seedExe = Join-Path $root 'target/release/seed.exe'

$presets = @{
  drag = @{
    Seed = 8; Image = 'docs/images/icon.png'; Data = 'target/uicheck-data'
    Steps = @('shot:drag-0-idle','down:120:119','sleep:150','shot:drag-1-press','sleep:700','shot:drag-2-armed','move:120:179','sleep:250','shot:drag-3-moved','move:120:309','sleep:250','shot:drag-4-moved-far','move:120:925','sleep:1600','shot:drag-5-autoscroll','up','sleep:500','shot:drag-6-released')
  }
  click = @{
    Seed = 2; Image = ''; Data = 'target/uicheck-data'
    Steps = @('shot:click-0-idle','down:120:187','sleep:90','up','sleep:400','shot:click-1-row2')
  }
  fixed = @{
    Seed = 16; Image = ''; Data = 'target/uicheck-data'
    Steps = @('shot:v-0-idle','down:120:187','sleep:90','up','sleep:400','shot:v-1-click-row2','down:120:119','sleep:60','move:120:600','sleep:180','shot:v-2-quick-mid','up','sleep:400','shot:v-3-quick-released','down:120:36','sleep:900','shot:v-4-fixed-hold','move:120:220','sleep:250','shot:v-5-fixed-moved','up','sleep:400','shot:v-6-fixed-click')
  }
  scrollup = @{
    Seed = 16; Image = ''; Data = 'target/uicheck-data'
    Steps = @('move:120:300','sleep:200','wheel:-14','sleep:400','shot:s-0-scrolled-down','down:120:300','sleep:420','shot:s-1-armed','move:120:88','sleep:700','shot:s-2-at-top-zone','sleep:700','shot:s-3-scrolled-up','up','sleep:500','shot:s-4-released')
  }
  map = @{
    Seed = 2; Image = 'docs/images/icon.png'; Data = 'target/uicheck-data'
    Steps = @('move:700:500','sleep:300','shot:m-0-idle','move:150:36','sleep:300','shot:m-1-fixed-hover','down:120:119','sleep:420','move:120:180','sleep:250','shot:m-2-ghost','up','sleep:400','shot:m-3-ghost-released')
  }
  # 第一行示例的锁图标（默认侧栏 248pt、125% 缩放 => 图标墨迹中心实测 x=270px、行中心 y=119px；
  # 命中区为行右端 20pt=25px 图标道，右沿约 x=282px。勿用 285：实测在命中区外，点击无效）。
  # 悬停显 🔓 -> 点击常显 🔒 且写库 -> 再点回 🔓（ADR-0007）。
  lock = @{
    Seed = 1; Image = ''; Data = 'target/uicheck-data'
    Steps = @('shot:lock-0-idle','move:270:119','sleep:350','shot:lock-1-hover','down:270:119','sleep:150','up','sleep:600','shot:lock-2-locked','down:270:119','sleep:150','up','sleep:600','shot:lock-3-unlocked')
  }
}

if ($Preset) {
  if (-not $presets.ContainsKey($Preset)) { throw ('unknown preset: ' + $Preset + ' (known: ' + (($presets.Keys | Sort-Object) -join ', ') + ')') }
  $p = $presets[$Preset]
  if (-not $Scene) { $Scene = $p.Steps }
  if ($Seed -lt 0) { $Seed = $p.Seed }
  if (-not $SeedImage -and $p.Image) { $SeedImage = $p.Image }
  if (-not $Data) { $Data = $p.Data }
}
if ($Seed -lt 0) { $Seed = 2 }
if (-not $Data) { $Data = 'target/uicheck-data' }
if (-not $OutDir) { $OutDir = 'target' }
if (-not $Scene) { throw 'nothing to do: pass -Preset or -Scene' }

function Resolve-UnderRoot($p) {
  if ([System.IO.Path]::IsPathRooted($p)) { return $p }
  return (Join-Path $root $p)
}
$data = Resolve-UnderRoot $Data
$outDir = Resolve-UnderRoot $OutDir
if (-not (Test-Path $exe)) { throw ('missing ' + $exe + ' -- run: cargo build --release') }
if (-not (Test-Path $seedExe)) { throw ('missing ' + $seedExe + ' -- run: cargo build --release') }
if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }

if (-not $KeepData) {
  if (Test-Path $data) { Remove-Item -Recurse -Force $data }
  $imgArg = @()
  if ($SeedImage) { $imgArg = @((Resolve-UnderRoot $SeedImage)) }
  for ($k = 0; $k -lt $Seed; $k++) { & $seedExe $data @imgArg | Out-Null }
  Write-Output ('seeded ' + $Seed + 'x -> ' + $data)
} else {
  Write-Output ('reusing ' + $data)
}

Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class Cap {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hWnd, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hWnd, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  public static int[] WindowRect(IntPtr h) { RECT r; GetWindowRect(h, out r); return new int[]{ r.Left, r.Top, r.Right-r.Left, r.Bottom-r.Top }; }
  public static IntPtr[] WindowsOf(int pid) {
    var list = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == (uint)pid && IsWindowVisible(h)) list.Add(h);
      return true;
    }, IntPtr.Zero);
    return list.ToArray();
  }
  public static int[] ClientSize(IntPtr h) { RECT r; GetClientRect(h, out r); return new int[]{ r.Right-r.Left, r.Bottom-r.Top }; }
  public static int[] ClientOrigin(IntPtr h) { POINT p = new POINT(); ClientToScreen(h, ref p); return new int[]{ p.X, p.Y }; }
  public static void TopMost(IntPtr h) { SetWindowPos(h, new IntPtr(-1), 0, 0, 0, 0, 0x0003); }
  public static void NoTopMost(IntPtr h) { SetWindowPos(h, new IntPtr(-2), 0, 0, 0, 0, 0x0003); }
  public static void ForceForeground(IntPtr h) { keybd_event(0x12, 0, 0, IntPtr.Zero); SetForegroundWindow(h); keybd_event(0x12, 0, 2, IntPtr.Zero); }
  public static bool IsForeground(IntPtr h) { return GetForegroundWindow() == h; }
  public static void Down(int x, int y) { SetCursorPos(x, y); System.Threading.Thread.Sleep(120); mouse_event(0x0002, 0, 0, 0, IntPtr.Zero); }
  public static void MoveTo(int x, int y) { SetCursorPos(x, y); }
  public static void Up() { mouse_event(0x0004, 0, 0, 0, IntPtr.Zero); }
  public static void Wheel(int notches) { mouse_event(0x0800, 0, 0, unchecked((uint)(notches * 120)), IntPtr.Zero); }
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int n);
  public static string Title(IntPtr h) { var sb = new System.Text.StringBuilder(256); GetWindowTextW(h, sb, 256); return sb.ToString(); }
}
'@
[void][Cap]::SetProcessDPIAware()

$env:PROMPT_GALLERY_DATA_DIR = $data
$proc = Start-Process -FilePath $exe -PassThru
$target = [IntPtr]::Zero
for ($i = 0; $i -lt 80; $i++) {
  Start-Sleep -Milliseconds 250
  $proc.Refresh()
  if ($proc.HasExited) { throw ('app exited early code=' + $proc.ExitCode) }
  $best = [IntPtr]::Zero; $bestArea = 0
  foreach ($w in [Cap]::WindowsOf($proc.Id)) { $cs = [Cap]::ClientSize($w); $a = $cs[0] * $cs[1]; if ($a -gt $bestArea) { $bestArea = $a; $best = $w } }
  if ($bestArea -gt 300000) { $target = $best; break }
}
if ($target -eq [IntPtr]::Zero) { if (-not $proc.HasExited) { $proc.Kill() }; throw 'no main window found (client area never exceeded 300000 px)' }

$co = [Cap]::ClientOrigin($target)
$cs = [Cap]::ClientSize($target)
[void][Cap]::ShowWindow($target, 5)
[Cap]::TopMost($target)
$fg = $false
for ($k = 0; $k -lt 10; $k++) {
  [Cap]::ForceForeground($target)
  Start-Sleep -Milliseconds 250
  if ([Cap]::IsForeground($target)) { $fg = $true; break }
}
if (-not $fg) {
  [Cap]::Down($co[0] + 5, $co[1] + 5)
  Start-Sleep -Milliseconds 150
  [Cap]::Up()
  Start-Sleep -Milliseconds 400
  [Cap]::ForceForeground($target)
  Start-Sleep -Milliseconds 250
  $fg = [Cap]::IsForeground($target)
}
if (-not $fg) { Write-Output 'WARN: window never became foreground; the first synthetic press may be consumed by activation' }
Start-Sleep -Milliseconds 1200
Write-Output ('origin=' + ($co -join ',') + ' client=' + ($cs -join 'x') + ' foreground=' + $fg)
$proc.Refresh()
Write-Output ('pid=' + $proc.Id + ' hwnd=' + $target + ' exited=' + $proc.HasExited)
foreach ($w in [Cap]::WindowsOf($proc.Id)) { $s = [Cap]::ClientSize($w); Write-Output ('  win hwnd=' + $w + ' client=' + ($s -join 'x') + ' origin=' + (([Cap]::ClientOrigin($w)) -join ',') + ' title=[' + [Cap]::Title($w) + ']') }

function ResolveTarget {
  $proc.Refresh()
  if ($proc.HasExited) { return [IntPtr]::Zero }
  $best = [IntPtr]::Zero; $bestArea = 0
  foreach ($w in [Cap]::WindowsOf($proc.Id)) { $cs = [Cap]::ClientSize($w); $a = $cs[0] * $cs[1]; if ($a -gt $bestArea) { $bestArea = $a; $best = $w } }
  return $best
}

function Shot($name) {
  $proc.Refresh()
  if ($proc.HasExited) { Write-Output ('PROCESS EXITED before shot ' + $name); return }
  if ([Cap]::ClientSize($target)[0] -eq 0) {
    $t = ResolveTarget
    if ($t -eq [IntPtr]::Zero) { Write-Output ('NO WINDOW before shot ' + $name); return }
    if ($t -ne $target) { Write-Output ('NOTE: main hwnd changed ' + $target + ' -> ' + $t); $script:target = $t; $script:co = [Cap]::ClientOrigin($t) }
  }
  $c = [Cap]::ClientSize($target)
  $o = [Cap]::ClientOrigin($target)
  $wr = [Cap]::WindowRect($target)
  $dx = $o[0] - $wr[0]; $dy = $o[1] - $wr[1]
  # PrintWindow(PW_RENDERFULLCONTENT=2) 渲染整个窗口（含标题栏），切成客户区大小后保存，
  # 好处：抓的是窗口自身像素，不受其它窗口遮挡影响。失败时退回屏幕拷贝。
  $bw = $c[0] + $dx; $bh = $c[1] + $dy
  $bmp = New-Object System.Drawing.Bitmap($bw, $bh)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  $ok = [Cap]::PrintWindow($target, $hdc, 2)
  $g.ReleaseHdc($hdc)
  if (-not $ok) {
    $g.CopyFromScreen($o[0], $o[1], $dx, $dy, (New-Object System.Drawing.Size($c[0], $c[1])))
  } else {
    # wgpu 窗口偶发 PrintWindow 成功但整幅纯色（实测全黑）：先采样探测，纯色则退回屏幕拷贝。
    $first = $bmp.GetPixel(2, 2).ToArgb()
    $xs = @(2, [int]($bw / 2), [int]($bw - 3))
    $ys = @(2, [int]($bh / 2), [int]($bh - 3))
    $uniform = $true
    foreach ($yy in $ys) {
      foreach ($xx in $xs) {
        if ($bmp.GetPixel($xx, $yy).ToArgb() -ne $first) { $uniform = $false; break }
      }
      if (-not $uniform) { break }
    }
    if ($uniform) {
      Write-Output ('  NOTE: PrintWindow uniform argb=' + $first + ' -> screen copy fallback')
      $g.CopyFromScreen($o[0], $o[1], $dx, $dy, (New-Object System.Drawing.Size($c[0], $c[1])))
    }
  }
  $g.Dispose()
  $frame = New-Object System.Drawing.Rectangle($dx, $dy, $c[0], $c[1])
  $client = $bmp.Clone($frame, $bmp.PixelFormat)
  $bmp.Dispose()
  $bmp = $client
  $bmp.Save((Join-Path $outDir ($name + '.png')), [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
  Write-Output ('shot ' + $name)
}

try {
  foreach ($step in $Scene) {
    $f = $step.Split(':')
    switch ($f[0]) {
      'shot'  { Shot $f[1] }
      'down'  { [Cap]::Down($co[0] + [int]$f[1], $co[1] + [int]$f[2]) }
      'move'  { [Cap]::MoveTo($co[0] + [int]$f[1], $co[1] + [int]$f[2]) }
      'up'    { [Cap]::Up() }
      'sleep' { Start-Sleep -Milliseconds ([int]$f[1]) }
      'wheel' { [Cap]::Wheel([int]$f[1]) }
      default { throw ('bad step: ' + $step) }
    }
    $proc.Refresh()
    if ($proc.HasExited) { Write-Output ('PROCESS EXITED after step: ' + $step); break }
  }
}
finally {
  [Cap]::NoTopMost($target)
  if (-not $proc.HasExited) { $proc.CloseMainWindow() | Out-Null }
  Start-Sleep -Milliseconds 1200
  if (-not $proc.HasExited) { $proc.Kill() }
  Write-Output ('done -> ' + $outDir)
}
