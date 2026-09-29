"""Real pointer input, native UIA label updates, resize, and reentrant removal."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import json
import subprocess
import sys
import time
from PIL import ImageGrab

root = Path(__file__).resolve().parent
out = root / 'out'
u = c.WinDLL('user32', use_last_error=True)
u.SetProcessDpiAwarenessContext.argtypes = [c.c_void_p]
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
u.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
u.IsWindowVisible.argtypes = [w.HWND]
u.SetForegroundWindow.argtypes = [w.HWND]
u.WindowFromPoint.argtypes = [w.POINT]; u.WindowFromPoint.restype = w.HWND
u.GetAncestor.argtypes = [w.HWND,w.UINT]; u.GetAncestor.restype = w.HWND
u.PostMessageW.argtypes = [w.HWND,w.UINT,w.WPARAM,w.LPARAM]
u.SetWindowPos.argtypes = [w.HWND,w.HWND,c.c_int,c.c_int,c.c_int,c.c_int,w.UINT]
found = []
@c.WINFUNCTYPE(w.BOOL,w.HWND,w.LPARAM)
def each(hwnd, unused):
    title = c.create_unicode_buffer(512)
    u.GetWindowTextW(hwnd,title,len(title))
    if title.value == 'Facet / generated WinUI' and u.IsWindowVisible(hwnd): found.append(hwnd)
    return True
u.EnumWindows(each,0)
assert len(found) == 1, f'Expected one sample window; found {len(found)}'
hwnd = found[0]
u.SetForegroundWindow(hwnd)
def inspect(count):
    result = subprocess.run(['powershell','-NoProfile','-ExecutionPolicy','Bypass','-File',str(root/'inspect_ui.ps1'),
        '-WindowHandle',str(hwnd),'-Count',str(count)],capture_output=True,text=True)
    if result.returncode:
        raise AssertionError(result.stderr + result.stdout)
    return json.loads(result.stdout)
def screenshot(name):
    rect = w.RECT(); assert u.GetWindowRect(hwnd,c.byref(rect))
    ImageGrab.grab(bbox=(rect.left,rect.top,rect.right,rect.bottom)).save(out/name)
state = inspect(0)
time.sleep(0.5)
screenshot('before-click.png')
for count in range(1,4):
    x,y,width,height = state['button']
    point = w.POINT(round(x+width/2),round(y+height/2))
    assert u.GetAncestor(u.WindowFromPoint(point),2) == hwnd, 'Sample is obscured; refusing to click another window'
    previous = w.POINT(); u.GetCursorPos(c.byref(previous))
    u.SetCursorPos(point.x,point.y); time.sleep(0.15)
    u.mouse_event(2,0,0,0,0); u.mouse_event(4,0,0,0,0)
    u.SetCursorPos(previous.x,previous.y)
    state = inspect(count)
    assert f'CLICK {count}:' in (out/'run.log').read_text()
    if count == 1:
        old_width = state['headingWidth']
        assert u.SetWindowPos(hwnd,None,0,0,980,680,0x0002|0x0004|0x0010)
        deadline = time.monotonic()+8
        while time.monotonic()<deadline:
            state = inspect(count)
            if abs(state['headingWidth']-old_width)>100: break
            time.sleep(0.1)
        else: raise AssertionError('Native label did not follow window resize')
time.sleep(0.5)
screenshot('after-click.png')
print('PASS: 3 real clicks, native label updates, resize, and replacement button events',flush=True)
if '--keep-open' not in sys.argv:
    u.PostMessageW(hwnd,0x10,0,0)
    deadline = time.monotonic()+8
    while time.monotonic()<deadline:
        log = (out/'run.log').read_text()
        if 'CLEAN EXIT' in log:
            assert 'clicks=3; live views=0; subscriptions=0' in log
            print('PASS: no live native records or event subscriptions after close',flush=True)
            break
        time.sleep(0.1)
    else: raise AssertionError('Clean exit did not complete')
