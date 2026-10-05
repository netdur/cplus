"""Exercise the probe through real pointer input, then close its window."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import sys
import time
from PIL import ImageGrab

root=Path(__file__).resolve().parent/'out'
u=c.WinDLL('user32',use_last_error=True)
u.SetProcessDpiAwarenessContext.argtypes=[c.c_void_p]
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
u.GetWindowTextW.argtypes=[w.HWND,w.LPWSTR,c.c_int]
u.GetWindowRect.argtypes=[w.HWND,c.POINTER(w.RECT)]
u.GetClientRect.argtypes=[w.HWND,c.POINTER(w.RECT)]
u.ClientToScreen.argtypes=[w.HWND,c.POINTER(w.POINT)]
u.GetDpiForWindow.argtypes=[w.HWND]
u.PostMessageW.argtypes=[w.HWND,w.UINT,w.WPARAM,w.LPARAM]
u.SetForegroundWindow.argtypes=[w.HWND]
u.IsWindowVisible.argtypes=[w.HWND]
u.WindowFromPoint.argtypes=[w.POINT]; u.WindowFromPoint.restype=w.HWND
u.GetAncestor.argtypes=[w.HWND,w.UINT]; u.GetAncestor.restype=w.HWND
found=[]
@c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM)
def each(hwnd,arg):
    title=c.create_unicode_buffer(512); u.GetWindowTextW(hwnd,title,len(title))
    if title.value=='Generated WinUI / standalone C+' and u.IsWindowVisible(hwnd): found.append(hwnd)
    return True
u.EnumWindows(each,0)
assert len(found)==1,f'Expected one visible probe window; found {len(found)}'
hwnd=found[0]; rect=w.RECT(); u.GetWindowRect(hwnd,c.byref(rect))
print('Window bounds',rect.left,rect.top,rect.right,rect.bottom,flush=True)
u.SetForegroundWindow(hwnd); time.sleep(0.5)
ImageGrab.grab(bbox=(rect.left,rect.top,rect.right,rect.bottom)).save(root/'before-click.png')
point=w.POINT(); dpi=u.GetDpiForWindow(hwnd)
point.x=round(170*dpi/96); point.y=round(64*dpi/96)
assert u.ClientToScreen(hwnd,c.byref(point))
assert u.GetAncestor(u.WindowFromPoint(point),2)==hwnd, 'Probe is obscured; refusing to click another window'
previous=w.POINT(); u.GetCursorPos(c.byref(previous))
before=(root/'run.log').read_text().count('CLICK: generated delegate reached C+')
u.SetCursorPos(point.x,point.y); time.sleep(0.15)
u.mouse_event(2,0,0,0,0); u.mouse_event(4,0,0,0,0)
u.SetCursorPos(previous.x,previous.y)
deadline=time.monotonic()+8
while time.monotonic()<deadline:
    if (root/'run.log').read_text().count('CLICK: generated delegate reached C+') > before: break
    time.sleep(0.1)
else: raise AssertionError('No C+ callback received after pointer input')
time.sleep(0.5)
ImageGrab.grab(bbox=(rect.left,rect.top,rect.right,rect.bottom)).save(root/'after-click.png')
print('PASS: actual pointer input delivered WinUI Click to C+',flush=True)
if '--keep-open' not in sys.argv:
    u.PostMessageW(hwnd,0x10,0,0)
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        log=(root/'run.log').read_text()
        if 'CLEAN EXIT' in log:
            assert 'clicks=1' in log
            print('PASS: Start returned and event cleanup completed',flush=True); break
        time.sleep(0.1)
    else: raise AssertionError('Clean exit did not complete')

