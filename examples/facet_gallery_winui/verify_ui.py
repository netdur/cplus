"""Desktop regression checks for the shared Facet gallery on native WinUI."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import json
import subprocess
import time
from PIL import ImageGrab, ImageStat

root = Path(__file__).resolve().parent
out = root / 'out'
u = c.WinDLL('user32', use_last_error=True)
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
u.FindWindowW.restype = w.HWND
u.GetWindowRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
u.SetForegroundWindow.argtypes = [w.HWND]
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
u.WindowFromPoint.argtypes = [w.POINT]; u.WindowFromPoint.restype = w.HWND
u.GetAncestor.argtypes = [w.HWND, w.UINT]; u.GetAncestor.restype = w.HWND
hwnd = u.FindWindowW(None, 'Facet Gallery / WinUI')
assert hwnd, 'Gallery window missing'
u.SetForegroundWindow(hwnd)

def inspect(action='inspect', key='', value=''):
    result = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
        str(root / 'inspect_ui.ps1'), '-Action', action, '-Id', key, '-Value', str(value)],
        capture_output=True, text=True, encoding='utf-8')
    assert result.returncode == 0, result.stderr + result.stdout
    return json.loads(result.stdout)

def node(nodes, key):
    return next(n for n in nodes if n['id'] == key)

def click(bounds):
    x, y, width, height = bounds
    p = w.POINT(round(x + width/2), round(y + height/2))
    assert u.GetAncestor(u.WindowFromPoint(p), 2) == hwnd, 'Gallery is obscured'
    u.SetCursorPos(p.x, p.y); time.sleep(.1)
    u.mouse_event(2, 0, 0, 0, 0); u.mouse_event(4, 0, 0, 0, 0)
    time.sleep(.25)

def screenshot(name):
    rect = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(rect))
    image = ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom))
    image.save(out / name)
    assert max(ImageStat.Stat(image.crop((25, 50, image.width-25, image.height-25))).stddev) > 12, 'Blank client rendering'

class Key(c.Structure):
    _fields_ = [('vk', w.WORD), ('scan', w.WORD), ('flags', w.DWORD), ('time', w.DWORD), ('extra', c.c_size_t)]
class Union(c.Union):
    _fields_ = [('key', Key), ('padding', c.c_byte*32)]
class Input(c.Structure):
    _fields_ = [('type', w.DWORD), ('data', Union)]
u.SendInput.argtypes = [w.UINT, c.POINTER(Input), c.c_int]

def type_text(text):
    for char in text:
        for flags in (4, 6):
            event = Input(type=1, data=Union(key=Key(scan=ord(char), flags=flags)))
            assert u.SendInput(1, c.byref(event), c.sizeof(event)) == 1
        time.sleep(.035)

previous = w.POINT(); u.GetCursorPos(c.byref(previous))
try:
    # A freshly activated WinUI window can still be waiting for its first
    # pointer/foreground transition. Settle activation on the title bar.
    initial = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(initial))
    click([initial.left + 220, initial.top + 6, 120, 20])
    time.sleep(.4)
    nodes = inspect()
    divider = next(n for n in nodes if n['name'] == 'Resize panes')
    x, y, width, height = divider['bounds']
    u.SetCursorPos(round(x + width/2), round(y + height/2))
    time.sleep(.2)
    u.mouse_event(2, 0, 0, 0, 0)
    time.sleep(.2)
    for offset in range(10, 81, 10):
        u.mouse_event(0x8001, round((x + width/2 + offset)*65535/(u.GetSystemMetrics(0)-1)), round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
        time.sleep(.035)
    u.mouse_event(4, 0, 0, 0, 0)
    moved = next(n for n in inspect() if n['name'] == 'Resize panes')
    assert moved['bounds'][0] > x + 40, (divider, moved)
    print('PASS: real divider drag changes pane layout', flush=True)
    nodes = inspect('navigate', 'Button')
    click(node(nodes, 'btn:click')['bounds'])
    nodes = inspect()
    assert node(nodes, 'btn:count')['name'] == 'Clicked 1 time(s)', node(nodes, 'btn:count')
    assert not node(nodes, 'btn:off')['enabled']
    inspect('toggle', 'btn:toggle')
    screenshot('verified-buttons.png')
    print('PASS: real button click, callback, disabled and toggle controls', flush=True)

    nodes = inspect('navigate', 'Inputs')
    click(node(nodes, 'in:name')['bounds']); type_text('WinUI gallery')
    nodes = inspect()
    assert node(nodes, 'in:name')['value'] == 'WinUI gallery', node(nodes, 'in:name')
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, WinUI gallery'
    assert node(nodes, 'in:secret')['password']
    inspect('value', 'in:search', 'native search')
    assert node(inspect(), 'in:search_echo')['name'] == 'Query: native search'
    screenshot('verified-inputs.png')
    print('PASS: real keyboard input, search callback and password masking', flush=True)

    inspect('navigate', 'Values')
    nodes = inspect('range', 'val:slider', '.8')
    assert node(nodes, 'val:readout')['name'] == 'Value: 80%'
    assert node(nodes, 'val:spin')['bounds'][2] <= 48
    print('PASS: slider callback and bounded spinner size', flush=True)

    nodes = inspect('navigate', 'Icons')
    click(node(nodes, 'ico:gear')['bounds'])
    assert node(inspect(), 'ico:status')['name'] == 'icon_button clicks: 1'
    screenshot('verified-icons.png')

    inspect('navigate', 'Graphics')
    inspect('scroll', 'demo:scroll', 100)
    screenshot('verified-graphics.png')
    inspect('navigate', 'Image'); time.sleep(.5)
    screenshot('verified-images.png')
    print('PASS: compact icon buttons, images and canvas rendering', flush=True)

    inspect('navigate', 'relist')
    nodes = inspect('invoke', 'row:0:star')
    assert 'row 0 starred: true' in node(nodes, 'rel:echo')['name']
    inspect('invoke', 'rel:scroll'); time.sleep(.5)
    nodes = inspect('invoke', 'rel:far')
    assert 'FOUND' in node(nodes, 'rel:echo')['name']
    assert any(n['id'] == 'row:120:star' for n in nodes)
    assert sum(n['id'].startswith('row:') for n in nodes) < 40, 'Rows are not virtualized'
    inspect('invoke', 'rel:add'); inspect('invoke', 'rel:grid')
    screenshot('verified-relist.png')
    print('PASS: observable row action, scroll-to, find, append and grid mode', flush=True)

    inspect('navigate', 'Shadow')
    inspect('invoke', 'sh:glow'); screenshot('verified-shadow.png')
    inspect('invoke', 'sh:none'); inspect('invoke', 'sh:drop')
    inspect('navigate', 'Clip & order'); inspect('invoke', 'cl:ell')
    screenshot('verified-clip.png')
    nodes = inspect('navigate', 'Basics', 'last')
    before = node(nodes, 'ab:card_text')['bounds'][0]
    inspect('invoke', 'ab:move'); time.sleep(.7)
    assert node(inspect(), 'ab:card_text')['bounds'][0] > before + 150
    screenshot('verified-animation.png')
    nodes = inspect('navigate', 'Swipe')
    x, y, width, height = node(nodes, 'sw:title')['bounds']
    u.SetCursorPos(round(x + width/2), round(y + height/2)); time.sleep(.2)
    u.mouse_event(8, 0, 0, 0, 0); u.mouse_event(16, 0, 0, 0, 0); time.sleep(.5)
    inspect('invoke', 'sw:archive')
    assert node(inspect(), 'sw:status')['name'] == 'last action: Archive'
    print('PASS: shadow cleanup, clipping, animation and native context menu', flush=True)

    rect = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(rect))
    assert u.SetWindowPos(hwnd, None, rect.left, rect.top, 1100, 760, 0x14)
    time.sleep(.5)
    inspect('navigate', 'Overview'); screenshot('verified-overview.png')
    print('PASS: resize and navigation after recycled rows are released', flush=True)
    # Close with a live composition shadow as well as native event handlers.
    inspect('navigate', 'Shadow'); inspect('invoke', 'sh:glow')
    assert u.PostMessageW(hwnd, 0x10, 0, 0)
finally:
    u.SetCursorPos(previous.x, previous.y)
