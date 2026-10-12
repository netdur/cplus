"""facet_runtime on WinUI, driven from outside; build examples/facet_runtime_winui first.

A close that the user starts is a WM_CLOSE from outside, the same request a
caption button or Alt+F4 makes, so that is what these checks post.
"""
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import subprocess
import time

REPO = Path(__file__).resolve().parents[1]
# The staged build by default; CPLUS_RUNTIME_WINUI_DIR names another (a plain
# `cpc build` output, which runs on the installed Windows App Runtime).
RUNTIME = Path(os.environ.get('CPLUS_RUNTIME_WINUI_DIR', REPO / 'examples/facet_runtime_winui/out/runtime'))
EXE = RUNTIME / os.environ.get('CPLUS_RUNTIME_WINUI_EXE', 'facet_runtime_winui.exe')
u = c.WinDLL('user32', use_last_error=True)
u.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]
u.FindWindowW.restype = w.HWND
u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
u.IsWindow.argtypes = [w.HWND]
u.GetForegroundWindow.restype = w.HWND
u.GetDpiForWindow.argtypes = [w.HWND]
u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
WM_CLOSE = 0x0010
UIA = REPO / 'tools/winui_uia.ps1'
# Physical pixels, as winui_uia.ps1's `rect` answers, for the cursor.
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))


def uia(action, hwnd, key):
    result = subprocess.run(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', str(UIA),
                             action, str(hwnd), key], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def shows(hwnd, key, seconds=10):
    end = time.time() + seconds
    while time.time() < end:
        if uia('find', hwnd, key) == '1':
            return True
        time.sleep(.2)
    return False


def read(log):
    return log.read_text(encoding='utf-8', errors='replace')


def wait_for(log, marker, process, seconds=30):
    end = time.time() + seconds
    while time.time() < end:
        content = read(log)
        if marker in content:
            return content
        if process.poll() is not None:
            raise AssertionError(f'exited {process.returncode} before {marker!r}:\n{content}')
        time.sleep(.05)
    raise AssertionError(f'timed out waiting for {marker!r}:\n{read(log)}')


def window(title, process, seconds=15):
    end = time.time() + seconds
    while time.time() < end:
        hwnd = u.FindWindowW(None, title)
        if hwnd:
            pid = w.DWORD()
            u.GetWindowThreadProcessId(hwnd, c.byref(pid))
            if pid.value == process.pid:
                return hwnd
        time.sleep(.05)
    raise AssertionError(f'no window titled {title!r}')


def launch(log, probe):
    env = {**os.environ, 'CPLUS_RUNTIME_WINUI_PROBE': probe}
    output = log.open('w', encoding='utf-8')
    process = subprocess.Popen([str(EXE)], cwd=RUNTIME, env=env, stdout=output,
                               stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
    return process, output


def run_close_question():
    """Closing the last window asks on_should_quit; another window does not."""
    log = RUNTIME / 'close_question.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        main = window('WinUI runtime', process)

        # Not the last window: it closes and the app is not asked.
        assert u.PostMessageW(settings, WM_CLOSE, 0, 0)
        end = time.time() + 10
        while u.IsWindow(settings) and time.time() < end:
            time.sleep(.05)
        assert not u.IsWindow(settings), 'settings did not close'
        time.sleep(.5)
        assert 'PROBE should_quit' not in read(log), 'a non-last window asked on_should_quit'

        # The last window: asked, refused, still up and still running.
        assert u.PostMessageW(main, WM_CLOSE, 0, 0)
        wait_for(log, 'PROBE should_quit 1', process, 10)
        time.sleep(.5)
        assert u.IsWindow(main), 'the refused close still closed the window'
        assert process.poll() is None, 'the refused close ended the run'
        assert 'PROBE quit' not in read(log)

        # Asked again; allowed this time, the run ends cleanly.
        assert u.PostMessageW(main, WM_CLOSE, 0, 0)
        wait_for(log, 'PROBE should_quit 2', process, 10)
        assert process.wait(15) == 0, read(log)
        content = read(log)
        assert 'PROBE quit' in content
        assert content.count('PROBE should_quit') == 2, content
        assert 'outlived the run' not in content, content
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: close question (non-last window not asked, refusal holds, allow ends the run)')


def activate(hwnd, key):
    assert uia('focus', hwnd, key) == 'focused'
    end = time.time() + 5
    while u.GetForegroundWindow() != hwnd and time.time() < end:
        time.sleep(.05)
    assert u.GetForegroundWindow() == hwnd, f'could not activate {key}'
    time.sleep(.3)


def run_alert_window():
    """An alert opens over the window whose control asked, not the active one."""
    log = RUNTIME / 'alert_window.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        main = window('WinUI runtime', process)
        assert shows(settings, 'settings:alert-later')
        # Settings asks for an alert 1.5 s from now (the press activates it);
        # main is made the active window before it fires.
        assert uia('invoke', settings, 'settings:alert-later') == 'invoked'
        activate(main, 'home:prompt')
        assert shows(settings, 'alert:title'), 'the alert is not on the window that asked'
        assert uia('find', main, 'alert:title') == '0', 'the alert went to the active window'
        assert uia('invoke', settings, 'alert:primary') == 'invoked'
        end = time.time() + 10
        while uia('find', settings, 'alert:title') == '1' and time.time() < end:
            time.sleep(.2)
        assert uia('find', settings, 'alert:title') == '0', 'the alert did not answer'
        # An immediate alert from a press goes to the pressed window.
        activate(main, 'home:prompt')
        assert uia('invoke', settings, 'settings:alert') == 'invoked'
        assert shows(settings, 'alert:title')
        assert uia('find', main, 'alert:title') == '0'
        assert uia('invoke', settings, 'alert:primary') == 'invoked'
        # NEGATIVE: the main window's own button keeps it on main.
        assert uia('invoke', main, 'home:alert') == 'invoked'
        assert shows(main, 'alert:title')
        assert uia('find', settings, 'alert:title') == '0'
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: alert window (a deferred alert stays on the window that asked; each window keeps its own)')


def run_worker_hop():
    """A job's worker hands its result back through run_on_main."""
    log = RUNTIME / 'worker_hop.log'
    process, output = launch(log, '0')
    try:
        wait_for(log, 'READY: facet_winui', process)
        main = window('WinUI runtime', process)
        assert shows(main, 'home:load')
        # Twice: the press writes "Loading...", so the second answer is a new hop.
        for round in (1, 2):
            assert uia('invoke', main, 'home:load') == 'invoked'
            end = time.time() + 10
            name = ''
            while time.time() < end:
                name = uia('name', main, 'home:loaded')
                if name.startswith('Loaded'):
                    break
                time.sleep(.2)
            assert name == 'Loaded 42 off the UI thread', f'round {round}: {name!r}\n{read(log)}'
            assert process.poll() is None, read(log)
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: worker hop (run_job runs off the UI thread and applies on it, twice)')


def run_density():
    """Density reads the window's real scale; a resize is not a density change.

    A real change needs a move between monitors of different scale, which one
    display cannot give: WinUI ignores a synthetic WM_DPICHANGED.
    """
    log = RUNTIME / 'density.log'
    process, output = launch(log, '1')
    try:
        content = wait_for(log, 'PROBE density read', process)
        wait_for(log, 'PROBE settings observed', process)
        main = window('WinUI runtime', process)
        expected = u.GetDpiForWindow(main) / 96
        read_value = float(content.split('PROBE density read ')[1].split()[0])
        assert abs(read_value - expected) < 1e-6, (read_value, expected)
        # Resizing raises XamlRoot.Changed with the scale unchanged: neither
        # the app-wide nor Settings' own density observer may fire.
        for width in (900, 700, 800):
            assert u.SetWindowPos(main, None, 60, 60, width, 560, 0x0014)
            time.sleep(.4)
        assert process.poll() is None, read(log)
        content = read(log)
        assert 'PROBE density 1' not in content and 'PROBE density 2' not in content, content
        assert 'PROBE settings density' not in content, content
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print(f'PASS: density (reads {expected}; resizing reports no density change)')


def chord(*keys):
    """Press keys together, as a person does: into the foreground window."""
    for key in keys:
        u.keybd_event(key, 0, 0, 0)
    for key in reversed(keys):
        u.keybd_event(key, 0, 2, 0)


def type_text(value):
    for ch in value.upper():
        chord(ord(ch))


def clipboard(value=None):
    """Read the clipboard's text, or replace it with `value` first."""
    k = c.WinDLL('kernel32')
    k.GlobalAlloc.restype = c.c_void_p
    k.GlobalLock.restype = c.c_void_p
    k.GlobalLock.argtypes = k.GlobalUnlock.argtypes = [c.c_void_p]
    u.GetClipboardData.restype = c.c_void_p
    u.SetClipboardData.argtypes = [w.UINT, c.c_void_p]
    assert u.OpenClipboard(None)
    try:
        if value is not None:
            data = (value + '\0').encode('utf-16-le')
            handle = k.GlobalAlloc(2, len(data))
            c.memmove(k.GlobalLock(handle), data, len(data))
            k.GlobalUnlock(handle)
            u.EmptyClipboard()
            u.SetClipboardData(13, handle)
            return value
        handle = u.GetClipboardData(13)
        if not handle:
            return None
        text = c.wstring_at(k.GlobalLock(handle))
        k.GlobalUnlock(handle)
        return text
    finally:
        u.CloseClipboard()


def wait_until(check, seconds=10):
    end = time.time() + seconds
    while time.time() < end:
        if check():
            return True
        time.sleep(.1)
    return False


def run_app_menu():
    """App::menu is a MenuBar in every window: clicks, live enabled state,
    Ctrl shortcuts, the edit actions on the text field that had the keyboard,
    and the quit token through on_should_quit."""
    log = RUNTIME / 'app_menu.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        main = window('WinUI runtime', process)
        for hwnd in (main, settings):
            assert shows(hwnd, 'menu:File') and shows(hwnd, 'menu:Edit')

        # A click, and an item whose enabled state follows the app.
        assert uia('expand', main, 'menu:Window') == 'expanded'
        assert uia('enabled', main, 'menu:Window:Close Settings') == 'True'
        assert uia('invoke', main, 'menu:Window:Close Settings') == 'invoked'
        assert wait_until(lambda: not u.IsWindow(settings)), 'Close Settings did not close it'
        assert uia('expand', main, 'menu:Window') == 'expanded'
        assert uia('enabled', main, 'menu:Window:Close Settings') == 'False', 'stale enabled state'
        chord(0x1B)

        # A shortcut with the menu closed: Ctrl+, opens Settings again.
        activate(main, 'home:prompt')
        chord(0x11, 0xBC)
        reopened = window('Settings', process)   # raises when Ctrl+, did nothing
        u.PostMessageW(reopened, WM_CLOSE, 0, 0)
        assert wait_until(lambda: not u.IsWindow(reopened))

        # Edit > Select All, Edit > Copy from the menu act on the field that
        # had the keyboard, although opening the menu took it.
        activate(main, 'home:prompt')
        assert uia('invoke', main, 'home:prompt') == 'invoked'
        assert shows(main, 'prompt:value')
        assert uia('focus', main, 'prompt:value') == 'focused'
        type_text('hello')
        assert wait_until(lambda: uia('value', main, 'prompt:value') == 'hello'), uia('value', main, 'prompt:value')
        clipboard('sentinel')
        for item in ('Select All', 'Copy'):
            assert uia('expand', main, 'menu:Edit') == 'expanded'
            assert uia('invoke', main, f'menu:Edit:{item}') == 'invoked'
            time.sleep(.3)
        assert wait_until(lambda: clipboard() == 'hello'), f'clipboard {clipboard()!r}'
        assert uia('invoke', main, 'prompt:cancel') == 'invoked'

        # The quit token asks on_should_quit (refused once), then Ctrl+Q ends the run.
        assert uia('expand', main, 'menu:File') == 'expanded'
        assert uia('invoke', main, 'menu:File:Quit') == 'invoked'
        wait_for(log, 'PROBE should_quit 1', process, 10)
        time.sleep(.5)
        assert process.poll() is None, 'a refused quit ended the run'
        activate(main, 'home:prompt')
        chord(0x11, ord('Q'))
        wait_for(log, 'PROBE should_quit 2', process, 10)
        assert process.wait(15) == 0, read(log)
        assert 'outlived the run' not in read(log), read(log)
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: app menu (every window; click, live enabled state, Ctrl shortcut, edit actions, quit)')


def window_of(nodes, key):
    """The id of the `window` node a described node belongs to."""
    index = next(i for i, n in enumerate(nodes) if n['id'] == key)
    while index is not None:
        if nodes[index]['role'] == 'window':
            return nodes[index]['id']
        index = nodes[index].get('parent')
    return None


def run_agent_windows():
    """One agent connection reaches every window, each named as registered."""
    import http.client
    import json
    log = RUNTIME / 'agent_windows.log'
    env = {**os.environ, 'CPLUS_RUNTIME_WINUI_PROBE': '1', 'CPLUS_RUNTIME_WINUI_AGENT': '1'}
    output = log.open('w', encoding='utf-8')
    process = subprocess.Popen([str(EXE)], cwd=RUNTIME, env=env, stdout=output,
                               stderr=subprocess.STDOUT, creationflags=subprocess.CREATE_NO_WINDOW)
    conn = None

    def rpc(method, params=None):
        conn.request('POST', '/', json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method,
                                              'params': params or {}}).encode(),
                     {'Content-Type': 'application/json'})
        value = json.loads(conn.getresponse().read())
        assert 'error' not in value, value
        return value['result']

    def described(key):
        end = time.time() + 10
        while time.time() < end:
            nodes = rpc('describe_ui', {'mode': 'full'})
            if any(n['id'] == key for n in nodes):
                return nodes
            time.sleep(.2)
        raise AssertionError(f'{key!r} never described')

    try:
        content = wait_for(log, 'AGENT PORT', process)
        port = int(content.split('AGENT PORT ')[1].split()[0])
        wait_for(log, 'READY: facet_winui', process)
        wait_for(log, 'facet_agent_winui: MCP on', process)
        settings = window('Settings', process)
        assert shows(settings, 'settings:alert')
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
        rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                           'clientInfo': {'name': 'operator', 'version': '1'}})

        nodes = described('settings:alert')
        windows = [n['id'] for n in nodes if n['role'] == 'window']
        assert sorted(windows) == ['main', 'settings'], windows
        assert window_of(nodes, 'home:open-settings') == 'main'
        assert window_of(nodes, 'settings:alert') == 'settings'

        # A control in the window opened second answers, and its sheet is
        # described under that window.
        assert rpc('click', {'id': 'settings:alert'})['outcome'] == 'allowed'
        nodes = described('alert:title')
        assert window_of(nodes, 'alert:title') == 'settings'
        assert rpc('click', {'id': 'alert:primary'})['outcome'] == 'allowed'

        # NEGATIVE: once Settings closes its controls are gone, and the first
        # window is still served on the same connection.
        u.PostMessageW(settings, WM_CLOSE, 0, 0)
        assert wait_until(lambda: not u.IsWindow(settings))
        nodes = rpc('describe_ui', {'mode': 'full'})
        assert [n['id'] for n in nodes if n['role'] == 'window'] == ['main'], nodes
        assert rpc('click', {'id': 'settings:alert'})['outcome'] == 'not_found'
        # And a window opened later joins.
        assert rpc('click', {'id': 'home:open-settings'})['outcome'] == 'allowed'
        nodes = described('settings:alert')
        assert window_of(nodes, 'settings:alert') == 'settings', 'a reopened window is not served'
    finally:
        if conn is not None:
            conn.close()
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: agent windows (every window on one connection, named as registered; a closed one leaves)')


def run_gestures():
    """The gesture band on a plain container, driven by the real mouse and
    keyboard: hover, click, double click, right click, long press, swipe,
    unhover, the key band, and the focus and blur commands with their
    handlers and is_focused."""
    log = RUNTIME / 'gestures.log'
    process, output = launch(log, '1')
    try:
        wait_for(log, 'READY: facet_winui', process)
        settings = window('Settings', process)
        # A container has no automation peer: find the pad by its title label
        # and aim at the empty space under it, which only a hit-testable
        # container answers.
        assert shows(settings, 'settings:pad-title')
        activate(settings, 'settings:focus-pad')
        scale = u.GetDpiForWindow(settings) / 96
        tx, ty, tw, th = (int(v) for v in uia('rect', settings, 'settings:pad-title').split())
        x, y = tx - round(8 * scale), ty - round(8 * scale)
        width, height = round(300 * scale), round(70 * scale)
        cx, cy = x + width // 2, ty + th + round(14 * scale)

        def says(expected, seconds=5):
            assert wait_until(lambda: uia('name', settings, 'settings:pad-log') == expected, seconds),                 (expected, uia('name', settings, 'settings:pad-log'))

        def mouse(flags):
            u.mouse_event(flags, 0, 0, 0, 0)

        def move_to(px, py):
            # Through the input queue, as tools/test_facet_winui_pointer.py
            # does: SetCursorPos does not always reach it (remote sessions).
            u.mouse_event(0x8001, round(px * 65535 / (u.GetSystemMetrics(0) - 1)),
                          round(py * 65535 / (u.GetSystemMetrics(1) - 1)), 0, 0)
            time.sleep(.03)

        move_to(cx, cy)
        says('hover')
        mouse(2); mouse(4)                               # left click
        says('click 1')
        time.sleep(.6)                                   # past the double-click time
        mouse(2); mouse(4); time.sleep(.05); mouse(2); mouse(4)
        says('double')
        mouse(8); mouse(16)                              # right click
        says('right')
        mouse(2); time.sleep(1.0); mouse(4)              # held past long_press_seconds
        says('long')
        time.sleep(.6)
        assert uia('name', settings, 'settings:pad-log') == 'long', 'a long press also clicked'
        move_to(x + 20, cy)                       # a drag across the pad
        mouse(2)
        for step in range(1, 13):
            move_to(x + 20 + step * 15, cy); time.sleep(.02)
        mouse(4)
        says('swipe')
        time.sleep(.6)
        assert uia('name', settings, 'settings:pad-log') == 'swipe', 'a swipe also clicked'
        move_to(x + width // 2, y - 40)           # out of the pad
        says('unhover')

        # The focus command takes the keyboard; the node reports it.
        assert uia('invoke', settings, 'settings:focus-pad') == 'invoked'
        says('focus true')
        chord(0x1B)                                      # Escape, to the focused pad
        says('key escape')
        assert uia('invoke', settings, 'settings:blur-pad') == 'invoked'
        says('blur false')
        assert process.poll() is None, read(log)
    finally:
        if process.poll() is None:
            process.kill()
        output.close()
    print('PASS: gestures (hover, click, double, right, long press, swipe, unhover, key band, focus/blur)')


if __name__ == '__main__':
    run_close_question()
    run_alert_window()
    run_worker_hop()
    run_density()
    run_app_menu()
    run_agent_windows()
    run_gestures()
