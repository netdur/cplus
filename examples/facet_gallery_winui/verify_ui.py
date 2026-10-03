"""Desktop regression checks for the shared Facet gallery on native WinUI."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import json
import subprocess
import sys
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
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
u.GetDpiForWindow.argtypes = [w.HWND]
hwnd = u.FindWindowW(None, 'Facet Gallery / WinUI')
assert hwnd, 'Gallery window missing'
u.SetForegroundWindow(hwnd)

def inspect(action='inspect', key='', value=''):
    result = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
        str(root / 'inspect_ui.ps1'), '-Action', action, '-Id', key, '-Value', str(value)],
        capture_output=True, text=True, encoding='utf-8')
    # Navigation can retire peers while FindAll is traversing the old page.
    # Retry only the read, never the already-issued action.
    for _ in range(2):
        if result.returncode == 0 or 'ElementNotAvailableException' not in result.stderr or 'FindAll' not in result.stderr:
            break
        time.sleep(.25)
        result = subprocess.run(['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
            str(root / 'inspect_ui.ps1')], capture_output=True, text=True, encoding='utf-8')
    assert result.returncode == 0, result.stderr + result.stdout
    return json.loads(result.stdout)

def node(nodes, key):
    return next(n for n in nodes if n['id'] == key)

def click(bounds):
    x, y, width, height = bounds
    p = w.POINT(round(x + width/2), round(y + height/2))
    target = u.GetAncestor(u.WindowFromPoint(p), 2)
    owner_pid, target_pid = w.DWORD(), w.DWORD()
    u.GetWindowThreadProcessId(hwnd, c.byref(owner_pid))
    u.GetWindowThreadProcessId(target, c.byref(target_pid))
    assert target_pid.value == owner_pid.value, 'Gallery or its native popup is obscured'
    u.mouse_event(0x8001, round(p.x*65535/(u.GetSystemMetrics(0)-1)),
                  round(p.y*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(.1)
    u.mouse_event(2, 0, 0, 0, 0); time.sleep(.05)
    u.mouse_event(4, 0, 0, 0, 0)
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

def verify_tree_rows():
    nodes = inspect('navigate', 'Tree')
    scale = u.GetDpiForWindow(hwnd)/96
    def row(name):
        return next(n for n in inspect() if n['name'] == name and n['type'] == 'ControlType.TreeItem')
    # The automation peer's hit rectangle excludes the row's theme margins.
    # Adjacent row positions measure the complete native row pitch.
    def pitch():
        return row('Cat')['bounds'][1] - row('Animals')['bounds'][1]
    initial = pitch()
    assert node(inspect(), 'tr:item:animals')['type'] == 'ControlType.TreeItem'
    assert abs(initial - 26*scale) <= 1.1, (initial, scale)
    click(row('Cat')['bounds'])
    selected = node(inspect(), 'tr:status')['name']
    assert '(id=cat)' in selected, selected
    click(node(inspect(), 'tr:resize')['bounds'])
    assert abs(pitch() - 44*scale) <= 1.1
    assert node(inspect(), 'tr:status')['name'] == selected
    assert not row('Cat')['offscreen'], 'Expanded branch collapsed during row resizing'
    screenshot('verified-tree-rows.png')
    click(node(inspect(), 'tr:resize')['bounds'])
    assert node(inspect(), 'tr:rows')['name'] == 'Rows: native default'
    assert 0 < pitch() < 44*scale
    click(node(inspect(), 'tr:resize')['bounds'])
    assert abs(pitch() - 26*scale) <= 1.1
    assert node(inspect(), 'tr:status')['name'] == selected
    print('PASS: tree row heights, native reset and retained selection/expansion', flush=True)
    click(node(inspect(), 'tr:custom')['bounds'])
    assert node(inspect(), 'tr:item:animals')['type'] == 'ControlType.TreeItem'
    editor = node(inspect(), 'tr:edit:animals')
    assert editor['bounds'][2] > 100 and not editor['offscreen']
    click(editor['bounds'])
    type_text(' edited')
    draft = node(inspect(), 'tr:edit:animals')['value']
    assert 'edited' in draft
    click(node(inspect(), 'tr:rebind')['bounds'])
    nodes = inspect()
    assert node(nodes, 'tr:edit:animals')['value'] == draft
    assert node(nodes, 'tr:label:animals')['name'].endswith('1')
    click(node(nodes, 'tr:use:animals')['bounds'])
    assert node(inspect(), 'tr:status')['name'] == 'used: Animals  (id=animals)'
    screenshot('verified-tree-custom-rows.png')
    # WinUI exposes its inner tree list peer, not the outer TreeView ID.
    # The pointer is still over the row's Use button; wheel its native list.
    u.mouse_event(0x0800, 0, 0, (-480) & 0xffffffff, 0)
    time.sleep(.5)
    assert not node(inspect(), 'tr:item:blue')['offscreen']
    u.mouse_event(0x0800, 0, 0, 480, 0)
    time.sleep(.5)
    assert node(inspect(), 'tr:edit:animals')['value'] == draft
    click(node(inspect(), 'tr:custom')['bounds'])
    assert abs(pitch() - 26*scale) <= 1.1
    print('PASS: custom tree editing, label rebinding, nested action identity and template reset', flush=True)



def verify_menus():
    nodes = inspect('navigate', 'Menus')
    def shortcut(vk):
        for code, flags in ((17, 0), (vk, 0), (vk, 2), (17, 2)):
            event = Input(type=1, data=Union(key=Key(vk=code, flags=flags)))
            assert u.SendInput(1, c.byref(event), c.sizeof(event)) == 1
        time.sleep(.25)
    inspect('focus', 'mn:tool')
    shortcut(0x44)
    initial_status = node(inspect(), 'mn:status')['name']
    assert initial_status == 'Action mn:delete: 1 click(s)', initial_status
    click(node(nodes, 'mn:menu')['bounds'])
    nodes = inspect()
    click(node(nodes, 'mn:archive')['bounds'])
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:archive: 2 click(s)'
    click(node(inspect(), 'mn:tool')['bounds'])
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:tool: 3 click(s)'
    click(node(inspect(), 'mn:style')['bounds'])
    toolbar_node = node(inspect(), 'mn:tool')
    assert toolbar_node['name'] == 'Remove', toolbar_node
    click(node(inspect(), 'mn:menu')['bounds'])
    nodes = inspect()
    assert node(nodes, 'mn:archive')['name'] == 'Delete archive'
    screenshot('verified-menus.png')
    click(node(nodes, 'mn:delete')['bounds'])
    delete_status = node(inspect(), 'mn:status')['name']
    assert delete_status == 'Action mn:delete: 4 click(s)', delete_status
    # Border hosts do not expose an automation peer; target their visible label.
    x, y, width, height = next(n for n in inspect() if n['name'] == 'Right-click this area')['bounds']
    u.mouse_event(0x8001, round((x+width/2)*65535/(u.GetSystemMetrics(0)-1)),
                  round((y+height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(.15)
    u.mouse_event(8, 0, 0, 0, 0); time.sleep(.05)
    u.mouse_event(16, 0, 0, 0, 0); time.sleep(.3)
    click(node(inspect(), 'mn:inspect')['bounds'])
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:inspect: 5 click(s)'
    # A global menu accelerator also works after the flyout is dismissed.
    inspect('focus', 'mn:tool')
    shortcut(0x44)
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:delete: 6 click(s)'
    shortcut(0x49)
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:delete: 6 click(s)', 'Context shortcut escaped its scope'
    inspect('focus', 'mn:scope')
    shortcut(0x49)
    assert node(inspect(), 'mn:status')['name'] == 'Action mn:inspect: 7 click(s)'
    shortcut(0x52)
    nodes = inspect()
    assert not any(n['id'] == 'mn:menu' for n in nodes)
    assert node(nodes, 'mn:status')['name'] == 'Menu removed; its shortcuts are detached'
    shortcut(0x44)
    assert node(inspect(), 'mn:status')['name'] == 'Menu removed; its shortcuts are detached'
    print('PASS: real menu/toolbar/context clicks, live styling, global/scoped shortcuts and shortcut callback removal', flush=True)


def verify_carousel():
    nodes = inspect('navigate', 'Carousel')
    inspect('toggle', 'cr:keep')
    x, y, width, height = node(nodes, 'cr:host')['bounds']
    u.mouse_event(0x8001, round((x + width/2)*65535/(u.GetSystemMetrics(0)-1)),
                  round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(.4)
    click(node(inspect(), 'NextButtonHorizontal')['bounds'])
    assert node(inspect(), 'cr:status')['name'] == 'Page 2 of 4'
    inspect('focus', 'cr:host')
    for flags in (0, 2):
        event = Input(type=1, data=Union(key=Key(vk=0x27, flags=flags)))
        assert u.SendInput(1, c.byref(event), c.sizeof(event)) == 1
    time.sleep(.4)
    assert node(inspect(), 'cr:status')['name'] == 'Page 3 of 4'
    inspect('invoke', 'cr:go_last')
    assert node(inspect(), 'cr:status')['name'] == 'Page 4 of 4'
    screenshot('verified-carousel-last.png')
    inspect('invoke', 'cr:go_first'); time.sleep(.4)
    assert node(inspect(), 'cr:keep')['toggle'] == 'On', 'Carousel rebuilt its retained page'
    assert node(inspect(), 'cr:status')['name'] == 'Page 1 of 4'
    inspect('invoke', 'cr:previous'); time.sleep(.1)
    assert node(inspect(), 'cr:status')['name'] == 'Page 1 of 4'
    inspect('toggle', 'cr:loop')
    inspect('invoke', 'cr:previous'); time.sleep(.1)
    assert node(inspect(), 'cr:status')['name'] == 'Page 4 of 4'
    inspect('invoke', 'cr:next'); time.sleep(.1)
    assert node(inspect(), 'cr:status')['name'] == 'Page 1 of 4'
    inspect('toggle', 'cr:animate')
    inspect('invoke', 'cr:next'); time.sleep(.1)
    assert node(inspect(), 'cr:status')['name'] == 'Page 2 of 4'
    inspect('invoke', 'cr:previous'); time.sleep(.1)
    assert node(inspect(), 'cr:keep')['toggle'] == 'On'
    screenshot('verified-carousel.png')
    print('PASS: native carousel arrow, keyboard paging, programmatic jumps, circular Previous/Next, animation switch and retained page state', flush=True)


previous = w.POINT(); u.GetCursorPos(c.byref(previous))
try:
    # A freshly activated WinUI window can still be waiting for its first
    # pointer/foreground transition. Settle activation on the title bar.
    initial = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(initial))
    click([initial.left + 220, initial.top + 6, 120, 20])
    time.sleep(.4)
    if '--tree-rows-only' in sys.argv:
        verify_tree_rows()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--menus-only' in sys.argv:
        verify_menus()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--carousel-only' in sys.argv:
        verify_carousel()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
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

    inspect('navigate', 'Controls')
    inspect('invoke', 'ctl:style')
    assert node(inspect(), 'ctl:style_status')['name'] == 'Custom purple colors'
    screenshot('verified-choice-colors.png')
    inspect('invoke', 'ctl:style')
    assert node(inspect(), 'ctl:style_status')['name'] == 'Default theme colors'
    print('PASS: choice colors switch between custom and native theme', flush=True)

    nodes = inspect('navigate', 'Pickers')
    click(node(nodes, 'pk:popup')['bounds']); time.sleep(.3)
    screenshot('verified-picker-dropdown.png')
    # Pick Green using the real keyboard and verify Facet's selection callback.
    for key in (40, 13):
        u.keybd_event(key, 0, 0, 0); u.keybd_event(key, 0, 2, 0)
    assert node(inspect(), 'pk:popup_status')['name'] == 'Selected index: 1'
    print('PASS: picker dropdown and selection callback', flush=True)

    nodes = inspect('navigate', 'Web')
    x, y, width, height = node(nodes, 'web:view')['bounds']
    # Ignore the frame/scrollbar: the old missing runtime left a uniform blank
    # interior. Allow time for browser startup and the sample's network request.
    deadline = time.monotonic() + 20
    while True:
        time.sleep(.5)
        content = ImageGrab.grab(bbox=tuple(round(v) for v in (x+40, y+40, x+width-40, y+height-40)))
        if max(ImageStat.Stat(content).stddev) > 12: break
        assert time.monotonic() < deadline, 'WebView remained blank'
    screenshot('verified-web.png')
    print('PASS: embedded browser paints page content', flush=True)

    inspect('navigate', 'Values')
    nodes = inspect('range', 'val:slider', '.8')
    assert node(nodes, 'val:readout')['name'] == 'Value: 80%'
    assert node(nodes, 'val:spin')['bounds'][2] <= 48
    inspect('invoke', 'val:colors')
    nodes = inspect()
    assert node(nodes, 'val:style_status')['name'] == 'Custom slider colors'
    # Leave the native PointerOver state before checking the normal-state color.
    x, y, width, height = node(nodes, 'val:style_status')['bounds']
    u.mouse_event(0x8001, round((x + width/2)*65535/(u.GetSystemMetrics(0)-1)),
                  round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(.3)
    x, y, width, height = node(nodes, 'val:slider')['bounds']
    pixels = ImageGrab.grab(bbox=tuple(round(v) for v in (x, y, x+width, y+height))).convert('RGB')
    assert sum(120 < r < 160 and 25 < g < 70 and 170 < b < 215 for r, g, b in pixels.getdata()) > 100, 'Custom slider color is not rendered'
    screenshot('verified-slider-colors.png')
    inspect('invoke', 'val:colors')
    assert node(inspect(), 'val:style_status')['name'] == 'Default slider colors'
    print('PASS: slider callback, custom/theme colors and bounded spinner size', flush=True)

    nodes = inspect('navigate', 'Icons')
    click(node(nodes, 'ico:gear')['bounds'])
    assert node(inspect(), 'ico:status')['name'] == 'icon_button clicks: 1'
    screenshot('verified-icons.png')

    inspect('navigate', 'Graphics')
    nodes = inspect('scroll', 'demo:scroll', 100)
    for expected, picture in [('Rounded dashed outline', 'verified-border-rounded.png'),
                              ('Ellipse dotted outline', 'verified-border-ellipse.png'),
                              ('Solid rectangular outline', 'verified-graphics.png')]:
        click(node(nodes, 'g:border_cycle')['bounds'])
        nodes = inspect()
        assert node(nodes, 'g:border_note')['name'] == expected
        assert not any(n['type'] == 'ControlType.ToolTip' and n['name'] in ('0.80', '0.8', '1') for n in nodes), 'Slider tooltip survived navigation'
        screenshot(picture)
    print('PASS: real clicks inside live rounded/dashed, ellipse/dotted and solid borders', flush=True)
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
    # Navigation can return before the newly mounted card has been arranged.
    # Establish its untransformed layout before measuring displacement.
    nodes = inspect('invoke', 'ab:reset')
    before = node(nodes, 'ab:card_text')['bounds'][0]
    inspect('invoke', 'ab:move'); time.sleep(.7)
    after = node(inspect(), 'ab:card_text')['bounds'][0]
    assert after > before + 150, (before, after)
    screenshot('verified-animation.png')
    inspect('invoke', 'ab:reset')
    assert abs(node(inspect(), 'ab:card_text')['bounds'][0] - before) < 2
    nodes = inspect('navigate', 'Swipe')
    x, y, width, height = node(nodes, 'sw:title')['bounds']
    u.SetCursorPos(round(x + width/2), round(y + height/2)); time.sleep(.2)
    u.mouse_event(8, 0, 0, 0, 0); u.mouse_event(16, 0, 0, 0, 0); time.sleep(.5)
    inspect('invoke', 'sw:archive')
    assert node(inspect(), 'sw:status')['name'] == 'last action: Archive'
    print('PASS: shadow cleanup, clipping, animation and native context menu', flush=True)

    nodes = inspect('navigate', 'Accessibility')
    close = node(nodes, 'ax:close')
    assert close['name'] == 'Close'
    assert close['bounds'][3] >= 32, 'Auto-sized button retained its pre-template height'
    x, y, width, height = close['bounds']
    # SetCursorPos alone need not deliver hover movement in remote sessions.
    u.mouse_event(0x8001, round((x + width/2)*65535/(u.GetSystemMetrics(0)-1)),
                  round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(1.2)
    nodes = inspect()
    assert any(n['type'] == 'ControlType.ToolTip' and n['name'] == 'Close (⌘W)' for n in nodes), 'Native tooltip did not appear'
    screenshot('verified-accessibility-tooltip.png')
    print('PASS: accessible button name and native hover tooltip', flush=True)

    nodes = inspect('navigate', 'Tabs')
    for _ in range(6):
        if any(n['name'] == 'Settings' and n['type'] == 'ControlType.TabItem' for n in nodes):
            break
        time.sleep(.2)
        nodes = inspect()
    assert any(n['name'] == 'Settings' and n['type'] == 'ControlType.TabItem' for n in nodes), [(n['type'], n['name']) for n in nodes if not n['offscreen']]
    settings = next(n for n in nodes if n['name'] == 'Settings' and n['type'] == 'ControlType.TabItem')
    click(settings['bounds'])
    assert node(inspect(), 'tabs:status')['name'] == 'Selected tab: 1'
    inspect('toggle', 'tabs:check')
    nodes = inspect()
    about = next(n for n in nodes if n['name'] == 'About' and n['type'] == 'ControlType.TabItem')
    click(about['bounds'])
    assert node(inspect(), 'tabs:status')['name'] == 'Selected tab: 2'
    inspect('invoke', 'tabs:colors'); screenshot('verified-tabs.png')
    inspect('invoke', 'tabs:colors')
    nodes = inspect()
    click(next(n for n in nodes if n['name'] == 'Settings' and n['type'] == 'ControlType.TabItem')['bounds'])
    assert node(inspect(), 'tabs:status')['name'] == 'Selected tab: 1'
    assert node(inspect(), 'tabs:check')['toggle'] == 'On', 'Tab pane lost its state'
    for vk, flags in ((0x11, 0), (0x09, 0), (0x09, 2), (0x11, 2)):
        event = Input(type=1, data=Union(key=Key(vk=vk, flags=flags)))
        assert u.SendInput(1, c.byref(event), c.sizeof(event)) == 1
    time.sleep(.25)
    assert node(inspect(), 'tabs:status')['name'] == 'Selected tab: 2'
    print('PASS: native tab clicks, Ctrl+Tab, persistent panes and color changes', flush=True)

    verify_carousel()
    verify_menus()
    verify_tree_rows()

    inspect('navigate', 'Refresh')
    inspect('invoke', 'rf:btn')
    assert node(inspect(), 'rf:status')['name'] == 'refreshed 1 time(s)'
    time.sleep(1)
    inspect('invoke', 'rf:btn')
    assert node(inspect(), 'rf:status')['name'] == 'refreshed 2 time(s)'
    screenshot('verified-refresh.png')
    # Detaching cancels the sample timer and completes the native deferral.
    inspect('navigate', 'Overview'); time.sleep(1)
    inspect('navigate', 'Refresh')
    assert node(inspect(), 'rf:status')['name'] == 'refreshed 0 time(s)'
    print('PASS: timed refresh, repeat request and navigation during refresh', flush=True)

    rect = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(rect))
    assert u.SetWindowPos(hwnd, None, rect.left, rect.top, 1100, 760, 0x14)
    time.sleep(.5)
    inspect('navigate', 'Overview'); screenshot('verified-overview.png')
    print('PASS: resize and navigation after recycled rows are released', flush=True)
    # Close with a live composition shadow as well as native event handlers.
    inspect('navigate', 'Shadow'); inspect('invoke', 'sh:glow')
    # Render callbacks must also unsubscribe when the window closes mid-flight.
    inspect('navigate', 'Basics', 'last'); inspect('invoke', 'ab:slow'); inspect('invoke', 'ab:flourish')
    assert u.PostMessageW(hwnd, 0x10, 0, 0)
finally:
    u.SetCursorPos(previous.x, previous.y)
