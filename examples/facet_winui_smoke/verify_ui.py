"""Real pointer/keyboard/wheel input, UIA updates, resize and callback removal."""
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
def click(bounds):
    x,y,width,height = bounds
    point = w.POINT(round(x+width/2),round(y+height/2))
    assert u.GetAncestor(u.WindowFromPoint(point),2) == hwnd, 'Sample is obscured'
    previous = w.POINT(); u.GetCursorPos(c.byref(previous))
    u.SetCursorPos(point.x,point.y); time.sleep(0.15)
    u.mouse_event(2,0,0,0,0); u.mouse_event(4,0,0,0,0)
    u.SetCursorPos(previous.x,previous.y)
class KeyInput(c.Structure):
    _fields_ = [('vk',w.WORD),('scan',w.WORD),('flags',w.DWORD),('time',w.DWORD),('extra',c.c_size_t)]
class InputUnion(c.Union):
    _fields_ = [('key',KeyInput),('padding',c.c_byte*32)]
class Input(c.Structure):
    _fields_ = [('type',w.DWORD),('data',InputUnion)]
u.SendInput.argtypes = [w.UINT,c.POINTER(Input),c.c_int]
def type_text(value):
    for char in value:
        for flags in (4,6):
            event = Input(type=1,data=InputUnion(key=KeyInput(scan=ord(char),flags=flags)))
            assert u.SendInput(1,c.byref(event),c.sizeof(event)) == 1
        time.sleep(0.1)
state = inspect(0)
time.sleep(0.5)
state = inspect(0)
assert state['scrollPercent'] > 0, 'Initial scroll offset was lost before loading'
initial_scroll = state['scrollPercent']
screenshot('before-click.png')
click(state['edit'])
time.sleep(0.4)
type_text('WinUI!') # MaxLength=5 rejects the sixth character.
time.sleep(0.4)
state = inspect(0)
assert state['text'] == 'WinUI' and state['echo'], state
edit_count = (out/'run.log').read_text().count('EDIT:')
assert edit_count > 0
# An unrelated label update after every keystroke must not reset the caret.
x,y,width,height = state['scroll']
point = w.POINT(round(x+width/2),round(y+height/2))
assert u.GetAncestor(u.WindowFromPoint(point),2) == hwnd
previous = w.POINT(); u.GetCursorPos(c.byref(previous))
u.SetCursorPos(point.x,point.y)
u.mouse_event(0x800,0,0,c.c_uint32(-360).value,0)
time.sleep(1)
u.SetCursorPos(previous.x,previous.y)
state = inspect(0)
assert state['scrollPercent'] > initial_scroll, state
wheel_scroll = state['scrollPercent']
assert 'SCROLL:' in (out/'run.log').read_text()
for count in range(1,4):
    x,y,width,height = state['button']
    point = w.POINT(round(x+width/2),round(y+height/2))
    assert u.GetAncestor(u.WindowFromPoint(point),2) == hwnd, 'Sample is obscured; refusing to click another window'
    previous = w.POINT(); u.GetCursorPos(c.byref(previous))
    u.SetCursorPos(point.x,point.y); time.sleep(0.15)
    u.mouse_event(2,0,0,0,0); u.mouse_event(4,0,0,0,0)
    u.SetCursorPos(previous.x,previous.y)
    state = inspect(count)
    assert state['scrollPercent'] > 0, 'Unrelated sync reset scroll position'
    expected = 'Programmatic update' if count == 3 else 'WinUI'
    assert state['text'] == expected, state
    assert (out/'run.log').read_text().count('EDIT:') == edit_count, 'Programmatic update fired user edit callback'
    if count == 1:
        click(state['edit']); time.sleep(0.3)
        u.keybd_event(0x11,0,0,0); u.keybd_event(0x41,0,0,0)
        u.keybd_event(0x41,0,2,0); u.keybd_event(0x11,0,2,0)
        type_text('X'); time.sleep(0.3)
        state = inspect(count)
        assert state['text'] == 'WinUI', 'Read-only TextBox accepted typing'
    if count == 3:
        assert state['scrollPercent'] > wheel_scroll, 'Programmatic scroll offset did not apply'
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
print('PASS: typing and Facet text readback, programmatic text update, wheel scrolling, 3 clicks, resize and callback replacement',flush=True)
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
