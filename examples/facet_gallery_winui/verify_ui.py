"""Desktop regression checks for the shared Facet gallery on native WinUI."""
from pathlib import Path
import ctypes as c
from ctypes import wintypes as w
import json
import subprocess
import sys
import time
from PIL import ImageGrab, ImageStat, ImageChops

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

def verify_button_breaks():
    nodes = inspect('navigate', 'Button')
    inspect('invoke','btn:reset')
    def reveal():
        nodes = inspect()
        target = node(nodes,'btn:break-button')
        bounds = w.RECT(); assert u.GetWindowRect(hwnd,c.byref(bounds))
        if target['offscreen'] or target['bounds'][1]+target['bounds'][3] > bounds.bottom-12:
            nodes = inspect('scroll','demo:scroll',100)
        return nodes
    nodes = reveal()
    def capture():
        value = node(inspect(), 'btn:break-button')
        x,y,width,height = value['bounds']
        # Exclude the button chrome: compare the actual caption pixels.
        return ImageGrab.grab(bbox=tuple(round(v) for v in (x+8,y+5,x+width-8,y+height-5))).convert('RGB')
    title = 'Beginning — a long caption — ending'
    for key in ('btn:break-button', 'btn:break-text'):
        assert node(nodes,key)['name'] == title
    images = []
    for expected in ('Head truncation','Middle truncation','Tail truncation','Character wrap','Word wrap','No wrap'):
        assert node(inspect(),'btn:break-status')['name'] == expected
        reveal()
        time.sleep(.15)
        images.append(capture())
        screenshot('verified-button-' + expected.lower().replace(' ','-') + '.png')
        click(node(inspect(),'btn:break-next')['bounds'])
    for a,b in ((images[0],images[1]),(images[1],images[2])):
        assert a.size == b.size
        assert sum(ImageStat.Stat(ImageChops.difference(a,b)).mean) > 3, 'Truncation modes render the same caption'
    assert images[3].height > images[0].height, 'Character wrapping stayed on one line'
    assert images[4].height > images[0].height, 'Word wrapping stayed on one line'
    before = node(inspect(),'btn:break-button')['bounds'][2]
    click(node(inspect(),'btn:break-width')['bounds'])
    after = node(inspect(),'btn:break-button')['bounds'][2]
    assert after > before * 1.8
    click(node(inspect(),'btn:break-button')['bounds'])
    click(node(inspect(),'btn:break-text')['bounds'])
    assert node(inspect(),'btn:count')['name'] == 'Clicked 2 time(s)'
    click(node(inspect(),'btn:break-width')['bounds'])
    screenshot('verified-button-breaks.png')
    print('PASS: painted head/middle/tail captions, wrapping, full accessibility names, live widths and real clicks',flush=True)


def verify_return_keys():
    inspect('navigate', 'Inputs')
    def enter():
        for flags in (0,2):
            event=Input(type=1,data=Union(key=Key(vk=13,flags=flags)))
            assert u.SendInput(1,c.byref(event),c.sizeof(event))==1
        time.sleep(.25)
    inspect('focus','in:search');enter()
    assert node(inspect(),'in:search')['focused']
    inspect('invoke','in:return')
    assert node(inspect(),'in:return-status')['name']=='Enter: next control'
    for source,target in (('in:name','in:reveal'),('in:secret','in:search-colors'),('in:search','in:area')):
        inspect('focus',source);enter()
        nodes=inspect()
        assert node(nodes,target)['focused'], (source,target,[(n['id'],n['name']) for n in nodes if n['focused']])
    # TextBox replacement must keep the live ReturnKey setting.
    inspect('invoke','in:reveal')
    inspect('focus','in:secret');enter()
    assert node(inspect(),'in:search-colors')['focused']
    inspect('invoke','in:return')
    inspect('focus','in:search');enter()
    assert node(inspect(),'in:search')['focused']
    screenshot('verified-return-keys.png')
    print('PASS: real Enter advances text/search/password in native tab order; live reset and password replacement',flush=True)


def verify_split_roles():
    nodes = inspect('navigate', 'Split')
    inspect('value', 'sp:edit', 'Retained through role changes')
    scale = u.GetDpiForWindow(hwnd)/96
    def divider():
        nodes = inspect()
        left = node(nodes, 'sp:edit')['bounds'][0]
        return next(n for n in nodes if n['name'] == 'Resize panes' and n['bounds'][0] > left)
    def colors():
        x,y,width,height = divider()['bounds']
        samples = []
        for offset in (-28,28):
            px,py = x+width/2+offset*scale,y+height-25*scale
            samples.append(ImageStat.Stat(ImageGrab.grab(bbox=tuple(round(v) for v in (px-5,py-5,px+5,py+5))).convert('RGB')).mean)
        return samples
    def distance(a,b):
        return sum(abs(x-y) for x,y in zip(a,b))/3
    time.sleep(.4)
    leading, content = colors()
    assert distance(leading,content)>3, (leading,content)
    screenshot('verified-split-leading-sidebar.png')
    click(node(inspect(), 'sp:switch')['bounds'])
    assert node(inspect(), 'sp:roles')['name'] == 'Sidebar: trailing'
    time.sleep(.4)
    a,b = colors()
    assert distance(a,content)<3 and distance(b,content)>3, (a,b,content)
    screenshot('verified-split-trailing-sidebar.png')
    click(node(inspect(), 'sp:switch')['bounds'])
    assert node(inspect(), 'sp:roles')['name'] == 'Both panes: content'
    time.sleep(.4)
    a,b = colors()
    assert distance(a,content)<3 and distance(b,content)<3, (a,b,content)
    assert node(inspect(), 'sp:edit')['value'] == 'Retained through role changes'
    before = divider()['bounds']
    click(before)
    x,y,width,height = before
    def move(offset):
        u.mouse_event(0x8001,round((x+width/2+offset)*65535/(u.GetSystemMetrics(0)-1)),round((y+height/2)*65535/(u.GetSystemMetrics(1)-1)),0,0)
    move(0);time.sleep(.15);u.mouse_event(2,0,0,0,0)
    try:
        time.sleep(.15)
        for offset in range(10,71,10):
            move(offset);time.sleep(.08)
    finally:
        u.mouse_event(4,0,0,0,0)
    time.sleep(.2)
    assert divider()['bounds'][0] > x+40
    click(node(inspect(), 'sp:switch')['bounds'])
    assert node(inspect(), 'sp:roles')['name'] == 'Sidebar: leading'
    assert node(inspect(), 'sp:edit')['value'] == 'Retained through role changes'
    time.sleep(.4)
    a,b = colors()
    assert distance(a,b)>3
    inspect('navigate','Overview')
    nodes = inspect('navigate','Split')
    assert node(nodes,'sp:roles')['name']=='Sidebar: leading'
    print('PASS: painted sidebar role switches, content reset, real divider drag, retained editor and remount', flush=True)


def verify_html_labels():
    nodes = inspect('navigate', 'Label')
    rendered = 'Bold and italic · underlined & 🚀\nSecond line with strikethrough and code.'
    assert node(nodes, 't:html')['name'] == rendered
    before = node(nodes, 't:html')['bounds']
    assert before[2] > 200 and before[3] > 30
    screenshot('verified-html-label.png')
    click(node(nodes, 't:html-toggle')['bounds'])
    nodes = inspect()
    assert node(nodes, 't:html-status')['name'] == 'Literal text'
    assert node(nodes, 't:html')['name'].startswith('<b>Bold <i>')
    click(node(nodes, 't:html-toggle')['bounds'])
    nodes = inspect()
    assert node(nodes, 't:html')['name'] == rendered
    assert node(nodes, 't:html-status')['name'] == 'HTML formatting'
    inspect('navigate', 'Overview')
    nodes = inspect('navigate', 'Label')
    assert node(nodes, 't:html')['name'] == rendered
    print('PASS: rendered HTML label, literal toggle, live layout and navigation/remount', flush=True)


def verify_window_buttons():
    nodes = inspect('navigate', 'Window chrome')
    inspect('value', 'wc:edit', 'Retained through window actions')
    scale = u.GetDpiForWindow(hwnd)/96
    before = node(nodes, 'wc:buttons:minimize')['bounds'][0]
    click(node(nodes, 'wc:space')['bounds'])
    nodes = inspect()
    assert node(nodes, 'wc:spacing')['name'] == 'Spacing: 40 pt'
    assert abs(before-node(nodes, 'wc:buttons:minimize')['bounds'][0]-40*scale)<2
    click(node(nodes, 'wc:space')['bounds'])
    nodes = inspect()
    first = node(nodes, 'wc:buttons:minimize')['bounds']
    last = node(nodes, 'wc:buttons:close')['bounds']
    region = tuple(round(v) for v in (first[0], first[1], last[0]+last[2], last[1]+last[3]))
    def glyph_pixels():
        return sum(min(pixel)>160 for pixel in ImageGrab.grab(bbox=region).convert('RGB').getdata())
    def move_to(bounds):
        x,y,width,height=bounds
        u.mouse_event(0x8001,round((x+width/2)*65535/(u.GetSystemMetrics(0)-1)),round((y+height/2)*65535/(u.GetSystemMetrics(1)-1)),0,0)
        time.sleep(.2)
    assert glyph_pixels()>15
    click(node(nodes, 'wc:hover')['bounds'])
    assert glyph_pixels()==0, 'Hover mode did not hide the controls'
    move_to(node(inspect(), 'wc:title')['bounds'])
    assert glyph_pixels()>15, 'Hovering the containing bar did not reveal controls'
    screenshot('verified-window-hover.png')
    move_to(node(inspect(), 'wc:edit')['bounds'])
    assert glyph_pixels()==0
    inspect('focus', 'wc:hover')
    for vk,flags in ((0x10,0),(0x09,0),(0x09,2),(0x10,2)):
        event=Input(type=1,data=Union(key=Key(vk=vk,flags=flags)))
        assert u.SendInput(1,c.byref(event),c.sizeof(event))==1
    time.sleep(.2)
    assert glyph_pixels()>15, 'Shift+Tab did not reach and reveal hidden controls'
    inspect('focus', 'wc:edit')
    assert glyph_pixels()==0
    inspect('focus', 'wc:buttons:maximize')
    assert glyph_pixels()>15, 'Keyboard focus did not reveal controls'
    inspect('focus', 'wc:edit')
    assert glyph_pixels()==0
    inspect('invoke', 'wc:hover')
    nodes = inspect()
    u.IsZoomed.argtypes=[w.HWND]
    click(node(nodes, 'wc:buttons:maximize')['bounds']); time.sleep(.3)
    assert u.IsZoomed(hwnd)
    nodes = inspect()
    assert node(nodes, 'wc:buttons:maximize')['name']=='Restore'
    click(node(nodes, 'wc:buttons:maximize')['bounds']); time.sleep(.3)
    assert not u.IsZoomed(hwnd)
    assert node(inspect(), 'wc:edit')['value']=='Retained through window actions'
    inspect('navigate', 'Overview')
    nodes = inspect('navigate', 'Window chrome')
    assert node(nodes, 'wc:visibility')['name']=='Buttons always visible'
    assert node(nodes, 'wc:spacing')['name']=='Spacing: default'
    screenshot('verified-window-buttons.png')
    print('PASS: window buttons live spacing, containing-bar hover, keyboard reveal, maximize/restore, retained input and remount', flush=True)

def verify_symbols():
    inspect('navigate', 'Controls')
    nodes = inspect('scroll', 'demo:scroll', 100)
    scale = u.GetDpiForWindow(hwnd)/96
    def corner_pixel():
        x, y, width, height = node(inspect(), 'ctl:radius-radio')['bounds']
        image = ImageGrab.grab().convert('RGB')
        return image.getpixel((round(x+3*scale), round(y+3*scale)))
    def purple(pixel):
        return all(abs(a-b)<8 for a,b in zip(pixel, (89,46,140)))
    assert purple(corner_pixel()), 'Radio surface background missing'
    click(node(nodes, 'ctl:radius')['bounds'])
    assert node(inspect(), 'ctl:radius-status')['name'] == 'Asymmetric corners'
    assert not purple(corner_pixel()), 'Radio corner radius did not paint'
    screenshot('verified-radio-corners.png')
    click(node(inspect(), 'ctl:radius')['bounds'])
    assert purple(corner_pixel()), 'Radio radius reset did not paint'
    nodes = inspect('navigate', 'Icons')
    if node(nodes, 'ico:switch')['offscreen']:
        nodes = inspect('scroll', 'demo:scroll', 100)
    assert node(nodes, 'ico:system-status')['name'] == 'Windows: Print (48 pt)'
    screenshot('verified-system-symbols.png')
    click(node(nodes, 'ico:switch')['bounds'])
    nodes = inspect()
    assert node(nodes, 'ico:system-status')['name'] == 'Bundled: home (32 pt)'
    screenshot('verified-bundled-symbol.png')
    click(node(nodes, 'ico:switch')['bounds'])
    assert node(inspect(), 'ico:system-status')['name'] == 'Windows: Print (48 pt)'
    inspect('navigate', 'Overview')
    inspect('navigate', 'Icons')
    assert node(inspect(), 'ico:system-status')['name'] == 'Windows: Print (48 pt)'
    print('PASS: native system symbols, bundled switch, resize/color changes and remount', flush=True)

def verify_tables():
    inspect('navigate', 'Table')
    nodes = inspect('scroll', 'demo:scroll', 100)
    scale = u.GetDpiForWindow(hwnd)/96
    def geometry(nodes):
        ada = node(nodes, 'tb:ada')['bounds']
        grace = node(nodes, 'tb:grace')['bounds']
        alan = node(nodes, 'tb:alan')['bounds']
        # TextBlock's reported bounds can end at the last glyph rather than
        # the allocated row edge. Equal-font row origins expose row pitch.
        return ada[0], grace[1]-ada[1], alan[1]-grace[1]
    baseline_x, first, second = geometry(nodes)
    assert abs(first - 36*scale) < 2 and abs(second - 36*scale) < 2
    click(node(nodes, 'tb:resize')['bounds'])
    _, first, second = geometry(inspect('scroll', 'demo:scroll', 100))
    assert abs(first - 52*scale) < 2 and abs(second - 52*scale) < 2
    click(node(inspect(), 'tb:vary')['bounds'])
    _, first, second = geometry(inspect('scroll', 'demo:scroll', 100))
    assert abs(second - 68*scale) < 2 and second > first+20*scale
    screenshot('verified-table-uneven.png')
    click(node(inspect(), 'tb:vary')['bounds'])
    for style, grouped in (('Form', True), ('Settings', True), ('Menu', False), ('Data', False)):
        click(node(inspect(), 'tb:cycle')['bounds'])
        nodes = inspect('scroll', 'demo:scroll', 100)
        assert node(nodes, 'tb:style')['name'] == style
        x, first, second = geometry(nodes)
        inset = x - baseline_x
        gap = first - 52*scale
        assert abs(inset - (12*scale if grouped else 0)) < 2, (style,inset)
        assert abs(gap - (8*scale if grouped else 0)) < 2, (style,gap)
        assert abs(second-first) < 2, (style,first,second, node(nodes,"tb:alan"))
        if style == 'Form':
            screenshot('verified-table-form.png')
    click(node(inspect(), 'tb:resize')['bounds'])
    _, first, second = geometry(inspect('scroll', 'demo:scroll', 100))
    assert abs(first - 36*scale) < 2 and abs(second - 36*scale) < 2
    print('PASS: real table sizing/uneven switches and four style transitions', flush=True)


def verify_input_visuals():
    inspect('navigate', 'Inputs')
    inspect('value', 'in:search', 'Find this')
    inspect('invoke', 'in:search-colors')
    nodes = inspect('focus', 'in:search')
    x, y, width, height = node(nodes, 'in:search')['bounds']
    pixels = ImageGrab.grab(bbox=tuple(round(v) for v in (x, y, x+width, y+height))).convert('RGB')
    assert sum(r > 220 and 100 < g < 180 and b < 100 for r, g, b in pixels.getdata()) > 8, 'Missing orange search icon'
    assert sum(70 < r < 140 and g > 170 and b > 220 for r, g, b in pixels.getdata()) > 8, 'Missing blue clear glyph'
    screenshot('verified-search-colors.png')
    scale = u.GetDpiForWindow(hwnd)/96
    click((x+width-28*scale, y, 25*scale, height))
    nodes = inspect()
    assert node(nodes, 'in:search')['value'] == ''
    assert node(nodes, 'in:search_echo')['name'] == 'Query: (empty)'
    inspect('invoke', 'in:search-colors')
    inspect('scroll', 'demo:scroll', 100)
    nodes = inspect()
    fixed = node(nodes, 'in:autosize')['bounds'][3]
    inspect('invoke', 'in:auto')
    nodes = inspect('scroll', 'demo:scroll', 100)
    growing = node(nodes, 'in:autosize')['bounds'][3]
    assert growing > fixed + 20*scale, (fixed, growing)
    screenshot('verified-editor-autosize.png')
    inspect('invoke', 'in:auto')
    assert abs(node(inspect(), 'in:autosize')['bounds'][3] - fixed) < 2

    inspect('navigate', 'Values')
    inspect('range', 'val:slider', '.4')
    inspect('invoke', 'val:thumb')
    nodes = inspect()
    assert node(nodes, 'val:thumb-status')['name'] == 'Image thumb'
    x, y, width, height = node(nodes, 'val:slider')['bounds']
    def thumb_pixels():
        return ImageGrab.grab(bbox=tuple(round(v) for v in (x, y, x+width, y+height))).convert('RGB')
    pixels = thumb_pixels()
    points = [(col, row) for row in range(pixels.height) for col in range(pixels.width)
              if all(abs(a-b) < 8 for a,b in zip(pixels.getpixel((col,row)), (89,48,16)))]
    assert len(points) > 15, 'Image thumb did not paint its diamond'
    start_x = x + sum(p[0] for p in points)/len(points)
    start_y = y + sum(p[1] for p in points)/len(points)
    def mouse(px, py):
        u.mouse_event(0x8001, round(px*65535/(u.GetSystemMetrics(0)-1)), round(py*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
        time.sleep(.08)
    mouse(start_x, start_y)
    u.mouse_event(2,0,0,0,0)
    try:
        mouse(start_x+25, start_y)
        inspect('invoke', 'val:colors')  # Must defer template replacement during the drag.
        for offset in (50, 75, 100):
            mouse(start_x+offset, start_y)
    finally:
        u.mouse_event(4,0,0,0,0)
    time.sleep(.3)
    nodes = inspect()
    assert node(nodes, 'val:readout')['name'] != 'Value: 40%'
    assert node(nodes, 'val:style_status')['name'] == 'Custom slider colors'
    screenshot('verified-slider-image.png')
    inspect('invoke', 'val:thumb')
    assert node(inspect(), 'val:thumb-status')['name'] == 'Native thumb'
    pixels = thumb_pixels()
    assert sum(all(abs(a-b) < 8 for a,b in zip(pixel,(89,48,16))) for pixel in pixels.getdata()) == 0
    inspect('invoke', 'val:colors')
    print('PASS: rendered search colors/native clear, editor growth/reset, image-thumb drag/recolor/native reset', flush=True)


def verify_label_alignment():
    nodes = inspect('navigate', 'Label')
    if any(node(nodes, key)['offscreen'] for key in ('t:top', 't:center', 't:bottom')):
        nodes = inspect('scroll', 'demo:scroll', 100)
    time.sleep(.3)
    centers = []
    for key in ('t:top', 't:center', 't:bottom'):
        x, y, width, height = node(nodes, key)['bounds']
        assert width > 0 and height > 0
        pixels = ImageGrab.grab(bbox=(round(x), round(y), round(x + width), round(y + height))).convert('RGB')
        rows = [row for row in range(pixels.height)
                if any(min(pixels.getpixel((col, row))) > 170 for col in range(pixels.width))]
        assert rows, f'Missing rendered text: {key}'
        centers.append(y + (min(rows) + max(rows))/2)
    assert centers[1] - centers[0] > 15, centers
    assert centers[2] - centers[1] > 15, centers
    assert abs((centers[0] + centers[2])/2 - centers[1]) < 3, centers
    screenshot('verified-label-alignment.png')
    print('PASS: rendered top/center/bottom label alignment', flush=True)


def verify_button_modes():
    nodes = inspect('navigate', 'Button')
    click(node(nodes, 'btn:reset')['bounds'])
    assert node(inspect(), 'btn:count')['name'] == 'Clicked 0 time(s)'
    targets = ('btn:mode-button', 'btn:mode-icon', 'btn:mode-text')
    for mode, count in (('toggle', 3), ('ordinary', 6), ('toggle', 9), ('ordinary', 12)):
        click(node(inspect(), 'btn:switch-mode')['bounds'])
        nodes = inspect()
        assert node(nodes, 'btn:mode-status')['name'] == f'Mode: {mode}'
        for offset, key in enumerate(targets):
            value = node(nodes, key)
            assert (value['toggle'] is not None) == (mode == 'toggle'), value
            click(value['bounds'])
            nodes = inspect()
            expected = f'Clicked {count - 2 + offset} time(s)'
            if node(nodes, 'btn:count')['name'] != expected:
                screenshot('failed-button-mode.png')
                raise AssertionError((mode, key, value, node(nodes, 'btn:count'), expected))
        assert node(nodes, 'btn:count')['name'] == f'Clicked {count} time(s)'
    screenshot('verified-button-modes.png')
    print('PASS: real clicks across live ordinary/toggle changes for all three button kinds', flush=True)


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

def verify_popup_caption():
    inspect('navigate', 'Pickers')
    click(node(inspect(), 'pk:caption')['bounds'])
    nodes = inspect()
    assert node(nodes, 'pk:caption-status')['name'] == 'Fixed caption'
    x, y, width, height = node(nodes, 'pk:popup')['bounds']
    caption_box = (round(x + 12), round(y + 10), round(x + min(width - 45, 260)), round(y + height - 10))
    def caption_pixels():
        return ImageGrab.grab(bbox=caption_box).convert('L').point(lambda p: 255 if p > 160 else 0)
    fixed_pixels = caption_pixels()
    click(node(nodes, 'pk:popup')['bounds'])
    # Pick Green from the actual native dropdown, then verify its fixed caption.
    nodes = inspect()
    click(next(n for n in nodes if n['name'] == 'Green')['bounds'])
    nodes = inspect()
    assert node(nodes, 'pk:popup_status')['name'] == 'Selected index: 1'
    difference = ImageStat.Stat(ImageChops.difference(fixed_pixels, caption_pixels())).mean[0] / 255
    assert difference < .02, f'Fixed caption changed after native selection ({difference:.1%} text pixels)'
    screenshot('verified-popup-caption.png')
    # The caption override must not change the dropdown's actual item text.
    click(node(nodes, 'pk:popup')['bounds'])
    choices = inspect()
    assert any(n['name'] == 'Red' for n in choices)
    assert any(n['name'] == 'Green' for n in choices)
    u.keybd_event(27, 0, 0, 0); u.keybd_event(27, 0, 2, 0)
    time.sleep(.25)
    click(node(nodes, 'pk:caption')['bounds'])
    nodes = inspect()
    assert node(nodes, 'pk:caption-status')['name'] == 'Selected item caption'
    # WinUI's ComboBox peer hides its closed presenter text. The native
    # caption probe checks that text; capture the rendered result here.
    screenshot('verified-popup-caption-restored.png')
    print('PASS: native popup selection, fixed caption and selected caption restoration', flush=True)


def verify_hybrid():
    nodes = inspect('navigate', 'Web')
    click(node(nodes, 'web:local')['bounds'])
    deadline = time.time() + 20
    while True:
        nodes = inspect()
        if node(nodes, 'web:lifecycle')['name'] == 'Starting: 1 / Ready: 1':
            x, y, width, height = node(nodes, 'web:view')['bounds']
            pixels = ImageGrab.grab(bbox=tuple(round(v) for v in (x, y, x+width, y+height))).convert('RGB')
            # WinUI's WebView2 peer does not expose this Chromium subtree in
            # the XAML UIA traversal. Locate the rendered demo button instead.
            points = [(i % pixels.width, i // pixels.width) for i, (r, g, b) in enumerate(pixels.getdata())
                      if abs(r-181) < 3 and abs(g-225) < 3 and abs(b-240) < 3]
            if len(points) > 100:
                left, right = min(p[0] for p in points), max(p[0] for p in points)
                top, bottom = min(p[1] for p in points), max(p[1] for p in points)
                page_button = [x+left, y+top, right-left, bottom-top]
                break
        if time.time() >= deadline:
            screenshot('hybrid-load-failure.png')
            raise AssertionError('Local hybrid page did not load: ' + repr([n for n in nodes if n['id'].startswith('web:') or n['name'] == 'Send to native']))
        time.sleep(.2)
    assert int(node(nodes, 'web:resources')['name'].split(': ')[1]) >= 3
    click(page_button)
    assert node(inspect(), 'web:messages')['name'] == 'Page messages: 1'
    message_box = tuple(round(v) for v in (x, y+bottom+8, x+width, y+height))
    before_message = ImageGrab.grab(bbox=message_box).convert('RGB')
    click(node(inspect(), 'web:send')['bounds'])
    deadline = time.time() + 5
    while node(inspect(), 'web:messages')['name'] != 'Page messages: 2':
        assert time.time() < deadline, 'Page did not acknowledge native message'
        time.sleep(.1)
    nodes = inspect()
    updated = ImageGrab.grab(bbox=message_box).convert('RGB')
    assert ImageChops.difference(before_message, updated).getbbox(), 'Page did not repaint the received-message region'
    screenshot('verified-hybrid.png')
    # Replace an active browser, then leave while its replacement initializes.
    click(node(nodes, 'web:local')['bounds'])
    inspect('navigate', 'Overview')
    time.sleep(.3)
    print('PASS: local hybrid assets, real web/native button messages, displayed payload and navigation during initialization', flush=True)


def verify_swiping():
    nodes = inspect('navigate', 'Swipe')
    original_x = node(nodes, 'sw:title')['bounds'][0]
    scale = u.GetDpiForWindow(hwnd) / 96

    def drag(distance, cancel=False, vertical=0):
        bounds = node(inspect(), 'sw:title')['bounds']
        start = (bounds[0] + min(bounds[2] / 2, 300 * scale), bounds[1] + bounds[3] / 2)
        def move(x, y):
            u.mouse_event(0x8001, round(x*65535/(u.GetSystemMetrics(0)-1)),
                          round(y*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
        move(*start); time.sleep(.15)
        u.mouse_event(2, 0, 0, 0, 0)
        try:
            for i in range(1, 21):
                move(start[0] - distance * scale * i / 20, start[1] + vertical * scale * i / 20)
                time.sleep(.025)
            if cancel:
                u.keybd_event(27, 0, 0, 0); time.sleep(.2)
                u.keybd_event(27, 0, 2, 0); time.sleep(.15)
        finally:
            u.mouse_event(4, 0, 0, 0, 0)
        time.sleep(.35)

    def events(expected):
        actual = node(inspect(), 'sw:events')['name']
        assert actual == expected, actual

    drag(2)
    events('Started 0 / ended 0 / opened 0 / closed 0')
    drag(120)
    events('Started 1 / ended 1 / opened 1 / closed 0')
    nodes = inspect()
    # UIA clips translated content bounds to the row; measure the revealed buttons.
    assert not node(nodes, 'sw:archive:reveal')['offscreen']
    assert abs(node(nodes, 'sw:archive:reveal')['bounds'][2] - 88 * scale) < 3
    assert int(node(nodes, 'sw:changes')['name'].split(': ')[1]) > 0
    screenshot('verified-swipe-reveal.png')
    click(node(nodes, 'sw:archive:reveal')['bounds'])
    assert node(inspect(), 'sw:status')['name'] == 'last action: Archive'
    events('Started 1 / ended 1 / opened 1 / closed 1')
    click(node(inspect(), 'sw:threshold')['bounds'])
    drag(120)
    events('Started 2 / ended 2 / opened 1 / closed 2')
    assert abs(node(inspect(), 'sw:title')['bounds'][0] - original_x) < 3
    drag(175, cancel=True)
    events('Started 3 / ended 3 / opened 1 / closed 2')
    assert abs(node(inspect(), 'sw:title')['bounds'][0] - original_x) < 3
    click(node(inspect(), 'sw:enable')['bounds'])
    drag(175)
    events('Started 3 / ended 3 / opened 1 / closed 2')
    click(node(inspect(), 'sw:enable')['bounds'])
    drag(175)
    events('Started 4 / ended 4 / opened 2 / closed 2')
    click(node(inspect(), 'sw:delete:reveal')['bounds'])
    assert node(inspect(), 'sw:status')['name'] == 'last action: Delete'
    events('Started 4 / ended 4 / opened 2 / closed 3')
    drag(175)
    events('Started 5 / ended 5 / opened 3 / closed 3')
    drag(-175)
    events('Started 6 / ended 6 / opened 3 / closed 4')
    assert abs(node(inspect(), 'sw:title')['bounds'][0] - original_x) < 3
    screenshot('verified-swipe-actions.png')
    inspect('navigate', 'Overview')
    print('PASS: real swipe reveal, action buttons, threshold changes, Escape, disabled gestures and callbacks', flush=True)


def verify_reordering():
    inspect('navigate', 'Collection')

    def drag(cancel=False, source_id='col:cell:0', destination_id='col:cell:2'):
        nodes = inspect()
        source_node, destination_node = node(nodes, source_id), node(nodes, destination_id)
        assert not source_node['offscreen'] and not destination_node['offscreen']
        source, destination = source_node['bounds'], destination_node['bounds']
        start = (source[0] + source[2]/2, source[1] + source[3]/2)
        end = (destination[0] + destination[2]/2, destination[1] + destination[3]/2)
        def move(x, y):
            u.mouse_event(0x8001, round(x*65535/(u.GetSystemMetrics(0)-1)),
                          round(y*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
        move(*start)
        time.sleep(.15)
        u.mouse_event(2, 0, 0, 0, 0)
        try:
            time.sleep(.2)
            for i in range(1, 31):
                move(start[0] + (end[0]-start[0])*i/30,
                     start[1] + (end[1]-start[1])*i/30)
                time.sleep(.035)
            time.sleep(.2)
            if cancel:
                u.keybd_event(27, 0, 0, 0)
                time.sleep(.2)
                u.keybd_event(27, 0, 2, 0)
                time.sleep(.2)
        finally:
            u.mouse_event(4, 0, 0, 0, 0)
        time.sleep(.4)

    drag()
    nodes = inspect()
    screenshot('reordering-first-drop.png')
    assert node(nodes, 'col:move-status')['name'] == 'Moves: 1', [n for n in nodes if n['id'].startswith('col:')]
    assert [node(nodes, f'col:cell:{i}')['name'] for i in range(3)] == ['Cell 2', 'Cell 3', 'Cell 1']
    click(node(nodes, 'col:reorder')['bounds'])
    drag()
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 1'
    click(node(inspect(), 'col:reorder')['bounds'])
    drag(cancel=True)
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 1'
    drag()
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 2'
    screenshot('verified-reordering.png')
    click(node(inspect(), 'col:groups')['bounds'])
    inspect('scroll', 'col:grid', 55)
    drag(source_id='col:cell:4', destination_id='col:cell:5')
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 2'
    click(node(inspect(), 'col:mix')['bounds'])
    drag(source_id='col:cell:4', destination_id='col:group:2')
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 2'
    drag(source_id='col:cell:4', destination_id='col:cell:5')
    assert node(inspect(), 'col:move-status')['name'] == 'Moves: 3'
    screenshot('verified-reordering-groups.png')
    print('PASS: real collection drag, application order, disabled dragging, Escape, headers and cross-group policy', flush=True)


def verify_grouped_items():
    for page, prefix, first in (('List', 'lst', 'lst:label:0'), ('Collection', 'col', 'col:cell:0')):
        nodes = inspect('navigate', page)
        click(node(nodes, prefix + ':groups')['bounds'])
        nodes = inspect()
        empty = node(nodes, prefix + ':group:0')
        header = node(nodes, prefix + ':group:1')
        item = node(nodes, first)
        assert empty['name'] == 'Empty group'
        assert header['name'] == 'Group 1'
        assert empty['bounds'][1] < header['bounds'][1] < item['bounds'][1]
        if page == 'Collection':
            second = node(nodes, 'col:cell:1')
            assert abs(item['bounds'][1] - second['bounds'][1]) < 2
            assert second['bounds'][0] > item['bounds'][0]
        screenshot('verified-grouped-' + prefix + '.png')
        click(node(nodes, prefix + ':groups')['bounds'])
        nodes = inspect()
        assert not any(n['id'].startswith(prefix + ':group:') for n in nodes)
        assert node(nodes, first)['name'] == item['name']
    print('PASS: real grouping toggles, empty groups, list headers and collection grid geometry', flush=True)


def verify_refresh_host():
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


def verify_list_refresh():
    nodes = inspect('navigate', 'List')
    click(node(nodes, 'lst:edit:0')['bounds'])
    u.keybd_event(17, 0, 0, 0); u.keybd_event(65, 0, 0, 0)
    u.keybd_event(65, 0, 2, 0); u.keybd_event(17, 0, 2, 0)
    type_text('Retained refresh draft')
    click(node(inspect(), 'lst:refresh-start')['bounds'])
    assert node(inspect(), 'lst:refresh-status')['name'] == 'Refreshing: 1'
    click(node(inspect(), 'lst:refresh-start')['bounds'])
    nodes = inspect()
    assert node(nodes, 'lst:refresh-status')['name'] == 'Refreshing: 1'
    assert node(nodes, 'lst:edit:0')['value'] == 'Retained refresh draft'
    screenshot('verified-list-refresh.png')
    click(node(nodes, 'lst:refresh-end')['bounds'])
    assert node(inspect(), 'lst:refresh-status')['name'] == 'Idle: 1'
    click(node(inspect(), 'lst:refresh-allow')['bounds'])
    def open_refresh_menu():
        bounds = node(inspect(), 'lst:label:0')['bounds']
        click(bounds)
        u.mouse_event(8, 0, 0, 0, 0); u.mouse_event(16, 0, 0, 0, 0)
        time.sleep(.3)
    open_refresh_menu()
    assert not node(inspect(), 'facet:refresh')['enabled']
    u.keybd_event(27, 0, 0, 0); u.keybd_event(27, 0, 2, 0)
    click(node(inspect(), 'lst:refresh-allow')['bounds'])
    open_refresh_menu()
    nodes = inspect()
    assert node(nodes, 'facet:refresh')['enabled']
    click(node(nodes, 'facet:refresh')['bounds'])
    assert node(inspect(), 'lst:refresh-status')['name'] == 'Refreshing: 2'
    click(node(inspect(), 'lst:refresh-end')['bounds'])
    nodes = inspect()
    assert node(nodes, 'lst:refresh-status')['name'] == 'Idle: 2'
    assert node(nodes, 'lst:edit:0')['value'] == 'Retained refresh draft'
    print('PASS: list begin/end refresh, repeat suppression, retained edits and native refresh menu availability', flush=True)


def verify_time_open():
    nodes = inspect('navigate', 'Pickers')
    assert node(nodes, 'FacetTimeButton')['name'], 'Time trigger lost its accessible caption'
    click(node(nodes, 'pk:time')['bounds'])
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 1, closed 0, selected 0'
    screenshot('verified-time-open.png')
    u.keybd_event(40, 0, 0, 0); u.keybd_event(40, 0, 2, 0)
    time.sleep(.4)
    u.keybd_event(13, 0, 0, 0); u.keybd_event(13, 0, 2, 0)
    time.sleep(.4)
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 1, closed 1, selected 1', node(inspect(), 'pk:time-status')
    click(node(inspect(), 'pk:time-open')['bounds'])
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 2, closed 1, selected 1'
    u.keybd_event(27, 0, 0, 0); u.keybd_event(27, 0, 2, 0)
    time.sleep(.4)
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 2, closed 2, selected 1'
    # Programmatic opening restores the launcher; focus the picker for Alt+Down.
    inspect('focus', 'FacetTimeButton')
    u.keybd_event(18, 0, 0, 0); u.keybd_event(40, 0, 0, 0)
    u.keybd_event(40, 0, 2, 0); u.keybd_event(18, 0, 2, 0)
    time.sleep(.4)
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 3, closed 2, selected 1', node(inspect(), 'pk:time-status')
    u.keybd_event(27, 0, 0, 0); u.keybd_event(27, 0, 2, 0)
    time.sleep(.4)
    assert node(inspect(), 'pk:time-status')['name'] == 'Opened 3, closed 3, selected 1'
    print('PASS: time picker mouse/Alt+Down opening, native selection, Escape and programmatic opening', flush=True)


def verify_secure_modes():
    nodes = inspect('navigate', 'Inputs')
    click(node(nodes, 'in:secret')['bounds'])
    type_text('MixedCase')
    assert node(inspect(), 'in:secret')['password']
    click(node(inspect(), 'in:reveal')['bounds'])
    value = node(inspect(), 'in:secret')
    assert not value['password'] and value['value'] == 'MixedCase', value
    inspect('focus', 'in:secret')
    u.keybd_event(35, 0, 0, 0); u.keybd_event(35, 0, 2, 0)
    type_text('More')
    assert node(inspect(), 'in:secret')['value'] == 'MixedCaseMore'
    click(node(inspect(), 'in:reveal')['bounds'])
    assert node(inspect(), 'in:secret')['password']
    inspect('focus', 'in:secret')
    u.keybd_event(35, 0, 0, 0); u.keybd_event(35, 0, 2, 0)
    type_text('Again')
    click(node(inspect(), 'in:reveal')['bounds'])
    value = node(inspect(), 'in:secret')
    assert not value['password'] and value['value'] == 'MixedCaseMoreAgain', value
    click(node(inspect(), 'in:reveal')['bounds'])
    assert node(inspect(), 'in:secret')['password']
    screenshot('verified-secure-modes.png')
    print('PASS: real typing across secure/plain switches and native password masking', flush=True)


def verify_clear_button():
    inspect('navigate', 'Inputs')
    click(node(inspect(), 'in:name')['bounds'])
    type_text('Clear me')
    def click_clear_position():
        x, y, width, height = node(inspect(), 'in:name')['bounds']
        scale = u.GetDpiForWindow(hwnd) / 96
        click([x + width - 15 * scale - 2, y + height / 2 - 2, 4, 4])
    click_clear_position()
    assert node(inspect(), 'in:name')['value'] == 'Clear me', 'Never still allowed clearing'
    click(node(inspect(), 'in:clear')['bounds'])
    inspect('focus', 'in:name')
    nodes = inspect()
    assert node(nodes, 'in:clear-label')['name'] == 'Clear: while editing'
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, Clear me'
    screenshot('verified-clear-button.png')
    click_clear_position()
    nodes = inspect()
    assert node(nodes, 'in:name')['value'] == ''
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, stranger'
    inspect('focus', 'in:name')
    type_text('Keep me')
    click(node(inspect(), 'in:clear')['bounds'])
    inspect('focus', 'in:name')
    click_clear_position()
    nodes = inspect()
    assert node(nodes, 'in:clear-label')['name'] == 'Clear: never'
    assert node(nodes, 'in:name')['value'] == 'Keep me'
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, Keep me'
    screenshot('verified-clear-button-never.png')
    print('PASS: real clear-button click, callback and live Never/WhileEditing switching', flush=True)


def verify_input_transform():
    inspect('navigate', 'Inputs')
    click(node(inspect(), 'in:name')['bounds'])
    type_text('AbC')
    assert node(inspect(), 'in:name_echo')['name'] == 'Hello, AbC'
    click(node(inspect(), 'in:case')['bounds'])
    nodes = inspect()
    assert node(nodes, 'in:name')['value'] == 'ABC'
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, AbC', 'Display casing fired a text callback'
    inspect('focus', 'in:name')
    type_text('xy')
    nodes = inspect()
    typed = node(nodes, 'in:name')['value']
    assert typed == typed.upper() and 'XY' in typed
    assert node(nodes, 'in:name_echo')['name'] == 'Hello, ' + typed
    click(node(nodes, 'in:case')['bounds'])
    inspect('focus', 'in:search')
    type_text('MiXeD')
    nodes = inspect()
    assert node(nodes, 'in:search')['value'] == 'mixed'
    assert node(nodes, 'in:search_echo')['name'] == 'Query: mixed'
    click(node(nodes, 'in:case')['bounds'])
    inspect('focus', 'in:name')
    type_text('zZ')
    assert 'zZ' in node(inspect(), 'in:name')['value']
    screenshot('verified-input-casing.png')
    print('PASS: real upper/lowercase typing, model readback, silent display transform and default reset', flush=True)


def verify_row_retention():
    inspect('navigate', 'List')
    click(node(inspect(), 'lst:edit:1')['bounds'])
    type_text(' retained')
    draft = node(inspect(), 'lst:edit:1')['value']
    assert 'retained' in draft
    click(node(inspect(), 'lst:rebind')['bounds'])
    nodes = inspect()
    assert node(nodes, 'lst:edit:1')['value'] == draft
    assert node(nodes, 'lst:label:1')['name'].endswith('update 1')
    click(node(nodes, 'lst:reshape')['bounds'])
    nodes = inspect()
    assert node(nodes, 'lst:badge')['name'] == 'Changed shape'
    assert node(nodes, 'lst:edit:1')['value'] == draft
    # Wheel down and back without discarding the nearby edited row.
    click(node(nodes, 'lst:edit:1')['bounds'])
    u.mouse_event(0x0800, 0, 0, (-120) & 0xffffffff, 0)
    time.sleep(.4)
    u.mouse_event(0x0800, 0, 0, 120, 0)
    time.sleep(.4)
    assert node(inspect(), 'lst:edit:1')['value'] == draft
    screenshot('verified-list-retention.png')
    print('PASS: list typing, retained edits, binding refresh and selective row-kind replacement', flush=True)


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
    def ordered(*keys):
        snapshot = inspect()
        values = [node(snapshot, key) for key in keys]
        for a, b in zip(values, values[1:]):
            assert a['bounds'][0] < b['bounds'][0], (a, b)
            assert abs(a['bounds'][1] - b['bounds'][1]) < 2, (a, b)
    ordered('mn:menu', 'mn:more')
    ordered('mn:sidebar', 'mn:primary', 'mn:tool', 'mn:secondary')
    click(node(inspect(), 'mn:priority')['bounds'])
    ordered('mn:more', 'mn:menu')
    ordered('mn:sidebar', 'mn:tool', 'mn:primary', 'mn:secondary')
    click(node(inspect(), 'mn:placement')['bounds'])
    ordered('mn:sidebar', 'mn:primary', 'mn:tool', 'mn:secondary')
    click(node(inspect(), 'mn:placement')['bounds'])
    click(node(inspect(), 'mn:priority')['bounds'])
    ordered('mn:menu', 'mn:more')
    ordered('mn:sidebar', 'mn:primary', 'mn:tool', 'mn:secondary')
    screenshot('verified-command-priority.png')
    print('PASS: live window-menu priority, toolbar placement/priority and reset preserve declared tie order', flush=True)
    nodes = inspect()
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
    nodes = inspect('navigate', 'Overview')
    assert not any(n['id'].startswith('mn:') for n in nodes)
    nodes = inspect('navigate', 'Menus')
    assert node(nodes, 'mn:status')['name'] == 'No actions yet'
    ordered('mn:menu', 'mn:more')
    ordered('mn:sidebar', 'mn:primary', 'mn:tool', 'mn:secondary')
    print('PASS: navigation removes window commands and remount restores their initial order', flush=True)


def verify_carousel():
    nodes = inspect('navigate', 'Carousel')
    inspect('toggle', 'cr:keep')
    click(node(inspect(), 'cr:bounces')['bounds'])
    assert node(inspect(), 'cr:bounces')['toggle'] == 'Off'
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
    # MeasureFirstItem changes retained page geometry, not only a props value.
    def painted_height(color):
        x, y, width, height = node(inspect(), 'cr:host')['bounds']
        pixels = ImageGrab.grab(bbox=tuple(round(v) for v in (x, y, x+width, y+height))).convert('RGB')
        rows = [row for row in range(pixels.height)
                if sum(all(abs(a-b)<4 for a,b in zip(pixels.getpixel((col,row)),color))
                       for col in range(0,pixels.width,4))>20]
        assert rows, 'Carousel card background was not painted'
        return max(rows)-min(rows)+1
    full = painted_height((31,48,82))
    click(node(inspect(), 'cr:size')['bounds'])
    assert node(inspect(), 'cr:size')['toggle'] == 'On'
    measured = painted_height((31,48,82))
    assert measured < full-10, (full, measured)
    assert node(inspect(), 'cr:keep')['toggle'] == 'On'
    inspect('invoke', 'cr:next')
    assert abs(painted_height((61,36,82))-measured)<2
    screenshot('verified-carousel-sizing.png')
    click(node(inspect(), 'cr:size')['bounds'])
    assert abs(painted_height((61,36,82))-full)<2
    inspect('invoke', 'cr:previous')
    assert node(inspect(), 'cr:keep')['toggle'] == 'On'
    print('PASS: real carousel sizing toggle, uniform painted page heights and viewport reset retain state', flush=True)
    click(node(inspect(), 'cr:bounces')['bounds'])
    assert node(inspect(), 'cr:bounces')['toggle'] == 'On'
    assert node(inspect(), 'cr:keep')['toggle'] == 'On'
    print('PASS: real bounce toggle and reset preserve navigation and retained page state', flush=True)
    print('PASS: native carousel arrow, keyboard paging, programmatic jumps, circular Previous/Next, animation switch and retained page state', flush=True)


previous = w.POINT(); u.GetCursorPos(c.byref(previous))
try:
    # Foreground activation may be refused after another test process exits.
    # Keep this test window visible for hit testing, then restore normal z-order.
    assert u.SetWindowPos(hwnd, -1, 0, 0, 0, 0, 0x43)
    time.sleep(.2)
    # A freshly activated WinUI window can still be waiting for its first
    # pointer/foreground transition. Settle activation on the title bar.
    initial = w.RECT(); assert u.GetWindowRect(hwnd, c.byref(initial))
    click([initial.left + 220, initial.top + 6, 120, 20])
    time.sleep(.4)
    if '--swiping-only' in sys.argv:
        verify_swiping()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--hybrid-only' in sys.argv:
        verify_hybrid()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--reordering-only' in sys.argv:
        verify_reordering()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--refresh-host-only' in sys.argv:
        verify_refresh_host()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--grouped-items-only' in sys.argv:
        verify_grouped_items()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--list-refresh-only' in sys.argv:
        verify_list_refresh()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--time-open-only' in sys.argv:
        verify_time_open()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--secure-modes-only' in sys.argv:
        verify_secure_modes()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--clear-button-only' in sys.argv:
        verify_clear_button()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--popup-caption-only' in sys.argv:
        verify_popup_caption()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--input-transform-only' in sys.argv:
        verify_input_transform()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
    if '--row-retention-only' in sys.argv:
        verify_row_retention()
        u.PostMessageW(hwnd, 0x0010, 0, 0)
        sys.exit(0)
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
    if '--symbols-only' in sys.argv:
        verify_symbols()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--window-buttons-only' in sys.argv:
        verify_window_buttons()
        click(node(inspect(), 'wc:buttons:close')['bounds'])
        sys.exit(0)
    if '--html-labels-only' in sys.argv:
        verify_html_labels()
        verify_label_alignment()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--split-roles-only' in sys.argv:
        verify_split_roles()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--return-keys-only' in sys.argv:
        verify_return_keys()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--button-breaks-only' in sys.argv:
        verify_button_breaks()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--tables-only' in sys.argv:
        verify_tables()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--input-visuals-only' in sys.argv:
        verify_input_visuals()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    if '--button-modes-only' in sys.argv:
        verify_button_modes()
        assert u.PostMessageW(hwnd, 0x10, 0, 0)
        sys.exit(0)
    nodes = inspect()
    divider = next(n for n in nodes if n['name'] == 'Resize panes')
    click(divider['bounds'])
    x, y, width, height = divider['bounds']
    u.mouse_event(0x8001, round((x + width/2)*65535/(u.GetSystemMetrics(0)-1)),
                  round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
    time.sleep(.2)
    u.mouse_event(2, 0, 0, 0, 0)
    time.sleep(.2)
    for offset in range(10, 81, 10):
        u.mouse_event(0x8001, round((x + width/2 + offset)*65535/(u.GetSystemMetrics(0)-1)), round((y + height/2)*65535/(u.GetSystemMetrics(1)-1)), 0, 0)
        time.sleep(.08)
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
    verify_button_modes()

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

    verify_input_visuals()
    verify_tables()
    verify_symbols()
    verify_window_buttons()
    verify_html_labels()
    verify_split_roles()
    verify_return_keys()
    verify_button_breaks()
    verify_label_alignment()
    verify_carousel()
    verify_menus()
    verify_tree_rows()
    verify_row_retention()
    verify_input_transform()
    verify_popup_caption()
    verify_grouped_items()
    verify_reordering()
    verify_hybrid()
    verify_swiping()
    verify_list_refresh()
    verify_time_open()
    verify_secure_modes()
    verify_clear_button()

    verify_refresh_host()

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
    u.SetWindowPos(hwnd, -2, 0, 0, 0, 0, 0x13)
    u.SetCursorPos(previous.x, previous.y)
