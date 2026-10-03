"""Native state checks plus real keyboard submission and local WebView navigation."""
import base64
import ctypes as c
from ctypes import wintypes as w
import os
from pathlib import Path
import re
import subprocess
import time
import sys
from test_agent_winui_http import connect, rpc

ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / 'examples/facet_agent_winui_smoke'
OUT = EXAMPLE / 'out'
u = c.WinDLL('user32', use_last_error=True)
u.SetProcessDpiAwarenessContext(c.c_void_p(-4))
u.FindWindowW.restype = w.HWND
u.GetForegroundWindow.restype = w.HWND
u.SetForegroundWindow.argtypes = [w.HWND]
u.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
u.SetWindowPos.argtypes = [w.HWND, w.HWND, c.c_int, c.c_int, c.c_int, c.c_int, w.UINT]
u.WindowFromPoint.argtypes = [w.POINT]
u.WindowFromPoint.restype = w.HWND
u.GetAncestor.argtypes = [w.HWND, w.UINT]
u.GetAncestor.restype = w.HWND
u.GetCursorPos.argtypes = [c.POINTER(w.POINT)]


def wait_for(log, marker, process, seconds=30):
    deadline = time.monotonic() + seconds
    while True:
        contents = log.read_text(encoding='utf-8', errors='replace')
        if marker in contents:
            return contents
        assert process.poll() is None, contents
        assert time.monotonic() < deadline, contents
        time.sleep(.05)


def run():
    env = {**os.environ, 'CPLUS_FACET_PARITY': '1'}
    for name in ('first', 'second', 'third'):
        page = OUT / (name + '.html')
        page.write_text(f'<html><body><h1>{name}</h1></body></html>', encoding='utf-8')
        env['CPLUS_PARITY_' + name.upper()] = page.as_uri()
    runtime = OUT / 'runtime'
    log = OUT / 'parity.log'
    with log.open('w', encoding='utf-8') as output:
        process = subprocess.Popen([str(runtime / 'facet_agent_winui_smoke.exe')],
            cwd=runtime, env=env, stdout=output, stderr=subprocess.STDOUT,
            creationflags=subprocess.CREATE_NO_WINDOW)
        conn = None
        try:
            content = wait_for(log, 'PARITY INPUT READY', process)
            conn = connect(int(re.search(r'PARITY INPUT READY (\d+)', content)[1]))
            wait_for(log, 'PASS: browser history', process)
            hwnd = u.FindWindowW(None, 'Facet WinUI parity')
            assert hwnd
            assert u.SetWindowPos(hwnd, None, 50, 50, 1100, 800, 0x0040)
            u.SetForegroundWindow(hwnd)
            time.sleep(.3)
            assert u.GetForegroundWindow() == hwnd, 'Parity window must be foreground for desktop input'
            origin = w.POINT(0, 0)
            assert u.ClientToScreen(hwnd, c.byref(origin))
            for key, marker in [('list-row-0', 'LIST TAP 1'), ('list-row-0', 'LIST TAP 2'),
                                ('list-row-1', 'LIST TAP 3'),
                                ('grid-row-0', 'GRID SELECT 1'), ('grid-row-1', 'GRID SELECT 2'),
                                ('grid-row-0', 'GRID SELECT 3')]:
                tree = rpc(conn, 'describe_ui', {'mode': 'full'})
                row = next(n for n in tree if n['id'] == key)
                frame = row['frame']
                assert frame['w'] > 0 and frame['h'] > 0, row
                point = w.POINT(origin.x + round(frame['x'] + frame['w']/2),
                                origin.y + round(frame['y'] + frame['h']/2))
                assert u.GetAncestor(u.WindowFromPoint(point), 2) == hwnd, (key, frame, 'Window obscured')
                # Queue movement before the click, as the pointer probe does;
                # cursor warping alone is unreliable in remote desktop sessions.
                u.mouse_event(0x8001, round(point.x * 65535 / (u.GetSystemMetrics(0) - 1)),
                              round(point.y * 65535 / (u.GetSystemMetrics(1) - 1)), 0, 0)
                time.sleep(.1)
                cursor = w.POINT()
                assert u.GetCursorPos(c.byref(cursor))
                assert abs(cursor.x - point.x) <= 1 and abs(cursor.y - point.y) <= 1, (key, point.x, point.y, cursor.x, cursor.y)
                if key.startswith('grid'):
                    u.keybd_event(17, 0, 0, 0)
                try:
                    u.mouse_event(2, 0, 0, 0, 0)
                    time.sleep(.05)  # Allow WinUI to process the pressed state before release.
                    u.mouse_event(4, 0, 0, 0, 0)
                    wait_for(log, marker, process, 5)
                finally:
                    if key.startswith('grid'):
                        u.keybd_event(17, 0, 2, 0)
                time.sleep(.55)  # Distinct taps, not a Windows double-tap gesture.
            for key in ('input', 'search', 'area'):
                assert rpc(conn, 'scroll_to', {'id': key})['outcome'] == 'allowed'
                time.sleep(.2)
                if key == 'area':
                    # Enter alone must remain a newline, not submit.
                    u.keybd_event(13, 0, 0, 0); u.keybd_event(13, 0, 2, 0)
                    time.sleep(.15)
                    assert 'SUBMIT area' not in log.read_text(encoding='utf-8')
                    u.keybd_event(17, 0, 0, 0)
                u.keybd_event(13, 0, 0, 0); u.keybd_event(13, 0, 2, 0)
                if key == 'area':
                    u.keybd_event(17, 0, 2, 0)
                wait_for(log, 'SUBMIT ' + key, process, 5)
            assert process.wait(timeout=30) == 0, log.read_text(encoding='utf-8')
            assert 'PASS: parity teardown' in log.read_text(encoding='utf-8')
            print(log.read_text(encoding='utf-8'))
        finally:
            if conn:
                conn.close()
            if process.poll() is None:
                process.kill(); process.wait()


def run_styles():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_STYLES': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: style/event teardown' in result.stdout


def run_text():
    runtime = OUT / 'runtime'
    animation = OUT / 'animated.gif'
    animation.write_bytes(base64.b64decode('R0lGODlhAgACAIEAAP8AAAAAAAAAAAAAACH/C05FVFNDQVBFMi4wAwEAAAAh+QQACgAAACwAAAAAAgACAAAIBgABCAQQEAAh+QQACgAAACwAAAAAAgACAIEAAP8AAAAAAAAAAAAIBgABCAQQEAA7'))
    fixture = OUT / 'pixel.png'
    fixture.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR3sAAAAASUVORK5CYII='))
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TEXT': '1', 'CPLUS_TEST_IMAGE': str(fixture), 'CPLUS_TEST_ANIMATION': str(animation)},
        capture_output=True, text=True, encoding='utf-8', timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: text/content teardown' in result.stdout


def run_lifecycle():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_LIFECYCLE': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: list lifecycle teardown' in result.stdout


def run_actions():
    runtime = OUT / 'runtime'
    fixture = OUT / 'pixel.png'
    fixture.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR3sAAAAASUVORK5CYII='))
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_ACTIONS': '1', 'CPLUS_TEST_IMAGE': str(fixture)},
        capture_output=True, text=True, encoding='utf-8', timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: action teardown' in result.stdout


def run_date_formats():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_DATE_FORMATS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: date format teardown' in result.stdout


def run_refresh():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_REFRESH': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: refresh teardown' in result.stdout


def run_layout():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_LAYOUT': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: layout teardown' in result.stdout


def run_choice_colors():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CHOICE_COLORS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: choice color teardown' in result.stdout


def run_canvas_redraw():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_CANVAS_REDRAW': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: canvas redraw teardown' in result.stdout


def run_borders():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_BORDERS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: border teardown' in result.stdout


def run_tree_rows():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TREE_ROWS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: tree row teardown' in result.stdout


def run_menus():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_MENUS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: menu teardown' in result.stdout


def run_tabs():
    runtime = OUT / 'runtime'
    result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
        env={**os.environ, 'CPLUS_FACET_TABS': '1'}, capture_output=True,
        text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
    print(result.stdout)
    assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
    assert 'PASS: tabs teardown' in result.stdout


def run_paging():
    runtime = OUT / 'runtime'
    for extra in ({}, {'CPLUS_FACET_PAGING_CURRENT': '1'}):
        result = subprocess.run([str(runtime / 'facet_agent_winui_smoke.exe')], cwd=runtime,
            env={**os.environ, 'CPLUS_FACET_PAGING': '1', **extra}, capture_output=True,
            text=True, encoding='utf-8', timeout=30, creationflags=subprocess.CREATE_NO_WINDOW)
        print(result.stdout)
        assert result.returncode == 0, f"exit={result.returncode}\n{result.stdout}{result.stderr}"
        assert 'PASS: carousel teardown' in result.stdout


if __name__ == '__main__':
    if '--tree-rows-only' in sys.argv:
        run_tree_rows()
        sys.exit(0)
    if '--menus-only' in sys.argv:
        run_menus()
        sys.exit(0)
    if '--paging-only' in sys.argv:
        run_paging()
        sys.exit(0)
    if '--tabs-only' in sys.argv:
        run_tabs()
        sys.exit(0)
    if '--borders-only' in sys.argv:
        run_borders()
        sys.exit(0)
    if '--canvas-redraw-only' in sys.argv:
        run_canvas_redraw()
        sys.exit(0)
    if '--choice-colors-only' in sys.argv:
        run_choice_colors()
        sys.exit(0)
    if '--layout-only' in sys.argv:
        run_layout()
        sys.exit(0)
    if '--refresh-only' in sys.argv:
        run_refresh()
        sys.exit(0)
    if '--date-formats-only' in sys.argv:
        run_date_formats()
        sys.exit(0)
    if '--actions-only' in sys.argv:
        run_actions()
        sys.exit(0)
    if '--lifecycle-only' in sys.argv:
        run_lifecycle()
        sys.exit(0)
    if '--styles-only' not in sys.argv and '--text-only' not in sys.argv:
        run()
    if '--text-only' not in sys.argv:
        run_styles()
    run_text()
    if len(sys.argv) == 1:
        run_lifecycle()
        run_actions()
        run_date_formats()
        run_refresh()
        run_layout()
        run_choice_colors()
        run_canvas_redraw()
        run_borders()

        run_tabs()

        run_paging()

        run_menus()

        run_tree_rows()
